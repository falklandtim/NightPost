# Using NightPost on several computers

You can run NightPost on as many computers as you like, Mac or Linux. Your mail lives on your mail server, so every computer sees the same mail, folders, read state and stars, as with any mail app.

NightPost also keeps a few things of its own on the mail server, so every computer agrees on them:

- **Snoozes:** what's snoozed, and until when.
- **Pins.**
- **Inbox tabs:** your own tabs, and which senders and conversations go in them.

Each computer keeps its own settings: theme, swipes, signatures and the other Preferences.

## The two NightPost folders

To do this, NightPost adds two folders to each account (on Gmail, two labels):

- **NightPost Snoozed** holds snoozed conversations until they wake. Your phone's mail app shows this folder, so you can find snoozed mail there. If you move a conversation out of it somewhere else, on your phone say, its snooze is cancelled.
- **NightPost Sync** holds one small message listing your snoozes, pins and tabs. Please leave it alone.

NightPost hides both folders itself.

## When things happen

- A change on one computer reaches the others the next time they sync, usually within a few minutes.
- If two computers change the same conversation, the newer change wins.
- When a snooze is due, whichever NightPost is running brings the conversation back to the Inbox. If none is running, it comes back when one starts.

Keep every copy of NightPost up to date, so they all understand each other: each computer, and an MCP server if you use one.

## Renamed from OwlPost

NightPost used to be called OwlPost. The first time it runs, it brings everything across from OwlPost: your accounts and mail, settings, saved passwords and Google sign-ins, and the snooze folders on the server, which it renames. On the Mac, it then asks once for each saved password: choose **Always Allow**.
