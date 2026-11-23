import AppKit
import SwiftUI

@main
struct KokoApp: App {
    @NSApplicationDelegateAdaptor(KokoAppDelegate.self) private var delegate

    var body: some Scene {
        Window("远程协助", id: "connection") {
            ConnectionView(connection: delegate.connection, updates: delegate.updates)
        }
        .defaultSize(width: 870, height: 640)
        .windowResizability(.contentMinSize)
        .windowStyle(.hiddenTitleBar)

        Window("远程控制", id: "remote") {
            RemoteSessionWindow(connection: delegate.connection)
        }
        .defaultSize(width: 1080, height: 700)
        .windowResizability(.contentMinSize)
        .windowStyle(.hiddenTitleBar)

        Window("设置", id: "settings") {
            KokoSettingsView(connection: delegate.connection, updates: delegate.updates)
        }
        .defaultSize(width: 550, height: 430)
        .windowResizability(.contentSize)
    }
}

@MainActor
private final class KokoAppDelegate: NSObject, NSApplicationDelegate {
    let connection = LocalConnection()
    let updates = ReleaseUpdates()

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let name = Bundle.main.object(forInfoDictionaryKey: "CFBundleIconFile") as? String,
           let url = Bundle.main.url(forResource: name, withExtension: nil),
           let icon = NSImage(contentsOf: url) {
            NSApp.applicationIconImage = icon
        }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        connection.refreshDesktopPermissions()
        Task { await updates.check(manual: false) }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        connection.stopForQuit()
    }
}

private struct ConnectionView: View {
    @ObservedObject var connection: LocalConnection
    @ObservedObject var updates: ReleaseUpdates
    @Environment(\.openWindow) private var openWindow
    @StateObject private var screenTest = ScreenTest()
    @StateObject private var inputTest = InputTest()
    @ViewState private var headerHeight = WindowButtonAlignment.defaultHeight
    @ViewState private var tool: DeviceToolRequest?
    @ViewState private var renaming: NearbyHost?
    @ViewState private var newDeviceName = ""
    @ViewState private var showRename = false
    @AppStorage("permissionGuideProgress") private var permissionProgress = PermissionGuide.Progress.introduction
    @ViewState private var reviewAfterRestart = CommandLine.arguments.contains(PermissionRelaunch.argument)
    @ViewState private var permissionGuide: PermissionGuide.Context?

    var body: some View {
        VStack(spacing: 0) {
            windowHeader
            Divider()
            if let release = updates.automaticNotice {
                HStack(spacing: 12) {
                    Label("KoKo \(release.version) 可更新", systemImage: "arrow.down.circle")
                    Spacer()
                    Link("更新说明", destination: release.pageURL)
                    Button("下载并安装") { updates.install(release) }
                    Button("跳过此版本", action: updates.dismissAutomaticNotice)
                        .buttonStyle(.borderless)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(Color.accentColor.opacity(0.08))
            }
            HStack(spacing: 0) {
                sidebar
                Divider()
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        if let device = connection.selectedNearbyHost {
                            nearbyDeviceCard(device)
                        } else {
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
                    }
                    .frame(maxWidth: 720)
                    .padding(25)
                    .frame(maxWidth: .infinity)
                }
            }
        }
        .frame(minWidth: 740, minHeight: 560)
        .ignoresSafeArea(.container, edges: .top)
        .onAppear {
            let lostPermissions = connection.checkForUpgradePermissionLoss()
            switch permissionProgress {
            case .introduction, .configuring: permissionGuide = .firstLaunch
            case .finished, .skipped:
                if reviewAfterRestart { permissionGuide = .settings }
                else if permissionProgress == .finished && lostPermissions { permissionGuide = .upgrade }
            }
            reviewAfterRestart = false
        }
        .task {
            await connection.prepareIdentity(for: .host)
            await connection.prepareIdentity(for: .controller)
        }
        .onChange(of: connection.session) { session in
            if session != nil { openWindow(id: "remote") }
        }
        .sheet(item: $permissionGuide, onDismiss: connection.permissionGuideDidDismiss) { context in
            PermissionGuide(connection: connection, context: context)
        }
        .sheet(item: $tool) { request in
            DeviceToolSheet(request: request)
        }
        .alert("重命名设备", isPresented: $showRename) {
            TextField("本地设备名称", text: $newDeviceName)
            Button("保存") {
                if let renaming { connection.rename(renaming, to: newDeviceName) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("名称只显示在这台 Mac 上。留空会恢复设备原名。")
        }
        .confirmationDialog("清除已信任设备？", isPresented: $connection.confirmForgetTrusted) {
            Button("清除信任", role: .destructive) { Task { await connection.forgetTrustedDevices() } }
        } message: {
            Text("下次连接将重新请求确认。当前共享中的会话也会结束。")
        }
    }

    private var windowHeader: some View {
        HStack(spacing: 8) {
            if let host = connection.selectedNearbyHost {
                Image(systemName: host.platformIcon)
                    .help(host.platformName)
                    .font(.system(size: 13))
                    .foregroundStyle(.green)
                    .accessibilityLabel("局域网已发现")
                Text(connection.name(for: host)).font(.callout.weight(.semibold)).lineLimit(1)
                    .allowsHitTesting(false)
                Menu {
                    Button("刷新设备", systemImage: "arrow.clockwise") {
                        Task { await connection.scanLAN() }
                    }
                    Button("重命名", systemImage: "pencil") {
                        renaming = host
                        newDeviceName = connection.name(for: host)
                        showRename = true
                    }
                    Button("从列表隐藏", systemImage: "eye.slash") { connection.hide(host) }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .accessibilityLabel("设备操作")
            } else {
                Text("远程协助").font(.callout.weight(.semibold)).allowsHitTesting(false)
            }
            Spacer()
            Button {
                openWindow(id: "settings")
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 15, weight: .medium))
                    .frame(width: 30, height: 26)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .keyboardShortcut(",", modifiers: .command)
            .accessibilityLabel("设置")
            .help("设置")
        }
        .padding(.leading, WindowButtonAlignment.leadingInset)
        .padding(.trailing, 12)
        .frame(height: headerHeight)
        .background(WindowButtonAlignment(height: $headerHeight))
        .background(.regularMaterial)
    }

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button {
                connection.selectedNearbyHostID = nil
            } label: {
                Label("本机与远程协助", systemImage: "macwindow")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
                    .background(connection.selectedNearbyHost == nil ? Color.primary.opacity(0.07) : .clear,
                                in: RoundedRectangle(cornerRadius: 6))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
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
                            connection.selectedNearbyHostID = host.id
                        } label: {
                            Label(connection.name(for: host), systemImage: host.platformIcon)
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(8)
                                .background(connection.selectedNearbyHostID == host.id ? Color.accentColor.opacity(0.13) : .clear,
                                            in: RoundedRectangle(cornerRadius: 6))
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help("\(host.platformName) · \(connection.name(for: host))")
                    }
                }
            }
        }
        .padding(15)
        .frame(width: 205)
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(.regularMaterial)
    }

    private func nearbyDeviceCard(_ host: NearbyHost) -> some View {
        let hasSession = connection.session?.hostID == host.id
        let canOpenDesktop = !connection.isChecking && (hasSession || (!connection.controllerID.isEmpty && connection.session == nil))
        let preferredAddress = hasSession ? connection.session?.address : nil
        return VStack(alignment: .leading, spacing: 16) {
            VStack(spacing: 0) {
                ZStack(alignment: .bottomLeading) {
                    DeviceCardBackdrop()
                    VStack(alignment: .leading, spacing: 11) {
                        if connection.isChecking {
                            HStack {
                                ProgressView().controlSize(.small)
                                Button("取消连接") { connection.cancelCheck() }
                            }
                            .environment(\.colorScheme, .dark)
                        } else {
                            Text("远程桌面")
                                .font(.system(size: 31, weight: .semibold))
                                .foregroundStyle(.white)
                            Button {
                                openDesktop(host, viewOnly: false)
                            } label: {
                                Label(hasSession && !connection.isViewOnly ? "打开远控窗口" : "进入桌面",
                                      systemImage: "arrow.up.right")
                                    .font(.callout.weight(.semibold))
                                    .foregroundStyle(Color(red: 0.035, green: 0.12, blue: 0.14))
                                    .padding(.horizontal, 14)
                                    .padding(.vertical, 8)
                                    .background(Color(red: 0.60, green: 0.93, blue: 0.85), in: RoundedRectangle(cornerRadius: 7))
                            }
                            .buttonStyle(.plain)
                            .opacity(canOpenDesktop ? 1 : 0.5)
                            .disabled(!canOpenDesktop)
                        }
                    }
                    .padding(23)
                }
                .frame(height: 210)

                HStack(spacing: 0) {
                    deviceAction("文件传输", icon: "folder.badge.plus") {
                        tool = DeviceToolRequest(.files, host: host, name: connection.name(for: host), preferredAddress: preferredAddress)
                    }
                    Divider()
                    deviceAction("观看模式", icon: "eye", enabled: canOpenDesktop) {
                        openDesktop(host, viewOnly: true)
                    }
                    Divider()
                    deviceAction("终端", icon: "terminal") {
                        tool = DeviceToolRequest(.terminal, host: host, name: connection.name(for: host), preferredAddress: preferredAddress)
                    }
                    Divider()
                    deviceAction("端口转发", icon: "point.3.connected.trianglepath.dotted") {
                        tool = DeviceToolRequest(.forwarding, host: host, name: connection.name(for: host), preferredAddress: preferredAddress)
                    }
                }
                .frame(height: 48)
                .background(Color(nsColor: .controlBackgroundColor))
            }
            .clipShape(RoundedRectangle(cornerRadius: 11))
            .overlay {
                RoundedRectangle(cornerRadius: 11).strokeBorder(.quaternary)
                    .allowsHitTesting(false)
            }

            if hasSession {
                HStack {
                    Label(connection.isViewOnly ? "观看模式已连接" : "远控已连接", systemImage: "checkmark.circle")
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("断开连接") { connection.disconnect() }
                }
            }
            notice(connection.controllerNotice)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func deviceAction(_ title: String, icon: String, enabled: Bool = true, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: icon)
                .font(.callout.weight(.medium))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func openDesktop(_ host: NearbyHost, viewOnly: Bool) {
        if connection.session?.hostID == host.id {
            connection.switchSessionMode(viewOnly: viewOnly)
            openWindow(id: "remote")
        } else {
            Task { await connection.verify(code: host.request, viewOnly: viewOnly) }
        }
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
                                if permissionProgress == .skipped { permissionProgress = .finished }
                                connection.refreshDesktopPermissions()
                                if connection.screenPermission == .granted && connection.inputPermission == .granted {
                                    await connection.startHosting()
                                } else {
                                    permissionGuide = .sharing
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
                VStack(alignment: .leading, spacing: 7) {
                    if request.isAgent {
                        Label("Agent「\(request.name)」（\(request.fingerprint)）请求\(agentAccessTitle(request.scope))",
                              systemImage: "sparkles")
                        Text("Agent 由 AI 程序驱动。只在首次连接时确认；信任后，它在本机共享开启时可直接连接。")
                            .font(.caption).foregroundStyle(.secondary)
                    } else {
                        Text("设备 \(request.fingerprint) 请求查看和控制此 Mac")
                        Text("信任后，此设备在本机共享开启时可直接连接。")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    HStack {
                        Button("拒绝") { Task { await connection.respondToPairing(request, allow: false) } }
                        Button("仅允许这次") { Task { await connection.respondToPairing(request, allow: true) } }
                        Button("信任并连接") { Task { await connection.respondToPairing(request, allow: true, remember: true) } }
                            .buttonStyle(.borderedProminent)
                    }
                }
            }
            if let agent = connection.agentSession {
                HStack {
                    Label("Agent「\(agent.name)」已连接，可\(agentAccessTitle(agent.scope))", systemImage: "sparkles")
                        .font(.callout)
                    Spacer()
                    Button("断开 Agent") { Task { await connection.endAgentSession() } }
                }
            }
            if connection.trustedDeviceCount > 0 {
                HStack {
                    Text("已信任 \(connection.trustedDeviceCount) 台设备")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("清除信任…") { connection.confirmForgetTrusted = true }
                        .buttonStyle(.link)
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
            HStack {
                Label("本机权限", systemImage: "lock.shield").font(.headline)
                Spacer()
                Button("管理权限…") { permissionGuide = .settings }
            }
            HStack(spacing: 18) {
                Text("屏幕录制 · \(connection.screenPermission.title)")
                Text("辅助功能 · \(connection.inputPermission.title)")
            }
            .font(.callout)
            .foregroundStyle(.secondary)
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
