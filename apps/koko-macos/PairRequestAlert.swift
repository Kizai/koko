import AppKit
import SwiftUI
@preconcurrency import UserNotifications

/// Posts macOS system notifications for incoming remote requests and forwards action clicks.
@MainActor
final class PairNotifications: NSObject, UNUserNotificationCenterDelegate {
    var onDecision: (String, Bool, Bool) -> Void = { _, _, _ in }
    var onOpenMain: () -> Void = {}

    private var isBundledApp: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
    }

    func configure() {
        guard isBundledApp else { return }
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let decline = UNNotificationAction(identifier: "DECLINE", title: "拒绝", options: [.destructive])
        let allowOnce = UNNotificationAction(identifier: "ALLOW_ONCE", title: "仅允许这次", options: [])
        let trust = UNNotificationAction(identifier: "TRUST", title: "信任并连接", options: [.foreground])
        let category = UNNotificationCategory(
            identifier: "PAIR_REQUEST",
            actions: [trust, allowOnce, decline],
            intentIdentifiers: [],
            options: []
        )
        center.setNotificationCategories([category])
    }

    func authorizationStatus() async -> ScreenPermission {
        guard isBundledApp else { return .unknown }
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional:
            return .granted
        case .denied, .notDetermined:
            return .required
        @unknown default:
            return .unknown
        }
    }

    func requestAuthorization() async -> ScreenPermission {
        guard isBundledApp else { return .unknown }
        configure()
        let center = UNUserNotificationCenter.current()
        let current = await center.notificationSettings().authorizationStatus
        if current == .notDetermined {
            _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
        }
        return await authorizationStatus()
    }

    func post(request: PendingPair) {
        guard isBundledApp else { return }
        configure()
        let content = UNMutableNotificationContent()
        content.title = request.isAgent ? "Agent「\(request.name)」请求连接" : "收到远程协助请求"
        let sender = request.name.isEmpty ? request.fingerprint : "\(request.name)（\(request.fingerprint)）"
        content.body = request.isAgent
            ? "请求\(agentAccessTitle(request.scope))"
            : "设备 \(sender) 请求查看和控制此 Mac"
        content.sound = .default
        content.categoryIdentifier = "PAIR_REQUEST"
        content.userInfo = ["number": request.number]

        let notification = UNNotificationRequest(
            identifier: "koko-pair-\(request.number)",
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(notification)
    }

    func clear(number: String) {
        guard isBundledApp else { return }
        let id = "koko-pair-\(number)"
        let center = UNUserNotificationCenter.current()
        center.removePendingNotificationRequests(withIdentifiers: [id])
        center.removeDeliveredNotifications(withIdentifiers: [id])
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound])
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let number = response.notification.request.content.userInfo["number"] as? String
        let action = response.actionIdentifier
        Task { @MainActor in
            if let number {
                switch action {
                case "DECLINE":
                    self.onDecision(number, false, false)
                case "ALLOW_ONCE":
                    self.onDecision(number, true, false)
                case "TRUST":
                    self.onDecision(number, true, true)
                default:
                    self.onOpenMain()
                }
            } else {
                self.onOpenMain()
            }
        }
        completionHandler()
    }
}

/// Top-right floating prompt when a pairing request arrives, visible even while KoKo is hidden
/// in the menu bar or the person is working in another app.
@MainActor
final class PairRequestPanel {
    var onDecision: (PendingPair, Bool, Bool) -> Void = { _, _, _ in }
    private var panel: NSPanel?

    func update(_ request: PendingPair?) {
        guard let request else {
            panel?.orderOut(nil)
            return
        }
        let panel = self.panel ?? makePanel()
        self.panel = panel
        let content = NSHostingView(rootView: PairRequestCard(request: request) { [weak self] allow, remember in
            self?.onDecision(request, allow, remember)
        })
        panel.contentView = content
        panel.setContentSize(content.fittingSize)
        if let frame = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame {
            panel.setFrameOrigin(CGPoint(
                x: frame.maxX - panel.frame.width - 16,
                y: frame.maxY - panel.frame.height - 16
            ))
        }
        panel.orderFrontRegardless()
        NSApp.requestUserAttention(.informationalRequest)
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(
            contentRect: .zero,
            styleMask: [.nonactivatingPanel, .borderless],
            backing: .buffered,
            defer: true
        )
        panel.level = .floating
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        return panel
    }
}

private struct PairRequestCard: View {
    let request: PendingPair
    let decide: (Bool, Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: request.isAgent ? "sparkles" : "display.and.arrow.down")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(Color.accentColor)
                Text(request.isAgent ? "Agent「\(request.name)」请求连接" : "收到远程协助请求")
                    .font(.headline)
            }
            let sender = request.name.isEmpty ? request.fingerprint : "\(request.name)（\(request.fingerprint)）"
            Text(request.isAgent
                 ? "请求\(agentAccessTitle(request.scope))。信任后，本机共享开启时可直接连接。"
                 : "设备 \(sender) 请求查看和控制此 Mac。")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Button("拒绝") { decide(false, false) }
                Spacer()
                Button("仅允许这次") { decide(true, false) }
                Button("信任并连接") { decide(true, true) }
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(14)
        .frame(width: 340, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
        .padding(6)
    }
}
