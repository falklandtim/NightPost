# Email engine trial

A test program, not part of NightPost. It draws made-up emails with different HTML engines so Tim can choose how NightPost shows HTML email once the GTK 3 bindings go. The plan is in [docs/gtk4-upgrade.md](../../docs/gtk4-upgrade.md) and the brief in [docs/tasks/email-engine-trial.md](../../docs/tasks/email-engine-trial.md).

It's kept out of the NightPost workspace, with its own `Cargo.lock`, so none of these engines reaches the app.

## Running it

Each engine is a Cargo feature. Build one at a time:

```sh
cd trials/email-engines
cargo run --release --features webkit-mac -- render   # draw every email (Mac: the reference)
cargo run --release --features webkit-mac -- check    # privacy, scripts, crashes, links, text, sharpness, licences, memory
cargo run --release -- report                         # out/report.html from whatever is in out/
open out/report.html
```

Other commands: `list` (the emails and the engines built in), `make-corpus` (rewrite the `.eml` emails from `src/corpus_gen.rs`), and `render --only 07` (draw just the emails whose names contain that text).

## How it works

- `corpus/` holds the emails, all made up. `{{REMOTE}}` stands for the web: a small server on `127.0.0.1` serves pictures made in `src/images.rs`, so nothing leaves the machine.
- Each email goes through NightPost's own `html_clean` and `mail_html::email_frame` (from `nightpost-core`), as in the app.
- `render` draws with remote images on, so pictures compare like for like. `check` tests privacy with them off.
- The broken emails (`13…`) and the memory figure each run in a child process, so a crash can't end the run.
- Dark pictures use the dark theme's colours, without the app's recolouring script, for every engine.
- `out/` holds the pictures, numbers and report. It isn't committed.

Engines: `webkit-mac` (the Mac's WKWebView, the reference), `litehtml` and `blitz` (Mac and Linux), `servo` (from `servo-build/`) and `webkit-gtk4` (Linux).

`./measure-build.sh litehtml blitz` times a clean release build with each engine alone and records how much it adds to the binary (into `out/build-cost.jsonl`).

On Linux, litehtml needs a C++ compiler (`sudo dnf install gcc-c++` on Fedora).

**WebKitGTK 6.0** (`--features webkit-gtk4`, Linux only) needs `gtk4-devel webkitgtk6.0-devel` (Fedora) or `gtk4 webkitgtk-6.0` (Arch), and a running desktop session. Over SSH, borrow the session's settings first, for example on KDE:

```sh
export $(tr '\0' '\n' < /proc/$(pgrep -u "$USER" -x plasmashell)/environ | grep -E '^(WAYLAND_DISPLAY|XDG_RUNTIME_DIR|DBUS_SESSION_BUS_ADDRESS|DISPLAY)=' | xargs)
```

(on Hyprland, use `pgrep -x Hyprland`). The window is never shown; `TRIAL_GTK_PRESENT=1` shows it, to compare. `TRIAL_DEBUG=1` prints the page's viewport size.

**Servo** builds from `servo-build/`, a package of its own: its copy of Stylo can't share a lock file with Blitz's, and it links its own SQLite, so it can't link `nightpost-core` either. Its pages come from the main build:

```sh
cargo run --release --features litehtml -- prepare            # writes out/pages/
cd servo-build && cargo run --release --features servo -- render --engine servo
```

On Linux, Servo's SpiderMonkey build also needs `clang`, `llvm` and `m4`.
