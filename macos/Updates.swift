import AppKit
import CryptoKit
import Foundation

struct ReleaseAsset: Decodable, Equatable, Sendable {
    let name: String
    let browser_download_url: String
    let size: Int64
    let digest: String?
}

struct ReleaseInfo: Decodable, Sendable {
    let tag_name: String
    let body: String?
    let draft: Bool
    let prerelease: Bool
    let assets: [ReleaseAsset]
}

struct MacUpdateFeed: Decodable, Sendable {
    let schemaVersion: Int
    let platform: String
    let channel: String
    let release: ReleaseInfo
}

struct AvailableUpdate: Sendable {
    let version: String
    let notes: String
    let asset: ReleaseAsset
}

@MainActor final class UpdateService: ObservableObject {
    @Published private(set) var checking = false
    @Published private(set) var downloading = false
    @Published private(set) var progress: Double = 0
    @Published private(set) var message = ""
    @Published private(set) var available: AvailableUpdate?
    let currentVersion = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.1.3"
    nonisolated static let feedURL = URL(string: "https://raw.githubusercontent.com/marumaruYGR/clipstack/main/macos/update.json")!
    nonisolated static let releaseURL = URL(string: "https://api.github.com/repos/marumaruYGR/clipstack/releases/latest")!
    private let defaults = UserDefaults.standard
    private var timer: Timer?
    private var lastAttempt = Date.distantPast

    func start(notices: SystemNotices) {
        timer = Timer.scheduledTimer(withTimeInterval: 3600, repeats: true) { [weak self, weak notices] _ in
            MainActor.assumeIsolated { if let notices { self?.check(automatic: true, notices: notices) } }
        }
        check(automatic: true, notices: notices)
    }

    func check(automatic: Bool = false, notices: SystemNotices? = nil) {
        guard !checking, !downloading else { return }
        if automatic {
            let last = defaults.object(forKey: "lastUpdateCheck") as? Date ?? .distantPast
            guard Date().timeIntervalSince(last) >= 86400, Date().timeIntervalSince(lastAttempt) >= 3600 else { return }
        }
        lastAttempt = Date()
        checking = true
        message = "正在检查更新…"
        available = nil
        Task {
            defer { checking = false }
            do {
                let release = try await Self.fetchRelease(current: currentVersion)
                available = try Self.select(release, current: currentVersion)
                defaults.set(Date(), forKey: "lastUpdateCheck")
                if let available {
                    message = "发现新版本 \(available.version)"
                    if automatic, let notices, defaults.string(forKey: "notifiedUpdate") != available.version {
                        if await notices.send("剪栈有更新版本，请在设置中检查更新。", id: "update-available") {
                            defaults.set(available.version, forKey: "notifiedUpdate")
                        }
                    }
                } else {
                    message = "当前已是最新版本（\(currentVersion)）。"
                }
            } catch {
                message = Self.displayText(error.localizedDescription)
            }
        }
    }

    nonisolated static func fetchRelease(current: String, session: URLSession = .shared) async throws -> ReleaseInfo {
        var lastError: Error = ClipstackError("无法读取 macOS 更新信息，请稍后重试。")
        for url in [feedURL, releaseURL] {
            do {
                var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20)
                request.setValue("Clipstack/\(current) macOS", forHTTPHeaderField: "User-Agent")
                request.setValue("no-cache", forHTTPHeaderField: "Cache-Control")
                if url == releaseURL {
                    request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
                    request.setValue("2022-11-28", forHTTPHeaderField: "X-GitHub-Api-Version")
                }
                let (data, response) = try await session.data(for: request)
                guard let response = response as? HTTPURLResponse else {
                    throw ClipstackError("更新服务未返回有效响应，请稍后重试。")
                }
                if [403, 429].contains(response.statusCode) {
                    throw ClipstackError("更新服务暂时繁忙，请稍后重试。")
                }
                guard response.statusCode == 200, data.count <= 2 * 1024 * 1024 else {
                    throw ClipstackError("无法读取 macOS 更新信息，请稍后重试。")
                }
                if url == feedURL {
                    let feed = try JSONDecoder().decode(MacUpdateFeed.self, from: data)
                    guard feed.schemaVersion == 1, feed.platform == "macos", feed.channel == "stable" else {
                        throw ClipstackError("更新信息不适用于当前系统。")
                    }
                    return feed.release
                }
                return try JSONDecoder().decode(ReleaseInfo.self, from: data)
            } catch {
                if Task.isCancelled { throw CancellationError() }
                lastError = error
            }
        }
        throw lastError
    }

    nonisolated static func version(_ text: String) -> [UInt64]? {
        let value = text.hasPrefix("v") ? String(text.dropFirst()) : text
        let parts = value.replacingOccurrences(of: "fix", with: "+").split(separator: "+", omittingEmptySubsequences: false)
        guard parts.count <= 2 else { return nil }
        let base = parts[0].split(separator: ".", omittingEmptySubsequences: false)
        guard base.count == 3 else { return nil }
        let values = base.compactMap { UInt64($0) }
        guard values.count == 3, let fix = UInt64(parts.count == 2 ? parts[1] : "0") else { return nil }
        return values + [fix]
    }

    nonisolated static func select(_ release: ReleaseInfo, current: String) throws -> AvailableUpdate? {
        guard !release.draft, !release.prerelease else { return nil }
        guard let current = version(current), let next = version(release.tag_name) else {
            throw ClipstackError("无法识别发布版本号。")
        }
        guard current.lexicographicallyPrecedes(next) else { return nil }
        let label = release.tag_name.hasPrefix("v") ? String(release.tag_name.dropFirst()) : release.tag_name
        let accepted = acceptedNames(label)
        guard let asset = accepted.lazy.compactMap({ name in release.assets.first { $0.name == name } }).first else {
            throw ClipstackError("发现 \(label)，该版本的 macOS 安装包尚未发布。")
        }
        _ = try validatedAssetURL(asset, tag: release.tag_name)
        return AvailableUpdate(version: label, notes: displayText(release.body ?? "此版本未提供更新日志。"), asset: asset)
    }

    nonisolated private static func acceptedNames(_ version: String) -> [String] {
        #if arch(arm64)
        let architecture = "arm64"
        #else
        let architecture = "x64"
        #endif
        return ["clipstack-\(version)-macos-universal.dmg", "剪栈-\(version).dmg",
                "clipstack-\(version)-macos-\(architecture).dmg"]
    }

    nonisolated private static func validatedAssetURL(_ asset: ReleaseAsset, tag: String) throws -> URL {
        let label = tag.hasPrefix("v") ? String(tag.dropFirst()) : tag
        var expected = URL(string: "https://github.com/marumaruYGR/clipstack/releases/download")!
        expected.append(path: tag)
        expected.append(path: asset.name)
        guard version(tag) != nil, acceptedNames(label).contains(asset.name),
              URL(string: asset.browser_download_url) == expected,
              asset.size > 0, asset.size <= Int64(PortableBackup.limit),
              let digest = asset.digest, digest.hasPrefix("sha256:"), digest.count == 71,
              digest.dropFirst(7).allSatisfy({ $0.isHexDigit && $0.isASCII }) else {
            throw ClipstackError("新版安装包地址、大小或 SHA-256 校验信息无效。")
        }
        return expected
    }

    nonisolated static func displayText(_ text: String) -> String {
        let links = try! NSRegularExpression(pattern: #"(?i)(?:https?://)?(?:[a-z0-9-]+\.)*github(?:usercontent)?\.com[^\s<>]*"#)
        let clean = links.stringByReplacingMatches(in: text, range: NSRange(text.startIndex..., in: text),
                                                  withTemplate: "更新发布服务")
        return clean.replacingOccurrences(of: "GitHub", with: "发布服务", options: .caseInsensitive)
    }

    func downloadAndInstall() {
        guard let available, !downloading, !checking else { return }
        downloading = true
        progress = 0
        message = "正在下载安装包…"
        Task {
            defer { downloading = false }
            do {
                let folder = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
                    .appending(path: "剪栈/Updates")
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let file = folder.appending(path: available.asset.name)
                try await Self.download(available.asset, to: file) { [weak service = self] fraction in
                    Task { @MainActor in service?.progress = fraction }
                }
                message = "正在验证并准备安装…"
                let current = Bundle.main.bundleURL
                if !FileManager.default.isWritableFile(atPath: current.deletingLastPathComponent().path)
                    || current.path.hasPrefix("/Volumes/") {
                    guard NSWorkspace.shared.open(file) else { throw ClipstackError("无法打开安装镜像。") }
                    message = "安装包已验证并打开。请退出剪栈，再将镜像中的剪栈拖到 Applications 完成安装。"
                    return
                }
                let staged = try await Task.detached(priority: .userInitiated) {
                    try UpdateInstaller.stage(image: file, current: current, version: available.version)
                }.value
                try UpdateInstaller.launch(staged: staged, current: current, folder: folder)
                NSApp.terminate(nil)
            } catch {
                message = "更新未完成，可重试。\(Self.displayText(error.localizedDescription))"
            }
        }
    }

    nonisolated static func download(_ asset: ReleaseAsset, to file: URL, session: URLSession = .shared,
                                     progress: @escaping @Sendable (Double) -> Void) async throws {
        let temp = file.deletingLastPathComponent().appending(path: ".download-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: temp) }
        guard let candidate = URL(string: asset.browser_download_url), candidate.pathComponents.count == 7 else {
            throw ClipstackError("安装包地址无效。")
        }
        let url = try validatedAssetURL(asset, tag: candidate.pathComponents[5])
        var request = URLRequest(url: url)
        request.timeoutInterval = 180
        let (bytes, response) = try await session.bytes(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              response.url?.scheme == "https",
              response.expectedContentLength < 0 || response.expectedContentLength == asset.size else {
            throw ClipstackError("安装包的响应或大小不符。")
        }
        FileManager.default.createFile(atPath: temp.path, contents: nil, attributes: [.posixPermissions: 0o600])
        let handle = try FileHandle(forWritingTo: temp)
        defer { try? handle.close() }
        var block = Data()
        var received: Int64 = 0
        var hash = SHA256()
        for try await byte in bytes {
            block.append(byte)
            received += 1
            guard received <= asset.size else { throw ClipstackError("安装包超过发布大小。") }
            if block.count == 65536 {
                try Task.checkCancellation()
                try handle.write(contentsOf: block)
                hash.update(data: block)
                block.removeAll(keepingCapacity: true)
                progress(Double(received) / Double(asset.size))
            }
        }
        try handle.write(contentsOf: block)
        hash.update(data: block)
        let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
        guard received == asset.size, asset.digest?.lowercased() == "sha256:" + digest else {
            throw ClipstackError("安装包不完整或 SHA-256 校验未通过。")
        }
        try handle.close()
        _ = try FileManager.default.replaceItemAtOrMove(file, with: temp)
        progress(1)
    }
}

private extension FileManager {
    func replaceItemAtOrMove(_ destination: URL, with source: URL) throws -> URL {
        if fileExists(atPath: destination.path) {
            return try replaceItemAt(destination, withItemAt: source) ?? destination
        }
        try moveItem(at: source, to: destination)
        return destination
    }
}

enum UpdateInstaller {
    static func stage(image: URL, current: URL, version: String) throws -> URL {
        let data = try run("/usr/bin/hdiutil", ["attach", "-readonly", "-nobrowse", "-noautoopen", "-plist", image.path])
        guard let plist = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let entities = plist["system-entities"] as? [[String: Any]],
              let mount = entities.compactMap({ $0["mount-point"] as? String }).first else {
            throw ClipstackError("无法挂载安装镜像。")
        }
        defer { _ = try? run("/usr/bin/hdiutil", ["detach", mount]) }
        let source = URL(fileURLWithPath: mount).appending(path: "剪栈.app")
        let values = try source.resourceValues(forKeys: [.isSymbolicLinkKey])
        guard values.isSymbolicLink != true,
              let bundle = Bundle(url: source), bundle.bundleIdentifier == Bundle(url: current)?.bundleIdentifier,
              bundle.infoDictionary?["CFBundleShortVersionString"] as? String == version else {
            throw ClipstackError("安装镜像中的应用与更新版本不符。")
        }
        _ = try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", source.path])
        let staged = current.deletingLastPathComponent().appending(path: ".剪栈-update-\(UUID().uuidString).app")
        do {
            _ = try run("/usr/bin/ditto", [source.path, staged.path])
            _ = try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", staged.path])
            return staged
        } catch {
            try? FileManager.default.removeItem(at: staged)
            throw error
        }
    }

    static func launch(staged: URL, current: URL, folder: URL) throws {
        let helper = folder.appending(path: "install-\(UUID().uuidString).sh")
        let script = """
        #!/bin/sh
        original="$2"
        staged="$3"
        previous="$4"
        count=0
        while kill -0 "$1" 2>/dev/null; do
            count=$((count + 1))
            if [ "$count" -ge 300 ]; then exit 1; fi
            sleep 0.2
        done
        if ! /usr/bin/codesign --verify --deep --strict "$staged"; then exit 1; fi
        if ! /bin/mv "$original" "$previous"; then /usr/bin/open "$original"; exit 1; fi
        if ! /bin/mv "$staged" "$original"; then
            /bin/mv "$previous" "$original"
            /usr/bin/open "$original"
            exit 1
        fi
        /usr/bin/open "$original"
        """
        try Data(script.utf8).write(to: helper, options: .atomic)
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/sh")
        task.arguments = [helper.path, String(ProcessInfo.processInfo.processIdentifier), current.path, staged.path,
                          current.deletingLastPathComponent().appending(path: ".剪栈-previous-\(UUID().uuidString).app").path]
        try task.run()
    }

    private static func run(_ command: String, _ args: [String]) throws -> Data {
        let process = Process()
        let output = Pipe()
        let errors = Pipe()
        process.executableURL = URL(fileURLWithPath: command)
        process.arguments = args
        process.standardOutput = output
        process.standardError = errors
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        let detail = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw ClipstackError(String(data: detail, encoding: .utf8) ?? "无法准备更新。")
        }
        return data
    }
}
