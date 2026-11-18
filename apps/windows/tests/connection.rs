#![cfg(windows)]
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use dagaz_protocol::{Device, Page, PairRequest, PairResponse, Platform};
use dagaz_windows::{ClientError, InstanceGuard, ServerConnection};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

const TOKEN: &str = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
#[derive(Clone)]
struct Mock {
    used: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
    id: Uuid,
}
async fn pair(
    State(state): State<Mock>,
    Json(request): Json<PairRequest>,
) -> Result<(StatusCode, Json<PairResponse>), StatusCode> {
    if request.pairing_code != "one-time-code" || state.used.swap(true, Ordering::SeqCst) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    assert!(matches!(request.platform, Platform::Windows));
    Ok((
        StatusCode::CREATED,
        Json(PairResponse {
            device_id: state.id,
            token: TOKEN.into(),
        }),
    ))
}
async fn devices(
    State(state): State<Mock>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Page<Device>>, StatusCode> {
    if state.revoked.load(Ordering::SeqCst)
        || headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            != Some(&format!("Bearer {TOKEN}"))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(Page {
        items: Vec::new(),
        next_cursor: None,
    }))
}
#[tokio::test]
async fn pairing_and_single_instance() {
    let state = Mock {
        used: Arc::new(AtomicBool::new(false)),
        revoked: Arc::new(AtomicBool::new(false)),
        id: Uuid::new_v4(),
    };
    let app = Router::new()
        .route("/api/v1/pair", post(pair))
        .route("/api/v1/devices", get(devices))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert!(matches!(
        ServerConnection::pair(url.clone(), "wrong", "PC").await,
        Err(ClientError::PairingRequired)
    ));
    let connection = ServerConnection::pair(url.clone(), "one-time-code", "PC")
        .await
        .unwrap();
    assert_eq!(connection.device_id(), state.id);
    connection.check_access().await.unwrap();
    assert!(matches!(
        ServerConnection::pair(url, "one-time-code", "PC").await,
        Err(ClientError::PairingRequired)
    ));
    let directory = tempfile::tempdir().unwrap();
    connection.save(directory.path()).unwrap();
    let settings = std::fs::read_to_string(directory.path().join("connection.json")).unwrap();
    assert!(!settings.contains(TOKEN));
    let restored = ServerConnection::restore(directory.path())
        .unwrap()
        .unwrap();
    assert_eq!(restored.device_id(), state.id);
    restored.check_access().await.unwrap();
    state.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        restored.check_access().await,
        Err(ClientError::PairingRequired)
    ));
    let guard = InstanceGuard::acquire(directory.path()).unwrap();
    assert!(matches!(
        InstanceGuard::acquire(directory.path()),
        Err(ClientError::AlreadyRunning)
    ));
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "instance_subprocess"])
        .env("DAGAZ_TEST_PROFILE", directory.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    drop(guard);
    let _guard = InstanceGuard::acquire(directory.path()).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&settings).unwrap();
    value["encrypted_token"] = serde_json::json!([1, 2, 3]);
    std::fs::write(directory.path().join("connection.json"), value.to_string()).unwrap();
    assert!(matches!(
        ServerConnection::restore(directory.path()),
        Err(ClientError::PairingRequired)
    ));
    task.abort();
}
#[test]
#[ignore = "child process of pairing_and_single_instance"]
fn instance_subprocess() {
    let profile = std::env::var_os("DAGAZ_TEST_PROFILE").unwrap();
    assert!(matches!(
        InstanceGuard::acquire(std::path::Path::new(&profile)),
        Err(ClientError::AlreadyRunning)
    ));
}
#[tokio::test]
async fn rejects_unsafe_origins_and_does_not_follow_redirects() {
    for url in [
        "http://music.example",
        "https://name:password@music.example",
        "https://music.example/path",
        "https://music.example/?token=secret",
        "https://music.example/#fragment",
    ] {
        assert!(matches!(
            ServerConnection::pair(url.parse().unwrap(), "code", "PC").await,
            Err(ClientError::InvalidAddress)
        ));
    }
    let received = Arc::new(AtomicBool::new(false));
    let flag = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/api/v1/devices", listener.local_addr().unwrap());
    let sink = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move || {
                let flag = flag.clone();
                async move {
                    flag.store(true, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .await
        .unwrap();
    });
    let redirect = axum::response::Redirect::temporary(&target);
    let app = Router::new()
        .route(
            "/api/v1/pair",
            post(|| async {
                (
                    StatusCode::CREATED,
                    Json(PairResponse {
                        device_id: Uuid::new_v4(),
                        token: TOKEN.into(),
                    }),
                )
            }),
        )
        .route(
            "/api/v1/devices",
            get(move || {
                let location = redirect.clone();
                async move { location }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let source = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let connection = ServerConnection::pair(url.parse().unwrap(), "code", "PC")
        .await
        .unwrap();
    assert!(connection.check_access().await.is_err());
    assert!(!received.load(Ordering::SeqCst));
    source.abort();
    sink.abort();
}
