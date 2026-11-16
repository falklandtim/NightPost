# Moving off `imap` 2.4.1

Status: **proposal, 2026-11-16. Nothing changed in the code yet. Tim decides which way to go.**

## Why

Every build prints a future-incompatibility warning for `imap-proto` 0.10.2. It comes from `imap` 2.4.1, our IMAP client, which has had no stable release since. A future Rust release will turn the warning into an error and the build will fail. The rule in [AGENTS.md](../AGENTS.md) section 6 is to fix this by moving to a maintained version, not by patching a private copy of `imap-proto`.

The maintained parser is `imap-proto` 0.16. Two IMAP clients use it.

## The two candidates

| | `imap` 3.0.0-alpha.15 | `async-imap` 0.11 |
|---|---|---|
| What it is | The next version of the crate we use (jonhoo/rust-imap) | A fork of rust-imap made async, used by Delta Chat (async-email/async-imap) |
| Release state | Alpha. alpha.15 came out in Feb 2025, nothing since. The README asks for new maintainers. | 0.x but used in production by Delta Chat on phones and desktops, with regular releases |
| Style | Blocking, like now | Async: needs a runtime (Tokio) |
| Parser | `imap-proto` 0.16 | `imap-proto` 0.16 |
| Licence | MIT or Apache-2.0 | MIT or Apache-2.0 |
| Our rules | Pre-release: needs Tim's say-so and an exact pin (`=3.0.0-alpha.15`) | Meets them |
| Risk | Getting stuck again on an alpha nobody finishes | A bigger change all at once, and a new runtime inside the core |

## What has to change either way

All IMAP code is in four files of `nightpost-core` (about 3,350 lines). The app (`crates/nightpost`) and the iPhone bridge (`crates/nightpost-ffi`) never touch `imap` directly, so neither needs to change as long as the `engine` functions keep their signatures.

| File | Uses | Notes for the port |
|---|---|---|
| `engine.rs` (1,433 lines) | connect, STARTTLS, login, select, `uid_fetch`, `uid_search`, `uid_store`, `uid_mv`, `uid_copy`, append (with flags and date), list, create, delete, subscribe, expunge, close, noop, capabilities, logout | Our own sockets with connect, read and write timeouts (`open_tcp`) must keep working. Gmail's `X-GM-LABELS` and `X-GM-RAW` are sent as plain command text, so they port as they are. 18 tests, four of them on a scripted fake server stream. |
| `idle.rs` (249 lines) | IDLE with a timeout, the `Guarded` stream that keeps a read timeout during IDLE | The part most different between the two crates. 5 tests on a local TCP server. |
| `oauth.rs` (493 lines) | `Authenticator` for XOAUTH2 (Gmail sign-in) | Both crates have the same kind of `Authenticator` trait. 7 tests. |
| `snooze_sync.rs` (1,177 lines) | list, create, rename, subscribe, select, search, fetch, store, move, append, expunge | Straight API calls, no special streams. 17 tests. |

### With `imap` 3.0 alpha

- `imap::connect` is replaced by `ClientBuilder`. We already open our own sockets, so check that a `Client` can still be built on a stream we provide (needed for our timeouts and for STARTTLS).
- `Flag`, `NameAttribute` and `Error` are now `#[non_exhaustive]`, so each `match` on them needs a fallback arm.
- Fetch results come back as a different type (`Fetches`), so the loops over `uid_fetch` change shape but not meaning.
- IDLE keeps the same idea (`idle()` then wait with a timeout); the timeout plumbing in `Guarded` needs checking against the new API.
- Mostly mechanical. The tests that drive a fake server should keep working with small changes.

### With `async-imap`

- Add Tokio (and `tokio-native-tls`, so TLS stays on the system's libraries as now) to `nightpost-core`.
- Keep the `engine` functions blocking from the outside: each one runs its async body on a small runtime owned by the core (`block_on`). The worker threads, the MCP server and the iPhone bridge then don't change. No async reaches the UI.
- Timeouts move from socket settings to `tokio::time::timeout` around each step, which is simpler and also covers a server that sends a trickle of bytes.
- IDLE becomes `idle().init()`, then `wait_with_timeout`, which hands back a way to interrupt it. That could let the app stop an IDLE at once on sleep or quit, instead of waiting for the read timeout.
- Fetch and list results arrive as streams, so each loop gets `while let Some(x) = stream.next().await`.
- The scripted-server tests need an async fake stream (or move to the local TCP server that `idle.rs` already uses).
- Tokio builds for iOS, but the iPhone app gets somewhat bigger.

## What I'd suggest

**`async-imap`.** It's the maintained line, and the reason for this work is to stop depending on code nobody is fixing. Swapping one stalled crate (2.4.1) for another possibly stalled one (3.0 alpha) risks doing all this again. Keeping the `engine` functions blocking from the outside keeps the change inside `nightpost-core`.

If a smaller step matters more right now, `imap` 3.0 alpha is the cheaper port, pinned exactly, with a note in Known issues to revisit once it's released or clearly abandoned.

## How to do it safely

1. One branch, no other changes in it. `nightpost-core` only.
2. Port `oauth.rs` and the connection set-up first, then `snooze_sync.rs`, then `engine.rs`, then `idle.rs`, running the tests after each.
3. `cargo test --workspace`, clippy, `scripts/audit.sh` (the future-incompatibility count should drop to 0), the Linux build, and `scripts/ios-rust.sh`.
4. Tim tests by hand on real accounts before it goes on the machines he relies on: first sync, push (new mail arriving, mail read on the phone), archive, move, labels, snooze and pin sync, drafts, Sign in with Google, and a dropped connection (Wi-Fi off and on, laptop sleep).
5. Run the new build next to an older one for a few days (Mac and the MCP server, say), since the sync state is shared.

This needs a machine with Rust and the network, where `cargo test` runs: Tim's Mac (Claude Code there), not the cloud session.
