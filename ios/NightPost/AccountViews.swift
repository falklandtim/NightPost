// Adding an account, and the welcome screen before there is one.
//
// Gmail and Google Workspace use Sign in with Google (GoogleSignInButton);
// other accounts sign in with an app-specific password. Gmail and iCloud
// addresses pick their provider by themselves; an address on your own
// domain (Google Workspace, iCloud custom domain) needs it picked.

import SwiftUI

struct WelcomeView: View {
    @Environment(MailModel.self) private var model
    @State private var adding = false

    var body: some View {
        VStack(spacing: 20) {
            Spacer()
            Image(systemName: "envelope.open")
                .font(.system(size: 56))
                .foregroundStyle(.tint)
            Text("NightPost")
                .font(.largeTitle.weight(.bold))
            Text("Your iCloud and Gmail accounts together, with snoozes and pins that follow you from your computers.")
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 32)
            Spacer()
            GoogleSignInButton()
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            Button { adding = true } label: {
                Text(GoogleClient.id == nil ? "Add an Account" : "Add Another Account").frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .controlSize(.large)
            Button("Try the Demo") { Task { await model.openDemo() } }
                .padding(.bottom)
        }
        .padding()
        .sheet(isPresented: $adding) { AddAccountView() }
    }
}

struct AddAccountView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var email = ""
    @State private var password = ""
    @State private var provider: MailProvider = .detect
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                if GoogleClient.id != nil {
                    Section {
                        GoogleSignInButton { dismiss() }
                    } footer: {
                        Text("For Gmail and Google Workspace. No password to make.")
                    }
                }
                Section {
                    TextField("Your name", text: $name)
                        .textContentType(.name)
                    TextField("Email address", text: $email)
                        .textContentType(.emailAddress)
                        .keyboardType(.emailAddress)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Picker("Provider", selection: $provider) {
                        ForEach(MailProvider.choices, id: \.self) { Text($0.title(for: email)).tag($0) }
                    }
                    SecureField("App-specific password", text: $password)
                        .textContentType(.password)
                } footer: {
                    Text(help)
                }
            }
            .navigationTitle("Add Account")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if working {
                        ProgressView()
                    } else {
                        Button("Add") {
                            working = true
                            Task {
                                if await model.addAccount(name: name, email: email, password: password, provider: provider) {
                                    dismiss()
                                }
                                working = false
                            }
                        }
                        .disabled(!email.contains("@") || password.isEmpty || (provider == .detect && MailProvider.guess(email) == nil))
                    }
                }
            }
            .interactiveDismissDisabled(working)
        }
    }

    /// Where to get an app password, for the provider chosen or guessed.
    private var help: String {
        let chosen = provider == .detect ? MailProvider.guess(email) : provider
        switch chosen {
        case .iCloud:
            return "Make an app-specific password at account.apple.com, under Sign-In and Security."
        case .google:
            return "Make an app password at myaccount.google.com/apppasswords, signed in as this address. It needs 2-Step Verification turned on. Sign in with Google above needs no password."
        case .none where email.contains("@"):
            return "For an address on your own domain, pick who runs its mail: Google for Google Workspace."
        default:
            return "Your password is kept in this iPhone's keychain."
        }
    }
}

extension MailProvider {
    /// In the picker's order.
    static let choices: [MailProvider] = [.detect, .google, .iCloud, .outlook, .yahoo, .fastmail]

    /// The provider an address's domain gives away, as the core guesses it;
    /// `nil` for your own domain.
    static func guess(_ email: String) -> MailProvider? {
        let domain = email.split(separator: "@").last.map { $0.lowercased() } ?? ""
        switch domain {
        case "gmail.com", "googlemail.com": return .google
        case "icloud.com", "me.com", "mac.com": return .iCloud
        case "outlook.com", "hotmail.com", "live.com", "msn.com": return .outlook
        case "yahoo.com", "ymail.com", "rocketmail.com": return .yahoo
        case "fastmail.com", "fastmail.fm": return .fastmail
        default: return nil
        }
    }

    /// "Automatic (iCloud)" once the address gives it away.
    func title(for email: String) -> String {
        switch self {
        case .detect:
            if let g = MailProvider.guess(email) { return "Automatic (\(g.title(for: "")))" }
            return "Automatic"
        case .google: return "Google (Gmail, Workspace)"
        case .iCloud: return "iCloud"
        case .outlook: return "Outlook"
        case .yahoo: return "Yahoo"
        case .fastmail: return "Fastmail"
        }
    }
}
