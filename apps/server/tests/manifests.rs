mod common;
use common::{TestServer, audio::*};
use dagaz_protocol::{ManifestCreated, ManifestPage};
use serde_json::{Value, json};
use uuid::Uuid;
async fn create(server: &TestServer, kind: &str, id: Uuid, key: Uuid) -> reqwest::Response {
    server
        .http
        .post(format!("{}/api/v1/download-manifests", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&json!({"kind":kind,"id":id,"quality":"original"}))
        .send()
        .await
        .unwrap()
}
#[tokio::test]
async fn manifest_is_an_immutable_paged_snapshot_with_duplicate_occurrences() {
    let server = TestServer::spawn().await;
    let first = seed(&server, "pcm.wav").await;
    let second = seed(&server, "tone.flac").await;
    let playlist: Value = server
        .http
        .post(format!("{}/api/v1/playlists", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&json!({"name":"Metro"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let playlist_id = playlist["id"].as_str().unwrap().parse().unwrap();
    let response = server
        .http
        .put(format!(
            "{}/api/v1/playlists/{playlist_id}/tracks",
            server.url
        ))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&json!({"expected_revision":1,"track_ids":[first.id,second.id,first.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let key = Uuid::new_v4();
    let response = create(&server, "playlist", playlist_id, key).await;
    assert_eq!(response.status(), 201);
    let manifest: ManifestCreated = response.json().await.unwrap();
    let repeated: ManifestCreated = create(&server, "playlist", playlist_id, key)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(manifest.id, repeated.id);
    let url = format!("{}/api/v1/download-manifests/{}", server.url, manifest.id);
    let page: ManifestPage = server
        .http
        .get(&url)
        .query(&[("limit", "1")])
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.items[0].track_id, first.id);
    assert_eq!(page.selection_revision, 2);
    assert!(page.next_cursor.is_some());
    assert_eq!(
        server
            .http
            .put(format!(
                "{}/api/v1/playlists/{playlist_id}/tracks",
                server.url
            ))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"expected_revision":2,"track_ids":[second.id]}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let rest: ManifestPage = server
        .http
        .get(&url)
        .query(&[("cursor", page.next_cursor.unwrap())])
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        rest.items
            .iter()
            .map(|item| item.track_id)
            .collect::<Vec<_>>(),
        vec![second.id, first.id]
    );
    assert_eq!(rest.selection_revision, 2);
    sqlx::query("UPDATE download_manifests SET expires_at=0 WHERE id=?")
        .bind(manifest.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .get(&url)
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    server.finish().await;
}
#[tokio::test]
async fn snapshot_pins_deleted_tracks_until_expiry() {
    let server = TestServer::spawn().await;
    let source = seed(&server, "pcm.wav").await;
    let response = create(&server, "track", source.id, Uuid::new_v4()).await;
    assert_eq!(response.status(), 201);
    let manifest: ManifestCreated = response.json().await.unwrap();
    assert_eq!(
        server
            .http
            .delete(format!("{}/api/v1/tracks/{}", server.url, source.id))
            .bearer_auth(&server.token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let response = server
        .http
        .get(format!(
            "{}/api/v1/download-manifests/{}",
            server.url, manifest.id
        ))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let page: ManifestPage = response.json().await.unwrap();
    assert_eq!(page.items[0].revision, 1);
    assert_eq!(
        server
            .http
            .get(format!("{}{}", server.url, page.items[0].url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
        fixture("pcm.wav")
    );
    server.finish().await;
}

#[tokio::test]
async fn metadata_change_keeps_snapshot_revision_and_pins_expired_source() {
    let server = TestServer::spawn().await;
    let source = seed(&server, "pcm.wav").await;
    let manifest: ManifestCreated = create(&server, "track", source.id, Uuid::new_v4())
        .await
        .json()
        .await
        .unwrap();
    let queued: dagaz_protocol::Job = server
        .http
        .patch(format!(
            "{}/api/v1/tracks/{}/metadata",
            server.url, source.id
        ))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&json!({"expected_revision":1,"patch":{"title":"Changed"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(matches!(
        finished(&server, queued.id).await.status,
        dagaz_protocol::JobStatus::Succeeded
    ));
    sqlx::query("UPDATE media_revisions SET superseded_at=0 WHERE track_id=? AND revision=1")
        .bind(source.id.to_string())
        .execute(&server.db)
        .await
        .unwrap();
    let url = format!("{}/api/v1/download-manifests/{}", server.url, manifest.id);
    let page: ManifestPage = server
        .http
        .get(&url)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.items[0].revision, 1);
    assert_eq!(page.items[0].byte_length, source.original_bytes);
    assert_eq!(
        server
            .http
            .get(format!("{}{}", server.url, page.items[0].url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
        fixture("pcm.wav")
    );
    sqlx::query("UPDATE download_manifests SET expires_at=0")
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .get(format!("{}{}", server.url, page.items[0].url))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    server.finish().await;
}

#[tokio::test]
async fn pending_manifest_quality_has_job_and_effective_original_descriptor() {
    let mut server = TestServer::spawn().await;
    server.pause_jobs().await;
    let source = seed(&server, "profiles/low.mp3").await;
    let response = server
        .http
        .post(format!("{}/api/v1/download-manifests", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&json!({"kind":"track","id":source.id,"quality":"high"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let manifest: ManifestCreated = response.json().await.unwrap();
    let url = format!("{}/api/v1/download-manifests/{}", server.url, manifest.id);
    let response = server
        .http
        .get(&url)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 425);
    let pending: dagaz_protocol::ApiError = response.json().await.unwrap();
    assert!(pending.job_id.is_some());
    server.restart().await;
    assert!(matches!(
        finished(&server, pending.job_id.unwrap()).await.status,
        dagaz_protocol::JobStatus::Succeeded
    ));
    let url = format!("{}/api/v1/download-manifests/{}", server.url, manifest.id);
    let page: ManifestPage = server
        .http
        .get(&url)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        page.items[0].effective_quality,
        dagaz_protocol::Quality::Original
    );
    assert!(page.items[0].url.ends_with("quality=original"));
    server.finish().await;
}

#[tokio::test]
async fn album_manifest_uses_album_order_and_rejects_foreign_cursor() {
    let server = TestServer::spawn().await;
    let first = seed(&server, "pcm.wav").await;
    let second = seed(&server, "tone.flac").await;
    let album = Uuid::new_v4();
    let value = json!({"id":album,"name":"Album","album_artist":null,"album_artist_id":null,"artwork_id":null});
    sqlx::query("INSERT INTO albums(id,identity_key,name,normalized_name,data_json) VALUES(?,'album','Album','album',?)").bind(album.to_string()).bind(value.to_string()).execute(&server.db).await.unwrap();
    for (track, order) in [(first.id, "02"), (second.id, "01")] {
        sqlx::query("UPDATE tracks SET album_id=?,sort_album_order=? WHERE id=?")
            .bind(album.to_string())
            .bind(order)
            .bind(track.to_string())
            .execute(&server.db)
            .await
            .unwrap();
    }
    let manifest: ManifestCreated = create(&server, "album", album, Uuid::new_v4())
        .await
        .json()
        .await
        .unwrap();
    let url = format!("{}/api/v1/download-manifests/{}", server.url, manifest.id);
    let page: ManifestPage = server
        .http
        .get(&url)
        .query(&[("limit", "1")])
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.items[0].track_id, second.id);
    let other: ManifestCreated = create(&server, "track", first.id, Uuid::new_v4())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .get(format!(
                "{}/api/v1/download-manifests/{}",
                server.url, other.id
            ))
            .query(&[("cursor", page.next_cursor.unwrap())])
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    server.finish().await;
}
