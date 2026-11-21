import Combine
import Foundation

enum DeviceRole: String, CaseIterable, Identifiable {
    case controller = "控制端"
    case host = "被控端"

    var id: Self { self }
    var executable: String { self == .host ? "koko-daemon" : "koko-agent" }
    var keyFilename: String { self == .host ? "host.key" : "controller.key" }
}

struct ConnectionNotice {
    let message: String
    var isError = false
}

enum ScreenPermission {
    case unknown, required, granted, failed
}

struct PendingPair {
    let number: String
    let fingerprint: String
}

private struct CommandError: LocalizedError {
    let errorDescription: String?
}

@MainActor
private struct LocalCommand {
    let process = Process()
    private let output = Pipe()
    private let input: Pipe?
    private let chunks: AsyncStream<Data>
    private let completion: AsyncStream<Int32>

    init(_ role: DeviceRole, arguments: [String], interactive: Bool = false) throws {
        let (chunks, outputEvents) = AsyncStream<Data>.makeStream()
        let (completion, exitEvents) = AsyncStream<Int32>.makeStream()
        self.chunks = chunks
        self.completion = completion

        let tools = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers", isDirectory: true)
        process.executableURL = tools.appendingPathComponent(role.executable)
        process.arguments = arguments
        input = interactive ? Pipe() : nil
        if let input {
            process.standardInput = input
        } else {
            process.standardInput = FileHandle.nullDevice
        }
        process.standardOutput = output
        process.standardError = output
        process.terminationHandler = { process in
            exitEvents.yield(process.terminationStatus)
            exitEvents.finish()
        }
        try process.run()
        output.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                outputEvents.finish()
            } else {
                outputEvents.yield(data)
            }
        }
    }

    func send(_ line: String) throws {
        try input?.fileHandleForWriting.write(contentsOf: Data((line + "\n").utf8))
    }

    func result(onLine: (String) -> Void = { _ in }) async throws -> String {
        var pending = Data()
        var lines: [String] = []
        for await chunk in chunks {
            pending.append(chunk)
            while let newline = pending.firstIndex(of: 0x0a) {
                let line = String(decoding: pending[..<newline], as: UTF8.self)
                pending.removeSubrange(...newline)
                lines.append(line)
                onLine(line)
            }
        }
        if !pending.isEmpty {
            let line = String(decoding: pending, as: UTF8.self)
            lines.append(line)
            onLine(line)
        }

        var exitCodes = completion.makeAsyncIterator()
        let exitCode = await exitCodes.next()
        let message = lines.joined(separator: "\n")
        guard exitCode == 0 else {
            throw CommandError(errorDescription: message.isEmpty ? "程序已退出。" : message)
        }
        return message
    }
}

@MainActor
final class LocalConnection: ObservableObject {
    @Published var listenPort = "4444"
    @Published var inviteInput = ""
    @Published private(set) var controllerID = ""
    @Published private(set) var hostID = ""
    @Published private(set) var isChecking = false
    @Published private(set) var isHosting = false
    @Published private(set) var connectionCode = ""
    @Published private(set) var directInvite = ""
    @Published private(set) var pendingPair: PendingPair?
    @Published private(set) var controllerNotice: ConnectionNotice?
    @Published private(set) var hostNotice: ConnectionNotice?
    @Published private(set) var screenPermission = ScreenPermission.unknown
    @Published private(set) var isCheckingScreenPermission = false

    private let identityDirectory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".koko", isDirectory: true)
    private var hostCommand: LocalCommand?
    private var checkProcess: Process?
    private var stopRequested = false
    private var cancelRequested = false

    func prepareIdentity(for role: DeviceRole) async {
        if role == .controller ? !controllerID.isEmpty : !hostID.isEmpty { return }

        do {
            try FileManager.default.createDirectory(at: identityDirectory, withIntermediateDirectories: true)
            let command = try LocalCommand(role, arguments: ["identity", keyPath(for: role)])
            let id = try await command.result()
            switch role {
            case .controller: controllerID = id
            case .host: hostID = id
            }
        } catch {
            let notice = ConnectionNotice(message: error.localizedDescription, isError: true)
            switch role {
            case .controller: controllerNotice = notice
            case .host: hostNotice = notice
            }
        }
    }

    func refreshScreenPermission(request: Bool = false) async {
        guard !isCheckingScreenPermission else { return }
        isCheckingScreenPermission = true
        defer { isCheckingScreenPermission = false }

        do {
            let command = try LocalCommand(.host, arguments: ["screen-permission", request ? "request" : "check"])
            let result = try await command.result()
            switch result.split(separator: "\n").last {
            case "granted": screenPermission = .granted
            case "required": screenPermission = .required
            default: screenPermission = .failed
            }
        } catch {
            screenPermission = .failed
        }
    }

    func startHosting() async {
        isHosting = true
        stopRequested = false
        hostNotice = ConnectionNotice(message: "正在启动监听…")
        defer {
            hostCommand = nil
            isHosting = false
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
        }

        do {
            let address = "0.0.0.0:\(listenPort.trimmingCharacters(in: .whitespacesAndNewlines))"
            let command = try LocalCommand(.host, arguments: [
                "serve-pair", keyPath(for: .host), address,
            ], interactive: true)
            hostCommand = command
            _ = try await command.result(onLine: updatePairingStatus)
            hostNotice = ConnectionNotice(message: "监听已停止。")
        } catch {
            hostNotice = stopRequested
                ? ConnectionNotice(message: "监听已停止。")
                : ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    private func updatePairingStatus(_ line: String) {
        guard !stopRequested else { return }
        switch line {
        case "Pair approved":
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "已授权本次会话。停止监听即可断开。")
        case "Pair declined", "Pair request ended":
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "配对未完成，仍可使用当前连接码。")
        case "Pair code expired":
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "连接码已过期，重新启动监听可生成新码。")
        default:
            if line.hasPrefix("Pair code: ") {
                connectionCode = String(line.dropFirst("Pair code: ".count))
                hostNotice = ConnectionNotice(message: "连接码 5 分钟内有效，授权一次后失效。")
            } else if line.hasPrefix("Pair invite: ") {
                directInvite = String(line.dropFirst("Pair invite: ".count))
            } else if line.hasPrefix("Pair request: ") {
                let fields = line.dropFirst("Pair request: ".count).split(separator: " ", maxSplits: 1)
                if fields.count == 2 {
                    pendingPair = PendingPair(number: String(fields[0]), fingerprint: String(fields[1]))
                }
            }
        }
    }

    func respondToPairing(_ request: PendingPair, allow: Bool) {
        do {
            try hostCommand?.send("\(allow ? "approve" : "decline") \(request.number)")
            pendingPair = nil
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    func stopHosting() {
        stopRequested = true
        connectionCode = ""
        directInvite = ""
        pendingPair = nil
        hostNotice = ConnectionNotice(message: "正在停止监听…")
        if let process = hostCommand?.process, process.isRunning { process.interrupt() }
    }

    func verify(code: String) async {
        isChecking = true
        cancelRequested = false
        controllerNotice = ConnectionNotice(message: "正在查找被控端，请在对方设备上确认请求。")
        defer {
            checkProcess = nil
            isChecking = false
        }

        do {
            let command = try LocalCommand(.controller, arguments: [
                "pair", keyPath(for: .controller), code,
            ])
            checkProcess = command.process
            _ = try await command.result()
            controllerNotice = ConnectionNotice(message: "配对完成，被控端已授权本次会话。")
        } catch {
            let message = error.localizedDescription
            if cancelRequested {
                controllerNotice = ConnectionNotice(message: "已取消配对。")
            } else if message.contains("Koko host not found") {
                controllerNotice = ConnectionNotice(message: "找不到被控端。请核对连接码。通过组网连接时，可以改用被控端的直连邀请。", isError: true)
            } else if message.contains("direct invite") {
                controllerNotice = ConnectionNotice(message: "直连邀请格式无效，请在被控端重新复制。", isError: true)
            } else if message.contains("pairing declined or expired") || message.contains("pairing unavailable") {
                controllerNotice = ConnectionNotice(message: "请求被拒绝、已过期或被控端正在处理另一请求。", isError: true)
            } else if message.contains("connection code") {
                controllerNotice = ConnectionNotice(message: "请输入被控端显示的 10 位连接码。", isError: true)
            } else {
                controllerNotice = ConnectionNotice(message: message, isError: true)
            }
        }
    }

    func cancelCheck() {
        cancelRequested = true
        if let checkProcess, checkProcess.isRunning { checkProcess.terminate() }
    }

    func stopForQuit() {
        if let process = hostCommand?.process, process.isRunning { process.terminate() }
        if let checkProcess, checkProcess.isRunning { checkProcess.terminate() }
    }

    private func keyPath(for role: DeviceRole) -> String {
        identityDirectory.appendingPathComponent(role.keyFilename).path
    }
}
