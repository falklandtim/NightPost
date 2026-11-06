//! A small HTML renderer for signatures: text with bold / italic /
//! underline / colour, links, line breaks and blocks, and images loaded from
//! the web. It isn't a browser; it covers what email signatures use.

use crate::style::Colors;
use eframe::egui::{self, Color32, RichText};

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text { text: String, bold: bool, italic: bool, underline: bool, link: Option<String>, color: Option<Color32> },
    Image { src: String, w: f32, h: f32, link: Option<String> },
}

/// Lines of inline pieces; an empty line is a blank line.
#[derive(Debug, Default, PartialEq)]
pub struct Doc {
    lines: Vec<Vec<Piece>>,
}

#[derive(Clone, Default)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    link: Option<String>,
    color: Option<Color32>,
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "nbsp" => Some('\u{a0}'),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "middot" => Some('·'),
            "copy" => Some('©'),
            "bull" => Some('•'),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `#rgb`, `#rrggbb`, `rgb(r,g,b)` or a few names.
fn parse_color(v: &str) -> Option<Color32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let hex: String = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.to_string() };
        let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
        return Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    if let Some(args) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")) {
        let p: Vec<u8> = args.trim_end_matches(')').split(',').take(3).filter_map(|x| x.trim().parse().ok()).collect();
        return (p.len() == 3).then(|| Color32::from_rgb(p[0], p[1], p[2]));
    }
    Some(match v.as_str() {
        "black" => Color32::BLACK,
        "white" => Color32::WHITE,
        "gray" | "grey" => Color32::GRAY,
        "red" => Color32::from_rgb(200, 30, 30),
        "blue" => Color32::from_rgb(30, 80, 220),
        "green" => Color32::from_rgb(30, 140, 60),
        _ => return None,
    })
}

fn css(style: &str, prop: &str) -> Option<String> {
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(prop)).then(|| v.trim().to_string())
    })
}

fn px(v: &str) -> Option<f32> {
    v.trim().trim_end_matches("px").trim().parse().ok()
}

const BLOCKS: &[&str] = &["div", "p", "li", "tr", "table", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "hr"];

/// Parse signature HTML into renderable lines.
pub fn parse(html: &str) -> Doc {
    let html = springpost_core::parse::clean_signature_html(html);
    let mut doc = Doc::default();
    let mut line: Vec<Piece> = vec![];
    let mut stack: Vec<(String, Style)> = vec![];
    let mut style = Style::default();
    let mut rest = html.as_str();
    // Whitespace collapses like HTML: one space between words, none at line starts.
    let push_text = |line: &mut Vec<Piece>, style: &Style, raw: &str| {
        let decoded = decode_entities(raw);
        let mut text = String::new();
        let mut last_space = line.last().map(|p| matches!(p, Piece::Text { text, .. } if text.ends_with(' '))).unwrap_or(true);
        for c in decoded.chars() {
            if c == '\u{a0}' {
                text.push(' ');
                last_space = false; // non-breaking spaces are kept
            } else if c.is_whitespace() {
                if !last_space {
                    text.push(' ');
                    last_space = true;
                }
            } else {
                text.push(c);
                last_space = false;
            }
        }
        if text.is_empty() {
            return;
        }
        line.push(Piece::Text { text, bold: style.bold, italic: style.italic, underline: style.underline, link: style.link.clone(), color: style.color });
    };
    let end_line = |doc: &mut Doc, line: &mut Vec<Piece>, force: bool| {
        // Trim trailing spaces of the line.
        if let Some(Piece::Text { text, .. }) = line.last_mut() {
            let t = text.trim_end_matches(' ').to_string();
            *text = t;
        }
        line.retain(|p| !matches!(p, Piece::Text { text, .. } if text.trim().is_empty()));
        if !line.is_empty() || force {
            doc.lines.push(std::mem::take(line));
        }
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut line, &style, rest);
            break;
        };
        if lt > 0 {
            push_text(&mut line, &style, &rest[..lt]);
        }
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map(|e| &rest[e + 3..]).unwrap_or("");
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if closing {
            if BLOCKS.contains(&name.as_str()) {
                end_line(&mut doc, &mut line, false);
            }
            if let Some(pos) = stack.iter().rposition(|(n, _)| *n == name) {
                style = stack[pos].1.clone();
                stack.truncate(pos);
            }
            continue;
        }
        let full = format!("<{tag}>");
        match name.as_str() {
            "br" => end_line(&mut doc, &mut line, true),
            "img" => {
                let Some(src) = springpost_core::parse::attr(&full, "src").map(|s| decode_entities(&s)) else { continue };
                let st = springpost_core::parse::attr(&full, "style").unwrap_or_default();
                let w = springpost_core::parse::attr(&full, "width").and_then(|v| px(&v)).or_else(|| css(&st, "width").and_then(|v| px(&v)));
                let h = springpost_core::parse::attr(&full, "height").and_then(|v| px(&v)).or_else(|| css(&st, "height").and_then(|v| px(&v)));
                let (w, h) = match (w, h) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w),
                    (None, Some(h)) => (h, h),
                    (None, None) => (120.0, 120.0),
                };
                line.push(Piece::Image { src, w, h, link: style.link.clone() });
            }
            _ => {
                if BLOCKS.contains(&name.as_str()) {
                    end_line(&mut doc, &mut line, false);
                }
                if tag.ends_with('/') {
                    continue;
                }
                stack.push((name.clone(), style.clone()));
                match name.as_str() {
                    "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => style.bold = true,
                    "i" | "em" => style.italic = true,
                    "u" => style.underline = true,
                    "a" => style.link = springpost_core::parse::attr(&full, "href").map(|h| decode_entities(&h)),
                    _ => {}
                }
                if let Some(c) = springpost_core::parse::attr(&full, "color").and_then(|c| parse_color(&c)) {
                    style.color = Some(c);
                }
                if let Some(st) = springpost_core::parse::attr(&full, "style") {
                    if let Some(c) = css(&st, "color").and_then(|c| parse_color(&c)) {
                        style.color = Some(c);
                    }
                    if css(&st, "font-weight").is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)) {
                        style.bold = true;
                    }
                    if css(&st, "font-style").is_some_and(|v| v == "italic") {
                        style.italic = true;
                    }
                }
            }
        }
    }
    end_line(&mut doc, &mut line, false);
    // No blank lines at the very start or end.
    while doc.lines.first().is_some_and(Vec::is_empty) {
        doc.lines.remove(0);
    }
    while doc.lines.last().is_some_and(Vec::is_empty) {
        doc.lines.pop();
    }
    doc
}

/// Draw signature HTML. Links open in the browser; images load from the web.
pub fn show(ui: &mut egui::Ui, col: &Colors, html: &str) {
    let doc = parse(html);
    let max_w = ui.available_width();
    ui.spacing_mut().item_spacing.y = 2.0;
    // Text rows as tall as the text, like an email client (not button height).
    ui.spacing_mut().interact_size.y = ui.text_style_height(&egui::TextStyle::Body);
    for line in &doc.lines {
        if line.is_empty() {
            ui.add_space(8.0);
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for p in line {
                match p {
                    Piece::Text { text, bold, italic, underline, link, color } => {
                        let mut rt = RichText::new(text).color(color.unwrap_or(if *bold { col.heading } else { col.text }));
                        if *bold {
                            rt = rt.strong();
                        }
                        if *italic {
                            rt = rt.italics();
                        }
                        if *underline {
                            rt = rt.underline();
                        }
                        match link {
                            Some(href) => {
                                ui.hyperlink_to(rt.color(color.unwrap_or(col.link)), href);
                            }
                            None if *bold => {
                                // egui's font has no bold weight: draw it twice, half a point apart.
                                let galley = egui::WidgetText::from(rt).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::FontSelection::Default);
                                let r = ui.label(galley.clone());
                                ui.painter().galley(r.rect.min + egui::vec2(0.5, 0.0), galley, col.heading);
                            }
                            None => {
                                ui.label(rt);
                            }
                        }
                    }
                    Piece::Image { src, w, h, link } => {
                        let scale = (max_w / w).min(1.0);
                        let img = egui::Image::new(src.as_str())
                            .fit_to_exact_size(egui::vec2(w * scale, h * scale))
                            .corner_radius(2)
                            .show_loading_spinner(true);
                        let r = ui.add(img.sense(egui::Sense::click()));
                        if let Some(href) = link {
                            if r.clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(href));
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &[Piece]) -> String {
        line.iter()
            .map(|p| match p {
                Piece::Text { text, .. } => text.clone(),
                Piece::Image { .. } => "[img]".into(),
            })
            .collect()
    }

    #[test]
    fn parses_canary_signature() {
        let doc = parse(include_str!("../../springpost-core/tests/fixtures/canary-signature.html"));
        let lines: Vec<String> = doc.lines.iter().map(|l| texts(l)).collect();
        assert_eq!(lines[0].trim_end(), "Tim Morrison");
        assert!(matches!(&doc.lines[0][0], Piece::Text { bold: true, .. }));
        assert_eq!(lines[1], "founder & creative");
        assert!(matches!(&doc.lines[1][0], Piece::Text { italic: true, .. }));
        let img = doc.lines.iter().flatten().find_map(|p| match p {
            Piece::Image { src, w, h, .. } => Some((src.clone(), *w, *h)),
            _ => None,
        });
        let (src, w, h) = img.expect("image");
        assert!(src.starts_with("https://www.dropbox.com/") && src.contains("&st=") && !src.contains("&amp;"), "{src}");
        assert_eq!((w, h), (250.0, 250.0));
        let link = doc.lines.iter().flatten().find_map(|p| match p {
            Piece::Text { link: Some(l), text, .. } if text == "tim@timson.co" => Some(l.clone()),
            _ => None,
        });
        assert_eq!(link.as_deref(), Some("mailto:tim@timson.co"));
        assert!(lines.iter().any(|l| l == "HEADSHOTS THAT MAKE YOU LOOK AWESOME FOR LESS"));
    }

    #[test]
    fn entities_and_colours() {
        assert_eq!(decode_entities("a&nbsp;&amp;&#233;&#x41;&bogus;"), "a\u{a0}&éA&bogus;");
        assert_eq!(parse_color("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color32::from_rgb(1, 2, 3)));
        let doc = parse("<span style=\"color:#ff0000; font-weight:700\">Hi</span><br><br>there");
        assert!(matches!(&doc.lines[0][0], Piece::Text { bold: true, color: Some(c), .. } if *c == Color32::from_rgb(255, 0, 0)));
        assert_eq!(doc.lines.len(), 3);
    }
}

/// Decodes web images by looking at their bytes, not the server's
/// Content-Type. Dropbox, for one, serves shared images as
/// `application/json` + attachment, which egui's stock loader refuses; mail
/// apps sniff the bytes and show them, so we do too.
#[derive(Default)]
pub struct SniffingImageLoader {
    cache: std::sync::Mutex<std::collections::HashMap<String, Result<std::sync::Arc<egui::ColorImage>, String>>>,
}

impl egui::load::ImageLoader for SniffingImageLoader {
    fn id(&self) -> &str {
        concat!(module_path!(), "::SniffingImageLoader")
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _size_hint: egui::SizeHint) -> egui::load::ImageLoadResult {
        use egui::load::{BytesPoll, ImagePoll, LoadError};
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        if let Some(done) = self.cache.lock().unwrap().get(uri) {
            return done.clone().map(|image| ImagePoll::Ready { image }).map_err(LoadError::Loading);
        }
        match ctx.try_load_bytes(uri)? {
            BytesPoll::Pending { size } => Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => {
                let decoded = image::load_from_memory(&bytes)
                    .map(|img| {
                        let rgba = img.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        std::sync::Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
                    })
                    .map_err(|e| format!("not an image: {e}"));
                self.cache.lock().unwrap().insert(uri.to_string(), decoded.clone());
                decoded.map(|image| ImagePoll::Ready { image }).map_err(LoadError::Loading)
            }
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache.lock().unwrap().values().filter_map(|r| r.as_ref().ok()).map(|i| i.as_raw().len()).sum()
    }
}
