use dagaz_protocol::{MetadataPatch, PlaybackState, Quality};
use serde_json::{from_value, json, to_value};

#[test]
fn track_keeps_artist_identity_for_catalog_links() {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contract/track-null-tags.json"
    ))
    .unwrap();
    value["artist_id"] = json!("00000000-0000-0000-0000-000000000003");
    let track: dagaz_protocol::Track = from_value(value.clone()).unwrap();
    assert_eq!(to_value(track).unwrap(), value);
}

fn state() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/contract/playback-duplicates.json"
    ))
    .unwrap()
}

#[test]
fn metadata_patch_distinguishes_absent_clear_and_value() {
    for value in [
        json!({}),
        json!({"album":null}),
        json!({"album":"Тестовый альбом"}),
    ] {
        let patch: MetadataPatch = from_value(value.clone()).unwrap();
        assert_eq!(to_value(patch).unwrap(), value);
    }
}

#[test]
fn playback_preserves_duplicate_occurrences_and_actual_shuffle_order() {
    let decoded: PlaybackState = from_value(state()).unwrap();
    assert_eq!(decoded.queue[0], decoded.queue[1]);
    assert_eq!(decoded.queue_index, Some(1));
    assert_eq!(decoded.play_order, vec![1, 0]);
    assert_eq!(to_value(decoded).unwrap(), state());
}

#[test]
fn rejects_invalid_playback_and_unknown_quality() {
    for (field, value) in [
        ("position_ms", json!(-1)),
        ("epoch", json!(0)),
        ("play_order", json!([0, 0])),
        ("queue_index", json!(2)),
    ] {
        let mut invalid = state();
        invalid[field] = value;
        assert!(
            from_value::<PlaybackState>(invalid).is_err(),
            "accepted {field}"
        );
    }
    assert!(from_value::<Quality>(json!("ultra")).is_err());
}
#[test]
fn idle_state_can_have_no_queue_occurrence() {
    let mut value = state();
    value["queue"] = json!([]);
    value["play_order"] = json!([]);
    value["queue_index"] = json!(null);
    value["track_id"] = json!(null);
    value["owner_device_id"] = json!(null);
    value["paused"] = json!(true);
    let decoded: PlaybackState = from_value(value.clone()).unwrap();
    assert_eq!(to_value(decoded).unwrap(), value);
}

#[test]
fn rejects_mistyped_or_unknown_metadata_fields() {
    for value in [
        json!({"album":7}),
        json!({"artwork_id":"bad-id"}),
        json!({"alubm":"typo"}),
    ] {
        assert!(from_value::<MetadataPatch>(value).is_err());
    }
}

#[test]
fn remaining_wire_models_decode_server_examples() {
    use dagaz_protocol::{ApiError, Job, MediaDescriptor, SyncPage, UploadState};
    let id = "00000000-0000-0000-0000-000000000001";
    let error: ApiError = from_value(
        json!({"code":"representation_pending","message":"Preparing","request_id":id,"job_id":id}),
    )
    .unwrap();
    assert!(error.job_id.is_some());
    let upload: UploadState = from_value(json!({"id":id,"offset":8,"total_bytes":10,"expires_at":"2026-11-18T12:00:00Z","status":"uploading"})).unwrap();
    assert_eq!(upload.offset, 8);
    let job: Job =
        from_value(json!({"id":id,"kind":"import","status":"queued","result":null,"errors":[]}))
            .unwrap();
    assert!(job.result.is_none());
    let media: MediaDescriptor = from_value(json!({"track_id":id,"revision":1,"requested_quality":"high","effective_quality":"original","byte_length":10,"sha256":"a".repeat(64),"etag":"opaque","url":"https://music.example/api/v1/tracks/id/media"})).unwrap();
    assert_eq!(media.effective_quality, Quality::Original);
    let page: SyncPage =
        from_value(json!({"generation":id,"changes":[],"next_cursor":"0","has_more":false}))
            .unwrap();
    assert!(page.changes.is_empty());
}

#[test]
fn playback_commands_reject_negative_positions_and_epochs() {
    use dagaz_protocol::PlaybackCommand;
    let id = "00000000-0000-0000-0000-000000000001";
    for (epoch, position) in [(1, -1), (0, 0)] {
        assert!(from_value::<PlaybackCommand>(json!({"operation_id":id,"expected_epoch":epoch,"action":"seek","payload":{"position_ms":position}})).is_err());
    }
}

#[test]
fn rejects_negative_upload_size_and_nonpositive_report_epoch() {
    use dagaz_protocol::{PlaybackReport, UploadCreate};
    let id = "00000000-0000-0000-0000-000000000001";
    assert!(
        from_value::<UploadCreate>(
            json!({"filename":"song.flac","total_bytes":-1,"sha256":"a".repeat(64)})
        )
        .is_err()
    );
    assert!(from_value::<PlaybackReport>(json!({"epoch":0,"sequence":1,"track_id":id,"queue_index":0,"position_ms":0,"paused":false})).is_err());
}
#[test]
fn catalog_contract_has_artist_filter_and_playlist_pagination() {
    let document = dagaz_protocol::openapi_document();
    for path in ["/api/v1/tracks", "/api/v1/albums"] {
        assert!(
            document["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value["name"] == "artist_id" && value["schema"]["format"] == "uuid")
        );
    }
    for name in ["limit", "cursor"] {
        assert!(
            document["paths"]["/api/v1/playlists/{id}"]["get"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value["name"] == name)
        );
    }
    assert!(
        document["paths"]["/api/v1/changes"]["get"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["name"] == "cursor" && value["required"] == true)
    );
}
