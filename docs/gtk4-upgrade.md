# Moving the Linux build off GTK 3

Status: **ported, 2026-11-17.** The Linux composer and HTML email use WebKitGTK 6.0 and GTK 4 directly (`webkit6` and `gtk4`, exact pins). The Mac then dropped `wry` too and uses WKWebView directly (`objc2-web-kit`, `mac_web.rs`), because `wry`'s Linux half kept the GTK 3 crates in `Cargo.lock`, where `cargo audit` still flagged them. HTML email is drawn in a GTK 4 window that's never shown (realised and sized by hand), the first of the two ways below. Tested on Fedora 44 KDE and Omarchy Hyprland with `NIGHTPOST_GTK_TESTS=1 cargo test -p nightpost` (they open windows, so they're off by default). The engine trial's results are under "Results".

## Why

On Linux, NightPost's web views (the formatted composer and HTML email) use the Rust bindings for GTK 3: `gtk`, `gdk`, `atk` and friends at 0.18, plus `webkit2gtk` 2.0 and `javascriptcore-rs`. Upstream archived those bindings in 2024. gtk-rs 0.18 was the last release with GTK 3, and `cargo audit` reports them as unmaintained (for example RUSTSEC-2024-0412 for `gdk`, -0415 for `gtk` and -0420 for `gtk-sys`). They still build and run, but nobody will fix them if a GLib, GTK or Rust change breaks them. They also hold us on old copies of `glib`, `cairo` and `pango`. Fedora has deprecated its packaged copies, which doesn't affect our Cargo builds but shows where things are heading.

The GTK 3 C library itself is still shipped by every distribution. The problem is the Rust layer, not GTK 3 on the machine.

## How we got here

The GTK 3 bindings arrived with the WebKit composer (`5ce36ce`, 2026-11-08). They came in through `wry`: on Linux, `wry` only supports WebKitGTK 4.1, which is the GTK 3 build of WebKit. Nobody checked the bindings' maintenance status at the time. AGENTS.md section 6 now requires that check for every new crate.

## Why `wry` can't fix it

`wry` still targets WebKitGTK 4.1 (GTK 3) on Linux, and there's no sign it's moving to GTK 4. Tauri, which owns `wry`, is on the same path. So we stop using `wry` on Linux and talk to WebKit directly, as `mail_view.rs` already partly does. **On the Mac nothing changes:** `wry` with WKWebView stays.

## The replacement

| Now (GTK 3) | Replacement (GTK 4) | Notes |
|---|---|---|
| `gtk` 0.18 | `gtk4` | gtk-rs, maintained, MIT |
| `webkit2gtk` 2.0 (WebKitGTK 4.1) | `webkit6` 0.6 (WebKitGTK 6.0) | gitlab.gnome.org/World/Rust/webkit6-rs, MIT |
| `javascriptcore-rs` 1.1 | `javascriptcore6` | same project |
| `wry` (Linux side only) | none: build the web views directly | `wry` stays on the Mac |
| GDK 3 Wayland calls (`activation`) | `gdk4-wayland` | window activation tokens |

Licences fit GPL-3.0-or-later. Upstream WebKit publishes a guide to the API changes: "Migrating to WebKitGTK 6.0".

System packages change too. The README's Linux section will need GTK 4 and WebKitGTK 6.0 development files in place of GTK 3 and WebKitGTK 4.1 (on Fedora `gtk4-devel webkitgtk6.0-devel`, on Arch `gtk4 webkitgtk-6.0`; check the exact names when updating it).

## What changes in the code

Only Linux-only code in two files. The core, the iPhone app, the Mac and the egui window are untouched.

| Where | Lines (approx.) | What it does now | GTK 4 version |
|---|---|---|---|
| `web_host.rs`: the GTK thread (around line 99) and Linux `imp` (around 320 to 486) | 280 | Runs `gtk::main()` on its own thread; opens each composer window as a `gtk::Window` holding a `wry` web view; turns on spell checking through `webkit2gtk` | `glib::MainLoop` on the same thread (GTK 4 has no `gtk::main`); a `gtk4::Window` holding a `webkit6::WebView`; scripts through `evaluate_javascript`; messages from the page through a `UserContentManager` message handler instead of `wry`'s IPC |
| `web_host.rs`: `activation` (around 487 to 597) | 110 | Gives the composer window focus on Wayland (xdg-activation) through GDK 3 | The same through `gdk4-wayland` |
| `mail_view.rs`: `offscreen::gtk_side` (around 836 to 1000) | 165 | Draws each email in a `webkit2gtk::WebView` inside a `gtk::OffscreenWindow`, measures it with JavaScript, snapshots it as a Cairo surface and hands egui a picture | See the hard part below |

The rest of `mail_view.rs`'s Linux path (the picture in the reading pane, click targets for links, folds, Reply and attachments) is egui code and stays as it is.

### The hard part: drawing an email off screen

egui can't hold a web view on Wayland, so on Linux each email is drawn off screen and shown as a picture. That relies on `gtk::OffscreenWindow`, which **GTK 4 removed**. Two ways to replace it, to try in this order:

1. **A GTK 4 window that's never shown on screen**, with WebKit's own snapshot call (`webkit6::WebView::snapshot`, which returns a GDK texture instead of a Cairo surface). The open question is whether WebKit lays out and paints a page in a window that's never presented, on both KDE and Hyprland. If it needs to be presented, a 1×1 window that's moved out of sight or made fully transparent may do.
2. **WPE WebKit**, the build of WebKit designed to render without a toolkit window, with a headless mode for exactly this. It means another system library, and writing our own Rust bindings, since none are maintained (see the next section).

Three more engines, which would replace the native view on the Mac too, are in the next section. If nothing works well, a fallback for Linux only: show the email's text version (as the reading pane does when HTML is turned off) and say so in the README.

## Other engines for HTML email

Reading email doesn't need a full browser: NightPost already blocks every script in an email, so it only needs HTML and CSS drawn well. Three engines can draw an email inside egui itself, the same way on the Mac and Linux, with no native view laid over the window and no off-screen window. Any of them would retire the hardest part of this plan and fix "text in an HTML email can't be selected on Linux". A fourth, WPE, is a Linux-only fallback for the WebKit route.

| | Servo (`servo` crate) | Blitz (DioxusLabs) | litehtml (`litehtml` crate) | WPE WebKit |
|---|---|---|---|---|
| What it is | A full browser engine in Rust, a library since v0.1.0 (April 2026) | An HTML and CSS renderer in Rust: Stylo (Firefox's CSS engine), Taffy, Vello | A small C++ HTML and CSS engine, with Rust bindings (franzos/litehtml-rs) | The build of WebKit made to run without a toolkit; has a headless mode for off-screen drawing |
| Platforms | Mac and Linux | Mac and Linux | Mac and Linux | Linux only |
| JavaScript | Yes (switched off for email) | None | None | Yes (switched off) |
| How it draws | Pixels read back, or a GPU texture | A wgpu texture or an image | Calls us back to draw each piece (text, boxes, images), or CPU pixels via tiny-skia | Pixels from its headless back end |
| Email track record | None known | None known | Claws Mail's HTML viewer uses it | WebKit (what the Mac shows today) |
| Release state | 0.x, monthly; long-term-support line with security fixes | Experimental, 0.x | Engine: mature. Rust bindings: young, one maintainer | Mature, but no maintained Rust bindings: we'd write our own |
| Licence | MPL-2.0 | MIT or Apache-2.0 | MIT, BSD-3-Clause and Apache-2.0 | LGPL-2.1 (the system library) |
| Main cost | A very large build; too heavy for the 4 GB Fedora machine | Messy email HTML may not draw correctly yet | We may have to look after the bindings ourselves | Another system library, and our own bindings |

litehtml stands out for a small, fast app. Its bindings ask us to do the drawing, so it could draw emails into egui as real text and shapes rather than a picture: selectable text, sharp at any scale, and no recolouring script for dark themes. Its bindings also have an `email` feature with default styles for email.

All licences above fit GPL-3.0-or-later. Under AGENTS.md section 6, any of these needs Tim's OK and an exact version pin (Servo's long-term-support line if we choose it), and moving to a newer version is its own change.

None of them can replace the **composer**: it needs a real editor (typing, selection, spelling, paste, input methods). The composer stays on each system's own WebKit: `wry` on the Mac, GTK 4 and `webkit6` on Linux, as above.

**What changes with an engine that has no JavaScript.** Today's email page leans on a small script of our own for three jobs, which would move into Rust:

- **Dark themes:** recolouring an email to match (`mail_view.rs`, the `NP_DARK` script) becomes a pass over the email's styles in `nightpost-core`, or, with litehtml, choosing the colours as we draw.
- **Sizing:** measuring each email's height comes straight from the engine's layout instead.
- **Clicks and keys:** links, folds and shortcuts are handled by egui, by asking the engine what's under the pointer.

Tracker blocking and the content policy already happen in Rust (`html_clean.rs`, `mail_html::email_frame`), so they carry over to any engine.

**Ruled out:**

- **Embedded Chromium (CEF):** handles both jobs, but adds hundreds of megabytes.
- **Ladybird:** a promising new engine, but there's no library to embed and it hasn't reached its first alpha.
- **Gosub:** a Rust engine still at the very start; it can't draw most pages yet.
- **Ultralight** and **Sciter:** closed source. Ultralight's free licence covers only non-commercial use and forbids static linking. Neither fits GPL-3.0 or the clean-room rule.
- **Replacing egui** with another toolkit (Qt, Tauri): Tauri uses `wry`, so it has the same GTK 3 problem on Linux, and Qt means rewriting the whole interface.

## How we'll test the engines

The aim is to choose how NightPost draws HTML email from evidence: pictures Tim can look at, numbers, and a short list of things each engine must be able to do. Nothing in the app changes during the test. The step-by-step brief for carrying it out is [docs/tasks/email-engine-trial.md](tasks/email-engine-trial.md).

### Where the test lives

A separate crate, `trials/email-engines/`, kept out of the main workspace (`exclude` in the root `Cargo.toml`) so none of these engines is built by `cargo test --workspace` or reaches the app. Each engine is a Cargo feature, so one can be built at a time:

```sh
cd trials/email-engines
cargo run --release --features litehtml -- render       # draw every test email
cargo run --release --features blitz    -- render
cargo run --release --features servo    -- render
cargo run --release --features webkit-gtk4 -- render    # Linux only
cargo run --release --features wpe      -- render       # Linux only, if needed
cargo run --release --features webkit-mac  -- render    # Mac only: today's look, the reference
cargo run --release -- report                           # build report.html from what's there
```

Every engine sits behind one small trait (load an email page, lay it out at a width, draw it to RGBA, report what's at a point, list what it tried to fetch), so they're tested the same way. The `webkit-mac` back end uses the same WKWebView as the app today and gives the reference pictures.

### The test emails

Written for the test, in `trials/email-engines/corpus/`. **No real mail** (AGENTS.md rule 3): every email is made up, styled the way common senders build them, but not copied from any one sender (rule 12). Each passes through NightPost's own cleaning (`html_clean.rs`) and `mail_html::email_frame`, exactly as in the app, so each engine gets what the app would give it.

| Email | What it checks |
|---|---|
| The demo newsletter (`nightpost_core::demo`) | What the app's demo shows today |
| A 600 px newsletter built from nested tables | The most common layout in marketing email |
| Old-style markup: `<font>`, `bgcolor`, `align`, `<center>`, `width` attributes | What older senders and bulk tools still produce |
| A newsletter with a mobile layout (`@media (max-width: …)`) | Media queries, and how it looks in a narrow reading pane |
| A receipt with a table of items and totals | Tables with alignment and borders |
| A plain reply quoting two earlier messages | Quotes (`blockquote`) and plain text at its best |
| Pictures inside the email (`cid:`) and a blocked remote picture | NightPost's image handling and the "Load images" placeholder |
| A tracking pixel, and an image from a tracker's address | Must never be fetched (see the privacy checks) |
| Buttons with rounded corners, a background image, a web font | Common "nice" CSS |
| A dark-mode-aware email (`prefers-color-scheme`, `color-scheme`) | How it meets NightPost's dark theme |
| Te reo Māori with macrons, Chinese, Arabic (right to left), emoji | Text shaping and fonts |
| A very long email (a 200-item digest) | Speed and memory |
| Broken HTML: unclosed tags, a cut-off file, a huge inline image | Must not crash or hang |

### What we measure

For each engine, each email, at two widths (600 and 900 points), at 2× scale, in the light theme and the dark theme:

1. **How it looks.** A PNG of each, plus a difference score against the Mac WebKit reference (the share of pixels that differ noticeably, from `image`). The score only flags where to look; Tim's eye decides.
2. **Speed.** Time to the first full drawing, cold (first email after start) and warm (the median of the rest), on the Mac and on Fedora.
3. **Memory.** Peak memory while drawing the long digest.
4. **Cost to build and ship.** Clean build time and how much bigger a release `nightpost` binary would be. Whether it builds at all on the 4 GB Fedora machine (`CARGO_BUILD_JOBS=2`).
5. **Where it runs.** Mac (Apple silicon), Fedora 44 KDE (ARM) and Omarchy (Hyprland). Report only what actually ran (AGENTS.md rule 7).

### What each engine must be able to do

Checked with small tests in the trial crate, pass or fail:

- **Privacy:** with the network blocked, it fetches nothing on its own. Every image, style or font it wants goes through our loader, so NightPost decides what loads (`cid:` from the message, remote only after "Load images", trackers never). The trial logs every request each engine makes.
- **No scripts:** a `<script>` and an `onload=` in an email never run.
- **No crashes:** every broken email draws something or fails cleanly, within a time limit. Nothing panics or hangs (AGENTS.md section 2).
- **Links:** we can ask what's under a point and get the link's address.
- **Selecting text:** we can get the text in a dragged range (or, for litehtml drawing through egui, egui can select it).
- **Sharp at 2×:** text and lines are crisp on a Retina display.
- **Licence:** confirmed for the engine and everything it pulls in (`cargo tree`, and each crate's licence).

### Scoring and choosing

`report` builds `trials/email-engines/report.html`: a grid with one row per email and one column per engine, with the Mac WebKit picture first, and the numbers and pass/fail checks underneath. Tim scores each picture:

| Score | Means |
|---|---|
| 3 | As good as WebKit, or differences you'd only see side by side |
| 2 | Small differences (spacing, a font), nothing wrong |
| 1 | Readable, but the layout is visibly off |
| 0 | Broken, unreadable, or crashed |

An engine is in the running only if it passes every privacy, script and crash check, averages at least 2.5, scores no 0 on the newsletters, receipt or reply, and builds and runs on all three machines. Among those, the lighter and faster one wins, with Tim deciding. If none is in the running, Linux takes the GTK 4 WebKit route above (with WPE as its fallback) and the Mac keeps its native view.

### Order

1. Write the test emails and the reference back end (`webkit-mac`) first, so there's something to compare against.
2. **litehtml** and **Blitz**: cheapest to build, and either could make the rest unnecessary.
3. **Servo**.
4. **GTK 4 WebKit** on Fedora and Omarchy: needed for the Linux route either way.
5. **WPE**, only if GTK 4 WebKit can't draw off screen.

### Recording the results

Each run adds a dated entry under "Results" at the end of this file: which engines ran on which machines, the numbers, the scores, and what was decided. The pictures stay in `trials/email-engines/out/`, which isn't committed. Once the choice was made (WebKit on both systems, 2026-11-17), the trial program stayed on its branch, `trial/email-engines`, rather than `main`: it pins pre-release engines and would slowly stop building. Check out that branch to re-test an engine.

## Ready for GTK 5

GTK 5 hasn't been released, but the GTK team has published what to prepare for: [Migrating from GTK 4 to GTK 5](https://docs.gtk.org/gtk4/migrating-4to5.html). GTK 5 will break both the API and the ABI, and the main way to prepare is to stop using anything GTK 4 has deprecated. Since we're writing this port now, we write it so the move to GTK 5 is small:

- **No deprecated GTK 4 calls.** gtk4-rs marks them `#[deprecated]`, so the compiler warns. Never silence that with `#[allow(deprecated)]` in this code: use the replacement the deprecation note names. Build with the newest GTK 4 the supported Linux versions have, so we see the newest deprecations.
- **Keep GTK in one place.** All GTK and WebKitGTK calls stay in the Linux-only modules of `web_host.rs` and `mail_view.rs`, behind the same functions the Mac uses. Moving to GTK 5 (and whatever WebKitGTK version goes with it) then changes those modules and nothing else.
- **Use only the parts of GTK we need:** a window, a web view, its snapshot and the main loop. No dialogs, info bars or other widgets, many of which go in GTK 5. Native dialogs come from `rfd` and egui, as now.
- **Pictures as `GdkTexture`,** never `GdkPixbuf`: GTK 5 drops GdkPixbuf as the way pictures move around. WebKitGTK 6.0's snapshot already returns a texture.
- **Sizes from `width()` and `height()`** (or `compute_bounds()`), not the "allocated" size calls or `allocation()`, which go. This matters for the off-screen view, which the trial had to size by hand.
- **Links open in the browser through our own code** (as now), not `gtk_show_uri()`, which is replaced by `GtkUriLauncher`.
- **Dark theme through `gtk-interface-color-scheme`,** not `gtk-application-prefer-dark-theme` (deprecated since GTK 4.20, as the trial found).

WebKitGTK will also need a new version for GTK 5. When one is announced, note it here, and check whether `webkit6`'s maintainers plan Rust bindings for it.

## Plan

1. **Test the engines for HTML email**, as set out in "How we'll test the engines" above. This decides how step 3 is done before any porting starts.
2. **Port the composer window** (`web_host.rs`), the easier half: window, web view, messages to and from the page, spell checking, Wayland activation.
3. **Port the HTML email view**, using whatever step 1 chose: `mail_view.rs` `gtk_side` for GTK 4 WebKit, or a new renderer shared by both platforms for Servo, Blitz or litehtml (then also checked on the Mac, where it replaces the native reading view).
4. **Remove the GTK 3 crates:** `gtk`, `webkit2gtk`, `javascriptcore-rs` and the Linux `wry` entry in `crates/nightpost/Cargo.toml`. Check `cargo tree` shows no `gtk-sys` 0.18 left and `cargo audit` no longer lists the GTK 3 advisories.
5. **Update the README** (Linux build packages, and remove this Known issue) and `docs/architecture.md` (the GTK thread row and the Linux web view notes).
6. **Check it:**
   - No deprecation warnings from GTK or WebKitGTK, and no `#[allow(deprecated)]` in the Linux modules (see "Ready for GTK 5").
   - `cargo test --workspace`, clippy and `scripts/audit.sh` on Linux and on the Mac (the Mac build must not change).
   - Tim, by hand, on Fedora KDE and Omarchy: write, format, paste and drag pictures into a message, spelling, send and drafts; open HTML newsletters in light and dark themes, click links, folds, Reply and attachments; the composer window comes to the front when it opens.

## Not part of this

- The Mac composer. (The Mac's reading view changes only if the engine test picks Servo, Blitz or litehtml.)
- Replacing egui on Linux or moving the whole app to GTK 4. Only the web views use GTK.
- The composer on any engine other than the system's own WebKit.
- Embedded Chromium (CEF).

## Results

### 2026-11-16: first run (branch `trial/email-engines`, on e542eeb)

**Where it ran.** The trial program is kept on the branch `trial/email-engines` (on GitHub), not on `main`: it's in `trials/email-engines/` there (see its README). Servo builds from `trials/email-engines/servo-build/`, because its copy of Stylo can't share a lock file with Blitz's.

| Engine | Mac (Apple silicon, macOS 26) | Fedora 44 KDE (ARM VM, Wayland) | Omarchy (x86_64, Hyprland) |
|---|---|---|---|
| WebKit, Mac (reference) | Ran | Mac only | Mac only |
| litehtml | Ran | Ran | Ran |
| Blitz | Ran | Ran | Ran |
| Servo | Ran | Didn't build: SpiderMonkey needs `clang llvm m4`; stopped (see below) | Build stopped part way (see below) |
| WebKitGTK 6.0 (GTK 4) | Linux only | Ran | Ran (drawn at 1×: the laptop's screen scale) |
| WPE | Not needed (GTK 4 drew off screen) | | |

Servo was stopped when Tim chose the GTK 4 route for Linux. Its Mac results were enough to rule it out for now.

**Versions and licences** (pinned exactly in the trial only):

| Engine | Crates | Licences |
|---|---|---|
| litehtml | `litehtml` 0.2.6, `litehtml-sys` 0.2.5 | MIT; the C++ engine MIT, BSD-3-Clause, Apache-2.0 |
| Blitz | `blitz-html`, `-dom`, `-traits`, `-paint` 0.3.0-beta.4 (pre-release); `anyrender` 0.14.0; `anyrender_vello_cpu` 0.18.0 | MIT or Apache-2.0; 138 crates added, all fit |
| Servo | `servo` 0.8.0 | MPL-2.0; one crate flagged: `webpki-roots` (root certificates, CDLA-Permissive-2.0), for Tim to check |
| WebKitGTK 6.0 | `webkit6` 0.6.1, `gtk4` 0.11.5 (with `v4_10`) | MIT; 44 crates added, all fit |
| WebKit, Mac | `objc2` 0.6.5, `objc2-web-kit` 0.3.2 (as the app) | MIT, Zlib, Apache-2.0 |

**The must-pass checks** (7: privacy, no scripts, no crashes, links, text, sharp at 2×, licences):

| Engine | Passed | Failed |
|---|---|---|
| WebKit, Mac | 7 | — |
| WebKitGTK 6.0, Fedora and Omarchy | 7 | — |
| Blitz (all three machines) | 7 | — |
| litehtml (all three machines) | 6 | No crashes: hangs on nested tables |
| Servo, Mac | 3 | No scripts, no crashes, text, licences (flagged) |

**Numbers** (median per email after the first; first email after starting; peak memory drawing the 200-item digest):

| Engine | Per email | First | Peak memory | Clean build (Mac) | Adds to the binary |
|---|---|---|---|---|---|
| WebKit, Mac | 88 ms | 1.6 s | 526 MB + WebKit's own process | 36 s | 0.3 MB |
| WebKitGTK 6.0, Fedora / Omarchy | 145 / 305 ms | 218 / 465 ms | 812 / 280 MB + WebKit's own process | — | — |
| Blitz, Mac / Fedora / Omarchy | 11 / 22 / 23 ms | 16 / 334 / 43 ms | 185 / 183 / 177 MB | 65 s | 13.7 MB |
| litehtml, Mac / Fedora / Omarchy | 14 / 20 / 40 ms | 20 / 20 / 60 ms | 318 / 294 / 291 MB | 54 s | 3.8 MB |
| Servo, Mac | 45 ms | 186 ms | 268 MB | 260 s | 85 MB |

Both WebKits make a fresh web view for every email (so nothing is cached between them), which the app wouldn't do: their per-email times are pessimistic. Their memory figures leave out WebKit's separate web process, and the GTK 4 figure is mostly the full-page picture of the long digest.

**What was found:**

- **GTK 4 WebKit draws without showing a window,** on KDE Wayland and on Hyprland. The window must be realised and the web view given its size by hand (`realize` and `allocate`): a window that's never shown is otherwise 0 × 0, and WebKit lays the page out with no width. With that, its pictures match the Mac's WebKit closely, and every check passes on both. A hidden window draws at its display's scale (2× on the Fedora VM, 1× on the laptop), and a full-page snapshot is never shorter than the view, so short emails get blank space below them in the pictures (not on screen).
- **GTK's dark preference is deprecated.** `gtk-application-prefer-dark-theme` still makes an email's `prefers-color-scheme` dark on both machines, but GTK 4.20 and later warn to use `gtk-interface-color-scheme`: the port should use that.
- **litehtml can't be used as it is.** Its layout time triples with each level of nested tables: 10 levels take 57 ms, 13 take 1.5 s, 15 take 13 s, 500 never finish. Real newsletters nest tables that deep. It also drops Arabic text entirely, and its sample renderer (`pixbuf`) picks fonts badly (bold comes out monospaced on the Mac).
- **Blitz is the strongest of the new engines.** It passes everything everywhere and is the fastest, but it's a pre-release, ignores `<font size color>` and table `border="1"`, sizes two 50% columns unequally, and draws emoji poorly (none on the Mac and Omarchy, black-and-white on Fedora).
- **Servo is close to WebKit in looks, but not safe as it is.** Its Content Security Policy lets an inline `onload` run (the app's sandboxed iframe still stops it), it crashes the whole process on 500 nested tables (200 are fine), and its text-selection query returned nothing. It also needs `rusqlite` 0.38 while NightPost uses 0.37 (only one SQLite can be linked), so using it would mean that upgrade too. It has no switch to turn JavaScript off.
- **The desktop app doesn't show `cid:` pictures** (pictures sent inside an email). This was already known: the README's "Not yet implemented" lists it. The core can already find the picture (`email_view::inline_part`), and the iPhone app and the MCP `view_message` tool use it; only the desktop's HTML view doesn't. Separate from this work, and left as a to-do.
- **`html_clean` doesn't remove scripts.** The page's Content Security Policy is what stops them (and, in the app, the sandboxed iframe).

**Scores:** the two WebKits (Mac and GTK 4) scored highest, in Tim's judgement (2026-11-17). No per-picture scores were recorded.

**Decided (Tim, 2026-11-16):** Linux takes the GTK 4 WebKit route for both reading and writing, as planned in "The replacement" above; it draws off screen on both desktops, so WPE isn't needed. Servo is stopped. Blitz stays the fallback.

**Decided (Tim, 2026-11-17):** NightPost stays with the system WebKit on both desktops: WKWebView on the Mac (as today) and WebKitGTK 6.0 on Linux. litehtml, Blitz and Servo aren't taken further.
