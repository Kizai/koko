import AppKit
import ServiceManagement
import SwiftUI

let defaultDownloadDirectory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads").path

struct KokoSettingsView: View {
    @ObservedObject var connection: LocalConnection
    @ObservedObject var updates: ReleaseUpdates
    @AppStorage("defaultFrameRate") private var defaultFrameRate = 60
    @AppStorage("downloadDirectory") private var downloadDirectory = defaultDownloadDirectory
    @AppStorage("forwardPhysicalKeyboard") private var forwardPhysicalKeyboard = true
    @ViewState private var section = Section.general
    @ViewState private var listenPort = ""
    @ViewState private var showForget = false
    @ViewState private var showPermissions = false
    @AppStorage("enhancedConnections") private var enhancedConnections = true
    @AppStorage("preferLAN") private var preferLAN = true
    @AppStorage("adaptiveStreaming") private var adaptiveStreaming = true
    @AppStorage("mapWindowsModifiers") private var mapWindowsModifiers = true
    @AppStorage(KeepAwake.key) private var keepAwake = KeepAwake.Mode.never.rawValue
    @AppStorage(CloseAction.key) private var closeAction = CloseAction.menuBar.rawValue
    @AppStorage("dragSendPanel") private var dragSendPanel = true
    @ViewState private var launchAtLogin = LaunchAtLogin.isEnabled
    @ViewState private var loginNeedsApproval = LaunchAtLogin.needsApproval
    @ViewState private var loginError: String?
    @Environment(\.openWindow) private var openWindow

    private enum Section: String, CaseIterable, Identifiable {
        case general = "常规", security = "安全", keyboard = "键盘", network = "网络", advanced = "高级"
        var id: Self { self }
        var icon: String {
            switch self {
            case .general: "gearshape"
            case .security: "lock"
            case .keyboard: "keyboard"
            case .network: "network"
            case .advanced: "slider.horizontal.3"
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            Picker("设置分类", selection: $section) {
                ForEach(Section.allCases) { section in
                    Label(section.rawValue, systemImage: section.icon).tag(section)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(18)
            Divider()
            Form {
                switch section {
                case .general:
                    SwiftUI.Section {
                        Toggle(isOn: Binding(get: { launchAtLogin }, set: setLaunchAtLogin)) {
                            Text("开机自启动")
                        }
                        .toggleStyle(.switch)
                        if loginNeedsApproval {
                            LabeledContent("需要在系统设置中允许 KoKo 登录时打开") {
                                Button("打开登录项设置") { SMAppService.openSystemSettingsLoginItems() }
                            }
                            .font(.callout)
                        }
                        Picker(selection: $keepAwake) {
                            ForEach(KeepAwake.Mode.allCases) { Text($0.title).tag($0.rawValue) }
                        } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text("让这台 Mac 保持唤醒状态")
                                Text("防止闲置时进入睡眠导致无法远程访问。屏幕仍可关闭，合上屏幕后仍会睡眠。")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        Picker("关闭窗口时", selection: $closeAction) {
                            ForEach(CloseAction.allCases) { Text($0.title).tag($0.rawValue) }
                        }
                    }
                    SwiftUI.Section {
                        Toggle(isOn: $dragSendPanel) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text("被控时拖拽文件显示发送浮窗")
                                Text("开启后，被控时拖动文件会在屏幕右侧出现浮窗，拖进去即可发给控制端。")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        .toggleStyle(.switch)
                    }
                    SwiftUI.Section {
                        Picker("新会话的帧率", selection: $defaultFrameRate) {
                            ForEach(LocalConnection.frameRates, id: \.self) { fps in
                                Text("\(fps) FPS").tag(fps)
                            }
                        }
                    }
                    SwiftUI.Section {
                        LabeledContent("传输文件存储路径") {
                            HStack {
                                Button("修改路径", action: chooseDownloadDirectory)
                                Button("打开文件夹") {
                                    NSWorkspace.shared.open(URL(fileURLWithPath: downloadDirectory, isDirectory: true))
                                }
                            }
                        }
                        Text(downloadDirectory)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(downloadDirectory)
                    }
                    SwiftUI.Section {
                        LabeledContent("版本") {
                            Text(updates.installedVersion ?? "—")
                            Button("检查更新") { Task { await updates.check(manual: true) } }
                                .disabled(updates.state == .checking)
                        }
                        switch updates.state {
                        case .idle:
                            EmptyView()
                        case .checking:
                            Text("正在检查 GitHub Releases…")
                        case .upToDate:
                            Text("当前已是最新可用版本。").foregroundStyle(.secondary)
                        case .available(let release):
                            LabeledContent("有可用更新 · \(release.version)") {
                                Button("查看并更新") {
                                    updates.offer(release)
                                    openWindow(id: "update")
                                }
                            }
                        case .failed(let message):
                            LabeledContent {
                                Link("打开发布页", destination: URL(string: "https://github.com/Kizai/koko/releases")!)
                            } label: {
                                Text(message).font(.callout)
                            }
                        }
                    }
                case .security:
                    Button("权限设置…") { showPermissions = true }
                    LabeledContent("录屏", value: connection.screenPermission.title)
                    LabeledContent("辅助功能", value: connection.inputPermission.title)
                    LabeledContent("信任的控制端") {
                        HStack {
                            Text("\(connection.trustedDeviceCount) 台")
                            Button("清除信任…") { showForget = true }
                                .disabled(connection.trustedDeviceCount == 0)
                        }
                    }
                    if !connection.hiddenDevices.isEmpty {
                        LabeledContent("已隐藏的局域网设备") {
                            Button("恢复 \(connection.hiddenDevices.count) 台") { connection.restoreHiddenDevices() }
                        }
                    }
                case .keyboard:
                    Toggle("转发实体键盘", isOn: $forwardPhysicalKeyboard)
                    LabeledContent("关闭远控窗口", value: "⌘W")
                    LabeledContent("退出本机 KoKo", value: "⌘Q")
                case .network:
                    LabeledContent("共享监听地址", value: "0.0.0.0 · QUIC / UDP")
                    LabeledContent("共享端口") {
                        HStack {
                            TextField("4444", text: $listenPort)
                                .multilineTextAlignment(.trailing)
                                .frame(width: 78)
                                .disabled(connection.isHosting)
                                .onSubmit(savePort)
                            Button("保存", action: savePort).disabled(!canSavePort)
                        }
                    }
                    LabeledContent("设备工具", value: "SSH · 默认 TCP 22")
                    if connection.isHosting {
                        Text("停止本机共享后可修改监听端口。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                case .advanced:
                    advancedToggle("增强连接", "连接未建立时重试握手。", $enhancedConnections)
                    advancedToggle("流畅度增强", "网络延迟升高时降低自动画质和自适应分辨率。", $adaptiveStreaming)
                    advancedToggle("局域网优先", "先尝试同一子网的地址，再尝试邀请中的其他地址。", $preferLAN)
                    advancedToggle("跨系统键位映射", "Windows 控制此 Mac 时，Ctrl 对应 ⌘，Win 对应 Control，Alt 对应 Option。", $mapWindowsModifiers)
                }
            }
            .formStyle(.grouped)
        }
        .frame(width: 560, height: 520)
        .onChange(of: keepAwake) { _ in KeepAwake.shared.update(hosting: connection.isHosting) }
        .onChange(of: dragSendPanel) { _ in connection.refreshSendPanel() }
        .alert("无法更改开机自启动", isPresented: Binding(get: { loginError != nil }, set: { if !$0 { loginError = nil } })) {
            Button("好") {}
        } message: {
            Text(loginError ?? "")
        }
        .sheet(isPresented: $showPermissions, onDismiss: connection.permissionGuideDidDismiss) { PermissionGuide(connection: connection) }
        .onAppear { listenPort = connection.listenPort }
        .onChange(of: connection.listenPort) { listenPort = $0 }
        .confirmationDialog("清除已信任设备？", isPresented: $showForget) {
            Button("清除信任", role: .destructive) { Task { await connection.forgetTrustedDevices() } }
        } message: {
            Text("当前共享会话会结束，下次连接需要重新确认。")
        }
    }

    private func setLaunchAtLogin(_ enabled: Bool) {
        do {
            try LaunchAtLogin.set(enabled)
        } catch {
            loginError = error.localizedDescription
        }
        launchAtLogin = LaunchAtLogin.isEnabled
        loginNeedsApproval = LaunchAtLogin.needsApproval
    }

    private func advancedToggle(_ title: String, _ hint: String, _ setting: Binding<Bool>) -> some View {
        Toggle(isOn: setting) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                Text(hint).font(.caption).foregroundStyle(.secondary)
            }
        }
        .toggleStyle(.switch)
    }

    private var canSavePort: Bool {
        guard !connection.isHosting, let port = UInt16(listenPort), port > 0 else { return false }
        return listenPort != connection.listenPort
    }

    private func savePort() {
        guard canSavePort else { return }
        connection.listenPort = listenPort
        UserDefaults.standard.set(listenPort, forKey: "listenPort")
    }

    private func chooseDownloadDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.directoryURL = URL(fileURLWithPath: downloadDirectory, isDirectory: true)
        panel.prompt = "选取"
        if panel.runModal() == .OK, let url = panel.url {
            downloadDirectory = url.path
        }
    }
}

struct DeviceCardBackdrop: View {
    var body: some View {
        Canvas { context, size in
            let width = size.width
            let height = size.height
            let background = Path(CGRect(origin: .zero, size: size))
            let ink = Color(red: 0.035, green: 0.11, blue: 0.16)
            let sea = Color(red: 0.09, green: 0.37, blue: 0.38)
            context.fill(background, with: .linearGradient(
                Gradient(colors: [ink, sea]), startPoint: .zero, endPoint: CGPoint(x: width, y: height)
            ))
            for index in 0..<8 {
                let spread = CGFloat(index) * 18
                let oval = CGRect(x: width * 0.45 - spread, y: -height * 0.78 - spread,
                                  width: width * 0.97 + spread * 1.5, height: height * 1.33 + spread * 1.55)
                context.stroke(Path(ellipseIn: oval), with: .color(.white.opacity(0.08)), lineWidth: 1)
            }
            let monitor = CGRect(x: width * 0.66, y: height * 0.18, width: width * 0.26, height: height * 0.53)
            let glass = Path(roundedRect: monitor, cornerRadius: 13)
            context.fill(glass, with: .color(ink.opacity(0.7)))
            context.stroke(glass, with: .color(.white.opacity(0.44)), lineWidth: 1)
            let screen = Path(roundedRect: monitor.insetBy(dx: 9, dy: 9), cornerRadius: 7)
            context.fill(screen, with: .linearGradient(
                Gradient(colors: [sea, ink]), startPoint: CGPoint(x: monitor.minX, y: monitor.maxY),
                endPoint: CGPoint(x: monitor.maxX, y: monitor.minY)
            ))
            var foot = Path()
            foot.move(to: CGPoint(x: monitor.midX, y: monitor.maxY))
            foot.addLine(to: CGPoint(x: monitor.midX, y: monitor.maxY + 22))
            foot.move(to: CGPoint(x: monitor.midX - 23, y: monitor.maxY + 22))
            foot.addLine(to: CGPoint(x: monitor.midX + 23, y: monitor.maxY + 22))
            context.stroke(foot, with: .color(.white.opacity(0.55)), style: StrokeStyle(lineWidth: 2, lineCap: .round))
        }
        .accessibilityHidden(true)
    }
}
