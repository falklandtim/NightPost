// Sign in with Google: Gmail and Google Workspace accounts, with no
// password to make.
//
// Apple's sign-in sheet (ASWebAuthenticationSession) shows Google's page and
// closes when Google returns to the app through the client ID reversed; the
// core checks that link and redeems it (crates/nightpost-ffi/src/google.rs).
// The client ID is built in by scripts/ios-rust.sh; without one the button
// isn't shown.

import AuthenticationServices
import SwiftUI

struct GoogleSignInButton: View {
    @Environment(MailModel.self) private var model
    @Environment(\.webAuthenticationSession) private var webAuth
    /// Pre-fills the address, when signing an account in again.
    var hint: String?
    /// Called once the account is added.
    var done: () -> Void = {}
    @State private var working = false

    var body: some View {
        if GoogleClient.id != nil {
            Button {
                working = true
                Task {
                    if await signIn() { done() }
                    working = false
                }
            } label: {
                HStack {
                    if working { ProgressView() } else { Image(systemName: "person.badge.key") }
                    Text("Sign in with Google")
                }
                .frame(maxWidth: .infinity)
            }
            .disabled(working)
        }
    }

    private func signIn() async -> Bool {
        guard let s = await model.startGoogleSignIn(hint: hint), let url = URL(string: s.url()) else { return false }
        let back: URL
        do {
            back = try await webAuth.authenticate(using: url, callbackURLScheme: s.callbackScheme())
        } catch ASWebAuthenticationSessionError.canceledLogin {
            // Closing the sheet isn't an error.
            return false
        } catch {
            model.error = error.localizedDescription
            return false
        }
        return await model.finishGoogleSignIn(s, redirect: back)
    }
}
