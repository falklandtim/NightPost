# OwlPost for iPhone: plan for later

Status: **not started**. These notes were written on 2026-11-07 so the thinking isn't lost. Settle the Mac app first: the HTML view and Google sign-in should be proven on real mail, since the iPhone app would reuse both.

## What carries over

`owlpost-core` is plain Rust and should build for iOS (`aarch64-apple-ios`) with small changes.

| Part | iOS |
|---|---|
| IMAP sync (`engine.rs`), SMTP (`smtp.rs`) | Reusable as is |
| SQLite store (`store.rs`, bundled rusqlite) | Reusable; the database goes in the app's container |
| Parsing, threading, snooze, theme palettes | Reusable |
| Keychain (`credentials.rs`, `keyring` with `apple-native`) | Should work; check the access group and entitlements |
| Google OAuth token logic (`oauth.rs`) | Reusable. The browser step must use `ASWebAuthenticationSession` with a custom URL scheme, not a loopback web server |
| Data paths (`directories`) | Replace them with paths the app passes in |
| IMAP IDLE (`idle.rs`) | Only while the app is open (see Push below) |
| MCP server (`mcp.rs`) | Not on iOS. Put it behind a cargo feature |

## What doesn't carry over: the interface

The egui app (`crates/owlpost`) does run on iOS technically, but it would feel wrong there: no native scrolling or swipe actions, and poor keyboard, text selection and IME handling. Build instead:

- a **SwiftUI app** on top of the Rust core;
- **UniFFI** bindings for the core: a new `crates/owlpost-ffi` crate that wraps `Store` and the engine and returns plain records (threads, messages, folders);
- **WKWebView** for HTML email, reusing the sandboxing and tracker blocking from `mail_view.rs`. The page builder (`document()`, `strip_trackers()`, `csp()`) would move into the core or the FFI crate so both apps share it;
- the **Moonlit** palette from `crates/owlpost-core/themes/owlpost-moonlit-*`, keeping the coloured avatars.

## Push mail: the hard part

iOS doesn't let an app keep an IMAP connection open in the background, so the Mac app's IDLE push can't work on the phone. Options:

1. **Background refresh** (`BGAppRefreshTask`). iOS decides when the app wakes, often only every 15 minutes to a few hours. Simple, needs no server, but there are no instant notifications. **Start here.**
2. **A small push server.** It holds IDLE connections and sends APNs notifications, which is how Canary and Spark do it. It means running a server that has your mail credentials, so treat it as a separate decision with its own security review.

## Distribution

- **Your own phone:** an Apple Developer account ($99 USD/year) and Xcode, through TestFlight or direct install.
- **Gmail / Workspace:** the existing *Internal* OAuth client for timson.co avoids Google verification. It needs an iOS client ID added in Google Cloud.
- **Public App Store release with Gmail:** the restricted Gmail scopes need Google's security assessment (CASA), which costs money and takes time. Only do this if the app is going public.

## Suggested order

1. **Prepare the core.** Feature-gate `mcp` and the loopback OAuth flow, let the caller pass in data paths, and check that `cargo build --target aarch64-apple-ios -p owlpost-core` succeeds.
2. **Add `crates/owlpost-ffi`** with UniFFI and generate the Swift bindings.
3. **Build the SwiftUI app:** account setup (Google sign-in and app passwords), unified inbox, thread view (WKWebView), compose, swipe to archive or snooze, background refresh.
4. **Later:** a push server, if instant notifications turn out to matter.

## Open questions

- Is it only for Tim's phone, or might it be published? This decides the Google verification and push-server questions.
- How much do instant new-mail notifications matter?
- Is Xcode installed and an Apple Developer account active?
