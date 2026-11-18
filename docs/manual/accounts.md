# Adding your accounts

The first time NightPost opens, it asks you to connect an account. To add more later, open **Preferences** (⌘ ,) → **Accounts** → **+ Add Account**.

Choose your provider: **Gmail / Google Workspace**, **iCloud Mail**, **Outlook.com / Office 365**, **Yahoo Mail**, **Fastmail**, or **IMAP / SMTP (other)**. For the named providers, the server settings are filled in for you.

## Gmail and Google Workspace

There are two ways in.

**An app password** is quickest:

1. Turn on 2-Step Verification for your Google account.
2. Go to [myaccount.google.com/apppasswords](https://myaccount.google.com/apppasswords) and create an app password.
3. Make sure IMAP is turned on in Gmail's settings.
4. In NightPost, enter your address and the app password (not your normal password).

**Sign in with Google** signs you in through your browser, so NightPost never sees your Google password. Google insists that every app has its own "OAuth client", so you have to make one first. It's free and takes about ten minutes:

1. At [console.cloud.google.com](https://console.cloud.google.com/auth/clients), create a project, for example "NightPost".
2. Under **Google Auth Platform → Branding**, enter the app's name and your email address.
3. Under **Audience**:
   - For a **Google Workspace** account, choose **Internal**. There's nothing more to do, and the sign-in never expires.
   - For a **personal @gmail.com** account, choose **External**, add yourself as a test user, then click **Publish app**. If you leave it in Testing, you have to sign in again every 7 days. When you sign in, Google warns that the app isn't verified: click **Advanced → Continue**.
4. Under **Data Access → Add or remove scopes**, add `https://mail.google.com/` by hand.
5. Under **Clients → Create client**, choose **Desktop app**. Copy the Client ID and Client secret into NightPost, in **Preferences → Accounts → Sign in with Google**.

Then click **Sign in with Google…** to add the account. An account that already uses an app password can switch over with **Use Google sign-in** in Preferences → Accounts.

You can withdraw NightPost's access at any time, at myaccount.google.com → Security → Third-party apps.

## iCloud Mail

Apple only lets other mail apps in with an app-specific password:

1. Go to [account.apple.com](https://account.apple.com) → **Sign-In and Security** → **App-Specific Passwords**, and create one.
2. In NightPost, enter your full `@icloud.com` address and that password.

## Other mail providers

Choose **IMAP / SMTP (other)**, then enter your address, password and the server names your provider gives. The connection must be encrypted (SSL/TLS or STARTTLS), so your password is never sent in the clear. The one exception: sending through a program on your own computer, such as Proton Mail Bridge, can use **None**.

## Is each account connected?

A mark beside each account's name in the sidebar shows how it's doing:

- **A small green dot:** it's connected and syncing.
- **Amber:** a brief problem, such as one failed sync, or push mail reconnecting. It usually sorts itself out.
- **A red !:** it can't connect. It has failed twice in a row, or the server turned down the password or Google sign-in.

Hover over the mark to see when the account last synced, and why it's failing. The bottom of the sidebar says when mail was last checked ("Synced 2 min ago"), or names the account that can't connect.

Click either one for **Sync activity**, which you can also open with ⌘ Shift L or **More (⋯) → Sync activity**. See [When something goes wrong](troubleshooting.md).

## How new mail arrives

Each account keeps a connection open to its Inbox, and the server tells NightPost about new mail as it arrives, usually within a couple of seconds. This is called push. Other folders are checked every 5 minutes. Preferences → Accounts shows how each account gets its mail: **instant (push)**, **reconnecting**, or **every 5 minutes** for servers that don't offer push.

The first time an account syncs, NightPost downloads the newest 300 emails in each folder, starting with the Inbox. Older mail stays on the server.

## Changing or removing an account

In Preferences → Accounts, click **Edit** to change an account's details, or **Remove** to take it out of NightPost. Removing an account doesn't delete any mail from the server.
