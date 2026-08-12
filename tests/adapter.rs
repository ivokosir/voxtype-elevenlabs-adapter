use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tempfile::tempdir;
use tower::ServiceExt;
use voxtype_elevenlabs_adapter::{AdapterState, app, write_api_key};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

fn multipart_request(audio: &str) -> Request<Body> {
    let boundary = "adapter-test-boundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n\
         {audio}\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"model\"\r\n\r\n\
         whisper-1\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"language\"\r\n\r\n\
         en\r\n\
         --{boundary}--\r\n"
    );

    Request::builder()
        .method("POST")
        .uri("/v1/audio/transcriptions")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn translates_voxtype_request_to_elevenlabs() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .and(header("xi-api-key", "test-key"))
        .and(body_string_contains("scribe_v2"))
        .and(body_string_contains("audio/wav"))
        .and(body_string_contains("language_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "language_code": "en",
            "text": "Hello from ElevenLabs."
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    let directory = tempdir().unwrap();
    let credentials = directory.path().join("api-key");
    write_api_key(&credentials, "test-key").unwrap();
    let state =
        AdapterState::new(credentials, upstream.uri(), "scribe_v2".to_owned(), false).unwrap();

    let response = app(state)
        .oneshot(multipart_request("fake-wave-data"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"text": "Hello from ElevenLabs."})
    );
}

#[tokio::test]
async fn reports_missing_api_key_without_contacting_upstream() {
    let upstream = MockServer::start().await;
    let directory = tempdir().unwrap();
    let state = AdapterState::new(
        directory.path().join("missing-key"),
        upstream.uri(),
        "scribe_v2".to_owned(),
        false,
    )
    .unwrap();

    let response = app(state)
        .oneshot(multipart_request("fake-wave-data"))
        .await
        .unwrap();
    assert_eq!(response.status(), 503);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("set-key"));
}

#[test]
fn stores_api_key_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().unwrap();
    let credentials = directory.path().join("private").join("api-key");
    write_api_key(&credentials, "secret-value").unwrap();

    assert_eq!(
        std::fs::read_to_string(&credentials).unwrap(),
        "secret-value"
    );
    assert_eq!(
        std::fs::metadata(&credentials)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
