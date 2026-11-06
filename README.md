# OwlPost 🌱

A fast, native email client written in Rust, inspired by [Mailspring](https://github.com/foundry376/mailspring).
It has one binary with no Electron, a local SQLite cache, a built-in MCP server, and it can use Mailspring themes.

## Features

- **Gmail, iCloud and any IMAP/SMTP account.** Presets for Gmail, iCloud, Outlook, Yahoo and Fastmail; passwords are kept in the macOS Keychain (or the Windows/Linux equivalent).
- **Unified Inbox** plus per-account folders, conversation threading (References/In-Reply-To), and Gmail-style search (`from:`, `to:`, `subject:`, `is:unread`, `is:starred`, `is:pinned`, `has:attachment`).
- **Snooze**: *Later today, Tonight, Tomorrow, This weekend, Next week, Next month*, or type a time like `3h`, `friday` or `2026-12-01 09:00`. Snoozed threads leave every mailbox and come back to the inbox unread when they wake.
- **Pin to top** (like Canary): press `p`, click the pin in the toolbar, or right-click a conversation. Pinned conversations stay above everything else in every list they appear in. Pins are stored locally.
- **Swipe actions**: swipe a conversation sideways with two fingers on a trackpad, or drag it with the mouse. By default swiping right archives and swiping left trashes. Each direction can be set to archive, trash, snooze, pin or mark read/unread in Preferences → Message list.
- **Quick actions on hover**: widen the message list (at least 420 points) and hovering over a conversation shows mark read/unread, archive, trash, snooze and pin buttons, as in Mailspring.
- **Compose in its own window**: new messages, replies and forwards each open a separate window. Closing a window that has text in it asks before discarding the draft.
- **Attachments**: click **Attach** in the composer or drag files onto its window. Each file shows as a chip with a thumbnail (for photos) and its size; × removes it. When you attach JPEG or PNG photos, an **Image size** menu offers Actual Size, Large (2048 px), Medium (1280 px) and Small (640 px), measured on the longest side. Photos are resized in the background and turned upright from their camera rotation, and the chip shows the new size next to the original. The composer shows the total message size and blocks Send above 25 MB, the limit at Gmail and iCloud. The MCP `send_email` tool takes an `attachments` list of file paths.
- **Move & label**: move a conversation to any folder (`v`, the folder button, or right-click → Move to), and add or remove Gmail labels (`l`, the tag button, or right-click → Label). Both pickers are searchable and can create a new folder or label. Labels show as removable chips on the open conversation. On Gmail, Archive and Move only take a conversation out of the Inbox, so its other labels stay.
- **Signatures**: Preferences → Signatures lets you create named signatures and choose a default for each account. Each signature has a **Plain text / HTML** toggle. HTML signatures appear as readable text while you write, and the message is sent with both a text and an HTML version, with your real HTML in the HTML version (the MCP `send_email` and `reply` tools do the same). HTML signatures are drawn in the composer and in Preferences, with bold and italic text, links, and images loaded from the web. Pasting a whole page copied from another app (such as Canary) is fine: on Save, OwlPost keeps only the signature and removes page styles, such as Canary's dark-mode colour inversion, which would otherwise restyle the whole email. Images load even when the host labels them with the wrong file type, as Dropbox share links do. The composer has a signature picker that swaps the signature in place, and changing the From account switches to that account's default.
- Archive, trash, spam, star, mark unread, reply, reply all and forward, with address autocomplete.
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
```

The app icon lives in `crates/owlpost/assets/icon-1024.png`. After changing it, run `cargo run -p owlpost --example icon_assets` to regenerate the padded macOS version. The bundle script also does this automatically when the source is newer.

## Connecting Gmail and iCloud

Both providers need an **app password** rather than your normal password:

| Provider | Create an app password | Servers (pre-filled) |
|---|---|---|
| Gmail | Turn on 2-Step Verification, then visit myaccount.google.com/apppasswords. Make sure IMAP is enabled in Gmail settings. | `imap.gmail.com:993` TLS · `smtp.gmail.com:465` TLS |
| iCloud | account.apple.com → Sign-In and Security → App-Specific Passwords. Use your full `@icloud.com` address. | `imap.mail.me.com:993` TLS · `smtp.mail.me.com:587` STARTTLS |

## MCP server

`owlpost mcp` speaks the Model Context Protocol over stdio and shares the app's database and accounts.

```sh
claude mcp add owlpost -- /path/to/owlpost mcp
```

Tools: `list_accounts`, `list_folders`, `list_threads`, `search`, `read_thread`,
`snooze_thread`, `unsnooze_thread`, `list_snoozed`, `pin_thread`, `mark_read`, `star_thread`,
`move_thread`, `label_thread`, `send_email`, `reply`, `sync`. The sending tools are annotated as destructive, so MCP clients ask you to confirm before anything is sent.

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

HTML message rendering (bodies are shown as text), attachment download, IMAP IDLE push (it polls every 5 minutes), Gmail OAuth (app passwords for now), server-side snooze that syncs across devices, drafts sync, and send-later or read receipts.
