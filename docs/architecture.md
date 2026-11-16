# How NightPost fits together

A map for anyone opening the code cold. The house rules are in [AGENTS.md](../AGENTS.md); what each feature does for users is in the [README](../README.md).

## The two crates

| Crate | What it is | Depends on egui? |
|---|---|---|
| `crates/nightpost-core` | Everything that isn't the window: models, the SQLite store, the IMAP engine and push, SMTP, Google sign-in, snooze and its server sync, inbox categories, themes, and the MCP server. | No, and it must stay that way |
| `crates/nightpost-ffi` | The core for the iPhone app: a `Mail` object that UniFFI turns into Swift (`scripts/ios-rust.sh`). Plain records only: ids, strings, Unix-second dates. See [ios.md](ios.md). | No |
| `crates/nightpost` | The app: the egui window and its parts, the background threads, the web views (HTML email and the composer), Contacts, sender pictures, logging. One binary, `nightpost`, which is also the MCP server and a small CLI. | Yes |

```
nightpost (binary)
├── main.rs            start-up: the window, or `nightpost mcp`, or `nightpost themes`
├── app/               the window (state in mod.rs; one file per part, see below)
├── worker.rs          background threads: sync, push, actions, sender pictures
├── composer.rs        compose / reply / forward windows
├── web_host.rs        native windows holding a web page (the formatted composer)
├── mail_view.rs       HTML email in the reading pane (WebKit / WebKitGTK)
├── prefs.rs, onboarding.rs, avatars.rs, contacts.rs, attachments.rs, …
└── uses nightpost-core
        ├── store/         SQLite: accounts, folders, mail, snoozes, pins, tabs
        ├── engine.rs      IMAP sync and every server-side action
        ├── idle.rs        push mail (IMAP IDLE)
        ├── snooze_sync.rs the state kept on each server (snoozes, pins, tabs)
        ├── smtp.rs, oauth.rs, credentials.rs, parse.rs, category.rs
        ├── mail_html.rs   reading and writing HTML: the email page's policy, quotes, drafts made safe to edit
        ├── mcp.rs         the MCP server
        └── theme.rs       Mailspring-compatible themes

nightpost-ffi (library for the iPhone app, UniFFI → Swift)
└── uses nightpost-core: the same store, engine and sync state
```

## Processes

On a computer, two kinds of process use the same database, `~/Library/Application Support/co.timson.NightPost/nightpost.db` on a Mac (`~/.local/share/…` on Linux, or `NIGHTPOST_DB`):

- **The app** (`nightpost`): the window and its threads.
- **The iPhone app** (`ios/`, SwiftUI) has its own database in its sandbox, set with `nightpost_core::use_folders`. It agrees with the computers through the mail server, like any other copy.
- **The MCP server** (`nightpost mcp`): started by an assistant (Claude, Dobby) over stdio. It reads mail from the database, and makes changes both there and on the server, with the same `engine` functions the app uses. It cannot send mail: `save_draft` puts a message in Drafts.

The database runs in WAL mode with a 10-second busy timeout, so both can read and write at once. Each thread opens its own `Store` connection. Anything both need to see, such as sync health and the activity log, lives in the database, not in memory.

## Threads in the app

The UI thread draws each frame and never waits on the network.

| Thread | Does | Started in |
|---|---|---|
| UI (main) | egui: draws, handles input, changes the local store at once | `main.rs`, `app/` |
| `sync` | One sync at a time, every 5 minutes or on request. Requests that queue up meanwhile are merged (`SyncPlan`). A pushed change syncs just that Inbox. After mail, each account's server state (snoozes, pins, tabs) is merged. | `worker::Worker::spawn` |
| push | One IMAP IDLE connection per account on its Inbox, run by a supervisor that replaces stalled ones, re-issued every 8 minutes and restarted after sleep. A change from the server asks `sync` for that Inbox. | `worker::push` |
| `actions` | Takes each `Cmd` from the UI and runs it on its own short-lived thread. Commands on the same conversation take turns in the order sent (`Turns`), so an undo never overtakes its action. | `worker::Worker::spawn` |
| `avatars-N` | A few sender-picture lookups at a time (Contacts photo, BIMI logo, site icon). | `worker::Worker::spawn` |
| GTK (Linux only) | GTK 4's main loop (a `glib::MainLoop`), for WebKitGTK 6.0: the composer windows and the off-screen HTML view. Other threads post work to it. | `web_host` |

Results come back to the UI as `worker::Event`s on a channel, and the UI repaints.

## How a click becomes a change on the server

Archiving a conversation, for example:

1. **UI** (`app/actions.rs`): removes the row and updates the store at once, records an `Undo`, shows the toast, and sends `Cmd::Move(account, thread, Archive)`.
2. **actions thread** (`worker.rs` `run_action`): waits its turn for that conversation, then calls `engine::move_thread`.
3. **engine** (`engine.rs`): connects over IMAP and moves the messages (on Gmail, only takes the Inbox label off), then updates the store to match the server.
4. **UI**: on `Event::Changed` it reloads the list; on `Event::Error` it shows what went wrong.

⌘Z sends the reverse command (`Cmd::Restore`, `Cmd::Unsnooze` …), which takes its turn after the original.

## Where things are kept

| What | Where |
|---|---|
| Accounts, folders, cached mail (headers, bodies, attachment names), snoozes, pins, tabs and rules, signatures, sync health and activity | SQLite (`store/`). Tables are created with `CREATE TABLE IF NOT EXISTS`; columns added later get guarded `ALTER TABLE` migrations in `Store::open`. The `outbox` table is created but unused: Undo Send holds the message in memory, and quitting sends it first. |
| Passwords, Google refresh tokens, the Google client secret | The system keychain (`credentials.rs`): macOS Keychain, Secret Service on Linux. Never in the database. |
| Window size, theme, list options, open folders, collapsed accounts | eframe's settings storage (keys `nightpost.*` in `app/mod.rs`) |
| Attachment files | Fetched when first opened and cached in the system cache folder (`attachments/`); the server keeps the real copy |
| Sender logos | Cached for a month in the cache folder (`logos/`) |
| Log | `~/Library/Logs/NightPost/nightpost.log` (Mac), `~/.local/state/nightpost/nightpost.log` (Linux); rotated at 2 MB |

## What's kept on the mail server

So that every computer running NightPost agrees, and the phone sees the same mail:

- **Mail and flags:** read, starred, folders and labels, as any mail app keeps them.
- **Drafts:** in the account's Drafts folder, so other apps see them.
- **"NightPost Snoozed"** (a folder, or a label on Gmail): snoozed conversations wait here.
- **"NightPost Sync"**: one small message holding `snooze_sync::State` as JSON: snoozes, pins, your inbox tabs, sender rules and conversation rules. Records name conversations by Message-ID, since thread keys differ between machines. Each record carries `updated` and `removed`: the newer change wins, and removals stay 30 days as tombstones. Writes append the new copy, then delete the old ones, so a failure leaves two copies (merged next time), never none.
- Before the rename these were **"OwlPost Snoozed"** and **"OwlPost Sync"**. `snooze_sync::migrate_legacy_folders` renames them (on the server and in the local store) the first time a NightPost syncs the account, and the state reader accepts OwlPost's header and sender. If an older OwlPost recreates the old folders, they're hidden and any mail in the old Snoozed folder goes back to the Inbox (`release_legacy_snoozed`).

Older copies of NightPost ignore fields they don't know, and may rewrite the state without them. Every sync puts back what this machine knows, so the loss is temporary. New fields must be `#[serde(default)]`.

## The window

`app/mod.rs` holds `NightPostApp`'s state, start-up and the frame. Each part of the window is in its own file, as another `impl NightPostApp` block:

| File | Part |
|---|---|
| `app/sidebar.rs` | All Accounts, then each account's folders as a tree |
| `app/list.rs` | The message list: header, tabs, rows, swipes, hover actions, right-click menu |
| `app/reader.rs` | The open conversation (cards, or the HTML web view), attachments |
| `app/toolbar.rs` | The toolbar and its pop-ups (Move, Label, More, Snooze calendar) |
| `app/sheets.rs` | Dialogs and other windows: shortcuts, Sync activity, folders, new tab, Show original, composers, preferences |
| `app/actions.rs` | What each command does, multi-select, undo, Undo Send |
| `app/input.rs` | Keyboard shortcuts and worker events |
| `app/ui_tests.rs` | Offscreen UI tests with the demo accounts |

Colours come from the theme (`style::Colors`, built from a Mailspring-style palette in `nightpost-core/src/theme.rs`); nothing is hard-coded.

### Web views

- **HTML email** (`mail_view.rs`): one page per conversation, each email body in a sandboxed `<iframe>` under a strict Content Security Policy. The page's own small script sizes frames, recolours emails for dark themes, forwards shortcut keys and draws the attachment menu. Clicks come back as `nightpost://` links.
  - On the Mac it's a native WebKit view laid over the reading pane.
  - On Linux an egui window can't hold a web view on Wayland, so WebKitGTK 6.0 draws the page in a GTK 4 window that's never shown. The app shows it as a picture and maps clicks to a list of targets (links, folds, attachments).
- **Composer** (`composer.rs` + `web_host.rs`): a formatted editor in its own native window (WebKit or WebKitGTK), talking to the app through `postMessage`. Where no web view is available, an egui composer is used.

## Testing

- `cargo test --workspace`: unit tests in both crates, plus offscreen UI tests (`app/ui_tests.rs`, egui_kittest) that render the app with the demo accounts and click through it. `NIGHTPOST_SHOTS=<dir>` saves their screenshots.
- Demo accounts (`nightpost_core::demo`, `--demo`) never touch the network; every engine function has a demo path. Real accounts are tested by hand.
- `scripts/audit.sh`: the code audit's numbers (see [code-audit.md](code-audit.md)).
