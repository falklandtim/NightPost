// Settings: accounts, appearance, and what the phone shares with the
// computers.

import SwiftUI

/// Light, dark, or whatever the phone is set to. Kept per phone (not
/// synced), like the desktop's theme choice.
enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: "Match iPhone"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    /// `nil` follows the phone's own setting.
    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }

    static let key = "appearance"
}

struct SettingsView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @AppStorage(Appearance.key) private var appearance: Appearance = .system
    @State private var adding = false
    @State private var removing: AccountInfo?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ForEach(model.accounts, id: \.id) { a in
                        HStack(spacing: 12) {
                            Circle().fill(Color(hex: a.color)).frame(width: 10, height: 10)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(a.name)
                                Text(a.email).font(.footnote).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if a.demo {
                                Text("Demo").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        .swipeActions {
                            Button("Remove", role: .destructive) { removing = a }
                        }
                        .contextMenu {
                            Button("Remove", systemImage: "trash", role: .destructive) { removing = a }
                        }
                    }
                    Button { adding = true } label: { Label("Add Account", systemImage: "plus") }
                } header: {
                    Text("Accounts")
                } footer: {
                    Text("Swipe left on an account to remove it from this iPhone. The mail stays on the server.")
                }

                Section("Appearance") {
                    Picker("Appearance", selection: $appearance) {
                        ForEach(Appearance.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }

                if model.isDemo {
                    Section {
                        Button("Leave the Demo") {
                            dismiss()
                            Task { await model.leaveDemo() }
                        }
                    } footer: {
                        Text("You're looking at made-up demo accounts. Nothing here reaches a mail server.")
                    }
                }

                Section {
                    LabeledContent("Version", value: version)
                } header: {
                    Text("About")
                } footer: {
                    Text("Snoozes, pins and inbox tabs are kept on your mail server, so the iPhone and your computers running OwlPost agree. Passwords stay in this iPhone's keychain.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .sheet(isPresented: $adding) { AddAccountView() }
            .confirmationDialog(
                "Remove \(removing?.email ?? "")?",
                isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                titleVisibility: .visible
            ) {
                Button("Remove from iPhone", role: .destructive) {
                    if let a = removing {
                        Task { await model.removeAccount(a) }
                    }
                    removing = nil
                }
            } message: {
                Text("Its mail and password are removed from this iPhone. Nothing is deleted on the server.")
            }
        }
        .preferredColorScheme(appearance.scheme)
    }

    private var version: String {
        let info = Bundle.main.infoDictionary
        let v = info?["CFBundleShortVersionString"] as? String ?? "?"
        let b = info?["CFBundleVersion"] as? String ?? "?"
        return "\(v) (\(b))"
    }
}
