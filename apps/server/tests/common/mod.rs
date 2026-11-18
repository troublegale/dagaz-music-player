use dagaz_protocol::{PairRequest, PairResponse, Platform};
use dagaz_server::{Config, build_app, create_s3, open_database};
use sqlx::SqlitePool;
use std::{net::SocketAddr, path::PathBuf};
use tokio::{sync::oneshot, task::JoinHandle};

pub struct TestServer {
    pub url: String,
    pub http: reqwest::Client,
    pub token: String,
    pub config: Config,
    pub db: SqlitePool,
    pub s3: aws_sdk_s3::Client,
    task: Option<JoinHandle<()>>,
    stop: Option<oneshot::Sender<()>>,
    _directory: tempfile::TempDir,
}

impl TestServer {
    pub async fn spawn() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let secrets = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/.env.test"),
        )
        .expect("run experiments/storage/run.ps1 once to initialize isolated RustFS credentials");
        let value = |name: &str| {
            std::env::var(name)
                .ok()
                .or_else(|| {
                    secrets
                        .lines()
                        .filter_map(|line| line.split_once('='))
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| value.to_owned())
                })
                .expect("missing test S3 setting")
        };
        let endpoint = std::env::var("DAGAZ_TEST_S3_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:19000".into());
        assert!(
            endpoint.starts_with("http://127.0.0.1:"),
            "tests require isolated loopback S3"
        );
        let config = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: directory.path().to_owned(),
            s3_endpoint: endpoint,
            s3_bucket: format!("dagaz-auth-{}", uuid::Uuid::new_v4()),
            s3_access_key: value("DAGAZ_TEST_S3_ACCESS_KEY"),
            s3_secret_key: value("DAGAZ_TEST_S3_SECRET_KEY"),
        };
        let s3 = create_s3(&config).unwrap();
        s3.create_bucket()
            .bucket(&config.s3_bucket)
            .send()
            .await
            .unwrap();
        let db = open_database(&config).await.unwrap();
        let mut server = Self {
            url: String::new(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            token: String::new(),
            config,
            db,
            s3,
            task: None,
            stop: None,
            _directory: directory,
        };
        server.start_http().await;
        let code = dagaz_server::new_pairing_code(&server.db).await.unwrap();
        let response = server
            .http
            .post(format!("{}/api/v1/pair", server.url))
            .json(&PairRequest {
                pairing_code: code,
                device_name: "Bootstrap".into(),
                platform: Platform::Windows,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        server.token = response.json::<PairResponse>().await.unwrap().token;
        server
    }
    async fn start_http(&mut self) {
        let app = build_app(self.config.clone(), self.db.clone(), self.s3.clone())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind(self.config.listen)
            .await
            .unwrap();
        self.url = format!("http://{}", listener.local_addr().unwrap());
        let (send, receive) = oneshot::channel();
        self.stop = Some(send);
        self.task = Some(tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = receive.await;
            })
            .await
            .unwrap();
        }));
    }
    async fn stop_http(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
    pub async fn restart(&mut self) {
        self.stop_http().await;
        self.db.close().await;
        self.db = open_database(&self.config).await.unwrap();
        self.start_http().await;
    }
    pub async fn finish(mut self) {
        self.stop_http().await;
        self.db.close().await;
        self.s3
            .delete_bucket()
            .bucket(&self.config.s3_bucket)
            .send()
            .await
            .unwrap();
    }
}
impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
