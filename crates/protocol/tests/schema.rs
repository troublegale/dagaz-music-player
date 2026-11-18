use dagaz_protocol::openapi_document;
use serde_json::json;

#[test]
fn metadata_schema_accepts_clear_but_rejects_wrong_type_and_typo() {
    let mut doc = openapi_document();
    doc["$ref"] = json!("#/components/schemas/MetadataPatch");
    let validator = jsonschema::validator_for(&doc).unwrap();
    assert!(validator.is_valid(&json!({})));
    assert!(validator.is_valid(&json!({"album":null})));
    assert!(validator.is_valid(&json!({"album":"Альбом"})));
    assert!(!validator.is_valid(&json!({"album":7})));
    assert!(!validator.is_valid(&json!({"alubm":"typo"})));
}

#[test]
fn shared_playback_example_matches_schema() {
    let mut doc = openapi_document();
    doc["$ref"] = json!("#/components/schemas/PlaybackState");
    let validator = jsonschema::validator_for(&doc).unwrap();
    let mut example: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contract/playback-duplicates.json"
    ))
    .unwrap();
    assert!(validator.is_valid(&example));
    example["position_ms"] = json!(-1);
    assert!(!validator.is_valid(&example));
}

#[test]
fn library_list_response_is_a_page_not_a_single_track() {
    let mut doc = openapi_document();
    let reference = doc["paths"]["/api/v1/tracks"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["$ref"].clone();
    doc["$ref"] = reference;
    let validator = jsonschema::validator_for(&doc).unwrap();
    assert!(validator.is_valid(&json!({"items":[],"next_cursor":null})));
    assert!(!validator.is_valid(&json!({"items":"not a list","next_cursor":null})));
}

#[test]
fn shared_messages_round_trip_and_validate_against_published_schemas() {
    use dagaz_protocol::*;
    fn check<T: serde::de::DeserializeOwned + serde::Serialize>(
        name: &str,
        value: &serde_json::Value,
    ) {
        let model: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(model).unwrap(), *value, "{name}");
        let mut doc = openapi_document();
        doc["$ref"] = json!(format!("#/components/schemas/{name}"));
        assert!(
            jsonschema::validator_for(&doc).unwrap().is_valid(value),
            "schema rejected {name}"
        );
    }
    let messages: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/contract/messages.json")).unwrap();
    macro_rules! check { ($($t:ty),+ $(,)?) => { $(check::<$t>(stringify!($t), &messages[stringify!($t)]);)+ }; }
    check!(
        PairRequest,
        PairResponse,
        Playlist,
        MediaDescriptor,
        UploadState,
        Job,
        Change,
        SyncPage,
        MetadataEdit,
        PlaybackCommand,
        QueueUpdate,
        PlaybackReport,
        PlaybackAck
    );
    for (name, source) in [
        (
            "Track",
            include_str!("../../../fixtures/contract/track-null-tags.json"),
        ),
        (
            "ApiError",
            include_str!("../../../fixtures/contract/errors.json"),
        ),
    ] {
        let value = serde_json::from_str(source).unwrap();
        match name {
            "Track" => check::<Track>(name, &value),
            _ => check::<ApiError>(name, &value),
        }
    }
}

#[test]
fn checked_in_contract_is_current_and_all_schema_references_resolve() {
    let generated = openapi_document();
    let published: serde_json::Value =
        serde_json::from_str(include_str!("../../../api/openapi.yaml")).unwrap();
    assert_eq!(
        published, generated,
        "run cargo run -p dagaz-protocol --example export_openapi"
    );
    for name in generated["components"]["schemas"]
        .as_object()
        .unwrap()
        .keys()
    {
        let mut doc = generated.clone();
        doc["$ref"] = json!(format!("#/components/schemas/{name}"));
        jsonschema::validator_for(&doc).unwrap_or_else(|error| panic!("invalid {name}: {error}"));
    }
}

#[test]
fn device_listing_declares_cursor_and_limit() {
    let document = openapi_document();
    let parameters = document["paths"]["/api/v1/devices"]["get"]["parameters"]
        .as_array()
        .expect("device pagination parameters");
    let names: Vec<_> = parameters
        .iter()
        .map(|parameter| parameter["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["cursor", "limit"]);
}
