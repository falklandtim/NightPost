# Reading mail

## The window

NightPost's window has three parts:

- **The sidebar** on the left. At the top, **All Accounts** brings every account together: **Unified Inbox**, **Starred**, **Snoozed**, **Drafts**, **Sent**, **Archive**, **Spam** and **Trash**. Under it, each account has its own folders. Folders inside other folders fold away under their parent: click the arrow beside a folder to show or hide what's inside, or the arrow beside an account to fold the whole account away. NightPost remembers what you left open.
- **The message list** in the middle: one row per conversation, newest first. Drag its right-hand edge to make it wider or narrower.
- **The reading pane** on the right: the conversation you've clicked.

The sidebar button at the left of the toolbar hides or shows the sidebar.

Emails that belong together (replies, and replies to replies) are shown as one conversation. Your own replies are part of it too, even though they're kept in Sent.

## The message list

The list is grouped under headings: **Pinned**, **Today**, **Yesterday**, **Earlier this week**, **Last week**, then by month. **Hide** beside Pinned folds the pinned conversations away.

Each row shows who's in the conversation, its subject and the start of its newest message (yours included), with the date. Unread conversations have a dot before the name and a light tint. Pinned conversations have a purple pin and a purple tint, which is deeper when they hold unread mail. A small box shows how many emails a conversation has, a star shows it's starred, and a paperclip shows it has attachments.

Above the list:

- **The Inbox tabs** (see below).
- **The filters:** **Unread**, **Starred**, **Pinned** and **With attachments**.
- **✓ Mark all as read** marks everything in the list as read.

**Archive** in All Accounts shows archived mail from every account. Gmail has no Archive folder, so for Gmail it shows the conversations in All Mail that aren't in the Inbox.

## Inbox tabs

The Inbox has tabs, each with its unread count:

- **All:** everything, and where the Inbox opens.
- **Primary:** mail from people, and replies.
- **Promotions:** newsletters and marketing.
- **Social:** mail from social networks.

NightPost sorts each email using hidden information in it. For example, newsletters carry an "unsubscribe" header, and social networks mark their own mail. Gmail accounts use Gmail's own sorting.

You can change where things go:

- **Always put a sender's mail in a tab:** right-click a conversation → **Always put their mail in ▸**, and choose a tab. This covers mail already there, and all future mail from that address.
- **Put one conversation in a tab:** right-click → **Put this conversation in ▸**. This beats the sender's rule, and replies stay with it.
- **Make your own tab**, say **Invoices:** choose **New tab…** from either menu. To remove one of your tabs, right-click it; its mail goes back to the usual tabs. On Gmail, a conversation in one of your tabs also gets a Gmail label of the same name, so you see it grouped on your phone too.
- **See everything from one sender:** right-click → **All mail from …**.

Your tabs and rules are kept on the mail server, so every computer running NightPost uses the same ones.

## Searching

Click the search box at the top right, or press ⌘ F or `/`. Type words to find them in the subject, sender or text. You can narrow a search:

| Type | To find |
|---|---|
| `from:anna` | mail from Anna |
| `to:sam@example.com` | mail to that address |
| `subject:invoice` | "invoice" in the subject |
| `is:unread`, `is:starred`, `is:pinned` | unread, starred or pinned conversations |
| `has:attachment` | conversations with attachments |
| `category:promotions` | one Inbox tab |

Combine them, for example `from:anna has:attachment`. Press Esc to clear the search.

## HTML email

Emails are shown as they were designed, with their layout, pictures and colours. In a conversation, older emails fold up; click one to open it.

NightPost protects you while you read:

- **Trackers are blocked.** Many emails contain an invisible picture that tells the sender when, and roughly where, you opened it. NightPost removes the ones it recognises, and a note above the email names the blocked tracker.
- **Other pictures load straight away.** Any of them could still tell the sender you've opened the email. To stop that too, turn off **Load remote images** in Preferences → Appearance. Each email then shows a **Load images** button.
- **Nothing in an email can run, or change NightPost.** Scripts and forms are blocked, and an email can't move you to a web page or change your settings.
- **Links open in your web browser.**

With a dark theme, emails are recoloured to match: pale backgrounds turn dark and dark text turns light. Pictures, buttons and coloured banners keep their look. To see emails on white as designed, turn off **Match emails to a dark theme** in Preferences → Appearance.

To see plain text instead, turn off **Show emails as designed (HTML)** in Preferences → Appearance.

**On Linux,** you can't select text in an HTML email yet.

## Attachments

Click an attachment to open it in its usual app. NightPost downloads it from the server the first time.

Files that could harm your computer ask first, with **Open Anyway** or **Cancel**. These are programs, scripts, installers, archives such as `.zip` and `.dmg`, and documents with macros. On the Mac, saved attachments are also marked as downloaded from the internet, so macOS checks them before they run. Linux has no such mark, so that question is the only check.

Right-click an attachment for **Save to Downloads** (it never overwrites a file already there) or **Save As…**.

## Sender photos and logos

Beside each email you'll see one of:

- **The person's photo,** from your Contacts. This is on the Mac only; the first time, macOS asks whether NightPost may use Contacts.
- **The company's logo,** when it publishes one, or its website's icon.
- **A coloured letter,** otherwise.

Logos are fetched only from the sender's own website, never from a lookup service that would learn who emails you. Addresses at Gmail, iCloud, Outlook and other mailbox providers never get a company logo. To turn pictures off, untick **Show sender photos and logos** in Preferences → Appearance.

## The original email

**More (⋯) → Show original** opens the newest email's raw source, to copy, or save as a `.eml` file.
