//! `worker.toml`: one file picks the profile, stages, models, precision,
//! batch size and VRAM budget. See fork/worker/worker.example.toml.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid worker config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid worker config: {0}")]
    Invalid(String),
}

/// Deployment presets from the plan. Explicit settings override them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// Regex, EXIF, hashes, blur. No GPU.
    #[default]
    CpuOnly,
    /// OCR + text Laya; CLIP and faces left to clients.
    SmallGpu,
    /// Every stage on a 16 GB card, about 6 GB budget.
    #[serde(rename = "home-16gb")]
    Home16gb,
    /// Each stage its own queue, scaled separately.
    LargeServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Device {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Precision {
    Fp32,
    Bf16,
    Fp16,
    Int8,
    Fp8,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Identity {
    /// A systemd credential (`LoadCredentialEncrypted=`), which systemd
    /// decrypts with the TPM into a non-swappable ramfs at start.
    SystemdCreds {
        #[serde(default = "default_credential")]
        credential: String,
    },
    /// Base64 of the 32 byte secret on stdin, piped from a secret store.
    Stdin,
}

fn default_credential() -> String {
    "ente-worker-key".to_owned()
}

/// The PII kinds the regex stage knows. See `stages::pii`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, serde::Serialize, PartialOrd, Ord,
)]
#[serde(rename_all = "kebab-case")]
pub enum PiiKind {
    /// Luhn-checked payment card numbers.
    Card,
    /// IBAN with a valid mod-97 checksum.
    Iban,
    /// Passport / ID machine-readable zone with valid check digits.
    Mrz,
    /// Indonesian NIK (16 digits, valid region code and birth date).
    Nik,
    Email,
    Phone,
}

impl PiiKind {
    pub const ALL: [PiiKind; 6] = [
        PiiKind::Card,
        PiiKind::Iban,
        PiiKind::Mrz,
        PiiKind::Nik,
        PiiKind::Email,
        PiiKind::Phone,
    ];
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    profile: Option<Profile>,
    server: RawServer,
    identity: Option<Identity>,
    #[serde(default)]
    runtime: RawRuntime,
    #[serde(default)]
    stages: RawStages,
    #[serde(default)]
    models: Models,
    #[serde(default)]
    memory: Memory,
    #[serde(default)]
    mldata: RawMlData,
    #[serde(default)]
    pii: RawPii,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    endpoint: String,
    /// Expected base64 X25519 public key; checked against the private key.
    public_key: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRuntime {
    device: Option<Device>,
    precision: Option<Precision>,
    batch_size: Option<usize>,
    concurrency: Option<usize>,
    vram_budget_mb: Option<u64>,
    poll_interval_secs: Option<u64>,
    max_original_mb: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStages {
    device: Option<bool>,
    hashes: Option<bool>,
    blur: Option<bool>,
    pii_regex: Option<bool>,
    ocr: Option<bool>,
    clip: Option<bool>,
    faces: Option<bool>,
    laya: Option<bool>,
    gliner_pii: Option<bool>,
}

/// Model paths for the later, model-backed stages. Unused by CPU stages.
#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub ocr: Option<PathBuf>,
    pub clip: Option<PathBuf>,
    pub faces: Option<PathBuf>,
    pub laya: Option<PathBuf>,
    pub gliner_pii: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    /// Refuse to start without Linux `memfd_secret`.
    #[serde(default)]
    pub require_memfd_secret: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMlData {
    require_face_clip: Option<bool>,
    min_face_version: Option<u64>,
    min_clip_version: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPii {
    kinds: Option<Vec<PiiKind>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub profile: Profile,
    pub endpoint: String,
    pub public_key: Option<String>,
    pub timeout_secs: u64,
    pub identity: Identity,
    pub runtime: Runtime,
    pub stages: Stages,
    pub models: Models,
    pub memory: Memory,
    pub mldata: MlData,
    pub pii_kinds: Vec<PiiKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub device: Device,
    pub precision: Precision,
    /// Files whose keys are held (and decrypted) at once.
    pub batch_size: usize,
    /// Files analysed in parallel, at most `batch_size`.
    pub concurrency: usize,
    /// Hard VRAM cap for model stages; 0 means no GPU use.
    pub vram_budget_mb: u64,
    pub poll_interval_secs: u64,
    /// Originals above this are not downloaded; thumbnail-only stages run.
    pub max_original_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stages {
    pub device: bool,
    pub hashes: bool,
    pub blur: bool,
    pub pii_regex: bool,
    pub ocr: bool,
    pub clip: bool,
    pub faces: bool,
    pub laya: bool,
    pub gliner_pii: bool,
}

impl Stages {
    fn names_needing_models(&self) -> Vec<&'static str> {
        [
            (self.ocr, "ocr"),
            (self.clip, "clip"),
            (self.faces, "faces"),
            (self.laya, "laya"),
            (self.gliner_pii, "gliner_pii"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlData {
    /// Only add `x_enrich` to records that already carry current `face` and
    /// `clip` indexes, so stock mobile and web never rewrite the record.
    pub require_face_clip: bool,
    pub min_face_version: u64,
    pub min_clip_version: u64,
}

struct Preset {
    device: Device,
    precision: Precision,
    batch_size: usize,
    vram_budget_mb: u64,
    model_stages: bool,
}

fn preset(profile: Profile) -> Preset {
    match profile {
        Profile::CpuOnly => Preset {
            device: Device::Cpu,
            precision: Precision::Fp32,
            batch_size: 1,
            vram_budget_mb: 0,
            model_stages: false,
        },
        Profile::SmallGpu => Preset {
            device: Device::Cuda,
            precision: Precision::Int8,
            batch_size: 4,
            vram_budget_mb: 2048,
            model_stages: true,
        },
        Profile::Home16gb => Preset {
            device: Device::Cuda,
            precision: Precision::Bf16,
            batch_size: 16,
            vram_budget_mb: 6144,
            model_stages: true,
        },
        Profile::LargeServer => Preset {
            device: Device::Cuda,
            precision: Precision::Int8,
            batch_size: 64,
            vram_budget_mb: 0,
            model_stages: true,
        },
    }
}

/// Upper bound on held keys: 1000 is the server's inbox batch cap.
pub const MAX_BATCH: usize = 1000;

impl Config {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let raw: RawConfig = toml::from_str(text)?;
        Self::resolve(raw)
    }

    fn resolve(raw: RawConfig) -> Result<Self, Error> {
        let profile = raw.profile.unwrap_or_default();
        let p = preset(profile);
        let r = raw.runtime;
        let s = raw.stages;
        let batch_size = r.batch_size.unwrap_or(p.batch_size);
        let runtime = Runtime {
            device: r.device.unwrap_or(p.device),
            precision: r.precision.unwrap_or(p.precision),
            batch_size,
            concurrency: r.concurrency.unwrap_or(1).min(batch_size.max(1)),
            vram_budget_mb: r.vram_budget_mb.unwrap_or(p.vram_budget_mb),
            poll_interval_secs: r.poll_interval_secs.unwrap_or(30),
            max_original_mb: r.max_original_mb.unwrap_or(64),
        };
        let stages = Stages {
            device: s.device.unwrap_or(true),
            hashes: s.hashes.unwrap_or(true),
            blur: s.blur.unwrap_or(true),
            pii_regex: s.pii_regex.unwrap_or(true),
            ocr: s.ocr.unwrap_or(p.model_stages),
            clip: s.clip.unwrap_or(false),
            faces: s.faces.unwrap_or(false),
            laya: s.laya.unwrap_or(p.model_stages),
            gliner_pii: s.gliner_pii.unwrap_or(p.model_stages),
        };
        let mut pii_kinds = raw.pii.kinds.unwrap_or_else(|| PiiKind::ALL.to_vec());
        pii_kinds.sort();
        pii_kinds.dedup();

        let config = Config {
            profile,
            endpoint: raw.server.endpoint.trim_end_matches('/').to_owned(),
            public_key: raw.server.public_key,
            timeout_secs: raw.server.timeout_secs.unwrap_or(60),
            identity: raw.identity.unwrap_or(Identity::SystemdCreds {
                credential: default_credential(),
            }),
            runtime,
            stages,
            models: raw.models,
            memory: raw.memory,
            mldata: MlData {
                require_face_clip: raw.mldata.require_face_clip.unwrap_or(true),
                min_face_version: raw.mldata.min_face_version.unwrap_or(1),
                min_clip_version: raw.mldata.min_clip_version.unwrap_or(1),
            },
            pii_kinds,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Error> {
        let invalid = |m: String| Err(Error::Invalid(m));
        if self.endpoint.is_empty() {
            return invalid("server.endpoint is required".to_owned());
        }
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) {
            return invalid(format!(
                "server.endpoint must be an http(s) URL, got {}",
                self.endpoint
            ));
        }
        let rt = &self.runtime;
        if rt.batch_size == 0 || rt.batch_size > MAX_BATCH {
            return invalid(format!(
                "runtime.batch_size must be 1..={MAX_BATCH}, got {}",
                rt.batch_size
            ));
        }
        if rt.concurrency == 0 {
            return invalid("runtime.concurrency must be at least 1".to_owned());
        }
        if rt.poll_interval_secs == 0 {
            return invalid("runtime.poll_interval_secs must be at least 1".to_owned());
        }
        // This build is the CPU-only worker (phase 4). GPU and model stages
        // come later; fail loudly rather than silently skip them.
        if rt.device == Device::Cuda {
            return invalid(format!(
                "profile {:?} / runtime.device = \"cuda\" needs the GPU worker, which is not \
                 built yet. Use profile = \"cpu-only\" or set runtime.device = \"cpu\"",
                self.profile
            ));
        }
        if rt.vram_budget_mb != 0 {
            return invalid("runtime.vram_budget_mb must be 0 on device = \"cpu\"".to_owned());
        }
        let model_stages = self.stages.names_needing_models();
        if !model_stages.is_empty() {
            return invalid(format!(
                "stages {} need models that this CPU-only build does not run yet; set them to false",
                model_stages.join(", ")
            ));
        }
        if matches!(
            rt.precision,
            Precision::Bf16 | Precision::Fp16 | Precision::Fp8
        ) {
            return invalid(format!(
                "precision {:?} is GPU-only; use fp32 or int8 on CPU",
                rt.precision
            ));
        }
        Ok(())
    }

    /// Bytes of locked memory the worker needs: the key slab, the private
    /// key and the tokens.
    pub fn locked_bytes(&self) -> usize {
        self.runtime.batch_size * crate::secure_mem::KEY_BYTES + 3 * crate::secure_mem::page_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = "[server]\nendpoint = \"http://localhost:8080/\"\n";

    #[test]
    fn minimal_config_is_cpu_only() {
        let c = Config::parse(MIN).unwrap();
        assert_eq!(c.profile, Profile::CpuOnly);
        assert_eq!(c.endpoint, "http://localhost:8080");
        assert_eq!(c.runtime.device, Device::Cpu);
        assert_eq!(c.runtime.batch_size, 1);
        assert_eq!(c.runtime.vram_budget_mb, 0);
        assert!(c.stages.device && c.stages.hashes && c.stages.blur && c.stages.pii_regex);
        assert!(!c.stages.ocr && !c.stages.laya);
        assert!(c.mldata.require_face_clip);
        assert_eq!(c.pii_kinds.len(), PiiKind::ALL.len());
        assert_eq!(
            c.identity,
            Identity::SystemdCreds {
                credential: "ente-worker-key".to_owned()
            }
        );
    }

    #[test]
    fn overrides_win_over_profile() {
        let c = Config::parse(&format!(
            "{MIN}[runtime]\nbatch_size = 32\nconcurrency = 64\n[stages]\nblur = false\n\
             [identity]\nsource = \"stdin\"\n[pii]\nkinds = [\"iban\", \"card\", \"iban\"]\n"
        ))
        .unwrap();
        assert_eq!(c.runtime.batch_size, 32);
        assert_eq!(
            c.runtime.concurrency, 32,
            "concurrency is capped at batch size"
        );
        assert!(!c.stages.blur);
        assert_eq!(c.identity, Identity::Stdin);
        assert_eq!(c.pii_kinds, vec![PiiKind::Card, PiiKind::Iban]);
    }

    #[test]
    fn gpu_profiles_are_refused_for_now() {
        for profile in ["small-gpu", "home-16gb", "large-server"] {
            let err = Config::parse(&format!("profile = \"{profile}\"\n{MIN}")).unwrap_err();
            assert!(err.to_string().contains("GPU worker"), "{profile}: {err}");
        }
        let err = Config::parse(&format!("{MIN}[stages]\nocr = true\n")).unwrap_err();
        assert!(err.to_string().contains("ocr"), "{err}");
        let err = Config::parse(&format!("{MIN}[runtime]\nvram_budget_mb = 100\n")).unwrap_err();
        assert!(err.to_string().contains("vram"), "{err}");
    }

    #[test]
    fn rejects_typos_and_bad_values() {
        assert!(Config::parse(&format!("{MIN}[runtime]\nbatch = 3\n")).is_err());
        assert!(Config::parse(&format!("{MIN}[runtime]\nbatch_size = 0\n")).is_err());
        assert!(Config::parse(&format!("{MIN}[runtime]\nbatch_size = 5000\n")).is_err());
        assert!(Config::parse("[server]\nendpoint = \"localhost\"\n").is_err());
        assert!(Config::parse(&format!("{MIN}[identity]\nsource = \"file\"\n")).is_err());
    }

    #[test]
    fn example_config_parses() {
        let text = include_str!("../worker.example.toml");
        Config::parse(text).unwrap();
    }
}
