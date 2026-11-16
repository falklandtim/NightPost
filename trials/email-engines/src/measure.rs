//! Numbers: how different a picture is from the WebKit reference, the
//! median of a set of times, and peak memory of a child process.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;

/// A channel must differ by more than this (of 255) to count, so faint
/// anti-aliasing differences don't swamp the score.
const THRESHOLD: u8 = 32;

/// The share of pixels (0 to 1) that differ noticeably between `a` and the
/// reference `b`, after resizing `a` to `b`'s size. Only a pointer to where
/// to look: Tim's eye decides.
pub fn difference(a: &Path, reference: &Path) -> Result<f64> {
    let a = image::open(a)?.to_rgba8();
    let b = image::open(reference)?.to_rgba8();
    let a = if a.dimensions() == b.dimensions() { a } else { image::imageops::resize(&a, b.width(), b.height(), FilterType::Triangle) };
    let total = (b.width() as u64 * b.height() as u64).max(1);
    let differ = a
        .pixels()
        .zip(b.pixels())
        .filter(|(p, q)| p.0.iter().zip(q.0.iter()).take(3).any(|(x, y)| x.abs_diff(*y) > THRESHOLD))
        .count() as u64;
    Ok(differ as f64 / total as f64)
}

/// The median, or `None` for an empty list.
pub fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Run this program with `args` under `/usr/bin/time` and return its
/// standard output and peak resident memory in megabytes.
pub fn child_with_memory(args: &[&str]) -> Result<(String, f64)> {
    let me = std::env::current_exe()?;
    // macOS's time takes -l and reports bytes; GNU time takes -v and kilobytes.
    let mac = cfg!(target_os = "macos");
    let out = Command::new("/usr/bin/time").arg(if mac { "-l" } else { "-v" }).arg(&me).args(args).output().context("running /usr/bin/time")?;
    let err = String::from_utf8_lossy(&out.stderr);
    let peak = err
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if mac {
                l.strip_suffix("maximum resident set size").and_then(|n| n.trim().parse::<f64>().ok()).map(|b| b / 1_048_576.0)
            } else {
                l.strip_prefix("Maximum resident set size (kbytes):").and_then(|n| n.trim().parse::<f64>().ok()).map(|k| k / 1024.0)
            }
        })
        .context("no memory figure from /usr/bin/time")?;
    if !out.status.success() {
        bail!("child failed: {}", err.lines().find(|l| l.starts_with("error")).unwrap_or("no message"));
    }
    Ok((String::from_utf8_lossy(&out.stdout).into_owned(), peak))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    #[test]
    fn identical_pictures_score_zero() {
        let dir = std::env::temp_dir().join("email-engines-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbaImage::from_pixel(10, 10, image::Rgba([10, 20, 30, 255])).save(&p).unwrap();
        let q = dir.join("b.png");
        let mut img = image::RgbaImage::from_pixel(10, 10, image::Rgba([10, 20, 30, 255]));
        for x in 0..10 {
            img.put_pixel(x, 0, image::Rgba([255, 255, 255, 255]));
        }
        img.save(&q).unwrap();
        assert_eq!(difference(&p, &p).unwrap(), 0.0);
        assert!((difference(&q, &p).unwrap() - 0.1).abs() < 1e-9);
    }
}
