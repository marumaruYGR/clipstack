import AppKit
import QuickLookThumbnailing
import UniformTypeIdentifiers

struct ClipboardFile: Sendable {
    enum Kind: Sendable {
        case image, document, other

        var label: String {
            switch self {
            case .image: return "图片文件"
            case .document: return "文档"
            case .other: return "文件"
            }
        }

        var symbol: String {
            switch self {
            case .image: return "photo"
            case .document: return "doc.text"
            case .other: return "doc"
            }
        }
    }

    enum Preview: Sendable {
        case image(URL)
        case document(URL)
        case finder
        case unavailable
    }

    let url: URL
    var name: String { url.lastPathComponent.isEmpty ? url.path : url.lastPathComponent }
    var kind: Kind { classify(UTType(filenameExtension: fileExtension)) }

    private var fileExtension: String { url.pathExtension.lowercased() }
    private static let documentExtensions: Set<String> = [
        "doc", "docx", "docm", "odt", "pages", "rtf", "rtfd",
        "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key", "epub",
    ]

    private func classify(_ type: UTType?) -> Kind {
        if type?.conforms(to: .image) == true { return .image }
        if type?.conforms(to: .text) == true || type?.conforms(to: .pdf) == true
            || Self.documentExtensions.contains(fileExtension) {
            return .document
        }
        return .other
    }

    func preview() -> Preview {
        let target = url.resolvingSymlinksInPath()
        guard let info = try? target.resourceValues(forKeys: [.contentTypeKey, .isRegularFileKey, .isPackageKey, .isReadableKey]),
              info.isReadable == true else { return .unavailable }
        let resolved = ClipboardFile(url: target)
        let kind = resolved.classify(info.contentType ?? UTType(filenameExtension: resolved.fileExtension))
        switch kind {
        case .image where info.isRegularFile == true:
            return .image(target)
        case .document where info.isRegularFile == true || info.isPackage == true:
            return .document(target)
        default:
            return .finder
        }
    }

    @MainActor func thumbnail(size: CGSize = CGSize(width: 260, height: 160)) async -> NSImage? {
        guard kind == .image else { return nil }
        let request = QLThumbnailGenerator.Request(fileAt: url, size: size,
                                                  scale: 2, representationTypes: [.thumbnail, .lowQualityThumbnail])
        return try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: request).nsImage
    }

    @MainActor func reveal() {
        if FileManager.default.fileExists(atPath: url.path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
            return
        }
        let parent = url.deletingLastPathComponent()
        if FileManager.default.fileExists(atPath: parent.path) {
            NSWorkspace.shared.open(parent)
        } else {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        }
    }
}
