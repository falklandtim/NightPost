# When something goes wrong

## An account isn't connecting

The mark beside each account's name in the sidebar shows its state: green when it's fine, amber for a brief problem, and a red **!** when it can't connect. Hover over it to see why, in plain words. See [Adding your accounts](accounts.md#is-each-account-connected).

**Sync activity** has the details. Open it by clicking the mark, or the "Synced …" line at the bottom of the sidebar, or press ⌘ Shift L, or choose **More (⋯) → Sync activity**. It shows:

- each account's state, with **Sync now**, and **Sign in again…** when the server turned down your sign-in;
- what happened, newest first: folders checked, new mail, push connecting and dropping, and errors.

You can show one account or only problems, **Copy** the lines to send to someone, or click **Show log file**.

Common causes:

- **"The server turned down the password":** the app password was removed or changed, or the Google sign-in expired. A personal Gmail account whose OAuth client is still in Testing must sign in every 7 days. Make a new app password, or click **Sign in again…**.
- **Push keeps dropping** ("the server didn't answer in time"): NightPost reconnects within seconds, and other folders still sync every 5 minutes. It's harmless unless new mail stops arriving.

## An action failed

If the server turns down an action (moving, archiving, deleting, labelling, starring, snoozing or marking read), a red message appears. It also goes into Sync activity, saying which step the server refused. Nothing is lost: a conversation that failed to move, archive or go to Trash comes back to where it was. Try again in a minute.

For example, Gmail sometimes answers "System Error" when moving mail to Trash. That's a fault on Google's side.

## Passwords keep being asked for

- **On the Mac,** when macOS asks whether NightPost may use the Keychain, choose **Always Allow**.
- **On Linux,** passwords live in your desktop's keyring. If your keyring is locked, or won't open, NightPost can't read them. Check that GNOME Keyring or KDE Wallet is running and unlocked.

## The log file

NightPost writes what it's doing, and every warning, to a log file:

| Where | Log file |
|---|---|
| Mac | `~/Library/Logs/NightPost/nightpost.log` (Console.app shows it too) |
| Linux, built from source | `~/.local/state/nightpost/nightpost.log` |
| Linux Flatpak | `~/.var/app/co.timson.NightPost/.local/state/nightpost/nightpost.log` |

At 2 MB it's renamed `nightpost.log.1` and a new one starts. Lines from the MCP server start with `[mcp]`.

## Things NightPost can't do yet

- Show pictures sent inside an email (`cid:` pictures) in the reading pane. An assistant using the MCP server does see them.
- Select text in an HTML email, on Linux.
- Schedule an email to send later, or ask for read receipts.
- In the Linux Flatpak, copying a picture to the clipboard may not work. Copying text does.
