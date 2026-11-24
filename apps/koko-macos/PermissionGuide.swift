import AppKit
import SwiftUI

enum DesktopPermissionHistory {
    static func record(version: String, screen: Bool, input: Bool, in defaults: UserDefaults) -> Bool {
        let key = "desktopPermissionHistory"
        let previous = defaults.dictionary(forKey: key)
        defaults.set(["version": version, "screen": screen, "input": input], forKey: key)
        guard let oldVersion = previous?["version"] as? String,
              version.compare(oldVersion, options: .numeric) == .orderedDescending else { return false }
        return (previous?["screen"] as? Bool == true && !screen) || (previous?["input"] as? Bool == true && !input)
    }
}

enum ReceiveFolderAccess: Equatable {
    case unchecked, checking, writable, denied, failed(String)

    var title: String {
        switch self {
        case .unchecked: "按需检查"
        case .checking: "检查中…"
        case .writable: "此目录可写"
        case .denied: "访问被拒绝"
        case .failed: "检查失败"
        }
    }

    static func check(_ path: String) async -> Self {
        let probe = URL(fileURLWithPath: path, isDirectory: true)
            .appendingPathComponent(".Koko-" + UUID().uuidString, isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: probe, withIntermediateDirectories: false,
                                                    attributes: [.posixPermissions: 0o700])
            try FileManager.default.removeItem(at: probe)
            return .writable
        } catch {
            return ClipboardError.from(error) == .permission ? .denied : .failed(error.localizedDescription)
        }
    }
}

struct PermissionGuide: View {
    enum Context: Identifiable {
        case firstLaunch, settings, sharing, upgrade
        var id: Self { self }
    }

    enum Progress: String {
        case introduction, configuring, finished, skipped
    }

    private enum Item: String, CaseIterable, Identifiable {
        case screen = "屏幕录制", input = "辅助功能", notifications = "消息通知", files = "文件与文件夹"
        var id: Self { self }

        var icon: String {
            switch self {
            case .screen: "display"
            case .input: "accessibility"
            case .notifications: "bell.badge.fill"
            case .files: "folder.fill"
            }
        }

        var color: Color {
            switch self {
            case .screen: .purple
            case .input: .blue
            case .notifications: .red
            case .files: .orange
            }
        }

        var purpose: String {
            switch self {
            case .screen: "让对方看到这台 Mac 的屏幕"
            case .input: "让对方使用键盘和鼠标操作这台 Mac"
            case .notifications: "后台运行时在右上角提醒远程请求"
            case .files: "收发文件时访问相应文件夹"
            }
        }

        var settingsPane: String {
            switch self {
            case .screen: "Privacy_ScreenCapture"
            case .input: "Privacy_Accessibility"
            case .notifications: "Notifications"
            case .files: "Privacy_FilesAndFolders"
            }
        }
    }

    @ObservedObject var connection: LocalConnection
    let context: Context
    @Environment(\.dismiss) private var dismiss
    @AppStorage("downloadDirectory") private var downloadDirectory = defaultDownloadDirectory
    @AppStorage("permissionGuideProgress") private var progress = Progress.introduction
    private var showsIntroduction: Bool { context == .firstLaunch && progress == .introduction }
    @ViewState private var selected: Item = .screen
    @ViewState private var confirmRestart = false
    @ViewState private var folderAccess: ReceiveFolderAccess = .unchecked

    init(connection: LocalConnection, context: Context = .settings) {
        self.connection = connection
        self.context = context
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            if showsIntroduction {
                Text("设置 Koko 的使用权限").font(.title2.bold())
                Text("如果要让其他设备协助操作这台 Mac，请先检查以下权限。只用这台 Mac 控制其他设备时，可以跳过。")
                    .foregroundStyle(.secondary)
                VStack(spacing: 14) {
                    ForEach(Item.allCases) { item in
                        HStack(spacing: 12) {
                            icon(for: item)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(item.rawValue).font(.headline)
                                Text(item.purpose).font(.callout).foregroundStyle(.secondary)
                            }
                            Spacer()
                        }
                    }
                }
                .padding(.vertical, 6)
            } else {
                Text(context == .sharing ? "准备共享此 Mac" : "安全与隐私").font(.title2.bold())
                if context == .upgrade {
                    Text("升级后，先前允许的权限已有变化。请检查未授权的项目。").foregroundStyle(.secondary)
                }
                HStack(alignment: .top, spacing: 16) {
                    instructions
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Divider()
                    VStack(spacing: 6) {
                        ForEach(Item.allCases) { item in
                            itemRow(item)
                        }
                    }
                    .frame(width: 238)
                }
                .frame(minHeight: 226)
            }
            Divider()
            HStack {
                if context == .firstLaunch && !showsIntroduction {
                    Button("返回") { progress = .introduction }
                }
                Spacer()
                if showsIntroduction {
                    Button("暂时跳过") { progress = .skipped; dismiss() }.keyboardShortcut(.cancelAction)
                    Button("去开启") { progress = .configuring }.buttonStyle(.borderedProminent)
                } else if context == .sharing {
                    Button("取消") { dismiss() }.keyboardShortcut(.cancelAction)
                    Button(connection.screenPermission == .granted && connection.inputPermission == .granted
                           ? "开启共享" : "暂按现有权限共享") {
                        dismiss()
                        Task { await connection.startHosting() }
                    }
                    .buttonStyle(.borderedProminent)
                } else {
                    Button("完成") { finish() }
                        .keyboardShortcut(.cancelAction)
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .padding(24)
        .frame(width: showsIntroduction ? 480 : 630)
        .fixedSize(horizontal: false, vertical: true)
        .interactiveDismissDisabled(context == .firstLaunch)
        .task {
            connection.refreshDesktopPermissions()
            if selected == .screen, connection.screenPermission == .granted {
                selected = connection.inputPermission == .granted ? .files : .input
            }
        }
        .task(id: showsIntroduction) {
            guard !showsIntroduction else { return }
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(1)) } catch { return }
                connection.refreshDesktopPermissions()
            }
        }
        .alert("结束共享和当前连接并重开 Koko？", isPresented: $confirmRestart) {
            Button("取消", role: .cancel) {}
            Button("重开并复查", action: restart)
        } message: {
            Text("重开后会回到权限页，你可以在那里复查授权。")
        }
        .onChange(of: downloadDirectory) { _ in folderAccess = .unchecked }
    }

    private func finish() {
        if context == .firstLaunch { progress = .finished }
        dismiss()
    }

    private func restart() {
        connection.permissionRestartRequested = true
        dismiss()
    }

    private var instructions: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(selected.rawValue, systemImage: selected.icon).font(.headline)
            switch selected {
            case .screen:
                Text("点击右侧「开启」，在系统提示中允许屏幕录制。也可以在「隐私与安全性 → 录屏与系统录音」中打开 Koko。")
                Text("此页会自动检查。若系统要求重启，或开启后仍显示未授权，可在这里重开 Koko。")
            case .input:
                Text("点击右侧「开启」，按系统提示在「辅助功能」或「设备控制和数据访问」中启用 Koko。")
                Text("此页会自动检查。若已允许辅助功能但仍无法控制键鼠，再点一次「开启」以完成系统请求。")
            case .notifications:
                Text("点击右侧「开启」，允许 Koko 发送系统通知。在「系统设置 → 通知 → Koko」中建议选择「横幅」或「提醒」。")
                Text("后台或菜单栏运行期间收到远程请求时，屏幕右上角会同时弹出可操作提醒。")
            case .files:
                Text("macOS 按文件夹管理访问。点击「检查」会在当前接收位置创建并移除一个临时文件夹。")
                Text(downloadDirectory).font(.caption).textSelection(.enabled)
                if case .failed(let reason) = folderAccess {
                    Text(reason)
                } else if folderAccess == .denied {
                    Text("请在系统设置中允许访问该文件夹，或在「设置 → 常规」更改接收位置。")
                }
            }
            HStack(spacing: 10) {
                Button("打开系统设置") {
                    let pane = selected == .notifications
                        ? "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
                        : "x-apple.systempreferences:com.apple.preference.security?" + selected.settingsPane
                    if let url = URL(string: pane), NSWorkspace.shared.open(url) { return }
                    if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.systempreferences") {
                        NSWorkspace.shared.open(url)
                    }
                }
                if selected != .files {
                    Button("重新检查") { connection.refreshDesktopPermissions() }
                }
            }
            if (selected == .screen && connection.screenPermission == .required)
                || (selected == .input && connection.inputPermission == .required) {
                Button("重开 Koko 并复查") {
                    if connection.isHosting || connection.session != nil || connection.isChecking {
                        confirmRestart = true
                    } else {
                        restart()
                    }
                }
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func icon(for item: Item) -> some View {
        Image(systemName: item.icon)
            .font(.system(size: 15, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 30, height: 30)
            .background(item.color, in: RoundedRectangle(cornerRadius: 7))
            .accessibilityHidden(true)
    }

    private func itemRow(_ item: Item) -> some View {
        let status: String
        let checking: Bool
        let granted: Bool
        switch item {
        case .screen:
            status = connection.screenPermission.title
            checking = connection.isCheckingScreenPermission
            granted = connection.screenPermission == .granted
        case .input:
            status = connection.inputPermission.title
            checking = connection.isCheckingInputPermission
            granted = connection.inputPermission == .granted
        case .notifications:
            status = connection.notificationPermission.title
            checking = connection.isCheckingNotificationPermission
            granted = connection.notificationPermission == .granted
        case .files:
            status = folderAccess.title
            checking = folderAccess == .checking
            granted = folderAccess == .writable
        }
        return HStack(spacing: 6) {
            Button { selected = item } label: {
                HStack(spacing: 9) {
                    icon(for: item)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.rawValue).font(.headline)
                        Text(checking ? "检查中…" : status).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if granted {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .accessibilityLabel(status)
            }
            if !granted || item == .files {
                Button(item == .files ? "检查" : "开启") {
                    selected = item
                    Task {
                        switch item {
                        case .screen: await connection.requestScreenPermission()
                        case .input: await connection.requestInputPermission()
                        case .notifications: await connection.requestNotificationPermission()
                        case .files:
                            let path = downloadDirectory
                            folderAccess = .checking
                            let result = await ReceiveFolderAccess.check(path)
                            if downloadDirectory == path { folderAccess = result }
                        }
                    }
                }
                .disabled(checking)
                .accessibilityLabel("\(item == .files ? "检查" : "开启")\(item.rawValue)")
            }
        }
        .padding(8)
        .background(selected == item ? Color.accentColor.opacity(0.1) : .clear,
                    in: RoundedRectangle(cornerRadius: 8))
    }
}

enum PermissionRelaunch {
    static let argument = "--review-desktop-permissions"

    static func schedule() throws -> Process {
        let waiter = Process()
        waiter.executableURL = URL(fileURLWithPath: "/bin/sh")
        // Wait for normal termination and session cleanup before opening this exact bundle again.
        waiter.arguments = ["-c", """
            while kill -0 "$1" 2>/dev/null; do /bin/sleep 0.2; done
            exec /usr/bin/open -n "$2" --args "$3"
            """, "koko-reopen", String(ProcessInfo.processInfo.processIdentifier), Bundle.main.bundlePath, argument]
        // A relaunched app must not keep the previous caller's pipes open.
        waiter.standardInput = FileHandle.nullDevice
        waiter.standardOutput = FileHandle.nullDevice
        waiter.standardError = FileHandle.nullDevice
        try waiter.run()
        return waiter
    }
}
