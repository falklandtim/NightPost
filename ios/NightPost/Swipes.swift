// Swipes on a conversation in the list, chosen in Settings → Swipes with
// the same choices and defaults as the desktop's Preferences → Message list:
// up to three buttons a side, the first being what a full swipe does, and
// Nothing first to turn that side off. Kept on this phone, as each computer
// keeps its own (the keys match the desktop's, so the two read alike).

import SwiftUI

/// One thing a swipe can do.
enum SwipeAction: String, CaseIterable, Identifiable {
    case archive, trash, snooze, move, more, pin, read, star, spam, reply
    case none

    var id: String { rawValue }

    /// The name in Settings, as the desktop's Preferences words it.
    var title: String {
        switch self {
        case .archive: "Archive"
        case .trash: "Move to Trash"
        case .snooze: "Snooze"
        case .move: "Move to…"
        case .more: "More…"
        case .pin: "Pin / unpin"
        case .read: "Mark read / unread"
        case .star: "Star / unstar"
        case .spam: "Report as spam"
        case .reply: "Reply"
        case .none: "Nothing"
        }
    }
}

/// The buttons on one side, outermost first, saved as `trash+snooze+more`.
struct SwipeSide: Equatable {
    var actions: [SwipeAction]

    static let leftDefault = SwipeSide(actions: [.trash, .snooze, .more])
    static let rightDefault = SwipeSide(actions: [.archive, .read])

    /// A side from its saved form. Anything unreadable falls back to the
    /// default, so a setting saved by a later build never leaves a side
    /// broken. Repeats and Nothing after the first are dropped, as on the
    /// desktop; Nothing first turns the side off.
    init(saved: String, default d: SwipeSide) {
        let parsed = saved.split(separator: "+").compactMap { SwipeAction(rawValue: String($0)) }
        guard let first = parsed.first else {
            self = d
            return
        }
        // The later buttons are kept while the side is off, as on the
        // desktop, so turning it back on brings them back.
        var acts = [first]
        for a in parsed.dropFirst() where a != .none && !acts.contains(a) && acts.count < 3 {
            acts.append(a)
        }
        actions = acts
    }

    init(actions: [SwipeAction]) {
        self.actions = actions
    }

    var saved: String { actions.map(\.rawValue).joined(separator: "+") }

    /// What the row shows: nothing at all when the first is Nothing.
    var shown: [SwipeAction] { actions.first == SwipeAction.none ? [] : actions }

    /// The button in `slot` (0 to 2), Nothing for an empty one.
    func at(_ slot: Int) -> SwipeAction { actions.indices.contains(slot) ? actions[slot] : .none }

    /// Put `a` in `slot`, keeping the side as the desktop would save it.
    func setting(_ slot: Int, to a: SwipeAction) -> SwipeSide {
        var acts = (0..<3).map(at)
        acts[slot] = a
        // Nothing in the first slot turns the side off; later slots close up.
        return SwipeSide(saved: SwipeSide(actions: acts).saved, default: self)
    }

    static let leftKey = "swipeLeft"
    static let rightKey = "swipeRight"
}

/// Settings → Swipes: three pickers a side.
struct SwipeSettingsView: View {
    @AppStorage(SwipeSide.rightKey) private var right = SwipeSide.rightDefault.saved
    @AppStorage(SwipeSide.leftKey) private var left = SwipeSide.leftDefault.saved

    var body: some View {
        Form {
            side("Swipe Right", $right, SwipeSide.rightDefault)
            side("Swipe Left", $left, SwipeSide.leftDefault)
            Section {
                Button("Use the Desktop's Defaults") {
                    right = SwipeSide.rightDefault.saved
                    left = SwipeSide.leftDefault.saved
                }
            } footer: {
                Text("A short swipe shows the buttons; a full swipe does the first one. Set the first to Nothing to turn that side off. These are kept on this iPhone: each computer has its own in Preferences → Message list.")
            }
        }
        .navigationTitle("Swipes")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func side(_ title: String, _ saved: Binding<String>, _ d: SwipeSide) -> some View {
        let s = SwipeSide(saved: saved.wrappedValue, default: d)
        return Section(title) {
            ForEach(0..<3, id: \.self) { slot in
                Picker(["Full Swipe", "Button 2", "Button 3"][slot], selection: Binding(
                    get: { s.at(slot) },
                    set: { saved.wrappedValue = s.setting(slot, to: $0).saved }
                )) {
                    ForEach(SwipeAction.allCases) { Text($0.title).tag($0) }
                }
                // Later buttons mean nothing while the side is off.
                .disabled(slot > 0 && s.shown.isEmpty)
                .accessibilityIdentifier("\(title) \(slot + 1)")
            }
        }
    }
}

/// The swipe buttons for one conversation, and what each does. The list
/// owns the sheets (snooze, move, more) that some of them open.
struct SwipeButtons: View {
    @Environment(MailModel.self) private var model
    let side: SwipeSide
    let row: ThreadRow
    /// In Snoozed, Snooze becomes Unsnooze (the conversation is snoozed
    /// already; open it to pick a new time).
    let inSnoozed: Bool
    let snooze: (ThreadRow) -> Void
    let move: (ThreadRow) -> Void
    let more: (ThreadRow) -> Void

    var body: some View {
        ForEach(side.shown) { a in
            button(a)
        }
    }

    @ViewBuilder
    private func button(_ a: SwipeAction) -> some View {
        let t = row
        switch a {
        case .archive:
            Button { Task { await model.archive(t) } } label: { Label("Archive", systemImage: "archivebox") }
                .tint(.green)
        case .trash:
            Button(role: .destructive) { Task { await model.trash(t) } } label: { Label("Trash", systemImage: "trash") }
        case .snooze where inSnoozed:
            Button { Task { await model.unsnooze(t) } } label: { Label("Unsnooze", systemImage: "tray.and.arrow.down") }
                .tint(.blue)
        case .snooze:
            Button { snooze(t) } label: { Label("Snooze", systemImage: "clock") }
                .tint(.orange)
        case .move:
            Button { move(t) } label: { Label("Move", systemImage: "folder") }
                .tint(.indigo)
        case .more:
            Button { more(t) } label: { Label("More", systemImage: "ellipsis.circle") }
                .tint(.gray)
        case .pin:
            Button { Task { await model.setPinned(t, !t.pinned) } } label: {
                Label(t.pinned ? "Unpin" : "Pin", systemImage: t.pinned ? "pin.slash" : "pin")
            }
            .tint(.yellow)
        case .read:
            Button { Task { await model.setRead(t, t.unread) } } label: {
                Label(t.unread ? "Read" : "Unread", systemImage: t.unread ? "envelope.open" : "envelope.badge")
            }
            .tint(.blue)
        case .star:
            Button { Task { await model.setStarred(t, !t.starred) } } label: {
                Label(t.starred ? "Unstar" : "Star", systemImage: t.starred ? "star.slash" : "star")
            }
            .tint(.yellow)
        case .spam:
            Button { Task { await model.spam(t) } } label: { Label("Spam", systemImage: "xmark.bin") }
                .tint(.orange)
        case .reply:
            Button { Task { await model.reply(t) } } label: { Label("Reply", systemImage: "arrowshape.turn.up.left") }
                .tint(.teal)
        case .none:
            EmptyView()
        }
    }
}

/// The More swipe: the press-and-hold actions, in a small sheet. (A
/// confirmation dialog on the list left a frosted layer over its top rows
/// on iOS 26, so it's a sheet, like Snooze.)
struct RowActionsSheet: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let row: ThreadRow
    @State private var moving = false

    var body: some View {
        let t = row
        NavigationStack {
            List {
                Button { moving = true } label: { Label("Move or Label…", systemImage: "folder") }
                Button { act { await model.setPinned(t, !t.pinned) } } label: {
                    Label(t.pinned ? "Unpin" : "Pin", systemImage: t.pinned ? "pin.slash" : "pin")
                }
                Button { act { await model.setStarred(t, !t.starred) } } label: {
                    Label(t.starred ? "Unstar" : "Star", systemImage: t.starred ? "star.slash" : "star")
                }
                Button { act { await model.setRead(t, t.unread) } } label: {
                    Label(t.unread ? "Mark as Read" : "Mark as Unread", systemImage: t.unread ? "envelope.open" : "envelope.badge")
                }
            }
            .navigationTitle(t.subject.isEmpty ? "(no subject)" : t.subject)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .sheet(isPresented: $moving) {
            // Moved or not, this sheet's done once the Move sheet closes.
            MoveSheet(row: t)
                .onDisappear { dismiss() }
        }
    }

    /// Do it, and close the sheet.
    private func act(_ work: @escaping () async -> Void) {
        dismiss()
        Task { await work() }
    }
}
