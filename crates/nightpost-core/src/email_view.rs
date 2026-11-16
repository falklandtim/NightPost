//! An email as an assistant should see it: the text, with a marker where
//! each picture sits, plus the pictures themselves (tracking pixels
//! removed), sized down to send over MCP.
//!
//! Plain text alone misses what's only in the images: banner-only
//! marketing mail, charts, screenshots and photos pasted into the body.

use crate::engine;
use crate::html_clean::{strip_trackers, tag_attr};
use crate::models::{Account, Message};
use crate::store::Store;
use anyhow::{anyhow, Result};
use std::io::Read;
use std::time::Duration;

/// A picture from an email (or an attachment), ready to show a model.
#[derive(Debug, Clone)]
pub struct Picture {
    /// `image/jpeg` or `image/png`.
    pub mime: String,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// One image in the email, in reading order.
#[derive(Debug, Clone)]
pub struct ImageRef {
    /// 1-based, matching the `[image N]` marker in the text.
    pub n: usize,
    pub alt: String,
    pub src: String,
    pub picture: Option<Picture>,
    /// Why it wasn't included (too small, failed to load, limit reached…).
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct View {
    /// The text with `[image N: alt]` where each picture was.
    pub text: String,
    pub html: bool,
    pub images: Vec<ImageRef>,
    /// Hosts of the tracking pixels that were removed.
    pub trackers: Vec<String>,
}

pub struct Options {
    /// Fetch `http(s)` images (the sender's server sees the request, as when
    /// you view the email; tracking pixels are never fetched).
    pub remote: bool,
    /// At most this many pictures are included.
    pub max_images: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { remote: true, max_images: 8 }
    }
}

/// Longest side of a picture sent to the model.
pub const MAX_EDGE: u32 = 1200;
/// Images smaller than this on both sides are icons or spacers.
const MIN_EDGE: u32 = 48;
const MAX_DOWNLOAD: u64 = 8 * 1024 * 1024;

/// Build the view of message `m`. Inline (`cid:`) images need the raw
/// message, which is fetched from the server only when there are some.
pub fn view(store: &Store, account: &Account, m: &Message, opts: &Options) -> Result<View> {
    let Some(html) = m.body_html.as_deref().filter(|h| !h.trim().is_empty()) else {
        return Ok(View { text: m.body_text.clone(), ..Default::default() });
    };
    let (clean, trackers) = strip_trackers(html);
    let (marked, mut images) = mark_images(&clean);
    let text = crate::parse::html_to_text(&marked);
    let mut raw: Option<Result<Vec<u8>>> = None;
    let mut included = 0;
    let mut seen: Vec<(String, usize)> = Vec::new();
    for img in &mut images {
        if included >= opts.max_images {
            img.skipped = Some(if opts.max_images == 0 { "pictures not requested" } else { "picture limit reached" }.into());
            continue;
        }
        if let Some((_, first)) = seen.iter().find(|(s, _)| *s == img.src) {
            img.skipped = Some(format!("same picture as image {first}"));
            continue;
        }
        seen.push((img.src.clone(), img.n));
        let bytes = load(&img.src, opts, || {
            raw.get_or_insert_with(|| if account.demo { Err(anyhow!("demo messages have no inline images")) } else { engine::fetch_raw(store, account, m) })
                .as_ref()
                .map(|r| r.clone())
                .map_err(|e| anyhow!("{e:#}"))
        });
        match bytes.and_then(|b| picture(&b)) {
            Ok(p) if p.width.max(p.height) < MIN_EDGE => img.skipped = Some("small icon".into()),
            Ok(p) => {
                img.picture = Some(p);
                included += 1;
            }
            Err(e) => img.skipped = Some(format!("{e:#}")),
        }
    }
    Ok(View { text, html: true, images, trackers })
}

/// Replace each `<img>` with a visible `[image N: alt]` marker and list the
/// images in order.
pub fn mark_images(html: &str) -> (String, Vec<ImageRef>) {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<img").map(|i| at + i) {
        let end = tag_end(html, i);
        let tag = &html[i..end];
        out.push_str(&html[at..i]);
        at = end;
        let Some(src) = tag_attr(tag, "src").map(|s| s.trim().replace("&amp;", "&")).filter(|s| !s.is_empty()) else { continue };
        let alt = tag_attr(tag, "alt").unwrap_or_default().trim().to_string();
        let n = images.len() + 1;
        let label = if alt.is_empty() { format!("[image {n}]") } else { format!("[image {n}: {}]", alt.replace(['[', ']'], "")) };
        out.push_str(&format!(" {} ", label.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")));
        images.push(ImageRef { n, alt, src, picture: None, skipped: None });
    }
    out.push_str(&html[at..]);
    (out, images)
}

fn tag_end(html: &str, start: usize) -> usize {
    let mut quote = None;
    for (j, c) in html[start..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return start + j + 1,
            _ => {}
        }
    }
    html.len()
}

/// The bytes behind an image `src`.
fn load(src: &str, opts: &Options, raw: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let lower = src.to_ascii_lowercase();
    if let Some(cid) = lower.strip_prefix("cid:").map(|_| &src[4..]) {
        return inline_part(&raw()?, cid);
    }
    if lower.starts_with("data:") {
        let (meta, data) = src[5..].split_once(',').ok_or_else(|| anyhow!("bad data: URL"))?;
        if !meta.ends_with(";base64") {
            return Err(anyhow!("unsupported data: URL"));
        }
        use base64::Engine;
        return Ok(base64::engine::general_purpose::STANDARD.decode(data.trim())?);
    }
    let url = if lower.starts_with("//") { format!("https:{src}") } else { src.to_string() };
    if !(url.to_ascii_lowercase().starts_with("https://") || url.to_ascii_lowercase().starts_with("http://")) {
        return Err(anyhow!("unsupported image address"));
    }
    if !opts.remote {
        return Err(anyhow!("remote images not loaded"));
    }
    fetch(&url)
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent("NightPost").build();
    let resp = agent.get(url).call().map_err(|e| anyhow!("couldn't load: {}", e.to_string().chars().take(120).collect::<String>()))?;
    let mut buf = Vec::new();
    resp.into_reader().take(MAX_DOWNLOAD + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_DOWNLOAD {
        return Err(anyhow!("image is too large"));
    }
    Ok(buf)
}

/// The MIME part with Content-ID `cid` in a raw message.
pub fn inline_part(raw: &[u8], cid: &str) -> Result<Vec<u8>> {
    use mail_parser::MimeHeaders;
    let msg = mail_parser::MessageParser::default().parse(raw).ok_or_else(|| anyhow!("couldn't read the message"))?;
    let want = cid.trim().trim_matches(['<', '>']);
    msg.parts
        .iter()
        .find(|p| p.content_id().map(|c| c.trim().trim_matches(['<', '>'])).is_some_and(|c| c.eq_ignore_ascii_case(want)))
        .map(|p| p.contents().to_vec())
        .ok_or_else(|| anyhow!("inline image {want} isn't in the message"))
}

/// Decode an image, turn it upright and size it for a model: at most
/// [`MAX_EDGE`] on the longest side, JPEG (PNG when it has transparency).
pub fn picture(data: &[u8]) -> Result<Picture> {
    picture_max(data, MAX_EDGE)
}

pub fn picture_max(data: &[u8], max_edge: u32) -> Result<Picture> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?;
    if reader.format().is_none() {
        return Err(anyhow!("not a picture NightPost can read (SVG and some formats aren't supported)"));
    }
    let mut decoder = reader.into_decoder().map_err(|e| anyhow!("couldn't read the picture: {e}"))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| anyhow!("couldn't read the picture: {e}"))?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > max_edge {
        img = img.resize(max_edge, max_edge, image::imageops::FilterType::Triangle);
    }
    let (width, height) = (img.width(), img.height());
    let mut out = Vec::new();
    let mime = if img.color().has_alpha() {
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&img.to_rgb8())?;
        "image/jpeg"
    };
    Ok(Picture { mime: mime.into(), data: out, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 90]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn marks_images_in_order() {
        let (html, imgs) = mark_images(r#"<p>Hi</p><img src="https://a.example/x.png" alt="Big sale"><img alt=x src='cid:logo@1'><img src="">"#);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].src, "https://a.example/x.png");
        assert_eq!(imgs[1].src, "cid:logo@1");
        assert!(html.contains("[image 1: Big sale]") && html.contains("[image 2: x]"));
        let text = crate::parse::html_to_text(&html);
        assert!(text.contains("[image 1: Big sale]"), "{text}");
    }

    #[test]
    fn sizes_pictures_down() {
        let p = picture(&png(3000, 1500)).unwrap();
        assert_eq!((p.width, p.height), (1200, 600));
        assert_eq!(p.mime, "image/jpeg");
        assert!(picture(b"<svg></svg>").is_err());
    }

    #[test]
    fn finds_inline_parts_and_data_urls() {
        use base64::Engine;
        let img = png(60, 60);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img);
        let raw = format!(
            "From: a@b.c\r\nTo: d@e.f\r\nSubject: x\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=B\r\n\r\n\
--B\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:pic1\">\r\n\
--B\r\nContent-Type: image/png\r\nContent-ID: <pic1>\r\nContent-Transfer-Encoding: base64\r\n\r\n{b64}\r\n--B--\r\n"
        );
        assert_eq!(inline_part(raw.as_bytes(), "pic1").unwrap(), img);
        let opts = Options { remote: false, max_images: 8 };
        assert_eq!(load(&format!("data:image/png;base64,{b64}"), &opts, || unreachable!()).unwrap(), img);
        assert!(load("https://x.example/a.png", &opts, || unreachable!()).is_err());
    }

    #[test]
    fn view_strips_trackers_and_skips_icons() {
        use base64::Engine;
        let store = Store::open(&crate::test_db_path("sp-view")).unwrap();
        crate::demo::seed(&store).unwrap();
        let t = store.threads(&crate::MailboxView::Everything, "", 1).unwrap().remove(0);
        let account = store.account(t.account_id).unwrap().unwrap();
        let big = base64::engine::general_purpose::STANDARD.encode(png(400, 200));
        let tiny = base64::engine::general_purpose::STANDARD.encode(png(16, 16));
        let mut m = store.thread_messages(account.id, &t.thread_key).unwrap().remove(0);
        m.body_html = Some(format!(
            "<p>Sale!</p><img src=\"data:image/png;base64,{big}\" alt=\"Banner\"><img src=\"data:image/png;base64,{tiny}\">\
             <img src=\"https://t.example/open.gif\" width=1 height=1>"
        ));
        let v = view(&store, &account, &m, &Options { remote: false, max_images: 8 }).unwrap();
        assert_eq!(v.trackers, vec!["t.example".to_string()]);
        assert_eq!(v.images.len(), 2);
        assert!(v.images[0].picture.is_some() && v.text.contains("[image 1: Banner]"));
        assert_eq!(v.images[1].skipped.as_deref(), Some("small icon"));
    }
}
