# OwlPost 🌱

A fast, native email client written in Rust, inspired by [Mailspring](https://github.com/foundry376/mailspring).
It has one binary with no Electron, a local SQLite cache, a built-in MCP server, and it can use Mailspring themes.

## Features

- **Gmail, iCloud and any IMAP/SMTP account.** Presets for Gmail, iCloud, Outlook, Yahoo and Fastmail; passwords are kept in the macOS Keychain (or the Windows/Linux equivalent).
- **Unified Inbox** plus per-account folders, conversation threading (References/In-Reply-To), and Gmail-style search (`from:`, `to:`, `subject:`, `is:unread`, `is:starred`, `is:pinned`, `has:attachment`).
- **Snooze**: *Later today, Tonight, Tomorrow, This weekend, Next week, Next month*, **Pick a date & time** (a calendar with an hour and minute, plus 9:00, 13:00 and 18:00 shortcuts), or type a time like `3h`, `friday` or `2026-12-01 09:00`. Snoozed threads leave every mailbox and come back to the inbox unread when they wake. Snoozes are kept on the mail server, so every Mac running OwlPost agrees and your phone's mail app doesn't show snoozed mail in the inbox (see [Snooze sync](#snooze-sync)).
- **Pin to top** (like Canary): press `p`, click the pin in the toolbar, or right-click a conversation. Pinned conversations stay above everything else in every list they appear in. Pins are stored locally.
- **Swipe actions**: swipe a conversation sideways with two fingers on a trackpad, or drag it with the mouse. By default swiping right archives and swiping left trashes. Each direction can be set to archive, trash, snooze, pin or mark read/unread in Preferences → Message list.
- **Select several conversations**: ⌘-click adds or removes one, Shift-click selects a range, Shift ↑ ↓ extends the selection and ⌘A selects the whole list. Archive, Trash, Spam, Move, Label, Snooze, Pin, Star and read/unread then apply to all of them, from the toolbar, the keyboard or the summary in the reading pane, and one ⌘Z undoes the lot. Move and Label need the conversations to be in one account, since folders belong to an account. Esc clears the selection.
- **Undo Send**: after you press Send, the message waits 10 seconds with **Undo** and **Send now** at the bottom of the window. Undo (or ⌘Z) reopens it in its window. Set the delay, or turn it off, in Preferences → Message list. Quitting during the wait sends the message first; if that fails, it's saved to Drafts.
- **Drag to a folder**: drag a conversation (or a selection of them) up or down to pick it up, then drop it on a folder in the sidebar. **All Accounts → Inbox, Archive, Spam or Trash** works for any account; an account's own folders take its conversations. A sideways drag is still a swipe, unless it carries on into the sidebar.
- **Show original**: **More (⋯) → Show original** opens the newest message's raw source in its own window, to copy or save as a `.eml` file.
- **Folder tree**: folders inside other folders fold away under their parent. Click the arrow beside a parent to show or hide its subfolders. A folded parent's count includes the unread mail inside it, and OwlPost remembers which are open, as well as which accounts are collapsed. A dot in a name (`timson.co`) is part of the name; only servers that keep folders under `INBOX.` use dots between levels.
- **Quick actions on hover**: widen the message list (at least 420 points) and hovering over a conversation shows mark read/unread, archive, trash, snooze and pin buttons, as in Mailspring.
- **Compose in its own window**: new messages, replies and forwards each open a separate window. What you write is saved to Drafts by itself, 3 seconds after you stop typing (⌘S saves at once); the footer shows when it was last saved. Closing the window keeps the message as a draft, and **Discard** deletes it.
- **Drafts**: drafts are saved in the account's Drafts folder on the server, so your phone and other mail apps see them too. A conversation that holds one of your drafts shows **Edit draft**, which opens it with its recipients (Bcc included), attachments and signature. Sending it removes the draft. **Discard** on a reopened draft deletes it, and closing without changes leaves it as it was. Drafts written by an assistant through the MCP `save_draft` tool work the same way.
- **Attachments**: click **Attach** in the composer or drag files onto its window. Each file shows as a chip with a thumbnail (for photos) and its size; × removes it. When you attach JPEG or PNG photos, an **Image size** menu offers Actual Size, Large (2048 px), Medium (1280 px) and Small (640 px), measured on the longest side. Photos are resized in the background and turned upright from their camera rotation, and the chip shows the new size next to the original. The composer shows the total message size and blocks Send above 25 MB, the limit at Gmail and iCloud. The MCP `send_email` tool takes an `attachments` list of file paths.
- **Push mail (IMAP IDLE)**: each account keeps a connection open on its Inbox, and the server reports new mail, or messages read or deleted elsewhere, as it happens. OwlPost then syncs just the Inbox, usually within a couple of seconds. Other folders sync every 5 minutes. The connection is re-issued every 8 minutes to keep home routers from dropping it and to check it's still alive. It reconnects with backoff after failures and starts over after the Mac wakes from sleep. Preferences → Accounts shows each account's status: *instant (push)*, *reconnecting*, or *every 5 minutes* for STARTTLS accounts and servers without IDLE.
- **HTML email (experimental, macOS)**: emails are shown as designed, using the system web view (WebKit, through `wry`). The conversation is one page of message cards; older messages fold. Each email body sits in its own sandboxed frame, so its styles can't affect anything else, and a strict Content Security Policy blocks scripts and forms. As in Canary, pictures load straight away but **tracking pixels are always removed**: tiny 1×1 images, and images from the open-tracking addresses of Marketo, Mailchimp, SendGrid, HubSpot and others, which report when and where you opened an email. A note above the message names the blocked tracker (e.g. "Blocked a tracker from go.repco.co.nz"). To load no remote content until you click **Load images**, turn off **Load remote images** in Preferences → Appearance. Links open in your browser, and keyboard shortcuts still work while a message has focus. Turn it off in Preferences → Appearance. Linux (WebKitGTK) is planned; until then messages show as text.
- **Undo (⌘Z)**: Archive, Trash, Spam, Move, labels, snooze, pin, star and read/unread can be undone with ⌘Z or the **Undo** button on the confirmation. Moves are reversed on the server, and Gmail archives are restored without touching All Mail. Actions on the same conversation run in order, so an undo never overtakes the action it reverses.
- **Sender photos and logos**: people show their photo from your Contacts (macOS asks once for access). Companies show their verified brand logo (BIMI, published in their DNS), or else their website's icon. Otherwise you see the coloured letter. Logos are fetched only from the sender's own domain, never from a lookup service that would learn who emails you. They're cached for a month in `~/Library/Caches/co.timson.OwlPost/logos`. Addresses at mailbox providers (Gmail, iCloud, Outlook, Xtra…) never get a company logo. Turn this off in Preferences → Appearance.
- **Message list sections**: Canary-style headings group the list into Pinned (with **Hide**/**Show**), Today, Yesterday, Earlier this week, Last week and then by month.
- **Received attachments**: click an attachment in a message to open it in its usual app (Preview, Pages…). Right-click it for **Save to Downloads**, which never overwrites an existing file and shows the saved file in Finder, or **Save As…**. Sync keeps only each attachment's name and size; the file itself is fetched from the server the first time you open it. It's then cached in `~/Library/Caches/co.timson.OwlPost/attachments`, which macOS may clear at any time. The server keeps the real copy.
- **Move & label**: move a conversation to any folder (`v`, the folder button, or right-click → Move to), and add or remove Gmail labels (`l`, the tag button, or right-click → Label). Both pickers are searchable and can create a new folder or label. Labels show as removable chips on the open conversation. On Gmail, Archive and Move only take a conversation out of the Inbox, so its other labels stay.
- **Signatures**: Preferences → Signatures lets you create named signatures and choose a default for each account. Each signature has a **Plain text / HTML** toggle. HTML signatures appear as readable text while you write, and the message is sent with both a text and an HTML version, with your real HTML in the HTML version (the MCP `send_email` and `reply` tools do the same). HTML signatures are drawn in the composer and in Preferences, with bold and italic text, links, and images loaded from the web. Pasting a whole page copied from another app (such as Canary) is fine: on Save, OwlPost keeps only the signature and removes page styles, such as Canary's dark-mode colour inversion, which would otherwise restyle the whole email. Images load even when the host labels them with the wrong file type, as Dropbox share links do. The composer has a signature picker that swaps the signature in place, and changing the From account switches to that account's default.
- Archive, trash, spam, star, mark unread, reply, reply all and forward. To, Cc and Bcc suggest addresses as you type: people you've written to come first, then people who've written to you, then your Mac's Contacts. Use ↑ ↓ and Enter or Tab to pick one, or click it.
- A unified toolbar with capsule button groups (it sits in the title bar on macOS). There's also a filter menu with All, Unread, Starred, Pinned and With attachments, and Mark all as read.
- Keyboard shortcuts: `⌘W` closes a composer or Preferences, plus `c` `r` `a` `f` `e` `#` `!` `b` `p` `s` `u` `j/k` `/` `g i` (full list in Preferences → Shortcuts).
- **Mailspring themes** (see below).
- **Built-in MCP server** (see below).
- Demo mode with sample accounts, so you can try it without a mail server.

## Build & run

```sh
cargo run --release              # the app
cargo run --release -- --demo    # with sample accounts
cargo test --workspace           # unit + offscreen UI render tests
scripts/bundle-macos.sh          # build target/release/OwlPost.app (with icon)
scripts/bundle-macos.sh --install   # …and copy it to /Applications
scripts/make-signing-cert.sh     # once: a local signing certificate (see below)
```

The bundle script signs the app with "OwlPost Local Signing" when that certificate is in your login keychain. The Keychain recognises an app by its signature, so builds signed with it keep access to your saved passwords and Google sign-in. Without it, the app is signed ad hoc, which changes with every build, and macOS asks for Keychain access again after each install. The certificate is self-signed and trusted only for code signing on your Mac. The first time `codesign` uses it, macOS asks for your login password: choose **Always Allow**.

The app icon lives in `crates/owlpost/assets/icon-1024.png`. After changing it, run `cargo run -p owlpost --example icon_assets` to regenerate the padded macOS version. The bundle script also does this automatically when the source is newer.

## Connecting Gmail and iCloud

### Gmail and Google Workspace: Sign in with Google

OwlPost can sign in to Google accounts in your browser with OAuth 2.0, as Mailspring does. Your Google password never reaches OwlPost, and you can revoke access at any time at myaccount.google.com → Security → Third-party apps. The sign-in is stored as a refresh token in the Keychain; IMAP, push and SMTP use short-lived access tokens (XOAUTH2).

Google requires every app to have its own OAuth client, so you create one once (free, about 10 minutes):

1. At [console.cloud.google.com](https://console.cloud.google.com/auth/clients), create a project (e.g. "OwlPost").
2. **Google Auth Platform → Branding**: enter the app name and your email.
3. **Audience**:
   - **Google Workspace domain**: choose **Internal**. There's no review and no warning screen, and sign-in doesn't expire. It works only for accounts in your Workspace.
   - **Personal @gmail.com**: choose **External**, add yourself as a test user, then **Publish app**. While it's left in Testing, sign-in expires every 7 days. Google shows an "unverified app" warning when you sign in; click Advanced → Continue. Unverified apps are limited to 100 users.
4. **Data Access → Add or remove scopes**: add `https://mail.google.com/` by hand.
5. **Clients → Create client → Desktop app**, then paste the Client ID and Client secret into **Preferences → Accounts → Sign in with Google**. Google doesn't treat a desktop app's secret as confidential, but OwlPost keeps it in the Keychain anyway.

Then use **Sign in with Google…** to add an account, or **Use Google sign-in** on an existing Gmail account to switch it over (its app password is removed from the Keychain). The add-account window also offers it when you pick Gmail.

To share OwlPost with other people without each of them creating a client, the app would need Google's verification for the restricted `mail.google.com` scope, which includes a yearly security assessment.

### App passwords

Otherwise, both providers accept an **app password** in place of your normal password. iCloud only works this way: Apple has no sign-in that third-party mail apps can use.

| Provider | Create an app password | Servers (pre-filled) |
|---|---|---|
| Gmail | Turn on 2-Step Verification, then visit myaccount.google.com/apppasswords. Make sure IMAP is enabled in Gmail settings. | `imap.gmail.com:993` TLS · `smtp.gmail.com:465` TLS |
| iCloud | account.apple.com → Sign-In and Security → App-Specific Passwords. Use your full `@icloud.com` address. | `imap.mail.me.com:993` TLS · `smtp.mail.me.com:587` STARTTLS |

## MCP server

`owlpost mcp` speaks the Model Context Protocol over stdio and shares the app's database and accounts.

```sh
claude mcp add owlpost -- /path/to/owlpost mcp
```

Tools: `list_accounts`, `list_folders`, `list_threads`, `search`, `read_thread`, `view_message`, `get_attachment`,
`snooze_thread`, `unsnooze_thread`, `list_snoozed`, `pin_thread`, `mark_read`, `star_thread`,
`move_thread`, `label_thread`, `save_draft`, `send_email`, `reply`, `sync`. The sending tools are annotated as destructive, so MCP clients ask you to confirm before anything is sent.

- `view_message` shows an email the way it was designed. It returns the text with an `[image N]` marker where each picture sits, and then the pictures. Inline (`cid:`), remote and `data:` images are included. Tracking pixels are removed and never fetched, small icons are skipped, and pictures are sized to at most 1200 px. Remote pictures are fetched from the sender's servers, as when you open the email; pass `images: false` to skip them.
- `get_attachment` downloads an attachment (using the same cache as the app) and returns its path. Pictures come back to look at, and small text files (CSV, JSON, ICS…) come back inline. Other files, such as PDFs, are opened from the path.
- `save_draft` writes a draft (or, with `thread_id`, a reply draft) to the Drafts folder instead of sending it. You finish it with **Edit draft** in OwlPost or in any other mail app.

## Themes (Mailspring-compatible)

A Mailspring theme is a folder with a `package.json` (`"theme": "ui"`) and `styles/*.less` files.
OwlPost reads those files and evaluates the LESS variables, including `lighten`, `darken`, `fade`, `mix`, `tint`, `spin`, `contrast`, `rgba`, `hsl`, plus CSS `var()` and `color-mix()`, using LESS's lazy evaluation where the last definition wins. It then maps the results (`@accent-primary`, `@list-focused-bg`, `@panel-background-color`, …) onto the native UI.

- Themes installed in Mailspring are found automatically (`~/Library/Application Support/Mailspring/packages`).
- `owlpost themes install <dir>` copies a theme folder in, or use Preferences → Appearance.
- `owlpost themes inspect <dir>` prints how each colour role resolves.

The built-in Light theme declares the same variable names and derivations as Mailspring's base `ui-variables.less`, so a theme's overrides spread through OwlPost the same way they do in Mailspring. This has been tested against Mailspring's own Dark, Darkside, Taiga, Ubuntu and Less Is More themes.

**Limitation:** CSS rules that target Mailspring's HTML (e.g. `.thread-list .item { … }`) can't apply to a native UI. When a theme relies on one of those for a text colour, OwlPost picks the most readable colour from the theme's own palette.

## Layout

```
crates/owlpost-core   models, SQLite store, IMAP sync engine, SMTP, snooze, themes, MCP server
crates/owlpost        egui desktop app (sidebar / thread list / reader, composer, preferences)
```

Data lives in `~/Library/Application Support/co.timson.OwlPost/` (override with `--db` or `OWLPOST_DB`).

## Not yet implemented

HTML message rendering on Linux (shown as text there), inline (cid:) images in the app's HTML view (the MCP `view_message` tool does include them), automatic draft saving while you type, and send-later or read receipts.

## Snooze sync

Each account gets two extra folders (labels on Gmail):

- **OwlPost Snoozed** holds snoozed conversations until they wake. Other mail apps show the folder, so you can find snoozed mail there. Moving a conversation out of it elsewhere (on your phone, say) cancels the snooze.
- **OwlPost Sync** holds one small message, "OwlPost snoozes", listing each snooze by Message-ID with its wake time. Leave it alone. OwlPost hides both folders and never shows the message as mail.

Each change merges with what's on the server, and for any one conversation the newer change wins. Cancelled snoozes are remembered for 30 days so a Mac that was offline doesn't bring them back. Whichever OwlPost is running when a snooze is due moves it back to the Inbox and marks it unread. When none is running, it comes back the next time one starts. Snoozes made before syncing existed are uploaded on the first sync. Code: `crates/owlpost-core/src/snooze_sync.rs`.

## Plans

- iPhone app: see [docs/ios.md](docs/ios.md) (not started).

## Known issues

### `imap-proto` future-incompatibility warning (needs fixing)

Builds print `warning: the following packages contain code that will be rejected by a future version of Rust: imap-proto v0.10.2`. It comes from `imap` 2.4.1 (`crates/owlpost-core/Cargo.toml`), which depends on `imap-proto` 0.10.2. That crate's macros end with a trailing semicolon, which Rust is phasing out ([rust-lang/rust#79813](https://github.com/rust-lang/rust/issues/79813)). For now it's only a warning and doesn't change how OwlPost behaves. A future Rust release will make it a hard error, and the build will then fail.

Planned fix: copy `imap-proto` 0.10.2 into the repo with the trailing semicolons removed, point Cargo at it with `[patch.crates-io]`, and run the test suite. This changes no behaviour. Don't upgrade to `imap` 3.0 for this: it's an alpha with a different API, and moving to it means rewriting and re-testing sync, IDLE and Google sign-in.

See the details with `cargo report future-incompatibilities --package imap-proto@0.10.2`.
