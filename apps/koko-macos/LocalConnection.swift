import AppKit
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

struct PairedHost: Decodable, Equatable {
    let hostID: String
    let address: String

    enum CodingKeys: String, CodingKey {
        case hostID = "host_id"
        case address
    }
}

struct NearbyHost: Decodable, Identifiable {
    let id: String
    let label: String
    let request: String
}

struct RemoteDisplay: Codable, Equatable, Identifiable {
    let id: UInt32
    let name: String
    let width: Int
    let height: Int

    func dimensions(edge: Int) -> String {
        let scale = min(1, Double(edge) / Double(max(width, height)))
        return "\(max(1, Int(Double(width) * scale))) × \(max(1, Int(Double(height) * scale)))"
    }
}

enum FrameResolution: Int, CaseIterable, Identifiable {
    case automatic = 0, compact = 1280, standard = 1920, large = 2560, original = 4096

    var id: Self { self }

    func title(for display: RemoteDisplay?) -> String {
        switch self {
        case .automatic: "自适应"
        case .original: "原始尺寸（最大 4096 px）"
        default: display?.dimensions(edge: rawValue) ?? "\(rawValue) px（长边）"
        }
    }
}

enum FrameQuality: Int, CaseIterable, Identifiable {
    case automatic = 0, fast = 45, balanced = 70, detailed = 90

    var id: Self { self }

    var title: String {
        switch self {
        case .automatic: "自动"
        case .fast: "流畅"
        case .balanced: "均衡"
        case .detailed: "清晰"
        }
    }
}

struct RemoteAction: Codable {
    let action: String
    var x: Int? = nil
    var y: Int? = nil
    var key: String? = nil
    var text: String? = nil
    var display: UInt32? = nil
    var edge: Int? = nil
    var quality: Int? = nil
    var events: [RemotePointerEvent]? = nil
}

private struct RemoteReply: Codable {
    let status: String
    var png: String? = nil
    var code: String? = nil
    var image: String? = nil
    var hostName: String? = nil
    var displays: [RemoteDisplay]? = nil

    enum CodingKeys: String, CodingKey {
        case status, png, code, image, displays
        case hostName = "host_name"
    }
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

    func writeRequest(_ data: Data) throws {
        try input?.fileHandleForWriting.write(contentsOf: data)
        try input?.fileHandleForWriting.close()
    }

    func finishInput() {
        try? input?.fileHandleForWriting.close()
    }

    func result(collectOutput: Bool = true, onLine: (String) -> Void = { _ in }) async throws -> String {
        var pending = Data()
        var lines: [String] = []
        for await chunk in chunks {
            pending.append(chunk)
            while let newline = pending.firstIndex(of: 0x0a) {
                let line = String(decoding: pending[..<newline], as: UTF8.self)
                pending.removeSubrange(...newline)
                if collectOutput { lines.append(line) } else { lines = [line] }
                onLine(line)
            }
        }
        if !pending.isEmpty {
            let line = String(decoding: pending, as: UTF8.self)
            if collectOutput { lines.append(line) } else { lines = [line] }
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
private final class RemoteChannel {
    private let command: LocalCommand
    private var pending: CheckedContinuation<RemoteReply, Error>?
    private var termination: Error?

    init(keyPath: String, host: PairedHost) throws {
        let command = try LocalCommand(.controller, arguments: [
            "remote-stream", keyPath, host.hostID, host.address,
        ], interactive: true)
        self.command = command
        Task { [weak self] in
            do {
                _ = try await command.result(collectOutput: false) { line in
                    self?.receive(line)
                }
                self?.finish(CommandError(errorDescription: "连接已经关闭。"))
            } catch {
                self?.finish(error)
            }
        }
    }

    func send(_ action: RemoteAction) async throws -> RemoteReply {
        try Task.checkCancellation()
        if let termination { throw termination }
        let request = try JSONEncoder().encode(action)
        try command.send(String(decoding: request, as: UTF8.self))
        return try await withCheckedThrowingContinuation { pending = $0 }
    }

    private func receive(_ line: String) {
        guard let reply = pending else { return }
        pending = nil
        if let response = try? JSONDecoder().decode(RemoteReply.self, from: Data(line.utf8)) {
            reply.resume(returning: response)
        } else {
            reply.resume(throwing: CommandError(errorDescription: line))
        }
    }

    private func finish(_ error: Error) {
        termination = error
        let reply = pending
        pending = nil
        reply?.resume(throwing: error)
    }

    func close() {
        command.finishInput()
        finish(CancellationError())
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
    @Published private(set) var isSharingWithPeer = false
    @Published private(set) var connectionCode = ""
    @Published private(set) var directInvite = ""
    @Published private(set) var pendingPair: PendingPair?
    @Published private(set) var controllerNotice: ConnectionNotice?
    @Published private(set) var hostNotice: ConnectionNotice?
    @Published private(set) var screenPermission = ScreenPermission.unknown
    @Published private(set) var isCheckingScreenPermission = false
    @Published private(set) var inputPermission = ScreenPermission.unknown
    @Published private(set) var isCheckingInputPermission = false
    @Published var showsHostPermissionGuide = false
    @Published private(set) var isScanning = false
    @Published private(set) var nearbyHosts: [NearbyHost] = []
    @Published private(set) var session: PairedHost?
    @Published private(set) var remoteFrame: NSImage?
    @Published private(set) var remoteHostName = ""
    @Published private(set) var displays: [RemoteDisplay] = []
    @Published private(set) var selectedDisplay: RemoteDisplay?
    @Published var frameQuality = FrameQuality.automatic
    @Published var frameResolution = FrameResolution.automatic
    @Published var showTextControls = false
    @Published private(set) var screenNotice: ConnectionNotice?
    @Published private(set) var inputNotice: ConnectionNotice?
    @Published private(set) var isSendingInput = false
    @Published var remoteText = ""
    let localHostName = Host.current().localizedName ?? "此 Mac"

    private let identityDirectory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".koko", isDirectory: true)
    private var hostCommand: LocalCommand?
    private var isCapturingScreen = false
    private var checkProcess: Process?
    private let hostPointer = DesktopPointer()
    private var frameChannel: RemoteChannel?
    private var pointerChannel: RemoteChannel?
    private var pointerQueue: [(display: UInt32, event: RemotePointerEvent)] = []
    private var pointerTask: Task<Void, Never>?
    private var displayRefresh = Date.distantPast
    private var captureTime = 0.18
    private var viewportEdge = 1600
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

        screenPermission = hasScreenCapturePermission(request: request) ? .granted : .required
    }

    func refreshInputPermission(request: Bool = false) async {
        guard !isCheckingInputPermission else { return }
        isCheckingInputPermission = true
        defer { isCheckingInputPermission = false }
        inputPermission = hasInputPermission(request: request) ? .granted : .required
    }

    func startHosting() async {
        isHosting = true
        stopRequested = false
        hostNotice = ConnectionNotice(message: "正在启动监听…")
        defer {
            hostCommand = nil
            isHosting = false
            isSharingWithPeer = false
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            hostPointer.release()
        }

        do {
            let address = "0.0.0.0:\(listenPort.trimmingCharacters(in: .whitespacesAndNewlines))"
            let command = try LocalCommand(.host, arguments: [
                "serve-pair", keyPath(for: .host), address, "--app-desktop",
            ], interactive: true)
            hostCommand = command
            _ = try await command.result(collectOutput: false, onLine: updatePairingStatus)
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
        case "Pointer closed":
            hostPointer.release()
        case "Pair approved":
            isSharingWithPeer = true
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "远程协助已授权，控制端可查看与操作。")
        case "Session closed":
            isSharingWithPeer = false
            hostPointer.release()
            hostNotice = ConnectionNotice(message: "远程会话已结束。重启远程协助可发新授权码。")
        case "Pair declined", "Pair request ended":
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "配对未完成，仍可使用当前连接码。")
        case "Pair code expired":
            connectionCode = ""
            directInvite = ""
            hostNotice = ConnectionNotice(message: "授权码已过期，仍可通过局域网搜索申请控制。")
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
            } else if line.hasPrefix("Desktop request: "), let command = hostCommand {
                let fields = line.dropFirst("Desktop request: ".count).split(separator: " ", maxSplits: 1)
                if fields.count == 2, let request = UInt64(fields[0]),
                   let action = try? JSONDecoder().decode(RemoteAction.self, from: Data(fields[1].utf8)) {
                    Task { await respondToDesktopRequest(request, action: action, command: command) }
                }
            }
        }
    }

    private func respondToDesktopRequest(_ request: UInt64, action: RemoteAction, command: LocalCommand) async {
        guard !stopRequested, hostCommand?.process === command.process else { return }
        guard isSharingWithPeer else { return }
        let reply = await executeDesktopAction(action)
        guard !stopRequested, hostCommand?.process === command.process else { return }
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = .withoutEscapingSlashes
            let json = try encoder.encode(reply)
            try command.send("desktop-reply \(request) \(String(decoding: json, as: UTF8.self))")
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    private func executeDesktopAction(_ action: RemoteAction) async -> RemoteReply {
        do {
            switch action.action {
            case "displays":
                let main = CGMainDisplayID()
                let displays: [RemoteDisplay] = NSScreen.screens.compactMap { screen in
                    let key = NSDeviceDescriptionKey("NSScreenNumber")
                    guard let id = (screen.deviceDescription[key] as? NSNumber)?.uint32Value,
                          let mode = CGDisplayCopyDisplayMode(id) else { return nil }
                    return RemoteDisplay(id: id, name: screen.localizedName, width: mode.pixelWidth, height: mode.pixelHeight)
                }
                let ordered = displays.filter { $0.id == main } + displays.filter { $0.id != main }
                return RemoteReply(status: "displays", hostName: localHostName, displays: ordered)
            case "screenshot", "capture":
                guard !isCapturingScreen else { return RemoteReply(status: "error", code: "capture_busy") }
                isCapturingScreen = true
                defer { isCapturingScreen = false }
                let bytes = try await captureScreen(displayID: action.display, edge: action.edge ?? 1600, quality: action.quality)
                screenPermission = .granted
                let image = bytes.base64EncodedString()
                return action.action == "screenshot"
                    ? RemoteReply(status: "frame", png: image)
                    : RemoteReply(status: "image", image: image)
            case "pointer":
                guard let display = action.display, let events = action.events else { throw DesktopError.invalidCommand }
                try hostPointer.apply(events, display: display)
            case "click", "key", "text":
                guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
                switch action.action {
                case "click":
                    guard let x = action.x, let y = action.y else { throw DesktopError.invalidCommand }
                    try click(Double(x), Double(y))
                case "key":
                    guard let key = action.key else { throw DesktopError.invalidCommand }
                    try postRemoteKey(key)
                default:
                    guard let text = action.text else { throw DesktopError.invalidCommand }
                    try postRemoteText(text)
                }
            default:
                throw DesktopError.invalidCommand
            }
            return RemoteReply(status: "ok")
        } catch let error as DesktopError {
            if error == .inputPermissionRequired { inputPermission = .required }
            return RemoteReply(status: "error", code: error.rawValue)
        } catch let error as ScreenCaptureError {
            if error == .permissionRequired { screenPermission = .required }
            return RemoteReply(status: "error", code: error.rawValue)
        } catch {
            return RemoteReply(status: "error", code: "capture_failed")
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
        isSharingWithPeer = false
        hostPointer.release()
        connectionCode = ""
        directInvite = ""
        pendingPair = nil
        hostNotice = ConnectionNotice(message: "正在停止监听…")
        if let process = hostCommand?.process, process.isRunning { process.interrupt() }
    }

    func verify(code: String) async {
        isChecking = true
        cancelRequested = false
        controllerNotice = ConnectionNotice(message: code.hasPrefix("koko://request/")
            ? "等待被控端同意远程控制…" : "正在使用授权码连接…")
        defer {
            checkProcess = nil
            isChecking = false
        }

        do {
            let command = try LocalCommand(.controller, arguments: [
                "pair", keyPath(for: .controller), code,
            ])
            checkProcess = command.process
            let reply = try await command.result()
            session = try JSONDecoder().decode(PairedHost.self, from: Data(reply.utf8))
            controllerNotice = nil
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

    func scanLAN() async {
        isScanning = true
        defer { isScanning = false }
        do {
            let command = try LocalCommand(.controller, arguments: ["discover"])
            let result = try await command.result()
            nearbyHosts = try JSONDecoder().decode([NearbyHost].self, from: Data(result.utf8))
                .filter { $0.id != hostID }
            controllerNotice = nearbyHosts.isEmpty
                ? ConnectionNotice(message: "未发现可连接的 Koko 设备。") : nil
        } catch {
            controllerNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    func watchRemoteScreen() async {
        guard let session else { return }
        while !Task.isCancelled && self.session == session {
            let changed = await refreshFrame(session)
            try? await Task.sleep(for: changed ? .milliseconds(90) : .seconds(1))
        }
    }

    private func refreshFrame(_ session: PairedHost) async -> Bool {
        do {
            if frameChannel == nil { frameChannel = try RemoteChannel(keyPath: keyPath(for: .controller), host: session) }
            guard let channel = frameChannel else { return false }
            if displays.isEmpty || Date().timeIntervalSince(displayRefresh) >= 4 {
                let reply = try await channel.send(.init(action: "displays"))
                guard frameChannel === channel else { return false }
                guard let displays = reply.displays, !displays.isEmpty else {
                    screenNotice = remoteError(reply.code ?? "display_unavailable")
                    return false
                }
                if self.displays != displays { self.displays = displays }
                if let selectedDisplay, let current = displays.first(where: { $0.id == selectedDisplay.id }) {
                    if selectedDisplay != current { self.selectedDisplay = current }
                } else {
                    selectDisplay(displays[0])
                }
                let name = reply.hostName ?? "远程 Mac"
                if remoteHostName != name { remoteHostName = name }
                displayRefresh = .now
            }
            guard let selectedDisplay else { return false }
            let edge = frameResolution == .automatic
                ? max(480, min(4096, Int(Double(viewportEdge) * (captureTime > 0.35 ? 0.60 : captureTime > 0.20 ? 0.85 : 1))))
                : frameResolution.rawValue
            let quality = frameQuality == .automatic
                ? (captureTime > 0.35 ? 45 : captureTime > 0.20 ? 65 : 82)
                : frameQuality.rawValue
            let start = ContinuousClock.now
            let reply = try await channel.send(.init(action: "capture", display: selectedDisplay.id, edge: edge, quality: quality))
            guard frameChannel === channel, self.selectedDisplay?.id == selectedDisplay.id else { return false }
            if reply.status == "image", let image = reply.image, let bytes = Data(base64Encoded: image),
               let frame = NSImage(data: bytes) {
                let elapsed = (ContinuousClock.now - start).components
                let seconds = Double(elapsed.seconds) + Double(elapsed.attoseconds) / 1e18
                captureTime = captureTime * 0.80 + seconds * 0.20
                remoteFrame = frame
                if screenNotice != nil { screenNotice = nil }
                return true
            }
            remoteFrame = nil
            if reply.code == "display_unavailable" { displayRefresh = .distantPast }
            screenNotice = remoteError(reply.code)
        } catch {
            if self.session == session && !Task.isCancelled {
                frameChannel?.close()
                frameChannel = nil
                remoteFrame = nil
                screenNotice = remoteError(error.localizedDescription)
            }
        }
        return false
    }

    func selectDisplay(_ display: RemoteDisplay) {
        guard selectedDisplay?.id != display.id else { return }
        if let selectedDisplay { queuePointer(.init(kind: .release), display: selectedDisplay.id) }
        remoteFrame = nil
        screenNotice = nil
        selectedDisplay = display
    }

    func setViewportEdge(_ edge: Int) {
        viewportEdge = edge
    }

    func queuePointer(_ event: RemotePointerEvent, display: UInt32) {
        guard let session else { return }
        if event.kind == .move, inputNotice != nil { return }
        if let last = pointerQueue.last, last.display == display,
           last.event.kind == .move, event.kind == .move {
            pointerQueue[pointerQueue.count - 1].event = event
        } else if let last = pointerQueue.last, last.display == display,
                  last.event.kind == .scroll, event.kind == .scroll {
            var sum = event
            sum.dx = Int(Int16(clamping: (last.event.dx ?? 0) + (event.dx ?? 0)))
            sum.dy = Int(Int16(clamping: (last.event.dy ?? 0) + (event.dy ?? 0)))
            if sum.dx == 0 && sum.dy == 0 {
                pointerQueue.removeLast()
            } else {
                pointerQueue[pointerQueue.count - 1].event = sum
            }
        } else {
            pointerQueue.append((display, event))
        }
        if pointerTask == nil {
            pointerTask = Task { await sendPointerQueue(session) }
        }
    }

    private func sendPointerQueue(_ session: PairedHost) async {
        defer {
            if !Task.isCancelled { pointerTask = nil }
        }
        do {
            if pointerChannel == nil { pointerChannel = try RemoteChannel(keyPath: keyPath(for: .controller), host: session) }
            guard let channel = pointerChannel else { return }
            while !Task.isCancelled, pointerChannel === channel, let next = pointerQueue.first {
                let batch = pointerQueue.prefix(32).prefix { $0.display == next.display }
                let events = batch.map(\.event)
                pointerQueue.removeFirst(events.count)
                let reply = try await channel.send(.init(action: "pointer", display: next.display, events: events))
                guard pointerChannel === channel else { return }
                if reply.status != "ok" {
                    inputNotice = remoteError(reply.code)
                    channel.close()
                    pointerChannel = nil
                    pointerQueue.removeAll()
                    return
                }
                if inputNotice != nil { inputNotice = nil }
            }
            return
        } catch {
            if !Task.isCancelled { inputNotice = remoteError(error.localizedDescription) }
        }
        if !Task.isCancelled {
            pointerChannel?.close()
            pointerChannel = nil
            pointerQueue.removeAll()
        }
    }

    func sendInput(_ action: RemoteAction) async {
        guard !isSendingInput, let session else { return }
        isSendingInput = true
        defer { isSendingInput = false }
        do {
            let reply = try await remote(action, session: session)
            guard self.session == session else { return }
            inputNotice = reply.status == "ok" ? nil : remoteError(reply.code)
        } catch {
            if self.session == session { inputNotice = remoteError(error.localizedDescription) }
        }
    }

    private func remote(_ action: RemoteAction, session: PairedHost) async throws -> RemoteReply {
        let request = try JSONEncoder().encode(action)
        let command = try LocalCommand(.controller, arguments: [
            "remote", keyPath(for: .controller), session.hostID, session.address,
        ], interactive: true)
        try command.writeRequest(request)
        let result = try await command.result()
        return try JSONDecoder().decode(RemoteReply.self, from: Data(result.utf8))
    }

    private func remoteError(_ reason: String?) -> ConnectionNotice {
        let message: String
        switch reason {
        case "screen_permission_required":
            message = "请在被控端允许 Koko 录屏并重新打开 Koko，然后重新连接。"
        case "input_permission_required":
            message = "被控端需要授予辅助功能权限，才能接收键鼠。"
        case "capture_busy":
            message = "被控端正在采集画面，请稍后点「刷新画面」。"
        case "display_unavailable":
            message = "这个显示器已离线，正在检查可用的显示器。"
        case "invalid_command":
            message = "请让两端使用同一版本的 Koko。"
        case "capture_failed":
            message = "被控端未能采集所选显示器，请检查登录会话和录屏权限。"
        case "input_failed":
            message = "被控端未能发送键鼠事件。"
        default:
            message = reason?.contains("peer not authorized") == true
                ? "远程会话已结束，请重新连接。"
                : "远程操作未完成，请检查被控端是否仍在运行。"
        }
        return ConnectionNotice(message: message, isError: true)
    }

    func disconnect() {
        let disconnected = session
        session = nil
        remoteFrame = nil
        screenNotice = nil
        inputNotice = nil
        remoteText = ""
        displays = []
        selectedDisplay = nil
        remoteHostName = ""
        displayRefresh = .distantPast
        captureTime = 0.18
        showTextControls = false
        frameChannel?.close()
        frameChannel = nil
        pointerTask?.cancel()
        pointerTask = nil
        pointerChannel?.close()
        pointerChannel = nil
        pointerQueue.removeAll()
        if let disconnected {
            Task { _ = try? await remote(.init(action: "disconnect"), session: disconnected) }
        }
    }

    func cancelCheck() {
        cancelRequested = true
        if let checkProcess, checkProcess.isRunning { checkProcess.terminate() }
    }

    func stopForQuit() {
        disconnect()
        hostPointer.release()
        if let process = hostCommand?.process, process.isRunning { process.terminate() }
        if let checkProcess, checkProcess.isRunning { checkProcess.terminate() }
    }

    private func keyPath(for role: DeviceRole) -> String {
        identityDirectory.appendingPathComponent(role.keyFilename).path
    }
}
