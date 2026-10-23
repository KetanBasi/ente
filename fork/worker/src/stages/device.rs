//! Which device took or made the picture: camera make/model from EXIF, or,
//! for screenshots (no camera tags), the devices whose screen has that
//! exact pixel size. A size can match several phones, so it is a candidate
//! list, not a verdict.

use serde_json::{Value, json};

/// (width, height in portrait, devices). Extend freely; entries only add
/// candidates.
const SCREENS: &[(u32, u32, &[&str])] = &[
    (1170, 2532, &["iPhone 12", "iPhone 13", "iPhone 14"]),
    (1179, 2556, &["iPhone 14 Pro", "iPhone 15", "iPhone 16"]),
    (
        1290,
        2796,
        &["iPhone 14 Pro Max", "iPhone 15 Plus", "iPhone 15 Pro Max"],
    ),
    (1080, 2400, &["Android 1080x2400 phone"]),
    (1080, 1920, &["Android 1080p phone"]),
    (1440, 3120, &["Android 1440x3120 phone"]),
    (2560, 1440, &["1440p desktop display"]),
    (1920, 1080, &["1080p desktop display"]),
    (3840, 2160, &["4K desktop display"]),
];

pub fn detect(exif: Option<&ente_exif::Metadata>, dims: Option<(u32, u32)>) -> Value {
    let make = exif
        .and_then(|e| e.camera_make())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let model = exif
        .and_then(|e| e.camera_model())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if make.is_some() || model.is_some() {
        return json!({ "kind": "camera", "make": make, "model": model });
    }
    if let Some((w, h)) = dims {
        let (a, b) = (w.min(h), w.max(h));
        if let Some((_, _, names)) = SCREENS
            .iter()
            .find(|(sw, sh, _)| (*sw, *sh) == (a, b) || (*sw, *sh) == (b, a))
        {
            return json!({ "kind": "screenshot", "candidates": names });
        }
    }
    json!({ "kind": "unknown" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_match_in_either_orientation() {
        assert_eq!(detect(None, Some((1179, 2556)))["kind"], "screenshot");
        assert_eq!(
            detect(None, Some((2556, 1179)))["candidates"][0],
            "iPhone 14 Pro"
        );
        assert_eq!(detect(None, Some((1234, 777)))["kind"], "unknown");
        assert_eq!(detect(None, None)["kind"], "unknown");
    }
}
