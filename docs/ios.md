# OwlPost for iPhone

Status: **first version builds; Sign in with Google added 2026-11-14.** The core builds for iPhone, the Swift bridge works, and a SwiftUI app with the first screens builds for the simulator and for a phone. It hasn't run on a phone yet. First plan written 2026-11-07; updated 2026-11-11.

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
ios/google-client-id.txt  the iOS OAuth client ID (not committed)
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
   - `setRead`, `setStarred`, `setPinned`, `archive`, `trash`, `snooze(until)`, `unsnooze`;
   - `folders(accountId)`: an account's folders as the desktop sidebar lists them (standard first, then its own as a tree, with depth and parent), and `folderThreads(folderId, query, limit)`: one folder's conversations;
   - reading: `messagePage(messageId, remoteImages)`, an HTML email's page (or nothing for plain text); `inlineImage(messageId, cid)`, a picture sent inside an email; `attachmentFile(messageId, index)`, an attachment as a file;
   - writing: `startCompose(messageId, kind)` (reply, reply all, forward with its attachments), `openDraft`, `saveDraft`, `deleteDraft`, `send`, and `suggestContacts(prefix)`; a `Draft` carries the editor's HTML, the addresses as typed and attachments as files;
   - filing: `inboxTabs(folderId?)` with unread counts, `filing(accountId, threadKey)` (folders, labels, tab and sender for the Move sheet), `moveToFolder`, `setLabel`, `createFolder`, `putInTab`, `sortSender`, `newTab`, `removeTab`;
   - `startGoogleSignIn(clientId, loginHint)` and `finishGoogleSignIn(signIn, redirect)` (see *Sign in with Google* below).
   
   Errors come to Swift as `MailError.Failed(message)`, in words to show. Tested in Rust against the demo accounts, and from a Swift program on the Mac (open, list, read, snooze, an error).
5. **The SwiftUI app, first screens:**
   - a welcome screen: add an account (app password, with where to make one; a Provider picker for your own domain) or try the demo;
   - Settings (☰ menu): accounts (add, or swipe to remove from the phone), appearance (Match iPhone, Light, Dark), leave the demo, version;
   - the list across all accounts: unread dot, pin, star, attachment and count badges, account colour, pinned rows tinted; swipe right to archive or mark read, left to snooze, trash or pin; pull to sync; search with the desktop's words; a menu for the other lists (Snoozed, Starred, Drafts, Sent, Archive, Trash);
   - a conversation as cards, oldest first, in plain text, older read ones folded; archive, snooze, star, pin and trash at the bottom; opening it marks it read;
   - snooze: later today, this evening, tomorrow, this weekend, next week, or a date and time.
   
   `MailModel.swift` runs every core call off the main thread and updates the list at once, then reloads after the server answers.

6. **Sign in with Google** (2026-11-14): a button on the welcome screen and at the top of Add Account. It opens Google's page in Apple's sign-in sheet (`ASWebAuthenticationSession`), which closes when Google returns to the app through the client ID reversed (`com.googleusercontent.apps.…`). The core checks the link belongs to this sign-in, redeems the code, saves the refresh token and the client ID in the phone's keychain, and adds the account; an account already here with an app password switches to Google sign-in. Removing an account also forgets its Google sign-in. Tested in Rust up to the call to Google; the real sign-in is for Tim to try.

7. **App icon** (2026-11-14): the desktop's owl, filled square for iOS (which rounds the corners itself), in `ios/OwlPost/Assets.xcassets`. **The list fills in during a sync**, every 3 seconds, so a long first sync doesn't show an empty Inbox until the end.

8. **Folders** (☰ → Folders): each account's own folders and Gmail labels, standard ones first, then the account's own as a tree; a parent folds its subfolders away and, folded, counts their unread mail too. Tapping one shows its conversations in the main list, with the same swipes; picking a list from the ☰ menu goes back to the unified lists. OwlPost's hidden folders are left out, as on the desktop.

9. **HTML email** (2026-11-14): each email's HTML in a web view inside its card, built by the same core code as the desktop's page (`owlpost_core::mail_html`): tracking pixels stripped and named, a strict Content Security Policy, remote images hidden until **Load images**. The web view runs none of the email's JavaScript. Wide newsletters are scaled to fit. Pictures sent inside an email (`cid:`) are fetched through the core. Web links open in Safari; `mailto:` links open the composer. Attachments open in Quick Look (share or save to Files from there). Emails show on white paper, in dark mode too.
10. **Writing** (2026-11-14): the ✎ button (new), the reply arrow in a conversation (tap for Reply; hold for Reply All and Forward), a draft in Drafts, or a `mailto:` link. Native From, To, Cc, Bcc and Subject fields, with suggestions from mail you've had and the iPhone's contacts (iOS asks first). The body is a formatted editor: bold, italic, underline, lists and links. 📎 attaches from Photos (sent as JPEG) or Files; a forward keeps the original's attachments (the desktop doesn't yet). Cancel asks whether to save the draft to the server's Drafts folder, where the Mac sees it. Sending goes through the core's SMTP and files a copy in Sent. Over 25 MB is refused before sending.

11. **Filing** (2026-11-15): press and hold a conversation in the list, or the folder button in a conversation, for the Move sheet: move it to a folder (searchable, or a new one), add or take off Gmail labels, put it in an Inbox tab, and choose which tab its sender's mail always goes to. Above the Inbox, a row of tabs (All, Primary, Promotions, Social and your own, each with its unread count) filters the list; a new tab can be made from the Move sheet, and pressing and holding your own tab removes it. Tabs and sorting rules sync with the computers through "OwlPost Sync". Pinned conversations sit in their own section, which can be hidden. Seen in the simulator with the demo accounts only.

## Sign in with Google

The phone uses its own OAuth client, an **iOS** client in the same Google Cloud project as the desktop's (so the same name, *Internal* audience and Gmail permission). A Desktop client can't be used: Google only returns a Desktop client's sign-in to a local web server, and it has a secret.

To set it up (done for timson.co on 2026-11-14):

1. At [console.cloud.google.com/auth/clients](https://console.cloud.google.com/auth/clients), in the OwlPost project: **Create client → iOS**, name "OwlPost iPhone", Bundle ID `co.timson.OwlPost`. Leave App Store ID and Team ID empty.
2. Put the Client ID in `ios/google-client-id.txt` (one line; not committed).
3. Run `scripts/ios-rust.sh`. It writes `ios/OwlPost/Generated/GoogleClient.swift` from that file. Without the file the button is hidden and accounts use app passwords.

## Building and installing

1. `scripts/ios-rust.sh` builds the core for the phone and the simulator and makes the Swift bindings. Run it again after changing `owlpost-core` or `owlpost-ffi`.
2. Open `ios/OwlPost.xcodeproj` in Xcode.
3. **On the phone (once):** Xcode > Settings > Accounts, add your Apple ID. In the project's *Signing & Capabilities*, pick your *Personal Team*. Connect the iPhone, turn on Developer Mode (Settings > Privacy & Security), and press Run. The first time, trust the developer in Settings > General > VPN & Device Management. With a free Apple ID the app stops opening after 7 days: press Run again to renew it.
4. **In the simulator** (it runs inside Xcode 27; there's no separate Simulator app): pick an iPhone at the top of the window and press ⌘R. *Try the Demo* opens the made-up accounts; *Add an Account* works with real ones. With a mouse or trackpad: click and drag a row sideways to swipe, drag the list down to sync. Light or dark: ☰ → Settings → Appearance (⇧⌘A doesn't switch the embedded simulator).
5. **After changing Rust code**, run `scripts/ios-rust.sh` again before ⌘R. Swift changes only need ⌘R.

Debug launch options (*Edit Scheme > Run > Arguments*, or `xcrun simctl launch <device> co.timson.OwlPost …`): `-demo` starts in the demo accounts; `-show thread|snooze|settings|add|folders|compose|new|move|html` opens that screen once the list loads, for screenshots. `-churn` reloads the list every second, as a long sync does. `-syncing` shows the sync spinner with a folder count.

UI tests (`ios/OwlPostUITests`, demo accounts only) tap and type in the composer, and file conversations from the Move sheet and tabs, as a finger would: `xcodebuild test -project ios/OwlPost.xcodeproj -scheme OwlPostUITests -destination 'id=<simulator>'`, or ⌘U in Xcode with that scheme.

## Adding your accounts

**Gmail and Google Workspace (timson.co):** tap **Sign in with Google** (on the welcome screen, or ☰ → Settings → Add Account), pick your account in Google's sheet and allow access. The first time, iOS asks whether OwlPost may use google.com to sign in: tap Continue. The sign-in is kept in that iPhone's (or simulator's) own keychain. To revoke it, remove the account on the phone, or at myaccount.google.com → Security → Third-party apps (that signs out every device using OwlPost).

Other accounts sign in with an **app-specific password**. The password is checked with the server before it's saved, and kept in that iPhone's (or simulator's) own keychain, not the Mac's. Revoking it doesn't affect the Mac app.

**Google with an app password instead** (only if Sign in with Google isn't set up):

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
2. **Filing on real accounts** (Tim, by hand): a move, a Gmail label, a new tab and a sender rule on the phone, then check each shows on the Mac.
3. **Reading and writing on real accounts** (Tim, by hand): HTML email, pictures inside emails, attachments, replies, drafts showing on the Mac, and sending.
4. **Sign in with Google on a real account** (Tim, by hand): built 2026-11-14, not yet tried with a real Google account.
5. **Background refresh** (`BGAppRefreshTask`): iOS wakes the app every 15 minutes to a few hours to call `sync`.
6. **Undo** after archive or snooze, as on the desktop (⌘Z there; a toast with Undo here).
7. **Composer extras**: signatures (they live on the Mac for now), pictures pasted into the body, autosaving while you write, Undo Send.

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
