// NightPost for iPhone: the app's entry point.
//
// The mail itself lives in the Rust core (crates/nightpost-ffi, through the
// generated Swift in Generated/). This app is the interface: MailModel holds
// what the screens show and runs every core call off the main thread.

import SwiftUI

@main
struct NightPostApp: App {
    @State private var model = MailModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task { await model.start() }
        }
    }
}

/// The inbox once there's an account (or the demo), otherwise the welcome.
struct RootView: View {
    @Environment(MailModel.self) private var model
    @AppStorage(Appearance.key) private var appearance: Appearance = .system

    var body: some View {
        @Bindable var model = model
        Group {
            if !model.started {
                ProgressView()
            } else if model.accounts.isEmpty {
                WelcomeView()
            } else {
                NavigationStack {
                    InboxView()
                }
            }
        }
        .preferredColorScheme(appearance.scheme)
        // The composer, from anywhere: a reply, a draft, a mailto: link.
        .sheet(item: $model.composing) { request in
            ComposeView(request: request)
                .environment(model)
                .preferredColorScheme(appearance.scheme)
        }
        .alert("Something went wrong", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.error ?? "")
        }
    }
}
