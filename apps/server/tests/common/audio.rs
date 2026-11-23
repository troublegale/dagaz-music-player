#![allow(dead_code)]
use super::TestServer;
use dagaz_protocol::{AudioFormat, Job, JobStatus, Quality, Track, UploadState};
use sha2::{Digest, Sha256};
use uuid::Uuid;
pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audio")
            .join(name),
    )
    .unwrap()
}
pub async fn seed(server: &TestServer, name: &str) -> Track {
    seed_bytes(server, name, fixture(name)).await
}
pub async fn seed_bytes(server: &TestServer, name: &str, bytes: Vec<u8>) -> Track {
    use lofty::{
        file::{AudioFile, TaggedFileExt},
        tag::Accessor,
    };
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join(name.rsplit('/').next().unwrap());
    std::fs::write(&file, &bytes).unwrap();
    let audio = lofty::read_from_path(file).unwrap();
    let tag = audio.primary_tag();
    let id = Uuid::new_v4();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let key = format!("tracks/{id}/1/original");
    let object = server
        .s3
        .put_object()
        .bucket(&server.config.s3_bucket)
        .key(&key)
        .metadata("sha256", &sha)
        .body(aws_sdk_s3::primitives::ByteStream::from(bytes.clone()))
        .send()
        .await
        .unwrap();
    let track = Track {
        id,
        revision: 1,
        filename: name.into(),
        title: tag
            .and_then(|tag| tag.title())
            .map(|value| value.into_owned()),
        artist: None,
        artist_id: None,
        album: None,
        album_id: None,
        album_artist: None,
        track_number: None,
        disc_number: None,
        duration_ms: audio.properties().duration().as_millis() as i64,
        format: if name.ends_with("mp3") {
            AudioFormat::Mp3
        } else if name.ends_with("flac") {
            AudioFormat::Flac
        } else {
            AudioFormat::Wav
        },
        artwork_id: None,
        original_bytes: bytes.len() as i64,
        ready_qualities: vec![Quality::Original],
    };
    sqlx::query(
        "INSERT INTO tracks(id,revision,filename,data_json,catalog_ready) VALUES(?,1,?,?,1)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(serde_json::to_string(&track).unwrap())
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO media_revisions(track_id,revision,quality,object_key,byte_length,sha256,etag,storage_etag,created_at) VALUES(?,1,'original',?,?,?,?,?,0)").bind(id.to_string()).bind(key).bind(bytes.len() as i64).bind(&sha).bind(format!("\"{sha}\"")).bind(object.e_tag().unwrap()).execute(&server.db).await.unwrap();
    track
}
pub async fn finished(server: &TestServer, id: Uuid) -> Job {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let job: Job = server
            .http
            .get(format!("{}/api/v1/jobs/{id}", server.url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if matches!(
            job.status,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Partial
        ) {
            return job;
        }
        assert!(std::time::Instant::now() < until, "{job:?}");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}
pub async fn import(server: &TestServer, name: &str) -> Track {
    let bytes = fixture(name);
    import_bytes(server, name, bytes).await
}
pub async fn import_bytes(server: &TestServer, name: &str, bytes: Vec<u8>) -> Track {
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let upload: UploadState = server
        .http
        .post(format!("{}/api/v1/uploads", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&serde_json::json!({"filename":name,"total_bytes":bytes.len(),"sha256":sha}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .patch(format!("{}/api/v1/uploads/{}", server.url, upload.id))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .header("Content-Type", "application/octet-stream")
            .header("Upload-Offset", "0")
            .header("X-Chunk-SHA256", sha)
            .body(bytes)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let job: Job = server
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
    let done = finished(server, job.id).await;
    assert!(matches!(done.status, JobStatus::Succeeded));
    server
        .http
        .get(format!(
            "{}/api/v1/tracks/{}",
            server.url,
            done.result.unwrap()["track_id"].as_str().unwrap()
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
