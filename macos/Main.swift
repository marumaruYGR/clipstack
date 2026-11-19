import AppKit
import Carbon
import QuartzCore
import ServiceManagement
import SwiftUI

final class QuickPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class GlassPanelSurface<Content: View>: NSView {
    private let host: NSHostingView<Content>

    init(radius: CGFloat, style: NSGlassEffectView.Style = .regular, content: Content) {
        host = NSHostingView(rootView: content)
        super.init(frame: .zero)

        wantsLayer = true
        layer?.cornerRadius = radius
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true

        let glass = NSGlassEffectView(frame: .zero)
        glass.style = style
        glass.cornerRadius = radius
        glass.contentView = host
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        NSLayoutConstraint.activate([
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("Glass panels are loaded programmatically") }

    var contentSize: NSSize { host.fittingSize }
}

@MainActor final class Controller: NSObject, NSApplicationDelegate, NSWindowDelegate, ObservableObject {
    let store = Store()
    let updates = UpdateService()
    private let notices = SystemNotices()
    private let pasteCoordinator = PasteCoordinator()
    @Published var page: Page? = .history {
        didSet {
            guard page != oldValue else { return }
            focusedClipID = nil
            recordingHotkey = false
        }
    }
    @Published private(set) var searchText = ""
    @Published private(set) var focusedClipID: UUID?
    @Published var greeting = ""
    @Published var pinned = false {
        didSet {
            quick?.level = pinned ? .statusBar : .floating
            if !pinned && quick?.isKeyWindow == false { quick?.orderOut(nil) }
        }
    }
    @Published var toasts = UserDefaults.standard.object(forKey: "toasts") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(toasts, forKey: "toasts")
            if !toasts { hideToast() }
            if !toasts { pendingManualCount = nil }
        }
    }
    @Published var noticeAnimation = UserDefaults.standard.object(forKey: "noticeAnimation") as? Bool ?? true {
        didSet { UserDefaults.standard.set(noticeAnimation, forKey: "noticeAnimation") }
    }
    @Published var startupNotifications = UserDefaults.standard.object(forKey: "startupNotifications") as? Bool ?? true {
        didSet { UserDefaults.standard.set(startupNotifications, forKey: "startupNotifications") }
    }
    @Published var pasteMode = PasteMode(rawValue: UserDefaults.standard.string(forKey: "pasteMode") ?? "") ?? .direct {
        didSet { UserDefaults.standard.set(pasteMode.rawValue, forKey: "pasteMode") }
    }
    @Published var theme = UserDefaults.standard.string(forKey: "theme") ?? "system" {
        didSet {
            UserDefaults.standard.set(theme, forKey: "theme")
            applyTheme()
        }
    }
    @Published var hotkeyLabel = "未注册"
    @Published private(set) var hotkeyKeys: [String] = []
    @Published var hotkeyMessage: String?
    @Published private(set) var recordingHotkey = false
    @Published var login = SMAppService.mainApp.status == .enabled
    private var status: NSStatusItem!
    private var main: NSWindow?
    weak var searchField: NSSearchField?
    private(set) var quick: QuickPanel?
    private var preview: NSPanel?
    private weak var previewSource: NSWindow?
    private var toast: NSPanel?
    private var timer: Timer?
    private var toastTimer: Timer?
    private var pendingManualCount: Int?
    private var backupWindow: BackupWindowController?
    private var documentWindow: DocumentWindowController?
    private var hotkey: RegisteredHotkey?
    private var hotkeyHandler: EventHandlerRef?
    private var activeModal = false
    private var monitors: [Any] = []
    private var pasteTap: CFMachPort?
    private var pasteSource: CFRunLoopSource?
    private var lastHotkeyID: UInt32 = 0
    private var sessionObserver: NSObjectProtocol?
    private let defaults = UserDefaults.standard
    private lazy var greetings: [String] = {
        guard let url = Bundle.main.url(forResource: "Greetings", withExtension: "txt"),
              let text = try? String(contentsOf: url, encoding: .utf8) else { return ["茶还温着。"] }
        return text.split(whereSeparator: \.isNewline).map(String.init)
    }()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        makeMenu()
        applyTheme()
        if !defaults.bool(forKey: "didConfigureLogin") && Bundle.main.bundlePath.hasPrefix("/Applications/") {
            defaults.set(true, forKey: "didConfigureLogin")
            setLogin(true)
        }
        installHotkeyHandler()
        if !defaults.bool(forKey: "hotkeyDisabled") {
            if let savedCode = defaults.object(forKey: "hotkeyCode") as? Int, let code = UInt32(exactly: savedCode),
               let savedModifiers = defaults.object(forKey: "hotkeyModifiers") as? Int, let modifiers = UInt32(exactly: savedModifiers) {
                registerHotkey(HotkeyChord(code: code, modifiers: modifiers))
            } else { restoreHotkey() }
        }

        timer = Timer(timeInterval: 0.28, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        RunLoop.main.add(timer!, forMode: .common)
        if let local = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event)
        }) { monitors.append(local) }
        if let global = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, !self.activeModal else { return }
                self.closePreview()
                if !self.pinned { self.quick?.orderOut(nil) }
            }
        }) { monitors.append(global) }
        refreshPasteMonitor()
        sessionObserver = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.willPowerOffNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.store.endSession() }
        }
        updates.start(notices: notices)
        if startupNotifications {
            Task {
                _ = await notices.send("剪栈已启动，正在后台运行。单击菜单栏图标打开快捷收纳区。",
                                       id: "startup", requestPermission: true)
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        return false
    }

    private func makeMenu() {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = status.button {
            button.image = NSImage(systemSymbolName: "clipboard", accessibilityDescription: "剪栈")
            button.image?.isTemplate = true
            button.toolTip = "剪栈"
            button.target = self
            button.action = #selector(statusClicked)
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        let root = NSMenu()
        let appItem = NSMenuItem(title: "剪栈", action: nil, keyEquivalent: "")
        root.addItem(appItem)
        let appMenu = NSMenu(title: "剪栈")
        appMenu.addItem(withTitle: "退出剪栈", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q").target = NSApp
        appItem.submenu = appMenu
        let edit = NSMenuItem()
        root.addItem(edit)
        let editMenu = NSMenu(title: "编辑")
        for (title, action, key) in [
            ("剪切", #selector(NSText.cut(_:)), "x"), ("复制", #selector(NSText.copy(_:)), "c"),
            ("粘贴", #selector(NSText.paste(_:)), "v"), ("全选", #selector(NSText.selectAll(_:)), "a")
        ] {
            editMenu.addItem(withTitle: title, action: action, keyEquivalent: key)
        }
        edit.submenu = editMenu
        NSApp.mainMenu = root
    }

    func applicationDockMenu(_ sender: NSApplication) -> NSMenu? {
        let menu = NSMenu()
        menu.addItem(withTitle: "检索剪贴板", action: #selector(searchFromDock), keyEquivalent: "").target = self
        return menu
    }

    @objc private func searchFromDock() {
        showMain()
        main?.contentView?.layoutSubtreeIfNeeded()
        focusSearch()
    }

    private func focusSearch() {
        guard let searchField, main?.makeFirstResponder(searchField) == true else { return }
        searchField.selectText(nil)
    }

    @objc private func statusClicked() {
        if NSApp.currentEvent?.type == .rightMouseUp {
            let menu = NSMenu()
            menu.addItem(withTitle: "显示主界面", action: #selector(openMain), keyEquivalent: "").target = self
            menu.addItem(withTitle: "退出", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "").target = NSApp
            if let button = status.button { menu.popUp(positioning: nil, at: NSPoint(x: 0, y: -2), in: button) }
        } else if (NSApp.currentEvent?.clickCount ?? 1) <= 1 { toggleQuick() }
    }

    @objc private func openMain() { showMain() }

    func showMain() {
        if !pinned { quick?.orderOut(nil) }
        if main?.isVisible != true && main?.isMiniaturized != true {
            showPage(.history)
            greeting = greetings.randomElement() ?? "茶还温着。"
        }
        if main == nil {
            let size = NSSize(width: 980, height: 635)
            let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                                  styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                                  backing: .buffered, defer: false)
            window.title = "剪栈"
            window.isReleasedWhenClosed = false
            window.contentMinSize = NSSize(width: 720, height: 510)
            window.delegate = self
            let split = WorkspaceSplitController(store: store, app: self)
            window.contentViewController = split
            split.installToolbar(in: window)
            window.setContentSize(size)
            window.setFrameAutosaveName("JianZhanMain")
            if !window.setFrameUsingName("JianZhanMain") { window.center() }
            main = window
            applyTheme()
        }
        if NSApp.activationPolicy() != .regular { NSApp.setActivationPolicy(.regular) }
        NSApp.activate()
        if main?.isMiniaturized == true { main?.deminiaturize(nil) }
        main?.makeKeyAndOrderFront(nil)
    }

    private func toggleQuick() {
        if quick?.isVisible == true { quick?.orderOut(nil); return }
        guard status.button?.window != nil else { return }
        if greeting.isEmpty { greeting = greetings.randomElement() ?? "茶还温着。" }
        let panel = quick ?? QuickPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        panel.level = pinned ? .statusBar : .floating
        panel.delegate = self
        if quick == nil {
            panel.contentView = GlassPanelSurface(radius: 24, content: QuickView(store: store, app: self))
        }
        quick = panel
        applyTheme()
        positionQuick()
        panel.makeKeyAndOrderFront(nil)
    }

    private func positionQuick() {
        guard let panel = quick, let surface = panel.contentView as? GlassPanelSurface<QuickView>,
              let button = status.button, let bar = button.window else { return }
        let anchor = bar.convertToScreen(button.convert(button.bounds, to: nil))
        let screen = bar.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? .zero
        let size = surface.contentSize
        guard size.width > 0, size.height > 0 else { return }
        let x = max(screen.minX, min(anchor.midX - size.width / 2, screen.maxX - size.width))
        let frame = NSRect(x: x, y: anchor.minY - size.height - 2, width: size.width, height: size.height)
        if panel.frame != frame { panel.setFrame(frame, display: true) }
    }

    func windowDidResignKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow else { return }
        if window === preview { closePreview() }
        if window === quick && !pinned && !activeModal { quick?.orderOut(nil) }
        if window === main { interruptHotkeyRecording() }
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow else { return }
        if window === preview {
            preview = nil
            previewSource = nil
        } else if window === main {
            recordingHotkey = false
            NSApp.setActivationPolicy(.accessory)
        }
    }

    func applicationDidResignActive(_ notification: Notification) {
        closePreview()
        interruptHotkeyRecording()
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        refreshPasteMonitor()
    }

    private func refreshPasteMonitor() {
        let allowed = CGPreflightListenEventAccess() || PasteCoordinator.authorized
        if !allowed, let pasteTap {
            CFMachPortInvalidate(pasteTap)
            if let pasteSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), pasteSource, .commonModes) }
            self.pasteTap = nil
            pasteSource = nil
        }
        guard allowed, pasteTap == nil else { return }
        let mask = CGEventMask(1) << CGEventType.keyDown.rawValue
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap,
                                          options: .listenOnly, eventsOfInterest: mask,
                                          callback: { _, type, event, userInfo in
            guard let userInfo else { return Unmanaged.passUnretained(event) }
            let controller = Unmanaged<Controller>.fromOpaque(userInfo).takeUnretainedValue()
            MainActor.assumeIsolated {
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                    if let tap = controller.pasteTap { CGEvent.tapEnable(tap: tap, enable: true) }
                } else if event.getIntegerValueField(.keyboardEventKeycode) == kVK_ANSI_V
                    && event.flags.contains(.maskCommand) {
                    if event.getIntegerValueField(.eventSourceUserData) != PasteCoordinator.eventTag {
                        controller.observedPaste()
                    }
                }
            }
            return Unmanaged.passUnretained(event)
        }, userInfo: Unmanaged.passUnretained(self).toOpaque()),
              let source = CFMachPortCreateRunLoopSource(nil, tap, 0) else { return }
        pasteTap = tap
        pasteSource = source
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
    }

    private func tick() {
        if store.poll() {
            pasteCoordinator.cancel()
            pendingManualCount = nil
            showToast("复制成功，按 ⌘+V 粘贴")
        }
        if quick?.isVisible == true { positionQuick() }
        if let error = store.error, !activeModal, main?.isVisible == true {
            store.error = nil
            _ = alert(error, buttons: ["好"])
        }
    }

    private func handleKey(_ event: NSEvent) -> NSEvent? {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        let command = modifiers == .command
        if command && event.keyCode == kVK_ANSI_V { observedPaste() }
        let unmodified = modifiers.isEmpty
        if let preview, preview.isKeyWindow && unmodified && [kVK_Escape, kVK_Space].contains(Int(event.keyCode)) {
            if !event.isARepeat { closePreview(returnFocus: true) }
            return nil
        }
        if recordingHotkey && searchField?.currentEditor() != nil {
            recordingHotkey = false
            hotkeyMessage = nil
        }
        if recordingHotkey && main?.isKeyWindow == true && page == .settings {
            if event.isARepeat { return nil }
            if event.keyCode == kVK_Escape && unmodified {
                recordingHotkey = false
                hotkeyMessage = nil
                return nil
            }
            let chord = HotkeyChord(event)
            guard chord.hasRequiredModifier else {
                hotkeyMessage = "请按下包含 Control、Option 或 Command 的组合键。"
                return nil
            }
            registerHotkey(chord)
            return nil
        }
        if event.keyCode == kVK_Escape, quick?.isKeyWindow == true {
            quick?.orderOut(nil)
            return nil
        }
        if !activeModal && main?.isKeyWindow == true {
            if command && event.keyCode == kVK_ANSI_F {
                focusSearch()
                return nil
            }
            if unmodified && event.keyCode == kVK_Escape && isSearching,
               (searchField?.currentEditor() as? NSTextView)?.hasMarkedText() != true {
                main?.makeFirstResponder(nil)
                updateSearchText("")
                return nil
            }
        }
        if unmodified && event.keyCode == kVK_Space && !activeModal {
            let hasLibraryFocus = main?.isKeyWindow == true && !(main?.firstResponder is NSTextView)
                && (isSearching || page != .settings)
            if event.isARepeat && (quick?.isKeyWindow == true || hasLibraryFocus) {
                return nil
            }
            if quick?.isKeyWindow == true, let clip = store.history.first {
                showPreview(clip)
                return nil
            }
            if hasLibraryFocus, let clip = selectedForPreview() {
                showPreview(clip)
                return nil
            }
        }
        if !activeModal && main?.isKeyWindow == true && !(main?.firstResponder is NSTextView)
            && (page == .history || isSearching) {
            if command && event.keyCode == kVK_ANSI_A {
                store.selection = Set(visibleHistory.map(\.id))
                return nil
            }
            if !store.selection.isEmpty && [kVK_Delete, kVK_ForwardDelete].contains(Int(event.keyCode)) {
                store.remove(store.selection.intersection(visibleHistory.map(\.id)))
                return nil
            }
        }
        return event
    }

    func choose(_ clip: Clip) {
        pasteCoordinator.cancel()
        pendingManualCount = nil
        guard store.use(clip) else { store.error = "无法写入系统剪贴板。"; return }
        let count = NSPasteboard.general.changeCount
        if pasteMode == .manual {
            pendingManualCount = toasts ? count : nil
            showToast("已选取，按 ⌘+V 粘贴")
        } else {
            hideToast()
            closePreview()
            pasteCoordinator.paste(changeCount: count) { [weak self] sent in
                guard let self, NSPasteboard.general.changeCount == count else { return }
                if !self.pinned { self.quick?.orderOut(nil) }
                self.showToast(sent ? "已发送粘贴" : "已复制，请按 ⌘+V 粘贴")
            }
        }
        showFirstPasteTip()
    }

    private func observedPaste() {
        hideToast()
        guard let count = pendingManualCount,
              NSPasteboard.general.changeCount == count,
              NSWorkspace.shared.frontmostApplication?.processIdentifier != ProcessInfo.processInfo.processIdentifier else { return }
        pendingManualCount = nil
        Task {
            try? await Task.sleep(for: .milliseconds(90))
            guard NSPasteboard.general.changeCount == count else { return }
            showToast("已发送粘贴")
        }
    }

    private func showFirstPasteTip() {
        let mode = pasteMode
        let key = "seenPasteMode-\(mode.rawValue)"
        guard !defaults.bool(forKey: key) else { return }
        Task {
            let message = mode == .direct
                ? "直接上屏已开启。点选记录后会切回原应用并尝试粘贴，可在设置中切换为手动粘贴。"
                : "手动粘贴已开启。点选记录后，在目标应用按 ⌘+V 粘贴，可在设置中切换为直接上屏。"
            if await notices.send(message, id: key, requestPermission: true) { defaults.set(true, forKey: key) }
        }
    }

    func authorizePaste() {
        PasteCoordinator.requestAuthorization()
        refreshPasteMonitor()
    }

    func selectCard(_ clip: Clip, favorite: Bool) {
        let previous = focusedClipID
        focusedClipID = clip.id
        if !favorite {
            if NSEvent.modifierFlags.contains(.command) {
                if !store.selection.insert(clip.id).inserted { store.selection.remove(clip.id) }
                return
            } else if NSEvent.modifierFlags.contains(.shift), let previous,
                      let start = visibleHistory.firstIndex(where: { $0.id == previous }),
                      let end = visibleHistory.firstIndex(where: { $0.id == clip.id }) {
                store.selection = Set(visibleHistory[min(start, end)...max(start, end)].map(\.id))
                return
            } else {
                store.selection = [clip.id]
            }
        }
        choose(clip)
    }

    var isSearching: Bool { searchText.contains { !$0.isWhitespace } }

    func updateSearchText(_ value: String) {
        guard value != searchText else { return }
        searchText = value
        focusedClipID = nil
        store.selection.removeAll()
        recordingHotkey = false
    }

    func showPage(_ destination: Page) {
        searchField?.abortEditing()
        updateSearchText("")
        page = destination
    }

    private var visibleHistory: [Clip] {
        isSearching ? store.search(searchText).history : store.history
    }

    private func selectedForPreview() -> Clip? {
        if isSearching {
            let results = store.search(searchText)
            let entries = results.history + results.favorites
            return entries.first { $0.id == focusedClipID }
                ?? results.history.first { store.selection.contains($0.id) }
                ?? entries.first
        }
        switch page {
        case .history:
            let selected = store.history.filter { store.selection.contains($0.id) }
            return selected.first { $0.id == focusedClipID } ?? selected.first ?? store.history.first
        case .favorites:
            return store.favorites.first { $0.id == focusedClipID } ?? store.favorites.first
        default:
            return nil
        }
    }

    func showPreview(_ clip: Clip) {
        let source = NSApp.keyWindow ?? main
        closePreview()
        let screen = source?.screen ?? NSScreen.main
        let visible = screen?.visibleFrame.insetBy(dx: 20, dy: 20)
        let size = NSSize(width: min(720, visible?.width ?? 720), height: min(510, visible?.height ?? 510))
        let panel = QuickPanel(contentRect: NSRect(origin: .zero, size: size),
                               styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        panel.title = "快速预览"
        panel.isReleasedWhenClosed = false
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = true
        panel.contentMinSize = NSSize(width: 400, height: 280)
        panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        panel.contentViewController = NSHostingController(rootView: PreviewWindowView(clip: clip))
        panel.setContentSize(size)
        panel.delegate = self
        if let visible {
            let anchor = main?.isVisible == true && main?.isMiniaturized == false ? main!.frame : visible
            let frame = panel.frame
            panel.setFrameOrigin(NSPoint(
                x: max(visible.minX, min(anchor.midX - frame.width / 2, visible.maxX - frame.width)),
                y: max(visible.minY, min(anchor.midY - frame.height / 2, visible.maxY - frame.height))
            ))
        } else {
            panel.center()
        }
        preview = panel
        previewSource = source
        applyTheme()
        NSApp.activate()
        panel.makeKeyAndOrderFront(nil)
    }

    private func closePreview(returnFocus: Bool = false) {
        guard let panel = preview else { return }
        let source = previewSource
        preview = nil
        previewSource = nil
        panel.close()
        if returnFocus && NSApp.isActive && source?.isVisible == true { source?.makeKey() }
    }

    func showToast(_ message: String) {
        guard toasts else { return }
        hideToast()
        let point = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(point) }) ?? NSScreen.main else { return }
        let style: NSGlassEffectView.Style = NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency ? .regular : .clear
        let surface = GlassPanelSurface(radius: 20, style: style, content: CopyBanner(message: message))
        let size = surface.contentSize
        let panel = NSPanel(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.contentView = surface
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.hidesOnDeactivate = false
        panel.level = .statusBar
        panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        let visible = screen.visibleFrame
        panel.setFrameOrigin(NSPoint(x: visible.midX - size.width / 2, y: visible.minY + 34))
        toast = panel
        applyTheme()
        let animated = noticeAnimation && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        let destination = panel.frame
        if animated {
            panel.setFrame(destination.offsetBy(dx: 0, dy: -6), display: false)
            panel.alphaValue = 0
        }
        panel.orderFrontRegardless()
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.5
                context.timingFunction = CAMediaTimingFunction(controlPoints: 0.16, 1, 0.3, 1)
                panel.animator().alphaValue = 1
                panel.animator().setFrame(destination, display: true)
            }
        }
        toastTimer = Timer.scheduledTimer(withTimeInterval: animated ? 2.5 : 3, repeats: false) { [weak self, weak panel] _ in
            MainActor.assumeIsolated {
                guard let self, let panel, self.toast === panel else { return }
                if animated {
                    NSAnimationContext.runAnimationGroup { context in
                        context.duration = 0.5
                        context.timingFunction = CAMediaTimingFunction(controlPoints: 0.7, 0, 0.84, 0)
                        panel.animator().alphaValue = 0
                        panel.animator().setFrame(destination.offsetBy(dx: 0, dy: -4), display: true)
                    } completionHandler: {
                        MainActor.assumeIsolated { if self.toast === panel { self.hideToast() } }
                    }
                } else { self.hideToast() }
            }
        }
    }

    private func hideToast() {
        toastTimer?.invalidate()
        toast?.orderOut(nil)
        toast = nil
    }

    private func applyTheme() {
        let appearance: NSAppearance? = switch theme {
        case "light": NSAppearance(named: .aqua)
        case "dark": NSAppearance(named: .darkAqua)
        default: nil
        }
        [main, quick, toast, preview].forEach { $0?.appearance = appearance }
    }

    @objc func addContent() {
        let input = NSTextView(frame: NSRect(x: 0, y: 0, width: 350, height: 115))
        input.isRichText = false
        input.isVerticallyResizable = true
        input.autoresizingMask = [.width]
        let scroll = NSScrollView(frame: input.frame)
        scroll.documentView = input
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        if alert("添加内容", detail: "输入要放入剪贴板历史的文字。", buttons: ["添加", "取消"], accessory: scroll, responder: input) == .alertFirstButtonReturn,
           !input.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            let clip = Clip.manual(input.string)
            pasteCoordinator.cancel()
            pendingManualCount = nil
            store.add(clip)
            if !store.use(clip) { store.error = "无法写入系统剪贴板。" }
        }
    }

    @objc func clearHistory() {
        if alert("清空全部剪贴板内容？", detail: "系统剪贴板和剪栈历史将一起清空，收藏继续保留。",
                 buttons: ["清空全部", "取消"]) == .alertFirstButtonReturn {
            clearAllContents()
        }
    }

    private func clearAllContents() {
        if store.clear() {
            pasteCoordinator.cancel()
            pendingManualCount = nil
            focusedClipID = nil
            hideToast()
        }
    }

    func openBackup(exporting: Bool) {
        guard let main else { return }
        if let sheet = main.attachedSheet { sheet.makeKeyAndOrderFront(nil); return }
        let controller = BackupWindowController(store: store, exporting: exporting)
        backupWindow = controller
        main.beginSheet(controller.window!)
    }

    func showManual() {
        showDocument(title: "剪栈使用手册",
                     file: Bundle.main.url(forResource: "index", withExtension: "html", subdirectory: "Manual"))
    }

    func showChangelog() {
        let text = Bundle.main.url(forResource: "CHANGELOG", withExtension: "md")
            .flatMap { try? String(contentsOf: $0, encoding: .utf8) }
        showDocument(title: "剪栈更新日志", text: text)
    }

    func showUpdateNotes() {
        showDocument(title: "版本 \(updates.available?.version ?? "") 更新日志", text: updates.available?.notes)
    }

    private func showDocument(title: String, file: URL? = nil, text: String? = nil) {
        guard let main else { return }
        if let sheet = main.attachedSheet { sheet.makeKeyAndOrderFront(nil); return }
        let controller = DocumentWindowController(title: title, file: file, text: text)
        documentWindow = controller
        main.beginSheet(controller.window!)
    }

    func confirmUnfavorite(_ clip: Clip) {
        if alert("移除这条收藏？", buttons: ["移除", "取消"]) == .alertFirstButtonReturn {
            store.toggleFavorite(clip)
        }
    }

    private func alert(_ title: String, detail: String = "", buttons: [String], accessory: NSView? = nil, responder: NSView? = nil) -> NSApplication.ModalResponse {
        let previousApp = NSWorkspace.shared.frontmostApplication
        activeModal = true
        defer {
            activeModal = false
            if let quick, quick.isVisible, !pinned { quick.makeKeyAndOrderFront(nil) }
            else if previousApp?.processIdentifier != ProcessInfo.processInfo.processIdentifier { previousApp?.activate() }
        }
        let message = NSAlert()
        message.messageText = title
        message.informativeText = detail
        buttons.forEach { _ = message.addButton(withTitle: $0) }
        message.accessoryView = accessory
        if let responder { message.window.initialFirstResponder = responder }
        NSApp.activate()
        return message.runModal()
    }

    func setLogin(_ enabled: Bool) {
        do {
            let service = SMAppService.mainApp
            if enabled { try service.register() } else { try service.unregister() }
            login = service.status == .enabled || service.status == .requiresApproval
            if service.status == .requiresApproval {
                _ = alert("请在系统设置中允许剪栈登录时启动。", buttons: ["好"])
            }
        } catch { store.error = "无法设置登录时启动。\(error.localizedDescription)" }
    }

    func moveFavorites() {
        let dialog = NSOpenPanel()
        dialog.canChooseDirectories = true
        dialog.canChooseFiles = false
        dialog.canCreateDirectories = true
        dialog.prompt = "移动到此处"
        guard dialog.runModal() == .OK, let folder = dialog.url,
              store.moveFavorites(to: folder.appending(path: "剪栈收藏夹.favorites")) else { return }
        if alert("收藏夹已移动", detail: "请确认重启剪栈。", buttons: ["确认重启", "稍后"]) == .alertFirstButtonReturn {
            let task = Process()
            task.executableURL = URL(fileURLWithPath: "/bin/sh")
            task.arguments = ["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open \"$2\"", "restart", String(ProcessInfo.processInfo.processIdentifier), Bundle.main.bundlePath]
            do { try task.run(); NSApp.terminate(nil) }
            catch { store.error = "无法自动重启，请重新打开剪栈。" }
        }
    }

    func recordHotkey() {
        let startRecording = !recordingHotkey
        if startRecording { main?.makeFirstResponder(nil) }
        recordingHotkey = startRecording
        hotkeyMessage = nil
    }

    private func interruptHotkeyRecording() {
        guard recordingHotkey else { return }
        recordingHotkey = false
        hotkeyMessage = "录入已中断，请重新点按快捷键录入。"
    }

    func restoreHotkey() {
        registerHotkey(.standard)
    }

    func disableHotkey() {
        removeHotkey()
        recordingHotkey = false
        hotkeyMessage = nil
        defaults.set(true, forKey: "hotkeyDisabled")
    }

    private func removeHotkey() {
        if let hotkey { UnregisterEventHotKey(hotkey.reference) }
        hotkey = nil
        hotkeyLabel = "未注册"
        hotkeyKeys = []
    }

    private static let hotkeySignature: OSType = 0x4A5A4854

    private func installHotkeyHandler() {
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, pointer in
            guard let event, let pointer else { return OSStatus(eventNotHandledErr) }
            let controller = Unmanaged<Controller>.fromOpaque(pointer).takeUnretainedValue()
            var id = EventHotKeyID()
            let result = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                           nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard result == noErr else { return OSStatus(eventNotHandledErr) }
            return MainActor.assumeIsolated {
                guard id.signature == Controller.hotkeySignature, let registered = controller.hotkey, id.id == registered.id
                else { return OSStatus(eventNotHandledErr) }
                if controller.recordingHotkey && controller.main?.isKeyWindow == true {
                    controller.registerHotkey(registered.chord)
                } else {
                    controller.toggleQuick()
                }
                return noErr
            }
        }, 1, &type, Unmanaged.passUnretained(self).toOpaque(), &hotkeyHandler)
    }

    @discardableResult private func registerHotkey(_ chord: HotkeyChord) -> Bool {
        recordingHotkey = false
        guard chord.hasRequiredModifier else {
            hotkeyMessage = "快捷键需包含 Control、Option 或 Command。"
            return false
        }
        if let conflict = SystemHotkeys.conflict(for: chord) {
            if hotkey?.chord == chord, conflict != .unavailable { removeHotkey() }
            hotkeyMessage = conflict.message(for: chord)
            return false
        }
        if hotkey?.chord == chord {
            hotkeyMessage = nil
            return true
        }
        guard hotkeyHandler != nil else {
            hotkeyMessage = HotkeyConflict.unavailable.message(for: chord)
            return false
        }

        lastHotkeyID &+= 1
        let id = EventHotKeyID(signature: Self.hotkeySignature, id: lastHotkeyID)
        switch SystemHotkeys.register(chord, id: id) {
        case .failure(let error):
            hotkeyMessage = error.conflict.message(for: chord)
            return false
        case .success(let registration):
            removeHotkey()
            hotkey = registration
            hotkeyLabel = chord.label
            hotkeyKeys = chord.keyNames
            hotkeyMessage = nil
            defaults.set(false, forKey: "hotkeyDisabled")
            defaults.set(Int(chord.code), forKey: "hotkeyCode")
            defaults.set(Int(chord.modifiers), forKey: "hotkeyModifiers")
            defaults.set(chord.label, forKey: "hotkeyName")
            return true
        }
    }
}

@main struct JianZhan {
    @MainActor static func main() {
        if CommandLine.arguments.contains("--self-test") {
            try! selfTest()
            return
        }

        do {
            guard let lock = try LoginSession.acquireLock() else {
                fputs("剪栈已在当前登录会话运行。\n", stderr)
                return
            }
            let app = NSApplication.shared
            let controller = Controller()
            app.delegate = controller
            withExtendedLifetime((lock, controller)) { app.run() }
        } catch {
            let app = NSApplication.shared
            app.setActivationPolicy(.accessory)
            app.activate()
            let alert = NSAlert(error: error)
            alert.messageText = "剪栈无法启动"
            alert.informativeText = "无法锁定当前登录会话。\n\(error.localizedDescription)"
            alert.runModal()
        }
    }

    @MainActor private static func selfTest() throws {
        let sample = Clip.manual("剪栈")
        let encoded = try PropertyListEncoder().encode([sample])
        let decoded = try PropertyListDecoder().decode([Clip].self, from: encoded)
        precondition(sample.text == "剪栈" && decoded == [sample])
        let html = Clip(items: [[NSPasteboard.PasteboardType.html.rawValue:
            Data("<p>剪<em>贴</em>板 &#x4E2D;&#25991; &amp; Ｃａｆé</p><script>隐藏内容</script>".utf8)
        ]])
        let searchable = foldedSearchText(html.searchableText)
        precondition(searchable == "剪贴板 中文 & cafe")

        let files = FileManager.default
        let directory = files.temporaryDirectory.appending(path: "JianZhanTests-\(UUID())")
        try files.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: directory) }
        let lockPath = directory.appending(path: "session.lock")
        var lock = try SessionLock.acquire(at: lockPath)
        precondition(lock != nil)
        let blocked = try SessionLock.acquire(at: lockPath)
        precondition(blocked == nil)
        lock = nil
        let reacquired = try SessionLock.acquire(at: lockPath)
        precondition(reacquired != nil)

        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let image = directory.appending(path: "图片 样例.png")
        let document = directory.appending(path: "文档 样例.txt")
        let video = directory.appending(path: "视频 样例.mov")
        guard let artwork = Bundle.main.url(forResource: "About", withExtension: "png") else {
            preconditionFailure("App 资源缺失")
        }
        try files.copyItem(at: artwork, to: image)
        for url in [document, video] { try Data("剪栈".utf8).write(to: url) }
        let refs = Clip.fileReferences([image, document, video])
        precondition(refs.write(board))
        guard let recaptured = Clip.read(board) else { preconditionFailure("无法读回文件剪贴板") }
        precondition(recaptured.files.map(\.url) == [image, document, video])
        precondition(recaptured.files.map(\.kind) == [.image, .document, .other])
        guard case .image = recaptured.files[0].preview(),
              case .document = recaptured.files[1].preview(),
              case .finder = recaptured.files[2].preview() else { preconditionFailure("文件预览分类错误") }
        let legacyType = NSPasteboard.PasteboardType("NSFilenamesPboardType")
        board.declareTypes([legacyType], owner: nil)
        precondition(board.setPropertyList([document.path], forType: legacyType))
        precondition(Clip.read(board)?.files.first?.url == document)

        let switchApp = HotkeyChord(code: UInt32(kVK_Tab), modifiers: UInt32(cmdKey))
        let paste = HotkeyChord(code: UInt32(kVK_ANSI_V), modifiers: UInt32(cmdKey))
        precondition(SystemHotkeys.conflict(for: switchApp) == .system)
        precondition(SystemHotkeys.conflict(for: paste) == .standardCommand)
        let rich = Clip(items: [[NSPasteboard.PasteboardType.html.rawValue: Data("<p>剪栈 <b>格式</b> 👋</p>".utf8)]])
        let samples = [Clip.manual("中文与表情 👋"), rich, refs,
                       Clip(items: [[NSPasteboard.PasteboardType.png.rawValue: try Data(contentsOf: image)]])]
        let backup = directory.appending(path: "sample.clipstack-backup")
        try PortableBackup.export(samples, password: "self-test passphrase", to: backup)
        let restored = try PortableBackup.read(backup, password: "self-test passphrase")
        precondition(restored.map(\.items) == samples.map(\.items))
        precondition(UpdateService.version("v1.1.0fix2") == [1, 1, 0, 2])
        print("剪栈自检通过：会话锁、文件分类、快捷键、富文本与图片备份往返、版本比较。")
    }
}
