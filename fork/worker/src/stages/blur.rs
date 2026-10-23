// Blur score: variance of the Laplacian of the grayscale thumbnail,
// downscaled so scores are comparable across sizes.

use ente_image::DecodedImage;

// Below this the picture is flagged blurry. A starting point, to be tuned
// against labelled photos; the raw score is stored too.
pub const BLURRY_BELOW: f64 = 60.0;

const MAX_SIDE: usize = 512;

pub fn laplacian_variance(img: &DecodedImage) -> Option<f64> {
    let (w, h, gray) = super::hashes::gray_bounded(img, MAX_SIDE)?;
    if w < 3 || h < 3 {
        return None;
    }
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut n = 0.0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = f64::from(gray[y * w + x]);
            let l = f64::from(gray[(y - 1) * w + x])
                + f64::from(gray[(y + 1) * w + x])
                + f64::from(gray[y * w + x - 1])
                + f64::from(gray[y * w + x + 1])
                - 4.0 * c;
            sum += l;
            sum_sq += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    Some(((sum_sq / n) - mean * mean).max(0.0))
}

#[cfg(test)]
mod tests {
    use ente_image::{DecodedImage, Dimensions};

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

    #[test]
    fn flat_is_blurry_and_checker_is_sharp() {
        let flat = laplacian_variance(&img(64, 64, |_, _| 128)).unwrap();
        let sharp = laplacian_variance(&img(64, 64, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 }))
            .unwrap();
        assert!(flat < BLURRY_BELOW);
        assert!(sharp > 1000.0);
    }

    #[test]
    fn tiny_images_have_no_score() {
        assert!(laplacian_variance(&img(2, 2, |_, _| 0)).is_none());
    }
}
