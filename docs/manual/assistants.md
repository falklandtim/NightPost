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

Other assistants, such as Claude Desktop, take the same program and arguments in their settings file; Preferences → MCP Server shows the text to paste.

The MCP server uses the same accounts and mail as the app. The app doesn't need to be open.

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
