// Writing an email: new, reply, reply all, forward, or a saved draft.
//
// From, To, Cc, Bcc and Subject are native fields, with suggestions from
// mail you've had and the iPhone's contacts. The body is the formatted
// editor (RichEditor). Attachments come from Photos (sent as JPEG, which
// every recipient can open) or Files. Closing asks whether to keep the
// draft in the server's Drafts folder, where the Mac sees it too.

import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

struct ComposeView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var draft: Draft
    private let original: Draft
    @State private var showCc: Bool
    @State private var editor = EditorController()
    @State private var working = false
    @State private var error: String?
    @State private var closing = false
    @State private var linking = false
    @State private var linkURL = ""
    @State private var linkText = ""
    @State private var choosingPhotos = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var choosingFiles = false
    @FocusState private var subjectFocused: Bool

    init(request: ComposeRequest) {
        _draft = State(initialValue: request.draft)
        original = request.draft
        _showCc = State(initialValue: !request.draft.cc.isEmpty || !request.draft.bcc.isEmpty)
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                fields
                FormatBar(editor: editor, link: startLink, photos: { choosingPhotos = true }, files: { choosingFiles = true })
                if !draft.attachments.isEmpty {
                    AttachmentStrip(paths: $draft.attachments)
                }
                Divider()
                RichEditor(html: original.html, controller: editor)
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { Task { await cancel() } }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if working {
                        ProgressView()
                    } else {
                        Button { Task { await send() } } label: { Image(systemName: "paperplane.fill") }
                            .accessibilityLabel("Send")
                            .disabled((draft.to + draft.cc + draft.bcc).trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            }
            .interactiveDismissDisabled()
            .disabled(working)
        }
        .confirmationDialog("Keep this draft?", isPresented: $closing, titleVisibility: .visible) {
            Button("Save Draft") { Task { await saveAndClose() } }
            Button("Delete Draft", role: .destructive) { Task { await discard() } }
            Button("Keep Writing", role: .cancel) {}
        }
        .alert("Add a link", isPresented: $linking) {
            TextField("Text", text: $linkText)
            TextField("Address", text: $linkURL)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("Add") { editor.link(normalised(linkURL), text: linkText) }
                .disabled(linkURL.trimmingCharacters(in: .whitespaces).isEmpty)
            Button("Cancel", role: .cancel) {}
        }
        .alert("Couldn't do that", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(error ?? "")
        }
        .photosPicker(isPresented: $choosingPhotos, selection: $photos, maxSelectionCount: 20, matching: .images)
        .onChange(of: photos) { _, items in
            guard !items.isEmpty else { return }
            photos = []
            Task { await addPhotos(items) }
        }
        .fileImporter(isPresented: $choosingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { addFiles(urls) }
        }
    }

    private var fields: some View {
        VStack(spacing: 0) {
            if model.accounts.count > 1 {
                HStack {
                    Text("From").foregroundStyle(.secondary).frame(width: 64, alignment: .leading)
                    Picker("From", selection: $draft.accountId) {
                        ForEach(model.accounts, id: \.id) { Text($0.email).tag($0.id) }
                    }
                    .labelsHidden()
                    Spacer()
                }
                .padding(.horizontal)
                .padding(.vertical, 4)
                Divider()
            }
            AddressField(label: "To", text: $draft.to) {
                if !showCc {
                    Button("Cc Bcc") { showCc = true }.font(.footnote)
                }
            }
            if showCc {
                AddressField(label: "Cc", text: $draft.cc) { EmptyView() }
                AddressField(label: "Bcc", text: $draft.bcc) { EmptyView() }
            }
            HStack {
                Text("Subject").foregroundStyle(.secondary).frame(width: 64, alignment: .leading)
                TextField("", text: $draft.subject)
                    .accessibilityLabel("Subject")
                    .focused($subjectFocused)
            }
            .padding(.horizontal)
            .padding(.vertical, 10)
            // The whole row is the field, label included, as in Mail.
            .contentShape(Rectangle())
            .onTapGesture { subjectFocused = true }
        }
    }

    private var title: String {
        if original.replaces != nil { return "Draft" }
        if original.inReplyTo != nil { return "Reply" }
        if original.subject.hasPrefix("Fwd:") { return "Forward" }
        return "New Message"
    }

    /// The draft with the editor's latest HTML.
    private func current() async -> Draft {
        var d = draft
        if let html = await editor.html() { d.html = html }
        return d
    }

    private func send() async {
        working = true
        defer { working = false }
        do {
            try await model.send(await current())
            dismiss()
        } catch {
            self.error = message(error)
        }
    }

    private func cancel() async {
        let d = await current()
        let untouched = d.html == original.html && d.to == original.to && d.cc == original.cc && d.bcc == original.bcc
            && d.subject == original.subject && d.attachments == original.attachments
        if untouched {
            dismiss()
        } else {
            closing = true
        }
    }

    private func saveAndClose() async {
        working = true
        defer { working = false }
        do {
            _ = try await model.saveDraft(await current())
            dismiss()
        } catch {
            self.error = message(error)
        }
    }

    private func discard() async {
        if let id = original.replaces {
            await model.deleteDraft(id)
        }
        dismiss()
    }

    private func startLink() {
        Task {
            linkText = await editor.keepSelection()
            linkURL = ""
            linking = true
        }
    }

    /// "example.com" → "https://example.com"; an address → "mailto:…".
    private func normalised(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespaces)
        if t.contains("://") || t.lowercased().hasPrefix("mailto:") || t.lowercased().hasPrefix("tel:") { return t }
        if t.contains("@") && !t.contains("/") { return "mailto:" + t }
        return "https://" + t
    }

    /// Photos go as JPEG: iPhone photos are HEIC, which many computers can't open.
    private func addPhotos(_ items: [PhotosPickerItem]) async {
        let dir = attachmentFolder()
        for (i, item) in items.enumerated() {
            guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
            let jpeg = UIImage(data: data)?.jpegData(compressionQuality: 0.85) ?? data
            let url = unique(dir.appendingPathComponent("Photo \(draft.attachments.count + i + 1).jpg"))
            if (try? jpeg.write(to: url)) != nil { draft.attachments.append(url.path) }
        }
    }

    /// Files from the Files app are copied in: the picker's access ends.
    private func addFiles(_ urls: [URL]) {
        let dir = attachmentFolder()
        for url in urls {
            let ok = url.startAccessingSecurityScopedResource()
            defer { if ok { url.stopAccessingSecurityScopedResource() } }
            let copy = unique(dir.appendingPathComponent(url.lastPathComponent))
            do {
                try FileManager.default.copyItem(at: url, to: copy)
                draft.attachments.append(copy.path)
            } catch {
                self.error = "Couldn't attach \(url.lastPathComponent): \(error.localizedDescription)"
            }
        }
    }

    private func attachmentFolder() -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("compose", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func unique(_ url: URL) -> URL {
        var candidate = url
        var n = 2
        while FileManager.default.fileExists(atPath: candidate.path) {
            let stem = url.deletingPathExtension().lastPathComponent
            candidate = url.deletingLastPathComponent().appendingPathComponent("\(stem) \(n)").appendingPathExtension(url.pathExtension)
            n += 1
        }
        return candidate
    }

    private func message(_ e: Error) -> String {
        if case let MailError.Failed(message) = e { return message }
        return e.localizedDescription
    }
}

/// Bold, italic, underline, lists, link, and attach.
private struct FormatBar: View {
    let editor: EditorController
    let link: () -> Void
    let photos: () -> Void
    let files: () -> Void

    var body: some View {
        HStack(spacing: 2) {
            button("bold", "Bold") { editor.exec("bold") }
            button("italic", "Italic") { editor.exec("italic") }
            button("underline", "Underline") { editor.exec("underline") }
            button("list.bullet", "Bulleted list") { editor.exec("insertUnorderedList") }
            button("list.number", "Numbered list") { editor.exec("insertOrderedList") }
            button("link", "Link", action: link)
            Spacer()
            Menu {
                Button(action: photos) { Label("Photo Library", systemImage: "photo.on.rectangle") }
                Button(action: files) { Label("Files", systemImage: "folder") }
            } label: {
                Image(systemName: "paperclip").frame(width: 36, height: 32)
            }
            .accessibilityLabel("Attach")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 2)
        .background(.background.secondary)
    }

    private func button(_ icon: String, _ name: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: icon).frame(width: 36, height: 32) }
            .accessibilityLabel(name)
    }
}

/// The files going with the message; tap ✕ to take one off.
private struct AttachmentStrip: View {
    @Binding var paths: [String]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(paths, id: \.self) { p in
                    HStack(spacing: 6) {
                        Image(systemName: "doc")
                        VStack(alignment: .leading, spacing: 0) {
                            Text(URL(fileURLWithPath: p).lastPathComponent).lineLimit(1)
                            Text(size(p)).foregroundStyle(.secondary)
                        }
                        Button { paths.removeAll { $0 == p } } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                            .accessibilityLabel("Remove \(URL(fileURLWithPath: p).lastPathComponent)")
                    }
                    .font(.caption)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(.background.tertiary, in: RoundedRectangle(cornerRadius: 8))
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 6)
        }
    }

    private func size(_ path: String) -> String {
        let bytes = (try? FileManager.default.attributesOfItem(atPath: path)[.size] as? Int) ?? 0
        return ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }
}

/// An address field: comma-separated, with suggestions for what you're
/// typing from mail you've had and the iPhone's contacts.
private struct AddressField<Extra: View>: View {
    @Environment(MailModel.self) private var model
    let label: String
    @Binding var text: String
    @ViewBuilder let extra: () -> Extra
    @State private var suggestions: [ContactSuggestion] = []
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(label).foregroundStyle(.secondary).frame(width: 64, alignment: .leading)
                TextField("", text: $text)
                    .accessibilityLabel(label)
                    .keyboardType(.emailAddress)
                    .textContentType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focused)
                extra()
            }
            .padding(.horizontal)
            .padding(.vertical, 10)
            // The whole row is the field, label included, as in Mail.
            .contentShape(Rectangle())
            .onTapGesture { focused = true }
            if focused && !suggestions.isEmpty {
                ForEach(suggestions, id: \.email) { c in
                    Button { take(c) } label: {
                        VStack(alignment: .leading, spacing: 1) {
                            if !c.name.isEmpty { Text(c.name).foregroundStyle(.primary) }
                            Text(c.email).font(.footnote).foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 80)
                        .padding(.vertical, 6)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            Divider()
        }
        .task(id: text) {
            // Wait for a pause in typing.
            try? await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled else { return }
            let typing = lastPart
            let taken = Set(text.split(separator: ",").dropLast().map { $0.lowercased() })
            suggestions = typing.count < 2 || typing.contains("<") ? [] : await model.suggestions(typing).filter { s in
                !taken.contains { $0.contains(s.email.lowercased()) }
            }
        }
    }

    private var lastPart: String {
        String(text.split(separator: ",", omittingEmptySubsequences: false).last ?? "").trimmingCharacters(in: .whitespaces)
    }

    private func take(_ c: ContactSuggestion) {
        var parts = text.split(separator: ",", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
        if !parts.isEmpty { parts.removeLast() }
        parts.append(c.name.isEmpty ? c.email : "\(c.name) <\(c.email)>")
        text = parts.filter { !$0.isEmpty }.joined(separator: ", ") + ", "
        suggestions = []
    }
}
