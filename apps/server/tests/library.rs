mod common;
use common::TestServer;
use dagaz_protocol::{
    Album, Artist, Job, JobStatus, Page, Playlist, PlaylistCreate, PlaylistRename, PlaylistTracks,
    RevisionRequest, Track, UploadCreate, UploadState,
};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, TaggedFileExt},
    tag::{Accessor, ItemKey},
};
use reqwest::Method;
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn rpc(server: &TestServer, method: Method, path: &str) -> reqwest::RequestBuilder {
    server
        .http
        .request(method, format!("{}{path}", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", Uuid::new_v4().to_string())
}
async fn song(
    server: &TestServer,
    title: &str,
    artist: &str,
    album_artist: &str,
    number: u32,
) -> Track {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("song.flac");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/tone.flac"),
        &file,
    )
    .unwrap();
    let mut audio = lofty::read_from_path(&file).unwrap();
    let tag = audio.primary_tag_mut().unwrap();
    tag.set_title(title.into());
    tag.set_artist(artist.into());
    tag.set_album("Ночь".into());
    tag.set_track(number);
    tag.set_disk(1);
    tag.insert_text(ItemKey::AlbumArtist, album_artist.into());
    audio.save_to_path(&file, WriteOptions::default()).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let response = rpc(server, Method::POST, "/api/v1/uploads")
        .json(&UploadCreate {
            filename: "песня.flac".into(),
            total_bytes: bytes.len() as i64,
            sha256: sha.clone(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let upload: UploadState = response.json().await.unwrap();
    assert_eq!(
        rpc(
            server,
            Method::PATCH,
            &format!("/api/v1/uploads/{}", upload.id)
        )
        .header("Upload-Offset", "0")
        .header("X-Chunk-SHA256", sha)
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    let job: Job = rpc(
        server,
        Method::POST,
        &format!("/api/v1/uploads/{}/complete", upload.id),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let id = loop {
        let current: Job = rpc(server, Method::GET, &format!("/api/v1/jobs/{}", job.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            !matches!(current.status, JobStatus::Failed),
            "{:?}",
            current.errors
        );
        if matches!(current.status, JobStatus::Succeeded) {
            break current.result.unwrap()["track_id"]
                .as_str()
                .unwrap()
                .to_owned();
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let response = rpc(server, Method::GET, &format!("/api/v1/tracks/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

#[tokio::test]
async fn unicode_search_album_identity_and_playlist_order() {
    let mut server = TestServer::spawn().await;
    let a = song(&server, "Первая", "Гость", "Группа А", 2).await;
    let b = song(&server, "Облака", "Гость", "Группа Б", 1).await;
    let c = song(&server, "Ночной поезд", "ГОСТЬ", "ГРУППА А", 1).await;
    assert!(a.artist_id.is_some());
    assert_eq!(a.artist_id, b.artist_id);
    assert_eq!(a.artist_id, c.artist_id);
    assert_ne!(a.album_id, b.album_id);
    assert_eq!(a.album_id, c.album_id);
    let found: Page<Track> = rpc(&server, Method::GET, "/api/v1/tracks")
        .query(&[("q", "гОсТь")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found.items.len(), 3);
    let safe = rpc(&server, Method::GET, "/api/v1/tracks")
        .query(&[("q", "\" OR artist:*")])
        .send()
        .await
        .unwrap();
    assert_eq!(safe.status(), 200);
    assert!(safe.json::<Page<Track>>().await.unwrap().items.is_empty());
    let albums: Page<Album> = rpc(&server, Method::GET, "/api/v1/albums")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(albums.items.len(), 2);
    assert!(
        albums
            .items
            .iter()
            .all(|album| album.album_artist_id.is_some())
    );
    let artists: Page<Artist> = rpc(&server, Method::GET, "/api/v1/artists")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(artists.items.len(), 3);
    let tracks: Page<Track> = rpc(
        &server,
        Method::GET,
        &format!("/api/v1/albums/{}/tracks", a.album_id.unwrap()),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        tracks
            .items
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        [c.id, a.id]
    );
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let mut request = rpc(&server, Method::GET, "/api/v1/tracks").query(&[("limit", 1)]);
        if let Some(cursor) = &cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let page: Page<Track> = request.send().await.unwrap().json().await.unwrap();
        for track in page.items {
            assert!(seen.insert(track.id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(seen.len() < 4);
    }
    assert_eq!(seen.len(), 3);
    let response = rpc(&server, Method::POST, "/api/v1/playlists")
        .json(&PlaylistCreate {
            name: "Метро".into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let playlist: Playlist = response.json().await.unwrap();
    let path = format!("/api/v1/playlists/{}", playlist.id);
    let members = PlaylistTracks {
        expected_revision: 1,
        track_ids: vec![b.id, a.id, b.id],
    };
    let key = Uuid::new_v4();
    let updated: Playlist = server
        .http
        .put(format!("{}{path}/tracks", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&members)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated.track_ids, members.track_ids);
    assert_eq!(updated.revision, 2);
    let retried: Playlist = server
        .http
        .put(format!("{}{path}/tracks", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&members)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retried.revision, 2);
    let first_page: Playlist = rpc(&server, Method::GET, &path)
        .query(&[("limit", 1)])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first_page.track_ids, vec![b.id]);
    let next_page: Playlist = rpc(&server, Method::GET, &path)
        .query(&[
            ("cursor", first_page.next_cursor.as_ref().unwrap()),
            ("limit", &"1".into()),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(next_page.track_ids, vec![a.id]);
    let invalid = rpc(&server, Method::PUT, &format!("{path}/tracks"))
        .json(&PlaylistTracks {
            expected_revision: 2,
            track_ids: vec![b.id, Uuid::new_v4()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    let different = server
        .http
        .put(format!("{}{path}/tracks", server.url))
        .bearer_auth(&server.token)
        .header("Idempotency-Key", key.to_string())
        .json(&PlaylistTracks {
            expected_revision: 2,
            track_ids: vec![b.id],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(different.status(), 409);
    assert_eq!(
        rpc(&server, Method::PATCH, &path)
            .json(&PlaylistRename {
                name: "Новое имя".into(),
                expected_revision: 1
            })
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let updated: Playlist = rpc(&server, Method::PUT, &format!("{path}/tracks"))
        .json(&PlaylistTracks {
            expected_revision: 2,
            track_ids: vec![b.id, a.id],
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated.total_count, 2);
    assert_eq!(
        rpc(&server, Method::GET, &path)
            .query(&[("cursor", first_page.next_cursor.unwrap())])
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        rpc(&server, Method::DELETE, &format!("/api/v1/tracks/{}", a.id))
            .json(&RevisionRequest {
                expected_revision: 1
            })
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let after: Playlist = rpc(&server, Method::GET, &path)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after.track_ids, vec![b.id]);
    assert_eq!(after.revision, 4);
    assert_eq!(
        rpc(&server, Method::DELETE, &path)
            .json(&RevisionRequest {
                expected_revision: 4
            })
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        rpc(&server, Method::GET, &format!("/api/v1/tracks/{}", b.id))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.restart().await;
    assert_eq!(
        rpc(&server, Method::GET, &path)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let response = rpc(&server, Method::POST, "/api/v1/playlists")
        .json(&PlaylistCreate {
            name: "Repeats".into(),
        })
        .send()
        .await
        .unwrap();
    let extra: Playlist = response.json().await.unwrap();
    let extra_path = format!("/api/v1/playlists/{}", extra.id);
    assert_eq!(
        rpc(&server, Method::PUT, &format!("{extra_path}/tracks"))
            .json(&PlaylistTracks {
                expected_revision: 1,
                track_ids: vec![b.id, b.id]
            })
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for track in [b, c] {
        assert_eq!(
            rpc(
                &server,
                Method::DELETE,
                &format!("/api/v1/tracks/{}", track.id)
            )
            .json(&RevisionRequest {
                expected_revision: 1
            })
            .send()
            .await
            .unwrap()
            .status(),
            204
        );
    }
    let emptied: Playlist = rpc(&server, Method::GET, &extra_path)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(emptied.track_ids.is_empty());
    assert_eq!(emptied.total_count, 0);
    assert_eq!(emptied.revision, 3);
    let artists: Page<Artist> = rpc(&server, Method::GET, "/api/v1/artists")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(artists.items.is_empty());
    let albums: Page<Album> = rpc(&server, Method::GET, "/api/v1/albums")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(albums.items.is_empty());
    let found: Page<Track> = rpc(&server, Method::GET, "/api/v1/tracks")
        .query(&[("q", "гость")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(found.items.is_empty());
    server.finish().await;
}

#[tokio::test]
#[ignore = "100,000-row catalog measurement; run explicitly"]
async fn catalog_pages_one_hundred_thousand_tracks() {
    let mut server = TestServer::spawn().await;
    let fixture = include_str!("../../../fixtures/contract/track-null-tags.json");
    let start = std::time::Instant::now();
    let mut tx = server.db.begin().await.unwrap();
    sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO tracks(id,revision,filename,data_json,sort_title,catalog_ready,last_sequence) SELECT printf('00000000-0000-0000-0000-%012x',x),1,'song.flac',json_set(?,'$.id',printf('00000000-0000-0000-0000-%012x',x),'$.title',printf('Track %06d',x)),printf('track %06d',x),1,x FROM n").bind(fixture).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO changes(sequence,entity,entity_id,revision,payload_json,created_at) SELECT last_sequence,'track',id,revision,data_json,? FROM tracks ORDER BY last_sequence").bind(1_795_000_000i64).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let seed_time = start.elapsed();
    let mut count = 0;
    let mut cursor = None;
    let start = std::time::Instant::now();
    let mut previous = Uuid::nil();
    loop {
        let mut request = rpc(&server, Method::GET, "/api/v1/tracks").query(&[("limit", 500)]);
        if let Some(cursor) = &cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        let page: Page<Track> = response.json().await.unwrap();
        assert!(page.items.len() <= 500);
        for track in page.items {
            assert!(track.id > previous);
            previous = track.id;
            count += 1;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(count, 100000);
    let pages_time = start.elapsed();
    let start = std::time::Instant::now();
    let found: Page<Track> = rpc(&server, Method::GET, "/api/v1/tracks")
        .query(&[("q", "Track 050000")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found.items.len(), 1);
    println!(
        "100000 tracks: seed={seed_time:?}, 200 HTTP pages={pages_time:?}, search={:?}; max page=500",
        start.elapsed()
    );
    server.restart().await;
    let start = std::time::Instant::now();
    let mut count = 0;
    let mut cursor = None;
    let mut previous = Uuid::nil();
    loop {
        let mut request = rpc(&server, Method::GET, "/api/v1/snapshot").query(&[("limit", 500)]);
        if let Some(cursor) = &cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        let page: dagaz_protocol::SnapshotPage = response.json().await.unwrap();
        assert!(page.records.len() <= 500);
        for record in page.records {
            assert!(record.entity_id > previous);
            assert!(record.sequence > 0);
            previous = record.entity_id;
            count += 1;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(count, 100000);
    println!(
        "100000 snapshot records: 200 HTTP pages={:?}; max page=500",
        start.elapsed()
    );
    server.finish().await;
}
