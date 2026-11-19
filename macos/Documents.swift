import AppKit
import SwiftUI
import WebKit

@MainActor final class DocumentWindowController: NSWindowController {
    init(title: String, file: URL?, text: String?) {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 780, height: 590),
                              styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentMinSize = NSSize(width: 560, height: 400)
        super.init(window: window)
        window.contentViewController = NSHostingController(rootView: DocumentView(title: title, file: file, text: text) { [weak self] in
            guard let window = self?.window else { return }
            window.sheetParent?.endSheet(window)
            window.orderOut(nil)
        })
    }

    required init?(coder: NSCoder) { fatalError("Documents are created in code") }
}

private struct DocumentView: View {
    let title: String
    let file: URL?
    let text: String?
    let close: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(title).font(.headline)
                Spacer()
            }.padding(18).background(.regularMaterial)
            if let file {
                OfflineDocument(url: file)
            } else {
                ScrollView {
                    Text(text ?? "暂无内容。")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(22)
                }
            }
            Divider()
            HStack {
                Spacer()
                Button("关闭", action: close).keyboardShortcut(.cancelAction)
            }.padding(14)
        }
        .textSelection(.disabled)
        .frame(minWidth: 540, minHeight: 370)
    }
}

private final class ManualWebView: WKWebView {
    override func menu(for event: NSEvent) -> NSMenu? { nil }
}

private struct OfflineDocument: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let view = ManualWebView(frame: .zero, configuration: config)
        view.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {}
}
