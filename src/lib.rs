use std::{
    env,
    fs::Permissions,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use tokio::fs;

const DEFAULT_API_BASE: &str = "https://api.elevenlabs.io";
const DEFAULT_MODEL: &str = "scribe_v2";
const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct AdapterState {
    client: reqwest::Client,
    credentials_path: PathBuf,
    api_base: String,
    model: String,
    no_verbatim: bool,
}

impl AdapterState {
    pub fn from_environment() -> Result<Self> {
        let credentials_path = credentials_path()?;
        let api_base =
            env::var("VOXTYPE_ELEVENLABS_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_owned());
        let model =
            env::var("VOXTYPE_ELEVENLABS_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_owned());
        let no_verbatim = env_bool("VOXTYPE_ELEVENLABS_NO_VERBATIM", false)?;

        Self::new(credentials_path, api_base, model, no_verbatim)
    }

    pub fn new(
        credentials_path: PathBuf,
        api_base: String,
        model: String,
        no_verbatim: bool,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(concat!(
                "voxtype-elevenlabs-adapter/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("failed to create HTTP client")?;

        Ok(Self {
            client,
            credentials_path,
            api_base: api_base.trim_end_matches('/').to_owned(),
            model,
            no_verbatim,
        })
    }

    async fn api_key(&self) -> Result<String> {
        if let Ok(key) = env::var("ELEVENLABS_API_KEY") {
            let key = key.trim().to_owned();
            if !key.is_empty() {
                return Ok(key);
            }
        }

        let key = fs::read_to_string(&self.credentials_path)
            .await
            .with_context(|| {
                format!(
                    "ElevenLabs API key is not configured; run `voxtype-elevenlabs-adapter set-key` (expected {})",
                    self.credentials_path.display()
                )
            })?;
        let key = key.trim().to_owned();
        if key.is_empty() {
            bail!("ElevenLabs API key file is empty; run `voxtype-elevenlabs-adapter set-key`");
        }
        Ok(key)
    }

    async fn is_configured(&self) -> bool {
        self.api_key().await.is_ok()
    }
}

pub fn app(state: AdapterState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/audio/transcriptions", post(transcribe))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state)
}

#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub configured: bool,
    pub model: String,
}

async fn health(State(state): State<AdapterState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        configured: state.is_configured().await,
        model: state.model,
    })
}

#[derive(Serialize)]
struct VoxtypeResponse {
    text: String,
}

#[derive(Deserialize)]
struct ElevenLabsResponse {
    text: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

struct Upload {
    bytes: Vec<u8>,
    filename: String,
    content_type: String,
    language: Option<String>,
}

async fn transcribe(State(state): State<AdapterState>, mut multipart: Multipart) -> Response {
    let upload = match read_upload(&mut multipart).await {
        Ok(upload) => upload,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, error.to_string()),
    };

    let api_key = match state.api_key().await {
        Ok(key) => key,
        Err(error) => return json_error(StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
    };

    let audio_size = upload.bytes.len();
    let part = match Part::bytes(upload.bytes)
        .file_name(upload.filename)
        .mime_str(&upload.content_type)
    {
        Ok(part) => part,
        Err(error) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid audio content type: {error}"),
            );
        }
    };

    let mut form = Form::new()
        .part("file", part)
        .text("model_id", state.model.clone())
        .text("tag_audio_events", "false")
        .text("diarize", "false")
        .text("timestamps_granularity", "none")
        .text("no_verbatim", state.no_verbatim.to_string());

    if let Some(language) = upload.language.filter(|value| value != "auto") {
        form = form.text("language_code", language);
    }

    tracing::info!(audio_bytes = audio_size, "sending audio to ElevenLabs");
    let started = Instant::now();
    let response = match state
        .client
        .post(format!("{}/v1/speech-to-text", state.api_base))
        .header("xi-api-key", &api_key)
        .multipart(form)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, "ElevenLabs request failed");
            return json_error(
                StatusCode::BAD_GATEWAY,
                "could not reach ElevenLabs; check the adapter service log".to_owned(),
            );
        }
    };

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let detail = sanitize_error(&body, &api_key);
        tracing::warn!(upstream_status = %status, "ElevenLabs rejected transcription");
        return json_error(
            StatusCode::BAD_GATEWAY,
            format!("ElevenLabs returned {status}: {detail}"),
        );
    }

    let result: ElevenLabsResponse = match response.json().await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(error = %error, "invalid ElevenLabs response");
            return json_error(
                StatusCode::BAD_GATEWAY,
                "ElevenLabs returned an invalid response".to_owned(),
            );
        }
    };

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        "transcription complete"
    );
    (StatusCode::OK, Json(VoxtypeResponse { text: result.text })).into_response()
}

async fn read_upload(multipart: &mut Multipart) -> Result<Upload> {
    let mut audio: Option<(Vec<u8>, String, String)> = None;
    let mut language = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .context("invalid multipart request")?
    {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "file" => {
                let filename = field.file_name().unwrap_or("audio.wav").to_owned();
                let content_type = field.content_type().unwrap_or("audio/wav").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .context("could not read uploaded audio")?
                    .to_vec();
                audio = Some((bytes, filename, content_type));
            }
            "language" => {
                let value = field
                    .text()
                    .await
                    .context("could not read language field")?;
                language = Some(value);
            }
            _ => {}
        }
    }

    let (bytes, filename, content_type) = audio.context("multipart field `file` is required")?;
    if bytes.is_empty() {
        bail!("uploaded audio is empty");
    }

    Ok(Upload {
        bytes,
        filename,
        content_type,
        language,
    })
}

fn json_error(status: StatusCode, error: String) -> Response {
    (status, Json(ErrorResponse { error })).into_response()
}

fn sanitize_error(body: &str, api_key: &str) -> String {
    let redacted = body.replace(api_key, "[redacted]");
    let compact = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "no error detail".to_owned()
    } else {
        compact.chars().take(500).collect()
    }
}

fn env_bool(name: &str, default: bool) -> Result<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => bail!("{name} must be true or false"),
        },
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

pub fn credentials_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("VOXTYPE_ELEVENLABS_CREDENTIALS") {
        return Ok(PathBuf::from(path));
    }

    let config_home = match env::var("XDG_CONFIG_HOME") {
        Ok(path) => PathBuf::from(path),
        Err(_) => PathBuf::from(env::var("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(config_home
        .join("voxtype-elevenlabs-adapter")
        .join("api-key"))
}

pub fn write_api_key(path: &Path, key: &str) -> Result<()> {
    use std::io::Write;

    let key = key.trim();
    if key.is_empty() {
        bail!("API key cannot be empty");
    }

    let parent = path.parent().context("credentials path has no parent")?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    std::fs::set_permissions(parent, Permissions::from_mode(0o700))?;

    let temporary = parent.join(format!(".api-key.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(key.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
