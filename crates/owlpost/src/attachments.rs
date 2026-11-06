//! Files attached in the composer, and shrinking photos before they're sent.

use owlpost_core::smtp::{mime_for, OutgoingAttachment};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How big attached photos are sent, like Apple Mail's Image Size menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ImageSize {
    #[default]
    Actual,
    Large,
    Medium,
    Small,
}

impl ImageSize {
    pub const ALL: [ImageSize; 4] = [
        ImageSize::Actual,
        ImageSize::Large,
        ImageSize::Medium,
        ImageSize::Small,
    ];

    /// The longest side, in pixels (`None`: leave the image alone).
    pub fn max_edge(self) -> Option<u32> {
        match self {
            ImageSize::Actual => None,
            ImageSize::Large => Some(2048),
            ImageSize::Medium => Some(1280),
            ImageSize::Small => Some(640),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageSize::Actual => "Actual Size",
            ImageSize::Large => "Large",
            ImageSize::Medium => "Medium",
            ImageSize::Small => "Small",
        }
    }
}

/// Photos we can shrink: JPEG and PNG (GIFs may be animated; HEIC can't be
/// decoded here, so those go as they are).
pub fn resizable(mime: &str) -> bool {
    matches!(mime, "image/jpeg" | "image/png")
}

/// Scale an image so its longest side is at most `max_edge`, keeping its
/// format. Phone photos are turned upright first, since their EXIF rotation
/// is lost on re-encoding. `None` when it's already small enough, can't be
/// read, or wouldn't come out smaller.
pub fn shrink(data: &[u8], mime: &str, max_edge: u32) -> Option<Vec<u8>> {
    use image::ImageDecoder;
    let format = match mime {
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/png" => image::ImageFormat::Png,
        _ => return None,
    };
    let mut decoder = image::ImageReader::with_format(std::io::Cursor::new(data), format)
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let (w, h) = decoder.dimensions();
    if w.max(h) <= max_edge {
        return None;
    }
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let img = img.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    match format {
        image::ImageFormat::Jpeg => {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
            img.to_rgb8().write_with_encoder(enc).ok()?;
        }
        _ => img
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?,
    }
    (out.len() < data.len()).then_some(out)
}

#[derive(Clone)]
enum Variant {
    /// Being resized on a background thread.
    Pending,
    /// The resized image.
    Ready(Arc<Vec<u8>>),
    /// Already that small (or couldn't be resized): send the original.
    Original,
}

/// One attached file.
pub struct Attached {
    pub name: String,
    pub mime: String,
    pub data: Arc<Vec<u8>>,
    variants: Arc<Mutex<HashMap<ImageSize, Variant>>>,
}

impl Attached {
    pub fn new(name: impl Into<String>, data: Vec<u8>) -> Self {
        let name = name.into();
        Self {
            mime: mime_for(&name).to_string(),
            name,
            data: Arc::new(data),
            variants: Default::default(),
        }
    }

    pub fn from_path(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        Ok(Self::new(name, data))
    }

    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    pub fn resizable(&self) -> bool {
        resizable(&self.mime)
    }

    /// Start resizing for `size` in the background, if needed. `done` runs
    /// when it's finished (to repaint the window).
    pub fn prepare(&self, size: ImageSize, done: impl FnOnce() + Send + 'static) {
        let Some(edge) = size.max_edge() else { return };
        if !self.resizable() {
            return;
        }
        let mut v = self.variants.lock().unwrap();
        if v.contains_key(&size) {
            return;
        }
        v.insert(size, Variant::Pending);
        let (data, mime, variants) = (self.data.clone(), self.mime.clone(), self.variants.clone());
        std::thread::spawn(move || {
            let r = shrink(&data, &mime, edge)
                .map(|b| Variant::Ready(Arc::new(b)))
                .unwrap_or(Variant::Original);
            variants.lock().unwrap().insert(size, r);
            done();
        });
    }

    /// Still resizing for `size`.
    pub fn pending(&self, size: ImageSize) -> bool {
        matches!(
            self.variants.lock().unwrap().get(&size),
            Some(Variant::Pending)
        )
    }

    /// The bytes sent at `size` (the original until a resize is ready).
    pub fn bytes(&self, size: ImageSize) -> Arc<Vec<u8>> {
        match self.variants.lock().unwrap().get(&size) {
            Some(Variant::Ready(b)) if size != ImageSize::Actual => b.clone(),
            _ => self.data.clone(),
        }
    }

    /// The attachment as sent, resizing now if the background job hasn't
    /// finished (or never started).
    pub fn outgoing(&self, size: ImageSize) -> OutgoingAttachment {
        if let Some(edge) = size.max_edge().filter(|_| self.resizable()) {
            let ready = matches!(
                self.variants.lock().unwrap().get(&size),
                Some(Variant::Ready(_) | Variant::Original)
            );
            if !ready {
                let r = shrink(&self.data, &self.mime, edge)
                    .map(|b| Variant::Ready(Arc::new(b)))
                    .unwrap_or(Variant::Original);
                self.variants.lock().unwrap().insert(size, r);
            }
        }
        OutgoingAttachment {
            filename: self.name.clone(),
            content_type: self.mime.clone(),
            data: self.bytes(size).to_vec(),
        }
    }
}

/// `2.4 MB`, `310 KB`.
pub fn human_size(bytes: usize) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{} KB", ((b / 1024.0).ceil() as u64).max(1))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A noisy photo-like JPEG, `w`×`h`, optionally marked as rotated.
    pub(crate) fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x ^ y) % 256) as u8,
            ])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
            .encode_image(&img)
            .unwrap();
        out
    }

    fn dims(b: &[u8]) -> (u32, u32) {
        let i = image::load_from_memory(b).unwrap();
        (i.width(), i.height())
    }

    #[test]
    fn shrinks_photos_to_each_size() {
        let a = Attached::new("IMG_0001.jpeg", jpeg(3000, 2000));
        assert!(a.resizable() && a.is_image());
        assert_eq!(dims(&a.outgoing(ImageSize::Actual).data), (3000, 2000));
        assert_eq!(dims(&a.outgoing(ImageSize::Large).data), (2048, 1365));
        assert_eq!(dims(&a.outgoing(ImageSize::Medium).data), (1280, 853));
        let small = a.outgoing(ImageSize::Small);
        assert_eq!(dims(&small.data), (640, 427));
        assert!(small.data.len() < a.data.len() / 4);
        assert_eq!(
            (small.filename.as_str(), small.content_type.as_str()),
            ("IMG_0001.jpeg", "image/jpeg")
        );

        // Already small: sent untouched.
        let tiny = Attached::new("icon.jpg", jpeg(300, 200));
        assert_eq!(*tiny.outgoing(ImageSize::Small).data, *tiny.data);
        // Not an image we resize.
        let pdf = Attached::new("Invoice.pdf", b"%PDF".to_vec());
        assert!(!pdf.resizable());
        assert_eq!(pdf.outgoing(ImageSize::Small).data, b"%PDF");
    }

    #[test]
    fn background_resize_finishes() {
        let a = Attached::new("photo.jpg", jpeg(1600, 1600));
        let (tx, rx) = std::sync::mpsc::channel();
        a.prepare(ImageSize::Medium, move || tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert!(!a.pending(ImageSize::Medium));
        assert_eq!(dims(&a.bytes(ImageSize::Medium)), (1280, 1280));
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(310 * 1024), "310 KB");
        assert_eq!(human_size(2_516_582), "2.4 MB");
        assert_eq!(human_size(10), "1 KB");
    }
}
