mod common;
use common::TestServer;
use dagaz_protocol::{Job, JobStatus, Track, UploadState};
use lofty::{
    file::TaggedFileExt,
    tag::{Accessor, TagType},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .unwrap()
}
async fn job(server: &TestServer, id: Uuid) -> Job {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result: Job = server
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
            result.status,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Partial
        ) {
            return result;
        }
        assert!(Instant::now() < deadline, "job did not finish: {result:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn import(server: &TestServer, name: &str) -> Track {
    let bytes = fixture(&format!("audio/{name}"));
    import_bytes(server, name, bytes).await
}
async fn import_bytes(server: &TestServer, name: &str, bytes: Vec<u8>) -> Track {
    let upload: UploadState = server.http.post(format!("{}/api/v1/uploads", server.url)).bearer_auth(&server.token).header("Idempotency-Key", Uuid::new_v4().to_string()).json(&json!({"filename":name,"total_bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))})).send().await.unwrap().json().await.unwrap();
    let response = server
        .http
        .patch(format!("{}/api/v1/uploads/{}", server.url, upload.id))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .header("Content-Type", "application/octet-stream")
        .header("Upload-Offset", "0")
        .header("X-Chunk-SHA256", format!("{:x}", Sha256::digest(&bytes)))
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
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
    let done = job(server, queued.id).await;
    assert!(matches!(done.status, JobStatus::Succeeded));
    track(
        server,
        done.result.unwrap()["track_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn embedded_artwork_is_indexed_and_album_cover_follows_edits() {
    use lofty::{
        config::WriteOptions,
        file::AudioFile,
        picture::{Picture, PictureType},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("covered.mp3");
    std::fs::write(&path, fixture("audio/vbr.mp3")).unwrap();
    let cover = fixture("artwork/cover.png");
    let mut audio = lofty::read_from_path(&path).unwrap();
    let tag = audio.primary_tag_mut().unwrap();
    tag.set_album("Embedded".into());
    let mut picture = Picture::from_reader(&mut std::io::Cursor::new(&cover)).unwrap();
    picture.set_pic_type(PictureType::CoverFront);
    tag.push_picture(picture);
    let mut back = Picture::from_reader(&mut std::io::Cursor::new(&cover)).unwrap();
    back.set_pic_type(PictureType::CoverBack);
    tag.push_picture(back);
    audio.save_to_path(&path, WriteOptions::default()).unwrap();
    let server = TestServer::spawn().await;
    let imported = import_bytes(&server, "covered.mp3", std::fs::read(path).unwrap()).await;
    assert!(imported.artwork_id.is_some());
    let albums: Value = server
        .http
        .get(format!("{}/api/v1/albums", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(albums["items"][0]["artwork_id"], json!(imported.artwork_id));
    let queued: Job = edit(
        &server,
        imported.id,
        1,
        json!({"artwork_id":null}),
        Uuid::new_v4(),
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(matches!(
        job(&server, queued.id).await.status,
        JobStatus::Succeeded
    ));
    let albums: Value = server
        .http
        .get(format!("{}/api/v1/albums", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(albums["items"][0]["artwork_id"].is_null());
    let file = directory.path().join("cleared.mp3");
    std::fs::write(&file, media(&server, imported.id, 2).await).unwrap();
    let parsed = lofty::read_from_path(file).unwrap();
    let pictures = parsed.primary_tag().unwrap().pictures();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].pic_type(), PictureType::CoverBack);
    server.finish().await;
}
async fn track(server: &TestServer, id: Uuid) -> Track {
    server
        .http
        .get(format!("{}/api/v1/tracks/{id}", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn edit(
    server: &TestServer,
    id: Uuid,
    revision: i64,
    patch: Value,
    key: Uuid,
) -> reqwest::Response {
    server
        .http
        .patch(format!("{}/api/v1/tracks/{id}/metadata", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&json!({"expected_revision":revision,"patch":patch}))
        .send()
        .await
        .unwrap()
}
async fn media(server: &TestServer, id: Uuid, revision: i64) -> Vec<u8> {
    let response = server
        .http
        .get(format!(
            "{}/api/v1/tracks/{id}/media?revision={revision}&quality=original",
            server.url
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.bytes().await.unwrap().to_vec()
}
fn pcm(directory: &Path, name: &str) -> Vec<u8> {
    let output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{}:/data:ro", directory.display()),
            "dagaz-audio-tools:validation",
            "-v",
            "error",
            "-i",
            &format!("/data/{name}"),
            "-f",
            "f64le",
            "-acodec",
            "pcm_f64le",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout
}

#[tokio::test]
async fn retag_is_atomic_preserves_audio_and_survives_retry() {
    let server = TestServer::spawn().await;
    let cover = fixture("artwork/cover.png");
    let response = server
        .http
        .post(format!("{}/api/v1/artworks", server.url))
        .bearer_auth(&server.token)
        .header("Content-Type", "image/png")
        .body(cover.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let artwork: Value = response.json().await.unwrap();
    for name in [
        "pcm.wav",
        "float.wav",
        "untagged.wav",
        "unknown-chunk.wav",
        "vbr.mp3",
        "tone.flac",
    ] {
        let original = fixture(&format!("audio/{name}"));
        let before = import(&server, name).await;
        let key = Uuid::new_v4();
        let patch =
            json!({"artist":"Новый исполнитель","album":"Метро","artwork_id":artwork["id"]});
        let response = edit(&server, before.id, 1, patch.clone(), key).await;
        assert_eq!(response.status(), 202);
        let queued: Job = response.json().await.unwrap();
        let done = job(&server, queued.id).await;
        assert!(matches!(done.status, JobStatus::Succeeded), "{done:?}");
        let replay: Job = edit(&server, before.id, 1, patch, key)
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(queued.id, replay.id);
        assert_eq!(
            edit(
                &server,
                before.id,
                1,
                json!({"title":"conflict"}),
                Uuid::new_v4()
            )
            .await
            .status(),
            409
        );
        let after = track(&server, before.id).await;
        assert_eq!(after.revision, 2);
        assert_eq!(after.title, before.title);
        assert_eq!(after.artist.as_deref(), Some("Новый исполнитель"));
        assert!(after.album_id.is_some());
        assert_eq!(media(&server, before.id, 1).await, original);
        let bytes = media(&server, before.id, 2).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("before"), &original).unwrap();
        std::fs::write(dir.path().join("after"), &bytes).unwrap();
        assert_eq!(
            pcm(dir.path(), "before"),
            pcm(dir.path(), "after"),
            "{name}"
        );
        let parsed = lofty::probe::Probe::open(dir.path().join("after"))
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag = parsed
            .tag(if name.ends_with("flac") {
                TagType::VorbisComments
            } else {
                TagType::Id3v2
            })
            .unwrap();
        assert_eq!(tag.artist().as_deref(), Some("Новый исполнитель"));
        assert_eq!(tag.album().as_deref(), Some("Метро"));
        assert_eq!(tag.pictures()[0].data(), cover);
        let previous = lofty::probe::Probe::open(dir.path().join("before"))
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let previous = previous
            .tag(TagType::Id3v2)
            .or_else(|| previous.primary_tag());
        assert_eq!(
            tag.comment().as_deref(),
            previous.and_then(|tag| tag.comment()).as_deref()
        );
        if name == "unknown-chunk.wav" {
            assert!(bytes.windows(9).any(|part| part == b"DoNotLose"));
        }
        let cleared: Job = edit(
            &server,
            before.id,
            2,
            json!({"album":null,"artwork_id":null}),
            Uuid::new_v4(),
        )
        .await
        .json()
        .await
        .unwrap();
        assert!(matches!(
            job(&server, cleared.id).await.status,
            JobStatus::Succeeded
        ));
        let cleared = track(&server, before.id).await;
        assert_eq!(cleared.revision, 3);
        assert!(cleared.album.is_none());
        assert!(cleared.album_id.is_none());
        assert!(cleared.artwork_id.is_none());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("clear"), media(&server, before.id, 3).await).unwrap();
        let parsed = lofty::probe::Probe::open(dir.path().join("clear"))
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        let tag = parsed
            .tag(if name.ends_with("flac") {
                TagType::VorbisComments
            } else {
                TagType::Id3v2
            })
            .unwrap();
        assert!(tag.album().is_none());
        assert!(tag.pictures().is_empty());
    }
    server.finish().await;
}

#[tokio::test]
async fn batch_and_crash_recovery_publish_each_revision_once() {
    let mut server = TestServer::spawn().await;
    let source = import(&server, "pcm.wav").await;
    let response=server.http.post(format!("{}/api/v1/tracks/metadata-batch",server.url)).bearer_auth(&server.token).header("Idempotency-Key",Uuid::new_v4().to_string()).json(&json!({"items":[{"track_id":source.id,"expected_revision":1},{"track_id":Uuid::new_v4(),"expected_revision":1}],"patch":{"title":"Партия"}})).send().await.unwrap();
    assert_eq!(response.status(), 202);
    let queued: Job = response.json().await.unwrap();
    let done = job(&server, queued.id).await;
    assert!(matches!(done.status, JobStatus::Partial));
    assert_eq!(done.result.unwrap()["items"].as_array().unwrap().len(), 2);
    sqlx::query("CREATE TRIGGER reject_retag BEFORE INSERT ON media_revisions WHEN NEW.revision=3 BEGIN SELECT RAISE(ABORT,'test commit failure'); END").execute(&server.db).await.unwrap();
    let queued: Job = edit(
        &server,
        source.id,
        2,
        json!({"title":"После сбоя"}),
        Uuid::new_v4(),
    )
    .await
    .json()
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let list = server
            .s3
            .list_objects_v2()
            .bucket(&server.config.s3_bucket)
            .prefix(format!("tracks/{}/3/", source.id))
            .send()
            .await
            .unwrap();
        if !list.contents().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.expect_job_worker_failure().await;
    let changes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM changes WHERE entity='track' AND entity_id=? AND revision=3",
    )
    .bind(source.id.to_string())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(changes, 0);
    let search: Value = server
        .http
        .get(format!("{}/api/v1/tracks", server.url))
        .bearer_auth(&server.token)
        .query(&[("q", "После сбоя")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(search["items"].as_array().unwrap().is_empty());
    assert_eq!(track(&server, source.id).await.revision, 2);
    assert!(!media(&server, source.id, 2).await.is_empty());
    sqlx::query("DROP TRIGGER reject_retag")
        .execute(&server.db)
        .await
        .unwrap();
    server.restart().await;
    assert!(matches!(
        job(&server, queued.id).await.status,
        JobStatus::Succeeded
    ));
    assert_eq!(track(&server, source.id).await.revision, 3);
    let search: Value = server
        .http
        .get(format!("{}/api/v1/tracks", server.url))
        .bearer_auth(&server.token)
        .query(&[("q", "После сбоя")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(search["items"][0]["id"], json!(source.id));
    sqlx::query("UPDATE jobs SET status='running',result_json=NULL WHERE id=?")
        .bind(queued.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    server.restart().await;
    assert!(matches!(
        job(&server, queued.id).await.status,
        JobStatus::Succeeded
    ));
    assert_eq!(track(&server, source.id).await.revision, 3);
    server.finish().await;
}

#[tokio::test]
async fn artwork_rejects_invalid_and_oversized_input() {
    let server = TestServer::spawn().await;
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(512, 256)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let response = server
        .http
        .post(format!("{}/api/v1/artworks", server.url))
        .bearer_auth(&server.token)
        .header("Content-Type", "image/jpeg")
        .body(jpeg.into_inner())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let artwork: Value = response.json().await.unwrap();
    let response = server
        .http
        .get(format!(
            "{}/api/v1/artworks/{}?size=256",
            server.url,
            artwork["id"].as_str().unwrap()
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "image/png");
    let thumb = response.bytes().await.unwrap();
    let image = image::load_from_memory(&thumb).unwrap();
    assert_eq!((image.width(), image.height()), (256, 128));
    let mut too_many_pixels = std::io::Cursor::new(Vec::new());
    image::GrayImage::new(5001, 5000)
        .write_to(&mut too_many_pixels, image::ImageFormat::Png)
        .unwrap();
    let response = server
        .http
        .post(format!("{}/api/v1/artworks", server.url))
        .bearer_auth(&server.token)
        .header("Content-Type", "image/png")
        .body(too_many_pixels.into_inner())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    for (mime, bytes, status) in [
        ("image/png", b"not a picture".to_vec(), 422),
        (
            "application/json",
            br#"{"url":"http://127.0.0.1/private"}"#.to_vec(),
            415,
        ),
        ("image/png", vec![0; 10 * 1024 * 1024 + 1], 413),
    ] {
        let response = server
            .http
            .post(format!("{}/api/v1/artworks", server.url))
            .bearer_auth(&server.token)
            .header("Content-Type", mime)
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let cover = fixture("artwork/cover.png");
    let artwork: Value = server
        .http
        .post(format!("{}/api/v1/artworks", server.url))
        .bearer_auth(&server.token)
        .header("Content-Type", "image/png")
        .body(cover.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!(
        "{}/api/v1/artworks/{}",
        server.url,
        artwork["id"].as_str().unwrap()
    );
    assert_eq!(server.http.get(&url).send().await.unwrap().status(), 401);
    let response = server
        .http
        .get(&url)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.bytes().await.unwrap(), cover);
    assert_eq!(
        server
            .http
            .get(format!("{url}?size=256"))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.finish().await;
}

#[tokio::test]
async fn concurrent_edits_and_invalid_batches_never_overwrite_changes() {
    let server = TestServer::spawn().await;
    let source = import(&server, "pcm.wav").await;
    let (first, second) = tokio::join!(
        edit(
            &server,
            source.id,
            1,
            json!({"title":"Первый"}),
            Uuid::new_v4()
        ),
        edit(
            &server,
            source.id,
            1,
            json!({"title":"Второй"}),
            Uuid::new_v4()
        )
    );
    let mut succeeded = 0;
    for response in [first, second] {
        if response.status() == 409 {
            continue;
        }
        assert_eq!(response.status(), 202);
        let queued: Job = response.json().await.unwrap();
        let done = job(&server, queued.id).await;
        if matches!(done.status, JobStatus::Succeeded) {
            succeeded += 1;
        } else {
            assert_eq!(done.errors[0].code, "revision_conflict");
        }
    }
    assert_eq!(succeeded, 1);
    assert_eq!(track(&server, source.id).await.revision, 2);
    for items in [
        json!([]),
        json!([{"track_id":source.id,"expected_revision":2},{"track_id":source.id,"expected_revision":2}]),
        Value::Array(
            (0..101)
                .map(|_| json!({"track_id":Uuid::new_v4(),"expected_revision":1}))
                .collect(),
        ),
    ] {
        let response = server
            .http
            .post(format!("{}/api/v1/tracks/metadata-batch", server.url))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"items":items,"patch":{"title":"bad batch"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }
    assert_eq!(
        edit(&server, source.id, 2, json!({"title":""}), Uuid::new_v4())
            .await
            .status(),
        422
    );
    assert_eq!(
        edit(
            &server,
            source.id,
            2,
            json!({"artwork_id":Uuid::new_v4()}),
            Uuid::new_v4()
        )
        .await
        .status(),
        404
    );
    assert_eq!(track(&server, source.id).await.revision, 2);
    let value = "x".repeat(4096);
    let response = edit(
        &server,
        source.id,
        2,
        json!({"title":value,"artist":value,"album":value,"album_artist":value}),
        Uuid::new_v4(),
    )
    .await;
    assert_eq!(response.status(), 202);
    let queued: Job = response.json().await.unwrap();
    assert!(matches!(
        job(&server, queued.id).await.status,
        JobStatus::Succeeded
    ));
    server.finish().await;
}

#[tokio::test]
async fn lost_storage_completion_response_does_not_duplicate_revision() {
    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::State,
        http::{Request, Response, StatusCode},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(Clone)]
    struct Proxy {
        target: String,
        http: reqwest::Client,
        lost: Arc<AtomicUsize>,
    }
    async fn forward(State(proxy): State<Proxy>, request: Request<Body>) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let completion = parts.method == "POST"
            && parts.uri.path().contains("/2/original/")
            && parts
                .uri
                .query()
                .is_some_and(|query| query.contains("uploadId="));
        let bytes = to_bytes(body, 16 * 1024 * 1024).await.unwrap();
        let response = proxy
            .http
            .request(
                parts.method,
                format!("{}{}", proxy.target, parts.uri.path_and_query().unwrap()),
            )
            .headers(parts.headers)
            .body(bytes)
            .send()
            .await
            .unwrap();
        if completion
            && response.status().is_success()
            && proxy
                .lost
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(Body::from("injected response loss after durable S3 write"))
                .unwrap();
        }
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.unwrap();
        let mut response = Response::builder()
            .status(status)
            .body(Body::from(bytes))
            .unwrap();
        *response.headers_mut() = headers;
        response
    }
    let mut server = TestServer::spawn().await;
    let source = import(&server, "pcm.wav").await;
    let lost = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let proxy = Proxy {
        target: server.config.s3_endpoint.clone(),
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        lost: lost.clone(),
    };
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(forward).with_state(proxy))
            .await
            .unwrap()
    });
    server.config.s3_endpoint = endpoint;
    server.s3 = dagaz_server::create_s3(&server.config).unwrap();
    server.restart().await;
    let queued: Job = edit(
        &server,
        source.id,
        1,
        json!({"album":"Ответ потерян"}),
        Uuid::new_v4(),
    )
    .await
    .json()
    .await
    .unwrap();
    let result = job(&server, queued.id).await;
    assert!(matches!(result.status, JobStatus::Succeeded), "{result:?}");
    assert_eq!(lost.load(Ordering::SeqCst), 1);
    assert_eq!(track(&server, source.id).await.revision, 2);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media_revisions WHERE track_id=? AND revision=2")
            .bind(source.id.to_string())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(media(&server, source.id, 1).await, fixture("audio/pcm.wav"));
    server.finish().await;
    task.abort();
}
