// The Inbox tabs above the list, as on the desktop: All, Primary,
// Promotions, Social and your own (Invoices), each with its unread count.
// Swipe sideways when they don't fit. Press and hold your own tab to remove
// it; its mail goes back to the other tabs.

import SwiftUI

struct TabStrip: View {
    @Environment(MailModel.self) private var model
    @State private var removing: InboxTab?

    var body: some View {
        ScrollViewReader { scroll in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(model.tabs, id: \.name) { t in
                        chip(t)
                            .id(t.name)
                    }
                }
                .padding(.horizontal)
                .padding(.vertical, 6)
            }
            .onChange(of: model.tab) {
                if let t = model.tabs.first(where: { $0.category == model.tab }) {
                    withAnimation { scroll.scrollTo(t.name, anchor: .center) }
                }
            }
        }
        .confirmationDialog("Remove the “\(removing?.name ?? "")” tab?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible) {
            Button("Remove Tab", role: .destructive) {
                if let c = removing?.category { Task { await model.removeTab(c) } }
            }
        } message: {
            Text("Its mail goes back to Primary, Promotions and Social. Your other devices lose the tab too.")
        }
    }

    private func chip(_ t: InboxTab) -> some View {
        let on = t.category == model.tab
        return Button {
            Task { await model.pick(tab: t.category) }
        } label: {
            HStack(spacing: 5) {
                Text(t.name)
                if t.unread > 0 {
                    Text(t.unread >= 500 ? "500+" : "\(t.unread)")
                        .font(.caption2.weight(.semibold).monospacedDigit())
                        .foregroundStyle(on ? Color.accentColor : .secondary)
                }
            }
            .font(.subheadline.weight(on ? .semibold : .regular))
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(on ? Color.accentColor.opacity(0.15) : Color.secondary.opacity(0.10), in: Capsule())
            .foregroundStyle(on ? Color.accentColor : .primary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(t.unread > 0 ? "\(t.name), \(t.unread) unread" : t.name)
        .accessibilityAddTraits(on ? .isSelected : [])
        .contextMenu {
            if t.own {
                Button(role: .destructive) { removing = t } label: { Label("Remove Tab", systemImage: "minus.circle") }
            }
        }
    }
}
