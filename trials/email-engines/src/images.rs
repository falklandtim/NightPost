//! The trial's pictures, made in code so the corpus stays small and nothing
//! is copied from anywhere: soft gradients and blocks of colour, each with a
//! distinct look so a missing or misplaced picture is easy to spot.
//!
//! The same pictures are served by the local "remote" server and packed
//! into the corpus's `.eml` files as `cid:` parts.

use std::collections::BTreeMap;
use std::io::Cursor;

use image::{ImageFormat, Rgb, RgbImage};

/// A transparent 1×1 GIF: spacers in old layouts, and tracking pixels.
pub const PIXEL_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

/// A picture's file name, its MIME type and its bytes.
pub struct Picture {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// Every picture by file name (as served under `/images/`).
pub fn all() -> anyhow::Result<BTreeMap<String, Picture>> {
    let mut out = BTreeMap::new();
    let mut png = |name: &str, img: RgbImage| -> anyhow::Result<()> {
        out.insert(name.to_string(), Picture { mime: "image/png", bytes: encode(&img)? });
        Ok(())
    };
    png("hero.png", gradient(1200, 480, [47, 93, 80], [217, 203, 178]))?;
    png("guji-hero.jpg.png", gradient(1120, 480, [120, 84, 52], [217, 203, 178]))?;
    png("logo.png", blocks(240, 72, &[[31, 58, 96], [242, 183, 5]]))?;
    png("bg-pattern.png", checks(64, 64, [236, 240, 247], [222, 229, 240]))?;
    for (i, colour) in [[198, 93, 74], [74, 128, 198], [88, 160, 102], [176, 120, 196]].iter().enumerate() {
        png(&format!("product-{}.png", i + 1), gradient(400, 300, *colour, [250, 248, 244]))?;
    }
    for i in 0..10u8 {
        let c = [40 + i * 18, 150u8.saturating_sub(i * 9), 120 + i * 12];
        png(&format!("thumb-{i}.png"), gradient(160, 120, c, [245, 245, 245]))?;
    }
    png("photo-1.png", gradient(640, 420, [64, 112, 160], [232, 214, 170]))?;
    png("photo-2.png", gradient(640, 420, [160, 64, 96], [240, 228, 200]))?;
    png("remote-banner.png", blocks(600, 160, &[[20, 20, 20], [230, 60, 60], [20, 20, 20]]))?;
    out.insert("spacer.gif".into(), Picture { mime: "image/gif", bytes: PIXEL_GIF.to_vec() });
    out.insert("open.gif".into(), Picture { mime: "image/gif", bytes: PIXEL_GIF.to_vec() });
    Ok(out)
}

/// (Only the main build makes pages; the Servo build reads them prepared.)
#[cfg_attr(feature = "servo", allow(dead_code))]
/// A large, hard-to-compress PNG of about `target` bytes, for the "huge
/// inline image" case. Deterministic noise, so every run is the same.
pub fn huge_png(target: usize) -> anyhow::Result<Vec<u8>> {
    // Noise barely compresses, so 3 bytes a pixel is close to the file size.
    let side = ((target / 3) as f64).sqrt() as u32;
    let mut seed: u32 = 0x9e37_79b9;
    let img = RgbImage::from_fn(side, side, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let [a, b, c, _] = seed.to_le_bytes();
        Rgb([a, b, c])
    });
    encode(&img)
}

fn encode(img: &RgbImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn gradient(w: u32, h: u32, a: [u8; 3], b: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| {
        let t = (x as f32 / w as f32 * 0.7 + y as f32 / h as f32 * 0.3).clamp(0.0, 1.0);
        let mix = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
        Rgb([mix(0), mix(1), mix(2)])
    })
}

fn blocks(w: u32, h: u32, colours: &[[u8; 3]]) -> RgbImage {
    let n = colours.len().max(1) as u32;
    RgbImage::from_fn(w, h, |x, _| Rgb(colours.get((x * n / w) as usize).copied().unwrap_or([0, 0, 0])))
}

fn checks(w: u32, h: u32, a: [u8; 3], b: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| Rgb(if (x / 16 + y / 16) % 2 == 0 { a } else { b }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_decode() {
        for (name, p) in all().unwrap() {
            assert!(image::load_from_memory(&p.bytes).is_ok(), "{name} doesn't decode");
        }
    }

    #[test]
    fn huge_is_about_the_size_asked() {
        let n = huge_png(2_000_000).unwrap().len();
        assert!((1_800_000..2_400_000).contains(&n), "{n}");
    }
}
