// Read, merge and write Ente's `mldata` file-data record.
//
// The record is gzipped JSON encrypted (single-chunk secretstream) with the
// fileKey. Museum cannot see inside it, so this module is what keeps Ente's
// own `face` and `clip` indexes, and any other key, byte-for-byte intact:
// the worker only ever replaces its own [`NAMESPACE`] key.

use std::io::{Read, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ente_core::crypto::{Header, Key, blob};
use serde_json::{Map, Value};

use crate::config::MlData;

// The worker's one top-level key in the record.
pub const NAMESPACE: &str = "x_enrich";

// Same cap web uses when it gunzips the record.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("mldata is not valid base64")]
    Base64,
    #[error("could not decrypt mldata: {0}")]
    Crypto(#[from] ente_core::crypto::Error),
    #[error("could not gunzip mldata: {0}")]
    Gzip(std::io::Error),
    #[error("mldata is larger than {MAX_BYTES} bytes")]
    TooLarge,
    #[error("mldata is not a JSON object: {0}")]
    Json(serde_json::Error),
}

// The decrypted record plus the server timestamp it was read at, which the
// write must send back as `lastUpdatedAt`.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub fields: Map<String, Value>,
    // 0 when the file had no record yet.
    pub updated_at: i64,
}

// Why a record is not ready for `x_enrich` yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    NoRecord,
    Missing(&'static str),
    Outdated {
        index: &'static str,
        version: u64,
        min: u64,
    },
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotReady::NoRecord => f.write_str("no mldata yet"),
            NotReady::Missing(i) => write!(f, "mldata has no {i} index yet"),
            NotReady::Outdated {
                index,
                version,
                min,
            } => {
                write!(f, "mldata {index} index is v{version}, need v{min}")
            }
        }
    }
}

impl Record {
    pub fn empty() -> Self {
        Self {
            fields: Map::new(),
            updated_at: 0,
        }
    }

    pub fn decrypt(
        encrypted_b64: &str,
        header_b64: &str,
        updated_at: i64,
        key: &Key,
    ) -> Result<Self, Error> {
        let data = STANDARD.decode(encrypted_b64).map_err(|_| Error::Base64)?;
        let header = STANDARD.decode(header_b64).map_err(|_| Error::Base64)?;
        let header = Header::try_from_slice(&header)?;
        let gz = blob::decrypt(&data, &header, key)?;
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(gz.as_slice())
            .take(MAX_BYTES + 1)
            .read_to_end(&mut json)
            .map_err(Error::Gzip)?;
        if json.len() as u64 > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let fields: Map<String, Value> = serde_json::from_slice(&json).map_err(Error::Json)?;
        Ok(Self { fields, updated_at })
    }

    // Checks that Ente's own indexes are present and current, so that
    // stock clients will not re-index (and overwrite) this record.
    pub fn ready(&self, rules: &MlData) -> Result<(), NotReady> {
        if !rules.require_face_clip {
            return Ok(());
        }
        if self.updated_at == 0 && self.fields.is_empty() {
            return Err(NotReady::NoRecord);
        }
        for (index, min) in [
            ("face", rules.min_face_version),
            ("clip", rules.min_clip_version),
        ] {
            let version = self
                .fields
                .get(index)
                .and_then(Value::as_object)
                .ok_or(NotReady::Missing(index))?
                .get("version")
                .and_then(Value::as_u64)
                .ok_or(NotReady::Missing(index))?;
            if version < min {
                return Err(NotReady::Outdated {
                    index,
                    version,
                    min,
                });
            }
        }
        Ok(())
    }

    // Replaces only the worker's namespaced key.
    pub fn with_enrichment(&self, enrichment: Value) -> Self {
        let mut fields = self.fields.clone();
        fields.insert(NAMESPACE.to_owned(), enrichment);
        Self {
            fields,
            updated_at: self.updated_at,
        }
    }

    // Gzips and encrypts the record: `(encryptedData, decryptionHeader)`
    // as standard base64, ready for `PUT /files/data`.
    pub fn encrypt(&self, key: &Key) -> Result<(String, String), Error> {
        let json = serde_json::to_vec(&self.fields).map_err(Error::Json)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&json).map_err(Error::Gzip)?;
        let gz = gz.finish().map_err(Error::Gzip)?;
        let enc = blob::encrypt(&gz, key)?;
        Ok((
            STANDARD.encode(&enc.encrypted_data),
            STANDARD.encode(enc.decryption_header.as_bytes()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rules() -> MlData {
        MlData {
            require_face_clip: true,
            min_face_version: 1,
            min_clip_version: 1,
        }
    }

    fn sample() -> Map<String, Value> {
        json!({
            "face": {"version": 1, "client": "web", "width": 10, "height": 10, "faces": []},
            "clip": {"version": 1, "client": "web", "embedding": [0.25, -1.5]},
            "somethingNew": {"keep": true},
            "x_enrich": {"version": 0, "stale": true}
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }

    #[test]
    fn roundtrip_and_merge_keep_other_keys() {
        let key = Key::generate();
        let rec = Record {
            fields: sample(),
            updated_at: 1234,
        };
        let (data, header) = rec.encrypt(&key).unwrap();
        let back = Record::decrypt(&data, &header, 1234, &key).unwrap();
        assert_eq!(back, rec);
        assert!(back.ready(&rules()).is_ok());

        let merged = back.with_enrichment(json!({"version": 1, "blur": 3.5}));
        assert_eq!(merged.updated_at, 1234);
        assert_eq!(
            merged.fields["x_enrich"],
            json!({"version": 1, "blur": 3.5})
        );
        for k in ["face", "clip", "somethingNew"] {
            assert_eq!(merged.fields[k], rec.fields[k], "{k} must survive");
        }
        // Key order is preserved too, so a re-encode only differs in x_enrich.
        let keys: Vec<_> = merged.fields.keys().cloned().collect();
        assert_eq!(keys, ["face", "clip", "somethingNew", "x_enrich"]);
    }

    #[test]
    fn readiness_rules() {
        let r = rules();
        assert_eq!(Record::empty().ready(&r), Err(NotReady::NoRecord));
        let mut f = sample();
        f.remove("clip");
        let rec = Record {
            fields: f,
            updated_at: 5,
        };
        assert_eq!(rec.ready(&r), Err(NotReady::Missing("clip")));
        let strict = MlData {
            min_face_version: 2,
            ..r
        };
        let rec = Record {
            fields: sample(),
            updated_at: 5,
        };
        assert_eq!(
            rec.ready(&strict),
            Err(NotReady::Outdated {
                index: "face",
                version: 1,
                min: 2
            })
        );
        let lax = MlData {
            require_face_clip: false,
            ..r
        };
        assert!(Record::empty().ready(&lax).is_ok());
    }

    #[test]
    fn wrong_key_fails() {
        let rec = Record {
            fields: sample(),
            updated_at: 1,
        };
        let (data, header) = rec.encrypt(&Key::generate()).unwrap();
        assert!(matches!(
            Record::decrypt(&data, &header, 1, &Key::generate()),
            Err(Error::Crypto(_))
        ));
    }
}
