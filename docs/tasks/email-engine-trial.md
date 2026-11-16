# Task: the email engine trial

A brief for Claude Code (or anyone) to carry out later. Written 2026-11-16. Tim asked for this test; the plan and the reasons behind it are in [docs/gtk4-upgrade.md](../gtk4-upgrade.md), section "How we'll test the engines". **Read that section, [AGENTS.md](../../AGENTS.md) and [docs/architecture.md](../architecture.md) before starting.** Where this brief and AGENTS.md disagree, AGENTS.md wins.

## The goal

Build a small, separate test program that draws a set of made-up emails with five HTML engines, measures them, checks they keep NightPost's privacy promises, and produces a report Tim can score. **Nothing in the app changes.** The result is evidence for one decision: how NightPost draws HTML email on Linux (and possibly the Mac) once the GTK 3 bindings go.

The engines: litehtml, Blitz, Servo (Mac and Linux); GTK 4 WebKit (`webkit6`) and, only if needed, WPE WebKit (Linux). Plus a reference back end on the Mac using today's WKWebView.

## Before you start

1. `git status`. If the NightPost rename (`crates/owlpost*` → `crates/nightpost*`) or other work is still uncommitted, stop and ask Tim how he wants it committed first. Don't mix this task into it.
2. Work on a new branch, `trial/email-engines`. Commit only when Tim asks (AGENTS.md rule 1).
3. Confirm the build works as is: `cargo test --workspace` on the Mac.
4. Check what each engine's crate looks like **today** (latest version, its API, its licence, what it needs installed). The details below were right on 2026-11-16 but were not checked against the code: open each crate's docs before using it (rule 6).

## Ground rules for this task

- **The trial crate stays outside the workspace.** Add `exclude = ["trials/email-engines"]` to the root `Cargo.toml`'s `[workspace]`, and give the trial its own `Cargo.toml` and `Cargo.lock`. `cargo test --workspace` must not build any of these engines.
- **It may use `nightpost-core`** (as a path dependency) for `html_clean`, `mail_html` and `demo`, so each engine gets exactly what the app would give it. It must not change `nightpost-core`. If something there needs a new public function, stop and ask.
- **Each engine is a Cargo feature**, off by default: `litehtml`, `blitz`, `servo`, `webkit-gtk4`, `wpe`, `webkit-mac`. Platform-only ones are behind `cfg(target_os)` as well, and fail with a clear message on the wrong platform.
- **Dependencies:** these engines are pre-1.0 and new to the project. Tim has approved them for this trial only (not for the app). Pin exact versions (`=x.y.z`), check each licence and its dependencies' licences fit GPL-3.0-or-later (`cargo tree` and each crate's `license` field), and record the pins and licences in the results. Anything with a licence you don't recognise: stop and ask.
- **No real mail** (rule 3). Every test email is written for this trial. Don't copy a real sender's email or template (rule 12): write in the style of common senders.
- **No network.** Nothing in the trial touches the internet. "Remote" images point at a local test server started by the trial, on `127.0.0.1`.
- **Never crash** (AGENTS.md section 2) applies to the trial's own code too: no `unwrap()` outside tests; time out and record a failure instead of hanging.
- **Report only what ran** (rule 7): say which engines ran on which machines, and which didn't and why.

## Layout

```
trials/email-engines/
├── Cargo.toml            # own package; features per engine; exact pins
├── README.md             # how to run it, kept short
├── corpus/               # the test emails (committed)
│   ├── 01-demo-newsletter.html
│   ├── ...
│   └── images/           # pictures for cid: and the local "remote" server
├── src/
│   ├── main.rs           # CLI: render | check | report | list
│   ├── engine.rs         # the Engine trait and shared types
│   ├── page.rs           # turns a corpus email into the page the app would show
│   ├── loader.rs         # the resource loader: cid:, data:, local "remote", request log
│   ├── measure.rs        # timing, memory, difference scores
│   ├── checks.rs         # the must-pass checks
│   ├── report.rs         # report.html
│   └── engines/
│       ├── litehtml.rs  blitz.rs  servo.rs
│       ├── webkit_gtk4.rs  wpe.rs        # Linux
│       └── webkit_mac.rs                 # Mac: the reference
└── out/                  # PNGs, numbers, report.html (not committed: add to .gitignore)
```

## The shared interface

Something like this (adjust to what the engines allow, and explain any change in a comment):

```rust
/// One way of drawing an email. Each engine module implements this.
pub trait Engine {
    /// Short name used in file names and the report, e.g. "litehtml".
    fn name(&self) -> &'static str;
    /// Lay out and draw `page` at `opts`. Must return within `opts.timeout`.
    fn render(&mut self, page: &Page, opts: &RenderOpts) -> anyhow::Result<Rendered>;
    /// After `render`: the link under a point (in points), if any.
    fn link_at(&mut self, x: f32, y: f32) -> Option<String>;
    /// After `render`: the text between two points, if the engine can say.
    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String>;
}

pub struct Page {
    /// The email as the app would show it: cleaned by `html_clean`
    /// (trackers stripped) and wrapped by `mail_html::email_frame`.
    pub html: String,
    /// Answers every resource the engine asks for, and logs it.
    pub loader: Loader,
}

pub struct RenderOpts {
    pub width_pt: u32,          // 600 or 900
    pub scale: f32,             // 2.0
    pub dark: bool,             // dark theme colours via email_frame's `css`
    pub remote_images: bool,    // false unless the test says otherwise
    pub timeout: std::time::Duration,
}

pub struct Rendered {
    pub image: image::RgbaImage,
    pub height_pt: f32,
    pub first_draw: std::time::Duration,
    /// Every resource the engine tried to load, in order.
    pub requests: Vec<String>,
}
```

The **loader** is where privacy is tested. Each engine must be set up so every image, style sheet or font goes through it: `cid:` from the email's parts, `data:` inline, and the local "remote" server **only** when `remote_images` is true. Anything else is refused and logged. If an engine can't route all loads through us, that's a failed check, not something to work around.

**Dark theme in the trial:** pass the dark colours through `email_frame`'s `css` argument for every engine. Don't port the app's recolouring script (`NP_DARK` in `crates/nightpost/src/mail_view.rs`) for engines without JavaScript; note in the results how each engine looks without it. Moving recolouring into Rust is a later decision.

## The test emails

Write these in `corpus/`. Each is an `.html` body; where an email has pictures inside it, add an `.eml` (multipart/related) instead, so `cid:` works as in real mail. Keep each one realistic in structure but obviously made up (names like "Harbourside Bakery", addresses at `example.com`).

| File | What it is | What it checks |
|---|---|---|
| `01-demo-newsletter` | The newsletter from `nightpost_core::demo` | What the app's demo shows today |
| `02-table-newsletter` | 600 px layout from nested tables: header, hero picture, two columns, footer with unsubscribe | The most common layout in marketing email |
| `03-legacy-markup` | `<font face size color>`, `bgcolor`, `align`, `valign`, `<center>`, `width` and `height` attributes, spacer GIFs | What older senders and bulk tools still send |
| `04-responsive` | Two columns that stack under `@media (max-width: 480px)` | Media queries, and a narrow reading pane |
| `05-receipt` | Itemised table, right-aligned prices, borders, totals | Tables with alignment |
| `06-quoted-reply` | A short reply quoting two earlier messages in nested `blockquote`s | Quotes, and plain conversation mail |
| `07-inline-pictures.eml` | Two `cid:` pictures and one remote picture | `cid:` loading; the remote one stays blocked |
| `08-trackers` | A 1×1 pixel and an image from a known tracker address (as `html_clean` lists them) | These must never be requested |
| `09-styled` | Rounded buttons, a background image, a web font, `box-shadow` | Common "nice" CSS |
| `10-dark-aware` | `color-scheme` and `@media (prefers-color-scheme: dark)` | How it meets the dark theme |
| `11-languages` | Te reo Māori with macrons, Chinese, Arabic (right to left), emoji | Text shaping and fonts |
| `12-long-digest` | 200 items, each with a title, a picture and a paragraph | Speed and memory |
| `13-broken` | Unclosed tags, a file cut off halfway, a 20 MB inline image, deeply nested tables (500 levels) | Must not crash or hang |
| `14-scripts` | `<script>`, `onload=`, `onclick=`, a `javascript:` link | No script may run |

`html_clean` removes scripts before any engine sees them, so `14-scripts` must also be fed to each engine **raw** (skipping `html_clean`, still inside `email_frame`) to check the engine itself won't run them. Detect a script running by having it request a marker URL through the loader.

## What `render` produces

For each engine × email × width (600, 900) × theme (light, dark), at scale 2:

- `out/<engine>/<email>-<width>-<theme>.png`
- one line in `out/<engine>/results.jsonl`: time to first draw (cold for the first email after starting the engine, warm for the rest), height, the request log, and any error.

Then, once per engine:

- **Memory:** peak resident memory while drawing `12-long-digest` (measure in a child process so engines don't share it).
- **Build cost:** clean release build time of the trial with only that engine's feature, and how much bigger the stripped release binary is than with no engine.
- **Difference score:** for each picture, the share of pixels that differ noticeably from the `webkit-mac` picture of the same email, width and theme (resize to match first; ignore differences below a small threshold). Only on runs where the Mac reference exists.

## The must-pass checks (`check`)

Pass or fail per engine, with the reason when it fails:

1. **Privacy:** for every email with `remote_images` false, no request outside `cid:`/`data:`. For `08-trackers`, never a request to the tracker addresses, even with `remote_images` true (they're stripped first). The engine never opens a network connection of its own (the local server sees nothing it didn't permit).
2. **No scripts:** the marker URL from `14-scripts` (raw) is never requested.
3. **No crashes or hangs:** every `13-broken` case returns a picture or an error within 10 seconds, without panicking or killing the process (run each in a child process).
4. **Links:** `link_at` on the centre of a known link in `02-table-newsletter` returns its address.
5. **Text:** `text_between` across a known paragraph in `06-quoted-reply` returns that text (or, for litehtml drawn through egui later, note that egui would provide it).
6. **Sharp at 2×:** a 1-point line in `05-receipt` is 2 pixels wide, not blurred over 3 or 4 (check a known border).
7. **Licences:** the engine and everything it pulls in fit GPL-3.0-or-later.

## The report (`report`)

`out/report.html`, a single page Tim opens in a browser:

- One row per email, one column per engine, `webkit-mac` first. Light and dark side by side or on a toggle; widths on a toggle.
- Under each picture: time, difference score, requests made, any error.
- A table of the must-pass checks per engine, then memory, build time and size.
- A score box (0 to 3) under each picture, saved in the page's own storage, and an **Export scores** button that downloads them as JSON, to go into the results.

Scores mean: 3 = as good as WebKit, or differences you'd only see side by side; 2 = small differences, nothing wrong; 1 = readable, layout visibly off; 0 = broken, unreadable or crashed.

## Machines and order

| Step | Engine | Run on |
|---|---|---|
| 1 | Write the corpus, the loader, the report, and `webkit-mac` | Mac |
| 2 | `litehtml` | Mac, Fedora 44 KDE (ARM), Omarchy |
| 3 | `blitz` | Mac, Fedora, Omarchy |
| 4 | `servo` | Mac first; Linux only if the build fits (on the 4 GB Fedora machine use `CARGO_BUILD_JOBS=2` under `systemd-run --user`, and record if it can't) |
| 5 | `webkit-gtk4` | Fedora, Omarchy |
| 6 | `wpe` | Only if `webkit-gtk4` can't draw without showing a window. Ask Tim before starting it: it needs Rust bindings written for this |

After each step, show Tim the report so far and stop if he wants to change course. If litehtml or Blitz is clearly good enough after step 3, ask whether to carry on with Servo.

## When it's done

1. Add a dated entry under **Results** in `docs/gtk4-upgrade.md`:
   - commit, date, machines, and which engines ran where (and which didn't, and why)
   - each engine's exact version and licences
   - the numbers (median warm time, cold time, peak memory, build time, size added)
   - the must-pass checks
   - Tim's scores (average, and any 0s), from the exported JSON
   - anything surprising, and what Tim decided
2. Leave `trials/email-engines/` in the repository for re-testing new engine versions. `out/` stays uncommitted.
3. Don't start porting the app. That's step 2 onward of the plan in `docs/gtk4-upgrade.md`, after Tim decides.
4. Tell Tim plainly what ran where, what didn't, and whether anything is committed (AGENTS.md section 7).
