mod common;
use common::TestServer;
use dagaz_protocol::{ApiError, Device, Page, PairRequest, PairResponse, Platform};

fn request(code: &str) -> PairRequest {
    PairRequest {
        pairing_code: code.into(),
        device_name: "Тестовый телефон".into(),
        platform: Platform::Android,
    }
}

#[tokio::test]
async fn auth_rejects_missing_revoked_and_reused_pairing_code() {
    let mut server = TestServer::spawn().await;
    let devices = format!("{}/api/v1/devices", server.url);
    let anonymous = server.http.get(&devices).send().await.unwrap();
    assert_eq!(anonymous.status(), 401);
    assert_eq!(
        anonymous.json::<ApiError>().await.unwrap().code,
        "unauthorized"
    );
    let code = dagaz_server::new_pairing_code(&server.db).await.unwrap();
    let pair_url = format!("{}/api/v1/pair", server.url);
    let (a, b) = tokio::join!(
        server.http.post(&pair_url).json(&request(&code)).send(),
        server.http.post(&pair_url).json(&request(&code)).send()
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        [a.status(), b.status()]
            .into_iter()
            .filter(|status| status.is_success())
            .count(),
        1
    );
    let paired: PairResponse = if a.status().is_success() {
        a.json().await.unwrap()
    } else {
        b.json().await.unwrap()
    };
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM devices WHERE id=?")
        .bind(paired.device_id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!stored.contains(&paired.token));
    assert_eq!(
        server
            .http
            .post(&pair_url)
            .json(&request(&code))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let expired = dagaz_server::new_pairing_code(&server.db).await.unwrap();
    sqlx::query("UPDATE pairing_codes SET expires_at=0 WHERE used_at IS NULL")
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .post(&pair_url)
            .json(&request(&expired))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let list = server
        .http
        .get(&devices)
        .bearer_auth(&paired.token)
        .send()
        .await
        .unwrap()
        .json::<Page<Device>>()
        .await
        .unwrap();
    assert!(list.items.iter().any(|device| device.id == paired.device_id && device.device_name == "Тестовый телефон"));
    assert_eq!(
        server
            .http
            .delete(format!("{devices}/{}", paired.device_id))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        server
            .http
            .get(&devices)
            .bearer_auth(&paired.token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    server.restart().await;
    assert_eq!(
        server
            .http
            .get(format!("{}/api/v1/devices", server.url))
            .bearer_auth(&paired.token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        server
            .http
            .get(format!("{}/api/v1/devices", server.url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        server
            .http
            .post(format!("{}/api/v1/pair", server.url))
            .json(&request(&code))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    server.finish().await;
}

#[tokio::test]
async fn enforces_pairing_and_device_rate_limits() {
    let server = TestServer::spawn().await;
    let pair = format!("{}/api/v1/pair", server.url);
    // Bootstrap consumed one of the IP's ten attempts.
    for _ in 0..9 {
        assert_eq!(
            server
                .http
                .post(&pair)
                .json(&request("incorrect"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let response = server
        .http
        .post(&pair)
        .json(&request("incorrect"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert!(response.headers().contains_key("retry-after"));
    let devices = format!("{}/api/v1/devices", server.url);
    for _ in 0..300 {
        assert_eq!(
            server
                .http
                .get(&devices)
                .bearer_auth(&server.token)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert_eq!(
        server
            .http
            .get(&devices)
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    server.finish().await;
}

#[tokio::test]
async fn validates_inputs_and_reports_unavailable_storage_without_secrets() {
    let server = TestServer::spawn().await;
    let code = dagaz_server::new_pairing_code(&server.db).await.unwrap();
    let mut invalid = request(&code);
    invalid.device_name = "\r\n".into();
    let pair = format!("{}/api/v1/pair", server.url);
    let response = server.http.post(&pair).json(&invalid).send().await.unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<ApiError>().await.unwrap().code,
        "invalid_request"
    );
    assert_eq!(
        server
            .http
            .post(&pair)
            .json(&request(&code))
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "invalid input must not consume pairing code"
    );
    let response = server
        .http
        .get(format!("{}/api/v1/devices?limit=0", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<ApiError>().await.unwrap().code,
        "invalid_request"
    );
    let first = server
        .http
        .get(format!("{}/api/v1/devices?limit=1", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json::<Page<Device>>()
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = server
        .http
        .get(format!(
            "{}/api/v1/devices?limit=1&cursor={}",
            server.url,
            first.next_cursor.unwrap()
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json::<Page<Device>>()
        .await
        .unwrap();
    assert_ne!(first.items[0].id, second.items[0].id);
    assert!(second.next_cursor.is_none());
    let mut config = server.config.clone();
    config.s3_secret_key = "incorrect-secret-never-include-in-errors".into();
    let s3 = dagaz_server::create_s3(&config).unwrap();
    let error = dagaz_server::build_app(config, server.db.clone(), s3)
        .await
        .expect_err("wrong credentials must reject startup");
    assert!(!error.to_string().contains("incorrect-secret"));
    let mut config = server.config.clone();
    config.s3_endpoint = "ftp://invalid".into();
    assert!(dagaz_server::create_s3(&config).is_err());
    let mut config = server.config.clone();
    config.data_dir = server.config.data_dir.join("not-a-directory");
    std::fs::write(&config.data_dir, b"keep this file").unwrap();
    assert!(dagaz_server::open_database(&config).await.is_err());
    assert_eq!(std::fs::read(&config.data_dir).unwrap(), b"keep this file");
    server.finish().await;
}
