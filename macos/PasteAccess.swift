import AppKit
import ApplicationServices
import Security

struct PastePermissionSnapshot: Codable, Equatable, Sendable {
    let accessibility: Bool
    let posting: Bool
    let listening: Bool

    // Posting keyboard events is the required permission. Accessibility helps
    // restore an editor's focus; observing input is a separate optional feature.
    var canPaste: Bool { posting }

    static func current() -> Self {
        Self(accessibility: AXIsProcessTrusted(), posting: CGPreflightPostEventAccess(),
             listening: CGPreflightListenEventAccess())
    }
}

struct AppSigningIdentity: Codable, Equatable, Sendable {
    let identifier: String
    let hash: String
    let team: String?
    let temporary: Bool

    static func current() -> Self {
        var code: SecCode?
        var staticCode: SecStaticCode?
        var information: CFDictionary?
        var values: [String: Any] = [:]
        if SecCodeCopySelf([], &code) == errSecSuccess, let code,
           SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
           SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess {
            values = information as? [String: Any] ?? [:]
        }
        let digest = (values[kSecCodeInfoUnique as String] as? Data)?.map { String(format: "%02x", $0) }.joined() ?? ""
        let flags = (values[kSecCodeInfoFlags as String] as? NSNumber)?.uint32Value ?? 0
        return Self(identifier: values[kSecCodeInfoIdentifier as String] as? String ?? Bundle.main.bundleIdentifier ?? "",
                    hash: digest, team: values[kSecCodeInfoTeamIdentifier as String] as? String,
                    temporary: flags & SecCodeSignatureFlags.adhoc.rawValue != 0)
    }
}

struct PastePermissionProbe: Codable, Sendable {
    let version: String
    let processID: Int32
    let permissions: PastePermissionSnapshot
    let signing: AppSigningIdentity

    static func current() -> Self {
        Self(version: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "",
             processID: ProcessInfo.processInfo.processIdentifier, permissions: .current(), signing: .current())
    }

    static func fresh(executable: URL) async -> Self? {
        await Task.detached(priority: .utility) {
            let process = Process()
            let output = Pipe()
            process.executableURL = executable
            process.arguments = ["--permission-probe"]
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
                let deadline = Date().addingTimeInterval(3)
                while process.isRunning && Date() < deadline {
                    try? await Task.sleep(for: .milliseconds(25))
                }
                if process.isRunning { process.terminate(); return nil }
                guard process.terminationStatus == 0 else { return nil }
                let data = output.fileHandleForReading.readDataToEndOfFile()
                guard data.count <= 16_384 else { return nil }
                return try JSONDecoder().decode(Self.self, from: data)
            } catch { return nil }
        }.value
    }
}

enum PasteFailure: String, Codable, Sendable {
    case permissionUnavailable, targetUnavailable, clipboardChanged, targetChanged
    case modifiersHeld, activationUnavailable, focusUnavailable, secureInput, eventUnavailable, cancelled
}

enum PasteOutcome {
    case sent
    case manual(PasteFailure)
}

@MainActor final class PasteAccessController: ObservableObject {
    @Published private(set) var permissions = PastePermissionSnapshot.current()
    @Published private(set) var freshPermissions: PastePermissionSnapshot?
    @Published private(set) var checking = false
    @Published private(set) var resetting = false
    @Published private(set) var status = ""
    private let signing = AppSigningIdentity.current()
    private var lastFreshCheck = Date.distantPast
    private var lastOutcome: String?
    private var lastTarget: String?

    var needsRestart: Bool {
        !permissions.canPaste && freshPermissions?.canPaste == true
    }

    var explanation: String {
        needsRestart
            ? "系统授权已更新，重新打开剪栈后即可使用。你也可以继续手动粘贴。"
            : "直接上屏需要系统授权，你也可以使用手动粘贴。"
    }

    func refresh(checkFresh: Bool = false) {
        let current = PastePermissionSnapshot.current()
        if permissions != current { permissions = current }
        if current.canPaste {
            freshPermissions = nil
            status = ""
            UserDefaults.standard.set(signing.hash, forKey: "pasteAccess.lastAllowedSignature")
        }
        writeReport()
        guard Bundle.main.bundleIdentifier == "local.jianzhan.clipboard", checkFresh, !current.canPaste, !checking,
              Date().timeIntervalSince(lastFreshCheck) >= 3,
              let executable = Bundle.main.executableURL else { return }
        lastFreshCheck = Date()
        checking = true
        Task {
            let result = await PastePermissionProbe.fresh(executable: executable)
            if let result, result.signing == signing, result.version == Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String {
                freshPermissions = result.permissions
            }
            checking = false
            writeReport()
        }
    }

    func request() {
        // Do not gate the posting request on AXIsProcessTrusted: the two checks
        // may have different grants, and either API can retain an earlier verdict.
        _ = CGRequestPostEventAccess()
        openSettings()
        refresh(checkFresh: true)
    }

    func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }

    func revealApplication() {
        NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
    }

    func resetAndRequest() {
        guard !resetting, let identifier = Bundle.main.bundleIdentifier,
              identifier == "local.jianzhan.clipboard" else { return }
        resetting = true
        status = ""
        Task {
            let completed = await Task.detached(priority: .utility) {
                var successes = 0
                for service in ["PostEvent", "Accessibility"] {
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/usr/bin/tccutil")
                    process.arguments = ["reset", service, identifier]
                    process.standardOutput = FileHandle.nullDevice
                    process.standardError = FileHandle.nullDevice
                    do {
                        try process.run()
                        process.waitUntilExit()
                        if process.terminationStatus == 0 { successes += 1 }
                    } catch {}
                }
                return successes == 2
            }.value
            resetting = false
            freshPermissions = nil
            status = completed
                ? "已重新设置剪栈的设备控制授权。是否允许直接上屏，由你选择。"
                : "可在系统设置中移除旧的剪栈条目，再添加当前应用。手动粘贴仍可使用。"
            request()
        }
    }

    func record(_ outcome: PasteOutcome, target: NSRunningApplication?) {
        switch outcome {
        case .sent: lastOutcome = "sent"
        case .manual(let failure): lastOutcome = failure.rawValue
        }
        lastTarget = target?.bundleIdentifier
        refresh(checkFresh: !permissions.canPaste)
    }

    func copyReport() {
        guard let data = reportData(), let text = String(data: data, encoding: .utf8) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    private func reportData() -> Data? {
        struct Report: Encodable {
            let version: String
            let processID: Int32
            let application: String
            let signing: AppSigningIdentity
            let permissions: PastePermissionSnapshot
            let freshPermissions: PastePermissionSnapshot?
            let needsRestart: Bool
            let lastOutcome: String?
            let targetApplication: String?
        }
        let value = Report(version: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "",
                           processID: ProcessInfo.processInfo.processIdentifier, application: Bundle.main.bundleURL.path,
                           signing: signing, permissions: permissions, freshPermissions: freshPermissions,
                           needsRestart: needsRestart, lastOutcome: lastOutcome, targetApplication: lastTarget)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try? encoder.encode(value)
    }

    private func writeReport() {
        guard Bundle.main.bundleIdentifier == "local.jianzhan.clipboard", let data = reportData(),
              let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return }
        let folder = base.appending(path: "剪栈/Diagnostics")
        let file = folder.appending(path: "paste-permissions.json")
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            if (try? Data(contentsOf: file)) == data { return }
            try data.write(to: file, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        } catch {}
    }
}
