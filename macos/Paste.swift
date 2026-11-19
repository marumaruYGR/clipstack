import AppKit
import ApplicationServices
import Carbon

enum PasteMode: String, CaseIterable, Identifiable {
    case direct, manual
    var id: Self { self }
    var title: String { self == .direct ? "直接上屏" : "手动粘贴" }
}

@MainActor final class PasteCoordinator {
    private(set) var target: NSRunningApplication?
    private var activationObserver: NSObjectProtocol?
    private var task: Task<Void, Never>?

    init() {
        remember(NSWorkspace.shared.frontmostApplication)
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] note in
            let application = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            MainActor.assumeIsolated { self?.remember(application) }
        }
    }

    private func remember(_ application: NSRunningApplication?) {
        guard let application, application.processIdentifier != ProcessInfo.processInfo.processIdentifier,
              application.activationPolicy == .regular, !application.isTerminated else { return }
        target = application
    }

    func cancel() {
        task?.cancel()
        task = nil
    }

    static var authorized: Bool { AXIsProcessTrusted() }

    static func requestAuthorization() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
    }

    func paste(changeCount: Int, completion: @escaping (Bool) -> Void) {
        cancel()
        guard let target, !target.isTerminated, Self.authorized else { completion(false); return }
        task = Task { @MainActor in
            let ready = await self.prepare(target, changeCount: changeCount)
            guard !Task.isCancelled else { return }
            guard ready, self.unchanged(target, changeCount: changeCount), target.isActive,
                  let source = CGEventSource(stateID: .combinedSessionState),
                  let down = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: true),
                  let up = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: false) else {
                completion(false)
                return
            }
            for event in [down, up] {
                event.flags = .maskCommand
                event.setIntegerValueField(.eventSourceUserData, value: Self.eventTag)
                event.postToPid(target.processIdentifier)
            }
            completion(true)
        }
    }

    static let eventTag: Int64 = 0x4A5A5041535445

    private func unchanged(_ target: NSRunningApplication, changeCount: Int) -> Bool {
        guard !Task.isCancelled, !target.isTerminated,
              NSPasteboard.general.changeCount == changeCount else { return false }
        let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
        return front == target.processIdentifier || front == ProcessInfo.processInfo.processIdentifier
    }

    private func prepare(_ target: NSRunningApplication, changeCount: Int) async -> Bool {
        let modifiers: [CGKeyCode] = [55, 54, 56, 60, 58, 61, 59, 62, 9]
        let releaseDeadline = ContinuousClock.now + .milliseconds(1500)
        while modifiers.contains(where: { CGEventSource.keyState(.combinedSessionState, key: $0) }) {
            guard unchanged(target, changeCount: changeCount), ContinuousClock.now < releaseDeadline else { return false }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return false }
        }
        guard unchanged(target, changeCount: changeCount) else { return false }
        if !target.isActive && !target.activate() { return false }

        let application = AXUIElementCreateApplication(target.processIdentifier)
        let deadline = ContinuousClock.now + .seconds(1)
        var stable: CFTypeRef?
        var since = ContinuousClock.now
        while ContinuousClock.now < deadline {
            guard unchanged(target, changeCount: changeCount) else { return false }
            var focus: CFTypeRef?
            let result = AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &focus)
            if target.isActive, result == .success, let focus {
                if let stable, CFEqual(stable, focus) {
                    if ContinuousClock.now - since >= .milliseconds(60) { return true }
                } else {
                    stable = focus
                    since = ContinuousClock.now
                }
            } else {
                stable = nil
                since = ContinuousClock.now
            }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return false }
        }
        return false
    }
}
