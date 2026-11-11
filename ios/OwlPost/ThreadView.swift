// One conversation: each email as a card, oldest first, in plain text.
// HTML email comes later, in a sandboxed WKWebView (see docs/ios.md).

import SwiftUI

struct ThreadView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let row: ThreadRow
    @State private var messages: [MessageInfo] = []
    @State private var starred: Bool
    @State private var pinned: Bool
    @State private var snoozing = false

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
        .task {
            messages = await model.messages(row)
            if row.unread {
                await model.setRead(row, true)
            }
        }
    }

    /// Do something that takes the conversation out of the list, and go back.
    private func act(_ work: @escaping () async -> Void) {
        dismiss()
        Task { await work() }
    }
}

struct MessageCard: View {
    let message: MessageInfo
    @State private var collapsed: Bool

    init(message: MessageInfo, collapsed: Bool) {
        self.message = message
        _collapsed = State(initialValue: collapsed)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
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
            if !collapsed {
                Text(message.bodyText)
                    .font(.body)
                    .textSelection(.enabled)
                if message.hasHtml && message.bodyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text("This email is HTML only. Formatted email isn't shown on the phone yet.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                ForEach(message.attachments, id: \.self) { a in
                    Label("\(a.filename) · \(ByteCountFormatter.string(fromByteCount: Int64(a.size), countStyle: .file))", systemImage: "paperclip")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(12)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal)
        .contentShape(Rectangle())
        .onTapGesture {
            withAnimation(.snappy) { collapsed.toggle() }
        }
    }

    private func firstLine(_ s: String) -> String {
        s.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
    }
}
