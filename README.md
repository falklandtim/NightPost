# NightPost 🌱

A fast, native email client written in Rust, inspired by [Mailspring](https://github.com/foundry376/mailspring).
It has one binary with no Electron, a local SQLite cache and a built-in MCP server.

**New to NightPost? Read [the manual](docs/manual/README.md)**: how to install it, add your accounts and use it, on the Mac and on Linux. This README lists every feature in detail.

## Features

- **Gmail, iCloud and any IMAP/SMTP account.** Presets for Gmail, iCloud, Outlook, Yahoo and Fastmail; passwords are kept in the macOS Keychain (or the Windows/Linux equivalent).
- **Unified Inbox** plus per-account folders, conversation threading (References/In-Reply-To), and Gmail-style search (`from:`, `to:`, `subject:`, `is:unread`, `is:starred`, `is:pinned`, `has:attachment`). **All Accounts → Archive** shows archived mail from every account: an account's Archive folder, or for Gmail (which has none) the conversations in All Mail that aren't in the Inbox.
- **Snooze**: *Later today, Tonight, Tomorrow, This weekend, Next week, Next month*, **Pick a date & time** (a calendar with an hour and minute, plus 9:00, 13:00 and 18:00 shortcuts), or type a time like `3h`, `friday` or `2026-12-01 09:00`. Snoozed threads leave every mailbox and come back to the inbox unread when they wake. Snoozes are kept on the mail server, so every Mac running NightPost agrees and your phone's mail app doesn't show snoozed mail in the inbox (see [Snooze sync](#snooze-sync)).
- **Inbox tabs** (like Canary): **All** (the whole Inbox, where it opens), **Primary**, **Promotions** and **Social**, each with its unread count. NightPost sorts each email from its hidden headers: newsletters and marketing mail go to Promotions (bulk senders mark these so mail apps can offer one-click unsubscribing, and marketing platforms such as Klaviyo add their own headers), mail from social networks goes to Social, and replies and everything else go to Primary; Gmail accounts use Gmail's own tabs. Right-click a conversation for **Always put their mail in ▸** to sort a sender yourself (it wins over the automatic sorting, covers mail already here, and ⌘Z undoes it) and **All mail from …** to list everything from that address. Add your own tabs (say **Invoices**) from the same menu with **New tab…**: fill them with sender rules, or put one conversation in with **Put this conversation in ▸** (it beats the sender's rule, and replies stay with it). Right-click your tab to remove it; its mail goes back to the automatic tabs. On Gmail a conversation in one of your tabs also gets a label of the same name, so it's grouped on the phone too. Tabs and rules are kept on the server with the snooze list, so every NightPost agrees.
- **Pin to top** (like Canary): press `p`, click the pin in the toolbar, or right-click a conversation. Pinned conversations stay above everything else in every list they appear in, with a faint purple tint that deepens when there's unread mail in them. Pins are kept on the mail server with the snooze list, so every computer running NightPost shows the same ones (see [Snooze sync](#snooze-sync)).
- **Swipe actions**: swipe a conversation sideways with two fingers on a trackpad, or drag it with the mouse. A short swipe opens the row on its buttons (up to three a side): click one, or click anywhere else to close it. A full swipe does the first button straight away. By default, swiping left shows **Trash**, **Snooze** and **More** (the conversation's right-click menu), and a full swipe left moves it to Trash. Swiping right shows **Archive** and **Read/Unread**, and a full swipe right archives. Choose each side's buttons in Preferences → Message list, from Archive, Trash, Snooze, Move to…, More, Pin, Read/Unread, Star, Spam and Reply. Set a side's first button to Nothing to turn that side off.
- **Select several conversations**: ⌘-click adds or removes one, Shift-click selects a range, Shift ↑ ↓ extends the selection and ⌘A selects the whole list. Archive, Trash, Spam, Move, Label, Snooze, Pin, Star and read/unread then apply to all of them, from the toolbar, the keyboard or the summary in the reading pane, and one ⌘Z undoes the lot. Move and Label need the conversations to be in one account, since folders belong to an account. Esc clears the selection.
- **Undo Send**: after you press Send, the message waits 10 seconds with **Undo** and **Send now** at the bottom of the window. Undo (or ⌘Z) reopens it in its window. Set the delay, or turn it off, in Preferences → Message list. Quitting during the wait sends the message first; if that fails, it's saved to Drafts.
- **Your replies in the list**: a conversation's preview shows its newest message, your own replies included, even though they're kept in Sent rather than the Inbox. After you send, NightPost fetches Sent straight away, so the reply shows within a few seconds. The row keeps its place and the time the last email reached that folder, so replying doesn't move a conversation up the list. Drafts don't count as the newest message.
- **Drag to a folder**: drag a conversation (or a selection of them) up or down to pick it up, then drop it on a folder in the sidebar. **All Accounts → Inbox, Archive, Spam or Trash** works for any account; an account's own folders take its conversations. A sideways drag is still a swipe, unless it carries on into the sidebar. Drop a conversation on another account's Inbox or folder to **move it to that account**: NightPost copies it across and puts the originals in that account's Trash (this can't be undone with ⌘Z). Move to (`v`) lists the other accounts' main folders too.
- **Show original**: **More (⋯) → Show original** opens the newest message's raw source in its own window, to copy or save as a `.eml` file.
- **Folder tree**: folders inside other folders fold away under their parent. Click the arrow beside a parent to show or hide its subfolders, or the arrow (or name) beside an account to fold the whole account away. A folded parent's count includes the unread mail inside it, and NightPost remembers which are open, as well as which accounts are collapsed. A dot in a name (`timson.co`) is part of the name; only servers that keep folders under `INBOX.` use dots between levels.
- **Add and delete folders**: right-click an account's name for **New folder…**, or one of its folders for **New folder inside…** and **Delete folder…**. Deleting asks first. On Gmail the label goes and its mail stays in All Mail; on other servers the folder's mail is moved to Trash first. Inbox, Sent, Drafts, Trash and the other standard folders can't be deleted, and the menu says why.
- **Quick actions on hover**: widen the message list (at least 420 points) and hovering over a conversation shows mark read/unread, archive, trash, snooze and pin buttons, as in Mailspring.
- **Compose in its own window**: new messages, replies and forwards each open a separate window. What you write is saved to Drafts by itself, 3 seconds after you stop typing (⌘S saves at once); the footer shows when it was last saved. Closing the window keeps the message as a draft, and **Discard** deletes it.
- **Formatted writing**: the composer is a WebKit editor in its own window (WebKitGTK 6.0 in a GTK 4 window on Linux, native on Wayland and X11), with fonts and sizes, bold, italic, underline and strikethrough, text colour and highlight, links (⌘K), pictures in the text (insert, paste or drag them in), bulleted and numbered lists, quotes, alignment and Clear formatting. Spelling is checked as you type, using the system's dictionaries. Messages go out as HTML with a plain-text version, and pictures in the text are sent inside the email. Your signature sits under what you write and changes with the signature menu; replies quote the earlier message as text. Address suggestions, attachments with photo sizes, Undo Send and automatic draft saving work as before. Where no web view is available, the plain-text composer is used.
- **Drafts**: drafts are saved in the account's Drafts folder on the server, so your phone and other mail apps see them too. A conversation that holds one of your drafts shows **Edit draft**, which opens it with its recipients (Bcc included), attachments and signature. Sending it removes the draft. **Discard** on a reopened draft deletes it, and closing without changes leaves it as it was. Drafts written by an assistant through the MCP `save_draft` tool work the same way.
- **Attachments**: click **Attach** in the composer or drag files onto its window. Each file shows as a chip with a thumbnail (for photos) and its size; × removes it. When you attach JPEG or PNG photos, an **Image size** menu offers Actual Size, Large (2048 px), Medium (1280 px) and Small (640 px), measured on the longest side. Photos are resized in the background and turned upright from their camera rotation, and the chip shows the new size next to the original. The composer shows the total message size and blocks Send above 25 MB, the limit at Gmail and iCloud. The MCP `save_draft` tool takes an `attachments` list of file paths.
- **First sync**: each folder brings down its newest 300 emails, the folders you look at first (Inbox, Sent, Drafts, Archive, Gmail's All Mail), then Trash and Spam, then the rest. Accounts sync side by side, so one big account doesn't hold up the others. A big email (over 100 KB) comes down without its attachments: just its text, and its attachments' names and sizes; each attachment is fetched when you open it. Gmail shows one email in every label it has, so for each label NightPost first asks only for the emails' Message-IDs and reuses the copies it already has, downloading only mail it hasn't seen. A connection that drops partway (a laptop sleeping, a phone changing networks) is reopened and the folder tried again; one that goes quiet is given up on after a minute instead of holding up the sync.
- **Push mail (IMAP IDLE)**: each account keeps a connection open on its Inbox, and the server reports new mail, or messages read or deleted elsewhere, as it happens. NightPost then syncs just the Inbox, usually within a couple of seconds. Other folders sync every 5 minutes; each sync first asks the server, in one short question a folder, whether anything in it changed, and leaves unchanged folders alone (the Inbox is always checked, and every folder in full once a day, or hourly on servers without CONDSTORE). The connection is re-issued every 8 minutes to check it's still alive, and sends a small keepalive after each quiet minute so routers on the way don't forget it. A connection that dies anyway is noticed within about two minutes. To stay under Gmail's limit of 15 connections per account (shared by every computer and phone you use), NightPost runs at most three server actions per account at once; a burst of archives or deletes waits its turn instead of opening more. It reconnects with backoff after failures and starts over after the Mac wakes from sleep. Preferences → Accounts shows each account's status: *instant (push)*, *reconnecting*, or *every 5 minutes* for STARTTLS accounts and servers without IDLE.
- **Is each account connected?** A mark beside each account's name in the sidebar shows it: a small green dot when it's syncing, amber for a blip (one failed sync, or push reconnecting), and a red **!** when it can't connect (failed twice in a row, or the server turned down the password or Google sign-in). Hover for when it last synced and why it's failing, in plain words. The sidebar footer says "Synced 2 min ago", or names the account that can't connect. Click either, or press ⌘⇧L (Ctrl+Shift+L on Linux, or **More (⋯) → Sync activity**), for **Sync activity**: each account's state with **Sync now** (and **Sign in again…** when the sign-in was refused), then what happened, newest first: folders checked and new mail, push connecting and dropping, errors. When moving, deleting, archiving, labelling, starring, snoozing or marking read fails on the server, the red message also goes in here (and in the log file), saying which step the server turned down, so you can look at it after the message has gone. Filter by account or to problems only, **Copy** the lines to paste into a message, or **Show log file**.
- **Log file**: NightPost writes syncing, push and every warning to `~/Library/Logs/NightPost/nightpost.log` on a Mac (Console.app shows it too) and `~/.local/state/nightpost/nightpost.log` on Linux. At 2 MB it moves to `nightpost.log.1` and starts again. Lines from the MCP server are tagged `[mcp]`. Set `RUST_LOG=info` (or `debug`) to see more in the terminal as well.
- **HTML email (experimental, macOS and Linux)**: emails are shown as designed, using the system web view (WebKit: WKWebView on the Mac, WebKitGTK 6.0 on Linux). The conversation is one page of message cards; older messages fold. Each email body sits in its own sandboxed frame, so its styles can't affect anything else, and a strict Content Security Policy blocks scripts and forms. As in Canary, pictures load straight away but **known trackers are removed**: tiny 1×1 images, and images from the open-tracking addresses of Marketo, Mailchimp, SendGrid, HubSpot and others, which report when and where you opened an email. Any other remote picture can still tell the sender you opened the email; turn off **Load remote images** to stop that too. They're removed wherever the email loads them from: an ordinary picture, a CSS background (in the email's styles or on one element), the old `background=` attribute, `srcset`, a `<picture>`, a video poster, an SVG picture or a stylesheet. An email also can't redirect itself to another page, bring frames of its own, or include links that act on NightPost (such as one that turns on remote images), so nothing can slip past these checks. A note above the message names the blocked tracker (e.g. "Blocked a tracker from go.repco.co.nz"). To load no remote content until you click **Load images**, turn off **Load remote images** in Preferences → Appearance. With a dark theme, emails are **recoloured to match**: white and pale backgrounds take the theme's background, dark text turns light, and coloured banners, buttons and pictures keep their look (turn off **Match emails to a dark theme** to see them on white, as designed). Links open in your browser, and keyboard shortcuts still work while a message has focus. Turn it off in Preferences → Appearance. On Linux the same page is drawn by WebKitGTK 6.0.
- **Undo (⌘Z)**: Archive, Trash, Spam, Move, labels, snooze, pin, star and read/unread can be undone with ⌘Z or the **Undo** button on the confirmation. Moves are reversed on the server, and Gmail archives are restored without touching All Mail. Actions on the same conversation run in order, so an undo never overtakes the action it reverses.
- **Sender photos and logos**: people show their photo from your Contacts (macOS asks once for access). Companies show their verified brand logo (BIMI, published in their DNS), or else their website's icon. Otherwise you see the coloured letter. Logos are fetched only from the sender's own domain, never from a lookup service that would learn who emails you. They're cached for a month in `~/Library/Caches/co.timson.NightPost/logos`. Addresses at mailbox providers (Gmail, iCloud, Outlook, Xtra…) never get a company logo. Turn this off in Preferences → Appearance.
- **Message list sections**: Canary-style headings group the list into Pinned (with **Hide**/**Show**), Today, Yesterday, Earlier this week, Last week and then by month. Unread conversations have a light wash of the sidebar's highlight colour, and pinned ones a faint purple, deeper when they hold unread mail.
- **Received attachments**: click an attachment in a message to open it in its usual app (Preview, Pages…). Programs, scripts, installers, archives (`.zip`, `.dmg`…) and documents with macros ask first (**Open Anyway** or **Cancel**), since files from email can be harmful. On the Mac, every attachment NightPost saves is marked as downloaded from the internet, as a browser does, so macOS checks it (and anything unzipped from it) before it runs; Linux has no such mark, so the question before opening is its only guard. Right-click it for **Save to Downloads**, which never overwrites an existing file and shows the saved file in Finder, or **Save As…**. Sync keeps only each attachment's name and size (a big email doesn't even download the file); the file itself is fetched from the server the first time you open it. It's then cached in `~/Library/Caches/co.timson.NightPost/attachments`, which macOS may clear at any time. The server keeps the real copy.
- **Move & label**: move a conversation to any folder (`v`, the folder button, or right-click → Move to), and add or remove Gmail labels (`l`, the tag button, or right-click → Label). Both pickers are searchable and can create a new folder or label. Labels show as removable chips on the open conversation. On Gmail, Archive and Move only take a conversation out of the Inbox, so its other labels stay.
- **Signatures**: Preferences → Signatures lets you create named signatures and choose a default for each account. Each signature has a **Plain text / HTML** toggle. HTML signatures appear as readable text while you write, and the message is sent with both a text and an HTML version, with your real HTML in the HTML version (as do drafts saved with the MCP `save_draft` tool). HTML signatures are drawn in the composer and in Preferences, with bold and italic text, links, and images loaded from the web. Pasting a whole page copied from another app (such as Canary) is fine: on Save, NightPost keeps only the signature and removes page styles, such as Canary's dark-mode colour inversion, which would otherwise restyle the whole email. Images load even when the host labels them with the wrong file type, as Dropbox share links do. The composer has a signature picker that swaps the signature in place, and changing the From account switches to that account's default.
- Archive, trash, spam, star, mark unread, reply, reply all and forward. To, Cc and Bcc suggest addresses as you type: people you've written to come first, then people who've written to you, then your Mac's Contacts. Use ↑ ↓ and Enter or Tab to pick one, or click it.
- A unified toolbar with capsule button groups (it sits in the title bar on macOS). There's also a filter menu with All, Unread, Starred, Pinned and With attachments, and Mark all as read.
- Keyboard shortcuts: `⌘F` or `/` searches, `v` and `l` open Move and Label (type a folder's name, `↑` `↓` to choose, Return to pick), plus `c` `r` `a` `f` `e` `#` `!` `b` `p` `s` `u` `j/k` `g i` and `⌘W`. Press `?` for the full list (also in Preferences → Shortcuts).
- **Themes**: Moonlit Light and Moonlit Dark, or match the system (Preferences → Appearance).
- **Built-in MCP server** (see below).
- Demo mode with sample accounts, so you can try it without a mail server.

## Renamed from OwlPost

NightPost was called OwlPost (and SpringPost before that). The first time it runs, it carries everything over: the data folder (`co.timson.OwlPost` becomes `co.timson.NightPost`, and `owlpost.db` becomes `nightpost.db`), window settings, saved passwords and Google sign-ins (copied from the old Keychain entries, which are left for any OwlPost still running), `OWLPOST_…` environment variables, and the snooze folders on the server (see [Snooze sync](#snooze-sync)). The old `co.timson.OwlPost` cache folder is left behind and can be deleted. Because the app's identity changed, macOS asks once for access to each saved password: choose **Always Allow**. Re-register the MCP server under its new name (see below).

## Build & run

```sh
cargo run --release              # the app
cargo run --release -- --demo    # with sample accounts, added to your real database
cargo run --release -- --db /tmp/demo.db --demo   # sample accounts in a throwaway database
cargo test --workspace           # unit + offscreen UI render tests
scripts/bundle-macos.sh          # build target/release/NightPost.app (with icon)
scripts/bundle-macos.sh --install   # …and copy it to /Applications
scripts/make-signing-cert.sh     # once: a local signing certificate (see below)
```

The bundle script signs the app with "NightPost Local Signing" when that certificate is in your login keychain. The Keychain recognises an app by its signature, so builds signed with it keep access to your saved passwords and Google sign-in. Without it, the app is signed ad hoc, which changes with every build, and macOS asks for Keychain access again after each install. The certificate is self-signed and trusted only for code signing on your Mac. The first time `codesign` uses it, macOS asks for your login password: choose **Always Allow**.

### Linux

```sh
scripts/install-linux.sh   # build, then install to ~/.local (program, app-menu launcher, icon)
```

It needs Rust, plus the OpenSSL, D-Bus and WebKitGTK development files (`sudo dnf install openssl-devel dbus-devel pkgconf-pkg-config gtk4-devel webkitgtk6.0-devel libsoup3-devel` on Fedora, `sudo pacman -S --needed openssl dbus pkgconf gtk4 webkitgtk-6.0` on Arch). These are GTK 4 and WebKitGTK 6.0; the GTK 3 packages (`webkit2gtk4.1-devel`, `gtk3-devel`) aren't needed any more. Install a Hunspell dictionary (e.g. `hunspell-en`) for spell checking. Tested on Fedora 44 KDE (ARM). Passwords and Google sign-ins go in the desktop's Secret Service (KDE Wallet or GNOME Keyring).

**Or as a Flatpak**, which needs no Rust or development packages, and brings its own GTK 4 and WebKitGTK 6.0 from the GNOME runtime, the same on every distribution:

```sh
scripts/flatpak.sh            # build, then install for this user (flatpak run co.timson.NightPost)
scripts/flatpak.sh --bundle   # also write target/flatpak/nightpost-<version>-<arch>.flatpak
```

A `.flatpak` file from a GitHub release installs with `flatpak install --user nightpost-<version>-<arch>.flatpak`. The Flatpak keeps its files in `~/.var/app/co.timson.NightPost/` (the log file is in `.local/state/nightpost/` there). The first time it runs, it copies a copy installed with `scripts/install-linux.sh` across, once: its accounts, mail and settings. Quit that NightPost first, so the copy is taken while nothing is writing to it. Passwords stay in the keyring, which both use. After that the two keep separate databases, and each syncs with the server. The Flatpak can read only your Downloads, Documents and Desktop folders (Downloads to save attachments, all three for the MCP server's `save_draft`), plus the installed copy's folders for that one copy; Save As and Attach Files use the desktop's own file dialog. Its MCP server starts with `flatpak run co.timson.NightPost mcp`, which Preferences → MCP Server shows. When `Cargo.lock` changes, run `scripts/flatpak-cargo-sources.py` and commit `flatpak/cargo-sources.json`: the Flatpak builds without the network, from that list. See [docs/flatpak.md](docs/flatpak.md).

NightPost runs natively on Wayland (tested on KDE Plasma); `NIGHTPOST_X11=1` runs it through XWayland instead. HTML email on Linux is drawn off screen by WebKitGTK 6.0 (in a GTK 4 window that's never shown) and shown as a picture, with links, folds, Reply and attachments (including their right-click menu) clickable. Text in it can't be selected yet.

**Touchpad momentum (Linux):** a flick up or down on a touchpad carries on and slows to a stop, as on a Mac. Wayland leaves this to each app, so NightPost does it itself. Touch the touchpad again, click or press a key to stop it. Sideways swipes and mouse wheels don't coast. On Omarchy, which slows every touchpad to 0.4, NightPost scrolls faster with a Hyprland rule in `~/.config/hypr/input.lua`: `o.window("co\\.timson\\.NightPost", { scroll_touchpad = 1.5 })`, the speed Omarchy gives terminals.

**Blitz (experimental, Linux only):** Preferences → Appearance → Email engine → **Blitz** draws HTML email inside NightPost instead, with [Blitz](https://github.com/DioxusLabs/blitz) (a pre-release, pinned at 0.3.0-beta.4), so you can **select text and copy it** (drag, then Ctrl+C or right-click → Copy). Each message is a card, as with HTML turned off, and the email sits inside it; in a dark theme it's shown on white. Blitz runs no scripts. Pictures from the web follow the same rules as in WebKit: they load straight away with **Load remote images** on, or after you click **Load images** under the email, and known trackers are removed first. NightPost fetches them itself, in the background, never from this computer or your local network, at most 8 MB each, and refuses any picture larger than 25 million pixels. Pictures sent inside the email (`cid:`) aren't shown yet, as in WebKit. Very long emails are cut off near 20,000 points, with a note. Some older emails look plainer than in WebKit: Blitz ignores `<font>` colours and sizes and table `border="1"`, and draws emoji poorly. If Blitz can't draw an email, its text is shown instead. WebKit stays the default, and the Mac always uses its own WebKit, where text can already be selected.

The app icon lives in `crates/nightpost/assets/icon-1024.png`. After changing it, run `cargo run -p nightpost --example icon_assets` to regenerate the padded macOS version. The bundle script also does this automatically when the source is newer.

## Connecting Gmail and iCloud

### Gmail and Google Workspace: Sign in with Google

NightPost can sign in to Google accounts in your browser with OAuth 2.0, as Mailspring does. Your Google password never reaches NightPost, and you can revoke access at any time at myaccount.google.com → Security → Third-party apps. The sign-in is stored as a refresh token in the Keychain; IMAP, push and SMTP use short-lived access tokens (XOAUTH2).

Google requires every app to have its own OAuth client, so you create one once (free, about 10 minutes):

1. At [console.cloud.google.com](https://console.cloud.google.com/auth/clients), create a project (e.g. "NightPost").
2. **Google Auth Platform → Branding**: enter the app name and your email.
3. **Audience**:
   - **Google Workspace domain**: choose **Internal**. There's no review and no warning screen, and sign-in doesn't expire. It works only for accounts in your Workspace.
   - **Personal @gmail.com**: choose **External**, add yourself as a test user, then **Publish app**. While it's left in Testing, sign-in expires every 7 days. Google shows an "unverified app" warning when you sign in; click Advanced → Continue. Unverified apps are limited to 100 users.
4. **Data Access → Add or remove scopes**: add `https://mail.google.com/` by hand.
5. **Clients → Create client → Desktop app**, then paste the Client ID and Client secret into **Preferences → Accounts → Sign in with Google**. Google doesn't treat a desktop app's secret as confidential, but NightPost keeps it in the Keychain anyway.

Then use **Sign in with Google…** to add an account, or **Use Google sign-in** on an existing Gmail account to switch it over (its app password is removed from the Keychain). The add-account window also offers it when you pick Gmail.

To share NightPost with other people without each of them creating a client, the app would need Google's verification for the restricted `mail.google.com` scope, which includes a yearly security assessment.

### App passwords

Otherwise, both providers accept an **app password** in place of your normal password. iCloud only works this way: Apple has no sign-in that third-party mail apps can use.

| Provider | Create an app password | Servers (pre-filled) |
|---|---|---|
| Gmail | Turn on 2-Step Verification, then visit myaccount.google.com/apppasswords. Make sure IMAP is enabled in Gmail settings. | `imap.gmail.com:993` TLS · `smtp.gmail.com:465` TLS |
| iCloud | account.apple.com → Sign-In and Security → App-Specific Passwords. Use your full `@icloud.com` address. | `imap.mail.me.com:993` TLS · `smtp.mail.me.com:587` STARTTLS |

Other servers must use SSL/TLS or STARTTLS, so your password is always encrypted on the way. The one exception is sending to a server on this computer (`localhost` or `127.0.0.1`, such as Proton Bridge), where **None** is offered for SMTP.

## MCP server

`nightpost mcp` speaks the Model Context Protocol over stdio and shares the app's database and accounts.

```sh
claude mcp add nightpost -- /path/to/nightpost mcp
```

Tools: `list_accounts`, `list_folders`, `list_threads`, `search`, `read_thread`, `view_message`, `get_attachment`,
`snooze_thread`, `unsnooze_thread`, `list_snoozed`, `pin_thread`, `mark_read`, `star_thread`,
`move_thread`, `label_thread`, `list_tabs`, `sort_sender`, `sort_thread`, `save_draft`, `sync_status`, `sync`. There is deliberately no tool that sends email: `save_draft` puts the message in Drafts, and you send it yourself from NightPost or any other mail app.

- `view_message` shows an email the way it was designed. It returns the text with an `[image N]` marker where each picture sits, and then the pictures. Inline (`cid:`), remote and `data:` images are included. Tracking pixels are removed and never fetched, small icons are skipped, and pictures are sized to at most 1200 px. Remote pictures are fetched only when the assistant passes `images: true`, since fetching them tells the sender the email was opened, and never from your own network (a private, local or Tailscale address).
- `get_attachment` downloads an attachment (using the same cache as the app) and returns its path. Pictures come back to look at, and small text files (CSV, JSON, ICS…) come back inline. Other files, such as PDFs, are opened from the path.
- `save_draft` writes a draft (or, with `thread_id`, a reply draft) to the Drafts folder instead of sending it. You finish it with **Edit draft** in NightPost or in any other mail app. It can attach files only from Downloads, Documents and Desktop (not hidden files or folders), or attachments it fetched with `get_attachment`, so an email can't trick an assistant into attaching something private such as an SSH key. Its reply lists the full path of everything it attached.

## Themes

NightPost has one theme, Moonlit, in Light and Dark; **Match system** switches between them with your Mac's (or desktop's) appearance. Its colours are written as Mailspring-style LESS variables (`crates/nightpost-core/themes/`), and `nightpost themes inspect <dir>` prints how each colour role of such a package resolves.

## Layout

```
crates/nightpost-core   models, SQLite store, IMAP sync engine, SMTP, snooze, themes, MCP server
crates/nightpost        egui desktop app (sidebar / thread list / reader, composer, preferences)
```

Data lives in `~/Library/Application Support/co.timson.NightPost/` (override with `--db` or `NIGHTPOST_DB`).

## Not yet implemented

Inline (cid:) images in the app's HTML view (the MCP `view_message` tool does include them), selecting text in an HTML email on Linux with the WebKit engine (choose Blitz to select it), and send-later or read receipts.

## Snooze sync

Each account gets two extra folders (labels on Gmail):

- **NightPost Snoozed** holds snoozed conversations until they wake. Other mail apps show the folder, so you can find snoozed mail there. Moving a conversation out of it elsewhere (on your phone, say) cancels the snooze. NightPost checks the folder on the server before deciding that, so a sync that was cut short (a phone losing its connection, say) never cancels snoozes. If it can't check the folder, or can't read the snooze list, it leaves your snoozes alone and says so in the activity log, which also records snoozes cancelled elsewhere.
- **NightPost Sync** holds one small message, "NightPost snoozes", listing each snooze by Message-ID with its wake time, plus your pins, inbox tabs and sorting rules. Leave it alone. NightPost hides both folders and never shows the message as mail.

NightPost used to be called OwlPost, and these folders were **OwlPost Snoozed** and **OwlPost Sync**. The first NightPost to sync an account renames them on the server, so snoozes, pins, tabs and rules carry over; if the rename fails, snooze sync waits and tries again rather than guess. An OwlPost that hasn't been updated makes new, empty folders under the old names; NightPost hides them, and sends any mail that OwlPost snoozes into its old folder back to the Inbox, so nothing stays out of sight. Update every copy (each computer, the MCP server, the iPhone app) to keep snoozes in step.

Each change merges with what's on the server, and for any one conversation the newer change wins. Cancelled snoozes are remembered for 30 days so a Mac that was offline doesn't bring them back. Whichever NightPost is running when a snooze is due moves it back to the Inbox and marks it unread. When none is running, it comes back the next time one starts. Snoozes made before syncing existed are uploaded on the first sync. Pins work the same way: a pin or unpin is saved to the server at once, the newer change to a conversation wins on every computer, unpins are remembered for 30 days, and pins made before they synced are uploaded on the first sync. Computers running an older NightPost ignore pins. Code: `crates/nightpost-core/src/snooze_sync.rs`.

## For developers

- [AGENTS.md](AGENTS.md): the house rules for anyone, person or agent, changing the code. Read it first.
- [docs/architecture.md](docs/architecture.md): how NightPost fits together: crates, threads, processes, the local database and what's kept on the mail server.
- [docs/code-audit.md](docs/code-audit.md): the code audit process and each audit's results. `scripts/audit.sh` prints the numbers.

## Plans

- iPhone app: see [docs/ios.md](docs/ios.md). A first version builds (`ios/`): Sign in with Google for Gmail and Workspace, app passwords for other accounts, the unified list with swipes chosen as on the desktop (Settings → Swipes, with the desktop's defaults), each account's folders, HTML email with tracker blocking and attachments, a composer (formatting, links, attachments from Photos or Files, drafts on the server, contacts suggestions), snooze and pin that sync with the computers, moving and labelling from a Move sheet, Inbox tabs with sender rules that sync too (the tabs are the list's first row and scroll with it), a folder count by the spinner while it syncs (every account's folders added up, as they sync side by side), and a folder you open syncs straight away rather than waiting its turn. Not yet run on a phone.

## Known issues

### `imap-proto` future-incompatibility warning (needs fixing)

Builds print `warning: the following packages contain code that will be rejected by a future version of Rust: imap-proto v0.10.2`. It comes from `imap` 2.4.1 (`crates/nightpost-core/Cargo.toml`), which depends on `imap-proto` 0.10.2. That crate's macros end with a trailing semicolon, which Rust is phasing out ([rust-lang/rust#79813](https://github.com/rust-lang/rust/issues/79813)). For now it's only a warning and doesn't change how NightPost behaves. A future Rust release will make it a hard error, and the build will then fail.

Planned fix: move to a maintained IMAP client that uses `imap-proto` 0.16, rather than patching the old copy. The choice between `async-imap` and `imap` 3.0 (still an alpha), and how to port and test it, is in [docs/imap-upgrade.md](docs/imap-upgrade.md).

See the details with `cargo report future-incompatibilities --package imap-proto@0.10.2`.

### Unmaintained crates that `cargo audit` warns about

`cargo audit` reports no vulnerabilities in these crates, only that nobody maintains them now (or, for `lexical-core`, that old versions are unsound). NightPost doesn't use any of them directly except `chrono`. Each comes in through another crate, and goes when that one moves on.

| Crate | Advisory | Comes from | Plan |
|---|---|---|---|
| `lexical-core` 0.7.6 (unsound) | RUSTSEC-2023-0086 | `nom` 5, used by `imap` 2.4.1 | Goes with the move off `imap` 2.4.1 ([docs/imap-upgrade.md](docs/imap-upgrade.md)) |
| `chrono` 0.4 | RUSTSEC-2026-0543 | NightPost's own dates, and `imap` 2.4.1 | Watch for a maintained fork or successor; dates are used throughout, so moving is its own planned change |
| `ttf-parser` 0.25 | RUSTSEC-2026-0192 | `egui`'s font drawing (`ab_glyph`) | Goes when `egui` moves to another font library |
| `zbus_names`, `zvariant`, `zvariant_derive` | RUSTSEC-2026-0370 to -0372 | Linux accessibility in `egui` (`accesskit_unix`, `atspi`) | Goes with an `egui`/`accesskit` update |
| `paste` 1.0 | RUSTSEC-2024-0436 | `wgpu` on the Mac (`metal`) | Goes with a `wgpu` update |
| `basic-toml` 0.1 | RUSTSEC-2026-0374 | The iPhone bindings generator (`uniffi`, `askama`), at build time only | Goes with a `uniffi` update |

These are checked each month with the code audit ([docs/code-audit.md](docs/code-audit.md)).
