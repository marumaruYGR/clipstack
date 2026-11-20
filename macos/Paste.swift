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
    private var authorizationRoundTrip = false

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
        guard let application else { return }
        if application.processIdentifier == ProcessInfo.processInfo.processIdentifier {
            authorizationRoundTrip = false
            return
        }
        if authorizationRoundTrip && application.bundleIdentifier == "com.apple.systempreferences" { return }
        guard
              application.activationPolicy == .regular, !application.isTerminated else { return }
        authorizationRoundTrip = false
        target = application
        targetWindow = nil
        targetElement = nil
        guard AXIsProcessTrusted() else { return }
        let accessibility = AXUIElementCreateApplication(application.processIdentifier)
        AXUIElementSetMessagingTimeout(accessibility, 0.15)
        targetWindow = Self.element(accessibility, attribute: kAXFocusedWindowAttribute)
        targetElement = Self.element(accessibility, attribute: kAXFocusedUIElementAttribute)
    }

    func captureTarget() {
        remember(NSWorkspace.shared.frontmostApplication)
    }

    func preserveTargetForAuthorization() { authorizationRoundTrip = true }

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

    static var authorized: Bool { CGPreflightPostEventAccess() }

    func paste(changeCount: Int, completion: @escaping (PasteOutcome) -> Void) {
        cancel()
        guard Self.authorized else { completion(.manual(.permissionUnavailable)); return }
        guard let target, !target.isTerminated else { completion(.manual(.targetUnavailable)); return }
        let window = targetWindow
        let element = targetElement
        task = Task { @MainActor in
            let failure = await self.prepare(target, window: window, element: element, changeCount: changeCount)
            guard !Task.isCancelled else { return }
            if let failure { completion(.manual(failure)); return }
            let outcome = await self.sendPaste(to: target, changeCount: changeCount)
            if !Task.isCancelled { completion(outcome) }
        }
    }

    static let eventTag: Int64 = 0x4A5A5041535445

    private func invalid(_ target: NSRunningApplication, changeCount: Int) -> PasteFailure? {
        guard !Task.isCancelled else { return .cancelled }
        guard !target.isTerminated else { return .targetUnavailable }
        guard NSPasteboard.general.changeCount == changeCount else { return .clipboardChanged }
        let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
        return front == target.processIdentifier || front == ProcessInfo.processInfo.processIdentifier || front == nil
            ? nil : .targetChanged
    }

    private func prepare(_ target: NSRunningApplication, window: AXUIElement?, element: AXUIElement?,
                         changeCount: Int) async -> PasteFailure? {
        let modifiers: [CGKeyCode] = [55, 54, 56, 60, 58, 61, 59, 62, 9]
        let releaseDeadline = ContinuousClock.now + .milliseconds(1500)
        while modifiers.contains(where: { CGEventSource.keyState(.hidSystemState, key: $0) }) {
            if let failure = invalid(target, changeCount: changeCount) { return failure }
            guard ContinuousClock.now < releaseDeadline else { return .modifiersHeld }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return .cancelled }
        }
        if let failure = invalid(target, changeCount: changeCount) { return failure }
        NSApp.yieldActivation(to: target)
        guard target.activate() || target.isActive else { return .activationUnavailable }

        let application = AXUIElementCreateApplication(target.processIdentifier)
        AXUIElementSetMessagingTimeout(application, 0.2)
        let accessibility = AXIsProcessTrusted()
        if accessibility, let window {
            var role: CFTypeRef?
            guard AXUIElementCopyAttributeValue(window, kAXRoleAttribute as CFString, &role) != .invalidUIElement
            else { return .focusUnavailable }
            _ = AXUIElementSetAttributeValue(window, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
            _ = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
            _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
        }
        if accessibility, let element { _ = AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) }
        let deadline = ContinuousClock.now + .milliseconds(2500)
        var stable: CFTypeRef?
        var since = ContinuousClock.now
        var activeSince: ContinuousClock.Instant?
        while ContinuousClock.now < deadline {
            if let failure = invalid(target, changeCount: changeCount) { return failure }
            var focus: CFTypeRef?
            let result = accessibility
                ? AXUIElementCopyAttributeValue(application, kAXFocusedUIElementAttribute as CFString, &focus)
                : AXError.apiDisabled
            if target.isActive, NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier,
               !NSApp.isActive {
                if activeSince == nil { activeSince = ContinuousClock.now }
                if result == .success, let focus {
                    if let stable, CFEqual(stable, focus) {
                        if ContinuousClock.now - since >= .milliseconds(100),
                           ContinuousClock.now - activeSince! >= .milliseconds(120) { return nil }
                    } else {
                        stable = focus
                        since = ContinuousClock.now
                    }
                } else if result == .attributeUnsupported || result == .noValue || result == .apiDisabled || result == .cannotComplete {
                    // AX focus inspection is optional. A stable foreground target
                    // and posting permission are sufficient for the native shortcut.
                    if ContinuousClock.now - activeSince! >= .milliseconds(250) { return nil }
                } else {
                    stable = nil
                    since = ContinuousClock.now
                }
            } else {
                stable = nil
                activeSince = nil
                since = ContinuousClock.now
            }
            do { try await Task.sleep(for: .milliseconds(8)) } catch { return .cancelled }
        }
        return .focusUnavailable
    }

    private func sendPaste(to target: NSRunningApplication, changeCount: Int) async -> PasteOutcome {
        if let failure = invalid(target, changeCount: changeCount) { return .manual(failure) }
        guard Self.authorized else { return .manual(.permissionUnavailable) }
        guard !IsSecureEventInputEnabled() else { return .manual(.secureInput) }
        guard target.isActive, !NSApp.isActive,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier else { return .manual(.targetChanged) }
        guard
              let source = CGEventSource(stateID: .privateState),
              let commandDown = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_Command), keyDown: true),
              let commandUp = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_Command), keyDown: false),
              let down = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: false) else { return .manual(.eventUnavailable) }
        source.localEventsSuppressionInterval = 0
        for event in [commandDown, down, up, commandUp] {
            event.flags = event === commandUp ? [] : .maskCommand
            event.setIntegerValueField(.eventSourceUserData, value: Self.eventTag)
        }
        commandDown.post(tap: .cghidEventTap)
        defer { commandUp.post(tap: .cghidEventTap) }
        do { try await Task.sleep(for: .milliseconds(12)) } catch { return .manual(.cancelled) }
        if let failure = invalid(target, changeCount: changeCount) { return .manual(failure) }
        guard target.isActive, !NSApp.isActive,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier else { return .manual(.targetChanged) }
        down.post(tap: .cghidEventTap)
        defer { up.post(tap: .cghidEventTap) }
        do { try await Task.sleep(for: .milliseconds(18)) } catch { return .manual(.cancelled) }
        return .sent
    }
}
