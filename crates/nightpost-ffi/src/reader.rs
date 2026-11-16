//! Reading HTML email on the phone: each email's page, the pictures inside
//! it, and its attachments.
//!
//! The page is built by the same code as the desktop's
//! ([`nightpost_core::mail_html`]): tracking pixels stripped, a strict
//! Content Security Policy, and nothing remote until "Load images". The
//! phone shows it in a WKWebView with JavaScript turned off, and opens
//! links itself.
//!
//! Pictures sent inside an email (`cid:` links) need the whole message from
//! the server, so the last few are kept in memory while a conversation is
//! open.

use super::{fail, Mail, Result};
use nightpost_core::html_clean::has_remote_content;
use nightpost_core::{email_view, engine, mail_html, Message};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One email's HTML, ready for a web view.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MessagePage {
    pub html: String,
    /// Where the tracking pixels it carried pointed (they never load).
    pub trackers: Vec<String>,
    /// Remote images are hidden: offer "Load images".
    pub remote_hidden: bool,
}

/// A picture from inside an email.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InlineImage {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// Raw messages fetched for their pictures, newest last.
static RAW: Mutex<VecDeque<(i64, Arc<Vec<u8>>)>> = Mutex::new(VecDeque::new());
/// A conversation's worth, without holding much memory.
const RAW_KEPT: usize = 4;

/// The phone shows email on white paper, as the desktop does in a dark
/// theme unless "dark emails" is on; the card around it follows the theme.
const PAPER: &str = "html{background:#fff;} body{color:#1d1d1f;padding:2px 0;}";

/// A guess at a picture's type from its first bytes (the web view needs one).
fn image_type(data: &[u8]) -> &'static str {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ if data.starts_with(b"<svg") || data.starts_with(b"<?xml") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// A random nonce: nothing on the phone runs by it (JavaScript is off),
/// but the policy needs one.
fn nonce() -> String {
    let mut b = [0u8; 12];
    if getrandom::getrandom(&mut b).is_err() {
        return "nightpost".into();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The page for `m`'s HTML, or `None` when it has only text.
pub(crate) fn page(m: &Message, remote_images: bool) -> Option<MessagePage> {
    let html = m.body_html.as_deref().filter(|h| !h.trim().is_empty())?;
    let (clean, trackers) = nightpost_core::html_clean::clean(html);
    let remote_hidden = !remote_images && has_remote_content(&clean);
    let policy = mail_html::csp(&nonce(), remote_images);
    Some(MessagePage { html: mail_html::email_frame(&clean, &policy, PAPER, true), trackers, remote_hidden })
}

impl Mail {
    fn message(&self, message_id: i64) -> Result<Message> {
        self.store()?.message(message_id)?.ok_or_else(|| fail("that email no longer exists"))
    }
}

#[uniffi::export]
impl Mail {
    /// The HTML page for an email, or `None` when it's plain text (show
    /// `bodyText`). `remote_images` lets pictures from the web load.
    pub fn message_page(&self, message_id: i64, remote_images: bool) -> Result<Option<MessagePage>> {
        Ok(page(&self.message(message_id)?, remote_images))
    }

    /// A picture sent inside an email (`cid:` in its HTML). Fetches the
    /// message from the server the first time. Blocks on the network.
    pub fn inline_image(&self, message_id: i64, cid: String) -> Result<InlineImage> {
        let cached = RAW.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(id, _)| *id == message_id).map(|(_, r)| r.clone());
        let raw = match cached {
            Some(r) => r,
            None => {
                let m = self.message(message_id)?;
                let store = self.worker()?;
                let acc = self.account(&store, m.account_id)?;
                if acc.demo {
                    return Err(fail("the demo emails have no pictures inside"));
                }
                let r = Arc::new(engine::fetch_raw(&store, &acc, &m)?);
                let mut kept = RAW.lock().unwrap_or_else(|p| p.into_inner());
                kept.push_back((message_id, r.clone()));
                while kept.len() > RAW_KEPT {
                    kept.pop_front();
                }
                r
            }
        };
        let data = email_view::inline_part(&raw, &cid)?;
        Ok(InlineImage { mime_type: image_type(&data).to_string(), data })
    }

    /// Attachment `index` of an email as a file on the phone, to open or
    /// share: downloaded the first time, then kept in the cache. Blocks on
    /// the network.
    pub fn attachment_file(&self, message_id: i64, index: u32) -> Result<String> {
        let m = self.message(message_id)?;
        let store = self.worker()?;
        let acc = self.account(&store, m.account_id)?;
        let path = engine::attachment_file(&store, &acc, &m, index as usize)?;
        Ok(path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_types_are_sniffed() {
        assert_eq!(image_type(b"\x89PNG\r\n"), "image/png");
        assert_eq!(image_type(b"\xFF\xD8\xFF\xE0"), "image/jpeg");
        assert_eq!(image_type(b"GIF89a"), "image/gif");
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8"), "image/webp");
        assert_eq!(image_type(b""), "application/octet-stream");
    }
}
