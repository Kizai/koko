import Combine
import Darwin
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

private struct CommandError: LocalizedError {
    let errorDescription: String?
}

@MainActor
private struct LocalCommand {
    let process = Process()
    private let output = Pipe()
    private let chunks: AsyncStream<Data>
    private let completion: AsyncStream<Int32>

    init(_ role: DeviceRole, arguments: [String]) throws {
        let (chunks, outputEvents) = AsyncStream<Data>.makeStream()
        let (completion, exitEvents) = AsyncStream<Int32>.makeStream()
        self.chunks = chunks
        self.completion = completion

        let tools = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers", isDirectory: true)
        process.executableURL = tools.appendingPathComponent(role.executable)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
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
    @Published var listenAddress = preferredLANAddress()
    @Published private(set) var controllerID = ""
    @Published private(set) var hostID = ""
    @Published private(set) var isChecking = false
    @Published private(set) var isHosting = false
    @Published private(set) var connectionCode = ""
    @Published private(set) var controllerNotice: ConnectionNotice?
    @Published private(set) var hostNotice: ConnectionNotice?

    private let identityDirectory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".koko", isDirectory: true)
    private var hostProcess: Process?
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

    func startHosting(address: String, allowing peer: String) async {
        isHosting = true
        stopRequested = false
        hostNotice = ConnectionNotice(message: "正在启动监听…")
        defer {
            hostProcess = nil
            isHosting = false
            connectionCode = ""
        }

        do {
            let command = try LocalCommand(.host, arguments: [
                "serve", keyPath(for: .host), address, peer.trimmingCharacters(in: .whitespacesAndNewlines),
            ])
            hostProcess = command.process
            let listeningPrefix = "Listening on "
            _ = try await command.result { line in
                guard line.hasPrefix(listeningPrefix), !stopRequested else { return }
                let boundAddress = line.dropFirst(listeningPrefix.count)
                connectionCode = "\(hostID)@\(boundAddress)"
                hostNotice = ConnectionNotice(message: "正在监听 \(boundAddress)")
            }
            hostNotice = ConnectionNotice(message: "监听已停止。")
        } catch {
            hostNotice = stopRequested
                ? ConnectionNotice(message: "监听已停止。")
                : ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    func stopHosting() {
        stopRequested = true
        connectionCode = ""
        hostNotice = ConnectionNotice(message: "正在停止监听…")
        if let hostProcess, hostProcess.isRunning { hostProcess.interrupt() }
    }

    func verify(code: String) async {
        let parts = code.trimmingCharacters(in: .whitespacesAndNewlines)
            .split(separator: "@", maxSplits: 1, omittingEmptySubsequences: false)
        guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else {
            controllerNotice = ConnectionNotice(message: "请粘贴被控端生成的完整连接码。", isError: true)
            return
        }

        isChecking = true
        cancelRequested = false
        controllerNotice = ConnectionNotice(message: "正在验证连接…")
        defer {
            checkProcess = nil
            isChecking = false
        }

        do {
            let command = try LocalCommand(.controller, arguments: [
                "check", keyPath(for: .controller), String(parts[0]), String(parts[1]),
            ])
            checkProcess = command.process
            _ = try await command.result()
            controllerNotice = ConnectionNotice(message: "连接通过，主机已授权此设备。")
        } catch {
            let message = error.localizedDescription
            if cancelRequested {
                controllerNotice = ConnectionNotice(message: "已取消连接验证。")
            } else if message.contains("peer not authorized") {
                controllerNotice = ConnectionNotice(message: "主机未授权此设备。请核对被控端填写的控制端 ID。", isError: true)
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
        if let hostProcess, hostProcess.isRunning { hostProcess.terminate() }
        if let checkProcess, checkProcess.isRunning { checkProcess.terminate() }
    }

    private func keyPath(for role: DeviceRole) -> String {
        identityDirectory.appendingPathComponent(role.keyFilename).path
    }
}

func preferredLANAddress() -> String {
    var interfaces: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&interfaces) == 0 else { return "" }
    defer { freeifaddrs(interfaces) }

    var addresses: [(name: String, ip: String)] = []
    var current = interfaces
    while let interface = current {
        let info = interface.pointee
        current = info.ifa_next
        let name = String(cString: info.ifa_name)
        guard name.hasPrefix("en"), (info.ifa_flags & UInt32(IFF_UP)) != 0,
              let address = info.ifa_addr, Int32(address.pointee.sa_family) == AF_INET else {
            continue
        }

        var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        if getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
            let ip = String(decoding: host.prefix(while: { $0 != 0 }).map(UInt8.init(bitPattern:)), as: UTF8.self)
            if !ip.hasPrefix("169.254.") { addresses.append((name, ip)) }
        }
    }
    return addresses.min(by: { $0.name < $1.name }).map { "\($0.ip):4444" } ?? ""
}
