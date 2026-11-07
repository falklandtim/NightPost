//! Sender pictures, like Canary's: a person's photo from Contacts, else the
//! company's logo, else the coloured letter.
//!
//! Logos come straight from the sender's side, never from a lookup service
//! (which would learn everyone who emails you):
//! 1. **BIMI**: the verified brand logo a company publishes in DNS
//!    (`default._bimi.<domain>` → an SVG). Banks, retailers and big senders.
//! 2. **Their website's icon**: `apple-touch-icon`/`icon` from the home
//!    page, or `/favicon.ico`.
//!
//! Each domain is looked up once and cached on disk for a month (misses
//! for a week). Personal addresses (gmail.com, icloud.com, …) never get a
//! logo, or every friend would wear Gmail's.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Pixel size pictures are stored and drawn from (2× for Retina).
pub const SIZE: u32 = 96;
const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);
const KEEP_MISS: Duration = Duration::from_secs(7 * 24 * 3600);
const TIMEOUT: Duration = Duration::from_secs(6);

/// A sender picture, as a square PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub png: Vec<u8>,
    /// Fills the whole circle (photos, BIMI logos). Otherwise it's a site
    /// icon, drawn smaller on a white disc.
    pub fill: bool,
}

/// Mailbox providers: an address here is a person, not a company.
const PERSONAL: &[&str] = &[
    "gmail.com", "googlemail.com", "icloud.com", "me.com", "mac.com", "outlook.com", "hotmail.com", "hotmail.co.uk", "hotmail.co.nz", "live.com",
    "live.co.uk", "msn.com", "yahoo.com", "yahoo.co.nz", "yahoo.co.uk", "yahoo.com.au", "ymail.com", "aol.com", "proton.me", "protonmail.com", "pm.me",
    "fastmail.com", "fastmail.fm", "hey.com", "gmx.com", "gmx.net", "gmx.de", "zoho.com", "mail.com", "yandex.com", "qq.com", "163.com",
    "xtra.co.nz", "orcon.net.nz", "slingshot.co.nz", "vodafone.co.nz", "clear.net.nz", "paradise.net.nz", "bigpond.com", "bigpond.net.au",
    "optusnet.com.au", "btinternet.com", "sky.com", "comcast.net", "verizon.net", "att.net",
];

/// Second-level labels under a country code that are public suffixes
/// (`co.nz`, `com.au`, `org.uk`…).
const SECOND_LEVEL: &[&str] = &["co", "com", "net", "org", "gov", "govt", "ac", "edu", "school", "geek", "gen", "iwi", "maori", "mil", "nom", "ltd", "plc", "or", "ne", "id"];

pub fn domain_of(email: &str) -> Option<String> {
    let d = email.rsplit_once('@')?.1.trim().trim_end_matches('>').to_ascii_lowercase();
    (d.contains('.') && !d.starts_with('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).then_some(d)
}

/// The registrable domain: `r.repco.co.nz` → `repco.co.nz`,
/// `mail.ollama.com` → `ollama.com`.
pub fn org_domain(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').collect();
    let n = labels.len();
    if n <= 2 {
        return domain.to_string();
    }
    let keep = if labels[n - 1].len() == 2 && SECOND_LEVEL.contains(&labels[n - 2]) { 3 } else { 2 };
    labels[n.saturating_sub(keep)..].join(".")
}

pub fn is_personal(domain: &str) -> bool {
    PERSONAL.contains(&domain) || PERSONAL.contains(&org_domain(domain).as_str())
}

/// Made-up domains (demo data, tests) are never looked up.
pub fn is_reserved(domain: &str) -> bool {
    let tld = domain.rsplit('.').next().unwrap_or("");
    matches!(tld, "demo" | "example" | "test" | "invalid" | "local" | "localhost" | "internal")
        || matches!(org_domain(domain).as_str(), "example.com" | "example.org" | "example.net")
}

/// The picture for a sender, or `None` for the letter.
pub fn find(email: &str, cache: &Path) -> Option<Picture> {
    let domain = domain_of(email)?;
    if is_reserved(&domain) {
        return None;
    }
    if let Some(png) = contacts::photo(email).and_then(|b| normalize(&b, true)) {
        return Some(Picture { png, fill: true });
    }
    if is_personal(&domain) {
        return None;
    }
    logo(&org_domain(&domain), &domain, cache)
}

/// A company's logo, cached per registrable domain.
fn logo(org: &str, domain: &str, cache: &Path) -> Option<Picture> {
    let fresh = |p: &Path, keep: Duration| {
        std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < keep)
    };
    let fill_path = cache.join(format!("{org}.bimi.png"));
    let icon_path = cache.join(format!("{org}.icon.png"));
    let miss_path = cache.join(format!("{org}.none"));
    for (p, fill) in [(&fill_path, true), (&icon_path, false)] {
        if fresh(p, KEEP) {
            if let Ok(png) = std::fs::read(p) {
                return Some(Picture { png, fill });
            }
        }
    }
    if fresh(&miss_path, KEEP_MISS) {
        return None;
    }
    let found = fetch_logo(org, domain);
    let _ = std::fs::create_dir_all(cache);
    for p in [&fill_path, &icon_path, &miss_path] {
        let _ = std::fs::remove_file(p);
    }
    match &found {
        Some(pic) => {
            let _ = std::fs::write(if pic.fill { &fill_path } else { &icon_path }, &pic.png);
        }
        None => {
            let _ = std::fs::write(&miss_path, b"");
        }
    }
    found
}

fn fetch_logo(org: &str, domain: &str) -> Option<Picture> {
    let mut bimi_domains = vec![domain.to_string()];
    if org != domain {
        bimi_domains.push(org.to_string());
    }
    for d in &bimi_domains {
        if let Some(url) = dns::txt(&format!("default._bimi.{d}")).iter().find_map(|r| bimi_logo_url(r)) {
            if let Some(png) = get(&url, 512 * 1024).and_then(|(b, _)| normalize(&b, true)) {
                log::debug!("logo for {org}: BIMI");
                return Some(Picture { png, fill: true });
            }
        }
    }
    for site in [org.to_string(), format!("www.{org}")] {
        if let Some(png) = site_icon(&site) {
            log::debug!("logo for {org}: site icon");
            return Some(Picture { png, fill: false });
        }
    }
    None
}

/// The `l=` logo URL of a BIMI record (`v=BIMI1; l=https://…svg; a=…`).
pub fn bimi_logo_url(record: &str) -> Option<String> {
    let mut tags = record.split(';').map(str::trim);
    if !tags.next()?.eq_ignore_ascii_case("v=BIMI1") {
        return None;
    }
    tags.find_map(|t| t.strip_prefix("l=").or_else(|| t.strip_prefix("L=")))
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("https://"))
}

/// The best icon a site offers, as a normalised PNG.
fn site_icon(site: &str) -> Option<Vec<u8>> {
    let mut candidates = vec![];
    if let Some((html, base)) = get(&format!("https://{site}/"), 384 * 1024) {
        candidates = page_icons(&String::from_utf8_lossy(&html), &base);
    }
    candidates.push(format!("https://{site}/apple-touch-icon.png"));
    candidates.push(format!("https://{site}/favicon.ico"));
    candidates.dedup();
    candidates.iter().take(5).find_map(|u| get(u, 512 * 1024).and_then(|(b, _)| normalize(&b, false)))
}

/// Icon links in a page, best first: apple-touch-icon (180 px), then
/// `icon`s by declared size (SVG counts as large).
pub fn page_icons(html: &str, base: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found: Vec<(u32, String)> = vec![];
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link").map(|i| at + i) {
        let end = lower[i..].find('>').map(|e| i + e + 1).unwrap_or(lower.len());
        let tag = &html[i..end];
        at = end;
        let (Some(rel), Some(href)) = (crate::mail_view::tag_attr(tag, "rel"), crate::mail_view::tag_attr(tag, "href")) else { continue };
        let rel = rel.to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_whitespace().collect();
        let size = crate::mail_view::tag_attr(tag, "sizes").and_then(|s| s.split(['x', 'X']).next().and_then(|n| n.trim().parse::<u32>().ok()));
        let svg = href.to_ascii_lowercase().ends_with(".svg") || crate::mail_view::tag_attr(tag, "type").is_some_and(|t| t.contains("svg"));
        let score = if rels.iter().any(|r| r.starts_with("apple-touch-icon")) {
            size.unwrap_or(180).max(152)
        } else if rels.contains(&"icon") || rels.contains(&"shortcut") {
            if svg { 200 } else { size.unwrap_or(16) }
        } else {
            continue;
        };
        if let Some(url) = resolve(base, &href.replace("&amp;", "&")) {
            found.push((score, url));
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, u)| u).collect()
}

/// `href` resolved against the page's URL.
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("https://") || href.starts_with("http://") {
        return Some(href.to_string());
    }
    if href.starts_with("data:") || href.is_empty() {
        return None;
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if href.starts_with('/') {
        return Some(format!("{scheme}://{host}{href}"));
    }
    let path = &rest[host.len()..];
    let dir = path.split(['?', '#']).next().unwrap_or("").rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    Some(format!("{scheme}://{host}{dir}/{href}"))
}

/// GET a URL (following redirects), at most `limit` bytes. Returns the body
/// and the final URL.
fn get(url: &str, limit: u64) -> Option<(Vec<u8>, String)> {
    if cfg!(test) {
        return None;
    }
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(5).user_agent("Mozilla/5.0 (Macintosh) OwlPost").build();
    let r = agent.get(url).call().map_err(|e| log::debug!("{url}: {e}")).ok()?;
    let final_url = r.get_url().to_string();
    let mut body = vec![];
    r.into_reader().take(limit).read_to_end(&mut body).ok()?;
    Some((body, final_url))
}

/// Any image (PNG, JPEG, ICO, WebP, GIF or SVG) as a `SIZE`² PNG: photos
/// and BIMI logos cropped to fill, icons fitted. Tiny or blank images are
/// refused.
pub fn normalize(bytes: &[u8], fill: bool) -> Option<Vec<u8>> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    let img = if head.contains("<svg") || head.contains("<?xml") {
        render_svg(bytes)?
    } else {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        if img.width().min(img.height()) < 16 {
            return None;
        }
        img
    };
    let (w, h) = img.dimensions();
    let out = if fill {
        let side = w.min(h);
        let crop = image::imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image();
        image::imageops::resize(&crop, SIZE, SIZE, image::imageops::FilterType::Lanczos3)
    } else {
        let scale = SIZE as f32 / w.max(h) as f32;
        let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let filter = if scale > 2.0 { image::imageops::FilterType::CatmullRom } else { image::imageops::FilterType::Lanczos3 };
        let small = image::imageops::resize(&img, nw, nh, filter);
        let mut canvas = image::RgbaImage::new(SIZE, SIZE);
        image::imageops::overlay(&mut canvas, &small, ((SIZE - nw) / 2) as i64, ((SIZE - nh) / 2) as i64);
        canvas
    };
    // A fully transparent "logo" is no logo.
    if out.pixels().all(|p| p.0[3] < 8) {
        return None;
    }
    let mut png = vec![];
    image::DynamicImage::ImageRgba8(out).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn render_svg(bytes: &[u8]) -> Option<image::RgbaImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let px = SIZE * 2;
    let scale = px as f32 / size.width().max(size.height());
    let (dx, dy) = ((px as f32 - size.width() * scale) / 2.0, (px as f32 - size.height() * scale) / 2.0);
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy), &mut pixmap.as_mut());
    let rgba: Vec<u8> = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect();
    image::RgbaImage::from_raw(px, px, rgba)
}

/// Where logos are cached: next to the attachment cache.
pub fn cache_dir() -> PathBuf {
    owlpost_core::engine::attachment_cache_dir().with_file_name("logos")
}

/// The UI's side: pictures by address, as textures for egui and data URIs
/// for the web view. Missing ones are requested from the worker once.
pub struct Faces {
    ctx: eframe::egui::Context,
    tx: crossbeam_channel::Sender<String>,
    map: std::cell::RefCell<std::collections::HashMap<String, Face>>,
    /// Preferences → Appearance → Show sender logos and photos.
    pub enabled: std::cell::Cell<bool>,
}

enum Face {
    Pending,
    Letter,
    Ready { texture: eframe::egui::TextureHandle, fill: bool, uri: String },
}

impl Faces {
    pub fn new(ctx: &eframe::egui::Context, tx: crossbeam_channel::Sender<String>) -> Self {
        Self { ctx: ctx.clone(), tx, map: Default::default(), enabled: std::cell::Cell::new(true) }
    }

    fn lookup<T>(&self, email: &str, f: impl FnOnce(&Face) -> Option<T>) -> Option<T> {
        if !self.enabled.get() || email.is_empty() {
            return None;
        }
        let key = email.trim().to_ascii_lowercase();
        let mut map = self.map.borrow_mut();
        match map.get(&key) {
            Some(face) => f(face),
            None => {
                map.insert(key.clone(), Face::Pending);
                let _ = self.tx.send(key);
                None
            }
        }
    }

    /// The texture for a sender, and whether it fills the circle.
    pub fn texture(&self, email: &str) -> Option<(eframe::egui::TextureId, bool)> {
        self.lookup(email, |f| match f {
            Face::Ready { texture, fill, .. } => Some((texture.id(), *fill)),
            _ => None,
        })
    }

    /// The picture as a `data:` URI for the web view.
    pub fn data_uri(&self, email: &str) -> Option<(String, bool)> {
        self.lookup(email, |f| match f {
            Face::Ready { uri, fill, .. } => Some((uri.clone(), *fill)),
            _ => None,
        })
    }

    /// A picture arrived from the worker.
    pub fn insert(&self, email: String, pic: Option<Picture>) {
        use base64::Engine;
        let face = pic
            .and_then(|p| {
                let img = image::load_from_memory(&p.png).ok()?.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                let color = eframe::egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                let texture = self.ctx.load_texture(format!("avatar:{email}"), color, eframe::egui::TextureOptions::LINEAR);
                let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&p.png));
                Some(Face::Ready { texture, fill: p.fill, uri })
            })
            .unwrap_or(Face::Letter);
        self.map.borrow_mut().insert(email, face);
    }

    /// Forget everything (e.g. after turning pictures back on).
    pub fn clear(&self) {
        self.map.borrow_mut().clear();
    }

    /// Draw a sender's picture, or their letter while there's none.
    pub fn paint(&self, painter: &eframe::egui::Painter, center: eframe::egui::Pos2, name: &str, email: &str, size: f32) {
        use eframe::egui;
        let Some((tex, fill)) = self.texture(email) else {
            crate::style::paint_avatar(painter, center, name, email, size);
            return;
        };
        let rect = egui::Rect::from_center_size(center, egui::vec2(size, size));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        if fill {
            painter.add(egui::epaint::RectShape::filled(rect, size / 2.0, egui::Color32::WHITE).with_texture(tex, uv));
        } else {
            // Site icons sit on a white disc, like Canary's.
            painter.circle(center, size / 2.0, egui::Color32::WHITE, egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(18)));
            painter.image(tex, rect.shrink(size * 0.2), uv, egui::Color32::WHITE);
        }
    }

    pub fn show(&self, ui: &mut eframe::egui::Ui, name: &str, email: &str, size: f32) {
        let (rect, _) = ui.allocate_exact_size(eframe::egui::vec2(size, size), eframe::egui::Sense::hover());
        self.paint(ui.painter(), rect.center(), name, email, size);
    }
}

/// A tiny DNS client for TXT records, using the system's resolvers — so no
/// third party sees which senders you look up.
pub mod dns {
    use std::net::UdpSocket;
    use std::time::Duration;

    pub fn txt(name: &str) -> Vec<String> {
        if cfg!(test) {
            return vec![];
        }
        for server in nameservers() {
            if let Some(answers) = query(&server, name) {
                return answers;
            }
        }
        vec![]
    }

    fn nameservers() -> Vec<String> {
        let conf = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        let mut v: Vec<String> = conf
            .lines()
            .filter_map(|l| l.trim().strip_prefix("nameserver"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| if s.contains(':') { format!("[{s}]:53") } else { format!("{s}:53") })
            .collect();
        v.truncate(3);
        v
    }

    /// `None` when the server didn't answer; `Some(vec![])` for "no records".
    fn query(server: &str, name: &str) -> Option<Vec<String>> {
        let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1) & 0xffff) as u16;
        let packet = build_query(id, name)?;
        let bind = if server.starts_with('[') { "[::]:0" } else { "0.0.0.0:0" };
        let sock = UdpSocket::bind(bind).ok()?;
        sock.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        sock.connect(server).ok()?;
        for _ in 0..2 {
            sock.send(&packet).ok()?;
            let mut buf = [0u8; 4096];
            if let Ok(n) = sock.recv(&mut buf) {
                if let Some(a) = parse_txt(&buf[..n], id) {
                    return Some(a);
                }
            }
        }
        None
    }

    pub fn build_query(id: u16, name: &str) -> Option<Vec<u8>> {
        let mut p = vec![];
        p.extend(id.to_be_bytes());
        p.extend([0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion desired, one question
        for label in name.trim_end_matches('.').split('.') {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            p.push(label.len() as u8);
            p.extend(label.as_bytes());
        }
        p.extend([0, 0, 16, 0, 1]); // TXT, IN
        Some(p)
    }

    fn skip_name(b: &[u8], mut i: usize) -> Option<usize> {
        loop {
            let len = *b.get(i)? as usize;
            if len == 0 {
                return Some(i + 1);
            }
            if len & 0xC0 == 0xC0 {
                return Some(i + 2);
            }
            i += 1 + len;
        }
    }

    /// The TXT strings in a response (each record's strings joined).
    pub fn parse_txt(b: &[u8], id: u16) -> Option<Vec<String>> {
        if b.len() < 12 || u16::from_be_bytes([b[0], b[1]]) != id || b[2] & 0x80 == 0 {
            return None;
        }
        let rcode = b[3] & 0x0f;
        if rcode == 3 {
            return Some(vec![]); // no such name
        }
        if rcode != 0 {
            return None;
        }
        let qd = u16::from_be_bytes([b[4], b[5]]) as usize;
        let an = u16::from_be_bytes([b[6], b[7]]) as usize;
        let mut i = 12;
        for _ in 0..qd {
            i = skip_name(b, i)? + 4;
        }
        let mut out = vec![];
        for _ in 0..an {
            i = skip_name(b, i)?;
            let kind = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]);
            let len = u16::from_be_bytes([*b.get(i + 8)?, *b.get(i + 9)?]) as usize;
            let data = b.get(i + 10..i + 10 + len)?;
            i += 10 + len;
            if kind == 16 {
                let mut s = String::new();
                let mut j = 0;
                while j < data.len() {
                    let n = data[j] as usize;
                    s.push_str(&String::from_utf8_lossy(data.get(j + 1..j + 1 + n)?));
                    j += 1 + n;
                }
                out.push(s);
            }
        }
        Some(out)
    }
}

/// Photos from the Mac's Contacts. Only inside OwlPost.app, whose
/// Info.plist explains why it asks (macOS ends any app that asks without).
#[cfg(all(target_os = "macos", not(test)))]
mod contacts {
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2_contacts::{CNAuthorizationStatus, CNContact, CNContactStore, CNContactThumbnailImageDataKey, CNEntityType, CNKeyDescriptor};
    use objc2_foundation::{ns_string, NSArray, NSBundle, NSError, NSString};
    use std::sync::OnceLock;

    fn allowed() -> bool {
        static ALLOWED: OnceLock<bool> = OnceLock::new();
        *ALLOWED.get_or_init(|| unsafe {
            if NSBundle::mainBundle().objectForInfoDictionaryKey(ns_string!("NSContactsUsageDescription")).is_none() {
                return false;
            }
            let status = CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts);
            if status == CNAuthorizationStatus::NotDetermined {
                // Ask once; this thread waits for the answer.
                let (tx, rx) = std::sync::mpsc::channel();
                let block = block2::RcBlock::new(move |granted: Bool, _e: *mut NSError| {
                    let _ = tx.send(granted.as_bool());
                });
                CNContactStore::new().requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block);
                return rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or(false);
            }
            status == CNAuthorizationStatus::Authorized || status == CNAuthorizationStatus::Limited
        })
    }

    pub fn photo(email: &str) -> Option<Vec<u8>> {
        if !allowed() {
            return None;
        }
        unsafe {
            let store = CNContactStore::new();
            let predicate = CNContact::predicateForContactsMatchingEmailAddress(&NSString::from_str(email));
            let key: &ProtocolObject<dyn CNKeyDescriptor> = ProtocolObject::from_ref(CNContactThumbnailImageDataKey);
            let keys = NSArray::from_slice(&[key]);
            let found = store.unifiedContactsMatchingPredicate_keysToFetch_error(&predicate, &keys).ok()?;
            found.iter().find_map(|c| c.thumbnailImageData()).map(|d| d.to_vec())
        }
    }
}

#[cfg(not(all(target_os = "macos", not(test))))]
mod contacts {
    pub fn photo(_email: &str) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains() {
        assert_eq!(domain_of("Members <members@r.repco.co.nz>").as_deref(), Some("r.repco.co.nz"));
        assert_eq!(org_domain("r.repco.co.nz"), "repco.co.nz");
        assert_eq!(org_domain("mail.ollama.com"), "ollama.com");
        assert_eq!(org_domain("ollama.com"), "ollama.com");
        assert_eq!(org_domain("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(org_domain("em.expedia.co.nz"), "expedia.co.nz");
        assert!(is_personal("gmail.com") && is_personal("xtra.co.nz") && !is_personal("timson.co"));
        assert!(is_reserved("icloud.demo") && is_reserved("example.com") && !is_reserved("ollama.com"));
        assert_eq!(find("alex@icloud.demo", Path::new("/nonexistent")), None);
    }

    #[test]
    fn bimi_records() {
        assert_eq!(
            bimi_logo_url("v=BIMI1; l=https://vmc.digicert.com/c730.svg; a=https://vmc.digicert.com/c730.pem").as_deref(),
            Some("https://vmc.digicert.com/c730.svg")
        );
        assert_eq!(bimi_logo_url("v=BIMI1;l=https://x.example/l.svg;a=").as_deref(), Some("https://x.example/l.svg"));
        assert_eq!(bimi_logo_url("v=BIMI1; l=; a=;"), None, "declined");
        assert_eq!(bimi_logo_url("v=spf1 include:_spf.google.com ~all"), None);
    }

    #[test]
    fn page_icons_best_first() {
        let html = r#"<head><link rel="stylesheet" href="/a.css"><link rel="shortcut icon" href="/favicon.ico">
            <link rel="icon" type="image/png" sizes="32x32" href="/icons/32.png"><link rel=icon sizes=192x192 href="icons/192.png">
            <link rel="apple-touch-icon" href="https://cdn.example.org/touch.png?v=1&amp;x=2"></head>"#;
        assert_eq!(
            page_icons(html, "https://www.example.org/shop/index.html"),
            [
                "https://www.example.org/shop/icons/192.png",
                "https://cdn.example.org/touch.png?v=1&x=2",
                "https://www.example.org/icons/32.png",
                "https://www.example.org/favicon.ico"
            ]
        );
        assert_eq!(resolve("https://a.nz", "//b.nz/i.png").as_deref(), Some("https://b.nz/i.png"));
        assert_eq!(resolve("https://a.nz", "x.png").as_deref(), Some("https://a.nz/x.png"));
    }

    #[test]
    fn dns_txt_round_trip() {
        let q = dns::build_query(0x1234, "default._bimi.ollama.com").unwrap();
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
        // A response: the question, then a TXT answer split into two strings,
        // its name compressed to point at the question.
        let mut r = q.clone();
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 1;
        let (a, b) = ("v=BIMI1; l=https://x.example/", "logo.svg");
        r.extend([0xC0, 12, 0, 16, 0, 1, 0, 0, 1, 0]);
        r.extend(((a.len() + b.len() + 2) as u16).to_be_bytes());
        r.push(a.len() as u8);
        r.extend(a.as_bytes());
        r.push(b.len() as u8);
        r.extend(b.as_bytes());
        assert_eq!(dns::parse_txt(&r, 0x1234), Some(vec!["v=BIMI1; l=https://x.example/logo.svg".to_string()]));
        assert_eq!(dns::parse_txt(&r, 0x9999), None, "wrong id");
        let mut nx = q.clone();
        nx[2] = 0x81;
        nx[3] = 0x83;
        assert_eq!(dns::parse_txt(&nx, 0x1234), Some(vec![]));
    }

    #[test]
    fn images_become_square_pngs() {
        // A wide red PNG, fitted (icon) and cropped (fill).
        let mut src = vec![];
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 32, image::Rgba([200, 0, 0, 255])))
            .write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        for fill in [false, true] {
            let out = image::load_from_memory(&normalize(&src, fill).unwrap()).unwrap().to_rgba8();
            assert_eq!(out.dimensions(), (SIZE, SIZE));
            let corner = out.get_pixel(1, 1).0[3];
            assert_eq!(corner > 0, fill, "fit leaves the corners clear; fill covers them");
        }
        // SVG (a BIMI-style square logo).
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" fill="#d00"/><circle cx="50" cy="50" r="30" fill="#fff"/></svg>"##;
        let out = image::load_from_memory(&normalize(svg, true).unwrap()).unwrap().to_rgba8();
        assert_eq!(out.get_pixel(SIZE / 2, SIZE / 2).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(2, 2).0[0], 0xdd);
        // Too small, or not an image.
        let mut tiny = vec![];
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 8)).write_to(&mut std::io::Cursor::new(&mut tiny), image::ImageFormat::Png).unwrap();
        assert_eq!(normalize(&tiny, false), None);
        assert_eq!(normalize(b"<html>nope</html>", false), None);
    }
}
