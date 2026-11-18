use dagaz_protocol::{Device, Page, PairRequest, PairResponse, Platform};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};
use url::Url;
use uuid::Uuid;

#[derive(Debug)]
pub enum ClientError {
    InvalidAddress,
    InvalidInput,
    PairingRequired,
    ServerUnavailable,
    InvalidResponse,
    Storage,
    AlreadyRunning,
    UnsupportedPlatform,
}
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
        Self::InvalidAddress=>"Укажи адрес HTTPS без логина, пути и параметров. HTTP доступен только на этом компьютере.",
        Self::InvalidInput=>"Проверь одноразовый код и имя устройства.",Self::PairingRequired=>"Нужна новая привязка. Проверь код или получи новый на сервере.",Self::ServerUnavailable=>"Сервер недоступен. Проверь адрес и подключение к сети.",Self::InvalidResponse=>"Сервер вернул неожиданный ответ.",Self::Storage=>"Не удалось прочитать или сохранить подключение.",Self::AlreadyRunning=>"Dagaz уже запущен для этого профиля.",Self::UnsupportedPlatform=>"Защищённое хранилище этого клиента требует Windows.",})
    }
}
impl std::error::Error for ClientError {}

fn validate_origin(url: &Url) -> Result<(), ClientError> {
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ClientError::InvalidAddress);
    }
    Ok(())
}
fn client() -> Result<reqwest::Client, ClientError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| ClientError::ServerUnavailable)
}
async fn response<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    expected: reqwest::StatusCode,
) -> Result<T, ClientError> {
    if matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return Err(ClientError::PairingRequired);
    }
    if response.status() != expected {
        return Err(ClientError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ClientError::ServerUnavailable)?
    {
        if bytes.len() + chunk.len() > 65536 {
            return Err(ClientError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
}
pub struct ServerConnection {
    base_url: Url,
    device_id: Uuid,
    token: String,
    http: reqwest::Client,
}
#[derive(Serialize, Deserialize)]
struct Saved {
    base_url: String,
    device_id: Uuid,
    encrypted_token: Vec<u8>,
}
impl ServerConnection {
    pub async fn pair(base_url: Url, code: &str, device_name: &str) -> Result<Self, ClientError> {
        validate_origin(&base_url)?;
        if code.is_empty()
            || code.len() > 128
            || code.chars().any(char::is_control)
            || device_name.trim().is_empty()
            || device_name.len() > 128
            || device_name.chars().any(char::is_control)
        {
            return Err(ClientError::InvalidInput);
        }
        let http = client()?;
        let result = http
            .post(
                base_url
                    .join("api/v1/pair")
                    .map_err(|_| ClientError::InvalidAddress)?,
            )
            .json(&PairRequest {
                pairing_code: code.into(),
                device_name: device_name.trim().into(),
                platform: Platform::Windows,
            })
            .send()
            .await
            .map_err(|_| ClientError::ServerUnavailable)?;
        let paired: PairResponse = response(result, reqwest::StatusCode::CREATED).await?;
        if !valid_token(&paired.token) {
            return Err(ClientError::InvalidResponse);
        }
        Ok(Self {
            base_url,
            device_id: paired.device_id,
            token: paired.token,
            http,
        })
    }
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }
    pub fn device_id(&self) -> Uuid {
        self.device_id
    }
    pub async fn check_access(&self) -> Result<(), ClientError> {
        let result = self
            .http
            .get(
                self.base_url
                    .join("api/v1/devices?limit=1")
                    .map_err(|_| ClientError::InvalidAddress)?,
            )
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| ClientError::ServerUnavailable)?;
        let _: Page<Device> = response(result, reqwest::StatusCode::OK).await?;
        Ok(())
    }
    pub fn save(&self, profile: &Path) -> Result<(), ClientError> {
        let encrypted_token = crate::platform::protect(self.token.as_bytes())?;
        let saved = Saved {
            base_url: self.base_url.as_str().into(),
            device_id: self.device_id,
            encrypted_token,
        };
        std::fs::create_dir_all(profile).map_err(|_| ClientError::Storage)?;
        let mut file =
            tempfile::NamedTempFile::new_in(profile).map_err(|_| ClientError::Storage)?;
        file.write_all(&serde_json::to_vec(&saved).map_err(|_| ClientError::Storage)?)
            .map_err(|_| ClientError::Storage)?;
        file.as_file()
            .sync_all()
            .map_err(|_| ClientError::Storage)?;
        file.persist(profile.join("connection.json"))
            .map_err(|_| ClientError::Storage)?;
        Ok(())
    }
    pub fn restore(profile: &Path) -> Result<Option<Self>, ClientError> {
        let file = match std::fs::File::open(profile.join("connection.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ClientError::Storage),
        };
        let mut bytes = Vec::new();
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| ClientError::Storage)?;
        if bytes.len() > 65536 {
            return Err(ClientError::PairingRequired);
        }
        let saved: Saved =
            serde_json::from_slice(&bytes).map_err(|_| ClientError::PairingRequired)?;
        let base_url = Url::parse(&saved.base_url).map_err(|_| ClientError::InvalidAddress)?;
        validate_origin(&base_url)?;
        let token = String::from_utf8(crate::platform::unprotect(&saved.encrypted_token)?)
            .map_err(|_| ClientError::PairingRequired)?;
        if !valid_token(&token) {
            return Err(ClientError::PairingRequired);
        }
        Ok(Some(Self {
            base_url,
            device_id: saved.device_id,
            token,
            http: client()?,
        }))
    }
}
fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}
