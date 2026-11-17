use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use std::{
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn multipart_and_revision_ranges() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::var("DAGAZ_TEST_S3_ENDPOINT")?;
    assert!(
        endpoint.starts_with("http://127.0.0.1:"),
        "probe must target its loopback service"
    );
    let credentials = Credentials::new(
        std::env::var("DAGAZ_TEST_S3_ACCESS_KEY")?,
        std::env::var("DAGAZ_TEST_S3_SECRET_KEY")?,
        None,
        None,
        "dagaz-test",
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version_latest()
        .timeout_config(
            aws_sdk_s3::config::timeout::TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(10))
                .operation_attempt_timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(2))
                .read_timeout(Duration::from_secs(5))
                .build(),
        )
        .region(Region::new("us-east-1"))
        .credentials_provider(credentials)
        .endpoint_url(endpoint)
        .force_path_style(true)
        .build();
    let client = Client::from_conf(config);
    for _ in 0..40 {
        if client.list_buckets().send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let bucket = format!(
        "dagaz-probe-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    client.create_bucket().bucket(&bucket).send().await?;
    let first = client
        .put_object()
        .bucket(&bucket)
        .key("track/1/original")
        .body(ByteStream::from_static(b"abcdefghij"))
        .send()
        .await?;
    let etag = first.e_tag().ok_or("missing ETag")?.to_string();
    let range = client
        .get_object()
        .bucket(&bucket)
        .key("track/1/original")
        .range("bytes=2-5")
        .if_match(&etag)
        .send()
        .await?;
    assert_eq!(range.content_range(), Some("bytes 2-5/10"));
    assert_eq!(range.body.collect().await?.into_bytes().as_ref(), b"cdef");
    client
        .put_object()
        .bucket(&bucket)
        .key("track/2/original")
        .body(ByteStream::from_static(b"new revision"))
        .send()
        .await?;
    let old = client
        .get_object()
        .bucket(&bucket)
        .key("track/1/original")
        .send()
        .await?;
    assert_eq!(
        old.body.collect().await?.into_bytes().as_ref(),
        b"abcdefghij"
    );
    let mismatch = client
        .get_object()
        .bucket(&bucket)
        .key("track/2/original")
        .if_match(&etag)
        .send()
        .await
        .unwrap_err();
    assert_eq!(
        mismatch.raw_response().map(|r| r.status().as_u16()),
        Some(412)
    );

    let pending = client
        .create_multipart_upload()
        .bucket(&bucket)
        .key("multipart")
        .send()
        .await?;
    let upload_id = pending.upload_id().ok_or("missing upload ID")?;
    let part1 = client
        .upload_part()
        .bucket(&bucket)
        .key("multipart")
        .upload_id(upload_id)
        .part_number(1)
        .body(ByteStream::from(vec![b'A'; 5 * 1024 * 1024]))
        .send()
        .await?;
    assert_eq!(
        client
            .list_parts()
            .bucket(&bucket)
            .key("multipart")
            .upload_id(upload_id)
            .send()
            .await?
            .parts()
            .len(),
        1
    );
    let invisible = client
        .get_object()
        .bucket(&bucket)
        .key("multipart")
        .send()
        .await
        .unwrap_err();
    assert_eq!(
        invisible.raw_response().map(|r| r.status().as_u16()),
        Some(404)
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("docker")
        .current_dir(&root)
        .args([
            "compose",
            "--env-file",
            "deploy/.env.test",
            "-f",
            "deploy/compose.test.yaml",
            "restart",
            "rustfs",
        ])
        .status()?;
    assert!(status.success());
    for _ in 0..40 {
        if client.list_buckets().send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        client
            .list_parts()
            .bucket(&bucket)
            .key("multipart")
            .upload_id(upload_id)
            .send()
            .await?
            .parts()
            .len(),
        1
    );
    let restored = client
        .get_object()
        .bucket(&bucket)
        .key("track/1/original")
        .send()
        .await?;
    assert_eq!(
        restored.body.collect().await?.into_bytes().as_ref(),
        b"abcdefghij"
    );
    let part2 = client
        .upload_part()
        .bucket(&bucket)
        .key("multipart")
        .upload_id(upload_id)
        .part_number(2)
        .body(ByteStream::from_static(b"abcdefghij"))
        .send()
        .await?;
    let parts = vec![
        CompletedPart::builder()
            .part_number(1)
            .e_tag(part1.e_tag().ok_or("no first ETag")?)
            .build(),
        CompletedPart::builder()
            .part_number(2)
            .e_tag(part2.e_tag().ok_or("no second ETag")?)
            .build(),
    ];
    client
        .complete_multipart_upload()
        .bucket(&bucket)
        .key("multipart")
        .upload_id(upload_id)
        .multipart_upload(
            CompletedMultipartUpload::builder()
                .set_parts(Some(parts))
                .build(),
        )
        .send()
        .await?;
    let result = client
        .get_object()
        .bucket(&bucket)
        .key("multipart")
        .range(format!(
            "bytes={}-{}",
            5 * 1024 * 1024 + 2,
            5 * 1024 * 1024 + 5
        ))
        .send()
        .await?;
    assert_eq!(result.body.collect().await?.into_bytes().as_ref(), b"cdef");
    let abandoned = client
        .create_multipart_upload()
        .bucket(&bucket)
        .key("abandoned")
        .send()
        .await?;
    client
        .upload_part()
        .bucket(&bucket)
        .key("abandoned")
        .upload_id(abandoned.upload_id().ok_or("no abandoned upload ID")?)
        .part_number(1)
        .body(ByteStream::from_static(b"orphan part"))
        .send()
        .await?;
    client
        .abort_multipart_upload()
        .bucket(&bucket)
        .key("abandoned")
        .upload_id(abandoned.upload_id().ok_or("no abandoned upload ID")?)
        .send()
        .await?;
    assert!(
        client
            .list_multipart_uploads()
            .bucket(&bucket)
            .send()
            .await?
            .uploads()
            .is_empty()
    );
    let objects = client.list_objects_v2().bucket(&bucket).send().await?;
    for object in objects.contents() {
        client
            .delete_object()
            .bucket(&bucket)
            .key(object.key().ok_or("missing key")?)
            .send()
            .await?;
    }
    client.delete_bucket().bucket(&bucket).send().await?;
    Ok(())
}
