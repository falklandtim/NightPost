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
    var search = ""
    var error: String?
    private(set) var syncing = false
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
        let (box, query) = (mailbox, search)
        do {
            let (a, r) = try await background { (try mail.accounts(), try mail.threads(mailbox: box, query: query, limit: 300)) }
            accounts = a
            // Another list may have been picked meanwhile; keep the newest.
            if box == mailbox && query == search {
                rows = r
            }
        } catch {
            show(error)
        }
    }

    /// Fetch new mail for every account.
    func sync() async {
        guard let mail, !syncing else { return }
        syncing = true
        defer { syncing = false }
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

    func removeAccount(_ a: AccountInfo) async {
        await act { try $0.removeAccount(accountId: a.id) }
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

    func messages(_ t: ThreadRow) async -> [MessageInfo] {
        guard let mail else { return [] }
        do {
            return try await background { try mail.messages(accountId: t.accountId, threadKey: t.threadKey) }
        } catch {
            show(error)
            return []
        }
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

/// Run blocking core work off the main thread.
func background<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await Task.detached(priority: .userInitiated) { try work() }.value
}
