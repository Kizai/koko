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
    @AppStorage("allowedControllerID") private var allowedPeer = ""
    @AppStorage("lastConnectionCode") private var remoteCode = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(spacing: 12) {
                Image(systemName: "link.circle.fill")
                    .font(.system(size: 36))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Koko").font(.title2.bold())
                    Text("局域网连接测试").foregroundStyle(.secondary)
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
        .task(id: role) { await connection.prepareIdentity(for: role) }
    }

    private var controllerForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("1. 复制控制端 ID，交给被控端。")
            copyRow(connection.controllerID, placeholder: "正在读取设备 ID…", button: "复制 ID")

            Text("2. 粘贴被控端生成的连接码。")
            TextField("粘贴连接码", text: $remoteCode)
                .textFieldStyle(.roundedBorder)
                .disabled(connection.isChecking)
                .accessibilityIdentifier("remote-code")

            HStack {
                Button("验证连接") { Task { await connection.verify(code: remoteCode) } }
                    .buttonStyle(.borderedProminent)
                    .disabled(connection.controllerID.isEmpty || remoteCode.isEmpty || connection.isChecking)
                if connection.isChecking {
                    Button("取消") { connection.cancelCheck() }
                }
            }
            notice(connection.controllerNotice)
        }
    }

    private var hostForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("1. 粘贴要授权的控制端 ID。")
            TextField("粘贴控制端 ID", text: $allowedPeer)
                .textFieldStyle(.roundedBorder)
                .disabled(connection.isHosting)
                .accessibilityIdentifier("allowed-peer")

            Text("2. 启动监听，将连接码交给控制端。")
            HStack {
                TextField("局域网 IP:端口", text: $connection.listenAddress)
                    .textFieldStyle(.roundedBorder)
                    .disabled(connection.isHosting)
                    .accessibilityIdentifier("listen-address")
                if connection.isHosting {
                    Button("停止监听") { connection.stopHosting() }
                } else {
                    Button("启动监听") {
                        Task { await connection.startHosting(address: connection.listenAddress, allowing: allowedPeer) }
                    }
                        .buttonStyle(.borderedProminent)
                        .disabled(connection.hostID.isEmpty || allowedPeer.isEmpty || connection.listenAddress.isEmpty)
                }
            }
            copyRow(connection.connectionCode, placeholder: "启动后生成连接码", button: "复制连接码")
            notice(connection.hostNotice)
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
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(value, forType: .string)
            }
            .disabled(value.isEmpty)
        }
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
