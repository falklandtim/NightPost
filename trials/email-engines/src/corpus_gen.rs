//! Writes the corpus's `.eml` files (`make-corpus`): emails with pictures
//! inside them (`multipart/related`, `cid:`), built the way mail programs
//! send them. Their pictures come from `images.rs`, so the files can be
//! remade at any time; the results are committed so every run uses the same
//! bytes.

use std::fmt::Write as _;

use anyhow::{Context, Result};
use base64::Engine as _;

use crate::images;

/// Write `07-inline-pictures.eml` and `12-long-digest.eml` into the corpus.
pub fn make() -> Result<()> {
    let pics = images::all()?;
    let pic = |name: &str| pics.get(name).map(|p| p.bytes.clone()).with_context(|| format!("no picture {name}"));
    let dir = crate::page::corpus_dir();

    let html07 = r#"<div style="font:15px/1.55 -apple-system,Helvetica,Arial,sans-serif;color:#222;max-width:600px;">
<p>Hi all,</p>
<p>Photos from Saturday's market stall, as promised. The first is the morning set-up, the second is the queue at ten.</p>
<p><img src="cid:photo-1@harbourside.example" alt="Stall set-up" width="320" style="border-radius:6px;"></p>
<p><img src="cid:photo-2@harbourside.example" alt="The queue" width="320" style="border-radius:6px;"></p>
<p>And the banner the printer sent over (this one is linked from their website, not attached):</p>
<p><img src="{{REMOTE}}/images/remote-banner.png" alt="Printer's banner proof" width="300"></p>
<p>Cheers,<br>Mere</p>
</div>"#;
    let parts07 = vec![("photo-1@harbourside.example", "photo-1.png", pic("photo-1.png")?), ("photo-2@harbourside.example", "photo-2.png", pic("photo-2.png")?)];
    std::fs::write(dir.join("07-inline-pictures.eml"), eml("Photos from the market", html07, &parts07))?;

    let mut html12 = String::from(
        "<div style=\"font:14px/1.5 -apple-system,Helvetica,Arial,sans-serif;color:#222;max-width:640px;margin:0 auto;\">\
         <h1 style=\"font-size:22px;\">The Example Digest: 200 things this week</h1>",
    );
    for i in 0..200 {
        let _ = write!(
            html12,
            "<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-bottom:1px solid #eee;\"><tr>\
             <td width=\"96\" valign=\"top\" style=\"padding:10px 12px 10px 0;\"><img src=\"cid:thumb-{t}@digest.example\" width=\"96\" height=\"72\" alt=\"\" style=\"display:block;border-radius:4px;\"></td>\
             <td valign=\"top\" style=\"padding:10px 0;\"><a href=\"https://digest.example/item/{i}\" style=\"font-weight:600;color:#1a4d8f;text-decoration:none;\">Item {n}: a headline about something interesting</a>\
             <p style=\"margin:4px 0 0;color:#555;\">A short paragraph summarising item {n}, long enough to wrap onto a second line in a normal reading pane so the layout has some work to do.</p></td></tr></table>",
            t = i % 10,
            n = i + 1
        );
    }
    html12.push_str("<p style=\"color:#999;font-size:12px;\">The Example Digest · <a href=\"https://digest.example/unsubscribe\">Unsubscribe</a></p></div>");
    let mut parts12 = Vec::new();
    let ids: Vec<String> = (0..10).map(|i| format!("thumb-{i}@digest.example")).collect();
    for (i, id) in ids.iter().enumerate() {
        parts12.push((id.as_str(), "thumb.png", pic(&format!("thumb-{i}.png"))?));
    }
    std::fs::write(dir.join("12-long-digest.eml"), eml("The Example Digest", &html12, &parts12))?;
    println!("wrote 07-inline-pictures.eml and 12-long-digest.eml in {}", dir.display());
    Ok(())
}

/// A `multipart/related` email: the HTML, then each picture by Content-ID.
fn eml(subject: &str, html: &str, parts: &[(&str, &str, Vec<u8>)]) -> String {
    let b = "=_trial_related_boundary";
    let mut out = format!(
        "From: Example Sender <sender@example.com>\r\nTo: Reader <reader@example.org>\r\nSubject: {subject}\r\n\
         Date: Thu, 14 May 2026 09:30:00 +1200\r\nMessage-ID: <{}@trial.example>\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/related; boundary=\"{b}\"; type=\"text/html\"\r\n\r\n\
         --{b}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{html}\r\n",
        subject.to_ascii_lowercase().replace(' ', "-")
    );
    for (id, file, bytes) in parts {
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        let _ = write!(
            out,
            "--{b}\r\nContent-Type: image/png; name=\"{file}\"\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <{id}>\r\n\
             Content-Disposition: inline; filename=\"{file}\"\r\n\r\n"
        );
        for line in data.as_bytes().chunks(76) {
            out.push_str(std::str::from_utf8(line).unwrap_or(""));
            out.push_str("\r\n");
        }
    }
    let _ = write!(out, "--{b}--\r\n");
    out
}
