import AppKit
import Carbon
import SwiftUI

/// A bar at the top of the host's main screen whenever an agent holds control or was stopped.
/// It stays out of the agent's own screenshots so it cannot cover or be clicked by the agent.
@MainActor
final class AgentIndicator {
    var onStop: () -> Void = {}
    var onResume: () -> Void = {}
    /// Answers a request for help with resolved, taken_over or declined.
    var onAnswer: (String) -> Void = { _ in }
    /// Decides a risky input: once, session or deny.
    var onDecide: (String) -> Void = { _ in }
    private var panel: NSPanel?
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?

    /// The window to leave out of agent screenshots while the bar is visible.
    var windowID: CGWindowID? {
        guard let panel, panel.isVisible else { return nil }
        return CGWindowID(panel.windowNumber)
    }

    func update(control: String, agentName: String?, help: String?) {
        guard let agentName, ["active", "paused", "awaiting_human", "awaiting_approval"].contains(control) else {
            hide()
            return
        }
        let panel = self.panel ?? makePanel()
        self.panel = panel
        let content = NSHostingView(rootView: AgentBar(
            name: agentName, control: control, help: help, stop: { [weak self] in self?.onStop() },
            resume: { [weak self] in self?.onResume() }, answer: { [weak self] in self?.onAnswer($0) },
            decide: { [weak self] in self?.onDecide($0) }
        ))
        panel.contentView = content
        panel.setContentSize(content.fittingSize)
        let screen = NSScreen.screens.first ?? NSScreen.main
        if let frame = screen?.visibleFrame {
            panel.setFrameOrigin(CGPoint(x: frame.midX - panel.frame.width / 2,
                                         y: frame.maxY - panel.frame.height - 8))
        }
        panel.orderFrontRegardless()
        registerHotKey()
    }

    func hide() {
        panel?.orderOut(nil)
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = nil
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(contentRect: .zero, styleMask: [.nonactivatingPanel, .borderless],
                            backing: .buffered, defer: true)
        panel.level = .statusBar
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        return panel
    }

    /// Control-Option-Command-period stops the agent from anywhere, without extra permissions.
    private func registerHotKey() {
        guard hotKey == nil else { return }
        let target = GetApplicationEventTarget()
        if handler == nil {
            var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            let context = Unmanaged.passUnretained(self).toOpaque()
            InstallEventHandler(target, { _, _, context in
                guard let context else { return OSStatus(eventNotHandledErr) }
                // Carbon delivers hot keys on the main run loop.
                MainActor.assumeIsolated {
                    Unmanaged<AgentIndicator>.fromOpaque(context).takeUnretainedValue().onStop()
                }
                return noErr
            }, 1, &type, context, &handler)
        }
        RegisterEventHotKey(UInt32(kVK_ANSI_Period), UInt32(cmdKey | optionKey | controlKey),
                            EventHotKeyID(signature: 0x4B4F_4B4F, id: 1), target, 0, &hotKey)
    }
}

private struct AgentBar: View {
    let name: String
    let control: String
    let help: String?
    let stop: () -> Void
    let resume: () -> Void
    let answer: (String) -> Void
    let decide: (String) -> Void

    private var tint: Color {
        switch control {
        case "paused": Color.secondary.opacity(0.4)
        case "awaiting_human": Color.blue
        case "awaiting_approval": Color.red
        default: Color.orange
        }
    }

    var body: some View {
        HStack(spacing: 10) {
            switch control {
            case "paused":
                Image(systemName: "pause.circle.fill").foregroundStyle(.secondary)
                Text("Agent「\(name)」已暂停").font(.callout.weight(.medium))
                Button("让 Agent 继续", action: resume)
            case "awaiting_approval":
                Image(systemName: "exclamationmark.shield.fill").foregroundStyle(.red)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Agent「\(name)」想要执行高危操作").font(.callout.weight(.medium))
                    if let help {
                        Text(help).font(.caption).foregroundStyle(.secondary)
                            .lineLimit(3).frame(maxWidth: 420, alignment: .leading)
                    }
                }
                Button("允许一次") { decide("once") }
                Button("本次会话都允许") { decide("session") }
                    .help("这个 Agent 断开前，同类操作不再询问")
                Button("拒绝", role: .destructive) { decide("deny") }
            case "awaiting_human":
                Image(systemName: "hand.raised.fill").foregroundStyle(.blue)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Agent「\(name)」请你协助").font(.callout.weight(.medium))
                    if let help {
                        Text(help).font(.caption).foregroundStyle(.secondary)
                            .lineLimit(3).frame(maxWidth: 420, alignment: .leading)
                    }
                }
                Button("已完成") { answer("resolved") }
                    .help("你已处理完，Agent 继续操作")
                Button("我来接管") { answer("taken_over") }
                Button("拒绝", role: .destructive) { answer("declined") }
            default:
                Image(systemName: "sparkles").foregroundStyle(.orange)
                Text("Agent「\(name)」正在操作这台 Mac").font(.callout.weight(.medium))
                Button("停止", role: .destructive, action: stop)
                    .help("也可按 ⌃⌥⌘.，或直接动一下这台 Mac 的鼠标键盘")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).strokeBorder(tint, lineWidth: 1.5))
        .padding(6)
    }
}
