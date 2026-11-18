mod common;
use common::TestServer;
use dagaz_protocol::{Page, RevisionRequest, SnapshotPage, SyncPage, Track};
use std::collections::BTreeMap;
use uuid::Uuid;

async fn seed(server: &TestServer, number: u128, revision: i64, title: &str) {
    let mut track: Track = serde_json::from_str(include_str!(
        "../../../fixtures/contract/track-null-tags.json"
    ))
    .unwrap();
    track.id = Uuid::from_u128(number);
    track.revision = revision;
    track.title = Some(title.into());
    sqlx::query("INSERT INTO tracks(id,revision,filename,data_json) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,data_json=excluded.data_json,catalog_ready=0").bind(track.id.to_string()).bind(revision).bind(&track.filename).bind(serde_json::to_string(&track).unwrap()).execute(&server.db).await.unwrap();
}
fn apply(
    map: &mut BTreeMap<(String, Uuid), dagaz_protocol::Change>,
    changes: Vec<dagaz_protocol::Change>,
) {
    for change in changes {
        let key = (
            serde_json::to_string(&change.entity).unwrap(),
            change.entity_id,
        );
        if map
            .get(&key)
            .is_none_or(|old| old.sequence < change.sequence)
        {
            map.insert(key, change);
        }
    }
}
#[tokio::test]
async fn snapshot_catches_concurrent_changes_and_deletes() {
    let mut server = TestServer::spawn().await;
    seed(&server, 1, 1, "One").await;
    seed(&server, 2, 1, "Two").await;
    server.restart().await;
    let response = server
        .http
        .get(format!("{}/api/v1/snapshot?limit=1", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let first: SnapshotPage = response.json().await.unwrap();
    assert_eq!(first.records.len(), 1);
    let mut map = BTreeMap::new();
    let mut cursor = first.next_cursor.clone();
    apply(&mut map, first.records);
    seed(&server, 0, 1, "Behind cursor").await;
    seed(&server, 3, 1, "Three").await;
    server.restart().await;
    seed(&server, 3, 2, "New title").await;
    server.restart().await;
    let response = server
        .http
        .delete(format!(
            "{}/api/v1/tracks/{}",
            server.url,
            Uuid::from_u128(2)
        ))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&RevisionRequest {
            expected_revision: 1,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    while let Some(next) = cursor {
        let response = server
            .http
            .get(format!("{}/api/v1/snapshot", server.url))
            .bearer_auth(&server.token)
            .query(&[("cursor", next), ("limit", "1".into())])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let page: SnapshotPage = response.json().await.unwrap();
        assert_eq!(page.high_watermark, first.high_watermark);
        assert_eq!(page.generation, first.generation);
        cursor = page.next_cursor;
        apply(&mut map, page.records);
    }
    let original_cursor = format!("{}:{}", first.generation, first.high_watermark);
    let mut delta_cursor = original_cursor.clone();
    loop {
        let response = server
            .http
            .get(format!("{}/api/v1/changes", server.url))
            .bearer_auth(&server.token)
            .query(&[("cursor", delta_cursor), ("limit", "2".into())])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let page: SyncPage = response.json().await.unwrap();
        delta_cursor = page.next_cursor;
        apply(&mut map, page.changes);
        if !page.has_more {
            break;
        }
    }
    let actual: Page<Track> = server
        .http
        .get(format!("{}/api/v1/tracks", server.url))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let expected: BTreeMap<_, _> = actual
        .items
        .into_iter()
        .map(|track| (track.id, track.title))
        .collect();
    let observed: BTreeMap<_, _> = map
        .into_iter()
        .filter(|((kind, _), value)| kind == "\"track\"" && !value.deleted)
        .map(|((_, id), value)| {
            let track: Track = serde_json::from_value(value.payload.unwrap()).unwrap();
            (id, track.title)
        })
        .collect();
    assert_eq!(observed, expected);
    assert_eq!(observed.len(), 3);
    assert_eq!(observed[&Uuid::from_u128(3)].as_deref(), Some("New title"));
    sqlx::query("UPDATE server_meta SET value=? WHERE key='sync_floor'")
        .bind((first.high_watermark + 1).to_string())
        .execute(&server.db)
        .await
        .unwrap();
    let expired = server
        .http
        .get(format!("{}/api/v1/changes", server.url))
        .bearer_auth(&server.token)
        .query(&[("cursor", &original_cursor)])
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 410);
    sqlx::query("UPDATE server_meta SET value=? WHERE key='generation'")
        .bind(Uuid::new_v4().to_string())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        server
            .http
            .get(format!("{}/api/v1/changes", server.url))
            .bearer_auth(&server.token)
            .query(&[("cursor", &delta_cursor)])
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    server.finish().await;
}
