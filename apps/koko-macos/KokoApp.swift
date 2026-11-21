import AppKit
import SwiftUI

@main
struct KokoApp: App {
    @NSApplicationDelegateAdaptor(KokoAppDelegate.self) private var delegate

    var body: some Scene {
        Window("Koko", id: "connection") {
            ConnectionView(connection: delegate.connection)
        }
        .defaultSize(width: 870, height: 640)
        .windowResizability(.contentMinSize)

        Window("Koko 远程控制", id: "remote") {
            RemoteSessionWindow(connection: delegate.connection)
        }
        .defaultSize(width: 1080, height: 700)
        .windowResizability(.contentMinSize)
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
    @Environment(\.openWindow) private var openWindow
    @StateObject private var screenTest = ScreenTest()
    @StateObject private var inputTest = InputTest()

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Text("远程协助")
                        .font(.system(size: 27, weight: .semibold))
                        .padding(.bottom, 3)
                    if connection.session != nil {
                        HStack {
                            Label(connection.remoteHostName.isEmpty ? "已连接远程设备" : connection.remoteHostName,
                                  systemImage: "display.and.arrow.down")
                                .font(.headline)
                            Spacer()
                            Button("打开远控窗口") { openWindow(id: "remote") }
                                .buttonStyle(.borderedProminent)
                            Button("断开") { connection.disconnect() }
                        }
                        .padding(16)
                        .background(Color.accentColor.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
                    }
                    sharingCard
                    controllerCard
                    permissionsCard
                }
                .frame(maxWidth: 720)
                .padding(25)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth: 740, minHeight: 560)
        .task {
            await connection.prepareIdentity(for: .host)
            await connection.prepareIdentity(for: .controller)
            await connection.refreshScreenPermission()
            await connection.refreshInputPermission()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task {
                await connection.refreshScreenPermission()
                await connection.refreshInputPermission()
            }
        }
        .onChange(of: connection.session) { session in
            if session != nil { openWindow(id: "remote") }
        }
        .sheet(isPresented: $connection.showsHostPermissionGuide) {
            hostPermissionGuide
        }
    }

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 9) {
                Image(systemName: "link.circle.fill")
                    .font(.system(size: 25))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityHidden(true)
                Text("Koko").font(.title3.bold())
                Spacer(minLength: 5)
                if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String {
                    Text(version).font(.caption.monospaced()).foregroundStyle(.secondary)
                }
            }
            .padding(.bottom, 12)
            HStack {
                Text("局域网设备").font(.headline)
                Spacer()
                Button {
                    Task { await connection.scanLAN() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .disabled(connection.isScanning || connection.isChecking)
                .accessibilityLabel("搜索局域网")
                .help("搜索局域网")
            }
            if connection.isScanning {
                ProgressView("正在搜索…").controlSize(.small)
            } else if connection.nearbyHosts.isEmpty {
                Text("搜索同一局域网内正在共享的设备。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(connection.nearbyHosts) { host in
                        Button {
                            Task { await connection.verify(code: host.request) }
                        } label: {
                            Label(host.label, systemImage: "desktopcomputer")
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(7)
                        }
                        .buttonStyle(.plain)
                        .disabled(connection.controllerID.isEmpty || connection.isChecking || connection.session != nil)
                        .help("请求远程控制")
                    }
                }
            }
        }
        .padding(15)
        .frame(width: 205)
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(.regularMaterial)
    }

    private var sharingCard: some View {
        VStack(alignment: .leading, spacing: 13) {
            HStack {
                Label("本机共享", systemImage: "desktopcomputer").font(.headline)
                Spacer()
                Toggle("允许远程协助", isOn: Binding(
                    get: { connection.isHosting },
                    set: { sharing in
                        if sharing {
                            Task {
                                await connection.refreshScreenPermission()
                                await connection.refreshInputPermission()
                                if connection.screenPermission == .granted && connection.inputPermission == .granted {
                                    await connection.startHosting()
                                } else {
                                    connection.showsHostPermissionGuide = true
                                }
                            }
                        } else {
                            connection.stopHosting()
                        }
                    }
                ))
                .toggleStyle(.switch)
                .controlSize(.small)
                .disabled(connection.hostID.isEmpty || connection.listenPort.isEmpty)
                .accessibilityIdentifier("host-sharing")
            }
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(connection.localHostName).font(.subheadline.weight(.semibold))
                    Text(connection.isSharingWithPeer ? "正在共享" : connection.isHosting ? "等待连接" : "共享已关闭")
                        .font(.caption)
                        .foregroundStyle(connection.isSharingWithPeer ? Color.green : .secondary)
                }
                Spacer()
                TextField("端口", text: $connection.listenPort)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 68)
                    .disabled(connection.isHosting)
                    .accessibilityLabel("监听端口")
                    .accessibilityIdentifier("listen-port")
            }
            Divider()
            copyRow(connection.connectionCode, placeholder: "开启共享后生成连接码", button: "复制连接码")
            if !connection.directInvite.isEmpty {
                Button("复制组网直连邀请") { copy(connection.directInvite) }
                    .help("可粘贴到 Koko 或传给 koko-agent pair")
            }
            if let request = connection.pendingPair {
                HStack {
                    Text("设备 \(request.fingerprint) 请求查看和控制此 Mac")
                    Spacer()
                    Button("拒绝") { connection.respondToPairing(request, allow: false) }
                    Button("允许") { connection.respondToPairing(request, allow: true) }
                        .buttonStyle(.borderedProminent)
                }
            }
            notice(connection.hostNotice)
        }
        .padding(17)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var controllerCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("连接伙伴", systemImage: "link").font(.headline)
            HStack {
                TextField("输入连接码或粘贴直连邀请", text: $connection.inviteInput)
                    .textFieldStyle(.roundedBorder)
                    .disabled(connection.isChecking || connection.session != nil)
                    .accessibilityIdentifier("remote-code")
                if connection.isChecking {
                    Button("取消") { connection.cancelCheck() }
                } else {
                    Button("连接") { Task { await connection.verify(code: connection.inviteInput) } }
                        .buttonStyle(.borderedProminent)
                        .disabled(connection.controllerID.isEmpty || connection.inviteInput.isEmpty || connection.session != nil)
                }
            }
            notice(connection.controllerNotice)
        }
        .padding(17)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var permissionsCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            permissionRows
            HStack {
                Button("测试本机画面") { screenTest.isPresented = true }
                    .popover(isPresented: $screenTest.isPresented) {
                        LocalScreenTest(test: screenTest)
                    }
                Button("键鼠测试区") { inputTest.show() }
                    .sheet(isPresented: $inputTest.isPresented) {
                        LocalInputTest(test: inputTest)
                    }
            }
            if connection.screenPermission == .required || connection.inputPermission == .required {
                if Bundle.main.object(forInfoDictionaryKey: "KokoAdHocSigned") as? Bool == true {
                    Text("此测试包使用临时签名。升级后 macOS 可能仍显示旧授权，需要为当前安装版本重新授权。")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Button("在访达显示当前 Koko") {
                    NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
                }
                .buttonStyle(.link)
            }
        }
        .padding(17)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var hostPermissionGuide: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("准备共享此 Mac").font(.title2.bold())
            Text("开启录屏后，对方可以看到画面。开启键鼠控制后，对方可以操作此 Mac。")
                .foregroundStyle(.secondary)
            permissionRows
            HStack {
                Spacer()
                Button("取消") { connection.showsHostPermissionGuide = false }
                    .keyboardShortcut(.cancelAction)
                Button(connection.screenPermission == .granted && connection.inputPermission == .granted
                       ? "开启共享" : "暂按现有权限共享") {
                    connection.showsHostPermissionGuide = false
                    Task { await connection.startHosting() }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(22)
        .frame(width: 510)
    }

    @ViewBuilder
    private var permissionRows: some View {
        permissionView(
            "屏幕录制",
            state: connection.screenPermission,
            checking: connection.isCheckingScreenPermission,
            hint: "在「录屏与系统录音」中允许 Koko，并按 macOS 提示退出重开。",
            requestTitle: "申请录屏权限",
            settingsPane: "Privacy_ScreenCapture"
        ) { request in
            await connection.refreshScreenPermission(request: request)
            return connection.screenPermission == .granted
        }
        Divider()
        permissionView(
            "辅助功能 · 键鼠控制",
            state: connection.inputPermission,
            checking: connection.isCheckingInputPermission,
            hint: "先申请，再在系统的「辅助功能」或「设备控制和数据访问」中启用 Koko。列表没有 Koko 时，用「＋」选择「应用程序」里的 Koko。启用后若仍显示未授权，回到这里再申请一次。",
            requestTitle: "申请键鼠权限",
            settingsPane: "Privacy_Accessibility"
        ) { request in
            await connection.refreshInputPermission(request: request)
            return connection.inputPermission == .granted
        }
    }

    private func permissionView(
        _ title: String,
        state: ScreenPermission,
        checking: Bool,
        hint: String,
        requestTitle: String,
        settingsPane: String,
        request: @escaping (Bool) async -> Bool
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(title).font(.headline)
                Spacer()
                if checking {
                    Text("正在检查…").foregroundStyle(.secondary)
                } else {
                    switch state {
                    case .unknown: Text("待检查").foregroundStyle(.secondary)
                    case .required: Text("未授权").foregroundStyle(.secondary)
                    case .granted: Text("已授权").foregroundStyle(.green)
                    case .failed: Text("检查失败").foregroundStyle(.red)
                    }
                }
            }
            if state != .granted {
                Text(hint)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button(requestTitle) {
                        Task {
                            if !(await request(true)) { openPrivacySettings(settingsPane) }
                        }
                    }
                    Button("打开系统设置") { openPrivacySettings(settingsPane) }
                    Button("重新检查") { Task { _ = await request(false) } }
                }
                .disabled(checking)
            }
        }
    }

    private func openPrivacySettings(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)"),
           NSWorkspace.shared.open(url) { return }
        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.systempreferences") {
            NSWorkspace.shared.open(url)
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

private struct RemoteSessionWindow: View {
    @ObservedObject var connection: LocalConnection
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        if connection.session != nil {
            RemoteControlView(connection: connection)
                .onDisappear { connection.disconnect() }
                .onChange(of: connection.session) { session in
                    if session == nil { dismiss() }
                }
        } else {
            Color.clear.onAppear { dismiss() }
        }
    }
}

@MainActor
private final class ScreenTest: ObservableObject {
    @Published var isPresented = false
    @Published private(set) var image: NSImage?
    @Published private(set) var message = ""

    func run() async {
        image = nil
        message = "正在采集本机主屏…"
        do {
            let png = try await captureScreen()
            guard !Task.isCancelled else { return }
            image = NSImage(data: png)
            message = image == nil ? "已采集主屏，但无法显示截图。" : "本机画面采集成功。"
        } catch ScreenCaptureError.permissionRequired {
            message = "当前运行的 Koko 没有录屏权限。请在系统设置中为下方路径重新授权。"
        } catch {
            let failure = error as NSError
            message = "本机画面采集失败（\(failure.domain) \(failure.code)）。"
        }
    }
}

private struct LocalScreenTest: View {
    @ObservedObject var test: ScreenTest

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let image = test.image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
                    .accessibilityLabel("本机主屏截图")
            }
            Text(test.message)
            Text(Bundle.main.bundleURL.path)
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding()
        .frame(width: 420, alignment: .leading)
        .task { await test.run() }
    }
}

@MainActor
private final class InputTest: ObservableObject {
    @Published var isPresented = false
    @Published var receivedClick = false
    @Published var text = ""

    func show() {
        text = ""
        receivedClick = false
        isPresented = true
    }
}

private struct LocalInputTest: View {
    @ObservedObject var test: InputTest

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("键鼠测试区").font(.headline)
            Text("在控制端点击下方按钮，再选中文本框，通过「控制中心 → 发送文本」发送 Koko。")
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            Button(test.receivedClick ? "已收到点击" : "测试点击") { test.receivedClick = true }
            TextField("等待输入 Koko", text: $test.text)
                .textFieldStyle(.roundedBorder)
            if test.text == "Koko" {
                Label("已收到文本", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            }
            Button("关闭") { test.isPresented = false }
                .keyboardShortcut(.cancelAction)
        }
        .padding()
        .frame(width: 380, alignment: .leading)
    }
}
