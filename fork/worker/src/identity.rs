// The worker's long-term X25519 keypair. The private key is read once at
// start from a systemd credential or stdin, moved into guarded memory, and
// the buffer it came in is wiped.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ente_core::crypto::{PublicKey, SecretKey, sealed};
use zeroize::{Zeroize, Zeroizing};

use crate::config::Identity;
use crate::secure_mem::{self, SecretRegion};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "systemd credential {0} not found: run under systemd with \
         LoadCredentialEncrypted= (see fork/worker/README.md)"
    )]
    NoCredentialsDirectory(String),
    #[error("could not read worker key: {0}")]
    Read(std::io::Error),
    #[error("worker key must be base64 of 32 bytes")]
    Format,
    #[error("worker key does not match server.public_key in worker.toml")]
    Mismatch,
    #[error(transparent)]
    Memory(#[from] secure_mem::Error),
    #[error("could not open sealed box: {0}")]
    Open(ente_core::crypto::Error),
}

pub struct WorkerIdentity {
    secret: Mutex<SecretRegion>,
    public: PublicKey,
    public_b64: String,
}

impl WorkerIdentity {
    pub fn load(
        source: &Identity,
        expected_public: Option<&str>,
        options: secure_mem::Options,
    ) -> Result<Self, Error> {
        let mut text = Zeroizing::new(Vec::new());
        match source {
            Identity::SystemdCreds { credential } => {
                let dir = std::env::var_os("CREDENTIALS_DIRECTORY")
                    .ok_or_else(|| Error::NoCredentialsDirectory(credential.clone()))?;
                let path = PathBuf::from(dir).join(credential);
                let mut f = std::fs::File::open(&path).map_err(Error::Read)?;
                f.read_to_end(&mut text).map_err(Error::Read)?;
            }
            Identity::Stdin => {
                std::io::stdin()
                    .lock()
                    .read_to_end(&mut text)
                    .map_err(Error::Read)?;
            }
        }
        Self::from_base64(&text, expected_public, options)
    }

    // Takes base64 of the 32 byte secret (surrounding whitespace allowed).
    pub fn from_base64(
        text: &[u8],
        expected_public: Option<&str>,
        options: secure_mem::Options,
    ) -> Result<Self, Error> {
        let trimmed = text.trim_ascii();
        let mut raw = Zeroizing::new(STANDARD.decode(trimmed).map_err(|_| Error::Format)?);
        if raw.len() != SecretKey::BYTES {
            return Err(Error::Format);
        }
        let mut region = SecretRegion::new(SecretKey::BYTES, options)?;
        region.with_mut(|bytes| bytes.copy_from_slice(&raw))?;
        raw.zeroize();
        let public = region
            .with(|bytes| SecretKey::try_from_slice(bytes).map(|sk| sk.public_key()))?
            .map_err(|_| Error::Format)?;
        let public_b64 = STANDARD.encode(public.as_bytes());
        if let Some(expected) = expected_public
            && expected.trim() != public_b64
        {
            return Err(Error::Mismatch);
        }
        Ok(Self {
            secret: Mutex::new(region),
            public,
            public_b64,
        })
    }

    pub fn public_key_b64(&self) -> &str {
        &self.public_b64
    }

    // Opens a `crypto_box_seal` box addressed to this worker.
    pub fn open(&self, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        let mut region = self
            .secret
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        region.with(|bytes| {
            let sk = SecretKey::try_from_slice(bytes).map_err(Error::Open)?;
            sealed::open(ciphertext, &self.public, &sk)
                .map(Zeroizing::new)
                .map_err(Error::Open)
        })?
    }
}

// `ente-worker keygen`: prints the secret (base64) on stdout for piping
// straight into `systemd-creds encrypt`, and the public key on stderr.
pub fn keygen(allow_terminal: bool) -> anyhow::Result<()> {
    if std::io::stdout().is_terminal() && !allow_terminal {
        anyhow::bail!(
            "refusing to print the private key to a terminal. Pipe it into a secret store, e.g.\n  \
             ente-worker keygen | sudo systemd-creds encrypt --name=ente-worker-key - \
             /etc/credstore.encrypted/ente-worker-key"
        );
    }
    let sk = SecretKey::generate();
    let public = STANDARD.encode(sk.public_key().as_bytes());
    let mut secret = Zeroizing::new(STANDARD.encode(sk.as_bytes()));
    {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        out.write_all(secret.as_bytes())?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    secret.zeroize();
    eprintln!("public key (put it in museum.yaml fork.worker-keys and worker.toml): {public}");
    eprintln!(
        "fingerprint (confirm this in the app): {}",
        fingerprint(&public)?
    );
    Ok(())
}

// Hex SHA-256 of the raw public key, as clients and museum show it.
pub fn fingerprint(public_b64: &str) -> anyhow::Result<String> {
    use sha2::Digest;
    let raw = STANDARD.decode(public_b64.trim())?;
    let digest = sha2::Sha256::digest(&raw);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_boxes_sealed_to_it() {
        let sk = SecretKey::generate();
        let b64 = STANDARD.encode(sk.as_bytes());
        let id =
            WorkerIdentity::from_base64(format!("{b64}\n").as_bytes(), None, Default::default())
                .unwrap();
        assert_eq!(
            id.public_key_b64(),
            STANDARD.encode(sk.public_key().as_bytes())
        );
        let boxed = sealed::seal(b"hello", &sk.public_key()).unwrap();
        assert_eq!(id.open(&boxed).unwrap().as_slice(), b"hello");
    }

    #[test]
    fn checks_expected_public_key() {
        let sk = SecretKey::generate();
        let b64 = STANDARD.encode(sk.as_bytes());
        let other = STANDARD.encode(SecretKey::generate().public_key().as_bytes());
        assert!(matches!(
            WorkerIdentity::from_base64(b64.as_bytes(), Some(&other), Default::default()),
            Err(Error::Mismatch)
        ));
        assert!(matches!(
            WorkerIdentity::from_base64(b"AAAA", None, Default::default()),
            Err(Error::Format)
        ));
    }

    #[test]
    fn fingerprint_is_hex_sha256() {
        // SHA-256 of 32 zero bytes.
        let fp = fingerprint(&STANDARD.encode([0u8; 32])).unwrap();
        assert_eq!(
            fp,
            "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
        );
    }
}
