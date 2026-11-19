import AppKit

func foldedSearchText(_ text: String) -> String {
    text.replacingOccurrences(of: "\0", with: " ")
        .folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: .current)
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

extension Clip {
    var searchableText: String {
        let files = files
        if !files.isEmpty { return files.map(\.name).joined(separator: "\n") }
        return items.compactMap(Self.textForSearch).joined(separator: "\n")
    }

    private static func textForSearch(_ item: [String: Data]) -> String? {
        let plain = [NSPasteboard.PasteboardType.string, .URL].compactMap { kind in
            item[kind.rawValue].flatMap { String(data: $0, encoding: .utf8) }
        }.filter { !$0.isEmpty }
        if !plain.isEmpty { return plain.joined(separator: "\n") }
        for (kind, encoding) in [
            (NSPasteboard.PasteboardType.rtfd, NSAttributedString.DocumentType.rtfd),
            (.rtf, .rtf),
        ] {
            guard let data = item[kind.rawValue],
                  let text = try? NSAttributedString(data: data, options: [.documentType: encoding],
                                                     documentAttributes: nil).string else { continue }
            return text
        }
        if let data = item[NSPasteboard.PasteboardType.html.rawValue] {
            return htmlText(String(decoding: data, as: UTF8.self))
        }
        return nil
    }

    private static let hiddenHTML = try! NSRegularExpression(
        pattern: #"(?is)<!--.*?-->|<(script|style|head)\b[^>]*>.*?</\1\s*>"#
    )
    private static let blockHTML = try! NSRegularExpression(pattern: #"(?i)</?(?:p|br|div|li|td|tr|h[1-6])\b[^>]*>"#)
    private static let tagHTML = try! NSRegularExpression(pattern: #"<[^>]+>"#)
    private static let htmlReference = try! NSRegularExpression(
        pattern: #"&(#x[0-9a-fA-F]+|#[0-9]+|amp|lt|gt|quot|apos|nbsp);"#
    )

    private static func htmlText(_ input: String) -> String {
        // Extract searchable text without opening a web view or resolving external resources.
        let visible = hiddenHTML.stringByReplacingMatches(in: input, range: NSRange(input.startIndex..., in: input),
                                                          withTemplate: "")
        let blocks = blockHTML.stringByReplacingMatches(in: visible, range: NSRange(visible.startIndex..., in: visible),
                                                         withTemplate: " ")
        let text = tagHTML.stringByReplacingMatches(in: blocks, range: NSRange(blocks.startIndex..., in: blocks),
                                                    withTemplate: "")
        let references = htmlReference.matches(in: text, range: NSRange(text.startIndex..., in: text))
        guard !references.isEmpty else { return text }
        let decoded = NSMutableString(string: text)
        let named = ["amp": "&", "lt": "<", "gt": ">", "quot": "\"", "apos": "'", "nbsp": " "]
        for match in references.reversed() {
            guard let range = Range(match.range(at: 1), in: text) else { continue }
            let reference = text[range]
            if let value = named[String(reference)] {
                decoded.replaceCharacters(in: match.range, with: value)
            } else {
                let hex = reference.hasPrefix("#x")
                if let value = UInt32(reference.dropFirst(hex ? 2 : 1), radix: hex ? 16 : 10),
                   let scalar = Unicode.Scalar(value) {
                    decoded.replaceCharacters(in: match.range, with: String(scalar))
                }
            }
        }
        return decoded as String
    }
}
