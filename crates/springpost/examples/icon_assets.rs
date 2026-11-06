//! Regenerate the macOS icon artwork from `assets/icon-1024.png`.
//!
//! macOS icons sit inside a margin (824 px of a 1024 px canvas) so they line
//! up with other apps in the Dock; the source art is full-bleed. This writes
//! `assets/icon-macos-1024.png` with that margin, transparent around it.
//!
//! `cargo run -p springpost --example icon_assets`

use image::{imageops, RgbaImage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let src = image::open(dir.join("icon-1024.png"))?.to_rgba8();
    let inner = imageops::resize(&src, 824, 824, imageops::FilterType::Lanczos3);
    let mut canvas = RgbaImage::new(1024, 1024); // fully transparent
    imageops::overlay(&mut canvas, &inner, 100, 100);
    let out = dir.join("icon-macos-1024.png");
    canvas.save(&out)?;
    assert_eq!(canvas.get_pixel(0, 0)[3], 0, "margin must be transparent");
    println!("wrote {}", out.display());
    Ok(())
}
