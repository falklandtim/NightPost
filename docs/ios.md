# OwlPost for iPhone

Status: **first version builds.** The core builds for iPhone, the Swift bridge works, and a SwiftUI app with the first screens builds for the simulator and for a phone. It hasn't run on a phone yet. First plan written 2026-11-07; updated 2026-11-11.

## Decisions

- **Just for Tim's phone, for now.** So no App Store review and no Google security assessment (CASA). Google sign-in uses the existing *Internal* OAuth client setup for timson.co, with an iOS client added.
- **No Apple Developer account yet.** A free Apple ID in Xcode can install the app on Tim's own iPhone; it stops opening after 7 days and needs reinstalling from Xcode. The $99/year account removes that and adds TestFlight. Decide later.
- **No instant notifications.** Tim keeps mail notifications off, so background refresh is enough and no push server is needed.
- **Same repo.** The phone app shares `owlpost-core` with the desktop, so a fix to sync or the sync state reaches both in one commit:

```
crates/owlpost-core/   shared by every app
crates/owlpost-ffi/    the bridge: Rust → Swift, through UniFFI
ios/OwlPost.xcodeproj  the Xcode project (hand-written; folders sync, so new .swift files need no project change)
ios/OwlPost/           the SwiftUI code
ios/OwlPost/Generated/ Swift bindings     } made by scripts/ios-rust.sh,
ios/build/             OwlPostCore.xcframework } not committed
```

## Done

1. **The core builds for iPhone** and the simulator (Apple silicon), with no code changes, for iOS 17 and later.
2. **The app chooses its folders:** `owlpost_core::use_folders(data, cache)`, set once at start-up (the iPhone sandbox decides where files go).
3. **Google sign-in without a local web server:** `oauth::authorize` with `oauth::ios_redirect(client_id)` builds the sign-in link; the app opens it in `ASWebAuthenticationSession`, then `Authorize::code_from_redirect` checks the link that comes back and `Authorize::exchange` redeems it. iOS clients have no secret, so none is sent. The desktop's loopback sign-in is built on the same pieces and works as before.
4. **The bridge, `crates/owlpost-ffi`:** one `Mail` object, every call blocking (Swift calls it off the main thread):
   - `open(dataDir, cacheDir)`, or `openDemo(path)` for previews and tests;
   - `accounts()`, `addPasswordAccount(name, email, password, provider)` (app passwords; `provider` is `detect` for gmail.com, icloud.com and the like, or picked for your own domain, e.g. `google` for Workspace), `removeAccount(id)`;
   - `threads(mailbox, query, limit)` for Inbox, Starred, Drafts, Sent, Archive, Trash and Snoozed across all accounts, with the desktop's search words;
   - `messages(accountId, threadKey)`: a conversation, oldest first, plain text;
   - `sync(accountId?)`: new mail, plus snoozes, pins and tabs from the server;
   - `setRead`, `setStarred`, `setPinned`, `archive`, `trash`, `snooze(until)`, `unsnooze`.
   
   Errors come to Swift as `MailError.Failed(message)`, in words to show. Tested in Rust against the demo accounts, and from a Swift program on the Mac (open, list, read, snooze, an error).
5. **The SwiftUI app, first screens:**
   - a welcome screen: add an account (app password, with where to make one; a Provider picker for your own domain) or try the demo;
   - Settings (☰ menu): accounts (add, or swipe to remove from the phone), appearance (Match iPhone, Light, Dark), leave the demo, version;
   - the list across all accounts: unread dot, pin, star, attachment and count badges, account colour, pinned rows tinted; swipe right to archive or mark read, left to snooze, trash or pin; pull to sync; search with the desktop's words; a menu for the other lists (Snoozed, Starred, Drafts, Sent, Archive, Trash);
   - a conversation as cards, oldest first, in plain text, older read ones folded; archive, snooze, star, pin and trash at the bottom; opening it marks it read;
   - snooze: later today, this evening, tomorrow, this weekend, next week, or a date and time.
   
   `MailModel.swift` runs every core call off the main thread and updates the list at once, then reloads after the server answers.

## Building and installing

1. `scripts/ios-rust.sh` builds the core for the phone and the simulator and makes the Swift bindings. Run it again after changing `owlpost-core` or `owlpost-ffi`.
2. Open `ios/OwlPost.xcodeproj` in Xcode.
3. **On the phone (once):** Xcode > Settings > Accounts, add your Apple ID. In the project's *Signing & Capabilities*, pick your *Personal Team*. Connect the iPhone, turn on Developer Mode (Settings > Privacy & Security), and press Run. The first time, trust the developer in Settings > General > VPN & Device Management. With a free Apple ID the app stops opening after 7 days: press Run again to renew it.
4. **In the simulator** (it runs inside Xcode 27; there's no separate Simulator app): pick an iPhone at the top of the window and press ⌘R. *Try the Demo* opens the made-up accounts; *Add an Account* works with real ones. With a mouse or trackpad: click and drag a row sideways to swipe, drag the list down to sync. Light or dark: ☰ → Settings → Appearance (⇧⌘A doesn't switch the embedded simulator).
5. **After changing Rust code**, run `scripts/ios-rust.sh` again before ⌘R. Swift changes only need ⌘R.

Debug launch options (*Edit Scheme > Run > Arguments*, or `xcrun simctl launch <device> co.timson.OwlPost …`): `-demo` starts in the demo accounts; `-show thread|snooze|settings|add` opens that screen once the list loads, for screenshots.

## Adding your accounts

On the phone, accounts sign in with an **app-specific password** for now (Sign in with Google comes later). The password is checked with the server before it's saved, and kept in that iPhone's (or simulator's) own keychain, not the Mac's. Revoking it doesn't affect the Mac app.

**Google Workspace (timson.co):**

1. Signed in as your timson.co address, go to **myaccount.google.com/apppasswords**, create one called "OwlPost iPhone", and copy the 16 letters. It needs 2-Step Verification on for that account. If the page says app passwords aren't available, check 2-Step Verification is on; as the Workspace admin you can allow it in the Admin console under Security → Authentication → 2-Step Verification. IMAP is already on (the Mac app uses it).
2. In OwlPost: ☰ → **Settings** → **Add Account** (or *Add an Account* on the welcome screen).
3. Your name, your timson.co address, **Provider: Google (Gmail, Workspace)**, then the app password, and **Add**. On *Automatic* the Add button stays grey: your own domain doesn't say who runs its mail.

**Gmail (gmail.com):** the same app-password page; Provider can stay on *Automatic*.

**iCloud:** make an app-specific password at **account.apple.com** → Sign-In and Security → App-Specific Passwords. Provider can stay on *Automatic* for icloud.com, me.com and mac.com; for an iCloud custom domain pick *iCloud*.

To remove an account from the phone: Settings → swipe left on it. Its mail and password leave the phone; nothing changes on the server.

The phone keeps its own copy of your mail and its own passwords, in its keychain. It agrees with the computers through the server: snoozes, pins and tabs via "OwlPost Sync", everything else as any mail app does.

## The MCP server

Not on the phone. It still compiles there, but the bridge never calls it, so the linker leaves it out. No feature flag is needed.

## Next

1. **Run it on Tim's phone** with his real accounts (Tim, by hand): adding iCloud, sync, archive and snooze, and that a snooze or pin made on the phone shows on the Mac.
2. **HTML email**: move the page builder (`document()`, `strip_trackers()`, `csp()`) from `crates/owlpost/src/mail_view.rs` into the core so the phone's WKWebView gets the same sandbox and tracker blocking. Then attachments.
3. **Google sign-in on the phone**: add an iOS OAuth client in Google Cloud (Tim), register its reversed client ID as a URL scheme, and add a bridge call around `authorize` / `exchange`. Until then, Gmail can use an app password.
4. **Background refresh** (`BGAppRefreshTask`): iOS wakes the app every 15 minutes to a few hours to call `sync`.
5. **Undo** after archive or snooze, as on the desktop (⌘Z there; a toast with Undo here).
6. **Compose and reply**, saving drafts and sending through the core's SMTP.

## What carries over from the desktop

| Part | iOS |
|---|---|
| IMAP sync (`engine.rs`), SMTP (`smtp.rs`) | As is |
| SQLite store (`store/`, bundled SQLite) | As is; the database is in the app's sandbox |
| Parsing, threading, snooze and its server sync, pins, tabs | As is |
| Keychain (`credentials.rs`, `keyring` with `apple-native`) | Should work; check the access group and entitlements once it runs on the phone |
| Google OAuth (`oauth.rs`) | Yes, through `authorize` / `exchange` (see above) |
| IMAP IDLE (`idle.rs`) | Only while the app is open |
| The egui interface (`crates/owlpost`) | No: it would feel wrong on a phone (scrolling, swipes, keyboard, text selection). SwiftUI instead, with the Moonlit palette and coloured avatars |
