import AppKit
import CryptoKit
import ImageIO
import Security

@_silgen_name("stack_backup")
private func stackBackup(_ operation: UInt32, _ input: UnsafePointer<UInt8>?, _ count: Int,
                         _ password: UnsafePointer<UInt8>?, _ passwordCount: Int, _ random: UnsafePointer<UInt8>?,
                         _ resultCount: inout Int, _ error: inout UnsafeMutablePointer<CChar>?) -> UnsafeMutablePointer<UInt8>?
@_silgen_name("stack_free_bytes")
private func stackFreeBytes(_ bytes: UnsafeMutablePointer<UInt8>, _ count: Int)
@_silgen_name("stack_free")
private func stackFreeError(_ error: UnsafeMutablePointer<CChar>)

struct ClipstackError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

struct PortableText: Codable, Equatable, Sendable {
    var text: String
    var html: Data?
    var rtf: Data?
    var url: String?
}

struct PortableBitmap: Codable, Equatable, Sendable {
    var version5: Bool
    var bytes: Data
}

struct PortableContent: Codable, Equatable, Sendable {
    var kind: String
    var text: String?
    var html: Data?
    var rtf: Data?
    var url: String?
    var png: Data?
    var width: UInt32?
    var height: UInt32?
    var rich: PortableText?
    var bitmap: PortableBitmap?
    var paths: [String]?
}

private struct PortableEntry: Codable {
    var id: String
    var created_at: UInt64
    var content: PortableContent
    var macos_items: [[String: Data]]?
}

enum PortableBackup {
    static let fileExtension = "clipstack-backup"
    static let limit = 512 * 1024 * 1024

    static func export(_ clips: [Clip], password: String, to url: URL) throws {
        guard !clips.isEmpty, clips.count <= 50_000 else {
            throw ClipstackError("可导出的收藏数量必须在 1 到 50,000 条之间。")
        }
        let records = try clips.map { clip in
            PortableEntry(id: clip.id.uuidString, created_at: UInt64(max(0, clip.date.timeIntervalSince1970 * 1000)),
                          content: try content(of: clip), macos_items: clip.items)
        }
        let plain = try JSONEncoder().encode(records)
        let encrypted = try crypt(plain, password: password, operation: 0)
        try encrypted.write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    static func read(_ url: URL, password: String) throws -> [Clip] {
        let file = try FileHandle(forReadingFrom: url)
        defer { try? file.close() }
        guard try file.seekToEnd() <= limit else { throw ClipstackError("请选择 512 MiB 以内的剪栈备份文件。") }
        try file.seek(toOffset: 0)
        let input = try file.read(upToCount: limit + 1) ?? Data()
        guard input.count <= limit else { throw ClipstackError("备份文件超过 512 MiB。") }
        let plain = try crypt(input, password: password, operation: 1)
        let records = try JSONDecoder().decode([PortableEntry].self, from: plain)
        guard !records.isEmpty, records.count <= 50_000,
              records.allSatisfy({ $0.created_at <= 8_640_000_000_000_000 }) else {
            throw ClipstackError("备份中的收藏记录数量或日期无效。")
        }
        return try records.map { record in
            let items = try representations(of: record.content)
            var clip = Clip(items: record.macos_items ?? items)
            guard !clip.items.isEmpty else { throw ClipstackError("备份中含有空收藏记录。") }
            clip.date = Date(timeIntervalSince1970: Double(record.created_at) / 1000)
            clip.portableContent = record.content
            return clip
        }
    }

    static func crypt(_ input: Data, password: String, operation: UInt32) throws -> Data {
        var secret = Data(password.utf8)
        defer { secret.resetBytes(in: secret.startIndex..<secret.endIndex) }
        var random = [UInt8](repeating: 0, count: 40)
        if operation == 0 && SecRandomCopyBytes(kSecRandomDefault, random.count, &random) != errSecSuccess {
            throw ClipstackError("无法为备份生成安全的随机数据。")
        }
        var count = 0
        var error: UnsafeMutablePointer<CChar>?
        let result = input.withUnsafeBytes { data in
            secret.withUnsafeBytes { key in
                random.withUnsafeBufferPointer { salt in
                    stackBackup(operation, data.baseAddress?.assumingMemoryBound(to: UInt8.self), data.count,
                                key.baseAddress?.assumingMemoryBound(to: UInt8.self), key.count, salt.baseAddress, &count, &error)
                }
            }
        }
        if let error {
            defer { stackFreeError(error) }
            throw ClipstackError(String(cString: error))
        }
        guard let result else { throw ClipstackError("无法处理收藏备份。") }
        defer { stackFreeBytes(result, count) }
        return Data(bytes: result, count: count)
    }

    static func content(of clip: Clip) throws -> PortableContent {
        if let original = clip.portableContent { return original }
        let accompanying = PortableText(text: clip.searchableText, html: clip.data(.html),
                                       rtf: clip.data(.rtf), url: clip.data(.URL).flatMap { String(data: $0, encoding: .utf8) })
        if !clip.files.isEmpty {
            return PortableContent(kind: "files", rich: accompanying, paths: clip.files.map { $0.url.path })
        }
        if let image = clip.image, let tiff = image.tiffRepresentation,
           let bitmap = NSBitmapImageRep(data: tiff),
           let png = clip.data(.png) ?? bitmap.representation(using: .png, properties: [:]) {
            return PortableContent(kind: "image", png: png, width: UInt32(bitmap.pixelsWide), height: UInt32(bitmap.pixelsHigh),
                                   rich: accompanying.text.isEmpty ? nil : accompanying)
        }
        var rtf = clip.data(.rtf)
        if rtf == nil, let rtfd = clip.data(.rtfd),
           let rich = try? NSAttributedString(data: rtfd, options: [.documentType: NSAttributedString.DocumentType.rtfd],
                                              documentAttributes: nil) {
            rtf = try rich.data(from: NSRange(location: 0, length: rich.length),
                               documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf])
        }
        if rtf != nil || accompanying.html != nil {
            return PortableContent(kind: "rich_text", text: clip.searchableText, html: accompanying.html, rtf: rtf, url: accompanying.url)
        }
        return PortableContent(kind: clip.kind == "链接" ? "link" : "text", text: clip.text)
    }

    private static func representations(of content: PortableContent) throws -> [[String: Data]] {
        var item: [String: Data] = [:]
        func text(_ rich: PortableText) {
            item[NSPasteboard.PasteboardType.string.rawValue] = Data(rich.text.utf8)
            item[NSPasteboard.PasteboardType.html.rawValue] = rich.html.map(HTMLDocument.content)
            item[NSPasteboard.PasteboardType.rtf.rawValue] = rich.rtf
            item[NSPasteboard.PasteboardType.URL.rawValue] = rich.url.map { Data($0.utf8) }
        }
        switch content.kind {
        case "text", "link", "rich_text":
            guard let value = content.text else { throw ClipstackError("备份中的文本记录无效。") }
            text(PortableText(text: value, html: content.html, rtf: content.rtf, url: content.url))
        case "image":
            guard let png = content.png, content.width != nil, content.height != nil,
                  CGImageSourceCreateWithData(png as CFData, nil) != nil else {
                throw ClipstackError("备份中的图片记录无效。")
            }
            if let rich = content.rich { text(rich) }
            item[NSPasteboard.PasteboardType.png.rawValue] = png
        case "files":
            guard let paths = content.paths, !paths.isEmpty else { throw ClipstackError("备份中的文件路径无效。") }
            if paths.allSatisfy({ $0.hasPrefix("/") }) {
                return Clip.fileReferences(paths.map { URL(fileURLWithPath: $0) }).items
            }
            text(content.rich ?? PortableText(text: paths.joined(separator: "\n")))
        default:
            throw ClipstackError("备份包含当前版本无法识别的收藏类型。")
        }
        return [item]
    }
}

enum HTMLDocument {
    static func content(_ bytes: Data) -> Data {
        let header = String(decoding: bytes.prefix(512), as: UTF8.self)
        func offset(_ key: String) -> Int? {
            guard let range = header.range(of: key) else { return nil }
            return Int(header[range.upperBound...].prefix { $0.isNumber || $0 == "-" })
        }
        guard let start = offset("StartHTML:"), let end = offset("EndHTML:"),
              start >= 0, start < end, end <= bytes.count else { return bytes }
        return bytes.subdata(in: start..<end)
    }
}

extension Clip {
    var contentFingerprint: Data {
        var hasher = SHA256()
        for item in items {
            for key in item.keys.sorted() {
                let bytes = item[key]!
                hasher.update(data: Data("\(key.utf8.count):\(key):\(bytes.count):".utf8))
                hasher.update(data: bytes)
            }
            hasher.update(data: Data([0xff]))
        }
        return Data(hasher.finalize())
    }
}
