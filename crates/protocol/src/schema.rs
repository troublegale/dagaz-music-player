use crate::*;
use schemars::JsonSchema;
use serde_json::{Map, Value, json};

fn register<T: JsonSchema>(schemas: &mut Map<String, Value>) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).expect("schema is JSON");
    if let Some(Value::Object(definitions)) = schema.as_object_mut().unwrap().remove("$defs") {
        schemas.extend(definitions);
    }
    schema.as_object_mut().unwrap().remove("$schema");
    let name = T::schema_name().into_owned();
    schemas.insert(name.clone(), schema);
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// Generate OpenAPI components from the same Rust types used on the wire.
pub fn openapi_document() -> Value {
    let mut schemas = Map::new();
    macro_rules! types { ($($t:ty),+ $(,)?) => { $(register::<$t>(&mut schemas);)+ }; }
    types!(
        PairRequest,
        PairResponse,
        ApiError,
        Track,
        Playlist,
        MediaDescriptor,
        UploadState,
        Job,
        MetadataPatch,
        MetadataEdit,
        SyncPage,
        PlaybackState,
        PlaybackCommand,
        PlaybackReport,
        PlaybackAck,
        QueueUpdate,
        Page<Track>,
        Page<Album>,
        Page<Artist>,
        Page<Playlist>,
        Page<Device>,
        UploadCreate,
        RevisionRequest,
        PlaylistCreate,
        PlaylistRename,
        PlaylistTracks,
        MetadataBatch,
        ManifestCreate,
        ManifestCreated,
        ManifestPage,
        ArtworkCreated,
        SnapshotPage
    );
    for (alias, actual) in [
        ("TrackPage", <Page<Track>>::schema_name()),
        ("AlbumPage", <Page<Album>>::schema_name()),
        ("ArtistPage", <Page<Artist>>::schema_name()),
        ("PlaylistPage", <Page<Playlist>>::schema_name()),
        ("DevicePage", <Page<Device>>::schema_name()),
    ] {
        schemas.insert(
            alias.into(),
            json!({"$ref":format!("#/components/schemas/{actual}")}),
        );
    }
    let mut paths = Map::<String, Value>::new();
    let routes = [
        ("/pair", "post", "PairResponse"),
        ("/artworks", "post", "ArtworkCreated"),
        ("/download-manifests", "post", "ManifestCreated"),
        ("/download-manifests/{id}", "get", "ManifestPage"),
        ("/devices", "get", "DevicePage"),
        ("/devices/{id}", "delete", ""),
        ("/tracks", "get", "TrackPage"),
        ("/albums", "get", "AlbumPage"),
        ("/artists", "get", "ArtistPage"),
        ("/albums/{id}/tracks", "get", "TrackPage"),
        ("/tracks/{id}", "get", "Track"),
        ("/uploads", "post", "UploadState"),
        ("/uploads/{id}", "get", "UploadState"),
        ("/uploads/{id}", "patch", "UploadState"),
        ("/uploads/{id}/complete", "post", "Job"),
        ("/jobs/{id}", "get", "Job"),
        ("/tracks/{id}/metadata", "patch", "Job"),
        ("/tracks/metadata-batch", "post", "Job"),
        ("/tracks/{id}", "delete", ""),
        ("/playlists", "get", "PlaylistPage"),
        ("/playlists", "post", "Playlist"),
        ("/playlists/{id}", "get", "Playlist"),
        ("/playlists/{id}", "patch", "Playlist"),
        ("/playlists/{id}", "delete", ""),
        ("/playlists/{id}/tracks", "put", "Playlist"),
        ("/snapshot", "get", "SnapshotPage"),
        ("/changes", "get", "SyncPage"),
        ("/playback", "get", "PlaybackState"),
        ("/playback/commands", "post", "PlaybackState"),
        ("/playback/reports", "post", "PlaybackState"),
        ("/playback/acks", "post", "PlaybackState"),
    ];
    for (path, method, response) in routes {
        let mut operation = json!({
            "responses": {"200": {"description":"Success", "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{response}")}}}},
            "default":{"description":"Error","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}}
        });
        let success_status = if matches!(
            path,
            "/uploads/{id}/complete" | "/tracks/{id}/metadata" | "/tracks/metadata-batch"
        ) {
            "202"
        } else if method == "post"
            && matches!(
                path,
                "/pair" | "/uploads" | "/artworks" | "/playlists" | "/download-manifests"
            )
        {
            "201"
        } else {
            "200"
        };
        if success_status != "200" {
            let success = operation["responses"]
                .as_object_mut()
                .unwrap()
                .remove("200")
                .unwrap();
            operation["responses"][success_status] = success;
        }
        if response.is_empty() {
            operation["responses"]
                .as_object_mut()
                .unwrap()
                .remove("200");
            operation["responses"]["204"] = json!({"description":"Deleted"});
        }
        if path.contains("{id}") {
            operation["parameters"] = json!([{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}]);
        }
        if path == "/pair" {
            operation["security"] = json!([]);
        }
        let request = match path {
            "/pair" => Some("PairRequest"),
            "/uploads" => Some("UploadCreate"),
            "/tracks/metadata-batch" => Some("MetadataBatch"),
            "/download-manifests" => Some("ManifestCreate"),
            "/playlists" if method == "post" => Some("PlaylistCreate"),
            "/playlists/{id}" if method == "patch" => Some("PlaylistRename"),
            "/playlists/{id}/tracks" => Some("PlaylistTracks"),
            _ if method == "delete" && path != "/devices/{id}" => Some("RevisionRequest"),
            "/playback/commands" => Some("PlaybackCommand"),
            "/playback/reports" => Some("PlaybackReport"),
            "/playback/acks" => Some("PlaybackAck"),
            "/tracks/{id}/metadata" => Some("MetadataEdit"),
            _ => None,
        };
        if let Some(request) = request {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}});
        }
        paths
            .entry(format!("/api/v1{path}"))
            .or_insert_with(|| json!({}))[method] = operation;
    }
    for (path, item) in &mut paths {
        for (method, operation) in item.as_object_mut().unwrap() {
            if !matches!(method.as_str(), "get" | "head")
                && !path.ends_with("/pair")
                && !path.contains("/playback/")
            {
                let parameters = operation
                    .as_object_mut()
                    .unwrap()
                    .entry("parameters")
                    .or_insert_with(|| json!([]));
                parameters.as_array_mut().unwrap().push(json!({"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if method == "get"
                && matches!(
                    path.as_str(),
                    "/api/v1/devices"
                        | "/api/v1/tracks"
                        | "/api/v1/albums"
                        | "/api/v1/artists"
                        | "/api/v1/playlists"
                        | "/api/v1/albums/{id}/tracks"
                        | "/api/v1/download-manifests/{id}"
                        | "/api/v1/snapshot"
                        | "/api/v1/changes"
                )
            {
                let parameters = operation
                    .as_object_mut()
                    .unwrap()
                    .entry("parameters")
                    .or_insert_with(|| json!([]));
                let names: &[&str] = if path == "/api/v1/devices" {
                    &["cursor"]
                } else {
                    &["q", "sort", "cursor"]
                };
                for name in names {
                    parameters
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"name":name,"in":"query","schema":{"type":"string"}}));
                }
                parameters.as_array_mut().unwrap().push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":500,"default":100}}));
            }
        }
    }
    paths["/api/v1/uploads/{id}"]["patch"]["requestBody"] = json!({"required":true,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary","maxLength":8388608}}}});
    for name in ["Upload-Offset", "X-Chunk-SHA256"] {
        paths["/api/v1/uploads/{id}"]["patch"]["parameters"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":name,"in":"header","required":true,"schema":{"type":"string"}}));
    }
    paths["/api/v1/artworks"]["post"]["requestBody"] = json!({"required":true,"content":{"image/jpeg":{"schema":{"type":"string","format":"binary"}},"image/png":{"schema":{"type":"string","format":"binary"}}}});
    for (path, ranged) in [
        ("/api/v1/tracks/{id}/media", true),
        ("/api/v1/artworks/{id}", false),
    ] {
        let mut parameters = vec![
            json!({"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        if ranged {
            parameters.extend([
                json!({"name":"revision","in":"query","required":true,"schema":{"type":"integer","minimum":1}}),
                json!({"name":"quality","in":"query","required":true,"schema":{"$ref":"#/components/schemas/Quality"}}),
                json!({"name":"Range","in":"header","schema":{"type":"string"}}),
                json!({"name":"If-Range","in":"header","schema":{"type":"string"}}),
            ]);
        }
        let headers = json!({"ETag":{"schema":{"type":"string"}},"Content-Length":{"schema":{"type":"integer","minimum":0}},"Content-Range":{"schema":{"type":"string"}},"Accept-Ranges":{"schema":{"type":"string"}}});
        let full = json!({"description":"File bytes","headers":headers,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}});
        let mut responses = json!({"200":full,"default":{"description":"Error","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}});
        if ranged {
            responses["206"] = full;
            for (status, description) in [
                ("410", "Revision removed"),
                ("416", "Invalid range"),
                ("425", "Representation preparing; job_id in error"),
            ] {
                responses[status] = json!({"description":description,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}});
            }
        }
        paths.insert(
            path.into(),
            json!({"get":{"parameters":parameters,"responses":responses}}),
        );
        if ranged {
            let mut head = paths[path]["get"].clone();
            for response in head["responses"].as_object_mut().unwrap().values_mut() {
                response.as_object_mut().unwrap().remove("content");
            }
            paths[path]["head"] = head;
        }
    }
    paths.insert("/api/v1/events".into(), json!({"get":{"responses":{"101":{"description":"Authenticated WebSocket: catalog_changed, job_changed, playback_state, prepare, stop, start, device_revoked"}}}}));
    paths.insert(
        "/healthz".into(),
        json!({"get":{"security":[],"responses":{"200":{"description":"Healthy"}}}}),
    );
    let document = json!({"openapi":"3.1.0","info":{"title":"Dagaz Music Player API","version":"0.1.0"},
        "security":[{"deviceToken":[]}],"paths":paths,
        "components":{"securitySchemes":{"deviceToken":{"type":"http","scheme":"bearer"}},"schemas":schemas}});
    // Schemars emits local $defs references; OpenAPI stores them under components.
    serde_json::from_str(
        &document
            .to_string()
            .replace("#/$defs/", "#/components/schemas/"),
    )
    .expect("schema is JSON")
}
