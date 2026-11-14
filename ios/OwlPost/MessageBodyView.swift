// An HTML email inside its conversation card.
//
// The page comes from the core (crates/owlpost-ffi/src/reader.rs): tracking
// pixels stripped, a strict Content Security Policy, nothing remote until
// "Load images". The web view runs no JavaScript from the email; the app's
// own measuring script still runs (WebKit allows the app's scripts with
// content JavaScript off). Links leave the page: web links open in Safari,
// mailto: links open the composer. Pictures sent inside the email (cid:)
// are fetched through the core.

import SwiftUI
import WebKit

struct MessageBodyView: View {
    @Environment(MailModel.self) private var model
    let message: MessageInfo
    @State private var page: MessagePage?
    @State private var remote = false
    @State private var height: CGFloat = 80

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let page {
                if page.remoteHidden || !page.trackers.isEmpty {
                    PrivacyBanner(page: page) { remote = true }
                }
                HTMLView(model: model, html: page.html, messageId: message.id, height: $height)
                    .frame(height: height)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 60)
            }
        }
        .task(id: remote) {
            page = await model.messagePage(message.id, remote: remote)
        }
    }
}

/// "Remote images are hidden", with a button to show them, and which
/// trackers were blocked.
private struct PrivacyBanner: View {
    let page: MessagePage
    let loadImages: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !page.trackers.isEmpty {
                Label("Blocked \(page.trackers.count == 1 ? "a tracker" : "trackers") from \(hosts)", systemImage: "shield.lefthalf.filled")
            }
            if page.remoteHidden {
                HStack {
                    Text("Remote images are hidden to protect your privacy.")
                    Spacer(minLength: 4)
                    Button("Load images", action: loadImages)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
            }
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .padding(10)
        .background(.background.tertiary, in: RoundedRectangle(cornerRadius: 8))
    }

    private var hosts: String {
        switch page.trackers.count {
        case 1: page.trackers[0]
        case 2: "\(page.trackers[0]) and \(page.trackers[1])"
        default: "\(page.trackers[0]), \(page.trackers[1]) and \(page.trackers.count - 2) more"
        }
    }
}

/// The web view itself: sized to its content, never scrolling on its own
/// (the conversation scrolls), wide emails scaled down to fit.
struct HTMLView: UIViewRepresentable {
    let model: MailModel
    let html: String
    let messageId: Int64
    @Binding var height: CGFloat

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // The email's own scripts never run (its policy blocks them too).
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.setURLSchemeHandler(context.coordinator.pictures, forURLScheme: "cid")
        config.dataDetectorTypes = [.link, .phoneNumber, .address, .calendarEvent]
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator
        web.scrollView.isScrollEnabled = false
        web.scrollView.bounces = false
        web.isOpaque = false
        web.backgroundColor = .white
        context.coordinator.watch(web)
        web.loadHTMLString(html, baseURL: nil)
        context.coordinator.loaded = html
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        context.coordinator.parent = self
        // "Load images" gives a new page.
        if context.coordinator.loaded != html {
            context.coordinator.loaded = html
            web.loadHTMLString(html, baseURL: nil)
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var parent: HTMLView
        var loaded = ""
        let pictures: InlinePictures
        private var sizeWatch: NSKeyValueObservation?

        init(_ parent: HTMLView) {
            self.parent = parent
            pictures = InlinePictures(model: parent.model, messageId: parent.messageId)
        }

        /// Follow the page's height as it lays out and pictures arrive.
        func watch(_ web: WKWebView) {
            sizeWatch = web.scrollView.observe(\.contentSize, options: [.new]) { [weak self] _, change in
                guard let h = change.newValue?.height, h > 0 else { return }
                Task { @MainActor in
                    if abs((self?.parent.height ?? 0) - h) > 1 { self?.parent.height = h }
                }
            }
        }

        func webView(_ web: WKWebView, didFinish navigation: WKNavigation!) {
            // Newsletters are often 600px wide: scale them to the card, as
            // Mail does, rather than cut them off.
            let width = web.bounds.width
            web.evaluateJavaScript("document.documentElement.scrollWidth") { result, _ in
                guard let content = result as? CGFloat, width > 0, content > width + 1 else { return }
                web.evaluateJavaScript("document.body.style.zoom = '\(width / content)'")
            }
        }

        func webView(_ web: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void) {
            guard let url = action.request.url else { return decisionHandler(.cancel) }
            // The page itself, and the email's own frames.
            if url.absoluteString == "about:blank" || url.scheme == "about" || (action.navigationType == .other && url.scheme == "data") {
                return decisionHandler(.allow)
            }
            decisionHandler(.cancel)
            // Only what the reader tapped leaves the page.
            guard action.navigationType == .linkActivated else { return }
            switch url.scheme?.lowercased() {
            case "mailto":
                parent.model.composeMailto(url)
            case "http", "https", "tel", "sms", "facetime", "maps":
                UIApplication.shared.open(url)
            default:
                break
            }
        }
    }
}

/// Pictures sent inside an email (`cid:` links), fetched through the core.
@MainActor
final class InlinePictures: NSObject, WKURLSchemeHandler {
    private let model: MailModel
    private let messageId: Int64
    /// Requests the web view has given up on: answering one would crash.
    private var stopped: Set<ObjectIdentifier> = []

    init(model: MailModel, messageId: Int64) {
        self.model = model
        self.messageId = messageId
    }

    func webView(_ web: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url else { return task.didFailWithError(URLError(.badURL)) }
        let cid = (url.absoluteString.dropFirst("cid:".count)).removingPercentEncoding ?? ""
        let key = ObjectIdentifier(task)
        Task {
            let picture = await model.inlineImage(messageId, cid: cid)
            guard !stopped.contains(key) else { return }
            if let picture {
                task.didReceive(URLResponse(url: url, mimeType: picture.mimeType, expectedContentLength: picture.data.count, textEncodingName: nil))
                task.didReceive(picture.data)
                task.didFinish()
            } else {
                task.didFailWithError(URLError(.resourceUnavailable))
            }
        }
    }

    func webView(_ web: WKWebView, stop task: any WKURLSchemeTask) {
        stopped.insert(ObjectIdentifier(task))
    }
}
