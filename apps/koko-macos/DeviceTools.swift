import AppKit
import Network
import SwiftUI

struct DeviceToolRequest: Identifiable {
    enum Action: String {
        case files = "文件传输", terminal = "终端", forwarding = "端口转发"
    }

    let id = UUID()
    let action: Action
    let name: String
    let addresses: [String]

    init(_ action: Action, host: NearbyHost, name: String, preferredAddress: String?) {
        self.action = action
        self.name = name
        let advertised = host.endpointAddresses
        var addresses = [String]()
        for endpoint in [preferredAddress].compactMap({ $0 }) + advertised {
            guard let ip = endpoint.split(separator: ":").first.map(String.init),
                  let address = IPv4Address(ip), !address.isLoopback else { continue }
            if !addresses.contains(ip) { addresses.append(ip) }
        }
        self.addresses = addresses
    }
}

private struct SSHEndpoint {
    let account: String
    let address: String
    let port: UInt16

    init?(account: String, address: String, port: String) {
        let account = account.trimmingCharacters(in: .whitespacesAndNewlines)
        guard account.range(of: #"^[A-Za-z_][A-Za-z0-9_.-]*$"#, options: .regularExpression) != nil,
              IPv4Address(address) != nil, let port = UInt16(port), port > 0 else { return nil }
        self.account = account
        self.address = address
        self.port = port
    }

    var destination: String { "\(account)@\(address)" }

    var terminalURL: URL? {
        var url = URLComponents()
        url.scheme = "ssh"
        url.user = account
        url.host = address
        url.port = Int(port)
        return url.url
    }

    var batchOptions: [String] {
        ["-F", "/dev/null", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=8"]
    }
}

struct DeviceToolSheet: View {
    let request: DeviceToolRequest
    @Environment(\.dismiss) private var dismiss
    @AppStorage("downloadDirectory") private var downloadDirectory = defaultDownloadDirectory
    @AppStorage("sshAccount") private var account = NSUserName()
    @ViewState private var address = ""
    @ViewState private var port = "22"
    @ViewState private var sending = true
    @ViewState private var file: URL?
    @ViewState private var remotePath = ""
    @ViewState private var localPort = ""
    @ViewState private var remotePort = ""
    @ViewState private var operation: LocalCommand?
    @ViewState private var notice: ConnectionNotice?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text(request.action.rawValue).font(.title2.weight(.semibold))
                Spacer()
                Text(request.name).foregroundStyle(.secondary).lineLimit(1)
            }
            Form {
                if request.addresses.count > 1 {
                    Picker("设备地址", selection: $address) {
                        ForEach(request.addresses, id: \.self) { Text($0).tag($0) }
                    }
                } else {
                    LabeledContent("设备地址", value: address.isEmpty ? "未发现可用地址" : address)
                }
                TextField("远端账户", text: $account)
                TextField("SSH 端口", text: $port)
                switch request.action {
                case .files:
                    Picker("方向", selection: $sending) {
                        Text("发送到设备").tag(true)
                        Text("接收到本机").tag(false)
                    }
                    .pickerStyle(.segmented)
                    if sending {
                        LabeledContent("本机文件") {
                            Button(file?.lastPathComponent ?? "选择文件…", action: chooseFile)
                                .lineLimit(1)
                        }
                        TextField("远端目录", text: $remotePath, prompt: Text("登录目录"))
                    } else {
                        TextField("远端文件路径", text: $remotePath)
                    }
                case .terminal:
                    EmptyView()
                case .forwarding:
                    TextField("本机端口", text: $localPort)
                    TextField("远端端口", text: $remotePort)
                }
            }
            .disabled(operation != nil)
            Text(hint).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if let notice {
                Text(notice.message)
                    .font(.callout)
                    .foregroundStyle(notice.isError ? Color.red : Color.secondary)
                    .lineLimit(5)
                    .textSelection(.enabled)
            }
            HStack {
                if operation != nil {
                    Button("停止", action: stop)
                }
                Spacer()
                Button("关闭") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(actionTitle, action: start)
                    .keyboardShortcut(.defaultAction)
                    .disabled(endpoint == nil || operation != nil || !validAction)
            }
        }
        .padding(21)
        .frame(width: 490)
        .onAppear { address = request.addresses.first ?? "" }
        .onDisappear(perform: stop)
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in stop() }
    }

    private var endpoint: SSHEndpoint? { SSHEndpoint(account: account, address: address, port: port) }

    private var actionTitle: String {
        switch request.action {
        case .files: sending ? "发送文件" : "选择保存位置…"
        case .terminal: "打开系统终端"
        case .forwarding: "启动转发"
        }
    }

    private var hint: String {
        switch request.action {
        case .files:
            sending
                ? "使用远程登录的 SSH 密钥和已验证的主机指纹。远端目录中的同名文件会被覆盖。"
                : "使用远程登录的 SSH 密钥和已验证的主机指纹。接收完成后保存到选定位置。"
        case .terminal:
            "被控 Mac 需开启「远程登录」并允许该账户访问。主机指纹确认和登录由 macOS 终端处理。"
        case .forwarding:
            "转发两端的 localhost 端口，使用 SSH 密钥和已验证的主机指纹。关闭此窗口会停止转发。"
        }
    }

    private var validAction: Bool {
        switch request.action {
        case .files:
            let path = remotePath.trimmingCharacters(in: .whitespacesAndNewlines)
            return (sending ? file?.lastPathComponent.contains(where: \.isNewline) == false : !path.isEmpty && !path.hasSuffix("/"))
                && !path.contains { $0.isNewline || $0 == "\0" || "*?[]\\".contains($0) }
        case .terminal:
            return true
        case .forwarding:
            return UInt16(localPort).map { $0 > 0 } == true && UInt16(remotePort).map { $0 > 0 } == true
        }
    }

    private func chooseFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK { file = panel.url }
    }

    private func start() {
        guard let endpoint, validAction else { return }
        do {
            switch request.action {
            case .files: try transfer(using: endpoint)
            case .forwarding: try forward(using: endpoint)
            case .terminal:
                guard let url = endpoint.terminalURL,
                      let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else {
                    notice = ConnectionNotice(message: "找不到 macOS 终端。", isError: true)
                    return
                }
                NSWorkspace.shared.open([url], withApplicationAt: terminal,
                                        configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil)
                dismiss()
            }
        } catch {
            notice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    private func transfer(using endpoint: SSHEndpoint) throws {
        let path = remotePath.trimmingCharacters(in: .whitespacesAndNewlines)
        let files = FileManager.default
        if sending, let file {
            let targetDirectory = path.isEmpty ? "./" : (path.hasSuffix("/") ? path : path + "/")
            let staging = try makeTemporaryDirectory()
            let local = staging.appendingPathComponent("upload")
            do {
                try files.createSymbolicLink(at: local, withDestinationURL: file)
            } catch {
                try? files.removeItem(at: staging)
                throw error
            }
            let remote = sftpPath(targetDirectory + ".koko-" + UUID().uuidString + ".part")
            let target = sftpPath(targetDirectory + file.lastPathComponent)
            let batch = "put -- \(sftpPath(local.path)) \(remote)\nrename -- \(remote) \(target)\n"
            let options = endpoint.batchOptions + ["-P", String(endpoint.port), "-b", "-", "--", endpoint.destination]
            try run("/usr/bin/sftp", options, starting: "正在发送文件…", input: Data(batch.utf8), cleanup: staging) {
                "文件已发送。"
            }
        } else if !sending {
            let options = endpoint.batchOptions + ["-s", "-B", "-P", String(endpoint.port), "--"]
            let panel = NSSavePanel()
            panel.directoryURL = URL(fileURLWithPath: downloadDirectory, isDirectory: true)
            panel.nameFieldStringValue = (path as NSString).lastPathComponent
            guard panel.runModal() == .OK, let destination = panel.url else { return }
            let staging = try files.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                        appropriateFor: destination, create: true)
            let temporaryFile = staging.appendingPathComponent("download")
            let shouldReplace = files.fileExists(atPath: destination.path)
            try run("/usr/bin/scp", options + ["\(endpoint.destination):\(path)", temporaryFile.path],
                    starting: "正在接收文件…", cleanup: staging) {
                if shouldReplace, files.fileExists(atPath: destination.path) {
                    _ = try files.replaceItemAt(destination, withItemAt: temporaryFile)
                } else {
                    try files.moveItem(at: temporaryFile, to: destination)
                }
                return "已接收：\(destination.lastPathComponent)"
            }
        }
    }

    private func sftpPath(_ path: String) -> String {
        let escaped = path.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }

    private func makeTemporaryDirectory() throws -> URL {
        let files = FileManager.default
        let directory = files.temporaryDirectory.appendingPathComponent("ks-" + UUID().uuidString.prefix(8), isDirectory: true)
        try files.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        return directory
    }

    private func forward(using endpoint: SSHEndpoint) throws {
        let files = FileManager.default
        let directory = try makeTemporaryDirectory()
        let socket = directory.appendingPathComponent("s")
        let options = endpoint.batchOptions + [
            "-N", "-T", "-M", "-S", socket.path,
            "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=15",
            "-L", "127.0.0.1:\(localPort):127.0.0.1:\(remotePort)", "-p", String(endpoint.port), endpoint.destination,
        ]
        let command = try run("/usr/bin/ssh", options, starting: "正在建立转发…", cleanup: directory) { "端口转发已停止。" }
        Task {
            while command.process.isRunning, operation?.process === command.process {
                if files.fileExists(atPath: socket.path) {
                    notice = ConnectionNotice(message: "转发已启动 · 127.0.0.1:\(localPort)")
                    break
                }
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }

    @discardableResult
    private func run(_ executable: String, _ arguments: [String], starting: String, input: Data? = nil, cleanup: URL? = nil,
                     finish: @escaping () throws -> String) throws -> LocalCommand {
        let command: LocalCommand
        do {
            command = try LocalCommand(executable: URL(fileURLWithPath: executable), arguments: arguments,
                                       interactive: input != nil)
        } catch {
            if let cleanup { try? FileManager.default.removeItem(at: cleanup) }
            throw error
        }
        operation = command
        notice = ConnectionNotice(message: starting)
        Task {
            defer {
                if let cleanup { try? FileManager.default.removeItem(at: cleanup) }
            }
            do {
                if let input { try await command.writeRequest(input) }
                _ = try await command.result()
                guard operation?.process === command.process else { return }
                notice = ConnectionNotice(message: try finish())
            } catch {
                guard operation?.process === command.process else { return }
                notice = ConnectionNotice(message: error.localizedDescription, isError: true)
            }
            if operation?.process === command.process { operation = nil }
        }
        return command
    }

    private func stop() {
        guard let command = operation else { return }
        operation = nil
        if command.process.isRunning { command.process.terminate() }
        notice = ConnectionNotice(message: "已停止操作。")
    }
}
