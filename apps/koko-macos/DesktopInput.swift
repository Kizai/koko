@preconcurrency import ApplicationServices
import Carbon
import Foundation
import os

/// When KoKo last posted an input event, in `DispatchTime` uptime nanoseconds.
private let lastInjectedInput = OSAllocatedUnfairLock<UInt64>(initialState: 0)

private func inject(_ event: CGEvent) {
    lastInjectedInput.withLock { $0 = DispatchTime.now().uptimeNanoseconds }
    event.post(tap: .cghidEventTap)
}

/// Whether this Mac's own keyboard or mouse was used within `seconds`, after both KoKo's last
/// event and `start` (uptime nanoseconds). Comparing the times works whether or not macOS counts
/// KoKo's posted events as HID activity, and ignores the click that let the agent continue.
func localInputSinceInjection(within seconds: Double, since start: UInt64) -> Bool {
    let idle = CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: CGEventType(rawValue: ~0)!)
    let now = DispatchTime.now().uptimeNanoseconds
    let injected = lastInjectedInput.withLock { $0 }
    let quiet = Double(now &- max(injected, start)) / 1e9
    return idle < seconds && idle + 0.15 < quiet
}

enum DesktopError: String, Error {
    case inputPermissionRequired = "input_permission_required"
    case inputFailed = "input_failed"
    case invalidCommand = "invalid_command"
}

func hasInputPermission(request: Bool) -> Bool {
    if CGPreflightPostEventAccess() { return true }
    guard request else { return false }

    let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    if AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary) {
        _ = CGRequestPostEventAccess()
    }
    return CGPreflightPostEventAccess()
}

func click(_ x: Double, _ y: Double) throws {
    let bounds = CGDisplayBounds(CGMainDisplayID())
    let point = CGPoint(
        x: bounds.minX + bounds.width * x / 10_000,
        y: bounds.minY + bounds.height * y / 10_000
    )
    guard let down = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
          let up = CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
    else { throw DesktopError.inputFailed }
    down.setIntegerValueField(.mouseEventClickState, value: 1)
    up.setIntegerValueField(.mouseEventClickState, value: 1)
    // A click without explicit flags inherits whatever modifiers macOS last saw posted.
    down.flags = []
    up.flags = []
    inject(down)
    inject(up)
}

func postRemoteKey(_ action: String) throws {
    let mapping: (Int, CGEventFlags) = switch action {
    case "enter": (kVK_Return, [])
    case "escape": (kVK_Escape, [])
    case "tab": (kVK_Tab, [])
    case "backspace": (kVK_Delete, [])
    case "space": (kVK_Space, [])
    case "up": (kVK_UpArrow, [])
    case "down": (kVK_DownArrow, [])
    case "left": (kVK_LeftArrow, [])
    case "right": (kVK_RightArrow, [])
    case "copy": (kVK_ANSI_C, .maskCommand)
    case "paste": (kVK_ANSI_V, .maskCommand)
    case "select_all": (kVK_ANSI_A, .maskCommand)
    case "undo": (kVK_ANSI_Z, .maskCommand)
    default: throw DesktopError.invalidCommand
    }
    try postKey(CGKeyCode(mapping.0), flags: mapping.1, unicode: [])
}

/// Modifier keys, in the order they are pressed for a shortcut.
private let shortcutModifiers: [(CGEventFlags, Int)] = [
    (.maskCommand, kVK_Command), (.maskControl, kVK_Control), (.maskAlternate, kVK_Option), (.maskShift, kVK_Shift),
]

/// Posts a key with its modifiers pressed and released around it. Posting only the flags leaves
/// macOS believing the modifier is still down, so later clicks became Command-clicks until a key
/// without modifiers, such as Esc, was pressed.
private func postKey(_ code: CGKeyCode, flags: CGEventFlags, unicode: [UInt16]) throws {
    let modifiers = shortcutModifiers.filter { flags.contains($0.0) }
    var held: CGEventFlags = []
    for (flag, key) in modifiers {
        held.insert(flag)
        try postModifier(CGKeyCode(key), down: true, flags: held)
    }
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
    else { throw DesktopError.inputFailed }
    for event in [down, up] {
        event.flags = held
        if !unicode.isEmpty {
            event.keyboardSetUnicodeString(stringLength: unicode.count, unicodeString: unicode)
        }
        inject(event)
    }
    for (flag, key) in modifiers.reversed() {
        held.remove(flag)
        try postModifier(CGKeyCode(key), down: false, flags: held)
    }
}

private func postModifier(_ code: CGKeyCode, down: Bool, flags: CGEventFlags) throws {
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else {
        throw DesktopError.inputFailed
    }
    event.type = .flagsChanged
    event.flags = flags
    inject(event)
}

/// Modifier keys arrive as their own press and release, so the remote Mac sees them as flag changes.
func isModifierKey(_ code: CGKeyCode) -> Bool {
    [kVK_Shift, kVK_RightShift, kVK_Control, kVK_RightControl, kVK_Option, kVK_RightOption,
     kVK_Command, kVK_RightCommand, kVK_Function].contains(Int(code))
}

/// Switches to the next input source the way this Mac's own shortcut does. The controller's Caps
/// Lock and Control-Space are taken by its own macOS, so they reach this Mac as this request.
func switchInputSource() throws {
    if let (code, flags) = inputSourceShortcut() {
        try postKey(code, flags: flags, unicode: [])
    } else {
        selectNextInputSource()
    }
}

/// The enabled "Select the previous / next input source" shortcut. macOS stores only shortcuts the
/// user changed, so a missing entry means the default Control-Space.
private func inputSourceShortcut() -> (CGKeyCode, CGEventFlags)? {
    let stored = UserDefaults(suiteName: "com.apple.symbolichotkeys")?.dictionary(forKey: "AppleSymbolicHotKeys") ?? [:]
    for id in ["60", "61"] {
        guard let entry = stored[id] as? [String: Any] else {
            return id == "60" ? (CGKeyCode(kVK_Space), .maskControl) : nil
        }
        guard (entry["enabled"] as? Bool) == true,
              let value = entry["value"] as? [String: Any],
              let parameters = value["parameters"] as? [Int], parameters.count == 3,
              parameters[1] >= 0, parameters[1] < 128
        else { continue }
        return (CGKeyCode(parameters[1]), CGEventFlags(rawValue: UInt64(parameters[2])).intersection(
            [.maskCommand, .maskControl, .maskAlternate, .maskShift]))
    }
    return nil
}

private func selectNextInputSource() {
    let filter = [
        kTISPropertyInputSourceCategory: kTISCategoryKeyboardInputSource,
        kTISPropertyInputSourceIsSelectCapable: true,
        kTISPropertyInputSourceIsEnabled: true,
    ] as CFDictionary
    guard let sources = TISCreateInputSourceList(filter, false)?.takeRetainedValue() as? [TISInputSource],
          sources.count > 1, let current = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else { return }
    func id(_ source: TISInputSource) -> String? {
        guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return nil }
        return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
    }
    let index = sources.firstIndex { id($0) == id(current) } ?? -1
    TISSelectInputSource(sources[(index + 1) % sources.count])
}

func postRemoteText(_ text: String) throws {
    for character in text.unicodeScalars {
        try postKey(0, flags: [], unicode: Array(String(character).utf16))
    }
}

enum RemoteMouseButton: String, Codable {
    case left, right, middle, back, forward

    var number: UInt32 {
        switch self {
        case .left: 0
        case .right: 1
        case .middle: 2
        case .back: 3
        case .forward: 4
        }
    }

    var events: (down: CGEventType, dragged: CGEventType, up: CGEventType) {
        switch self {
        case .left: (.leftMouseDown, .leftMouseDragged, .leftMouseUp)
        case .right: (.rightMouseDown, .rightMouseDragged, .rightMouseUp)
        default: (.otherMouseDown, .otherMouseDragged, .otherMouseUp)
        }
    }
}

struct RemotePointerEvent: Codable {
    enum Kind: String, Codable {
        case move, down, up, scroll, release
        case keyDown = "key_down"
        case keyUp = "key_up"
    }

    let kind: Kind
    var x: Int? = nil
    var y: Int? = nil
    var button: RemoteMouseButton? = nil
    var clicks: Int? = nil
    var dx: Int? = nil
    var dy: Int? = nil
    var code: UInt16? = nil
    var flags: UInt8? = nil
    var `repeat`: Bool? = nil
}

// The wire format uses Mac virtual key positions. A Windows sender encodes Win as Command and Alt as Option.
func windowsShortcut(code: CGKeyCode, flags: UInt8) -> (CGKeyCode, UInt8) {
    let key: Int = switch Int(code) {
    case kVK_Control: kVK_Command
    case kVK_RightControl: kVK_RightCommand
    case kVK_Command: kVK_Control
    case kVK_RightCommand: kVK_RightControl
    default: Int(code)
    }
    let control = UInt8(CGEventFlags.maskControl.rawValue >> 16)
    let command = UInt8(CGEventFlags.maskCommand.rawValue >> 16)
    var mapped = flags & ~(control | command)
    if flags & control != 0 { mapped |= command }
    if flags & command != 0 { mapped |= control }
    return (CGKeyCode(key), mapped)
}

@MainActor
final class DesktopInput {
    private var held: [RemoteMouseButton: Int] = [:]
    private var heldKeys: [CGKeyCode: CGKeyCode] = [:]
    /// The controller's modifiers, given to every mouse event so a click never inherits a stale one.
    private var modifiers: CGEventFlags = []
    private var position: CGPoint?

    func apply(_ events: [RemotePointerEvent], display: CGDirectDisplayID, mapWindowsModifiers: Bool = false) throws {
        guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
        guard CGDisplayIsActive(display) != 0 else { throw ScreenCaptureError.displayUnavailable }
        let bounds = CGDisplayBounds(display)
        for event in events {
            if event.kind == .release {
                release()
                continue
            }
            if event.kind == .keyDown || event.kind == .keyUp {
                guard let code = event.code, let flags = event.flags else { throw DesktopError.invalidCommand }
                let down = event.kind == .keyDown
                if Int(code) == kVK_CapsLock {
                    if down { try switchInputSource() }
                    continue
                }
                let mapped = mapWindowsModifiers ? windowsShortcut(code: code, flags: flags) : (code, flags)
                // Release the key actually posted even if the host changed its settings while it was held.
                let postedCode = heldKeys[code] ?? mapped.0
                if !down && heldKeys[code] == nil { continue }
                modifiers = CGEventFlags(rawValue: UInt64(mapped.1) << 16)
                    .intersection([.maskCommand, .maskControl, .maskAlternate, .maskShift])
                try postKeyboardEvent(postedCode, down: down, flags: mapped.1, repeating: event.repeat == true)
                heldKeys[code] = down ? postedCode : nil
                continue
            }
            guard let x = event.x, let y = event.y else { throw DesktopError.invalidCommand }
            let point = CGPoint(
                x: bounds.minX + bounds.width * CGFloat(x) / 10_000,
                y: bounds.minY + bounds.height * CGFloat(y) / 10_000
            )
            switch event.kind {
            case .move:
                if held.isEmpty {
                    try post(.mouseMoved, button: .left, point: point)
                } else {
                    for (button, clicks) in held {
                        try post(button.events.dragged, button: button, point: point, clicks: clicks)
                    }
                }
            case .down:
                guard let button = event.button, let clicks = event.clicks else { throw DesktopError.invalidCommand }
                try post(button.events.down, button: button, point: point, clicks: clicks)
                held[button] = clicks
            case .up:
                guard let button = event.button else { throw DesktopError.invalidCommand }
                if let clicks = held.removeValue(forKey: button) {
                    try post(button.events.up, button: button, point: point, clicks: clicks)
                }
            case .scroll:
                guard let dx = event.dx, let dy = event.dy,
                      let scroll = CGEvent(
                        scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                        wheel1: Int32(dy), wheel2: Int32(dx), wheel3: 0
                      )
                else { throw DesktopError.inputFailed }
                scroll.location = point
                scroll.flags = modifiers
                inject(scroll)
            case .release, .keyDown, .keyUp:
                break
            }
            position = point
        }
    }

    func release() {
        if let position {
            for (button, clicks) in held {
                try? post(button.events.up, button: button, point: position, clicks: clicks)
            }
        }
        held.removeAll()
        // Letters first, then the modifiers that were held around them.
        for code in heldKeys.values.sorted(by: { !isModifierKey($0) && isModifierKey($1) }) {
            try? postKeyboardEvent(code, down: false, flags: 0, repeating: false)
        }
        heldKeys.removeAll()
        modifiers = []
    }

    private func postKeyboardEvent(_ code: CGKeyCode, down: Bool, flags: UInt8, repeating: Bool) throws {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else {
            throw DesktopError.inputFailed
        }
        if isModifierKey(code) { event.type = .flagsChanged }
        event.flags = CGEventFlags(rawValue: UInt64(flags) << 16)
        event.setIntegerValueField(.keyboardEventAutorepeat, value: repeating ? 1 : 0)
        inject(event)
    }

    private func post(_ type: CGEventType, button: RemoteMouseButton, point: CGPoint, clicks: Int = 0) throws {
        guard let mouseButton = CGMouseButton(rawValue: button.number),
              let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: mouseButton)
        else { throw DesktopError.inputFailed }
        event.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button.number))
        event.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
        event.flags = modifiers
        if let position {
            event.setIntegerValueField(.mouseEventDeltaX, value: Int64(point.x - position.x))
            event.setIntegerValueField(.mouseEventDeltaY, value: Int64(point.y - position.y))
        }
        inject(event)
    }
}
