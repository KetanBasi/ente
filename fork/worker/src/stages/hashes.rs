//! Perceptual hashes of the thumbnail, for near-duplicate grouping:
//! a 64-bit difference hash and a 64-bit DCT hash. Hamming distance up to
//! about 10 means "probably the same picture".

use ente_image::DecodedImage;

/// Grayscale (BT.601) downscale by area average so the longest side is at
/// most `max_side`. Returns `(width, height, pixels)`.
pub(crate) fn gray_bounded(img: &DecodedImage, max_side: usize) -> Option<(usize, usize, Vec<u8>)> {
    let w = img.dimensions.width as usize;
    let h = img.dimensions.height as usize;
    if w == 0 || h == 0 || img.rgb.len() != w * h * 3 {
        return None;
    }
    let scale = w.max(h).div_ceil(max_side).max(1);
    let (ow, oh) = (w.div_ceil(scale), h.div_ceil(scale));
    let mut out = vec![0u8; ow * oh];
    for oy in 0..oh {
        for ox in 0..ow {
            let (mut acc, mut n) = (0u32, 0u32);
            for y in oy * scale..((oy + 1) * scale).min(h) {
                for x in ox * scale..((ox + 1) * scale).min(w) {
                    let p = &img.rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
                    acc += (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]))
                        / 1000;
                    n += 1;
                }
            }
            out[oy * ow + ox] = u8::try_from(acc / n.max(1)).unwrap_or(255);
        }
    }
    Some((ow, oh, out))
}

/// Area-average resize of a gray image to exactly `tw` x `th`.
fn resize_gray(src: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<f64> {
    let mut out = vec![0.0; tw * th];
    for ty in 0..th {
        let (y0, y1) = (
            ty * h / th,
            ((ty + 1) * h).div_ceil(th).max(ty * h / th + 1).min(h),
        );
        for tx in 0..tw {
            let (x0, x1) = (
                tx * w / tw,
                ((tx + 1) * w).div_ceil(tw).max(tx * w / tw + 1).min(w),
            );
            let mut acc = 0.0;
            let mut n = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    acc += f64::from(src[y * w + x]);
                    n += 1.0;
                }
            }
            out[ty * tw + tx] = acc / n;
        }
    }
    out
}

pub struct Hashes {
    /// 16 lowercase hex digits each.
    pub dhash: String,
    pub phash: String,
}

pub fn compute(img: &DecodedImage) -> Option<Hashes> {
    let (w, h, gray) = gray_bounded(img, 256)?;
    Some(Hashes {
        dhash: format!("{:016x}", dhash(&gray, w, h)),
        phash: format!("{:016x}", phash(&gray, w, h)),
    })
}

fn dhash(gray: &[u8], w: usize, h: usize) -> u64 {
    let s = resize_gray(gray, w, h, 9, 8);
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            bits = (bits << 1) | u64::from(s[y * 9 + x] > s[y * 9 + x + 1]);
        }
    }
    bits
}

fn phash(gray: &[u8], w: usize, h: usize) -> u64 {
    const N: usize = 32;
    let s = resize_gray(gray, w, h, N, N);
    let cos: Vec<f64> = (0..8)
        .flat_map(|u| {
            (0..N).map(move |x| {
                (std::f64::consts::PI * (2.0 * x as f64 + 1.0) * u as f64 / (2.0 * N as f64)).cos()
            })
        })
        .collect();
    // Low-frequency 8x8 block of the 2D DCT.
    let mut coeffs = [0.0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut acc = 0.0;
            for y in 0..N {
                for x in 0..N {
                    acc += s[y * N + x] * cos[u * N + x] * cos[v * N + y];
                }
            }
            coeffs[v * 8 + u] = acc;
        }
    }
    // Median of all but the DC term.
    let mut rest: Vec<f64> = coeffs[1..].to_vec();
    rest.sort_by(f64::total_cmp);
    let median = rest[rest.len() / 2];
    coeffs
        .iter()
        .fold(0u64, |bits, c| (bits << 1) | u64::from(*c > median))
}

pub fn hamming(a: &str, b: &str) -> Option<u32> {
    Some((u64::from_str_radix(a, 16).ok()? ^ u64::from_str_radix(b, 16).ok()?).count_ones())
}

#[cfg(test)]
mod tests {
    use ente_image::Dimensions;

    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> DecodedImage {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        DecodedImage {
            dimensions: Dimensions {
                width: w,
                height: h,
            },
            rgb,
        }
    }

    fn scene(x: u32, y: u32) -> u8 {
        u8::try_from(((x * 3 + y * 2) % 200) + if x > 40 && y < 30 { 50 } else { 0 }).unwrap_or(255)
    }

    #[test]
    fn same_picture_at_other_size_is_close_and_different_is_far() {
        let big = compute(&img(120, 90, |x, y| scene(x * 160 / 120, y * 120 / 90))).unwrap();
        let small = compute(&img(160, 120, scene)).unwrap();
        let other = compute(&img(160, 120, |x, y| {
            u8::try_from((x * y) % 251).unwrap_or(0)
        }))
        .unwrap();
        assert!(hamming(&big.dhash, &small.dhash).unwrap() <= 12);
        assert!(hamming(&big.phash, &small.phash).unwrap() <= 12);
        assert!(hamming(&small.phash, &other.phash).unwrap() > 12);
        assert_eq!(small.dhash.len(), 16);
    }

    #[test]
    fn rejects_malformed_buffers() {
        let bad = DecodedImage {
            dimensions: Dimensions {
                width: 4,
                height: 4,
            },
            rgb: vec![0; 5],
        };
        assert!(compute(&bad).is_none());
    }
}
