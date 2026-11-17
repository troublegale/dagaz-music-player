use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use stream_download::{
    Settings, StreamDownload,
    http::reqwest::{self, header::HeaderMap},
    http::{Client, HttpStream},
    source::DecodeError,
    storage::temp::TempStorageProvider,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

struct PinnedClient {
    client: reqwest::Client,
    etag: String,
    length: u64,
}

impl PinnedClient {
    async fn request(
        &self,
        url: &reqwest::Url,
        range: Option<(u64, Option<u64>)>,
    ) -> io::Result<reqwest::Response> {
        let mut request = self.client.get(url.clone()).header("If-Match", &self.etag);
        if let Some((start, end)) = range {
            request = request.header(
                "Range",
                stream_download::http::format_range_header_bytes(start, end),
            );
        }
        let response = request.send().await.map_err(io::Error::other)?;
        let expected_status = if range.is_some() { 206 } else { 200 };
        if response.status().as_u16() != expected_status
            || response.headers().get("ETag").and_then(|h| h.to_str().ok()) != Some(&self.etag)
        {
            return Err(io::Error::other(
                "server changed revision or ignored byte range",
            ));
        }
        let expected_length = if let Some((start, end)) = range {
            let end = end.unwrap_or(self.length - 1).min(self.length - 1);
            if start > end {
                return Err(io::Error::other("invalid byte range"));
            }
            let expected = format!("bytes {start}-{end}/{}", self.length);
            if response
                .headers()
                .get("Content-Range")
                .and_then(|h| h.to_str().ok())
                != Some(expected.as_str())
            {
                return Err(io::Error::other("server returned incorrect Content-Range"));
            }
            end - start + 1
        } else {
            self.length
        };
        if response.content_length() != Some(expected_length) {
            return Err(io::Error::other("server changed content length"));
        }
        Ok(response)
    }
}

impl Client for PinnedClient {
    type Url = reqwest::Url;
    type Headers = HeaderMap;
    type Response = reqwest::Response;
    type Error = io::Error;
    fn create() -> Self {
        panic!("PinnedClient requires a validated descriptor; use open_stream")
    }
    async fn get(&self, url: &Self::Url) -> io::Result<Self::Response> {
        self.request(url, None).await
    }
    async fn get_range(
        &self,
        url: &Self::Url,
        start: u64,
        end: Option<u64>,
    ) -> io::Result<Self::Response> {
        self.request(url, Some((start, end))).await
    }
}

pub async fn open_stream(
    url: &str,
    progress: Arc<AtomicU64>,
) -> Result<(StreamDownload<TempStorageProvider>, u64), Error> {
    let url: reqwest::Url = url.parse()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(Duration::from_secs(5))
        .build()?;
    let head = client.head(url.clone()).send().await?.error_for_status()?;
    let length = head
        .headers()
        .get("Content-Length")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or("finite Content-Length required")?;
    if head
        .headers()
        .get("Accept-Ranges")
        .and_then(|h| h.to_str().ok())
        != Some("bytes")
    {
        return Err("byte ranges required for seek/reconnect".into());
    }
    let etag = head
        .headers()
        .get("ETag")
        .and_then(|h| h.to_str().ok())
        .filter(|h| h.starts_with('"') && h.ends_with('"'))
        .ok_or("strong ETag required")?
        .to_owned();
    let source = HttpStream::new(
        PinnedClient {
            client,
            etag,
            length,
        },
        url,
    )
    .await
    .map_err(io::Error::other)?;
    let settings = Settings::default()
        .prefetch_bytes(128 * 1024)
        .retry_timeout(Duration::from_secs(2))
        .on_progress(move |_, state, _| {
            progress.store(state.current_position, Ordering::SeqCst);
        });
    let reader =
        match StreamDownload::from_stream(source, TempStorageProvider::new(), settings).await {
            Ok(reader) => reader,
            Err(error) => return Err(io::Error::other(error.decode_error().await).into()),
        };
    Ok((reader, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[tokio::test]
    async fn rejects_changed_revision_ignored_range_and_wrong_offsets() {
        for response in [
            "200 OK\r\nETag: \"one\"\r\nContent-Length: 10",
            "206 Partial Content\r\nETag: \"two\"\r\nContent-Range: bytes 2-5/10\r\nContent-Length: 4",
            "206 Partial Content\r\nETag: \"one\"\r\nContent-Range: bytes 3-6/10\r\nContent-Length: 4",
            "412 Precondition Failed\r\nContent-Length: 0",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/audio", listener.local_addr().unwrap())
                .parse()
                .unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 2048];
                let count = socket.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]).to_lowercase();
                assert!(request.contains("if-match: \"one\""));
                assert!(request.contains("range: bytes=2-5"));
                write!(socket, "HTTP/1.1 {response}\r\nConnection: close\r\n\r\n").unwrap();
            });
            let client = PinnedClient {
                client: reqwest::Client::new(),
                etag: "\"one\"".into(),
                length: 10,
            };
            assert!(client.get_range(&url, 2, Some(5)).await.is_err());
            server.join().unwrap();
        }
    }
}
