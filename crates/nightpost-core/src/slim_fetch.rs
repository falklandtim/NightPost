//! Downloading a big email without its attachments.
//!
//! A sync used to fetch every new email whole and keep only its text and
//! its attachments' names; opening an attachment downloads the email again
//! anyway (`engine::attachment_file`). For an email with a few megabytes of
//! attachments that was nearly all wasted, and most of a first sync.
//!
//! Now a sync first asks for each new email's size and part structure
//! (`BODYSTRUCTURE`). A big multipart email comes down as its header, each
//! part's own header, and the bodies of its text and HTML parts only. Those
//! are put back together as a smaller email with the same parts in the same
//! order, the attachments empty, and parsed as usual; the attachments'
//! sizes come from the structure. Everything else is fetched whole, as
//! before.
//!
//! The replies are read by a small parser of our own ([`Value`]): the IMAP
//! library has already checked them, and its own types for the structure
//! belong to `imap-proto` 0.10, which we're moving off (docs/imap-upgrade.md).

use crate::models::Message;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Emails bigger than this (bytes, as the server counts them) are fetched
/// slim, if they have parts worth leaving behind.
pub const SLIM_ABOVE: u32 = 100 * 1024;

/// Text and HTML parts up to this size (bytes) are downloaded; a bigger
/// one is an attachment in all but name.
const TEXT_LIMIT: u32 = 1024 * 1024;

/// How deep a reply's brackets may go before it's treated as nonsense.
const MAX_DEPTH: usize = 64;

/// One item of an IMAP reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Atom(String),
    Str(Vec<u8>),
    Nil,
    List(Vec<Value>),
}

impl Value {
    /// A string or atom as text, lower-cased (types, subtypes, encodings).
    fn word(&self) -> Option<String> {
        match self {
            Value::Atom(a) => Some(a.to_ascii_lowercase()),
            Value::Str(s) => Some(String::from_utf8_lossy(s).to_ascii_lowercase()),
            _ => None,
        }
    }

    fn number(&self) -> Option<u32> {
        match self {
            Value::Atom(a) => a.parse().ok(),
            _ => None,
        }
    }
}

/// Reads values from a reply, never past its end.
struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn skip_space(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\r' | b'\n' | b'\t')) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_space();
        match *self.b.get(self.i)? {
            b'(' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if *self.b.get(self.i)? == b')' {
                        self.i += 1;
                        return Some(Value::List(items));
                    }
                    items.push(self.value(depth + 1)?);
                }
            }
            b')' => None,
            b'"' => {
                self.i += 1;
                let mut s = Vec::new();
                loop {
                    match *self.b.get(self.i)? {
                        b'"' => {
                            self.i += 1;
                            return Some(Value::Str(s));
                        }
                        b'\\' => {
                            s.push(*self.b.get(self.i + 1)?);
                            self.i += 2;
                        }
                        c => {
                            s.push(c);
                            self.i += 1;
                        }
                    }
                }
            }
            b'{' => {
                // A literal: {n} then a line break, then n bytes.
                let close = self.i + self.b.get(self.i..)?.iter().position(|&c| c == b'}')?;
                let n: usize = std::str::from_utf8(self.b.get(self.i + 1..close)?).ok()?.trim_end_matches('+').parse().ok()?;
                let mut start = close + 1;
                if self.b.get(start) == Some(&b'\r') {
                    start += 1;
                }
                if self.b.get(start) != Some(&b'\n') {
                    return None;
                }
                start += 1;
                let data = self.b.get(start..start.checked_add(n)?)?.to_vec();
                self.i = start + n;
                Some(Value::Str(data))
            }
            _ => {
                // An atom, with any [section] kept whole: BODY[1.MIME].
                let start = self.i;
                while let Some(&c) = self.b.get(self.i) {
                    match c {
                        b'[' => {
                            self.i += self.b.get(self.i..)?.iter().position(|&c| c == b']')? + 1;
                        }
                        b' ' | b'(' | b')' | b'\r' | b'\n' | b'\t' | b'"' | b'{' => break,
                        _ => self.i += 1,
                    }
                }
                if self.i == start {
                    return None;
                }
                let a = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
                Some(if a.eq_ignore_ascii_case("NIL") { Value::Nil } else { Value::Atom(a) })
            }
        }
    }
}

/// Each `* n FETCH (…)` in a reply, as its items by name (upper-cased):
/// `UID`, `FLAGS`, `BODYSTRUCTURE`, `BODY[1.MIME]`… Other replies, and any
/// FETCH we can't read, are left out.
pub fn fetch_responses(raw: &[u8]) -> Vec<HashMap<String, Value>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = next_fetch(raw, i) {
        let mut r = Reader { b: raw, i: at };
        let Some(Value::List(items)) = r.value(0) else {
            i = at + 1;
            continue;
        };
        // Past the whole list, so a literal's contents are never mistaken
        // for the start of another reply.
        i = r.i;
        let mut map = HashMap::new();
        let mut it = items.into_iter();
        while let (Some(Value::Atom(k)), Some(v)) = (it.next(), it.next()) {
            map.insert(k.to_ascii_uppercase(), v);
        }
        out.push(map);
    }
    out
}

/// Where the `(` of the next `* n FETCH (` at the start of a line is.
fn next_fetch(raw: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    loop {
        let line = raw.get(i..)?;
        if line.starts_with(b"* ") {
            let rest = &line[2..];
            let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && rest.get(digits..digits + 8).is_some_and(|s| s.eq_ignore_ascii_case(b" FETCH (")) {
                return Some(i + 2 + digits + 7);
            }
        }
        i += line.iter().position(|&c| c == b'\n')? + 1;
    }
}

/// An email's part structure, as far as slim fetching needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Multi { subtype: String, parts: Vec<Part> },
    Leaf { ty: String, subtype: String, encoding: String, octets: u32 },
}

impl Part {
    /// Read a `BODYSTRUCTURE`. `None` if it isn't one we understand.
    pub fn parse(v: &Value) -> Option<Self> {
        Self::parse_at(v, 0)
    }

    fn parse_at(v: &Value, depth: usize) -> Option<Self> {
        let Value::List(items) = v else { return None };
        if depth > MAX_DEPTH {
            return None;
        }
        if matches!(items.first()?, Value::List(_)) {
            let n = items.iter().take_while(|x| matches!(x, Value::List(_))).count();
            let parts = items[..n].iter().map(|p| Self::parse_at(p, depth + 1)).collect::<Option<Vec<_>>>()?;
            let subtype = items.get(n).and_then(Value::word).unwrap_or_else(|| "mixed".into());
            return Some(Part::Multi { subtype, parts });
        }
        Some(Part::Leaf {
            ty: items.first()?.word()?,
            subtype: items.get(1)?.word()?,
            encoding: items.get(5).and_then(Value::word).unwrap_or_else(|| "7bit".into()),
            octets: items.get(6)?.number()?,
        })
    }

    /// A part worth downloading: text or HTML of a sensible size.
    fn is_text(&self) -> bool {
        matches!(self, Part::Leaf { ty, subtype, octets, .. } if ty == "text" && (subtype == "plain" || subtype == "html") && *octets <= TEXT_LIMIT)
    }

    /// Every part that isn't a multipart, with its section number (`1.2`),
    /// in the email's order.
    fn leaves(&self) -> Vec<(String, &Part)> {
        fn walk<'a>(p: &'a Part, path: &str, out: &mut Vec<(String, &'a Part)>) {
            match p {
                Part::Multi { parts, .. } => {
                    for (k, c) in parts.iter().enumerate() {
                        let sub = if path.is_empty() { format!("{}", k + 1) } else { format!("{path}.{}", k + 1) };
                        walk(c, &sub, out);
                    }
                }
                leaf => out.push((path.to_string(), leaf)),
            }
        }
        let mut out = Vec::new();
        if matches!(self, Part::Multi { .. }) {
            walk(self, "", &mut out);
        }
        out
    }
}

/// Whether an email of `size` bytes with this structure is fetched slim: a
/// big multipart email with something besides text in it.
pub fn worth_slimming(size: u32, part: &Part) -> bool {
    size > SLIM_ABOVE && part.leaves().iter().any(|(_, p)| !p.is_text())
}

/// The sections to ask for: the email's header, every part's own header,
/// and the bodies of its text parts.
pub fn sections(part: &Part) -> Vec<String> {
    let mut v = vec!["BODY.PEEK[HEADER]".to_string()];
    for (path, p) in part.leaves() {
        v.push(format!("BODY.PEEK[{path}.MIME]"));
        if p.is_text() {
            v.push(format!("BODY.PEEK[{path}]"));
        }
    }
    v
}

/// Put a slim email back together from its sections (keyed as the server
/// names them, without `.PEEK`: `BODY[HEADER]`, `BODY[1.MIME]`, `BODY[1]`).
/// The multiparts get boundaries of our own, so their headers aren't
/// needed. `None` if a section is missing.
pub fn assemble(part: &Part, got: &HashMap<String, Value>) -> Option<Vec<u8>> {
    let bytes = |k: &str| match got.get(k) {
        Some(Value::Str(s)) => Some(s.as_slice()),
        _ => None,
    };
    let Part::Multi { .. } = part else { return None };
    let mut out = without_content_type(bytes("BODY[HEADER]")?);
    let mut next = 0;
    write_part(&mut out, part, "", &mut next, &bytes)?;
    Some(out)
}

fn write_part<'a>(out: &mut Vec<u8>, part: &Part, path: &str, next: &mut usize, bytes: &impl Fn(&str) -> Option<&'a [u8]>) -> Option<()> {
    match part {
        Part::Multi { subtype, parts } => {
            *next += 1;
            // Not a string base64 or quoted-printable text can hold.
            let b = format!("=_nightpost_slim_{next}_=");
            out.extend_from_slice(format!("Content-Type: multipart/{subtype}; boundary=\"{b}\"\r\n\r\n").as_bytes());
            for (k, c) in parts.iter().enumerate() {
                let sub = if path.is_empty() { format!("{}", k + 1) } else { format!("{path}.{}", k + 1) };
                out.extend_from_slice(format!("--{b}\r\n").as_bytes());
                write_part(out, c, &sub, next, bytes)?;
            }
            out.extend_from_slice(format!("--{b}--\r\n").as_bytes());
        }
        leaf => {
            let mime = bytes(&format!("BODY[{path}.MIME]"))?;
            out.extend_from_slice(mime.strip_suffix(b"\r\n\r\n").or_else(|| mime.strip_suffix(b"\n\n")).unwrap_or(mime));
            out.extend_from_slice(b"\r\n\r\n");
            if leaf.is_text() {
                out.extend_from_slice(bytes(&format!("BODY[{path}]"))?);
            }
            out.extend_from_slice(b"\r\n");
        }
    }
    Some(())
}

/// A header without its Content-Type field (and that field's folded
/// lines), and without the blank line that ends it.
fn without_content_type(header: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + 80);
    let mut skipping = false;
    for line in header.split_inclusive(|&c| c == b'\n') {
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let folded = matches!(line.first(), Some(b' ' | b'\t'));
        if !folded {
            skipping = line.len() >= 13 && line[..13].eq_ignore_ascii_case(b"content-type:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out
}

/// Give the attachments left behind their real sizes (decoded, roughly),
/// in order: parsing saw them empty.
pub fn fill_in_sizes(m: &mut Message, part: &Part) {
    let mut left_behind = part.leaves().into_iter().filter(|(_, p)| !p.is_text()).map(|(_, p)| p);
    for a in m.attachments.iter_mut().filter(|a| a.size == 0) {
        let Some(Part::Leaf { encoding, octets, .. }) = left_behind.next() else { break };
        let n = *octets as usize;
        // Base64: 57 bytes to each 78-character line.
        a.size = if encoding == "base64" { n / 78 * 57 + (n % 78) * 3 / 4 } else { n };
    }
}

/// Of these new emails, the big ones worth fetching slim, with their
/// structure. Errors leave the connection in doubt: see `engine`.
pub fn plan<T: std::io::Read + std::io::Write>(session: &mut imap::Session<T>, uids: &[u32]) -> Result<HashMap<u32, Part>> {
    let mut slim = HashMap::new();
    for chunk in uids.chunks(200) {
        let raw = session.run_command_and_read_response(format!("UID FETCH {} (UID RFC822.SIZE BODYSTRUCTURE)", crate::engine::uid_set(chunk)))?;
        for f in fetch_responses(&raw) {
            let (Some(uid), Some(size), Some(part)) = (
                f.get("UID").and_then(Value::number),
                f.get("RFC822.SIZE").and_then(Value::number),
                f.get("BODYSTRUCTURE").and_then(Part::parse),
            ) else {
                continue;
            };
            if worth_slimming(size, &part) {
                slim.insert(uid, part);
            }
        }
    }
    Ok(slim)
}

/// Fetch one email slim: (the email put back together, unread, starred).
pub fn fetch<T: std::io::Read + std::io::Write>(session: &mut imap::Session<T>, uid: u32, part: &Part) -> Result<(Vec<u8>, bool, bool)> {
    let raw = session.run_command_and_read_response(format!("UID FETCH {uid} (UID FLAGS {})", sections(part).join(" ")))?;
    let got = fetch_responses(&raw)
        .into_iter()
        .find(|f| f.get("UID").and_then(Value::number) == Some(uid))
        .ok_or_else(|| anyhow!("the server didn't send email {uid}"))?;
    let flags: Vec<String> = match got.get("FLAGS") {
        Some(Value::List(f)) => f.iter().filter_map(Value::word).collect(),
        _ => Vec::new(),
    };
    let email = assemble(part, &got).ok_or_else(|| anyhow!("email {uid} came back incomplete"))?;
    Ok((email, !flags.iter().any(|f| f == "\\seen"), flags.iter().any(|f| f == "\\flagged")))
}

#[cfg(test)]
mod tests;
