use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use voxtype_elevenlabs_adapter::{AdapterState, app, credentials_path, write_api_key};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the local compatibility server.
    Serve {
        #[arg(long, default_value = "127.0.0.1:17811")]
        bind: SocketAddr,
    },
    /// Securely save the ElevenLabs API key.
    SetKey {
        #[arg(long)]
        credentials: Option<PathBuf>,
    },
    /// Check whether the running adapter is ready.
    Status {
        #[arg(long, default_value = "http://127.0.0.1:17811")]
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    match Cli::parse().command.unwrap_or(Command::Serve {
        bind: "127.0.0.1:17811".parse().expect("valid default address"),
    }) {
        Command::Serve { bind } => serve(bind).await,
        Command::SetKey { credentials } => set_key(credentials),
        Command::Status { url } => status(&url).await,
    }
}

async fn serve(bind: SocketAddr) -> Result<()> {
    if !bind.ip().is_loopback() {
        bail!("refusing to listen outside localhost: {bind}");
    }

    let state = AdapterState::from_environment()?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not listen on {bind}"))?;
    tracing::info!(address = %bind, "adapter ready");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("adapter server stopped unexpectedly")
}

fn set_key(credentials: Option<PathBuf>) -> Result<()> {
    let path = match credentials {
        Some(path) => path,
        None => credentials_path()?,
    };
    let key = rpassword::prompt_password("ElevenLabs API key: ")?;
    write_api_key(&path, &key)?;
    println!("API key saved securely to {}", path.display());
    Ok(())
}

async fn status(url: &str) -> Result<()> {
    let response = reqwest::get(format!("{}/health", url.trim_end_matches('/')))
        .await
        .context("adapter is not reachable")?
        .error_for_status()
        .context("adapter health check failed")?;
    let health: serde_json::Value = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&health)?);
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
