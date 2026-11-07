//! Mailspring-compatible themes.
//!
//! A Mailspring theme is a package directory with a `package.json` that has
//! `"theme": "ui"` and a `styles/` folder of `.less` files. Almost all of a
//! theme's look comes from overriding the variables in `ui-variables.less`
//! (`@accent-primary`, `@list-selected-bg`, …). This module reads those
//! packages, evaluates the LESS variable expressions (including colour
//! functions like `darken()`, `fade()` and `mix()`) with LESS's lazy,
//! last-definition-wins semantics, and resolves them into a [`Palette`] that
//! the native UI uses.
//!
//! Rules that target Mailspring's HTML (`.thread-list .item {…}`) have no
//! equivalent in a native UI and are ignored.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32, // 0..=255
    pub g: f32,
    pub b: f32,
    pub a: f32, // 0..=1
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    pub fn to_u8(self) -> [u8; 4] {
        let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        [c(self.r), c(self.g), c(self.b), (self.a.clamp(0.0, 1.0) * 255.0).round() as u8]
    }
    pub fn hex(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        let p = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok().map(|v| v as f32);
        match h.len() {
            3 | 4 => {
                let d = |i| p(i, 1).map(|v| v * 17.0);
                let a = if h.len() == 4 { d(3)? / 255.0 } else { 1.0 };
                Some(Self::new(d(0)?, d(1)?, d(2)?, a))
            }
            6 | 8 => {
                let a = if h.len() == 8 { p(6, 2)? / 255.0 } else { 1.0 };
                Some(Self::new(p(0, 2)?, p(2, 2)?, p(4, 2)?, a))
            }
            _ => None,
        }
    }
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_u8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
    /// Relative luminance, 0..=1.
    pub fn luma(self) -> f32 {
        let f = |c: f32| {
            let c = c / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(self.r) + 0.7152 * f(self.g) + 0.0722 * f(self.b)
    }
    /// Flatten a translucent colour onto an opaque background.
    pub fn over(self, bg: Rgba) -> Rgba {
        let a = self.a.clamp(0.0, 1.0);
        Rgba::new(
            self.r * a + bg.r * (1.0 - a),
            self.g * a + bg.g * (1.0 - a),
            self.b * a + bg.b * (1.0 - a),
            1.0,
        )
    }
    /// WCAG contrast ratio of `self` drawn on `bg`.
    pub fn contrast(self, bg: Rgba) -> f32 {
        let (a, b) = (self.over(bg).luma(), bg.luma());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r / 255.0, self.g / 255.0, self.b / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f32::EPSILON {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s, l)
    }
    fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Self {
        let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
        let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
        if s == 0.0 {
            return Self::new(l * 255.0, l * 255.0, l * 255.0, a);
        }
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let hue = |mut t: f32| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        Self::new(hue(h + 1.0 / 3.0) * 255.0, hue(h) * 255.0, hue(h - 1.0 / 3.0) * 255.0, a)
    }
    fn map_hsl(self, f: impl FnOnce(f32, f32, f32) -> (f32, f32, f32)) -> Self {
        let (h, s, l) = self.to_hsl();
        let (h, s, l) = f(h, s, l);
        Self::from_hsl(h, s, l, self.a)
    }
    /// LESS `mix(a, b, weight)`.
    pub fn mix(a: Self, b: Self, weight: f32) -> Self {
        let p = weight.clamp(0.0, 1.0);
        let w = p * 2.0 - 1.0;
        let alpha = a.a - b.a;
        let w1 = (if w * alpha == -1.0 { w } else { (w + alpha) / (1.0 + w * alpha) } + 1.0) / 2.0;
        let w2 = 1.0 - w1;
        Self::new(
            a.r * w1 + b.r * w2,
            a.g * w1 + b.g * w2,
            a.b * w1 + b.b * w2,
            a.a * p + b.a * (1.0 - p),
        )
    }
}

fn named_color(name: &str) -> Option<Rgba> {
    let hex = match name.to_ascii_lowercase().as_str() {
        "white" => "#ffffff",
        "black" => "#000000",
        "red" => "#ff0000",
        "green" => "#008000",
        "blue" => "#0000ff",
        "yellow" => "#ffff00",
        "orange" => "#ffa500",
        "purple" => "#800080",
        "gray" | "grey" => "#808080",
        "silver" => "#c0c0c0",
        "navy" => "#000080",
        "teal" => "#008080",
        "maroon" => "#800000",
        "olive" => "#808000",
        "lime" => "#00ff00",
        "aqua" | "cyan" => "#00ffff",
        "fuchsia" | "magenta" => "#ff00ff",
        "pink" => "#ffc0cb",
        "gold" => "#ffd700",
        "whitesmoke" => "#f5f5f5",
        "gainsboro" => "#dcdcdc",
        "lightgray" | "lightgrey" => "#d3d3d3",
        "darkgray" | "darkgrey" => "#a9a9a9",
        "dimgray" | "dimgrey" => "#696969",
        "transparent" => "#00000000",
        _ => return None,
    };
    Rgba::hex(hex)
}

// ---------------------------------------------------------------------------
// LESS variable evaluation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Color(Rgba),
    /// A number and its unit (`%`, `px`, `deg`, or empty).
    Number(f32, String),
    Text(String),
}

impl Value {
    fn color(&self) -> Option<Rgba> {
        match self {
            Value::Color(c) => Some(*c),
            _ => None,
        }
    }
    /// Numbers as fractions: `20%` → 0.2, `0.2` → 0.2.
    fn fraction(&self) -> Option<f32> {
        match self {
            Value::Number(n, u) if u == "%" => Some(n / 100.0),
            Value::Number(n, _) => Some(if *n > 1.0 { n / 100.0 } else { *n }),
            _ => None,
        }
    }
    fn number(&self) -> Option<f32> {
        match self {
            Value::Number(n, _) => Some(*n),
            _ => None,
        }
    }
}

/// Raw `@name: expression;` declarations, evaluated on demand.
#[derive(Debug, Clone, Default)]
pub struct Variables {
    raw: HashMap<String, String>,
}

impl Variables {
    /// Read every top-level `@var: value;` in a LESS source. Later
    /// declarations win, as in LESS.
    pub fn parse_less(&mut self, src: &str) {
        let src = strip_comments(src);
        let mut depth = 0i32;
        let mut stmt = String::new();
        for ch in src.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    stmt.clear();
                }
                '}' => {
                    depth -= 1;
                    stmt.clear();
                }
                ';' if depth == 0 => {
                    self.declare(&stmt);
                    stmt.clear();
                }
                _ if depth == 0 => stmt.push(ch),
                _ => {}
            }
        }
    }

    fn declare(&mut self, stmt: &str) {
        let s = stmt.trim();
        let Some(rest) = s.strip_prefix('@') else { return };
        if rest.starts_with("import") || rest.starts_with("media") || rest.starts_with("font-face") {
            return;
        }
        if let Some((name, value)) = rest.split_once(':') {
            let name = name.trim();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                let v = value.trim().trim_end_matches("!important").trim();
                self.raw.insert(name.to_string(), v.to_string());
            }
        }
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.raw.insert(name.trim_start_matches('@').to_string(), value.to_string());
    }

    pub fn extend(&mut self, other: &Variables) {
        for (k, v) in &other.raw {
            self.raw.insert(k.clone(), v.clone());
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.raw.keys()
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn eval(&self, name: &str) -> Option<Value> {
        self.eval_depth(name.trim_start_matches('@'), 0)
    }

    pub fn color(&self, name: &str) -> Option<Rgba> {
        self.eval(name)?.color()
    }

    /// CSS `color-mix(in srgb, A [p%], B [q%])`.
    fn color_mix(&self, inner: &str, depth: usize) -> Option<Value> {
        let args = split_args(inner);
        let colors: Vec<&str> = args.iter().copied().filter(|a| !a.trim_start().starts_with("in ")).collect();
        if colors.len() != 2 {
            return None;
        }
        let part = |s: &str| -> Option<(Rgba, Option<f32>)> {
            let s = s.trim();
            if let Some((c, pct)) = s.rsplit_once(' ') {
                if let Some(n) = pct.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
                    return Some((self.eval_expr(c, depth)?.color()?, Some(n / 100.0)));
                }
            }
            Some((self.eval_expr(s, depth)?.color()?, None))
        };
        let (a, pa) = part(colors[0])?;
        let (b, pb) = part(colors[1])?;
        let wa = match (pa, pb) {
            (Some(x), Some(y)) if x + y > 0.0 => x / (x + y),
            (Some(x), None) => x,
            (None, Some(y)) => 1.0 - y,
            _ => 0.5,
        };
        let w = wa.clamp(0.0, 1.0);
        Some(Value::Color(Rgba::new(
            a.r * w + b.r * (1.0 - w),
            a.g * w + b.g * (1.0 - w),
            a.b * w + b.b * (1.0 - w),
            a.a * w + b.a * (1.0 - w),
        )))
    }

    fn eval_depth(&self, name: &str, depth: usize) -> Option<Value> {
        if depth > 32 {
            return None; // cycle
        }
        let expr = self.raw.get(name)?;
        self.eval_expr(expr, depth + 1)
    }

    fn eval_expr(&self, expr: &str, depth: usize) -> Option<Value> {
        let e = expr.trim();
        if e.is_empty() {
            return None;
        }
        // Parenthesised.
        if e.starts_with('(') && matching_paren(e, 0) == Some(e.len() - 1) {
            return self.eval_expr(&e[1..e.len() - 1], depth);
        }
        // Simple arithmetic at top level: `@x * 2`, `@a + @b` (numbers only).
        if let Some((l, op, r)) = split_binary(e) {
            let (a, b) = (self.eval_expr(l, depth)?, self.eval_expr(r, depth)?);
            if let (Value::Number(x, u), Some(y)) = (&a, b.number()) {
                let n = match op {
                    '+' => x + y,
                    '-' => x - y,
                    '*' => x * y,
                    '/' => x / y,
                    _ => return None,
                };
                return Some(Value::Number(n, u.clone()));
            }
            return None;
        }
        if let Some(var) = e.strip_prefix('@') {
            return self.eval_depth(var, depth);
        }
        if e.starts_with('#') {
            return Rgba::hex(e).map(Value::Color);
        }
        if e.starts_with("~\"") || e.starts_with('"') || e.starts_with('\'') {
            return Some(Value::Text(e.trim_start_matches('~').trim_matches(['"', '\'']).to_string()));
        }
        if let Some(open) = e.find('(') {
            if e.ends_with(')') && matching_paren(e, open) == Some(e.len() - 1) {
                let fname = e[..open].trim().to_ascii_lowercase();
                let inner = &e[open + 1..e.len() - 1];
                // CSS custom property with a fallback: OwlPost has no runtime
                // properties (e.g. Mailspring's --system-accent), so use the fallback.
                if fname == "var" {
                    let args = split_args(inner);
                    return args.get(1).and_then(|f| self.eval_expr(f, depth));
                }
                if fname == "color-mix" {
                    return self.color_mix(inner, depth);
                }
                let args: Vec<Value> = split_args(&e[open + 1..e.len() - 1])
                    .iter()
                    .map(|a| self.eval_expr(a, depth))
                    .collect::<Option<_>>()?;
                return call(&fname, &args);
            }
        }
        if let Some(c) = named_color(e) {
            return Some(Value::Color(c));
        }
        let num_end = e
            .char_indices()
            .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
            .map(|(i, _)| i)
            .unwrap_or(e.len());
        if num_end > 0 {
            if let Ok(n) = e[..num_end].parse::<f32>() {
                return Some(Value::Number(n, e[num_end..].trim().to_string()));
            }
        }
        Some(Value::Text(e.to_string()))
    }
}

fn call(f: &str, a: &[Value]) -> Option<Value> {
    let c0 = || a.first().and_then(Value::color);
    let amt = |i: usize| a.get(i).and_then(Value::fraction);
    let col = match f {
        "lighten" => c0()?.map_hsl(|h, s, l| (h, s, l + amt(1).unwrap_or(0.0))),
        "darken" => c0()?.map_hsl(|h, s, l| (h, s, l - amt(1).unwrap_or(0.0))),
        "saturate" => c0()?.map_hsl(|h, s, l| (h, s + amt(1).unwrap_or(0.0), l)),
        "desaturate" => c0()?.map_hsl(|h, s, l| (h, s - amt(1).unwrap_or(0.0), l)),
        "greyscale" | "grayscale" => c0()?.map_hsl(|h, _, l| (h, 0.0, l)),
        "spin" => {
            let d = a.get(1)?.number()?;
            c0()?.map_hsl(|h, s, l| (h + d, s, l))
        }
        "fade" => Rgba { a: amt(1)?, ..c0()? },
        "fadein" => {
            let c = c0()?;
            Rgba { a: (c.a + amt(1)?).clamp(0.0, 1.0), ..c }
        }
        "fadeout" => {
            let c = c0()?;
            Rgba { a: (c.a - amt(1)?).clamp(0.0, 1.0), ..c }
        }
        "mix" => Rgba::mix(c0()?, a.get(1)?.color()?, amt(2).unwrap_or(0.5)),
        "tint" => Rgba::mix(Rgba::new(255.0, 255.0, 255.0, 1.0), c0()?, amt(1).unwrap_or(0.5)),
        "shade" => Rgba::mix(Rgba::new(0.0, 0.0, 0.0, 1.0), c0()?, amt(1).unwrap_or(0.5)),
        "contrast" => {
            let c = c0()?;
            let dark = a.get(1).and_then(Value::color).unwrap_or(Rgba::new(0.0, 0.0, 0.0, 1.0));
            let light = a.get(2).and_then(Value::color).unwrap_or(Rgba::new(255.0, 255.0, 255.0, 1.0));
            if c.luma() > 0.43 { dark } else { light }
        }
        "rgb" => Rgba::new(a.first()?.number()?, a.get(1)?.number()?, a.get(2)?.number()?, 1.0),
        "rgba" => {
            if let Some(c) = c0() {
                Rgba { a: amt(1)?, ..c }
            } else {
                let alpha = a.get(3)?;
                let alpha = match alpha {
                    Value::Number(n, u) if u == "%" => n / 100.0,
                    Value::Number(n, _) => *n,
                    _ => return None,
                };
                Rgba::new(a[0].number()?, a[1].number()?, a[2].number()?, alpha)
            }
        }
        "hsl" | "hsla" => Rgba::from_hsl(
            a.first()?.number()?,
            amt(1)?,
            amt(2)?,
            a.get(3).and_then(Value::number).unwrap_or(1.0),
        ),
        _ => return None,
    };
    Some(Value::Color(col))
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    let mut in_str: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = in_str {
            out.push(c as char);
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            in_str = Some(c);
            out.push(c as char);
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') && (i == 0 || b[i - 1] != b':') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            // Keep multi-byte UTF-8 intact.
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i < open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

/// Split `a op b` at the last top-level arithmetic operator surrounded by spaces.
fn split_binary(s: &str) -> Option<(&str, char, &str)> {
    let mut depth = 0;
    let bytes = s.as_bytes();
    let mut found = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '+' | '-' | '*' | '/' if depth == 0 && i > 0 && i + 1 < s.len()
                && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' => {
                    found = Some((i, c));
                }
            _ => {}
        }
    }
    found.map(|(i, c)| (&s[..i], c, &s[i + 1..]))
}

// ---------------------------------------------------------------------------
// Palette: the semantic colours the UI paints with
// ---------------------------------------------------------------------------

macro_rules! palette {
    ($( $field:ident => [$($var:literal),+] ),* $(,)?) => {
        /// Resolved theme colours, keyed by UI role.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Palette { $( pub $field: Rgba, )* pub dark: bool }

        impl Palette {
            /// Each role tries its Mailspring variables in order.
            pub fn from_vars(v: &Variables) -> Self {
                let base = base_variables();
                let get = |names: &[&str]| -> Rgba {
                    names.iter().find_map(|n| v.color(n))
                        .or_else(|| names.iter().find_map(|n| base.color(n)))
                        .unwrap_or(Rgba::new(255.0, 0.0, 255.0, 1.0))
                };
                let mut p = Palette { $( $field: get(&[$($var),+]), )* dark: false };
                p.dark = p.background.luma() < 0.25;
                p.ensure_contrast();
                p
            }

            /// `(role, rgba hex)` pairs, for debugging and the MCP server.
            pub fn entries(&self) -> Vec<(&'static str, String)> {
                vec![ $( (stringify!($field), self.$field.to_hex()), )* ]
            }
        }
    };
}

palette! {
    accent => ["accent-primary"],
    accent_dark => ["accent-primary-dark", "accent-primary"],
    background => ["background-primary"],
    background_off => ["background-off-primary", "background-primary"],
    background_secondary => ["background-secondary"],
    background_tertiary => ["background-tertiary"],
    text => ["text-color"],
    text_subtle => ["text-color-subtle", "text-color"],
    text_very_subtle => ["text-color-very-subtle", "text-color-subtle"],
    text_inverse => ["text-color-inverse"],
    heading => ["text-color-heading", "text-color"],
    link => ["text-color-link", "accent-primary"],
    link_hover => ["text-color-link-hover", "accent-primary-dark"],
    border => ["border-color-primary"],
    divider => ["border-color-divider", "border-color-secondary"],
    panel => ["panel-background-color", "background-off-primary"],
    toolbar => ["toolbar-background-color", "background-secondary"],
    toolbar_text => ["toolbar-text-color", "text-color"],
    sidebar => ["source-list-bg", "background-secondary"],
    sidebar_text => ["source-list-color", "sidebar-text", "text-color-subtle"],
    sidebar_active => ["source-list-active-bg"],
    sidebar_active_text => ["source-list-active-color", "active-sidebar-text", "text-color"],
    list => ["list-bg", "background-primary"],
    list_border => ["list-border", "border-color-primary"],
    list_hover => ["list-hover-bg"],
    list_selected => ["list-selected-bg"],
    list_selected_text => ["list-selected-color", "text-color"],
    list_focused => ["list-focused-bg", "accent-primary"],
    list_focused_text => ["list-focused-color", "text-color-inverse"],
    unread => ["unread-color", "accent-primary"],
    input => ["input-bg", "background-primary"],
    input_border => ["input-border-color", "border-color-primary"],
    button => ["btn-default-bg-color", "background-secondary"],
    button_text => ["btn-default-text-color", "text-color"],
    button_emphasis => ["btn-emphasis-bg-color", "accent-primary"],
    button_emphasis_text => ["btn-emphasis-text-color", "text-color-inverse"],
    info => ["color-info", "accent-primary"],
    success => ["color-success"],
    warning => ["color-warning"],
    error => ["color-error"],
    star => ["star-color", "color-warning"],
    snooze => ["snooze-color", "accent-primary"],
}

impl Palette {
    /// Mailspring themes often colour parts of the UI with CSS rules that a
    /// native app can't apply (e.g. Darkside's light-on-dark sidebar text).
    /// Where a resolved foreground would be unreadable on its background,
    /// swap in whichever of the theme's text colours reads best.
    fn ensure_contrast(&mut self) {
        let candidates = [self.text, self.text_inverse, self.heading, Rgba::new(255.0, 255.0, 255.0, 1.0), Rgba::new(0.0, 0.0, 0.0, 1.0)];
        let fix = |fg: Rgba, bg: Rgba, min: f32| -> Rgba {
            if fg.contrast(bg) >= min {
                return fg;
            }
            // Prefer the theme's own colours, in order; only fall back to the
            // highest-contrast candidate when none of them reads well.
            let best = candidates
                .iter()
                .copied()
                .find(|c| c.contrast(bg) >= min)
                .or_else(|| candidates.iter().copied().max_by(|a, b| a.contrast(bg).total_cmp(&b.contrast(bg))))
                .unwrap_or(fg);
            // Keep the original's translucency so "subtle" stays subtle.
            Rgba { a: fg.a.max(0.75), ..best }
        };
        // Some themes mark the focused row purely with CSS; make sure it shows.
        if self.list_focused.over(self.list).contrast(self.list) < 1.15 {
            self.list_focused = Rgba::mix(self.accent, self.list, 0.22);
        }
        if self.sidebar_active.over(self.sidebar).contrast(self.sidebar) < 1.05 {
            self.sidebar_active = Rgba::mix(self.accent, self.sidebar, 0.15);
        }
        let sidebar_active_bg = self.sidebar_active.over(self.sidebar);
        self.sidebar_text = fix(self.sidebar_text, self.sidebar, 3.0);
        self.sidebar_active_text = fix(self.sidebar_active_text, sidebar_active_bg, 3.0);
        self.toolbar_text = fix(self.toolbar_text, self.toolbar, 3.0);
        self.list_focused_text = fix(self.list_focused_text, self.list_focused.over(self.list), 3.0);
        let selected = self.list_selected.over(self.list);
        self.list_selected_text = fix(self.list_selected_text, selected, 3.0);
        self.button_text = fix(self.button_text, self.button.over(self.background), 3.0);
        self.button_emphasis_text = fix(self.button_emphasis_text, self.button_emphasis, 2.5);
        self.text = fix(self.text, self.list, 4.5);
        // Headings sit on the thread list and the message area.
        let heading_bg = if self.list.contrast(self.background_secondary) > 1.5 { self.background_secondary } else { self.list };
        self.heading = fix(self.heading, self.list, 3.0);
        if self.heading.contrast(heading_bg) < 3.0 {
            self.heading = self.text;
        }
    }
}

// ---------------------------------------------------------------------------
// Theme packages
// ---------------------------------------------------------------------------

const LIGHT_LESS: &str = include_str!("../themes/owlpost-light/styles/ui-variables.less");
const DARK_LESS: &str = include_str!("../themes/owlpost-dark/styles/ui-variables.less");
const DARKSIDE_LESS: &str = include_str!("../themes/owlpost-darkside/styles/ui-variables.less");
const MOONLIT_DARK_LESS: &str = include_str!("../themes/owlpost-moonlit-dark/styles/ui-variables.less");
const MOONLIT_LIGHT_LESS: &str = include_str!("../themes/owlpost-moonlit-light/styles/ui-variables.less");

/// The theme that follows the system appearance, switching between
/// [`MOONLIT_DARK`] and [`MOONLIT_LIGHT`]. On its own it resolves as light.
pub const MOONLIT: &str = "owlpost-moonlit";
pub const MOONLIT_DARK: &str = "owlpost-moonlit-dark";
pub const MOONLIT_LIGHT: &str = "owlpost-moonlit-light";

pub fn base_variables() -> Variables {
    let mut v = Variables::default();
    v.parse_less(LIGHT_LESS);
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSource {
    BuiltIn,
    /// Installed into OwlPost's own themes directory.
    User,
    /// Found in an existing Mailspring install's packages directory.
    Mailspring,
}

#[derive(Debug, Clone)]
pub struct Theme {
    /// Package name, used as the stable id.
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub source: ThemeSource,
    pub dir: Option<PathBuf>,
    /// The theme's own declarations (overrides only).
    pub overrides: Variables,
}

impl Theme {
    /// Base variables with this theme's overrides on top.
    pub fn variables(&self) -> Variables {
        let mut v = base_variables();
        v.extend(&self.overrides);
        v
    }
    pub fn palette(&self) -> Palette {
        Palette::from_vars(&self.variables())
    }

    fn builtin(id: &str, name: &str, desc: &str, less: &str) -> Self {
        let mut overrides = Variables::default();
        overrides.parse_less(less);
        Self {
            id: id.into(),
            display_name: name.into(),
            description: desc.into(),
            source: ThemeSource::BuiltIn,
            dir: None,
            overrides,
        }
    }

    /// Load a Mailspring-style theme package directory.
    pub fn load_package(dir: &Path, source: ThemeSource) -> Result<Self> {
        let pkg_path = dir.join("package.json");
        let pkg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&pkg_path).with_context(|| format!("reading {}", pkg_path.display()))?,
        )?;
        if pkg.get("theme").is_none() {
            return Err(anyhow!("{} is not a theme package (no \"theme\" key)", dir.display()));
        }
        let id = pkg["name"].as_str().map(String::from).unwrap_or_else(|| {
            dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        });
        let display_name = pkg["displayName"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| prettify(&id));
        let description = pkg["description"].as_str().unwrap_or_default().to_string();

        let mut files = Vec::new();
        collect_less(&dir.join("styles"), &mut files);
        if files.is_empty() {
            collect_less(dir, &mut files);
        }
        // Variable files last so their declarations win, as they would after
        // Mailspring's import order.
        // `theme-colors.less` is what Mailspring's theme picker reads to
        // summarise a theme, so it goes last and wins.
        files.sort_by_key(|p| {
            let n = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let rank = if n == "theme-colors.less" { 2 } else if n.contains("variables") { 1 } else { 0 };
            (rank, n)
        });
        let mut overrides = Variables::default();
        for f in files {
            if let Ok(src) = std::fs::read_to_string(&f) {
                overrides.parse_less(&src);
            }
        }
        if overrides.is_empty() {
            return Err(anyhow!("{} declares no LESS variables", dir.display()));
        }
        Ok(Self { id, display_name, description, source, dir: Some(dir.to_path_buf()), overrides })
    }
}

fn prettify(id: &str) -> String {
    id.trim_start_matches("ui-")
        .trim_start_matches("mailspring-theme-")
        .trim_start_matches("mailspring-")
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_less(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n != "node_modules").unwrap_or(true) {
                collect_less(&p, out);
            }
        } else if p.extension().map(|x| x == "less").unwrap_or(false) {
            out.push(p);
        }
    }
}

/// OwlPost's own themes directory.
pub fn user_themes_dir() -> PathBuf {
    let dir = crate::data_dir().join("themes");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Where Mailspring keeps installed packages (themes included).
pub fn mailspring_packages_dir() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(base.config_dir().join("Mailspring").join("packages"))
}

/// Every theme available: built-ins, OwlPost themes, Mailspring themes.
pub fn discover() -> Vec<Theme> {
    let mut themes = vec![
        Theme::builtin(MOONLIT, "Moonlit", "Light or dark, following your system", MOONLIT_LIGHT_LESS),
        Theme::builtin(MOONLIT_LIGHT, "Moonlit Light", "Cool off-white with a navy accent", MOONLIT_LIGHT_LESS),
        Theme::builtin(MOONLIT_DARK, "Moonlit Dark", "Deep navy with a soft periwinkle accent", MOONLIT_DARK_LESS),
        Theme::builtin("owlpost-light", "Light", "The default OwlPost theme", ""),
        Theme::builtin("owlpost-dark", "Dark", "Easy on the eyes at night", DARK_LESS),
        Theme::builtin("owlpost-darkside", "Darkside", "Light messages, dark sidebar", DARKSIDE_LESS),
    ];
    let mut scan = |dir: PathBuf, source: ThemeSource| {
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            match Theme::load_package(&d, source.clone()) {
                Ok(t) if !themes.iter().any(|x| x.id == t.id) => themes.push(t),
                Ok(_) => {}
                Err(e) => log::debug!("skipping {}: {e:#}", d.display()),
            }
        }
    };
    scan(user_themes_dir(), ThemeSource::User);
    if let Some(ms) = mailspring_packages_dir() {
        scan(ms, ThemeSource::Mailspring);
    }
    themes
}

/// Copy a theme package directory into OwlPost's themes folder.
pub fn install(from: &Path) -> Result<Theme> {
    let probe = Theme::load_package(from, ThemeSource::User)?;
    let dest = user_themes_dir().join(&probe.id);
    copy_dir(from, &dest)?;
    Theme::load_package(&dest, ThemeSource::User)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name == "node_modules" || name == ".git" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(&name))?;
        } else {
            std::fs::copy(&p, to.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, hex: &str) -> bool {
        let b = Rgba::hex(hex).unwrap();
        (a.r - b.r).abs() < 1.5 && (a.g - b.g).abs() < 1.5 && (a.b - b.b).abs() < 1.5 && (a.a - b.a).abs() < 0.01
    }

    #[test]
    fn less_functions_match_less_js() {
        let mut v = Variables::default();
        v.parse_less(
            "@a: #419bf9; @b: darken(@a, 10%); @c: lighten(#000, 50%); @d: fade(@a, 50%);
             @e: mix(#ff0000, #0000ff, 50%); @f: rgba(0,0,0,0.5); @g: fadeout(#fff, 20%);
             @h: @b; // comment
             /* block */ @i: spin(#ff0000, 120); @j: contrast(#ffffff);",
        );
        assert!(close(v.color("a").unwrap(), "#419bf9"));
        assert!(close(v.color("b").unwrap(), "#1182f7"), "{:?}", v.color("b").unwrap().to_hex());
        assert!(close(v.color("c").unwrap(), "#808080"));
        assert!((v.color("d").unwrap().a - 0.5).abs() < 0.01);
        assert!(close(v.color("e").unwrap(), "#800080"));
        assert!((v.color("f").unwrap().a - 0.5).abs() < 0.01);
        assert!((v.color("g").unwrap().a - 0.8).abs() < 0.01);
        assert_eq!(v.color("h"), v.color("b"));
        assert!(close(v.color("i").unwrap(), "#00ff00"));
        assert!(close(v.color("j").unwrap(), "#000000"));
    }

    #[test]
    fn css_functions_used_by_mailspring() {
        let mut v = Variables::default();
        v.parse_less(
            "@blue: #419bf9; @list-bg: #ffffff;
             @accent-primary: var(--system-accent, @blue);
             @accent-primary-dark: var(--system-accent-dark, color-mix(in srgb, @accent-primary, black 10%));
             @list-selected-bg: color-mix(in srgb, @accent-primary 17%, @list-bg);
             @list-selected-color: inherit;",
        );
        assert!(close(v.color("accent-primary").unwrap(), "#419bf9"));
        // 90% blue + 10% black
        assert!(close(v.color("accent-primary-dark").unwrap(), "#3b8be0"), "{}", v.color("accent-primary-dark").unwrap().to_hex());
        // 17% blue over white
        assert!(close(v.color("list-selected-bg").unwrap(), "#e0effe"), "{}", v.color("list-selected-bg").unwrap().to_hex());
        assert!(v.color("list-selected-color").is_none());
    }

    #[test]
    fn unreadable_text_is_corrected() {
        // A theme that only darkens the sidebar (text set via CSS in Mailspring).
        let mut v = base_variables();
        v.parse_less("@panel-background-color: #313042;");
        let p = Palette::from_vars(&v);
        assert!(p.sidebar_text.contrast(p.sidebar) >= 3.0);
    }

    #[test]
    fn lazy_last_definition_wins() {
        // A theme overriding a base variable changes everything derived from it.
        let mut v = base_variables();
        let before = v.color("list-focused-bg").unwrap();
        v.parse_less("@accent-primary: #ff0000;");
        let after = v.color("list-focused-bg").unwrap();
        assert_ne!(before, after);
        // Mailspring focuses rows with @component-active-color = accent darkened 10%.
        assert!(close(after, "#e60000"), "{}", after.to_hex());
    }

    #[test]
    fn ignores_rule_blocks() {
        let mut v = Variables::default();
        v.parse_less(".thread-list { @local: #000; color: red; } @top: #123456;");
        assert!(v.color("local").is_none());
        assert!(v.color("top").is_some());
    }

    #[test]
    fn builtins_resolve() {
        for t in discover().into_iter().filter(|t| t.source == ThemeSource::BuiltIn) {
            let p = t.palette();
            for (role, hex) in p.entries() {
                assert_ne!(hex, "#ff00ff", "{} has unresolved {role}", t.id);
            }
        }
        let dark = discover().into_iter().find(|t| t.id == "owlpost-dark").unwrap();
        assert!(dark.palette().dark);
        let moon = |id: &str| discover().into_iter().find(|t| t.id == id).unwrap().palette();
        assert!(moon(MOONLIT_DARK).dark && !moon(MOONLIT_LIGHT).dark && !moon(MOONLIT).dark);
        // The mockup's colours survive the contrast fixes.
        assert!(close(moon(MOONLIT_DARK).list_focused, "#1a2645"));
        assert!(close(moon(MOONLIT_LIGHT).list_focused, "#dde4f9"));
        assert!(close(moon(MOONLIT_DARK).sidebar_active_text, "#b8c7ff"));
    }

    #[test]
    fn loads_mailspring_style_package() {
        let dir = std::env::temp_dir().join(format!("sp-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("styles")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"ui-solarized","theme":"ui","engines":{"mailspring":"*"}}"#).unwrap();
        std::fs::write(
            dir.join("styles/ui-variables.less"),
            "@import 'ui-mixins';\n@base03: #002b36;\n@accent-primary: #268bd2;\n@background-primary: @base03;\n@text-color: #93a1a1;\n",
        )
        .unwrap();
        std::fs::write(dir.join("styles/index.less"), "@import 'ui-variables';\n.sheet { background: @background-primary; }\n").unwrap();
        let t = Theme::load_package(&dir, ThemeSource::Mailspring).unwrap();
        assert_eq!(t.display_name, "Solarized");
        let p = t.palette();
        assert!(close(p.background, "#002b36"));
        assert!(close(p.accent, "#268bd2"));
        assert!(close(p.list_focused, "#227dbd"), "{}", p.list_focused.to_hex());
        assert!(p.dark);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
