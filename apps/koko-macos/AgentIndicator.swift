import AppKit
import Carbon
import SwiftUI

/// A bar at the top of the host's main screen whenever an agent holds control or was stopped.
/// It stays out of the agent's own screenshots so it cannot cover or be clicked by the agent.
@MainActor
final class AgentIndicator {
    var onStop: () -> Void = {}
    var onResume: () -> Void = {}
    private var panel: NSPanel?
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?

    /// The window to leave out of agent screenshots while the bar is visible.
    var windowID: CGWindowID? {
        guard let panel, panel.isVisible else { return nil }
        return CGWindowID(panel.windowNumber)
    }

    func update(control: String, agentName: String?) {
        guard let agentName, control == "active" || control == "paused" else {
            hide()
            return
        }
        let panel = self.panel ?? makePanel()
        self.panel = panel
        let content = NSHostingView(rootView: AgentBar(
            name: agentName, paused: control == "paused", stop: { [weak self] in self?.onStop() },
            resume: { [weak self] in self?.onResume() }
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
    let paused: Bool
    let stop: () -> Void
    let resume: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: paused ? "pause.circle.fill" : "sparkles")
                .foregroundStyle(paused ? Color.secondary : Color.orange)
            Text(paused ? "Agent「\(name)」已暂停" : "Agent「\(name)」正在操作这台 Mac")
                .font(.callout.weight(.medium))
            if paused {
                Button("让 Agent 继续", action: resume)
            } else {
                Button("停止", role: .destructive, action: stop)
                    .help("也可按 ⌃⌥⌘.")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(paused ? Color.secondary.opacity(0.4) : Color.orange, lineWidth: 1.5))
        .padding(6)
    }
}
