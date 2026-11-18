use crate::AppError;
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
}
impl Config {
    pub fn from_env() -> Result<Self, AppError> {
        let required = |name: &str| {
            std::env::var(name)
                .map_err(|_| AppError::invalid("Required S3 environment variable is missing"))
        };
        let config = Self {
            listen: std::env::var("DAGAZ_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .map_err(|_| AppError::invalid("DAGAZ_LISTEN must be an IP address and port"))?,
            data_dir: std::env::var_os("DAGAZ_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("data")),
            s3_endpoint: required("DAGAZ_S3_ENDPOINT")?,
            s3_bucket: required("DAGAZ_S3_BUCKET")?,
            s3_access_key: required("DAGAZ_S3_ACCESS_KEY")?,
            s3_secret_key: required("DAGAZ_S3_SECRET_KEY")?,
        };
        config.validate()?;
        Ok(config)
    }
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        let endpoint: axum::http::Uri = self
            .s3_endpoint
            .parse()
            .map_err(|_| AppError::invalid("Invalid S3 endpoint"))?;
        if !matches!(endpoint.scheme_str(), Some("http" | "https"))
            || endpoint.authority().is_none()
            || endpoint
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || endpoint
                .path_and_query()
                .is_some_and(|path| path.as_str() != "/")
        {
            return Err(AppError::invalid(
                "S3 endpoint must be an HTTP(S) origin without credentials or query",
            ));
        }
        if self.s3_bucket.len() < 3
            || self.s3_bucket.len() > 63
            || !self
                .s3_bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !self.s3_bucket.as_bytes()[0].is_ascii_alphanumeric()
            || !self.s3_bucket.as_bytes()[self.s3_bucket.len() - 1].is_ascii_alphanumeric()
        {
            return Err(AppError::invalid("Invalid S3 bucket name"));
        }
        if self.s3_access_key.is_empty()
            || self.s3_secret_key.is_empty()
            || self.data_dir.as_os_str().is_empty()
        {
            return Err(AppError::invalid(
                "S3 credentials and data directory must not be empty",
            ));
        }
        Ok(())
    }
}

pub fn create_s3(config: &Config) -> Result<Client, AppError> {
    config.validate()?;
    let sdk = aws_sdk_s3::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            config.s3_access_key.clone(),
            config.s3_secret_key.clone(),
            None,
            None,
            "dagaz",
        ))
        .endpoint_url(&config.s3_endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(2))
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(10))
                .operation_attempt_timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(2))
                .read_timeout(Duration::from_secs(5))
                .build(),
        )
        .build();
    Ok(Client::from_conf(sdk))
}

pub async fn open_database(config: &Config) -> Result<SqlitePool, AppError> {
    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .map_err(|_| {
            AppError::internal("Cannot create data directory; check filesystem permissions")
        })?;
    let options = SqliteConnectOptions::new()
        .filename(config.data_dir.join("dagaz.sqlite"))
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5));
    let db = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(|_| {
            AppError::internal("Cannot open SQLite database; check data directory permissions")
        })?;
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .map_err(|_| AppError::internal("SQLite migration failed"))?;
    Ok(db)
}
