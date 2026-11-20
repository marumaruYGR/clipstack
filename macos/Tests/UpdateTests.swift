import AppKit
import CryptoKit
import Foundation

struct ClipstackError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
enum PortableBackup { static let limit = 512 * 1024 * 1024 }

final class UpdateTransport: URLProtocol, @unchecked Sendable {
    static let content = Data("verified native installer fixture".utf8)
    static let nativeName = "clipstack-9.0.0-macos-universal.dmg"
    static let nativeURL = URL(string: "https://github.com/marumaruYGR/clipstack/releases/download/v9.0.0/" + nativeName)!
    static var responses: [URL: (Int, Data)] = [:]
    static var requests: [URLRequest] = []
    static let lock = NSLock()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock()
        Self.requests.append(request)
        let (status, data) = Self.responses[request.url!] ?? (404, Data())
        Self.lock.unlock()
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Length": String(data.count)])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

@main struct UpdateProbe {
    static func require(_ condition: @autoclosure () throws -> Bool, _ label: String) throws {
        if try !condition() { throw ClipstackError("FAIL: " + label) }
    }
    static func hash(_ data: Data) -> String { "sha256:" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
    static func nativeAsset() -> ReleaseAsset {
        ReleaseAsset(name: UpdateTransport.nativeName, browser_download_url: UpdateTransport.nativeURL.absoluteString,
                     size: Int64(UpdateTransport.content.count), digest: hash(UpdateTransport.content))
    }
    static func release(_ assets: [ReleaseAsset], draft: Bool = false, prerelease: Bool = false) -> ReleaseInfo {
        ReleaseInfo(tag_name: "v9.0.0", body: "更新说明", draft: draft, prerelease: prerelease, assets: assets)
    }
    static func releaseJSON() throws -> Data {
        try JSONSerialization.data(withJSONObject: ["tag_name": "v9.0.0", "body": "更新说明", "draft": false, "prerelease": false,
            "assets": [["name": UpdateTransport.nativeName, "browser_download_url": UpdateTransport.nativeURL.absoluteString,
                        "size": UpdateTransport.content.count, "digest": hash(UpdateTransport.content)]]])
    }
    static func feedJSON(platform: String = "macos") throws -> Data {
        try JSONSerialization.data(withJSONObject: ["schemaVersion": 1, "platform": platform, "channel": "stable",
                                                     "release": JSONSerialization.jsonObject(with: releaseJSON())])
    }
    static func reset(_ responses: [URL: (Int, Data)]) {
        UpdateTransport.lock.lock(); defer { UpdateTransport.lock.unlock() }
        UpdateTransport.responses = responses
        UpdateTransport.requests = []
    }
    static func rejected(_ body: () throws -> Void, _ label: String) throws {
        var failed = false
        do { try body() } catch { failed = true }
        try require(failed, label)
    }

    @MainActor static func main() async {
        do { try await run(); print("ALL UPDATE CHECKS PASSED") }
        catch { fputs("\(error)\n", stderr); exit(1) }
    }

    @MainActor static func run() async throws {
        let folder = URL(fileURLWithPath: CommandLine.arguments[1])
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateTransport.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        reset([UpdateService.feedURL: (200, try feedJSON()), UpdateService.releaseURL: (403, Data())])
        let primary = try await UpdateService.fetchRelease(current: "1.1.3", session: session)
        try require(primary.tag_name == "v9.0.0", "native feed available despite release API rate limit")
        try require(UpdateTransport.requests.count == 1 && UpdateTransport.requests[0].url == UpdateService.feedURL,
                    "working native feed never depends on release API")
        try require(UpdateTransport.requests[0].value(forHTTPHeaderField: "Cache-Control") == "no-cache", "fresh update check")

        reset([UpdateService.feedURL: (503, Data()), UpdateService.releaseURL: (200, try releaseJSON())])
        let fallback = try await UpdateService.fetchRelease(current: "1.1.3", session: session)
        try require(fallback.tag_name == "v9.0.0" && UpdateTransport.requests.count == 2, "release API fallback")
        reset([UpdateService.feedURL: (200, try feedJSON(platform: "other")), UpdateService.releaseURL: (403, Data())])
        var invalidFeedRejected = false
        do { _ = try await UpdateService.fetchRelease(current: "1.1.3", session: session) } catch { invalidFeedRejected = true }
        try require(invalidFeedRejected, "foreign platform feed rejected")
        reset([UpdateService.feedURL: (200, Data("bad json".utf8)), UpdateService.releaseURL: (200, try releaseJSON())])
        _ = try await UpdateService.fetchRelease(current: "1.1.3", session: session)
        try require(UpdateTransport.requests.count == 2, "malformed primary feed handled")

        let valid = nativeAsset()
        let foreign = ReleaseAsset(name: "clipstack-9.0.0-other-x64-setup.exe",
                                   browser_download_url: "https://github.com/marumaruYGR/clipstack/releases/download/v9.0.0/clipstack-9.0.0-other-x64-setup.exe",
                                   size: valid.size, digest: valid.digest)
        try require(try UpdateService.select(release([foreign, valid]), current: "1.1.3")?.asset == valid, "mixed assets choose native DMG")
        try rejected({ _ = try UpdateService.select(release([foreign]), current: "1.1.3") }, "foreign installer excluded")
        try require(try UpdateService.select(release([valid]), current: "9.0.0") == nil, "no same-version update")
        try require(try UpdateService.select(release([valid]), current: "10.0.0") == nil, "no downgrade")
        try require(try UpdateService.select(release([valid], draft: true), current: "1.1.3") == nil, "draft excluded")
        try require(try UpdateService.select(release([valid], prerelease: true), current: "1.1.3") == nil, "prerelease excluded")
        for bad in [
            ReleaseAsset(name: valid.name, browser_download_url: "https://example.com/" + valid.name, size: valid.size, digest: valid.digest),
            ReleaseAsset(name: valid.name, browser_download_url: valid.browser_download_url + "?other=1", size: valid.size, digest: valid.digest),
            ReleaseAsset(name: valid.name, browser_download_url: valid.browser_download_url, size: 0, digest: valid.digest),
            ReleaseAsset(name: valid.name, browser_download_url: valid.browser_download_url, size: valid.size, digest: nil),
            ReleaseAsset(name: valid.name, browser_download_url: valid.browser_download_url, size: valid.size, digest: "sha256:" + String(repeating: "z", count: 64))
        ] { try rejected({ _ = try UpdateService.select(release([bad]), current: "1.1.3") }, "invalid native metadata excluded") }

        let destination = folder.appending(path: "verified-fixture.dmg")
        reset([UpdateTransport.nativeURL: (200, UpdateTransport.content)])
        try await UpdateService.download(valid, to: destination, session: session) { _ in }
        try require(try Data(contentsOf: destination) == UpdateTransport.content, "verified installer written")
        for corrupted in [Data("truncated".utf8), Data(repeating: 0x41, count: UpdateTransport.content.count)] {
            reset([UpdateTransport.nativeURL: (200, corrupted)])
            var failed = false
            do { try await UpdateService.download(valid, to: destination, session: session) { _ in } } catch { failed = true }
            try require(failed && (try Data(contentsOf: destination)) == UpdateTransport.content, "size/hash failures preserve previous download")
        }
        reset([:])
        var refused = false
        do { try await UpdateService.download(foreign, to: destination, session: session) { _ in } } catch { refused = true }
        try require(refused && UpdateTransport.requests.isEmpty, "foreign downloads refused before network request")
        let screen = UpdateService.displayText("GitHub https://api.github.com/repos/example")
        try require(screen.range(of: "github", options: .caseInsensitive) == nil, "service branding filtered")
        print("Transport fallback, API throttling, platform selection, integrity and safe retry verified")

        guard CommandLine.arguments.count > 2 else { return }
        let live = try await UpdateService.fetchRelease(current: "1.1.1")
        guard let update = try UpdateService.select(live, current: "1.1.1") else { throw ClipstackError("No live native release") }
        let file = folder.appending(path: update.asset.name)
        try await UpdateService.download(update.asset, to: file) { _ in }
        let bytes = try Data(contentsOf: file)
        try require(bytes.count >= 512 && String(data: bytes[(bytes.count - 512)..<(bytes.count - 508)], encoding: .ascii) == "koly", "live download is UDIF disk image")
        let current = URL(fileURLWithPath: CommandLine.arguments[2])
        let staged = try UpdateInstaller.stage(image: file, current: current, version: update.version)
        defer { try? FileManager.default.removeItem(at: staged) }
        try require(Bundle(url: staged)?.infoDictionary?["CFBundleShortVersionString"] as? String == update.version, "live installer staged with verified bundle version")
        print("LIVE native feed, DMG download, SHA-256 and application staging passed:", update.version, file.lastPathComponent)
    }
}
