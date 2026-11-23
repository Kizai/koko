import AppKit
@preconcurrency import ApplicationServices

/// A rule an agent's input matched. `rule` identifies the kind of action, so a person can allow
/// it for the rest of the agent's session; `summary` tells them what the agent is about to do.
struct PolicyHit: Equatable {
    let rule: String
    let summary: String
}

/// Which agent inputs need the host owner's approval. Read from `~/.koko/agent-policy.json`;
/// missing keys keep their defaults, and an empty list turns that rule off.
struct AgentPolicy: Decodable {
    var shortcuts = true
    /// Apps where typed text or Return runs commands.
    var commandApps = [
        "com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.mitchellh.ghostty",
        "net.kovidgoyal.kitty", "org.alacritty", "com.github.wez.wezterm",
    ]
    /// Apps where any click or typing can change the system or reveal secrets.
    var protectedApps = [
        "com.apple.systempreferences", "com.apple.keychainaccess", "com.apple.DiskUtility",
        "com.apple.ActivityMonitor",
    ]
    /// Words in a clicked control's name that mark an irreversible or costly action.
    var keywords = [
        "删除", "移除", "清空", "抹掉", "卸载", "付款", "支付", "购买", "转账", "发送", "提交订单", "永久",
        "delete", "remove", "erase", "empty trash", "uninstall", "pay", "purchase", "buy", "transfer", "send",
        "place order",
    ]

    enum CodingKeys: String, CodingKey {
        case shortcuts, keywords
        case commandApps = "command_apps"
        case protectedApps = "protected_apps"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let defaults = AgentPolicy()
        shortcuts = try values.decodeIfPresent(Bool.self, forKey: .shortcuts) ?? defaults.shortcuts
        commandApps = try values.decodeIfPresent([String].self, forKey: .commandApps) ?? defaults.commandApps
        protectedApps = try values.decodeIfPresent([String].self, forKey: .protectedApps) ?? defaults.protectedApps
        keywords = try values.decodeIfPresent([String].self, forKey: .keywords) ?? defaults.keywords
    }

    /// Loads the policy file. A missing file means defaults; a broken file is reported to the caller.
    static func load(from url: URL) throws -> AgentPolicy {
        guard FileManager.default.fileExists(atPath: url.path) else { return AgentPolicy() }
        return try JSONDecoder().decode(AgentPolicy.self, from: Data(contentsOf: url))
    }

    /// Checks one agent input against the rules, in order: shortcuts, protected apps, command apps,
    /// then the names of clicked controls.
    @MainActor
    func check(_ action: RemoteAction, frontmost: NSRunningApplication?) -> PolicyHit? {
        let events = action.events ?? []
        let bundle = frontmost?.bundleIdentifier ?? ""
        let app = frontmost?.localizedName ?? "当前应用"
        let presses = events.filter { $0.kind == .down || $0.kind == .keyDown }

        if shortcuts {
            for event in events where event.kind == .keyDown {
                if let code = event.code, let name = shortcutName(code: code, flags: event.flags ?? 0) {
                    return PolicyHit(rule: "shortcut:\(name)", summary: "按 \(name)（当前应用：\(app)）")
                }
            }
        }
        if protectedApps.contains(bundle) && (action.action == "text" || !presses.isEmpty) {
            return PolicyHit(rule: "app:\(bundle)", summary: "在「\(app)」中点击或输入")
        }
        if commandApps.contains(bundle) {
            if action.action == "text" {
                return PolicyHit(rule: "command:\(bundle)", summary: "在「\(app)」中输入：\(preview(action.text ?? ""))")
            }
            if events.contains(where: { $0.kind == .keyDown && [36, 76].contains($0.code ?? 0) }) {
                return PolicyHit(rule: "command:\(bundle)", summary: "在「\(app)」中按回车执行命令")
            }
        }
        if !keywords.isEmpty, let display = action.display {
            let bounds = CGDisplayBounds(display)
            for event in events where event.kind == .down {
                guard let x = event.x, let y = event.y else { continue }
                let point = CGPoint(x: bounds.minX + bounds.width * CGFloat(x) / 10_000,
                                    y: bounds.minY + bounds.height * CGFloat(y) / 10_000)
                let labels = accessibilityLabels(at: point)
                if let word = matchingKeyword(in: labels) {
                    return PolicyHit(rule: "click:\(word)", summary: "点击「\(labels.first ?? word)」")
                }
            }
        }
        return nil
    }

    func matchingKeyword(in labels: [String]) -> String? {
        for label in labels {
            let lower = label.lowercased()
            if let word = keywords.first(where: { lower.contains($0.lowercased()) }) { return word }
        }
        return nil
    }
}

/// Names of shortcuts that quit apps, log out, lock the screen or delete files.
/// `flags` is the pointer protocol's Core Graphics modifier byte; Caps Lock and Fn are ignored.
func shortcutName(code: UInt16, flags: UInt8) -> String? {
    let shift: UInt8 = 0x02, control: UInt8 = 0x04, option: UInt8 = 0x08, command: UInt8 = 0x10
    let modifiers = flags & (shift | control | option | command)
    switch (code, modifiers) {
    case (12, command): return "⌘Q"
    case (12, control | command): return "⌃⌘Q"
    case (12, shift | command): return "⇧⌘Q"
    case (12, option | shift | command): return "⌥⇧⌘Q"
    case (51, command): return "⌘⌫"
    case (51, option | command): return "⌥⌘⌫"
    case (51, shift | command): return "⇧⌘⌫"
    case (51, option | shift | command): return "⌥⇧⌘⌫"
    case (53, option | command): return "⌥⌘⎋"
    default: return nil
    }
}

private func preview(_ text: String) -> String {
    let line = text.replacingOccurrences(of: "\n", with: "⏎")
    return line.count > 80 ? String(line.prefix(80)) + "…" : line
}

/// The names of the control under a screen point and of its nearest few containers.
private func accessibilityLabels(at point: CGPoint) -> [String] {
    let system = AXUIElementCreateSystemWide()
    AXUIElementSetMessagingTimeout(system, 0.5)
    var hit: AXUIElement?
    guard AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &hit) == .success,
          var element = hit else { return [] }
    var labels: [String] = []
    for _ in 0..<4 {
        var role: CFTypeRef?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &role)
        // A window title describes the whole window, not the control being clicked.
        if role as? String == "AXWindow" { break }
        for name in [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute] {
            var value: CFTypeRef?
            if AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success,
               let text = value as? String, !text.isEmpty, text.count <= 200 {
                labels.append(text)
            }
        }
        var parent: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXParentAttribute as CFString, &parent) == .success,
              let next = parent, CFGetTypeID(next) == AXUIElementGetTypeID() else { break }
        element = next as! AXUIElement
    }
    return labels
}
