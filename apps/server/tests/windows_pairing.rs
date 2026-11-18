#![cfg(windows)]
#[allow(dead_code)] // This shared fixture also offers server-restart checks used by other suites.
mod common;
use common::TestServer;
use dagaz_windows::{ClientError, ServerConnection};

#[tokio::test]
async fn windows_credentials_pair_and_restore_against_real_server() {
    let server = TestServer::spawn().await;
    let code = dagaz_server::new_pairing_code(&server.db).await.unwrap();
    let client = ServerConnection::pair(server.url.parse().unwrap(), &code, "Windows integration")
        .await
        .unwrap();
    client.check_access().await.unwrap();
    let profile = tempfile::tempdir().unwrap();
    client.save(profile.path()).unwrap();
    client.save(profile.path()).unwrap();
    let restored = ServerConnection::restore(profile.path()).unwrap().unwrap();
    assert_eq!(client.device_id(), restored.device_id());
    assert_eq!(client.base_url(), restored.base_url());
    restored.check_access().await.unwrap();
    let revoke = server
        .http
        .delete(format!(
            "{}/api/v1/devices/{}",
            server.url,
            client.device_id()
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 204);
    assert!(matches!(
        restored.check_access().await,
        Err(ClientError::PairingRequired)
    ));
    server.finish().await;
}
