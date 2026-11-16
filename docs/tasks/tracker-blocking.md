# Task: close the gaps in tracker blocking

**Priority: important.** This is a privacy fix. Do it before the GTK 4 port and other feature work. Written 2026-11-16.

Read [AGENTS.md](../../AGENTS.md) first. Where this brief and AGENTS.md disagree, AGENTS.md wins.

## Why

NightPost promises that **trackers are always blocked** (README, "HTML email"; Preferences → Appearance says "Load remote images (trackers are always blocked)"). With **Load remote images** on, which is the default, the page's Content Security Policy lets images load from anywhere (`mail_html::csp` with `remote` true). So the only thing stopping a tracker is `strip_trackers` in `crates/nightpost-core/src/html_clean.rs`.

`strip_trackers` only looks at `<img src=…>`. A tracker loaded any other way is fetched, and the sender learns when and roughly where the email was opened. The same function serves the desktop app (`crates/nightpost/src/mail_view.rs`), the iPhone app (`crates/nightpost-ffi/src/reader.rs`) and the MCP server (`email_view.rs`), so a fix in the core reaches all three.

## The gaps

| # | Gap | Example |
|---|---|---|
| 1 | CSS backgrounds in a `style` attribute | `<td style="background-image:url(https://t.example/open.gif?u=1)">` |
| 2 | CSS in `<style>` blocks | `.x{background:url('https://t.example/o.gif')}` |
| 3 | The old `background=` attribute | `<table background="https://t.example/pixel.png">` |
| 4 | `srcset` on `<img>` | `<img src="data:…" srcset="https://t.example/open.gif 1x">` |
| 5 | `<picture><source srcset=…>` | a tracker in a `<source>` |
| 6 | Other image-loading tags | `<input type="image" src=…>`, `<video poster=…>`, `<image href=…>` inside SVG |
| 7 | A tracker on a known tracking path but with no size given | `<img src="https://x.example/wf/open?upn=1">` (should already be caught by the path list: confirm with a test) |
| 8 | `has_remote_content` misses `srcset`, `//host/…` addresses, `@import` and `url(` with spaces | decides whether "Load images" shows when remote images are off |

**Not a gap: hidden images.** `display:none` alone doesn't make an image a tracker: newsletters hide their mobile-only pictures that way, and an existing test keeps `mobile-only.jpg`. Leave that rule as it is.

**Not part of this:** removing scripts. They're stopped by the Content Security Policy and, in the app, the sandboxed frame. Say so in the file's `//!` summary so the name doesn't suggest otherwise.

## What to do

1. **Move the tests.** `strip_trackers` and `has_remote_content` are tested in `crates/nightpost/src/mail_view.rs` (the `tests` module, around the tracker test and the remote-content assert). Move those tests into a `#[cfg(test)] mod tests` in `html_clean.rs`, so `cargo test -p nightpost-core` covers it. Keep `host_list` and anything desktop-only where it is.
2. **Write a failing test for each gap** 1 to 8, using made-up addresses at `example` domains (no real mail, AGENTS.md rule 3). Run them and confirm they fail before changing the code (rule 9).
3. **Fix `strip_trackers`** so every place an email can load a picture is checked with the same rules as `<img src>` (tiny size, or a known tracking path):
   - `style` attributes and `<style>` blocks: any `url(…)` that's a tracker is replaced with `url()` (or the declaration dropped), leaving the rest of the CSS alone.
   - `background=`, `srcset` (each candidate), `<source srcset>`, `<input type="image" src>`, `poster`, SVG `<image href>` / `xlink:href`.
   - Size rules apply only where there's a size (an `<img>` or `<input>`); for CSS and `srcset`, use the tracking-path list.
   - The host of every tracker found goes in the returned list, once, as now, so the "Blocked a tracker from …" note stays right.
4. **Fix `has_remote_content`** for gap 8.
5. **Stay within the current approach if you can.** The file scans text instead of parsing HTML; keep that if every test passes cleanly. If it gets fragile, a proper rewriter would be sturdier (for example `lol_html`, BSD-3-Clause, used by Cloudflare), but that's a new dependency: **stop and ask Tim first** (AGENTS.md section 6).
6. **Keep it safe** (AGENTS.md section 2): no `unwrap()`; slice only at character boundaries; a huge or broken email must not hang it. Add a test with a long malformed email (unclosed quotes, a cut-off tag) that finishes quickly.
7. **Update the file's `//!` summary:** what it removes, where it's used, and that scripts are left to the content policy.

## Checks before finishing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
scripts/audit.sh
scripts/ios-rust.sh          # the iPhone app uses the same function
```

- Build on Linux too, or say it wasn't (no platform code changes, so it should be fine).
- Look at the demo newsletter in the app (`cargo run --release -- --demo`): it must look the same, with its blocked-tracker note.
- **README:** remove the Known issues entry "Some trackers can get past the tracker blocking", and check the "HTML email" paragraph still describes what's removed accurately.
- Say plainly what changed, what was tested and where, and whether it's committed (AGENTS.md section 7). Commit only when Tim asks.
