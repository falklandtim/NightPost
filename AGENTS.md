# AGENTS.md: house rules for OwlPost

OwlPost is Tim Morrison's own native email client, written in Rust (egui/eframe, IMAP, SQLite), for macOS and Linux. Read this file first, then `docs/architecture.md`. These rules apply to every agent and every person working on the code. A rule may be made stricter for one task, never weaker.

## 1. House rules

1. **Tim decides.** He owns the product and answers for it. Commit only when he asks. There is no remote: never add one, push, publish or release without his say-so.
2. **OwlPost never sends mail on an agent's behalf.** The MCP server saves drafts; Tim sends. `send_email` and `reply` were removed on purpose. Never add a tool, flag or script that sends.
3. **Real mail is private.** Never print, log, paste, commit or upload message content, addresses or attachments from a real account, including in crash dumps, screenshots and test fixtures. Look for the one fact you need (an error line, a count), not the mail around it. Keep personal details out of requests to outside services: no names or email addresses in headers, URLs or User-Agents.
4. **Never touch a real mailbox from a test.** Tests use the demo accounts (`owlpost_core::demo`). Every engine function has a demo path. Real-account behaviour is checked by Tim, by hand.
5. **Mac and Linux stay alike.** A feature works on both, or the README says it's Mac-only and why. Linux means Omarchy (Arch, Hyprland) and Fedora KDE, both on Wayland.
6. **Verify, don't guess.** Open a definition before using it. Confirm a cause in the code or the log before fixing it, even when someone names a place.
7. **Report only what ran.** Say what you tested, on which machine, and what you could not run ("compiles on Linux, not run there"; "demo account only"). Never call something fixed that hasn't been seen working.
8. **Show visual changes before asking about them.** Render them in a UI test screenshot, then install on the Mac (`scripts/bundle-macos.sh --install`) so Tim can look. Running copies must be quit and reopened to pick up a new build. Say so.
9. **Keep changes narrow and tested.** Change what the task asks for, where the problem is. Every fix or feature comes with a test that fails without it. Nothing unrelated in the same change.
10. **Guards say why.** A check that looks removable has a comment saying what it prevents. Read it before changing the check.
11. **Ask before anything hard to undo:** deleting mail, folders or files outside `target/`; changing data on a mail server outside the normal action paths; database or sync-state formats that older copies can't read; new dependencies with unfamiliar licences; spending money; destructive git. Otherwise keep working.
12. **Clean room.** OwlPost learns from Mailspring and Canary by observing behaviour. Don't copy code, strings or artwork from closed-source apps. Check a dependency's licence fits GPL-3.0-or-later before adding it.

## 2. Never crash

People's mail is in here. Bad server replies, odd emails, a full disk or a dropped connection must produce a message Tim can act on, never a panic.

- **Treat everything from outside as hostile:** IMAP responses, MIME, HTML, headers, file names, MCP parameters, the sync state on the server. Use `get()` instead of `[i]` on input-derived indices, slice strings only at char boundaries, and cap sizes before allocating.
- **No new `unwrap()`, `expect()` or `panic!` outside tests.** Return `anyhow::Result` and use `?`, `let … else`, `ok_or_else` or a correct fallback. Allowed: locks and thread spawns, which are already in use, with `expect("why it can't fail")`.
- **`unsafe` only for platform interop** (AppKit, Contacts, GTK, Wayland in `web_host.rs`, `contacts.rs`, `mail_view.rs`), and every block has a `// SAFETY:` comment.
- **The UI thread never touches the network.** Server work goes through a `worker::Cmd`; the UI changes the local store first, so it feels instant, then the worker brings the server into line.
- **Every crash fix gets a test** that failed before the fix.

## 3. Data that outlives a build

Old and new copies of OwlPost run side by side: the Mac, the laptop, the Fedora VM and Dobby's MCP server can each be on a different build.

- **Sync state on the server** (`snooze_sync::State`, in the hidden "OwlPost Sync" folder): new fields get `#[serde(default)]`; records carry `updated` and `removed`, the newer change wins, and removals stay as 30-day tombstones. Assume an older copy will rewrite the state without your new field, and make the next sync put it back.
- **The SQLite schema** (`store.rs`): add tables with `CREATE TABLE IF NOT EXISTS` and columns with the guarded `ALTER TABLE` migrations. Never drop or rewrite user data in a migration.
- **Mail is never deleted outright** by an action: it goes to Trash (Gmail's All Mail keeps a copy). Undo (⌘Z) reverses actions on the server, in order per conversation.

## 4. Documentation and comments

The code should explain itself to the next person or agent who opens it cold.

- **Every file starts with a `//!` summary** of what it does and, for anything non-obvious, how it's designed.
- **Every public item gets a `///` doc comment**: what it is or does, what it returns, and anything surprising (blocking, network, demo behaviour).
- **Comments say why, not what.** "Gmail's Important folder duplicates INBOX, so skip it", not "skip Important".
- **The README changes in the same commit as the behaviour.** It's the user manual. Nothing in it may go stale.
- **`docs/architecture.md` changes when the structure does:** a new thread, process, module, table or synced record.
- **Plain English, British spelling** (colour, behaviour), short sentences, in UI text, docs and comments alike.

## 5. Size and shape

- **Files stay under 1,500 lines, functions under 100.** Don't grow anything over the limit. Split it when you're working there. `scripts/audit.sh` lists what's over.
- **New UI goes in its own module** under `crates/owlpost/src/app/`, not at the end of an existing file.
- **Core logic belongs in `owlpost-core`**, which has no egui. The `owlpost` crate is the UI and its threads.
- Keep clippy clean: `cargo clippy --workspace --all-targets` adds no new warnings.

## 6. Before you finish a task

```sh
cargo test --workspace                       # all green
cargo clippy --workspace --all-targets       # no new warnings
scripts/audit.sh                             # no number got worse
```

- **Platform code** (`web_host.rs`, `mail_view.rs`, `contacts.rs`, anything `cfg(target_os)`): also build it on Linux, and say if you couldn't.
- **User-visible change:** update the README; install on the Mac for Tim to look.
- **Then say plainly** what changed, what was tested and where, what wasn't, and whether it's committed.

## 7. Building and installing

| Where | How |
|---|---|
| Mac | `scripts/bundle-macos.sh --install` (signs with the local certificate, so Keychain access survives) |
| Linux | `scripts/install-linux.sh` (installs to `~/.local/bin/owlpost` with a launcher) |
| MCP | `owlpost mcp` on stdio; register with `claude mcp add owlpost -- <path> mcp` |
| Demo | `cargo run --release -- --demo` |
| UI screenshots | `OWLPOST_SHOTS=<dir> cargo test -p owlpost <test>` saves PNGs from the offscreen UI tests |

On a small Linux machine (4 GB), build with `CARGO_BUILD_JOBS=2`, and run long builds under `systemd-run --user` so a dropped SSH connection or sleep doesn't kill them. Clean `target/` when disk runs low; it only holds build output.

## 8. Where things are

| Read | For |
|---|---|
| `README.md` | What OwlPost does, for users: every feature, the MCP server, snooze sync, known issues |
| `docs/architecture.md` | How it fits together: crates, threads, processes, data, sync |
| `docs/code-audit.md` | The audit process and each audit's results |
| `docs/ios.md` | The iPhone app plan (not started) |
| `scripts/audit.sh` | The audit numbers |
