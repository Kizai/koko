import AppKit
import SwiftUI

@main
struct KokoApp: App {
    @NSApplicationDelegateAdaptor(KokoAppDelegate.self) private var delegate

    var body: some Scene {
        Window("Koko", id: "connection") {
            ConnectionView(connection: delegate.connection)
        }
        .windowResizability(.contentSize)
    }
}

@MainActor
private final class KokoAppDelegate: NSObject, NSApplicationDelegate {
    let connection = LocalConnection()

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        connection.stopForQuit()
    }
}

private struct ConnectionView: View {
    @ObservedObject var connection: LocalConnection
    @AppStorage("deviceRole") private var role = DeviceRole.controller

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(spacing: 12) {
                Image(systemName: "link.circle.fill")
                    .font(.system(size: 36))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("Koko").font(.title2.bold())
                        if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String {
                            Text("v\(version)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Text("设备配对").foregroundStyle(.secondary)
                }
                Spacer()
            }

            Picker("设备角色", selection: $role) {
                ForEach(DeviceRole.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .disabled(connection.isHosting || connection.isChecking)
            .accessibilityIdentifier("device-role")

            if role == .controller {
                controllerForm
            } else {
                hostForm
            }
        }
        .padding(24)
        .frame(width: 520)
        .task(id: role) {
            await connection.prepareIdentity(for: role)
            if role == .host { await connection.refreshScreenPermission() }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            if role == .host { Task { await connection.refreshScreenPermission() } }
        }
    }

    private var controllerForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("输入被控端的连接码，或粘贴直连邀请。")
            TextField("连接码或直连邀请", text: $connection.inviteInput)
                .textFieldStyle(.roundedBorder)
                .disabled(connection.isChecking)
                .accessibilityIdentifier("remote-code")

            HStack {
                Button("请求配对") { Task { await connection.verify(code: connection.inviteInput) } }
                    .buttonStyle(.borderedProminent)
                    .disabled(connection.controllerID.isEmpty || connection.inviteInput.isEmpty || connection.isChecking)
                if connection.isChecking {
                    Button("取消") { connection.cancelCheck() }
                }
            }
            notice(connection.controllerNotice)
        }
    }

    private var hostForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("启动监听后，把临时连接码或直连邀请交给控制端。")
            HStack {
                Text("监听端口")
                TextField("4444", text: $connection.listenPort)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 88)
                    .disabled(connection.isHosting)
                    .accessibilityIdentifier("listen-port")
                Spacer()
                if connection.isHosting {
                    Button("停止监听") { connection.stopHosting() }
                } else {
                    Button("启动监听") {
                        Task { await connection.startHosting() }
                    }
                        .buttonStyle(.borderedProminent)
                        .disabled(connection.hostID.isEmpty || connection.listenPort.isEmpty)
                }
            }
            copyRow(connection.connectionCode, placeholder: "启动后生成连接码", button: "复制连接码")
            if !connection.directInvite.isEmpty {
                HStack {
                    Text("组网内找不到设备时，可用直连邀请。")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("复制直连邀请") { copy(connection.directInvite) }
                }
            }
            if let request = connection.pendingPair {
                HStack {
                    Text("设备 \(request.fingerprint) 请求配对")
                    Spacer()
                    Button("拒绝") { connection.respondToPairing(request, allow: false) }
                    Button("允许") { connection.respondToPairing(request, allow: true) }
                        .buttonStyle(.borderedProminent)
                }
            }
            notice(connection.hostNotice)
            Divider()
            screenPermissionView
        }
    }

    private var screenPermissionView: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("屏幕录制").font(.headline)
                Spacer()
                if connection.isCheckingScreenPermission {
                    Text("正在检查…").foregroundStyle(.secondary)
                } else {
                    switch connection.screenPermission {
                    case .unknown: Text("待检查").foregroundStyle(.secondary)
                    case .required: Text("未授权").foregroundStyle(.secondary)
                    case .granted: Text("已授权").foregroundStyle(.green)
                    case .failed: Text("检查失败").foregroundStyle(.red)
                    }
                }
            }
            if connection.screenPermission != .granted {
                Text("如未弹出授权提示，可在「隐私与安全性 → 录屏与系统录音」中允许 Koko。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                HStack {
                    Button("申请录屏权限") { Task { await connection.refreshScreenPermission(request: true) } }
                    Button("打开系统设置") {
                        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.systempreferences") {
                            NSWorkspace.shared.open(url)
                        }
                    }
                    Button("重新检查") { Task { await connection.refreshScreenPermission() } }
                }
                .disabled(connection.isCheckingScreenPermission)
            }
        }
    }

    private func copyRow(_ value: String, placeholder: String, button: String) -> some View {
        HStack {
            Text(value.isEmpty ? placeholder : value)
                .font(.system(.callout, design: .monospaced))
                .foregroundStyle(value.isEmpty ? .secondary : .primary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(button) {
                copy(value)
            }
            .disabled(value.isEmpty)
        }
    }

    private func copy(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }

    @ViewBuilder
    private func notice(_ value: ConnectionNotice?) -> some View {
        if let value {
            Text(value.message)
                .font(.callout)
                .foregroundStyle(value.isError ? Color.red : Color.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
