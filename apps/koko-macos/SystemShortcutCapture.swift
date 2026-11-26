@preconcurrency import AppKit
@preconcurrency import ApplicationServices

/// Takes this Mac's keyboard while the remote desktop has keyboard focus, so system shortcuts
/// such as ⌘⇧4, ⌃Space, ⌃←/→ and ⌘Tab reach the remote Mac instead of acting here.
///
/// macOS matches system shortcuts before an app sees the key, so a view's `keyDown` never gets
/// them. A HID-level event tap sees the key first; while active it hands every key event to
/// `handler` and drops the ones the handler forwarded. Creating the tap needs this Mac's
/// Accessibility permission; without it `start()` returns false and only app-level keys forward.
@MainActor
final class SystemShortcutCapture {
    /// The setting that turns this on; on by default.
    nonisolated static let settingKey = "forwardSystemShortcuts"
    /// Returns true when the event was sent to the remote Mac and must not act locally.
    var handler: ((NSEvent) -> Bool)?
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?

    var isActive: Bool { tap != nil }

    @discardableResult
    func start() -> Bool {
        if tap != nil { return true }
        let mask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue)
            | (1 << CGEventType.flagsChanged.rawValue)
        guard let tap = CGEvent.tapCreate(
            tap: .cghidEventTap, place: .headInsertEventTap, options: .defaultTap,
            eventsOfInterest: CGEventMask(mask), callback: systemShortcutTapCallback,
            userInfo: Unmanaged.passUnretained(self).toOpaque()
        ) else { return false }
        let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        self.tap = tap
        self.source = source
        return true
    }

    func stop() {
        guard let tap else { return }
        CGEvent.tapEnable(tap: tap, enable: false)
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        CFMachPortInvalidate(tap)
        self.tap = nil
        source = nil
    }

    fileprivate func handle(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            return Unmanaged.passUnretained(event)
        }
        // Keys this Mac posts while it is itself being controlled are not the person's typing.
        if event.getIntegerValueField(.eventSourceUnixProcessID) == Int64(getpid()) {
            return Unmanaged.passUnretained(event)
        }
        guard let key = NSEvent(cgEvent: event), handler?(key) == true else {
            return Unmanaged.passUnretained(event)
        }
        return nil
    }
}

private func systemShortcutTapCallback(
    proxy: CGEventTapProxy, type: CGEventType, event: CGEvent, userInfo: UnsafeMutableRawPointer?
) -> Unmanaged<CGEvent>? {
    guard let pointer = userInfo.map(UInt.init(bitPattern:)) else { return Unmanaged.passUnretained(event) }
    // The tap's run loop source is on the main run loop, so this runs on the main thread.
    return MainActor.assumeIsolated {
        let capture = Unmanaged<SystemShortcutCapture>.fromOpaque(UnsafeRawPointer(bitPattern: pointer)!)
        return capture.takeUnretainedValue().handle(type, event)
    }
}
