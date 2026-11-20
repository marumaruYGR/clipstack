import AppKit
import Foundation

@main struct PasteAccessTests {
    static func require(_ value: @autoclosure () -> Bool, _ message: String) throws {
        if !value() { throw NSError(domain: "PasteAccessTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }

    static func main() async throws {
        // Regression: a valid posting grant must work without optional AX reads
        // or input-monitoring permission. File access is not part of this model.
        let minimum = PastePermissionSnapshot(accessibility: false, posting: true, listening: false)
        try require(minimum.canPaste, "Posting permission alone must enable direct paste")
        let unrelated = PastePermissionSnapshot(accessibility: true, posting: false, listening: true)
        try require(!unrelated.canPaste, "Other grants must not claim posting permission")
        let missing = PastePermissionSnapshot(accessibility: false, posting: false, listening: false)
        try require(!missing.canPaste, "Missing permission must preserve the manual fallback")

        let identity = AppSigningIdentity(identifier: "fixture", hash: "1234", team: nil, temporary: true)
        let probe = PastePermissionProbe(version: "1.1.4", processID: 42, permissions: minimum, signing: identity)
        let payload = try JSONEncoder().encode(probe)
        let restored = try JSONDecoder().decode(PastePermissionProbe.self, from: payload)
        try require(restored.permissions == minimum && restored.signing == identity, "Probe must retain separate permission and identity fields")
        let keys = Set((try JSONSerialization.jsonObject(with: payload) as! [String: Any]).keys)
        try require(keys == ["version", "processID", "permissions", "signing"], "Probe must not include clipboard content or document data")

        let files = FileManager.default
        let folder = files.temporaryDirectory.appending(path: "jianzhan-permission-tests-\(UUID())")
        try files.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? files.removeItem(at: folder) }
        let script = folder.appending(path: "probe")
        let json = String(data: payload, encoding: .utf8)!.replacingOccurrences(of: "'", with: "'\\''")
        try Data("#!/bin/sh\nprintf '%s' '\(json)'\n".utf8).write(to: script)
        try files.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        let fresh = await PastePermissionProbe.fresh(executable: script)
        try require(fresh?.permissions == minimum, "Fresh process must report newly granted posting permission")
        try Data("#!/bin/sh\nprintf 'invalid data'\n".utf8).write(to: script)
        let malformed = await PastePermissionProbe.fresh(executable: script)
        try require(malformed == nil, "Malformed results must not claim permission")
        try Data("#!/bin/sh\nexec /bin/sleep 10\n".utf8).write(to: script)
        let start = ContinuousClock.now
        let timedOut = await PastePermissionProbe.fresh(executable: script)
        try require(timedOut == nil && ContinuousClock.now - start < .seconds(5), "Fresh check must time out without blocking settings")
        print("Posting-only permission, manual fallback, private diagnostic data and fresh-process recovery tests passed.")
    }
}
