# Plan: a Flatpak for Linux

**Status: first version built, 2026-11-19.** Written 2026-11-18.

**Tim's decisions (2026-11-19):** shared as `.flatpak` bundles on GitHub releases, starting with local builds. The Flatpak copies the installed copy's database and settings across once, on its first run (`crates/nightpost-core/src/flatpak.rs`).

**What was built:** the manifest `flatpak/co.timson.NightPost.yml` (GNOME 51, Rust 1.99.0 installed from the official release for the build only), `flatpak/cargo-sources.json` made by `scripts/flatpak-cargo-sources.py` (Python's standard library only, instead of flatpak-builder-tools' generator: NightPost's crates all come from crates.io), the desktop file, AppStream metainfo and icons in `flatpak/`, and `scripts/flatpak.sh` to build, install and bundle. The plan below is kept as written; the checks in step 3 are where the testing stands.

**Checked on the Omarchy laptop (x86_64), 2026-11-19, demo accounts only:** it builds with `scripts/flatpak.sh --bundle` (an 8 MB bundle) and installs; the permissions are the ones listed below; the MCP server answers over stdio and lists the demo accounts; the sandbox reaches the keyring service; the window starts natively on Hyprland and stays up. Not yet checked: everything in step 3 that needs looking at the window, the one-time copy with Tim's real data (step 4), Fedora and aarch64 (step 5).

**Lessons from the first build:**

- **Test runs can't use their own folders.** `flatpak run` sets `XDG_DATA_HOME` and friends itself, and `--env=` doesn't override them, so even a `--demo` run writes a database into `~/.var/app/co.timson.NightPost/`. A database there means the one-time copy never happens. After testing, and before the first real run, delete `~/.var/app/co.timson.NightPost/`.
- **Don't strip the compiler.** flatpak-builder splits out and strips debug information from everything a module installs, which broke Rust's `libLLVM`. The Rust module turns that off (`no-debuginfo`, `strip: false`).
- **Copying a picture to the clipboard** may not work: egui copies text through the Wayland clipboard, but pictures only through X11, which the sandbox doesn't give a Wayland session (the log says "Failed to initialize arboard clipboard"). Text copy and paste are unaffected. To check. Read [AGENTS.md](../AGENTS.md) first. Where this plan and AGENTS.md disagree, AGENTS.md wins.

## Why

Today Linux gets NightPost from `scripts/install-linux.sh`. Each machine needs Rust and the GTK, WebKitGTK, OpenSSL and D-Bus development packages, a long build (11 minutes on the Omarchy laptop), and whatever GTK and WebKitGTK versions that distribution ships. Arch and Fedora already differ.

A Flatpak would give:

- **One package for every distribution**, including Omarchy and Fedora, with no development packages to install.
- **The same GTK and WebKitGTK everywhere**, from the GNOME runtime. That fits the house rule that Mac and Linux stay alike (AGENTS.md rule 5): fewer "works on Fedora, not on Arch" surprises.
- **Updates that don't need a build on each machine.**

The install script stays. The Flatpak is an extra way to install, at least until it has proved itself.

## Decisions for Tim before starting

1. **How it's shared.** In order of effort:
   - **Local only** (recommended first): build on a machine, install there with `flatpak install --user`, or copy a single `.flatpak` bundle file to another machine.
   - **Bundles attached to GitHub releases.** This is publishing, so it needs Tim's say-so (AGENTS.md rule 1).
   - **Flathub.** It reaches everyone, with automatic updates, but the source must be public and Flathub reviews the manifest and permissions. This needs Tim's say-so, and must not change the repository's visibility without it.
2. **What happens to existing accounts and mail.** A Flatpak keeps its data in `~/.var/app/co.timson.NightPost/`, separate from the installed copy's `~/.local/share/nightpost/`. The choices:
   - **Start fresh** (simplest): add the accounts again and let them sync. Passwords are in the keyring under each address, so they may be found again (see step 2.4).
   - **Import once:** on first run, offer to copy the old database across. That needs read access to the old folder, which is one more permission.
3. **How far MCP's `save_draft` may reach** for attachments (see the permissions table). Today it can attach from Downloads, Documents and Desktop. The sandbox can allow exactly those folders, read-only.

## What NightPost needs from the sandbox

Each row is something the code does today, and the permission (`finish-args`) or portal that lets it work inside the sandbox.

| Need | Where in the code | Permission or portal |
|---|---|---|
| IMAP, SMTP, Google sign-in, sender logos | `engine.rs`, `smtp.rs`, `oauth.rs`, `avatars.rs` | `--share=network` |
| The window, native on Wayland, through XWayland with `NIGHTPOST_X11=1` | `main.rs` (`linux_wayland`) | `--socket=wayland --socket=fallback-x11 --device=dri` |
| Passwords and Google tokens in GNOME Keyring or KDE Wallet | `credentials.rs`, through the `keyring` crate's `sync-secret-service` (D-Bus) | `--talk-name=org.freedesktop.secrets` |
| Opening links and attachments in other apps | `worker.rs` runs `xdg-open` | The runtime's `xdg-open` hands these to the OpenURI portal. No permission needed. **Check** that files open, not just links, and that "show in folder" works. |
| **Save to Downloads** | `worker.rs` (`SaveToDownloads`) | `--filesystem=xdg-download` |
| Save As, Attach Files, Insert Picture | `rfd` 0.15, which already uses the portal (`ashpd`) on Linux | The FileChooser portal. No permission needed. |
| MCP `save_draft` attaching from Downloads, Documents, Desktop | `mcp.rs` (allowed folders) | `--filesystem=xdg-documents:ro --filesystem=xdg-desktop:ro` (Downloads is already allowed above). **Check** that `directories::UserDirs` finds the same folders inside the sandbox. |
| Spelling in the composer | `web_host.rs` (`spell_check`, Hunspell dictionaries) | **Check** which dictionaries the GNOME runtime has. If English is missing, add a dictionary module to the manifest. |
| HTML email and the composer (WebKitGTK 6.0) | `web_host.rs`, `mail_view.rs` | Comes with the GNOME runtime. WebKit runs its own sandbox inside Flatpak's, which the runtime supports. |
| Database, themes, attachment cache, log file | `lib.rs` (`data_dir`), `log_path` | Nothing extra. Inside the sandbox these move under `~/.var/app/co.timson.NightPost/` (the README must say so). |

**Leave out** host-wide access such as `--filesystem=home` or `--talk-name=org.freedesktop.Flatpak`. None of NightPost's features need them, and Flathub would question them.

## The code changes it needs

These are small, and none is Flatpak-only beyond checking one environment variable.

1. **The MCP command.** Preferences → MCP Server (`prefs.rs`, `current_exe`) and the install script show `…/nightpost mcp`. Inside a Flatpak the program's own path is `/app/bin/nightpost`, which doesn't exist outside the sandbox. When `FLATPAK_ID` is set, show `flatpak run co.timson.NightPost mcp` instead. This is the command for `claude mcp add`, and for Dobby on Fedora. Add a test.
2. **The README's paths.** In the Linux section, describe where the log file, database and attachment cache live inside the Flatpak, and how to install it.
3. **Optional, later:** use the Secret portal instead of talking to the keyring directly, which would drop `--talk-name=org.freedesktop.secrets`. That's a change in the `keyring` crate, or a new dependency. Not needed for a first version.

## Building it

### The manifest

Put it in `flatpak/co.timson.NightPost.yml`, with the app ID NightPost already uses. The ID matches `timson.co`, which Flathub would verify.

- **Runtime:** `org.gnome.Platform` and `org.gnome.Sdk`, the newest stable version. It must have GTK 4.10 or later (`gtk4` is built with `v4_10`) and WebKitGTK 6.0. **Check both** in the chosen version before starting.
- **Rust:** the house rule pins the compiler (`rust-toolchain.toml`, 1.99.0 now), and rustup doesn't run inside a Flatpak build. The common route is the `org.freedesktop.Sdk.Extension.rust-stable` extension, but it ships whatever Rust its SDK branch has, which may be older than 1.99. **Recommended:** a manifest module that installs the official Rust 1.99.0 release, downloaded from `static.rust-lang.org` and checked by SHA-256. The Flatpak then builds with the same compiler as the Mac, the laptop and the iPhone. When the pin moves (AGENTS.md section 6), this module moves with it.
- **Crates without the network:** `flatpak-builder` builds offline. `flatpak-cargo-generator.py` (from `flatpak-builder-tools`) turns `Cargo.lock` into a list of sources, `flatpak/cargo-sources.json`, which is committed. The build then runs `cargo --offline build --release -p nightpost`. **Regenerate this file whenever `Cargo.lock` changes**, so add it to the monthly "Update dependencies" step.
- **Installed files:**
  - The program in `/app/bin/nightpost`.
  - The `.desktop` file, the same as the install script's, with `Exec=nightpost`.
  - Icons at 64, 128, 256 and 512. These would be committed, made from `crates/nightpost/assets/icon-1024.png`, since the SDK has no ImageMagick.
  - An AppStream `co.timson.NightPost.metainfo.xml` describing the app. Flathub requires it, and GNOME Software and KDE Discover show it.
- **Build settings:** `CARGO_BUILD_JOBS=2` for the 4 GB laptop (AGENTS.md section 8).

### A script

Add `scripts/flatpak.sh`, in the style of the other scripts:

- `flatpak-builder --user --install --force-clean` builds the Flatpak and installs it for the current user.
- `--bundle` also writes a `nightpost-<version>-<arch>.flatpak` file, which can be copied to another machine.

Each processor type is built on its own kind of machine: x86_64 on the Omarchy laptop, aarch64 on the Fedora VM. A GitHub Actions build for both would come later, with Tim's OK.

### Licences

No new crates. The tools are `flatpak-builder` (LGPL) and `flatpak-builder-tools` (MIT). They're used to build, not shipped in NightPost, so they don't affect its GPL-3.0-or-later licence.

## Steps

| # | Step | Where | Done when |
|---|---|---|---|
| 0 | Tim decides how it's shared, and what happens to existing data (above) | | Answered |
| 1 | Install `flatpak` and `flatpak-builder`, add the Flathub remote for the runtime, write the manifest, generate the crate sources, and build | Omarchy laptop | It builds and starts |
| 2 | Make the code changes above, with tests. Update the README. | Mac, then the laptop | `cargo test`, clippy and the audit all pass |
| 3 | Run the checks below with the **demo accounts** (`flatpak run co.timson.NightPost --demo`). The sandbox has its own empty database, so the demo can't touch real mail (AGENTS.md rule 4). | Omarchy laptop | Every check passes or is written down |
| 4 | Tim adds a real account and uses it alongside the installed copy | Omarchy laptop | Tim is happy with it |
| 5 | The same on Fedora KDE (aarch64, KDE Wallet). Register Dobby's MCP server with `flatpak run co.timson.NightPost mcp`. | Fedora VM | Dobby can list threads and save a draft |
| 6 | Update AGENTS.md: a Flatpak row in section 8; the GNOME runtime version in section 6, moved on purpose like the Rust pin (each runtime is supported for about a year) | Mac | Committed |
| 7 | Optional, with Tim's say-so: bundles on GitHub releases, a CI build, or Flathub | | |

## Checks (step 3)

Do each one on Hyprland (Omarchy) and on KDE (Fedora), because they differ:

- [ ] **The window:** native on Wayland, the right icon and name in the dock or task bar, and dark themes work.
- [ ] **Reading:** HTML email draws (in the hidden GTK 4 window), links open in the browser, folds work, trackers are blocked, and the dark recolouring works.
- [ ] **The composer:** typing, formatting, spelling underlines, pasting and dragging in pictures, and Attach Files (the portal dialog).
- [ ] **Attachments:** open one (and the "risky file" question), Save to Downloads, and Save As.
- [ ] **Accounts:** add one with a password, restart and check it's remembered (Secret Service), and do a Google sign-in through the browser.
- [ ] **Syncing:** push stays connected, and moving, archiving and snoozing work.
- [ ] **MCP:** `flatpak run co.timson.NightPost mcp` answers over stdio, and `save_draft` can attach from Downloads.
- [ ] **The log file:** written inside `~/.var/app/co.timson.NightPost/`, and **Show log file** opens it.
- [ ] **Running side by side with the installed copy:** each keeps its own database, and both see the same server.

## Risks and open questions

- **The Rust version:** covered by installing the pinned compiler as a module (see the manifest).
- **The runtime's WebKitGTK differs from Arch's and Fedora's.** That's the point, but an HTML email bug may then appear in one and not the other. Note which one was being tested when reporting.
- **Keyring prompts inside the sandbox:** the installed copy already works with GNOME Keyring on Omarchy, after the keyring fix of 2026-11-17. Check that the Flatpak gets the same keyring and doesn't make a new one.
- **Spelling dictionaries** may not be in the runtime (see the permissions table).
- **The first download:** installing the GNOME runtime fetches several hundred megabytes, once. Later NightPost updates are small.
- **Two databases:** if both copies are used, each syncs the mail separately. Decision 2 covers this.

## Not part of this

- An AppImage or Snap.
- Windows.
- Changing how the Mac or iPhone apps are built.
