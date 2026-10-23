//! Client for museum's fork worker routes (fork/docs/worker-api.md).

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const TOKEN_HEADER: &str = "X-Worker-Access-Token";
const CLIENT_PACKAGE: &str = "io.ente.fork.worker";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error at {path}: {source}")]
    Network {
        path: String,
        source: reqwest::Error,
    },
    #[error("HTTP {status} at {path}: {body}")]
    Http {
        status: u16,
        path: String,
        body: String,
    },
    #[error("bad response at {path}: {source}")]
    Parse {
        path: String,
        source: reqwest::Error,
    },
}

impl Error {
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    #[serde(rename = "userID")]
    pub user_id: i64,
    pub sealed_token: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: i64,
    #[serde(rename = "fileID")]
    pub file_id: i64,
    pub sealed_file_key: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttributes {
    #[serde(default)]
    pub encrypted_data: Option<String>,
    #[serde(default)]
    pub decryption_header: String,
    #[serde(default)]
    pub size: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MagicMetadata {
    pub version: i64,
    #[serde(default)]
    pub count: i64,
    pub data: String,
    pub header: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSizes {
    #[serde(default)]
    pub file_size: i64,
    #[serde(default, rename = "thumbSize")]
    pub thumbnail_size: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFile {
    pub id: i64,
    #[serde(rename = "ownerID")]
    pub owner_id: i64,
    pub file: FileAttributes,
    pub thumbnail: FileAttributes,
    pub metadata: FileAttributes,
    #[serde(default)]
    pub pub_magic_metadata: Option<MagicMetadata>,
    #[serde(default)]
    pub info: Option<FileSizes>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    #[serde(rename = "fileID")]
    pub file_id: i64,
    pub encrypted_data: String,
    pub decryption_header: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesDataResponse {
    #[serde(default)]
    pub data: Vec<FileData>,
    #[serde(default, rename = "pendingIndexFileIDs")]
    pub pending_index_file_ids: Vec<i64>,
    #[serde(default, rename = "errFileIDs")]
    pub err_file_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutFileData<'a> {
    #[serde(rename = "fileID")]
    pub file_id: i64,
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub encrypted_data: String,
    pub decryption_header: String,
    pub last_updated_at: i64,
}

#[derive(Deserialize)]
struct Pairings {
    #[serde(default)]
    pairings: Vec<Pairing>,
}

#[derive(Deserialize)]
struct Items {
    #[serde(default)]
    items: Vec<InboxItem>,
}

#[derive(Deserialize)]
struct FileEnvelope {
    file: RemoteFile,
}

#[derive(Deserialize)]
struct UrlEnvelope {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileIds<'a> {
    #[serde(rename = "fileIDs")]
    file_ids: &'a [i64],
}

/// Unauthenticated client: pairing and presigned object downloads.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
}

impl Client {
    pub fn new(endpoint: &str, timeout: Duration) -> reqwest::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("X-Client-Package", HeaderValue::from_static(CLIENT_PACKAGE));
        headers.insert(
            "X-Client-Version",
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        );
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()?;
        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn pairings(&self, worker_public_key: &str) -> Result<Vec<Pairing>> {
        let path = "/fork/worker/pairing";
        let req = self
            .http
            .get(format!("{}{path}", self.endpoint))
            .query(&[("workerPublicKey", worker_public_key)]);
        let p: Pairings = send_json(req, path).await?;
        Ok(p.pairings)
    }

    /// The worker routes for one user, with that user's worker token.
    pub fn for_token(&self, token: Zeroizing<String>) -> Result<WorkerClient> {
        let mut value = HeaderValue::from_str(&token).map_err(|_| Error::Http {
            status: 0,
            path: "token".to_owned(),
            body: "worker token is not a valid header value".to_owned(),
        })?;
        value.set_sensitive(true);
        Ok(WorkerClient {
            base: self.clone(),
            token: value,
        })
    }

    /// Downloads a presigned object URL (no Ente headers needed).
    pub async fn download(&self, url: &str, max_bytes: u64) -> Result<Zeroizing<Vec<u8>>> {
        let path = "object";
        let mut resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|source| Error::Network {
                path: path.to_owned(),
                source,
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                path: path.to_owned(),
                body: String::new(),
            });
        }
        if resp.content_length().is_some_and(|n| n > max_bytes) {
            return Err(too_big(path, max_bytes));
        }
        let mut out = Zeroizing::new(Vec::new());
        while let Some(chunk) = resp.chunk().await.map_err(|source| Error::Network {
            path: path.to_owned(),
            source,
        })? {
            if (out.len() + chunk.len()) as u64 > max_bytes {
                return Err(too_big(path, max_bytes));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

fn too_big(path: &str, max_bytes: u64) -> Error {
    Error::Http {
        status: 413,
        path: path.to_owned(),
        body: format!("object larger than {max_bytes} bytes"),
    }
}

/// The `/fork/worker-api` routes, authorised by one user's worker token.
#[derive(Clone)]
pub struct WorkerClient {
    base: Client,
    token: HeaderValue,
}

impl WorkerClient {
    fn url(&self, path: &str) -> String {
        format!("{}/fork/worker-api{path}", self.base.endpoint)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.base
            .http
            .get(self.url(path))
            .header(TOKEN_HEADER, self.token.clone())
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.base
            .http
            .post(self.url(path))
            .header(TOKEN_HEADER, self.token.clone())
    }

    pub fn base(&self) -> &Client {
        &self.base
    }

    pub async fn inbox(&self, since_id: i64, limit: usize) -> Result<Vec<InboxItem>> {
        let req = self.get("/inbox").query(&[
            ("sinceID", since_id.to_string()),
            ("limit", limit.to_string()),
        ]);
        let items: Items = send_json(req, "/inbox").await?;
        Ok(items.items)
    }

    pub async fn consume(&self, file_ids: &[i64]) -> Result<()> {
        let req = self.post("/inbox/consume").json(&FileIds { file_ids });
        send_ok(req, "/inbox/consume").await
    }

    pub async fn file_info(&self, file_id: i64) -> Result<RemoteFile> {
        let path = format!("/file-info/{file_id}");
        let env: FileEnvelope = send_json(self.get(&path), &path).await?;
        Ok(env.file)
    }

    pub async fn thumbnail_url(&self, file_id: i64) -> Result<String> {
        let path = format!("/files/thumbnail/v3/{file_id}");
        let env: UrlEnvelope = send_json(self.get(&path), &path).await?;
        Ok(env.url)
    }

    pub async fn file_url(&self, file_id: i64) -> Result<String> {
        let path = format!("/files/download/v3/{file_id}");
        let env: UrlEnvelope = send_json(self.get(&path), &path).await?;
        Ok(env.url)
    }

    pub async fn ml_data(&self, file_ids: &[i64]) -> Result<FilesDataResponse> {
        let req = self.post("/files/data/fetch").json(&FileIds { file_ids });
        send_json(req, "/files/data/fetch").await
    }

    pub async fn put_ml_data(&self, body: &PutFileData<'_>) -> Result<()> {
        let req = self
            .base
            .http
            .put(self.url("/files/data"))
            .header(TOKEN_HEADER, self.token.clone())
            .json(body);
        send_ok(req, "/files/data").await
    }
}

async fn send(req: reqwest::RequestBuilder, path: &str) -> Result<reqwest::Response> {
    let resp = req.send().await.map_err(|source| Error::Network {
        path: path.to_owned(),
        source,
    })?;
    let status = resp.status();
    if !status.is_success() {
        let mut body = resp.text().await.unwrap_or_default();
        body.truncate(512);
        return Err(Error::Http {
            status: status.as_u16(),
            path: path.to_owned(),
            body,
        });
    }
    Ok(resp)
}

async fn send_json<T: DeserializeOwned>(req: reqwest::RequestBuilder, path: &str) -> Result<T> {
    send(req, path)
        .await?
        .json()
        .await
        .map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })
}

async fn send_ok(req: reqwest::RequestBuilder, path: &str) -> Result<()> {
    send(req, path).await.map(|_| ())
}
