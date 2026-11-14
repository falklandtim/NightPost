// The composer's body: formatted text in a web view, as on the desktop.
//
// The page is a single editable block under a policy that runs no scripts
// and loads nothing remote; the app drives it with its own scripts (bold,
// lists, links) and reads the HTML back to save or send. Quoted text and
// reopened drafts arrive already defanged by the core (mail_html.rs).

import SwiftUI
import WebKit

/// Talks to the editor page: formatting, links, and the HTML out.
@MainActor
final class EditorController {
    fileprivate weak var web: WKWebView?

    /// The HTML as written, or `nil` if the editor isn't there.
    func html() async -> String? {
        guard let web else { return nil }
        return try? await web.evaluateJavaScript("document.getElementById('body').innerHTML") as? String
    }

    /// A formatting command on the selection: bold, italic, underline,
    /// insertUnorderedList, insertOrderedList, removeFormat.
    func exec(_ command: String) {
        run("document.execCommand(\(js(command)), false, null)")
    }

    /// Remember the selection before something else (the link prompt)
    /// takes focus; returns the selected text, to show as the link's text.
    func keepSelection() async -> String {
        let script = "var s = getSelection(); window.owlRange = s.rangeCount ? s.getRangeAt(0).cloneRange() : null; s.toString()"
        return (try? await web?.evaluateJavaScript(script) as? String) ?? ""
    }

    /// Make the kept selection a link to `url`; with nothing selected,
    /// insert `text` (or the address itself) as the link.
    func link(_ url: String, text: String) {
        let label = escapeHTML(text.isEmpty ? url : text)
        let a = "<a href=\"\(escapeHTML(url).replacingOccurrences(of: "\"", with: "&quot;"))\">\(label)</a>"
        run("""
            var s = getSelection();
            if (window.owlRange) { s.removeAllRanges(); s.addRange(window.owlRange); }
            if (s.isCollapsed || s.toString() !== \(js(text))) { document.execCommand('insertHTML', false, \(js(a))); }
            else { document.execCommand('createLink', false, \(js(url))); }
            """)
    }

    private func run(_ script: String) {
        web?.evaluateJavaScript("document.getElementById('body').focus(); \(script)")
    }

    /// A Swift string as a JavaScript string literal.
    private func js(_ s: String) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: [s])) ?? Data("[\"\"]".utf8)
        let array = String(decoding: data, as: UTF8.self)
        return String(array.dropFirst().dropLast())
    }
}

struct RichEditor: UIViewRepresentable {
    let html: String
    let controller: EditorController

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // Nothing in the draft runs; the controller's scripts still do.
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.navigationDelegate = context.coordinator
        controller.web = web
        web.loadHTMLString(Self.page(html), baseURL: nil)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator() }

    /// Links in the editor are for the recipient: tapping one never leaves.
    final class Coordinator: NSObject, WKNavigationDelegate {
        func webView(_ web: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void) {
            decisionHandler(action.navigationType == .other ? .allow : .cancel)
        }
    }

    static func page(_ html: String) -> String {
        """
        <!doctype html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data: cid:">
        <style>
        :root { color-scheme: light dark; }
        html, body { margin: 0; background: transparent; }
        #body { min-height: 60vh; padding: 12px 16px 40px; outline: none; font: 17px/1.4 -apple-system, system-ui, sans-serif; word-wrap: break-word; }
        #body img { max-width: 100%; height: auto; }
        blockquote { margin: 6px 0 6px 2px; padding-left: 12px; border-left: 3px solid #b0b4ba; color: #888; }
        .owlpost-quote { color: #888; font-size: 15px; }
        a { color: #2f7bf5; }
        </style></head><body><div id="body" contenteditable="true">\(html)</div></body></html>
        """
    }
}
