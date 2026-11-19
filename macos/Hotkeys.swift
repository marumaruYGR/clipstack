import AppKit
import Carbon

struct HotkeyChord: Equatable {
    private static let options: [(flag: NSEvent.ModifierFlags, mask: UInt32, glyph: String)] = [
        (.control, UInt32(controlKey), "⌃"),
        (.option, UInt32(optionKey), "⌥"),
        (.shift, UInt32(shiftKey), "⇧"),
        (.command, UInt32(cmdKey), "⌘"),
    ]
    private static let modifierMask = options.reduce(0) { $0 | $1.mask }
    static let standard = HotkeyChord(code: UInt32(kVK_ANSI_V), modifiers: UInt32(controlKey))

    let code: UInt32
    let modifiers: UInt32

    var hasRequiredModifier: Bool { modifiers & ~UInt32(shiftKey) != 0 }
    var label: String {
        let keys = Self.options.filter { modifiers & $0.mask != 0 }.map(\.glyph).joined()
        return keys + (hotkeyNames[code] ?? "键 \(code)")
    }

    var keyNames: [String] {
        let names = ["Control", "Option", "Shift", "Command"]
        return Self.options.enumerated().compactMap { index, option in
            modifiers & option.mask != 0 ? names[index] : nil
        } + [hotkeyNames[code] ?? "键 \(code)"]
    }

    init(code: UInt32, modifiers: UInt32) {
        self.code = code
        self.modifiers = modifiers & Self.modifierMask
    }

    init(_ event: NSEvent) {
        let modifiers = Self.options.filter { event.modifierFlags.contains($0.flag) }.reduce(0) { $0 | $1.mask }
        self.init(code: UInt32(event.keyCode), modifiers: modifiers)
    }
}

private let hotkeyNames = Dictionary(uniqueKeysWithValues: [
        ("A", kVK_ANSI_A), ("B", kVK_ANSI_B), ("C", kVK_ANSI_C), ("D", kVK_ANSI_D),
        ("E", kVK_ANSI_E), ("F", kVK_ANSI_F), ("G", kVK_ANSI_G), ("H", kVK_ANSI_H),
        ("I", kVK_ANSI_I), ("J", kVK_ANSI_J), ("K", kVK_ANSI_K), ("L", kVK_ANSI_L),
        ("M", kVK_ANSI_M), ("N", kVK_ANSI_N), ("O", kVK_ANSI_O), ("P", kVK_ANSI_P),
        ("Q", kVK_ANSI_Q), ("R", kVK_ANSI_R), ("S", kVK_ANSI_S), ("T", kVK_ANSI_T),
        ("U", kVK_ANSI_U), ("V", kVK_ANSI_V), ("W", kVK_ANSI_W), ("X", kVK_ANSI_X),
        ("Y", kVK_ANSI_Y), ("Z", kVK_ANSI_Z),
        ("0", kVK_ANSI_0), ("1", kVK_ANSI_1), ("2", kVK_ANSI_2), ("3", kVK_ANSI_3),
        ("4", kVK_ANSI_4), ("5", kVK_ANSI_5), ("6", kVK_ANSI_6), ("7", kVK_ANSI_7),
        ("8", kVK_ANSI_8), ("9", kVK_ANSI_9),
        ("空格", kVK_Space), ("Tab", kVK_Tab), ("Return", kVK_Return), ("Esc", kVK_Escape),
        ("←", kVK_LeftArrow), ("↑", kVK_UpArrow), ("→", kVK_RightArrow), ("↓", kVK_DownArrow),
        ("Delete", kVK_Delete), ("Forward Delete", kVK_ForwardDelete),
        ("`", kVK_ANSI_Grave), ("-", kVK_ANSI_Minus), ("=", kVK_ANSI_Equal),
        ("[", kVK_ANSI_LeftBracket), ("]", kVK_ANSI_RightBracket), ("\\", kVK_ANSI_Backslash),
        (";", kVK_ANSI_Semicolon), ("'", kVK_ANSI_Quote), (",", kVK_ANSI_Comma),
        (".", kVK_ANSI_Period), ("/", kVK_ANSI_Slash),
        ("F1", kVK_F1), ("F2", kVK_F2), ("F3", kVK_F3), ("F4", kVK_F4), ("F5", kVK_F5),
        ("F6", kVK_F6), ("F7", kVK_F7), ("F8", kVK_F8), ("F9", kVK_F9), ("F10", kVK_F10),
        ("F11", kVK_F11), ("F12", kVK_F12), ("F13", kVK_F13), ("F14", kVK_F14), ("F15", kVK_F15),
        ("F16", kVK_F16), ("F17", kVK_F17), ("F18", kVK_F18), ("F19", kVK_F19), ("F20", kVK_F20),
].map { (UInt32($0.1), $0.0) })

enum HotkeyConflict: Equatable {
    case system, standardCommand, occupied, unavailable

    func message(for chord: HotkeyChord) -> String {
        switch self {
        case .system: return "\(chord.label) 已被 macOS 的系统快捷键占用。"
        case .standardCommand: return "\(chord.label) 与 macOS 的标准操作快捷键冲突。"
        case .occupied: return "\(chord.label) 已被其他应用注册。"
        case .unavailable: return "macOS 无法验证或注册 \(chord.label)，请换一个组合键。"
        }
    }
}

enum SystemHotkeys {
    private static let commandKeys: Set<UInt32> = Set([
        kVK_ANSI_A, kVK_ANSI_C, kVK_ANSI_F, kVK_ANSI_H, kVK_ANSI_M, kVK_ANSI_N, kVK_ANSI_O,
        kVK_ANSI_P, kVK_ANSI_Q, kVK_ANSI_S, kVK_ANSI_V, kVK_ANSI_W, kVK_ANSI_X, kVK_ANSI_Z,
        kVK_Delete, kVK_ANSI_Comma,
    ].map(UInt32.init))

    @MainActor static func conflict(for chord: HotkeyChord) -> HotkeyConflict? {
        // Application switching and screen locking also have fixed shortcuts outside the preferences list.
        let command = UInt32(cmdKey)
        let shift = UInt32(shiftKey)
        let option = UInt32(optionKey)
        let control = UInt32(controlKey)
        if ([UInt32(kVK_Tab), UInt32(kVK_ANSI_Grave)].contains(chord.code)
            && (chord.modifiers == command || chord.modifiers == command | shift))
            || (chord.code == kVK_Escape && chord.modifiers == command | option)
            || (chord.code == kVK_ANSI_Q && chord.modifiers == command | control) {
            return .system
        }
        if (chord.modifiers == command && commandKeys.contains(chord.code))
            || (chord.modifiers == command | shift && chord.code == kVK_ANSI_Z)
            || (chord.modifiers == command | option && [UInt32(kVK_ANSI_H), UInt32(kVK_ANSI_M)].contains(chord.code)) {
            return .standardCommand
        }

        var result: Unmanaged<CFArray>?
        guard CopySymbolicHotKeys(&result) == noErr,
              let entries = result?.takeRetainedValue() as? [[String: NSNumber]] else { return .unavailable }
        let enabled = entries.contains {
            $0["kHISymbolicHotKeyEnabled"]?.boolValue == true
                && $0["kHISymbolicHotKeyCode"]?.uint32Value == chord.code
                && $0["kHISymbolicHotKeyModifiers"]?.uint32Value == chord.modifiers
        }
        return enabled ? .system : nil
    }

    @MainActor static func register(_ chord: HotkeyChord, id: EventHotKeyID) -> Result<RegisteredHotkey, RegistrationError> {
        let target = GetApplicationEventTarget()
        var probe: EventHotKeyRef?
        let probeStatus = RegisterEventHotKey(chord.code, chord.modifiers, id, target,
                                              UInt32(kEventHotKeyExclusive), &probe)
        guard probeStatus == noErr, let probe else {
            return .failure(RegistrationError(conflict: probeStatus == eventHotKeyExistsErr ? .occupied : .unavailable))
        }
        guard UnregisterEventHotKey(probe) == noErr else {
            return .failure(RegistrationError(conflict: .unavailable))
        }

        // The exclusive probe detects exclusive holders. Keep the real registration
        // nonexclusive so it does not silence other apps using shared hotkeys.
        var reference: EventHotKeyRef?
        let status = RegisterEventHotKey(chord.code, chord.modifiers, id, target, 0, &reference)
        guard status == noErr, let reference else {
            return .failure(RegistrationError(conflict: status == eventHotKeyExistsErr ? .occupied : .unavailable))
        }
        return .success(RegisteredHotkey(reference: reference, chord: chord, id: id.id))
    }

    struct RegistrationError: Error {
        let conflict: HotkeyConflict
    }
}

struct RegisteredHotkey {
    let reference: EventHotKeyRef
    let chord: HotkeyChord
    let id: UInt32
}
