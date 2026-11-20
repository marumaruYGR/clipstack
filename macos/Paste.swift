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
    private var targetWindow: AXUIElement?
    private var targetElement: AXUIElement?
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
        targetWindow = nil
        targetElement = nil
        guard Self.authorized else { return }
        let accessibility = AXUIElementCreateApplication(application.processIdentifier)
        AXUIElementSetMessagingTimeout(accessibility, 0.15)
        targetWindow = Self.element(accessibility, attribute: kAXFocusedWindowAttribute)
        targetElement = Self.element(accessibility, attribute: kAXFocusedUIElementAttribute)
    }

    func captureTarget() {
        remember(NSWorkspace.shared.frontmostApplication)
    }

    private static func element(_ parent: AXUIElement, attribute: String) -> AXUIElement? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(parent, attribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }

    func cancel() {
        task?.cancel()
        task = nil
    }

    static var authorized: Bool { AXIsProcessTrusted() && CGPreflightPostEventAccess() }

    static func requestAuthorization() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        if AXIsProcessTrustedWithOptions(options) && !CGPreflightPostEventAccess() {
            _ = CGRequestPostEventAccess()
        }
    }

    func paste(changeCount: Int, completion: @escaping (Bool) -> Void) {
        cancel()
        guard let target, !target.isTerminated, Self.authorized else { completion(false); return }
        let window = targetWindow
        let element = targetElement
        task = Task { @MainActor in
            let ready = await self.prepare(target, window: window, element: element, changeCount: changeCount)
            guard !Task.isCancelled else { return }
            guard ready else { completion(false); return }
            let sent = await self.sendPaste(to: target, changeCount: changeCount)
            if !Task.isCancelled { completion(sent) }
        }
    }

    static let eventTag: Int64 = 0x4A5A5041535445

    private func unchanged(_ target: NSRunningApplication, changeCount: Int) -> Bool {
        guard !Task.isCancelled, !target.isTerminated,
              NSPasteboard.general.changeCount == changeCount else { return false }
        let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
        return front == target.processIdentifier || front == ProcessInfo.processInfo.processIdentifier || front == nil
    }

    private func prepare(_ target: NSRunningApplication, window: AXUIElement?, element: AXUIElement?,
                         changeCount: Int) async -> Bool {
        let modifiers: [CGKeyCode] = [55, 54, 56, 60, 58, 61, 59, 62, 9]
        let releaseDeadline = ContinuousClock.now + .milliseconds(1500)
        while modifiers.contains(where: { CGEventSource.keyState(.hidSystemState, key: $0) }) {
            guard unchanged(target, changeCount: changeCount), ContinuousClock.now < releaseDeadline else { return false }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return false }
        }
        guard unchanged(target, changeCount: changeCount) else { return false }
        NSApp.yieldActivation(to: target)
        guard target.activate() || target.isActive else { return false }

        let application = AXUIElementCreateApplication(target.processIdentifier)
        AXUIElementSetMessagingTimeout(application, 0.2)
        if let window {
            var role: CFTypeRef?
            guard AXUIElementCopyAttributeValue(window, kAXRoleAttribute as CFString, &role) != .invalidUIElement
            else { return false }
            _ = AXUIElementSetAttributeValue(window, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
            _ = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
            _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
        }
        if let element { _ = AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) }
        let deadline = ContinuousClock.now + .milliseconds(2500)
        var stable: CFTypeRef?
        var since = ContinuousClock.now
        var activeSince: ContinuousClock.Instant?
        while ContinuousClock.now < deadline {
            guard unchanged(target, changeCount: changeCount) else { return false }
            var focus: CFTypeRef?
            let result = AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &focus)
            if target.isActive, NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier,
               !NSApp.isActive {
                if activeSince == nil { activeSince = ContinuousClock.now }
                if result == .success, let focus {
                    if let stable, CFEqual(stable, focus) {
                        if ContinuousClock.now - since >= .milliseconds(100),
                           ContinuousClock.now - activeSince! >= .milliseconds(120) { return true }
                    } else {
                        stable = focus
                        since = ContinuousClock.now
                    }
                } else if result == .attributeUnsupported || result == .noValue {
                    // Some editors expose their window but no focused accessibility node.
                    if ContinuousClock.now - activeSince! >= .milliseconds(250) { return true }
                } else {
                    stable = nil
                    since = ContinuousClock.now
                }
            } else {
                stable = nil
                activeSince = nil
                since = ContinuousClock.now
            }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return false }
        }
        return false
    }

    private func sendPaste(to target: NSRunningApplication, changeCount: Int) async -> Bool {
        guard unchanged(target, changeCount: changeCount), target.isActive, !NSApp.isActive,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier,
              Self.authorized, !IsSecureEventInputEnabled(),
              let source = CGEventSource(stateID: .privateState),
              let commandDown = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_Command), keyDown: true),
              let commandUp = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_Command), keyDown: false),
              let down = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: false) else { return false }
        source.localEventsSuppressionInterval = 0
        for event in [commandDown, down, up, commandUp] {
            event.flags = event === commandUp ? [] : .maskCommand
            event.setIntegerValueField(.eventSourceUserData, value: Self.eventTag)
        }
        commandDown.post(tap: .cghidEventTap)
        defer { commandUp.post(tap: .cghidEventTap) }
        do { try await Task.sleep(for: .milliseconds(12)) } catch { return false }
        guard unchanged(target, changeCount: changeCount), target.isActive, !NSApp.isActive,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier else { return false }
        down.post(tap: .cghidEventTap)
        defer { up.post(tap: .cghidEventTap) }
        do { try await Task.sleep(for: .milliseconds(18)) } catch { return false }
        return true
    }
}
