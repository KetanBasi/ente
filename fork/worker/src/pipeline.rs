// One poll of the worker: pairings, inbox, decrypt, CPU stages, mldata
// merge, consume.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ente_core::crypto::{Header, Key, blob, stream};
use futures_util::StreamExt;
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::api::{self, Client, FileData, InboxItem, PutFileData, RemoteFile, WorkerClient};
use crate::config::Config;
use crate::identity::WorkerIdentity;
use crate::mldata::{self, Record};
use crate::secure_mem::{self, KeySlab, Slot};
use crate::stages;

// Ente's metadata `fileType` for still images (0) and live photos (2).
const FILE_TYPE_IMAGE: i64 = 0;
const MAX_THUMBNAIL_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Api(#[from] api::Error),
    #[error(transparent)]
    Memory(#[from] secure_mem::Error),
    #[error(transparent)]
    MlData(#[from] mldata::Error),
    #[error("decrypt: {0}")]
    Crypto(#[from] ente_core::crypto::Error),
    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub users: usize,
    pub written: usize,
    pub already_done: usize,
    pub deferred: usize,
    pub failed: usize,
    pub unopenable: usize,
}

impl std::ops::AddAssign for Summary {
    fn add_assign(&mut self, o: Self) {
        self.users += o.users;
        self.written += o.written;
        self.already_done += o.already_done;
        self.deferred += o.deferred;
        self.failed += o.failed;
        self.unopenable += o.unopenable;
    }
}

enum Outcome {
    Written,
    AlreadyDone,
    Deferred(String),
}

pub struct Worker {
    config: Config,
    identity: WorkerIdentity,
    client: Client,
    slab: KeySlab,
}

impl Worker {
    pub fn new(config: Config, identity: WorkerIdentity) -> anyhow::Result<Self> {
        let client = Client::new(&config.endpoint, Duration::from_secs(config.timeout_secs))?;
        let slab = KeySlab::new(
            config.runtime.batch_size,
            secure_mem::Options {
                require_memfd_secret: config.memory.require_memfd_secret,
                ..Default::default()
            },
        )?;
        log::info!(
            "worker key {} ({} key slots, {:?} memory)",
            identity.public_key_b64(),
            slab.capacity(),
            slab.backing()
        );
        Ok(Self {
            config,
            identity,
            client,
            slab,
        })
    }

    // Processes every user's inbox once.
    pub async fn run_once(&self) -> Result<Summary> {
        let mut total = Summary::default();
        for pairing in self.client.pairings(self.identity.public_key_b64()).await? {
            let token = match self.open_token(&pairing.sealed_token) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("user {}: cannot open worker token: {e}", pairing.user_id);
                    continue;
                }
            };
            let wc = self.client.for_token(token)?;
            match self.drain(&wc).await {
                Ok(mut s) => {
                    s.users = 1;
                    total += s;
                }
                Err(e) => log::error!("user {}: {e}", pairing.user_id),
            }
        }
        Ok(total)
    }

    fn open_token(&self, sealed_b64: &str) -> Result<Zeroizing<String>> {
        let sealed = STANDARD
            .decode(sealed_b64)
            .map_err(|_| Error::Other("sealed token is not base64".to_owned()))?;
        let raw = self
            .identity
            .open(&sealed)
            .map_err(|e| Error::Other(e.to_string()))?;
        let text =
            std::str::from_utf8(&raw).map_err(|_| Error::Other("token is not UTF-8".to_owned()))?;
        Ok(Zeroizing::new(text.to_owned()))
    }

    async fn drain(&self, wc: &WorkerClient) -> Result<Summary> {
        let mut summary = Summary::default();
        let mut since = 0;
        loop {
            let items = wc.inbox(since, self.config.runtime.batch_size).await?;
            let Some(last) = items.last() else { break };
            since = last.id;
            summary += self.process_batch(wc, items).await?;
        }
        Ok(summary)
    }

    async fn process_batch(&self, wc: &WorkerClient, items: Vec<InboxItem>) -> Result<Summary> {
        let mut summary = Summary::default();
        let mut consume = Vec::new();
        let mut jobs = Vec::new();
        for item in items {
            match self.open_file_key(&item.sealed_file_key) {
                Ok(slot) => jobs.push((item.file_id, slot)),
                Err(e) => {
                    // Sealed to an old keypair, or corrupt: it can never be
                    // opened, so drop it from the inbox instead of retrying.
                    log::warn!(
                        "file {}: cannot open sealed fileKey ({e}); dropping it",
                        item.file_id
                    );
                    summary.unopenable += 1;
                    consume.push(item.file_id);
                }
            }
        }
        let ids: Vec<i64> = jobs.iter().map(|(id, _)| *id).collect();
        let fetched = if ids.is_empty() {
            Ok(api::FilesDataResponse::default())
        } else {
            wc.ml_data(&ids).await
        };
        let outcomes: Vec<(i64, Slot, Result<Outcome>)> = match fetched {
            Ok(fetched) => {
                let data = Arc::new(fetched.data);
                futures_util::stream::iter(jobs.into_iter().map(|(id, slot)| {
                    let data = Arc::clone(&data);
                    async move {
                        let existing = data.iter().find(|d| d.file_id == id);
                        let r = self.process_file(wc, id, &slot, existing).await;
                        (id, slot, r)
                    }
                }))
                .buffer_unordered(self.config.runtime.concurrency)
                .collect()
                .await
            }
            Err(e) => {
                // Wipe the keys before giving up on the batch.
                for (_, slot) in jobs {
                    self.slab.remove(slot)?;
                }
                return Err(e.into());
            }
        };
        for (id, slot, r) in outcomes {
            self.slab.remove(slot)?;
            match r {
                Ok(Outcome::Written) => {
                    summary.written += 1;
                    consume.push(id);
                }
                Ok(Outcome::AlreadyDone) => {
                    summary.already_done += 1;
                    consume.push(id);
                }
                Ok(Outcome::Deferred(why)) => {
                    log::info!("file {id}: deferred, {why}");
                    summary.deferred += 1;
                }
                Err(e) => {
                    log::warn!("file {id}: failed, will retry: {e}");
                    summary.failed += 1;
                }
            }
        }
        if !consume.is_empty() {
            wc.consume(&consume).await?;
        }
        Ok(summary)
    }

    fn open_file_key(&self, sealed_b64: &str) -> Result<Slot> {
        let sealed = STANDARD
            .decode(sealed_b64)
            .map_err(|_| Error::Other("sealed fileKey is not base64".to_owned()))?;
        let mut key = self
            .identity
            .open(&sealed)
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(self.slab.insert(&mut key)?)
    }

    fn with_key<R>(&self, slot: &Slot, f: impl FnOnce(&Key) -> R) -> Result<R> {
        Ok(self.slab.with_key(slot, f)?)
    }

    async fn process_file(
        &self,
        wc: &WorkerClient,
        file_id: i64,
        slot: &Slot,
        existing: Option<&FileData>,
    ) -> Result<Outcome> {
        let record = match existing {
            Some(d) => self.with_key(slot, |k| {
                Record::decrypt(&d.encrypted_data, &d.decryption_header, d.updated_at, k)
            })??,
            None => Record::empty(),
        };
        if let Err(why) = record.ready(&self.config.mldata) {
            return Ok(Outcome::Deferred(why.to_string()));
        }
        if record
            .fields
            .get(mldata::NAMESPACE)
            .and_then(|v| v.get("version"))
            .and_then(Value::as_u64)
            == Some(stages::SCHEMA_VERSION)
        {
            return Ok(Outcome::AlreadyDone);
        }

        let file = wc.file_info(file_id).await?;
        let metadata = self.with_key(slot, |k| {
            decrypt_json(
                file.metadata.encrypted_data.as_deref(),
                &file.metadata.decryption_header,
                k,
            )
        })??;
        let pub_magic = match &file.pub_magic_metadata {
            Some(m) => self.with_key(slot, |k| decrypt_json(Some(&m.data), &m.header, k))??,
            None => Map::new(),
        };

        let thumb_url = wc.thumbnail_url(file_id).await?;
        let enc_thumb = wc.base().download(&thumb_url, MAX_THUMBNAIL_BYTES).await?;
        let thumb = self.with_key(slot, |k| {
            decrypt_stream(&enc_thumb, &file.thumbnail.decryption_header, k)
        })??;
        drop(enc_thumb);

        let original = self.original(wc, &file, &metadata, slot).await?;

        let stages_cfg = self.config.stages;
        let kinds = self.config.pii_kinds.clone();
        let enrichment = tokio::task::spawn_blocking(move || {
            analyse(
                &stages_cfg,
                &kinds,
                &metadata,
                &pub_magic,
                &thumb,
                original.as_ref().map(|o| o.as_slice()),
            )
        })
        .await
        .map_err(|e| Error::Other(format!("analysis task: {e}")))?;

        let merged = record.with_enrichment(enrichment);
        let (encrypted_data, decryption_header) = self.with_key(slot, |k| merged.encrypt(k))??;
        let put = PutFileData {
            file_id,
            kind: "mldata",
            encrypted_data,
            decryption_header,
            last_updated_at: record.updated_at,
        };
        match wc.put_ml_data(&put).await {
            Ok(()) => Ok(Outcome::Written),
            // Someone wrote the record after we read it: never overwrite,
            // retry from the new record on the next poll.
            Err(e) if e.status() == Some(409) => {
                Ok(Outcome::Deferred("mldata changed since read".to_owned()))
            }
            Err(e) => Err(e.into()),
        }
    }

    // The decrypted original, for EXIF, when it is a still image under the
    // size cap and a stage needs it.
    async fn original(
        &self,
        wc: &WorkerClient,
        file: &RemoteFile,
        metadata: &Map<String, Value>,
        slot: &Slot,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        if !self.config.stages.device
            || metadata.get("fileType").and_then(Value::as_i64) != Some(FILE_TYPE_IMAGE)
        {
            return Ok(None);
        }
        let max = self.config.runtime.max_original_mb * 1024 * 1024;
        let size = file.info.as_ref().map_or(0, |i| i.file_size);
        if u64::try_from(size).unwrap_or(u64::MAX) > max {
            log::info!(
                "file {}: original is {size} bytes, over the cap; skipping EXIF",
                file.id
            );
            return Ok(None);
        }
        let url = wc.file_url(file.id).await?;
        // Encrypted size is a little larger than the plaintext.
        let enc = wc.base().download(&url, max + max / 64 + 4096).await?;
        Ok(Some(self.with_key(slot, |k| {
            decrypt_stream(&enc, &file.file.decryption_header, k)
        })??))
    }
}

fn header(b64: &str) -> Result<Header> {
    let raw = STANDARD
        .decode(b64)
        .map_err(|_| Error::Other("header is not base64".to_owned()))?;
    Ok(Header::try_from_slice(&raw)?)
}

fn decrypt_json(data_b64: Option<&str>, header_b64: &str, key: &Key) -> Result<Map<String, Value>> {
    let Some(data_b64) = data_b64 else {
        return Ok(Map::new());
    };
    let data = STANDARD
        .decode(data_b64)
        .map_err(|_| Error::Other("metadata is not base64".to_owned()))?;
    let plain = Zeroizing::new(blob::decrypt(&data, &header(header_b64)?, key)?);
    serde_json::from_slice(&plain).map_err(|e| Error::Other(format!("metadata JSON: {e}")))
}

fn decrypt_stream(data: &[u8], header_b64: &str, key: &Key) -> Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(stream::decrypt_file_data(
        data,
        &header(header_b64)?,
        key,
    )?))
}

fn dims(m: &Map<String, Value>, w: &str, h: &str) -> Option<(u32, u32)> {
    let g = |k: &str| {
        m.get(k)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    Some((g(w)?, g(h)?))
}

// Runs the CPU stages on decrypted inputs. Pure apart from the clock.
pub fn analyse(
    stages_cfg: &crate::config::Stages,
    kinds: &[crate::config::PiiKind],
    metadata: &Map<String, Value>,
    pub_magic: &Map<String, Value>,
    thumbnail: &[u8],
    original: Option<&[u8]>,
) -> Value {
    let decoded = ente_image::decode::decode_image_from_bytes(thumbnail)
        .map_err(|e| log::debug!("thumbnail decode failed: {e}"))
        .ok();
    let exif = original.and_then(|o| {
        ente_exif::read(
            &mut Cursor::new(o),
            ente_exif::Mode::Summary,
            ente_exif::Limits::default(),
        )
        .ok()
    });
    let dimensions = dims(pub_magic, "w", "h")
        .or_else(|| dims(metadata, "width", "height"))
        .or_else(|| exif.as_ref().and_then(|e| Some((e.width()?, e.height()?))));
    let mut texts: Vec<&str> = Vec::new();
    for (m, k) in [
        (metadata, "title"),
        (pub_magic, "editedName"),
        (pub_magic, "caption"),
    ] {
        if let Some(s) = m.get(k).and_then(Value::as_str) {
            texts.push(s);
        }
    }
    if let Some(d) = exif
        .as_ref()
        .and_then(ente_exif::Metadata::exif_description)
    {
        texts.push(d);
    }
    let input = stages::Input {
        rgb: decoded.as_ref(),
        exif: exif.as_ref(),
        dimensions,
        texts,
    };
    let mut out = stages::run(stages_cfg, kinds, &input);
    if let Value::Object(o) = &mut out {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        o.insert(
            "updatedAt".to_owned(),
            Value::from(u64::try_from(now).unwrap_or(u64::MAX)),
        );
    }
    if let Some(mut img) = decoded {
        zeroize::Zeroize::zeroize(&mut img.rgb);
    }
    out
}
