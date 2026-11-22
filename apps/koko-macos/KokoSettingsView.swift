import AppKit
import SwiftUI

let defaultDownloadDirectory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads").path

struct KokoSettingsView: View {
    @ObservedObject var connection: LocalConnection
    @AppStorage("defaultFrameRate") private var defaultFrameRate = 60
    @AppStorage("downloadDirectory") private var downloadDirectory = defaultDownloadDirectory
    @AppStorage("forwardPhysicalKeyboard") private var forwardPhysicalKeyboard = true
    @ViewState private var section = Section.general
    @ViewState private var listenPort = ""
    @ViewState private var showForget = false

    private enum Section: String, CaseIterable, Identifiable {
        case general = "常规", security = "安全", keyboard = "键盘", network = "网络"
        var id: Self { self }
        var icon: String {
            switch self {
            case .general: "gearshape"
            case .security: "lock"
            case .keyboard: "keyboard"
            case .network: "network"
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
                    Picker("新会话的帧率", selection: $defaultFrameRate) {
                        ForEach(LocalConnection.frameRates, id: \.self) { fps in
                            Text("\(fps) FPS").tag(fps)
                        }
                    }
                    LabeledContent("接收文件位置") {
                        VStack(alignment: .trailing, spacing: 6) {
                            HStack {
                                Button("选择文件夹…", action: chooseDownloadDirectory)
                                Button("在访达打开") {
                                    NSWorkspace.shared.open(URL(fileURLWithPath: downloadDirectory, isDirectory: true))
                                }
                            }
                            Text(downloadDirectory)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .help(downloadDirectory)
                        }
                    }
                    LabeledContent("版本", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")
                case .security:
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
                }
            }
            .formStyle(.grouped)
        }
        .frame(width: 550, height: 356)
        .onAppear { listenPort = connection.listenPort }
        .onChange(of: connection.listenPort) { listenPort = $0 }
        .confirmationDialog("清除已信任设备？", isPresented: $showForget) {
            Button("清除信任", role: .destructive) { Task { await connection.forgetTrustedDevices() } }
        } message: {
            Text("当前共享会话会结束，下次连接需要重新确认。")
        }
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
