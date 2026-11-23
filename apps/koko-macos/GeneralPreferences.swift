import AppKit
import IOKit.pwr_mgt
import ServiceManagement

/// Opens KoKo when the person logs in, through macOS Login Items.
enum LaunchAtLogin {
    static var isEnabled: Bool { SMAppService.mainApp.status == .enabled }
    /// macOS registered KoKo but waits for the person to allow it in System Settings.
    static var needsApproval: Bool { SMAppService.mainApp.status == .requiresApproval }

    static func set(_ enabled: Bool) throws {
        if enabled {
            try SMAppService.mainApp.register()
        } else {
            try SMAppService.mainApp.unregister()
        }
    }
}

/// Keeps the Mac from sleeping when idle, so it stays reachable. The screen may still turn off,
/// and closing the lid still sleeps the Mac.
@MainActor
final class KeepAwake {
    enum Mode: String, CaseIterable, Identifiable {
        case never, sharing, always
        var id: Self { self }
        var title: String {
            switch self {
            case .never: "永不"
            case .sharing: "开启远程协助时"
            case .always: "始终"
            }
        }
    }

    static let key = "keepAwake"
    static let shared = KeepAwake()
    private var assertion = IOPMAssertionID(0)

    static var mode: Mode {
        Mode(rawValue: UserDefaults.standard.string(forKey: key) ?? "") ?? .never
    }

    func update(hosting: Bool) {
        let wanted = Self.mode == .always || (Self.mode == .sharing && hosting)
        if wanted, assertion == 0 {
            IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleSystemSleep as CFString,
                                        IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                        "KoKo 保持唤醒以便远程访问" as CFString, &assertion)
        } else if !wanted, assertion != 0 {
            IOPMAssertionRelease(assertion)
            assertion = 0
        }
    }
}

/// What closing the last KoKo window does.
enum CloseAction: String, CaseIterable, Identifiable {
    case menuBar, quit
    var id: Self { self }
    static let key = "closeAction"

    var title: String {
        switch self {
        case .menuBar: "隐藏到菜单栏"
        case .quit: "退出 KoKo"
        }
    }

    static var current: CloseAction {
        CloseAction(rawValue: UserDefaults.standard.string(forKey: key) ?? "") ?? .menuBar
    }

    /// Hides the Dock icon once no KoKo window is open, and brings it back with the next window.
    @MainActor
    static func observeWindows() {
        let center = NotificationCenter.default
        center.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) { _ in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard current == .menuBar, !hasOpenWindow else { return }
                    NSApp.setActivationPolicy(.accessory)
                }
            }
        }
        center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                guard let window = NSApp.keyWindow, window.styleMask.contains(.titled),
                      NSApp.activationPolicy() != .regular else { return }
                NSApp.setActivationPolicy(.regular)
            }
        }
    }

    @MainActor
    private static var hasOpenWindow: Bool {
        NSApp.windows.contains { $0.isVisible && $0.styleMask.contains(.titled) }
    }
}
