mod common;
use common::TestServer;

#[tokio::test]
#[ignore = "live Android fixture; driven by apps/android/run-device.ps1 -Phase RealServer"]
async fn android_pairing_fixture() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let directory = std::path::PathBuf::from(
        std::env::var_os("DAGAZ_ANDROID_FIXTURE_DIR").expect("fixture directory required"),
    )
    .canonicalize()
    .unwrap();
    assert!(directory.starts_with(workspace.join(".superpowers")));
    let ready = directory.join("android-fixture.json");
    let finished = directory.join("android-fixture.finished");
    if ready.exists() {
        std::fs::remove_file(&ready).unwrap();
    }
    if finished.exists() {
        std::fs::remove_file(&finished).unwrap();
    }
    let mut server = TestServer::spawn().await;
    server.restart().await;
    let code = dagaz_server::new_pairing_code(&server.db).await.unwrap();
    let mut settings = serde_json::json!({"url":server.url,"code":code});
    if std::env::var_os("DAGAZ_ANDROID_MEDIA").is_some() {
        let audio = tempfile::tempdir().unwrap();
        let path = audio.path().join("tone.wav");
        assert!(
            std::process::Command::new(&server.config.ffmpeg_path)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=44100:duration=30",
                    "-c:a",
                    "pcm_s16le"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let track =
            common::audio::import_bytes(&server, "tone.wav", std::fs::read(path).unwrap()).await;
        let alias = common::audio::import(&server, "profiles/low.mp3").await;
        for id in [track.id, alias.id] {
            let jobs: Vec<String> =
                sqlx::query_scalar("SELECT job_id FROM quality_jobs WHERE track_id=?")
                    .bind(id.to_string())
                    .fetch_all(&server.db)
                    .await
                    .unwrap();
            for job in jobs {
                assert!(matches!(
                    common::audio::finished(&server, job.parse().unwrap())
                        .await
                        .status,
                    dagaz_protocol::JobStatus::Succeeded
                ));
            }
        }
        settings["track_id"] = serde_json::json!(track.id);
        settings["alias_track_id"] = serde_json::json!(alias.id);
    }
    std::fs::write(&ready, settings.to_string()).unwrap();
    println!("Android fixture ready; secrets remain in the ignored fixture file");
    let result = tokio::time::timeout(std::time::Duration::from_secs(180), async {
        loop {
            if finished.exists() {
                break std::fs::read_to_string(&finished).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await;
    server.finish().await;
    std::fs::remove_file(&ready).unwrap();
    assert_eq!(result.expect("Android fixture timed out").trim(), "ok");
}
