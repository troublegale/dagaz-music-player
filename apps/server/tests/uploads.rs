mod common;
use common::TestServer;
use dagaz_protocol::{AudioFormat, Job, JobStatus, Track, UploadCreate, UploadState, UploadStatus};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audio")
            .join(name),
    )
    .unwrap()
}
async fn create(server: &TestServer, name: &str, bytes: &[u8], key: Uuid) -> UploadState {
    let response = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&UploadCreate {
            filename: name.into(),
            total_bytes: bytes.len() as i64,
            sha256: digest(bytes),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response.json().await.unwrap()
}
async fn patch(
    server: &TestServer,
    id: Uuid,
    offset: i64,
    bytes: &[u8],
    key: Uuid,
) -> reqwest::Response {
    server
        .http
        .patch(format!("{}/api/v1/uploads/{id}", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .header("Upload-Offset", offset.to_string())
        .header("X-Chunk-SHA256", digest(bytes))
        .header("Content-Type", "application/octet-stream")
        .body(bytes.to_vec())
        .send()
        .await
        .unwrap()
}
async fn complete(server: &TestServer, id: Uuid, key: Uuid) -> Job {
    let response = server
        .http
        .post(format!("{}/api/v1/uploads/{id}/complete", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    response.json().await.unwrap()
}
async fn finished(server: &TestServer, id: Uuid) -> Job {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let response = server
            .http
            .get(format!("{}/api/v1/jobs/{id}", server.url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let job: Job = response.json().await.unwrap();
        if matches!(job.status, JobStatus::Succeeded | JobStatus::Failed) {
            return job;
        }
        assert!(Instant::now() < deadline, "import job did not finish");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
async fn import(server: &TestServer, name: &str, bytes: &[u8]) -> Uuid {
    let upload = create(server, name, bytes, Uuid::new_v4()).await;
    assert_eq!(
        patch(server, upload.id, 0, bytes, Uuid::new_v4())
            .await
            .status(),
        200
    );
    let job = complete(server, upload.id, Uuid::new_v4()).await;
    let done = finished(server, job.id).await;
    assert!(
        matches!(done.status, JobStatus::Succeeded),
        "import failed: {:?}",
        done.errors
    );
    done.result.unwrap()["track_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn upload_resume_duplicate_and_failed_import() {
    let mut server = TestServer::spawn().await;
    let bytes = fixture("pcm.wav");
    let create_key = Uuid::new_v4();
    let (upload, repeated) = tokio::join!(
        create(&server, "../песня.mp3", &bytes, create_key),
        create(&server, "../песня.mp3", &bytes, create_key)
    );
    assert_eq!(upload.id, repeated.id);
    let first = bytes.len() / 2;
    let chunk_key = Uuid::new_v4();
    let (a, b) = tokio::join!(
        patch(&server, upload.id, 0, &bytes[..first], chunk_key),
        patch(&server, upload.id, 0, &bytes[..first], Uuid::new_v4())
    );
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    let repeated = patch(&server, upload.id, 0, &bytes[..first], chunk_key).await;
    assert_eq!(repeated.status(), 200);
    assert_eq!(
        repeated.json::<UploadState>().await.unwrap().offset,
        first as i64
    );
    assert_eq!(
        patch(&server, upload.id, 1, &bytes[..first], chunk_key)
            .await
            .status(),
        409
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        0
    );
    let mut staging = std::fs::OpenOptions::new()
        .append(true)
        .open(
            server
                .config
                .data_dir
                .join("staging")
                .join(upload.id.to_string()),
        )
        .unwrap();
    staging
        .write_all(b"uncommitted bytes after a crash")
        .unwrap();
    drop(staging);
    server.restart().await;
    let state = server
        .http
        .get(format!("{}/api/v1/uploads/{}", server.url, upload.id))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json::<UploadState>()
        .await
        .unwrap();
    assert_eq!(state.offset, first as i64);
    assert_eq!(
        patch(
            &server,
            upload.id,
            first as i64,
            &bytes[first..],
            Uuid::new_v4()
        )
        .await
        .status(),
        200
    );
    let complete_key = Uuid::new_v4();
    let job = complete(&server, upload.id, complete_key).await;
    assert_eq!(complete(&server, upload.id, complete_key).await.id, job.id);
    let done = finished(&server, job.id).await;
    assert!(matches!(done.status, JobStatus::Succeeded));
    let id: Uuid = done.result.unwrap()["track_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        complete(&server, upload.id, Uuid::new_v4()).await.id,
        job.id
    );
    let payload: String = sqlx::query_scalar("SELECT data_json FROM tracks WHERE id=?")
        .bind(id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    let track: Track = serde_json::from_str(&payload).unwrap();
    assert!(matches!(track.format, AudioFormat::Wav));
    assert_eq!(track.filename, "../песня.mp3");
    let key: String = sqlx::query_scalar("SELECT object_key FROM media_revisions WHERE track_id=?")
        .bind(id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!key.contains("песня"));
    let stored = server
        .s3
        .get_object()
        .bucket(&server.config.s3_bucket)
        .key(key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(stored.as_ref(), bytes);
    assert_eq!(import(&server, "renamed.wav", &bytes).await, id);
    let flac = import(&server, "same-name", &fixture("tone.flac")).await;
    let mp3 = import(&server, "same-name", &fixture("vbr.mp3")).await;
    assert_ne!(flac, mp3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        3
    );
    server.restart().await;
    let preserved = server
        .s3
        .head_object()
        .bucket(&server.config.s3_bucket)
        .key(format!("tracks/{id}/1/original"))
        .send()
        .await
        .unwrap();
    assert_eq!(preserved.content_length(), Some(bytes.len() as i64));
    server.finish().await;
}

fn large_wav() -> Vec<u8> {
    let length = 9 * 1024 * 1024u32;
    let mut bytes = vec![0; 44 + length as usize];
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(&(length + 36).to_le_bytes());
    bytes[8..16].copy_from_slice(b"WAVEfmt ");
    bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&1u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&1u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&44100u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&88200u32.to_le_bytes());
    bytes[32..34].copy_from_slice(&2u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&16u16.to_le_bytes());
    bytes[36..40].copy_from_slice(b"data");
    bytes[40..44].copy_from_slice(&length.to_le_bytes());
    bytes
}

#[tokio::test]
async fn recovers_multipart_and_unpublished_ready_object() {
    use aws_sdk_s3::primitives::ByteStream;
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let bytes = large_wav();
    let upload = create(&server, "large.wav", &bytes, Uuid::new_v4()).await;
    // Probe rejection before sending the forbidden body; a writing client can see
    // an OS connection reset when HTTP/1 closes an oversized upload early.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let address = server.url.strip_prefix("http://").unwrap();
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!(
            "PATCH /api/v1/uploads/{} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nIdempotency-Key: {}\r\nUpload-Offset: 0\r\nX-Chunk-SHA256: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            upload.id,
            server.token,
            Uuid::new_v4(),
            "0".repeat(64),
            bytes.len()
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 413"));
    }
    let first = 8 * 1024 * 1024;
    assert_eq!(
        patch(&server, upload.id, 0, &bytes[..first], Uuid::new_v4())
            .await
            .status(),
        200
    );
    assert_eq!(
        patch(
            &server,
            upload.id,
            first as i64,
            &bytes[first..],
            Uuid::new_v4()
        )
        .await
        .status(),
        200
    );
    let job = complete(&server, upload.id, Uuid::new_v4()).await;
    let candidate: String = sqlx::query_scalar("SELECT candidate_track_id FROM jobs WHERE id=?")
        .bind(job.id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    let object_key = format!("tracks/{candidate}/1/original");
    let created = server
        .s3
        .create_multipart_upload()
        .bucket(&server.config.s3_bucket)
        .key(&object_key)
        .metadata("sha256", digest(&bytes))
        .send()
        .await
        .unwrap();
    let multipart = created.upload_id().unwrap();
    server
        .s3
        .upload_part()
        .bucket(&server.config.s3_bucket)
        .key(&object_key)
        .upload_id(multipart)
        .part_number(1)
        .body(ByteStream::from(bytes[..first].to_vec()))
        .send()
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET status='running',s3_upload_id=? WHERE id=?")
        .bind(multipart)
        .bind(job.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    server.restart().await;
    assert!(matches!(
        finished(&server, job.id).await.status,
        JobStatus::Succeeded
    ));
    let stored = server
        .s3
        .get_object()
        .bucket(&server.config.s3_bucket)
        .key(&object_key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(stored.as_ref(), bytes);
    assert!(
        server
            .s3
            .list_multipart_uploads()
            .bucket(&server.config.s3_bucket)
            .send()
            .await
            .unwrap()
            .uploads()
            .is_empty()
    );

    server.pause_jobs().await;
    let bytes = fixture("tone.flac");
    let upload = create(&server, "ready.flac", &bytes, Uuid::new_v4()).await;
    assert_eq!(
        patch(&server, upload.id, 0, &bytes, Uuid::new_v4())
            .await
            .status(),
        200
    );
    let job = complete(&server, upload.id, Uuid::new_v4()).await;
    let candidate: String = sqlx::query_scalar("SELECT candidate_track_id FROM jobs WHERE id=?")
        .bind(job.id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    let object_key = format!("tracks/{candidate}/1/original");
    let published = server
        .s3
        .put_object()
        .bucket(&server.config.s3_bucket)
        .key(&object_key)
        .metadata("sha256", digest(&bytes))
        .body(ByteStream::from(bytes))
        .send()
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET status='running' WHERE id=?")
        .bind(job.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    server.restart().await;
    assert!(matches!(
        finished(&server, job.id).await.status,
        JobStatus::Succeeded
    ));
    let storage_etag: String =
        sqlx::query_scalar("SELECT storage_etag FROM media_revisions WHERE track_id=?")
            .bind(candidate)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(Some(storage_etag.as_str()), published.e_tag());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        2
    );
    server.finish().await;
}

#[tokio::test]
async fn rejects_corruption_expiry_conflicts_and_insufficient_space() {
    let mut server = TestServer::spawn().await;
    let bytes = fixture("pcm.wav");
    for (filename, total, status) in [
        ("bad\r\nHeader.wav", bytes.len() as i64, 400),
        (
            "too-big.wav",
            server.config.max_upload_bytes as i64 + 1,
            413,
        ),
    ] {
        let response = server
            .http
            .post(format!("{}/api/v1/uploads", server.url))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&UploadCreate {
                filename: filename.into(),
                total_bytes: total,
                sha256: digest(&bytes),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let key = Uuid::new_v4();
    let upload = create(&server, "song.wav", &bytes, key).await;
    let conflict = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&UploadCreate {
            filename: "other.wav".into(),
            total_bytes: bytes.len() as i64,
            sha256: digest(&bytes),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    let bad = server
        .http
        .patch(format!("{}/api/v1/uploads/{}", server.url, upload.id))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .header("Upload-Offset", "0")
        .header("X-Chunk-SHA256", "0".repeat(64))
        .header("Content-Type", "application/octet-stream")
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    let response = server
        .http
        .post(format!(
            "{}/api/v1/uploads/{}/complete",
            server.url, upload.id
        ))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    sqlx::query("UPDATE uploads SET expires_at=0 WHERE id=?")
        .bind(upload.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    let state = server
        .http
        .get(format!("{}/api/v1/uploads/{}", server.url, upload.id))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json::<UploadState>()
        .await
        .unwrap();
    assert!(matches!(state.status, UploadStatus::Expired));
    assert_eq!(
        patch(&server, upload.id, 0, &bytes, Uuid::new_v4())
            .await
            .status(),
        410
    );
    let corrupt = b"not an audio file";
    let upload = create(&server, "looks-like.mp3", corrupt, Uuid::new_v4()).await;
    assert_eq!(
        patch(&server, upload.id, 0, corrupt, Uuid::new_v4())
            .await
            .status(),
        200
    );
    let job = complete(&server, upload.id, Uuid::new_v4()).await;
    assert!(matches!(
        finished(&server, job.id).await.status,
        JobStatus::Failed
    ));
    let response = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&UploadCreate {
            filename: "bad-hash.wav".into(),
            total_bytes: bytes.len() as i64,
            sha256: "0".repeat(64),
        })
        .send()
        .await
        .unwrap();
    let upload: UploadState = response.json().await.unwrap();
    assert_eq!(
        patch(&server, upload.id, 0, &bytes, Uuid::new_v4())
            .await
            .status(),
        200
    );
    let job = complete(&server, upload.id, Uuid::new_v4()).await;
    assert!(matches!(
        finished(&server, job.id).await.status,
        JobStatus::Failed
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        0
    );
    server.config.min_free_bytes = u64::MAX;
    server.restart().await;
    let response = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&UploadCreate {
            filename: "full.wav".into(),
            total_bytes: bytes.len() as i64,
            sha256: digest(&bytes),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 507);
    server.finish().await;
}
