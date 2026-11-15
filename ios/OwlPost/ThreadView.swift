// One conversation: each email as a card, oldest first. HTML email shows
// in a sandboxed web view (MessageBodyView), plain text as text.
// Attachments open in Quick Look, where they can be shared or saved to
// Files. Reply, Reply All and Forward answer the newest email. The folder
// button files it (MoveSheet): a folder, labels, an Inbox tab.

import QuickLook
import SwiftUI

struct ThreadView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let row: ThreadRow
    @State private var messages: [MessageInfo] = []
    @State private var starred: Bool
    @State private var pinned: Bool
    @State private var snoozing = false
    @State private var moving = false

    init(row: ThreadRow) {
        self.row = row
        _starred = State(initialValue: row.starred)
        _pinned = State(initialValue: row.pinned)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text(row.subject.isEmpty ? "(no subject)" : row.subject)
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal)
                ForEach(messages, id: \.id) { m in
                    MessageCard(message: m, collapsed: m.id != messages.last?.id && !m.unread)
                }
            }
            .padding(.vertical)
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .bottomBar) {
                Button { act { await model.archive(row) } } label: { Label("Archive", systemImage: "archivebox") }
                Spacer()
                Button { moving = true } label: { Label("Move or Label", systemImage: "folder") }
                Spacer()
                Menu {
                    Button { compose(.reply) } label: { Label("Reply", systemImage: "arrowshape.turn.up.left") }
                    Button { compose(.replyAll) } label: { Label("Reply All", systemImage: "arrowshape.turn.up.left.2") }
                    Button { compose(.forward) } label: { Label("Forward", systemImage: "arrowshape.turn.up.right") }
                } label: {
                    Label("Reply", systemImage: "arrowshape.turn.up.left")
                } primaryAction: {
                    compose(.reply)
                }
                Spacer()
                Button { snoozing = true } label: { Label("Snooze", systemImage: "clock") }
                Spacer()
                Button {
                    starred.toggle()
                    Task { await model.setStarred(row, starred) }
                } label: { Label("Star", systemImage: starred ? "star.fill" : "star") }
                Spacer()
                Button {
                    pinned.toggle()
                    Task { await model.setPinned(row, pinned) }
                } label: { Label("Pin", systemImage: pinned ? "pin.fill" : "pin") }
                Spacer()
                Button(role: .destructive) { act { await model.trash(row) } } label: { Label("Trash", systemImage: "trash") }
            }
        }
        .sheet(isPresented: $snoozing) {
            SnoozeSheet { date in
                snoozing = false
                act { await model.snooze(row, until: date) }
            }
            .presentationDetents([.medium])
        }
        .sheet(isPresented: $moving) {
            // A move takes the conversation out of the list: go back to it.
            MoveSheet(row: row) { dismiss() }
        }
        .task {
            messages = await model.messages(row)
            if row.unread {
                await model.setRead(row, true)
            }
        }
    }

    private func compose(_ kind: ComposeKind) {
        guard let last = messages.last else { return }
        Task { await model.startCompose(last.id, kind) }
    }

    /// Do something that takes the conversation out of the list, and go back.
    private func act(_ work: @escaping () async -> Void) {
        dismiss()
        Task { await work() }
    }
}

struct MessageCard: View {
    @Environment(MailModel.self) private var model
    let message: MessageInfo
    @State private var collapsed: Bool
    @State private var preview: URL?
    @State private var fetching: Int?

    init(message: MessageInfo, collapsed: Bool) {
        self.message = message
        _collapsed = State(initialValue: collapsed)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if !collapsed {
                if message.hasHtml {
                    MessageBodyView(message: message)
                } else {
                    Text(message.bodyText)
                        .font(.body)
                        .textSelection(.enabled)
                }
                ForEach(Array(message.attachments.enumerated()), id: \.offset) { i, a in
                    Button { open(i) } label: {
                        HStack {
                            Image(systemName: "paperclip")
                            Text(a.filename).lineLimit(1)
                            Text(ByteCountFormatter.string(fromByteCount: Int64(a.size), countStyle: .file)).foregroundStyle(.secondary)
                            Spacer()
                            if fetching == i { ProgressView().controlSize(.small) }
                        }
                        .font(.footnote)
                        .padding(8)
                        .background(.background.tertiary, in: RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                    .disabled(fetching != nil)
                }
            }
        }
        .padding(12)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal)
        .quickLookPreview($preview)
    }

    /// Download (the first time) and show attachment `i`.
    private func open(_ i: Int) {
        fetching = i
        Task {
            preview = await model.attachmentFile(message.id, index: i)
            fetching = nil
        }
    }

    /// Who, when, to whom; tap to fold or unfold. Only the header folds, so
    /// a tap on a link in the email never does.
    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            Avatar(name: message.fromName, email: message.fromEmail, size: 32)
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(message.fromName.isEmpty ? message.fromEmail : message.fromName)
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)
                    Spacer()
                    Text(shortDate(message.date))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if collapsed {
                    Text(firstLine(message.bodyText))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else if !message.recipients.isEmpty {
                    Text("to " + message.recipients.joined(separator: ", "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .contentShape(Rectangle())
        .onTapGesture {
            withAnimation(.snappy) { collapsed.toggle() }
        }
    }

    private func firstLine(_ s: String) -> String {
        s.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
    }
}
