// What the screens show, and every call into the Rust core.
//
// Core calls block (the database, and for actions and sync the network), so
// each runs on a background task and the result comes back to the main
// actor. Actions update the list at once (the row goes, the dot clears),
// then the core changes the server; on failure the list is reloaded and the
// error shown, so what's on screen never lies for long.

import Foundation
import Observation

@MainActor
@Observable
final class MailModel {
    private(set) var mail: Mail?
    private(set) var started = false
    private(set) var accounts: [AccountInfo] = []
    private(set) var rows: [ThreadRow] = []
    var mailbox: Mailbox = .inbox
    /// One account's folder, picked in Folders; `nil` shows `mailbox`
    /// across every account.
    var folder: FolderInfo?
    var search = ""
    /// The Inbox tab showing (a category number); `nil` is All.
    var tab: UInt8?
    /// The tabs above the Inbox, with unread counts; empty when the list
    /// isn't an Inbox (see `tabsShown`).
    private(set) var tabs: [InboxTab] = []
    var error: String?
    private(set) var syncing = false
    /// How far the running sync has got (folder 12 of 110), for the
    /// caption by the spinner; `nil` when not syncing.
    private(set) var syncStatus: SyncStatus?
    /// The demo accounts: seeded locally, never on the network.
    private(set) var isDemo = false

    /// Open the database in the app's own folders, or the demo when the
    /// app is launched with `-demo` (for screenshots).
    func start() async {
        if ProcessInfo.processInfo.arguments.contains("-demo") {
            await openDemo()
            return
        }
        do {
            let fm = FileManager.default
            let data = try fm.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            let cache = try fm.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            mail = try await background { try Mail.open(dataDir: data.path, cacheDir: cache.path) }
            await reload()
        } catch {
            show(error)
        }
        started = true
        if !isDemo && !accounts.isEmpty {
            await sync()
        }
    }

    /// Switch to the demo accounts, in a database of their own.
    func openDemo() async {
        do {
            let path = FileManager.default.temporaryDirectory.appendingPathComponent("owlpost-demo.db").path
            try? FileManager.default.removeItem(atPath: path)
            mail = try await background { try Mail.openDemo(path: path) }
            isDemo = true
            await reload()
            #if DEBUG
            // UI tests: reload as a long sync does, to check screens
            // survive the list changing underneath them.
            // Screenshots of the sync caption (the demo never syncs).
            if ProcessInfo.processInfo.arguments.contains("-syncing") {
                syncing = true
                syncStatus = SyncStatus(email: "alex@icloud.demo", folder: "INBOX", done: 83, total: 110)
            }
            if ProcessInfo.processInfo.arguments.contains("-churn") {
                Task {
                    while true {
                        try? await Task.sleep(for: .seconds(1))
                        await reload()
                    }
                }
            }
            #endif
        } catch {
            show(error)
        }
        started = true
    }

    /// Leave the demo for the real database.
    func leaveDemo() async {
        isDemo = false
        rows = []
        accounts = []
        started = false
        await start()
    }

    func reload() async {
        guard let mail else { return }
        let (box, folder, search, shown) = (mailbox, folder, search, tabsShown)
        let wanted = shown ? tab : nil
        do {
            let (a, t, used, r) = try await background {
                let tabs = shown ? try mail.inboxTabs(folderId: folder?.id) : []
                // The tab was removed (perhaps on another device): show All.
                let used = tabs.contains(where: { $0.category == wanted }) ? wanted : nil
                let query = used.map { "\(search) category:\($0)" } ?? search
                let rows = if let folder {
                    try mail.folderThreads(folderId: folder.id, query: query, limit: 300)
                } else {
                    try mail.threads(mailbox: box, query: query, limit: 300)
                }
                return (try mail.accounts(), tabs, used, rows)
            }
            accounts = a
            // Another list may have been picked meanwhile; keep the newest.
            if box == mailbox && folder == self.folder && search == self.search && shown == tabsShown && wanted == (shown ? tab : nil) {
                rows = r
                tabs = t
                if shown { tab = used }
            }
        } catch {
            // The folder went (removed on the server, or its account
            // removed): go back to the Inbox rather than show an error.
            if folder != nil && folder == self.folder {
                self.folder = nil
                mailbox = .inbox
                await reload()
                return
            }
            show(error)
        }
    }

    /// Tabs show over an Inbox (every account's, or one account's Inbox
    /// folder), and not while searching, as on the desktop.
    var tabsShown: Bool {
        search.isEmpty && (folder.map { $0.kind == .inbox } ?? (mailbox == .inbox))
    }

    /// Show one tab (`nil`: All).
    func pick(tab c: UInt8?) async {
        guard c != tab else { return }
        tab = c
        await reload()
    }

    /// Fetch new mail for every account.
    func sync() async {
        guard let mail, !syncing else { return }
        syncing = true
        defer { syncing = false }
        // A first sync of a big account takes many minutes; show mail as it
        // arrives rather than an empty Inbox until the very end.
        // The folder count every second (it only reads a number the sync
        // keeps), the list every three.
        let progress = Task {
            var tick = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard !Task.isCancelled else { break }
                syncStatus = mail.syncProgress()
                tick += 1
                if tick % 3 == 0 { await reload() }
            }
        }
        defer {
            progress.cancel()
            syncStatus = nil
        }
        do {
            let results = try await background { try mail.sync(accountId: nil) }
            let failed = results.compactMap { r in r.error.map { "\(r.email): \($0)" } }
            if !failed.isEmpty {
                error = failed.joined(separator: "\n\n")
            }
        } catch {
            show(error)
        }
        await reload()
    }

    func addAccount(name: String, email: String, password: String, provider: MailProvider) async -> Bool {
        if isDemo {
            await leaveDemo()
        }
        guard let mail else { return false }
        do {
            _ = try await background { try mail.addPasswordAccount(name: name, email: email, password: password, provider: provider) }
            await reload()
            Task { await sync() }
            return true
        } catch {
            show(error)
            return false
        }
    }

    /// Start Sign in with Google: the page and return scheme for the
    /// sign-in sheet. Leaves the demo first, so the account lands in the
    /// real database. `nil` (with the error shown) if it can't start.
    func startGoogleSignIn(hint: String? = nil) async -> GoogleSignIn? {
        if isDemo {
            await leaveDemo()
        }
        guard let mail, let id = GoogleClient.id else { return nil }
        do {
            return try mail.startGoogleSignIn(clientId: id, loginHint: hint)
        } catch {
            show(error)
            return nil
        }
    }

    /// Finish Sign in with Google with the link the sheet returned: adds the
    /// account (or moves one here over from an app password), then syncs.
    func finishGoogleSignIn(_ s: GoogleSignIn, redirect: URL) async -> Bool {
        guard let mail else { return false }
        do {
            _ = try await background { try mail.finishGoogleSignIn(signIn: s, redirect: redirect.absoluteString) }
            await reload()
            Task { await sync() }
            return true
        } catch {
            show(error)
            return false
        }
    }

    func removeAccount(_ a: AccountInfo) async {
        await act { try $0.removeAccount(accountId: a.id) }
    }

    /// An account's folders, standard ones first, then its own as a tree.
    func folders(_ accountId: Int64) async -> [FolderInfo] {
        guard let mail else { return [] }
        do {
            return try await background { try mail.folders(accountId: accountId) }
        } catch {
            show(error)
            return []
        }
    }

    /// Show one folder's conversations (or, with `nil`, the unified lists).
    func open(_ f: FolderInfo?) async {
        folder = f
        rows = []
        await reload()
    }

    /// What the list is called: the folder's name, or the unified list's.
    var title: String {
        folder?.name ?? mailbox.title
    }

    func account(_ id: Int64) -> AccountInfo? {
        accounts.first { $0.id == id }
    }

    // MARK: Actions on a conversation

    func archive(_ t: ThreadRow) async {
        drop(t)
        await act { try $0.archive(accountId: t.accountId, threadKey: t.threadKey) }
    }

    func trash(_ t: ThreadRow) async {
        drop(t)
        await act { try $0.trash(accountId: t.accountId, threadKey: t.threadKey) }
    }

    func snooze(_ t: ThreadRow, until: Date) async {
        drop(t)
        await act { try $0.snooze(accountId: t.accountId, threadKey: t.threadKey, until: Int64(until.timeIntervalSince1970)) }
    }

    func unsnooze(_ t: ThreadRow) async {
        drop(t)
        await act { try $0.unsnooze(accountId: t.accountId, threadKey: t.threadKey) }
    }

    func setRead(_ t: ThreadRow, _ read: Bool) async {
        change(t) { $0.unread = !read }
        await act { try $0.setRead(accountId: t.accountId, threadKey: t.threadKey, read: read) }
    }

    func setStarred(_ t: ThreadRow, _ starred: Bool) async {
        change(t) { $0.starred = starred }
        await act { try $0.setStarred(accountId: t.accountId, threadKey: t.threadKey, starred: starred) }
    }

    func setPinned(_ t: ThreadRow, _ pinned: Bool) async {
        change(t) { $0.pinned = pinned }
        await act { try $0.setPinned(accountId: t.accountId, threadKey: t.threadKey, pinned: pinned) }
    }

    // MARK: Filing: folders, labels and tabs

    /// Where a conversation is filed, and the choices, for the Move sheet.
    func filing(_ t: ThreadRow) async -> Filing? {
        guard let mail else { return nil }
        do {
            return try await background { try mail.filing(accountId: t.accountId, threadKey: t.threadKey) }
        } catch {
            show(error)
            return nil
        }
    }

    /// Move a conversation into a folder (on Gmail, a label, out of the
    /// Inbox). In a folder, only out of that folder, as on the desktop.
    func move(_ t: ThreadRow, to f: FolderInfo) async {
        guard f.id != folder?.id else { return }
        drop(t)
        let from = folder?.id
        await act { try $0.moveToFolder(accountId: t.accountId, threadKey: t.threadKey, folderId: f.id, fromFolderId: from) }
    }

    /// Add a label, or take it off.
    func setLabel(_ t: ThreadRow, _ f: FolderInfo, on: Bool) async {
        // Taking off the label being looked at takes it out of this list.
        if !on && f.id == folder?.id { drop(t) }
        await act { try $0.setLabel(accountId: t.accountId, threadKey: t.threadKey, folderId: f.id, on: on) }
    }

    /// Make a folder (a label on Gmail), or find the one called that.
    func createFolder(_ accountId: Int64, _ name: String) async -> FolderInfo? {
        guard let mail else { return nil }
        do {
            return try await background { try mail.createFolder(accountId: accountId, name: name) }
        } catch {
            show(error)
            return nil
        }
    }

    /// Put a conversation in a tab (`nil`: sort it like the rest).
    func putInTab(_ t: ThreadRow, _ c: UInt8?) async {
        if tabsShown, let tab, tab != c { drop(t) }
        await act { try $0.putInTab(accountId: t.accountId, threadKey: t.threadKey, category: c) }
    }

    /// Always put a sender's mail in a tab (`nil`: sort it automatically).
    func sortSender(_ email: String, _ c: UInt8?) async {
        await act { try $0.sortSender(email: email, category: c) }
    }

    /// Make one of your own tabs; returns its number.
    func newTab(_ name: String) async -> UInt8? {
        guard let mail else { return nil }
        do {
            let c = try await background { try mail.newTab(name: name) }
            await reload()
            return c
        } catch {
            show(error)
            return nil
        }
    }

    /// Remove one of your own tabs; its mail goes back to the others.
    func removeTab(_ c: UInt8) async {
        if tab == c { tab = nil }
        await act { try $0.removeTab(category: c) }
    }

    func messages(_ t: ThreadRow) async -> [MessageInfo] {
        guard let mail else { return [] }
        do {
            return try await background { try mail.messages(accountId: t.accountId, threadKey: t.threadKey) }
        } catch {
            show(error)
            return []
        }
    }

    // MARK: Reading

    /// An email's HTML page, or `nil` for plain text (or on error, shown).
    func messagePage(_ id: Int64, remote: Bool) async -> MessagePage? {
        guard let mail else { return nil }
        do {
            return try await background { try mail.messagePage(messageId: id, remoteImages: remote) }
        } catch {
            show(error)
            return nil
        }
    }

    /// A picture sent inside an email; `nil` if it can't be had (the web
    /// view shows it missing, which is no reason for an alert).
    func inlineImage(_ id: Int64, cid: String) async -> InlineImage? {
        guard let mail else { return nil }
        return try? await background { try mail.inlineImage(messageId: id, cid: cid) }
    }

    /// An attachment as a file on the phone, downloading it the first time.
    func attachmentFile(_ id: Int64, index: Int) async -> URL? {
        guard let mail else { return nil }
        do {
            let path = try await background { try mail.attachmentFile(messageId: id, index: UInt32(index)) }
            return URL(fileURLWithPath: path)
        } catch {
            show(error)
            return nil
        }
    }

    // MARK: Writing

    /// The composer showing now, if any.
    var composing: ComposeRequest?

    /// A new message from the first account (the demo's, in the demo).
    func newMessage(to: String = "", cc: String = "", subject: String = "", body: String = "") {
        guard let account = accounts.first else { return }
        let html = body.isEmpty ? "<div><br></div>" : body.split(separator: "\n", omittingEmptySubsequences: false)
            .map { "<div>\($0.isEmpty ? "<br>" : escapeHTML(String($0)))</div>" }.joined()
        composing = ComposeRequest(draft: Draft(accountId: account.id, to: to, cc: cc, bcc: "", subject: subject, html: html,
                                                inReplyTo: nil, references: [], attachments: [], replaces: nil))
    }

    /// A mailto: link tapped in an email.
    func composeMailto(_ url: URL) {
        let parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let query = { (name: String) in parts?.queryItems?.first { $0.name.lowercased() == name }?.value ?? "" }
        let to = (parts?.path.removingPercentEncoding ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        newMessage(to: (to + [query("to")]).filter { !$0.isEmpty }.joined(separator: ", "), cc: query("cc"), subject: query("subject"), body: query("body"))
    }

    /// Reply, reply all or forward the email `id`.
    func startCompose(_ id: Int64, _ kind: ComposeKind) async {
        guard let mail else { return }
        do {
            let d = try await background { try mail.startCompose(messageId: id, kind: kind) }
            composing = ComposeRequest(draft: d)
        } catch {
            show(error)
        }
    }

    /// Carry on writing a saved draft (the newest email in `row`).
    func openDraft(_ row: ThreadRow) async {
        guard let mail, let last = await messages(row).last else { return }
        do {
            let d = try await background { try mail.openDraft(messageId: last.id) }
            composing = ComposeRequest(draft: d)
        } catch {
            show(error)
        }
    }

    /// Save to the server's Drafts; returns the saved draft's id. Throws,
    /// so the composer can say what went wrong while it's still open.
    func saveDraft(_ d: Draft) async throws -> Int64 {
        guard let mail else { throw MailError.Failed(message: "OwlPost isn't open yet") }
        let id = try await background { try mail.saveDraft(draft: d) }
        Task { await reload() }
        return id
    }

    func send(_ d: Draft) async throws {
        guard let mail else { throw MailError.Failed(message: "OwlPost isn't open yet") }
        try await background { try mail.send(draft: d) }
        Task { await reload() }
    }

    func deleteDraft(_ id: Int64) async {
        await act { try $0.deleteDraft(messageId: id) }
    }

    /// People to suggest for an address field: from mail you've had, then
    /// the iPhone's contacts (if you've allowed it).
    func suggestions(_ prefix: String) async -> [ContactSuggestion] {
        guard let mail else { return [] }
        let fromMail = (try? await background { try mail.suggestContacts(prefix: prefix) }) ?? []
        // The demo's people are made up: don't ask for the phone's contacts
        // (or mix real ones in) just to try it.
        let fromPhone = isDemo ? [] : await PhoneContacts.shared.search(prefix)
        var seen = Set<String>()
        return (fromMail + fromPhone).filter { seen.insert($0.email.lowercased()).inserted }.prefix(8).map { $0 }
    }

    // MARK: Helpers

    private func drop(_ t: ThreadRow) {
        rows.removeAll { $0.accountId == t.accountId && $0.threadKey == t.threadKey }
    }

    private func change(_ t: ThreadRow, _ edit: (inout ThreadRow) -> Void) {
        if let i = rows.firstIndex(where: { $0.accountId == t.accountId && $0.threadKey == t.threadKey }) {
            edit(&rows[i])
        }
    }

    /// Run an action in the core, then reload so the list matches it.
    private func act(_ work: @escaping @Sendable (Mail) throws -> Void) async {
        guard let mail else { return }
        do {
            try await background { try work(mail) }
        } catch {
            show(error)
        }
        await reload()
    }

    private func show(_ e: Error) {
        if case let MailError.Failed(message) = e {
            error = message
        } else {
            error = e.localizedDescription
        }
    }
}

/// A composer to show: a draft and an identity for the sheet.
struct ComposeRequest: Identifiable {
    let id = UUID()
    var draft: Draft
}

func escapeHTML(_ s: String) -> String {
    s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;").replacingOccurrences(of: ">", with: "&gt;")
}

/// Run blocking core work off the main thread.
func background<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await Task.detached(priority: .userInitiated) { try work() }.value
}
