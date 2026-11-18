mod common;
use aws_sdk_s3::primitives::ByteStream;
use common::TestServer;
use dagaz_protocol::{AudioFormat, Quality, Track};
use sha2::{Digest, Sha256};
use uuid::Uuid;

async fn revision(server: &TestServer, id: Uuid, number: i64, bytes: &[u8]) -> String {
    let key = format!("tracks/{id}/{number}/original");
    let sha = format!("{:x}", Sha256::digest(bytes));
    let etag = format!("\"{sha}\"");
    let stored = server
        .s3
        .put_object()
        .bucket(&server.config.s3_bucket)
        .key(&key)
        .metadata("sha256", &sha)
        .body(ByteStream::from(bytes.to_vec()))
        .send()
        .await
        .unwrap();
    let track = Track {
        id,
        revision: number,
        filename: "..\\bad\r\nX-Injected: yes.mp3".into(),
        title: None,
        artist: None,
        artist_id: None,
        album_id: None,
        album: None,
        album_artist: None,
        track_number: None,
        disc_number: None,
        duration_ms: 1000,
        format: AudioFormat::Mp3,
        artwork_id: None,
        original_bytes: bytes.len() as i64,
        ready_qualities: vec![Quality::Original],
    };
    sqlx::query("INSERT INTO tracks(id,revision,filename,data_json) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,data_json=excluded.data_json")
        .bind(id.to_string()).bind(number).bind(&track.filename).bind(serde_json::to_string(&track).unwrap()).execute(&server.db).await.unwrap();
    sqlx::query("INSERT INTO media_revisions(track_id,revision,quality,object_key,byte_length,sha256,etag,storage_etag,created_at) VALUES(?,?,'original',?,?,?,?,?,0)")
        .bind(id.to_string()).bind(number).bind(key).bind(bytes.len() as i64).bind(sha).bind(&etag).bind(stored.e_tag().unwrap()).execute(&server.db).await.unwrap();
    etag
}

#[tokio::test]
async fn range_if_range_and_revision_change() {
    let mut server = TestServer::spawn().await;
    let id = Uuid::new_v4();
    let etag = revision(&server, id, 1, b"abcdefghij").await;
    server.restart().await;
    let old = format!(
        "{}/api/v1/tracks/{id}/media?revision=1&quality=original",
        server.url
    );
    assert_eq!(server.http.get(&old).send().await.unwrap().status(), 401);
    let full = server
        .http
        .get(&old)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(full.status(), 200);
    assert_eq!(full.headers()["etag"], etag);
    assert_eq!(full.headers()["content-length"], "10");
    assert_eq!(full.headers()["content-type"], "audio/mpeg");
    assert!(!full.headers().contains_key("x-injected"));
    assert_eq!(full.bytes().await.unwrap().as_ref(), b"abcdefghij");
    for (range, expected, content_range) in [
        ("bytes=2-5", b"cdef".as_slice(), "bytes 2-5/10"),
        ("bytes=-2", b"ij".as_slice(), "bytes 8-9/10"),
        ("bytes=8-", b"ij".as_slice(), "bytes 8-9/10"),
    ] {
        let response = server
            .http
            .get(&old)
            .bearer_auth(&server.token)
            .header("Range", range)
            .header("If-Range", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 206);
        assert_eq!(response.headers()["content-range"], content_range);
        assert_eq!(response.bytes().await.unwrap().as_ref(), expected);
    }
    let head = server
        .http
        .head(&old)
        .bearer_auth(&server.token)
        .header("Range", "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 206);
    assert_eq!(head.headers()["content-length"], "4");
    assert!(head.bytes().await.unwrap().is_empty());
    let invalid = server
        .http
        .get(&old)
        .bearer_auth(&server.token)
        .header("Range", "bytes=10-")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 416);
    assert_eq!(invalid.headers()["content-range"], "bytes */10");
    let mismatch = server
        .http
        .get(&old)
        .bearer_auth(&server.token)
        .header("Range", "bytes=2-5")
        .header("If-Range", "\"different\"")
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status(), 200);
    assert_eq!(mismatch.bytes().await.unwrap().as_ref(), b"abcdefghij");
    let changed = revision(&server, id, 2, b"1234567890").await;
    assert_ne!(changed, etag);
    let original = server
        .http
        .get(&old)
        .bearer_auth(&server.token)
        .header("Range", "bytes=2-5")
        .header("If-Range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(original.status(), 206);
    assert_eq!(original.bytes().await.unwrap().as_ref(), b"cdef");
    let new_url = format!(
        "{}/api/v1/tracks/{id}/media?revision=2&quality=original",
        server.url
    );
    let mismatch = server
        .http
        .get(&new_url)
        .bearer_auth(&server.token)
        .header("Range", "bytes=2-5")
        .header("If-Range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status(), 200);
    assert_eq!(mismatch.bytes().await.unwrap().as_ref(), b"1234567890");
    let failed_match = server
        .http
        .get(&new_url)
        .bearer_auth(&server.token)
        .header("If-Match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(failed_match.status(), 412);
    sqlx::query("UPDATE media_revisions SET superseded_at=0 WHERE track_id=? AND revision=1")
        .bind(id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .get(&old)
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    assert_eq!(
        server
            .http
            .get(format!(
                "{}/api/v1/tracks/{id}/media?revision=2&quality=high",
                server.url
            ))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        425
    );
    server.finish().await;
}

#[tokio::test]
async fn stream_permits_and_revision_leases_follow_response_lifetime() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let server = TestServer::spawn().await;
    let id = Uuid::new_v4();
    let etag = revision(&server, id, 1, b"abcdefghij").await;
    let state =
        dagaz_server::open_state(server.config.clone(), server.db.clone(), server.s3.clone())
            .await
            .unwrap();
    let descriptor = dagaz_server::media_descriptor(&state, id, 1, Quality::Original)
        .await
        .unwrap();
    assert_eq!(descriptor.etag, etag);
    assert_eq!(descriptor.byte_length, 10);
    let app = dagaz_server::router(state.clone());
    let request = |method: &str| {
        Request::builder()
            .method(method)
            .uri(&descriptor.url)
            .header("Authorization", format!("Bearer {}", server.token))
            .body(Body::empty())
            .unwrap()
    };
    let mut held = Vec::new();
    for _ in 0..16 {
        let response = app.clone().oneshot(request("GET")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(state.active_media_reads().unwrap(), 16);
    assert_eq!(
        app.clone().oneshot(request("GET")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let head = app.clone().oneshot(request("HEAD")).await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        state.active_media_reads().unwrap(),
        16,
        "HEAD must not open a media stream"
    );
    drop(held.pop());
    assert_eq!(state.active_media_reads().unwrap(), 15);
    held.clear();
    assert_eq!(
        state.active_media_reads().unwrap(),
        0,
        "cancelled bodies leaked revision leases"
    );
    let response = app.clone().oneshot(request("GET")).await.unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap().as_ref(),
        b"abcdefghij"
    );
    assert_eq!(
        state.active_media_reads().unwrap(),
        0,
        "completed body leaked a revision lease"
    );
    server
        .s3
        .delete_object()
        .bucket(&server.config.s3_bucket)
        .key(format!("tracks/{id}/1/original"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        app.oneshot(request("GET")).await.unwrap().status(),
        StatusCode::GONE
    );
    assert_eq!(
        state.active_media_reads().unwrap(),
        0,
        "failed upstream leaked a revision lease"
    );
    drop(state);
    server.finish().await;
}

#[tokio::test]
async fn imported_original_downloads_byte_for_byte_through_authenticated_media() {
    use dagaz_protocol::{Job, JobStatus, UploadCreate, UploadState};
    let server = TestServer::spawn().await;
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/tone.flac"),
    )
    .unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let response = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&UploadCreate {
            filename: "Музыка.flac".into(),
            total_bytes: bytes.len() as i64,
            sha256: sha.clone(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let upload: UploadState = response.json().await.unwrap();
    assert_eq!(
        server
            .http
            .patch(format!("{}/api/v1/uploads/{}", server.url, upload.id))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .header("Upload-Offset", "0")
            .header("X-Chunk-SHA256", &sha)
            .header("Content-Type", "application/octet-stream")
            .body(bytes.clone())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let queued: Job = server
        .http
        .post(format!(
            "{}/api/v1/uploads/{}/complete",
            server.url, upload.id
        ))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let id = loop {
        let job: Job = server
            .http
            .get(format!("{}/api/v1/jobs/{}", server.url, queued.id))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!matches!(job.status, JobStatus::Failed));
        if matches!(job.status, JobStatus::Succeeded) {
            break job.result.unwrap()["track_id"].as_str().unwrap().to_owned();
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let response = server
        .http
        .get(format!(
            "{}/api/v1/tracks/{id}/media?revision=1&quality=original",
            server.url
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "audio/flac");
    assert_eq!(response.headers()["x-content-sha256"], sha);
    assert_eq!(response.bytes().await.unwrap().as_ref(), bytes);
    server.finish().await;
}
