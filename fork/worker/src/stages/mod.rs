// CPU analysis stages. Each is a pure function of its inputs so it can be
// tested without a server; `Enrichment` assembles their output into the
// value stored under `x_enrich`.

pub mod blur;
pub mod device;
pub mod hashes;
pub mod pii;

use serde_json::{Value, json};

use crate::config::{PiiKind, Stages};

// Bumped when the shape of `x_enrich` changes; older records are rebuilt.
pub const SCHEMA_VERSION: u64 = 1;

// Everything a file offers the stages. Image data is the decoded thumbnail.
pub struct Input<'a> {
    pub rgb: Option<&'a ente_image::DecodedImage>,
    pub exif: Option<&'a ente_exif::Metadata>,
    // Width and height of the original, from Ente's metadata.
    pub dimensions: Option<(u32, u32)>,
    // Free text the owner or camera wrote: title, caption, EXIF description.
    pub texts: Vec<&'a str>,
}

pub fn run(stages: &Stages, pii_kinds: &[PiiKind], input: &Input<'_>) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("version".to_owned(), json!(SCHEMA_VERSION));
    out.insert(
        "client".to_owned(),
        json!(format!("ente-fork-worker/{}", env!("CARGO_PKG_VERSION"))),
    );
    if stages.device {
        out.insert(
            "device".to_owned(),
            device::detect(input.exif, input.dimensions),
        );
    }
    if let Some(img) = input.rgb {
        if stages.hashes
            && let Some(h) = hashes::compute(img)
        {
            out.insert(
                "hashes".to_owned(),
                json!({ "dhash": h.dhash, "phash": h.phash }),
            );
        }
        if stages.blur
            && let Some(b) = blur::laplacian_variance(img)
        {
            out.insert(
                "blur".to_owned(),
                json!({ "laplacianVariance": b, "isBlurry": b < blur::BLURRY_BELOW }),
            );
        }
    }
    if stages.pii_regex {
        let hits = pii::scan(input.texts.iter().copied(), pii_kinds);
        out.insert("pii".to_owned(), pii::to_json(&hits));
    }
    Value::Object(out)
}
