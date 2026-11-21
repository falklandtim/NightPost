# Assistants and the MCP server

NightPost has a built-in **MCP server** (Model Context Protocol). It lets an AI assistant, such as Claude, work with your mail through NightPost: find and read conversations, triage them, snooze them, and write drafts for you.

**An assistant can never send email through NightPost.** When it writes a reply, it saves it as a draft, and you check it and send it yourself, from NightPost or any other mail app. This is on purpose.

## Connecting an assistant

The exact command is in **Preferences → MCP Server**, ready to copy. It depends on how NightPost is installed:

| Installed as | Command for Claude Code |
|---|---|
| The Mac app | `claude mcp add nightpost -- /Applications/NightPost.app/Contents/MacOS/nightpost mcp` |
| The Linux Flatpak | `claude mcp add nightpost -- flatpak run co.timson.NightPost mcp` |
| Built on Linux | `claude mcp add nightpost -- ~/.local/bin/nightpost mcp` |

The MCP server uses the same accounts and mail as the app, so add your accounts in NightPost first. The app doesn't need to be open.

## Connecting Claude Desktop

1. In Claude Desktop, open **Settings → Developer → Edit Config**. This opens Claude's settings file, `claude_desktop_config.json`. On a Mac it's in `~/Library/Application Support/Claude/`.
2. If the file is empty, or only has `{}` in it, replace everything with this:

   ```json
   {
     "mcpServers": {
       "nightpost": {
         "command": "/Applications/NightPost.app/Contents/MacOS/nightpost",
         "args": ["mcp"]
       }
     }
   }
   ```

   If it already has an `"mcpServers"` section with other entries, add only the `"nightpost": { … }` part inside it, with a comma after the entry before it.

   For the Linux Flatpak, use `"command": "flatpak"` and `"args": ["run", "co.timson.NightPost", "mcp"]` instead. **Preferences → MCP Server** shows the right text for your computer, ready to copy.
3. **Quit Claude Desktop completely** (⌘ Q, not just closing its window), then open it again. NightPost appears in Claude's list of tools.
4. On a Mac, the first time Claude reads your mail, macOS may ask whether `nightpost` may use your Keychain, where your mail passwords are kept. Choose **Always Allow**.

Then ask Claude things like:

- "What's unread in my inbox?"
- "Snooze the newsletter from Kōwhai until Saturday."
- "Draft a reply to Sam saying Thursday works."

If NightPost doesn't appear in Claude, check that the path in step 2 is where NightPost is installed. NightPost's log file marks the MCP server's lines with `[mcp]` (see [When something goes wrong](troubleshooting.md#the-log-file)).

## What an assistant can do

- **Look around:** list your accounts and folders, list conversations, and search (with the same `from:`, `subject:`, `is:unread` and other words as the search box).
- **Read:** read whole conversations; see an email as it was designed, with its pictures; and open attachments.
- **Triage:** mark read or unread, star, move (archive, trash, spam or a folder), add or remove Gmail labels, pin, and sort senders or conversations into Inbox tabs.
- **Snooze and unsnooze** conversations, and list what's snoozed.
- **Save drafts,** new or as replies, with attachments.
- **Sync** and check whether each account is connected.

## What it's kept away from

- **Sending:** there's no way to send (see above).
- **Your files:** a draft can only attach files from your Downloads, Documents and Desktop folders, or attachments it fetched from your mail. An email can't trick an assistant into attaching something private, such as a key file.
- **Trackers:** pictures in an email are only fetched when the assistant asks for them, since fetching them can tell the sender you opened it. Known trackers are never fetched.
