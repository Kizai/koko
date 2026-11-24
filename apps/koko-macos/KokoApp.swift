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

        Window("软件更新", id: "update") {
            UpdateWindow(updates: delegate.updates)
        }
        .windowResizability(.contentSize)

        Window("设置", id: "settings") {
            KokoSettingsView(connection: delegate.connection, updates: delegate.updates)
        }
        .defaultSize(width: 560, height: 520)
        .windowResizability(.contentSize)

        MenuBarExtra {
            KokoMenu(connection: delegate.connection)
        } label: {
            KokoMenuIcon(connection: delegate.connection, opener: delegate.opener)
        }
    }
}

/// Lets AppKit callbacks open SwiftUI windows. Set by a view that is always on screen.
@MainActor
final class WindowOpener {
    var openMain: () -> Void = {}
}

/// The menu bar icon stays while KoKo runs in the background, so sharing does not depend on a window.
private struct KokoMenuIcon: View {
    @ObservedObject var connection: LocalConnection
    let opener: WindowOpener
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Image(systemName: connection.agentControl == "active" ? "sparkles.tv"
              : connection.isSharingWithPeer || connection.agentSession != nil ? "display.2" : "display")
            .onAppear {
                opener.openMain = {
                    NSApp.setActivationPolicy(.regular)
                    openWindow(id: "connection")
                    NSApp.activate(ignoringOtherApps: true)
                }
            }
    }
}

private struct KokoMenu: View {
    @ObservedObject var connection: LocalConnection
    @Environment(\.openWindow) private var openWindow

    private var status: String {
        if connection.agentControl == "active" { return "Agent 正在操作本机" }
        if connection.isSharingWithPeer { return "正在共享本机" }
        if connection.agentSession != nil { return "Agent 已连接" }
        return connection.isHosting ? "等待连接" : "共享已关闭"
    }

    var body: some View {
        Text(status)
        Divider()
        Button("打开 KoKo") {
            NSApp.setActivationPolicy(.regular)
            openWindow(id: "connection")
            NSApp.activate(ignoringOtherApps: true)
        }
        if connection.agentControl == "active" {
            Button("停止 Agent") { Task { await connection.stopAgent() } }
        }
        Divider()
        Button("退出 KoKo") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}

@MainActor
private final class KokoAppDelegate: NSObject, NSApplicationDelegate {
    let connection = LocalConnection()
    let updates = ReleaseUpdates()
    let opener = WindowOpener()

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let name = Bundle.main.object(forInfoDictionaryKey: "CFBundleIconFile") as? String,
           let url = Bundle.main.url(forResource: name, withExtension: nil),
           let icon = NSImage(contentsOf: url) {
            NSApp.applicationIconImage = icon
        }
        CloseAction.observeWindows()
        KeepAwake.shared.update(hosting: false)
        connection.pairNotifications.configure()
        connection.pairNotifications.onOpenMain = { [weak self] in self?.opener.openMain() }
        Task { await connection.restoreSharing() }
    }

    /// Clicking the Dock icon brings the main window back after it was closed.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { opener.openMain() }
        return true
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        connection.refreshDesktopPermissions()
        Task { await updates.check(manual: false) }
    }

    /// By default closing windows keeps sharing running from the menu bar; the setting can quit instead.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        CloseAction.current == .quit
    }

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
    @ViewState private var helpNote = ""
    @ViewState private var hideVerificationCode = false
    @ViewState private var showCustomCodeSheet = false
    @ViewState private var customCodeDraft = ""
    @ViewState private var inlineDeviceCode = ""
    @ViewState private var showPartnerHistory = false
    @ViewState private var devicesExpanded = true
    @ViewState private var assistExpanded = true

    private func answerHelp(_ result: String) {
        let note = helpNote
        helpNote = ""
        Task { await connection.answerAgentHelp(result, note: note) }
    }

    var body: some View {
        VStack(spacing: 0) {
            windowHeader
            Divider()
            if let release = updates.automaticNotice {
                HStack(spacing: 12) {
                    Label("KoKo \(release.version) 可更新", systemImage: "arrow.down.circle")
                    Spacer()
                    Link("更新说明", destination: release.pageURL)
                    Button("查看并更新") {
                        updates.offer(release)
                        openWindow(id: "update")
                    }
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
                        } else if connection.showAllDevices {
                            allDevicesView
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
                            if connection.screenPermission != .granted || connection.inputPermission != .granted {
                                permissionsCard
                            }
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
            await connection.scanLAN()
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
        .sheet(isPresented: $showCustomCodeSheet) {
            customCodeSheet
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
                HStack(spacing: 5) {
                    Circle()
                        .fill(host.isOnline ? Color.green : Color.secondary.opacity(0.5))
                        .frame(width: 6, height: 6)
                    Text(host.isOnline ? "在线" : "离线")
                        .font(.caption.weight(.medium))
                }
                .foregroundStyle(host.isOnline ? .white : .secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(host.isOnline ? Color.black.opacity(0.75) : Color.secondary.opacity(0.15), in: Capsule())
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
                    if connection.rememberedDevices.contains(where: { $0.id == host.id }) {
                        Divider()
                        Button("移除设备记录", systemImage: "trash", role: .destructive) {
                            connection.removeDeviceRecord(host.id)
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .accessibilityLabel("设备操作")
            } else if connection.showAllDevices {
                Text("全部设备").font(.callout.weight(.semibold)).allowsHitTesting(false)
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
            HStack(spacing: 6) {
                Button {
                    withAnimation(.easeInOut(duration: 0.15)) { devicesExpanded.toggle() }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "laptopcomputer.and.iphone")
                            .foregroundStyle(.secondary)
                        Text("我的设备").font(.subheadline.weight(.medium))
                        Spacer()
                        Image(systemName: devicesExpanded ? "chevron.up" : "chevron.down")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                Button {
                    Task { await connection.scanLAN() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .disabled(connection.isScanning || connection.isChecking)
                .accessibilityLabel("搜索局域网")
                .help("搜索局域网")
            }

            if devicesExpanded {
                if connection.isScanning {
                    ProgressView("正在搜索…").controlSize(.small)
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(connection.nearbyHosts) { host in
                            Button {
                                connection.showAllDevices = false
                                connection.selectedNearbyHostID = host.id
                                inlineDeviceCode = ""
                            } label: {
                                HStack(spacing: 7) {
                                    Circle()
                                        .fill(host.isOnline ? Color.green : .clear)
                                        .frame(width: 6, height: 6)
                                    Image(systemName: host.platformIcon)
                                        .font(.system(size: 13))
                                        .foregroundStyle(host.isOnline ? .primary : .secondary)
                                        .frame(width: 16)
                                    Text(connection.name(for: host))
                                        .lineLimit(1)
                                        .foregroundStyle(host.isOnline ? .primary : .secondary)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 6)
                                .background(connection.selectedNearbyHostID == host.id ? Color.primary.opacity(0.08) : .clear,
                                            in: RoundedRectangle(cornerRadius: 6))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .help("\(host.platformName) · \(connection.name(for: host))\(host.isOnline ? "" : "（离线）")")
                        }
                        Button {
                            connection.selectedNearbyHostID = nil
                            connection.showAllDevices = true
                        } label: {
                            HStack(spacing: 7) {
                                Spacer().frame(width: 6)
                                Image(systemName: "square.grid.2x2.fill")
                                    .font(.system(size: 13))
                                    .foregroundStyle(Color.accentColor)
                                    .frame(width: 16)
                                Text("全部设备")
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 6)
                            .background(connection.showAllDevices && connection.selectedNearbyHost == nil ? Color.primary.opacity(0.08) : .clear,
                                        in: RoundedRectangle(cornerRadius: 6))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(maxHeight: 260)
            }

            Divider()

            Button {
                withAnimation(.easeInOut(duration: 0.15)) { assistExpanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "rectangle.connected.to.line.below")
                        .foregroundStyle(.secondary)
                    Text("远程协助").font(.subheadline.weight(.medium))
                    Spacer()
                    Image(systemName: assistExpanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if assistExpanded {
                Button {
                    connection.selectedNearbyHostID = nil
                    connection.showAllDevices = false
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "display.2")
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 16)
                        Text("开始协助")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 7)
                    .background(!connection.showAllDevices && connection.selectedNearbyHost == nil ? Color.primary.opacity(0.08) : .clear,
                                in: RoundedRectangle(cornerRadius: 6))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }

            Spacer()
        }
        .padding(14)
        .frame(width: 220)
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(.regularMaterial)
    }

    private var allDevicesView: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("电脑 \(1 + connection.nearbyHosts.count)")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(.secondary)

            VStack(spacing: 8) {
                HStack(spacing: 12) {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Color.accentColor)
                        .frame(width: 30, height: 30)
                        .overlay {
                            Image(systemName: "apple.logo")
                                .font(.system(size: 15, weight: .medium))
                                .foregroundStyle(.white)
                        }
                    Text(connection.localHostName)
                        .font(.body.weight(.medium))
                    Text("本机")
                        .font(.caption2.weight(.medium))
                        .foregroundStyle(Color.accentColor)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                    Spacer()
                    if !connection.hostDeviceID.isEmpty {
                        Text(formatDeviceID(connection.hostDeviceID))
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10))

                ForEach(connection.nearbyHosts) { host in
                    Button {
                        connection.showAllDevices = false
                        connection.selectedNearbyHostID = host.id
                        inlineDeviceCode = ""
                    } label: {
                        HStack(spacing: 12) {
                            RoundedRectangle(cornerRadius: 8)
                                .fill(host.isOnline ? Color.accentColor : Color.secondary.opacity(0.35))
                                .frame(width: 30, height: 30)
                                .overlay {
                                    Image(systemName: host.platformIcon)
                                        .font(.system(size: 14, weight: .medium))
                                        .foregroundStyle(.white)
                                }
                            Text(connection.name(for: host))
                                .font(.body.weight(.medium))
                                .foregroundStyle(host.isOnline ? .primary : .secondary)
                            if !host.isOnline {
                                Text("离线")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 2)
                                    .background(.quaternary, in: Capsule())
                            }
                            Spacer()
                            if let deviceID = host.deviceID, !deviceID.isEmpty {
                                Text(formatDeviceID(deviceID))
                                    .font(.system(.caption, design: .monospaced))
                                    .foregroundStyle(.secondary)
                            }
                            Image(systemName: "chevron.right")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 12)
                        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func nearbyDeviceCard(_ host: NearbyHost) -> some View {
        let hasSession = connection.session?.hostID == host.id
        let canOpenDesktop = host.isOnline && !connection.isChecking && (hasSession || (!connection.controllerID.isEmpty && connection.session == nil))
        let preferredAddress = hasSession ? connection.session?.address : nil
        let needsCodePrompt = host.isOnline && connection.codePromptHostID == host.id
        return VStack(alignment: .leading, spacing: 16) {
            VStack(spacing: 0) {
                ZStack {
                    if let cover = connection.deviceCovers[host.id] {
                        Image(nsImage: cover)
                            .resizable()
                            .scaledToFill()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .clipped()
                            .saturation(host.isOnline ? 1 : 0.25)
                            .opacity(host.isOnline ? 1 : 0.65)
                            .overlay(
                                LinearGradient(
                                    colors: [.black.opacity(0.05), .black.opacity(0.45)],
                                    startPoint: .top,
                                    endPoint: .bottom
                                )
                            )
                    } else {
                        DeviceCardBackdrop()
                            .saturation(host.isOnline ? 1 : 0.25)
                            .opacity(host.isOnline ? 1 : 0.7)
                    }

                    VStack(spacing: 12) {
                        Spacer()
                        if connection.isChecking {
                            HStack {
                                ProgressView().controlSize(.small)
                                Button("取消连接") { connection.cancelCheck() }
                            }
                            .environment(\.colorScheme, .dark)
                        } else if !host.isOnline {
                            Text("设备当前离线")
                                .font(.callout.weight(.medium))
                                .foregroundStyle(.white.opacity(0.85))
                                .padding(.horizontal, 14)
                                .padding(.vertical, 6)
                                .background(.black.opacity(0.45), in: Capsule())
                        } else if needsCodePrompt {
                            HStack(spacing: 8) {
                                TextField("输入设备验证码", text: $inlineDeviceCode)
                                    .textFieldStyle(.roundedBorder)
                                    .frame(width: 155)
                                    .onSubmit {
                                        Task { await connection.openSavedDevice(host, code: inlineDeviceCode, viewOnly: false) }
                                    }
                                Button("进入桌面") {
                                    Task { await connection.openSavedDevice(host, code: inlineDeviceCode, viewOnly: false) }
                                }
                                .buttonStyle(.borderedProminent)
                                .disabled(inlineDeviceCode.trimmingCharacters(in: .whitespaces).isEmpty)
                                Button("请求确认") {
                                    Task { await connection.openSavedDevice(host, quiet: false, viewOnly: false) }
                                }
                            }
                            .padding(10)
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 9))
                        } else {
                            Button {
                                openDesktop(host, viewOnly: false)
                            } label: {
                                Label(hasSession && !connection.isViewOnly ? "打开远控窗口" : "进入桌面",
                                      systemImage: "arrow.right")
                                    .labelStyle(.titleAndIcon)
                                    .font(.title3.weight(.semibold))
                                    .foregroundStyle(.white)
                                    .padding(.horizontal, 18)
                                    .padding(.vertical, 9)
                                    .background(.black.opacity(0.35), in: Capsule())
                            }
                            .buttonStyle(.plain)
                            .opacity(canOpenDesktop ? 1 : 0.5)
                            .disabled(!canOpenDesktop)
                        }
                    }
                    .padding(24)
                }
                .frame(height: 220)
                .clipped()

                HStack(spacing: 0) {
                    deviceAction("文件传输", icon: "folder.badge.plus", enabled: host.isOnline) {
                        tool = DeviceToolRequest(.files, host: host, name: connection.name(for: host), preferredAddress: preferredAddress)
                    }
                    Divider()
                    deviceAction("观看模式", icon: "eye", enabled: canOpenDesktop) {
                        openDesktop(host, viewOnly: true)
                    }
                    Divider()
                    deviceAction("终端", icon: "terminal", enabled: host.isOnline) {
                        tool = DeviceToolRequest(.terminal, host: host, name: connection.name(for: host), preferredAddress: preferredAddress)
                    }
                    Divider()
                    deviceAction("端口转发", icon: "point.3.connected.trianglepath.dotted", enabled: host.isOnline) {
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
            Task { await connection.openSavedDevice(host, viewOnly: viewOnly) }
        }
    }

    private var displayedVerificationCode: String {
        switch connection.verificationMode {
        case .temporary, .both:
            connection.connectionCode
        case .custom:
            connection.customVerificationCode
        }
    }

    private var sharingCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text("本设备").font(.headline)
                if connection.isSharingWithPeer {
                    Text("正在共享")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.green)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Color.green.opacity(0.12), in: Capsule())
                }
                Spacer()
                Text("允许他人远程协助")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Toggle("", isOn: Binding(
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
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                .disabled(connection.hostID.isEmpty || connection.listenPort.isEmpty)
                .accessibilityIdentifier("host-sharing")
            }
            Divider()
            HStack(alignment: .top, spacing: 28) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("本设备 ID")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(connection.hostDeviceID.isEmpty ? "— — —" : formatDeviceID(connection.hostDeviceID))
                        .font(.system(size: 24, weight: .bold, design: .monospaced))
                        .foregroundStyle(connection.hostDeviceID.isEmpty ? .secondary : .primary)
                        .textSelection(.enabled)
                    Text(connection.localHostName)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(minWidth: 190, alignment: .leading)

                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 4) {
                        Text("验证方式：")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Menu {
                            ForEach(VerificationCodeMode.allCases) { mode in
                                Button {
                                    Task { await connection.setVerificationMode(mode) }
                                } label: {
                                    if connection.verificationMode == mode {
                                        Label(mode.title, systemImage: "checkmark")
                                    } else {
                                        Text(mode.title)
                                    }
                                }
                            }
                            Divider()
                            Button("修改自定义验证码") {
                                customCodeDraft = connection.customVerificationCode
                                showCustomCodeSheet = true
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Text(connection.verificationMode.title)
                                Image(systemName: "chevron.up.chevron.down")
                                    .font(.caption2)
                            }
                            .font(.caption.weight(.medium))
                        }
                        .menuStyle(.borderlessButton)
                        .fixedSize()
                    }
                    HStack(spacing: 10) {
                        let code = displayedVerificationCode
                        Text(code.isEmpty ? "开启共享后生成" : (hideVerificationCode ? String(repeating: "•", count: max(6, code.count)) : code))
                            .font(.system(size: code.isEmpty ? 15 : 24, weight: .bold, design: .monospaced))
                            .foregroundStyle(code.isEmpty ? .secondary : .primary)
                            .textSelection(.enabled)
                        Spacer()
                        Button {
                            hideVerificationCode.toggle()
                        } label: {
                            Image(systemName: hideVerificationCode ? "eye.slash" : "eye")
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.borderless)
                        .help(hideVerificationCode ? "显示验证码" : "隐藏验证码")
                        if connection.verificationMode != .custom {
                            Button {
                                Task { await connection.refreshTemporaryCode() }
                            } label: {
                                Image(systemName: "arrow.clockwise")
                                    .foregroundStyle(.secondary)
                            }
                            .buttonStyle(.borderless)
                            .disabled(!connection.isHosting)
                            .help("刷新临时验证码")
                        }
                        Button {
                            customCodeDraft = connection.customVerificationCode
                            showCustomCodeSheet = true
                        } label: {
                            Image(systemName: "gearshape")
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.borderless)
                        .help("设置自定义验证码")
                        Button("复制并分享") {
                            let idText = formatDeviceID(connection.hostDeviceID)
                            let shareText = if !connection.directInvite.isEmpty {
                                "设备 ID：\(idText)\n验证码：\(displayedVerificationCode)\n直连邀请：\(connection.directInvite)"
                            } else {
                                "设备 ID：\(idText)\n验证码：\(displayedVerificationCode)"
                            }
                            copy(shareText)
                        }
                        .disabled(connection.hostDeviceID.isEmpty || displayedVerificationCode.isEmpty)
                    }
                    Text(connection.verificationMode.hint)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
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
                    VStack(alignment: .leading, spacing: 3) {
                        Label("Agent「\(agent.name)」已连接，可\(agentAccessTitle(agent.scope))", systemImage: "sparkles")
                            .font(.callout)
                        switch connection.agentControl {
                        case "active":
                            Text("正在操作这台 Mac。移动鼠标、按键或按 ⌃⌥⌘. 都会让它暂停。")
                                .font(.caption).foregroundStyle(.orange)
                        case "paused":
                            Text("已暂停，等你允许后才能继续操作。").font(.caption).foregroundStyle(.secondary)
                        case "awaiting_approval":
                            Text("等你确认高危操作：\(connection.agentApproval ?? "")")
                                .font(.caption).foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true)
                        case "awaiting_human":
                            Text("请你协助：\(connection.agentHelp ?? "")")
                                .font(.caption).foregroundStyle(.blue)
                                .fixedSize(horizontal: false, vertical: true)
                        default:
                            EmptyView()
                        }
                    }
                    Spacer()
                    switch connection.agentControl {
                    case "active": Button("停止") { Task { await connection.stopAgent() } }
                    case "paused": Button("让 Agent 继续") { Task { await connection.resumeAgent() } }
                    default: EmptyView()
                    }
                    Button("断开 Agent") { Task { await connection.endAgentSession() } }
                }
                if connection.agentControl == "awaiting_approval" {
                    HStack {
                        Spacer()
                        Button("允许一次") { Task { await connection.decideAgentApproval("once") } }
                        Button("本次会话都允许") { Task { await connection.decideAgentApproval("session") } }
                        Button("拒绝") { Task { await connection.decideAgentApproval("deny") } }
                            .buttonStyle(.borderedProminent)
                    }
                }
                if connection.agentControl == "awaiting_human" {
                    HStack {
                        TextField("给 Agent 的附言（可选）", text: $helpNote)
                            .textFieldStyle(.roundedBorder)
                        Button("已完成") { answerHelp("resolved") }
                            .buttonStyle(.borderedProminent)
                        Button("我来接管") { answerHelp("taken_over") }
                        Button("拒绝") { answerHelp("declined") }
                    }
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
            Text("远控伙伴设备").font(.headline)
            Text("通过其他设备的【设备 ID】及【设备验证码】启动远程控制")
                .font(.caption)
                .foregroundStyle(.secondary)
            Divider()
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("伙伴的设备 ID")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    TextField("请输入设备 ID 或直连邀请", text: $connection.partnerDeviceID)
                        .textFieldStyle(.plain)
                        .padding(.leading, 10)
                        .padding(.trailing, connection.rememberedDevices.isEmpty ? 10 : 30)
                        .frame(height: 28)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                        .overlay {
                            RoundedRectangle(cornerRadius: 6)
                                .strokeBorder(Color(nsColor: .separatorColor))
                        }
                        .overlay(alignment: .trailing) {
                            if !connection.rememberedDevices.isEmpty {
                                Button {
                                    showPartnerHistory.toggle()
                                } label: {
                                    Image(systemName: "chevron.down")
                                        .font(.caption2.weight(.semibold))
                                        .foregroundStyle(.secondary)
                                        .frame(width: 28, height: 28)
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .help("选择历史连接设备")
                            }
                        }
                        .popover(isPresented: $showPartnerHistory, arrowEdge: .bottom) {
                            VStack(alignment: .leading, spacing: 2) {
                                ForEach(connection.rememberedDevices) { record in
                                    let targetID = record.deviceID.isEmpty ? record.id : formatDeviceID(record.deviceID)
                                    let isOnline = connection.nearbyHosts.contains { $0.id == record.id && $0.isOnline }
                                    Button {
                                        connection.partnerDeviceID = targetID
                                        showPartnerHistory = false
                                    } label: {
                                        HStack(spacing: 8) {
                                            Circle()
                                                .fill(isOnline ? Color.green : Color.secondary.opacity(0.35))
                                                .frame(width: 6, height: 6)
                                            Text(connection.deviceNames[record.id] ?? record.name)
                                                .lineLimit(1)
                                            Spacer(minLength: 12)
                                            Text(record.deviceID.isEmpty ? String(record.id.prefix(8)) : formatDeviceID(record.deviceID))
                                                .font(.system(.caption, design: .monospaced))
                                                .foregroundStyle(.secondary)
                                        }
                                        .padding(.horizontal, 10)
                                        .padding(.vertical, 6)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                            .padding(6)
                            .frame(minWidth: 280)
                        }
                        .disabled(connection.isChecking || connection.session != nil)
                        .accessibilityIdentifier("remote-code")
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("设备验证码")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    TextField("验证码（已信任可留空）", text: $connection.partnerCode)
                        .textFieldStyle(.plain)
                        .padding(.horizontal, 10)
                        .frame(width: 170, height: 28)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                        .overlay {
                            RoundedRectangle(cornerRadius: 6)
                                .strokeBorder(Color(nsColor: .separatorColor))
                        }
                        .disabled(connection.isChecking || connection.session != nil)
                        .onSubmit {
                            Task { await connection.connectByDeviceID(connection.partnerDeviceID, code: connection.partnerCode) }
                        }
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text(" ").font(.caption)
                    if connection.isChecking {
                        Button("取消") { connection.cancelCheck() }
                    } else {
                        Button("连接") {
                            Task { await connection.connectByDeviceID(connection.partnerDeviceID, code: connection.partnerCode) }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(connection.controllerID.isEmpty || connection.partnerDeviceID.trimmingCharacters(in: .whitespaces).isEmpty || connection.session != nil)
                    }
                }
            }
            notice(connection.controllerNotice)
        }
        .padding(17)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var customCodeSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("自定义验证码").font(.headline)
            Text("设置 6–16 位字母或数字验证码。留空保存则清除自定义验证码。")
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("例如 KOKO2026", text: $customCodeDraft)
                .textFieldStyle(.roundedBorder)
            let cleaned = customCodeDraft.filter { $0.isLetter || $0.isNumber }
            let valid = cleaned.isEmpty || (6...16).contains(cleaned.count)
            if !valid {
                Text("请输入 6–16 位字母或数字。").font(.caption).foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button("取消") { showCustomCodeSheet = false }
                    .keyboardShortcut(.cancelAction)
                Button("保存") {
                    Task {
                        await connection.saveCustomVerificationCode(customCodeDraft)
                        showCustomCodeSheet = false
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!valid)
            }
        }
        .padding(20)
        .frame(width: 340)
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
