mod common;
use common::{TestServer, audio::*};
use dagaz_protocol::{ApiError, JobStatus};
use uuid::Uuid;

async fn request(server: &TestServer, id: Uuid, quality: &str) -> reqwest::Response {
    server
        .http
        .get(format!(
            "{}/api/v1/tracks/{id}/media?revision=1&quality={quality}",
            server.url
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
}
#[tokio::test]
async fn quality_generation_is_deduplicated_and_reuses_small_mp3() {
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let source = seed(&server, "profiles/low.mp3").await;
    let (first, second) = tokio::join!(
        request(&server, source.id, "high"),
        request(&server, source.id, "high")
    );
    assert_eq!(first.status(), 425);
    assert_eq!(second.status(), 425);
    let first: ApiError = first.json().await.unwrap();
    let second: ApiError = second.json().await.unwrap();
    assert!(first.job_id.is_some());
    assert_eq!(first.job_id, second.job_id);
    let head = server
        .http
        .head(format!(
            "{}/api/v1/tracks/{}/media?revision=1&quality=high",
            server.url, source.id
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 425);
    assert_eq!(
        head.headers().get("x-job-id").unwrap().to_str().unwrap(),
        first.job_id.unwrap().to_string()
    );
    server.restart().await;
    assert!(matches!(
        finished(&server, first.job_id.unwrap()).await.status,
        JobStatus::Succeeded
    ));
    for quality in ["high", "low"] {
        let mut response = request(&server, source.id, quality).await;
        if response.status() == 425 {
            let pending: ApiError = response.json().await.unwrap();
            assert!(matches!(
                finished(&server, pending.job_id.unwrap()).await.status,
                JobStatus::Succeeded
            ));
            response = request(&server, source.id, quality).await;
        }
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["x-effective-quality"], "original");
        assert_eq!(response.bytes().await.unwrap(), fixture("profiles/low.mp3"));
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_revisions WHERE track_id=?")
        .bind(source.id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    server.finish().await;
}
#[tokio::test]
async fn derived_mp3_has_correct_headers_tags_and_persistent_job() {
    use lofty::{
        file::{AudioFile, TaggedFileExt},
        tag::Accessor,
    };
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    use lofty::{
        config::WriteOptions,
        picture::{Picture, PictureType},
    };
    let source_dir = tempfile::tempdir().unwrap();
    let path = source_dir.path().join("tone.flac");
    std::fs::write(&path, fixture("tone.flac")).unwrap();
    let cover = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/artwork/cover.png"),
    )
    .unwrap();
    let mut tagged = lofty::read_from_path(&path).unwrap();
    let mut picture = Picture::from_reader(&mut std::io::Cursor::new(&cover)).unwrap();
    picture.set_pic_type(PictureType::CoverFront);
    tagged.primary_tag_mut().unwrap().push_picture(picture);
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();
    let source = seed_bytes(&server, "tone.flac", std::fs::read(path).unwrap()).await;
    let pending = request(&server, source.id, "low").await;
    assert_eq!(pending.status(), 425);
    let pending: ApiError = pending.json().await.unwrap();
    server.restart().await;
    let done = finished(&server, pending.job_id.unwrap()).await;
    assert!(matches!(done.status, JobStatus::Succeeded), "{done:?}");
    let response = request(&server, source.id, "low").await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "audio/mpeg");
    assert_eq!(response.headers()["x-effective-quality"], "low");
    let bytes = response.bytes().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("low.mp3");
    std::fs::write(&file, &bytes).unwrap();
    let probe = std::process::Command::new(&server.config.ffprobe_path)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(probe.status.success());
    let bitrate = String::from_utf8(probe.stdout)
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    let audio = lofty::read_from_path(file).unwrap();
    assert_eq!(
        audio.primary_tag().unwrap().title().as_deref(),
        Some("Keep")
    );
    assert_eq!(audio.primary_tag().unwrap().pictures()[0].data(), cover);
    assert!(
        bitrate <= 128000,
        "FFprobe={bitrate}, Lofty estimate={:?}",
        audio.properties().audio_bitrate()
    );
    server.restart().await;
    assert_eq!(
        request(&server, source.id, "low")
            .await
            .bytes()
            .await
            .unwrap(),
        bytes
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quality_jobs WHERE track_id=? AND revision=1 AND profile='low'",
    )
    .bind(source.id.to_string())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    server.finish().await;
}
#[tokio::test]
async fn import_enqueues_both_profiles_without_waiting_for_them() {
    let server = TestServer::spawn().await;
    let source = import(&server, "pcm.wav").await;
    let jobs: Vec<String> = sqlx::query_scalar("SELECT job_id FROM quality_jobs WHERE track_id=?")
        .bind(source.id.to_string())
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 2);
    for id in jobs {
        assert!(matches!(
            finished(&server, id.parse().unwrap()).await.status,
            JobStatus::Succeeded
        ));
    }
    assert_eq!(
        request(&server, source.id, "original")
            .await
            .bytes()
            .await
            .unwrap(),
        fixture("pcm.wav")
    );
    server.finish().await;
}

#[tokio::test]
async fn timeout_cleans_partial_and_does_not_block_other_quality() {
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let source = seed(&server, "pcm.wav").await;
    server.config.media_timeout = std::time::Duration::from_millis(1);
    server.restart().await;
    let pending: ApiError = request(&server, source.id, "low")
        .await
        .json()
        .await
        .unwrap();
    let done = finished(&server, pending.job_id.unwrap()).await;
    assert!(matches!(done.status, JobStatus::Failed));
    assert_eq!(done.errors[0].code, "media_timeout");
    assert_eq!(request(&server, source.id, "low").await.status(), 422);
    assert_eq!(request(&server, source.id, "original").await.status(), 200);
    assert!(
        std::fs::read_dir(server.config.data_dir.join("staging"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("transcode-"))
    );
    server.config.media_timeout = std::time::Duration::from_secs(30);
    server.restart().await;
    let pending: ApiError = request(&server, source.id, "high")
        .await
        .json()
        .await
        .unwrap();
    assert!(matches!(
        finished(&server, pending.job_id.unwrap()).await.status,
        JobStatus::Succeeded
    ));
    server.finish().await;
}

#[tokio::test]
async fn missing_encoder_retries_the_same_durable_job_after_restart() {
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let source = seed(&server, "pcm.wav").await;
    let encoder = server.config.ffmpeg_path.clone();
    server.config.ffmpeg_path = server.config.data_dir.join("missing-ffmpeg.exe");
    server.restart().await;
    let pending: ApiError = request(&server, source.id, "low")
        .await
        .json()
        .await
        .unwrap();
    let id = pending.job_id.unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&server.db)
            .await
            .unwrap();
        let errors: String = sqlx::query_scalar("SELECT errors_json FROM jobs WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&server.db)
            .await
            .unwrap();
        if attempts > 0 && errors.contains("media_tool_unavailable") {
            break;
        }
        assert!(std::time::Instant::now() < until);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(request(&server, source.id, "original").await.status(), 200);
    server.config.ffmpeg_path = encoder;
    server.restart().await;
    assert!(matches!(
        finished(&server, id).await.status,
        JobStatus::Succeeded
    ));
    assert_eq!(request(&server, source.id, "low").await.status(), 200);
    server.finish().await;
}

#[tokio::test]
async fn deleted_unpinned_track_does_not_generate_unused_quality() {
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let source = seed(&server, "pcm.wav").await;
    let pending: ApiError = request(&server, source.id, "low")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .delete(format!("{}/api/v1/tracks/{}", server.url, source.id))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&serde_json::json!({"expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    server.restart().await;
    let done = finished(&server, pending.job_id.unwrap()).await;
    assert!(matches!(done.status, JobStatus::Failed));
    assert_eq!(done.errors[0].code, "revision_gone");
    server.finish().await;
}
