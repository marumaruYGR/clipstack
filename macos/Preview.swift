import AppKit
import ImageIO
import QuickLookUI
import SwiftUI
import WebKit

struct PreviewWindowView: View {
    let clip: Clip
    @State private var fileIndex = 0

    var body: some View {
        let files = clip.files
        Group {
            if files.isEmpty {
                contentPreview
            } else {
                let file = files[fileIndex]
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Label(file.name, systemImage: file.kind.symbol)
                            .font(.headline)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(file.url.path)
                        Spacer()
                        if files.count > 1 {
                            Button { fileIndex -= 1 } label: { Image(systemName: "chevron.left") }
                                .disabled(fileIndex == 0)
                                .accessibilityLabel("上一个文件")
                            Text("\(fileIndex + 1) / \(files.count)").font(.caption.monospacedDigit())
                            Button { fileIndex += 1 } label: { Image(systemName: "chevron.right") }
                                .disabled(fileIndex + 1 == files.count)
                                .accessibilityLabel("下一个文件")
                        }
                        Button("在 Finder 中显示", systemImage: "folder", action: file.reveal)
                    }
                    .buttonStyle(.borderless)
                    .padding(12)
                    .background(.regularMaterial)
                    Divider()
                    FilePreviewView(file: file).id(file.url)
                }
            }
        }
        .frame(minWidth: 400, maxWidth: .infinity, minHeight: 280, maxHeight: .infinity)
    }

    @ViewBuilder private var contentPreview: some View {
        if let paths = clip.portableContent?.paths {
            ContentUnavailableView {
                Label("源文件路径暂不可用", systemImage: "doc.badge.questionmark")
            } description: {
                Text(paths.joined(separator: "\n")).textSelection(.enabled)
            }
        } else if let image = clip.image {
            ImageAlbumPreview(image: image)
        } else if let html = clip.data(.html) {
            HTMLPreview(html: HTMLDocument.content(html))
        } else if let rich = clip.data(.rtfd) ?? clip.data(.rtf) {
            RichPreview(data: rich, type: clip.data(.rtfd) != nil ? .rtfd : .rtf)
        } else {
            ScrollView {
                Text(clip.text).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(20)
            }
        }
    }
}

private struct FilePreviewView: View {
    let file: ClipboardFile
    @State private var preview: ClipboardFile.Preview?

    var body: some View {
        Group {
            switch preview {
            case .image(let url):
                FileImagePreview(url: url)
            case .document(let url):
                QuickLookFileView(url: url)
            case .finder:
                ContentUnavailableView {
                    Label("请在 Finder 中查看此文件", systemImage: file.kind.symbol)
                } description: {
                    Text(file.url.path).lineLimit(3).truncationMode(.middle).textSelection(.enabled)
                }
            case .unavailable:
                ContentUnavailableView {
                    Label("无法访问此文件", systemImage: "doc.badge.questionmark")
                } description: {
                    Text("文件可能已移动、删除，或当前没有读取权限。")
                }
            case nil:
                ProgressView("正在载入预览…")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: file.url) {
            let file = file
            let destination = await Task.detached(priority: .userInitiated) { file.preview() }.value
            if !Task.isCancelled { preview = destination }
        }
    }
}

private struct FileImagePreview: View {
    let url: URL
    @State private var photo: NSImage?
    @State private var finishedLoading = false

    var body: some View {
        Group {
            if let photo {
                ImageAlbumPreview(image: photo, sourceURL: url)
            } else if finishedLoading {
                QuickLookFileView(url: url)
            } else {
                ProgressView("正在载入图片…")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: url) {
            let url = url
            let pixels = await Task.detached(priority: .userInitiated) {
                previewPixels(at: url)
            }.value
            if let pixels {
                if !Task.isCancelled {
                    photo = NSImage(cgImage: pixels, size: .zero)
                    finishedLoading = true
                }
            } else if !Task.isCancelled {
                let thumbnail = await ClipboardFile(url: url).thumbnail(size: CGSize(width: 900, height: 660))
                if !Task.isCancelled {
                    photo = thumbnail
                    finishedLoading = true
                }
            }
        }
    }
}

private func previewPixels(at url: URL) -> CGImage? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary)
    else { return nil }
    return CGImageSourceCreateThumbnailAtIndex(source, 0, [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
        kCGImageSourceThumbnailMaxPixelSize: 2400,
    ] as CFDictionary)
}

private struct ImageAlbumPreview: View {
    let image: NSImage
    var sourceURL: URL? = nil
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var replay = 0
    @State private var spread = false
    @State private var revealed = false

    var body: some View {
        Button {
            if !reduceMotion { replay &+= 1 }
        } label: {
            GeometryReader { proxy in
                let canvas = proxy.size
                let imageWidth = max(1, image.size.width)
                let imageHeight = max(1, image.size.height)
                let scale = min(max(1, canvas.width - 88) / imageWidth,
                                max(1, canvas.height - 78) / imageHeight, 1)
                let size = CGSize(width: imageWidth * scale, height: imageHeight * scale)

                ZStack {
                    sheet(size)
                        .saturation(0)
                        .opacity(reduceMotion ? 0 : (spread ? 0.31 : 0))
                        .rotationEffect(.degrees(spread && !reduceMotion ? -6 : 0))
                        .offset(x: spread && !reduceMotion ? -18 : 0, y: spread && !reduceMotion ? 9 : 0)
                        .accessibilityHidden(true)
                    sheet(size)
                        .rotationEffect(.degrees(revealed || reduceMotion ? 0 : 2))
                        .offset(x: revealed || reduceMotion ? 0 : 7, y: revealed || reduceMotion ? 0 : 12)
                        .scaleEffect(revealed || reduceMotion ? 1 : 0.965)
                        .opacity(revealed || reduceMotion ? 1 : 0.78)
                        .shadow(color: .black.opacity(0.20), radius: 14, y: 6)
                }
                .frame(width: canvas.width, height: canvas.height)
                .contentShape(Rectangle())
            }
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button("复制图像") {
                let original = sourceURL.flatMap(NSImage.init(contentsOf:)) ?? image
                guard let data = original.tiffRepresentation,
                      let bitmap = NSBitmapImageRep(data: data),
                      let png = bitmap.representation(using: .png, properties: [:]) else { return }
                let board = NSPasteboard.general
                board.clearContents()
                board.setData(png, forType: .png)
            }
        }
        .accessibilityLabel(reduceMotion ? "图片预览" : "重播图片展开动画")
        .help(reduceMotion ? "图片预览" : "点按图片重播展开动画")
        .background {
            ZStack {
                Color(nsColor: .underPageBackgroundColor)
                RadialGradient(colors: [.accentColor.opacity(0.09), .clear],
                               center: .center, startRadius: 0, endRadius: 390)
            }
        }
        .task(id: replay) {
            if reduceMotion { return }
            var reset = Transaction()
            reset.disablesAnimations = true
            withTransaction(reset) {
                spread = false
                revealed = false
            }
            do {
                try await Task.sleep(for: .milliseconds(35))
                withAnimation(.easeOut(duration: 0.43)) { spread = true }
                try await Task.sleep(for: .milliseconds(85))
                withAnimation(.smooth(duration: 0.43)) { revealed = true }
            } catch {}
        }
    }

    private func sheet(_ size: CGSize) -> some View {
        Image(nsImage: image)
            .resizable()
            .interpolation(.high)
            .frame(width: size.width, height: size.height)
            .clipShape(.rect(cornerRadius: 8))
    }
}

private struct QuickLookFileView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> QLPreviewView {
        let view = QLPreviewView(frame: .zero, style: .normal)!
        view.shouldCloseWithWindow = false
        view.autostarts = false
        view.previewItem = url as NSURL
        return view
    }

    func updateNSView(_ view: QLPreviewView, context: Context) {}

    static func dismantleNSView(_ view: QLPreviewView, coordinator: Void) {
        view.close()
    }
}

private struct RichPreview: NSViewRepresentable {
    let data: Data
    let type: NSAttributedString.DocumentType

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        let view = scroll.documentView as! NSTextView
        view.isEditable = false
        view.textContainerInset = NSSize(width: 16, height: 16)
        view.textStorage?.setAttributedString(
            (try? NSAttributedString(data: data, options: [.documentType: type], documentAttributes: nil))
                ?? NSAttributedString(string: "无法预览这段富文本。")
        )
        return scroll
    }

    func updateNSView(_ view: NSScrollView, context: Context) {}
}

private struct HTMLPreview: NSViewRepresentable {
    let html: Data

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator
        let content = String(decoding: html, as: UTF8.self)
        let head = """
        <meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; form-action 'none'">
        <style>:root{color-scheme:light dark}body{margin:16px;font:14px -apple-system}img{max-width:100%;height:auto}</style>
        """
        web.loadHTMLString(head + content, baseURL: nil)
        return web
    }

    func updateNSView(_ view: WKWebView, context: Context) {}

    func makeCoordinator() -> Navigation { Navigation() }

    final class Navigation: NSObject, WKNavigationDelegate {
        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            decisionHandler(action.request.url?.absoluteString == "about:blank" ? .allow : .cancel)
        }
    }
}
