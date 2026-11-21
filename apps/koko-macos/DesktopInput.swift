@preconcurrency import ApplicationServices
import Carbon
import Foundation

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
    down.post(tap: .cghidEventTap)
    up.post(tap: .cghidEventTap)
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

private func postKey(_ code: CGKeyCode, flags: CGEventFlags, unicode: [UInt16]) throws {
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
    else { throw DesktopError.inputFailed }
    for event in [down, up] {
        event.flags = flags
        if !unicode.isEmpty {
            event.keyboardSetUnicodeString(stringLength: unicode.count, unicodeString: unicode)
        }
        event.post(tap: .cghidEventTap)
    }
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
    enum Kind: String, Codable { case move, down, up, scroll, release }

    let kind: Kind
    var x: Int? = nil
    var y: Int? = nil
    var button: RemoteMouseButton? = nil
    var clicks: Int? = nil
    var dx: Int? = nil
    var dy: Int? = nil
}

@MainActor
final class DesktopPointer {
    private var held: [RemoteMouseButton: Int] = [:]
    private var position: CGPoint?

    func apply(_ events: [RemotePointerEvent], display: CGDirectDisplayID) throws {
        guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
        guard CGDisplayIsActive(display) != 0 else { throw ScreenCaptureError.displayUnavailable }
        let bounds = CGDisplayBounds(display)
        for event in events {
            if event.kind == .release {
                release()
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
                scroll.post(tap: .cghidEventTap)
            case .release:
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
    }

    private func post(_ type: CGEventType, button: RemoteMouseButton, point: CGPoint, clicks: Int = 0) throws {
        guard let mouseButton = CGMouseButton(rawValue: button.number),
              let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: mouseButton)
        else { throw DesktopError.inputFailed }
        event.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button.number))
        event.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
        if let position {
            event.setIntegerValueField(.mouseEventDeltaX, value: Int64(point.x - position.x))
            event.setIntegerValueField(.mouseEventDeltaY, value: Int64(point.y - position.y))
        }
        event.post(tap: .cghidEventTap)
    }
}
