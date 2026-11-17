use dagaz_windows_audio_probe::open_stream;
use rodio::Source;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[tokio::test(flavor = "multi_thread")]
async fn starts_before_eof_and_resumes_same_revision() {
    let progress = Arc::new(AtomicU64::new(0));
    let url = format!(
        "http://127.0.0.1:18888/long.wav?drop=1&windows={}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let (reader, length) = open_stream(&url, progress.clone()).await.unwrap();
    tokio::task::spawn_blocking(move || {
        let mut decoder = rodio::Decoder::builder()
            .with_data(reader)
            .with_byte_len(length)
            .with_seekable(true)
            .build()
            .unwrap();
        assert!(decoder.next().is_some());
        assert!(
            progress.load(Ordering::SeqCst) < length,
            "decoder waited for full download"
        );
        // Reading the whole source crosses the injected disconnect at 512 KiB.
        assert_eq!(decoder.count() + 1, 30 * 44100);
    })
    .await
    .unwrap();

    let (reader, length) = open_stream(
        "http://127.0.0.1:18888/long.wav",
        Arc::new(AtomicU64::new(0)),
    )
    .await
    .unwrap();
    tokio::task::spawn_blocking(move || {
        let mut decoder = rodio::Decoder::builder()
            .with_data(reader)
            .with_byte_len(length)
            .with_seekable(true)
            .build()
            .unwrap();
        decoder.try_seek(Duration::from_secs(25)).unwrap();
        assert!(
            decoder.count().abs_diff(5 * 44100) <= 44100 / 2,
            "seek error exceeds 500ms"
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn network_vbr_mp3_and_flac_decode() {
    for name in ["vbr.mp3", "tone.flac"] {
        let (reader, length) = open_stream(
            &format!("http://127.0.0.1:18888/{name}"),
            Arc::new(AtomicU64::new(0)),
        )
        .await
        .unwrap();
        tokio::task::spawn_blocking(move || {
            let decoder = rodio::Decoder::builder()
                .with_data(reader)
                .with_byte_len(length)
                .with_seekable(true)
                .build()
                .unwrap();
            assert!(decoder.count() > 0);
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn rejects_unknown_length_and_unseekable_sources_promptly() {
    for headers in [
        "Accept-Ranges: bytes\r\nETag: \"one\"\r\n",
        "Content-Length: 100\r\nETag: \"one\"\r\n",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/audio", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
            )
            .unwrap();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            open_stream(&url, Arc::new(AtomicU64::new(0))),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        server.join().unwrap();
    }
}
