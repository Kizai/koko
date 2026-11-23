import AppKit
import Combine
import IOKit.pwr_mgt
import ScreenCaptureKit
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

    var title: String {
        switch self {
        case .unknown: "待检查"
        case .required: "未授权"
        case .granted: "已授权"
        case .failed: "检查失败"
        }
    }
}

struct PendingPair {
    let number: String
    let fingerprint: String
    var kind = "human"
    var name = ""
    var scope = "human"

    var isAgent: Bool { kind == "agent" }
}

struct AgentSession: Equatable {
    let fingerprint: String
    let name: String
    let scope: String
}

/// What an agent may do on this Mac, as the host owner sees it.
func agentAccessTitle(_ scope: String) -> String {
    scope == "agent_safe" ? "查看并操作此 Mac" : "查看此 Mac 的屏幕"
}

private struct PeerDescription: Decodable {
    var kind = "agent"
    let name: String
    let scope: String
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
    let os: String?

    var platformName: String {
        switch os {
        case "windows": "Windows"
        case "macos": "Mac"
        case "linux": "Linux"
        default: "系统未知"
        }
    }

    var platformIcon: String {
        switch os {
        case "windows": "square.grid.2x2.fill"
        case "macos": "apple.logo"
        default: "desktopcomputer"
        }
    }
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
        default:
            if let display, rawValue <= max(display.width, display.height) {
                display.dimensions(edge: rawValue)
            } else {
                "\(rawValue) px（长边上限）"
            }
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
    var transfer: String?
    var paths: [String]? = nil
    var x: Int? = nil
    var y: Int? = nil
    var key: String? = nil
    var text: String? = nil
    var display: UInt32? = nil
    var edge: Int? = nil
    var quality: Int? = nil
    var fps: Int? = nil
    var events: [RemotePointerEvent]? = nil
    var sourceOS: String? = nil
    var videoSocket: String? = nil
    var format: String? = nil
    var cursor: Bool? = nil
    var maxNodes: Int? = nil
    /// Set by the daemon on input from an agent, and `approved` once the person allowed it.
    var actor: String? = nil
    var approved: Bool? = nil

    enum CodingKeys: String, CodingKey {
        case action, transfer, paths, x, y, key, text, display, edge, quality, fps, events, format, cursor
        case actor, approved
        case maxNodes = "max_nodes"
        case sourceOS = "source_os"
        case videoSocket = "video_socket"
    }
}

struct RemoteNetworkStats {
    let rttMS: Int?
    let frameMS: Int
    var framesPerSecond: Double?
    var kilobitsPerSecond: Double?
    let direct: Bool?
    var isVideo = false
}

private struct RemoteReply: Codable {
    let status: String
    var clipboard: Bool?
    var png: String? = nil
    var code: String? = nil
    var image: String? = nil
    var hostName: String? = nil
    var displays: [RemoteDisplay]? = nil
    var rttMS: Int? = nil
    var direct: Bool? = nil
    var video: Bool? = nil
    var width: Int? = nil
    var height: Int? = nil
    var format: String? = nil
    var control: String? = nil
    var app: String? = nil
    var nodes: [UINode]? = nil
    var truncated: Bool? = nil
    var reason: String? = nil
    var clipboardStream: Bool? = nil
    var text: String? = nil
    var paths: [String]? = nil
    var path: String? = nil

    enum CodingKeys: String, CodingKey {
        case status, clipboard, png, code, image, displays, direct, video, width, height, format, control
        case app, nodes, truncated, reason, text, paths, path
        case clipboardStream = "clipboard_stream"
        case rttMS = "rtt_ms"
        case hostName = "host_name"
    }
}

private struct CommandError: LocalizedError {
    let errorDescription: String?
}

@MainActor
struct LocalCommand {
    let process = Process()
    private let output = Pipe()
    private let writer = DispatchQueue(label: "KoKo.helper.stdin")
    private let input: Pipe?
    private let chunks: AsyncStream<Data>
    private let completion: AsyncStream<Int32>

    init(_ role: DeviceRole, arguments: [String], interactive: Bool = false) throws {
        let tools = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers", isDirectory: true)
        try self.init(executable: tools.appendingPathComponent(role.executable), arguments: arguments, interactive: interactive)
    }

    init(executable: URL, arguments: [String], interactive: Bool = false) throws {
        let (chunks, outputEvents) = AsyncStream<Data>.makeStream()
        let (completion, exitEvents) = AsyncStream<Int32>.makeStream()
        self.chunks = chunks
        self.completion = completion

        process.executableURL = executable
        process.arguments = arguments
        input = interactive ? Pipe() : nil
        if let input {
            // A helper can exit between isRunning and write. Report EPIPE on this pipe without terminating the GUI.
            guard fcntl(input.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1) != -1 else {
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
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

    func send(_ line: String) async throws {
        try await write(Data((line + "\n").utf8))
    }

    private func write(_ data: Data) async throws {
        guard let handle = input?.fileHandleForWriting else { throw CommandError(errorDescription: "辅助进程没有输入管道。") }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            writer.async {
                continuation.resume(with: Result { try handle.write(contentsOf: data) })
            }
        }
    }

    func writeRequest(_ data: Data) async throws {
        defer { finishInput() }
        try await write(data)
    }

    func finishInput() {
        let handle = input?.fileHandleForWriting
        writer.async { try? handle?.close() }
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
        let encoder = JSONEncoder()
        encoder.outputFormatting = .withoutEscapingSlashes
        let request = try encoder.encode(action)
        return try await withCheckedThrowingContinuation { reply in
            pending = reply
            Task {
                do {
                    guard termination == nil else { return }
                    try await command.send(String(decoding: request, as: UTF8.self))
                } catch { finish(error) }
            }
        }
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
        guard termination == nil else { return }
        command.finishInput()
        finish(CancellationError())
    }
}

@MainActor
final class LocalConnection: ObservableObject {
    static let frameRates = [30, 60, 90, 120, 144, 165, 240]
    @Published var listenPort = UserDefaults.standard.string(forKey: "listenPort") ?? "4444"
    @Published var inviteInput = ""
    @Published private(set) var controllerID = ""
    @Published private(set) var hostID = ""
    @Published private(set) var isChecking = false
    @Published private(set) var isHosting = false {
        didSet { KeepAwake.shared.update(hosting: isHosting) }
    }
    @Published private(set) var isSharingWithPeer = false
    @Published private(set) var connectionCode = ""
    @Published private(set) var directInvite = ""
    @Published private(set) var pendingPair: PendingPair?
    @Published private(set) var agentSession: AgentSession? {
        didSet { updateAgentIndicator() }
    }
    /// The host arbiter's view of agent control: idle, active, paused or awaiting_human.
    @Published private(set) var agentControl = "idle" {
        didSet {
            if agentControl != "awaiting_human" { agentHelp = nil }
            if agentControl != "awaiting_approval" { agentApproval = nil }
            updateAgentIndicator()
            watchLocalInput()
        }
    }
    /// What the agent asked the person at this Mac to do, while it waits for an answer.
    @Published private(set) var agentHelp: String? {
        didSet { updateAgentIndicator() }
    }
    /// What a risky agent input would do, while it waits for the person's decision.
    @Published private(set) var agentApproval: String? {
        didSet { updateAgentIndicator() }
    }
    /// Kinds of risky input the person allowed for the rest of this agent session.
    private var agentAllowances: Set<String> = []
    private var pendingApprovalRule: String?
    private var localInputWatch: Task<Void, Never>?
    private var agentPolicyFile: URL { identityDirectory.appendingPathComponent("agent-policy.json") }
    /// What the remote host reports about an agent while this Mac is controlling it.
    @Published private(set) var remoteAgentControl = "idle"
    private let agentIndicator = AgentIndicator()
    @Published private(set) var trustedDeviceCount = 0
    @Published var confirmForgetTrusted = false
    @Published private(set) var controllerNotice: ConnectionNotice?
    @Published private(set) var hostNotice: ConnectionNotice?
    @Published private(set) var screenPermission = ScreenPermission.unknown
    @Published private(set) var isCheckingScreenPermission = false
    @Published private(set) var inputPermission = ScreenPermission.unknown
    @Published private(set) var isCheckingInputPermission = false
    private var detectedUpgradePermissionLoss = false
    @Published private(set) var isScanning = false
    @Published private var discoveredHosts: [NearbyHost] = []
    @Published private(set) var deviceNames = UserDefaults.standard.dictionary(forKey: "deviceNames") as? [String: String] ?? [:] {
        didSet { UserDefaults.standard.set(deviceNames, forKey: "deviceNames") }
    }
    @Published private(set) var hiddenDevices = UserDefaults.standard.stringArray(forKey: "hiddenDevices") ?? [] {
        didSet { UserDefaults.standard.set(hiddenDevices, forKey: "hiddenDevices") }
    }
    @Published var selectedNearbyHostID: String?
    @Published private(set) var session: PairedHost?
    @Published private(set) var isViewOnly = false
    @Published private(set) var remoteFrame: NSImage?
    @Published private(set) var frameDisplayID: UInt32?
    @Published private(set) var networkStats: RemoteNetworkStats?
    @Published private(set) var remoteHostName = ""
    @Published private(set) var displays: [RemoteDisplay] = []
    @Published private(set) var selectedDisplay: RemoteDisplay?
    @Published var frameQuality = FrameQuality.automatic
    @Published var frameResolution = FrameResolution.automatic
    @Published var targetFPS = 60
    @Published private(set) var remoteVideoSize: CGSize?
    @Published var showTextControls = false
    @Published var showDisplayControls = false
    @Published var showNetworkStats = false
    @Published var showControlCenter = false
    @Published private(set) var screenNotice: ConnectionNotice?
    @Published private(set) var clipboardNotice: ConnectionNotice?
    @Published private(set) var inputNotice: ConnectionNotice?
    @Published private(set) var isSendingInput = false
    @Published var remoteText = ""
    let localHostName = Host.current().localizedName ?? "此 Mac"

    var nearbyHosts: [NearbyHost] {
        discoveredHosts.filter { !hiddenDevices.contains($0.id) }
    }

    private let identityDirectory = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".koko", isDirectory: true)
    /// The `koko-agent` process copying or pasting files for the current transfer.
    private var clipboardCommand: LocalCommand?
    @Published private(set) var clipboardTransferID: String?
    private(set) var supportsClipboardStream = false
    private var displayActivity = IOPMAssertionID(0)
    private var hostCommand: LocalCommand?
    private var isCapturingScreen = false
    private var checkProcess: Process?
    private let hostPointer = DesktopInput()
    private var frameChannel: RemoteChannel?
    let videoSurface = VideoSurface()
    private var videoLatency = StreamLatency()
    private var videoSettings: VideoSettings?
    private var videoReceiver: VideoReceiver?
    private var videoTask: Task<Void, Never>?
    private var videoRetryAfter = ContinuousClock.now
    private var lastVideoFrameAt: ContinuousClock.Instant?
    private(set) var supportsVideo = false

    var displayedSize: CGSize? { remoteVideoSize ?? remoteFrame?.size }
    private var pointerChannel: RemoteChannel?
    private var pointerQueue: [(display: UInt32, event: RemotePointerEvent)] = []
    private var pointerTask: Task<Void, Never>?
    private var displayRefresh = Date.distantPast
    private var captureTime = 0.18
    private var lastFrameAt: ContinuousClock.Instant?
    private var cachedFrames: [UInt32: NSImage] = [:]
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
            case .host:
                hostID = id
                if let data = try? Data(contentsOf: trustedDeviceFile),
                   let records = try? JSONSerialization.jsonObject(with: data) as? [Any] {
                    trustedDeviceCount = records.count
                } else if let peers = try? String(contentsOf: legacyTrustedDeviceFile, encoding: .utf8) {
                    trustedDeviceCount = peers.split(whereSeparator: \.isNewline).count
                }
            }
        } catch {
            let notice = ConnectionNotice(message: error.localizedDescription, isError: true)
            switch role {
            case .controller: controllerNotice = notice
            case .host: hostNotice = notice
            }
        }
    }

    func refreshDesktopPermissions() {
        let screen = hasScreenCapturePermission(request: false)
        let input = hasInputPermission(request: false)
        let screenState: ScreenPermission = screen ? .granted : .required
        let inputState: ScreenPermission = input ? .granted : .required
        guard screenPermission != screenState || inputPermission != inputState else { return }
        if screenPermission != screenState { screenPermission = screenState }
        if inputPermission != inputState { inputPermission = inputState }
        if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
           DesktopPermissionHistory.record(version: version, screen: screen, input: input, in: .standard) {
            detectedUpgradePermissionLoss = true
        }
    }

    func checkForUpgradePermissionLoss() -> Bool {
        // The app can become active and run its first preflight before the connection window appears.
        refreshDesktopPermissions()
        defer { detectedUpgradePermissionLoss = false }
        return detectedUpgradePermissionLoss
    }

    func requestScreenPermission() async {
        guard !isCheckingScreenPermission else { return }
        isCheckingScreenPermission = true
        defer { isCheckingScreenPermission = false }
        _ = await Task.detached { hasScreenCapturePermission(request: true) }.value
        refreshDesktopPermissions()
    }

    func requestInputPermission() async {
        guard !isCheckingInputPermission else { return }
        isCheckingInputPermission = true
        defer { isCheckingInputPermission = false }
        _ = await Task.detached { hasInputPermission(request: true) }.value
        refreshDesktopPermissions()
    }

    /// Whether the person left sharing on. New installs start with it on.
    private static let sharingKey = "allowRemoteAssistance"

    /// Turns sharing back on at launch if it was on before, so updates and restarts keep it.
    func restoreSharing() async {
        guard UserDefaults.standard.object(forKey: Self.sharingKey) as? Bool ?? true, !isHosting else { return }
        await prepareIdentity(for: .host)
        refreshDesktopPermissions()
        guard !hostID.isEmpty, screenPermission == .granted, inputPermission == .granted, !isHosting else { return }
        await startHosting()
    }

    func startHosting() async {
        isHosting = true
        stopRequested = false
        UserDefaults.standard.set(true, forKey: Self.sharingKey)
        hostNotice = ConnectionNotice(message: "正在启动监听…")
        // macOS slows apps that are hidden or have no open window; a host must answer at once.
        let activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiatedAllowingIdleSystemSleep, .latencyCritical], reason: "KoKo 正在共享本机"
        )
        defer {
            ProcessInfo.processInfo.endActivity(activity)
            endHostDesktop()
            hostCommand = nil
            isHosting = false
            isSharingWithPeer = false
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            agentSession = nil
            agentControl = "idle"
            hostPointer.release()
        }
        agentIndicator.onStop = { [weak self] in Task { await self?.sendHost("stop-agent") } }
        agentIndicator.onResume = { [weak self] in Task { await self?.sendHost("resume-agent") } }
        agentIndicator.onAnswer = { [weak self] result in Task { await self?.answerAgentHelp(result) } }
        agentIndicator.onDecide = { [weak self] choice in Task { await self?.decideAgentApproval(choice) } }

        do {
            let address = "0.0.0.0:\(listenPort.trimmingCharacters(in: .whitespacesAndNewlines))"
            let command = try LocalCommand(.host, arguments: [
                "serve-pair", keyPath(for: .host), address, "--app-desktop", localHostName,
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
            endHostDesktop()
            isSharingWithPeer = true
            connectionCode = ""
            directInvite = ""
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "远程协助已授权，控制端可查看与操作。")
        case "Session closed":
            endHostDesktop()
            isSharingWithPeer = false
            hostPointer.release()
            hostNotice = ConnectionNotice(message: "远程会话已结束。仍可从局域网重新连接。")
        case "Pair declined", "Pair request ended":
            pendingPair = nil
            hostNotice = ConnectionNotice(message: "配对未完成，仍可使用当前连接码。")
        case "Pair code used":
            connectionCode = ""
            directInvite = ""
        case "Agent session closed":
            agentSession = nil
            agentControl = "idle"
            agentAllowances = []
            hostNotice = ConnectionNotice(message: "Agent 会话已结束。")
        case "Agent control: idle", "Agent control: active", "Agent control: paused",
             "Agent control: awaiting_human", "Agent control: awaiting_approval":
            agentControl = String(line.dropFirst("Agent control: ".count))
        case "Pair code expired":
            connectionCode = ""
            directInvite = ""
            hostNotice = ConnectionNotice(message: "授权码已过期，仍可通过局域网搜索申请控制。")
        default:
            if line.hasPrefix("Trusted devices: "), let count = Int(line.dropFirst("Trusted devices: ".count)) {
                trustedDeviceCount = count
            } else if line.hasPrefix("Trust storage failed: ") {
                hostNotice = ConnectionNotice(message: "无法保存设备信任。请检查 Koko 的数据目录。", isError: true)
            } else if line.hasPrefix("Pair code: ") {
                connectionCode = String(line.dropFirst("Pair code: ".count))
                hostNotice = ConnectionNotice(message: "连接码 5 分钟内有效，授权一次后失效。")
            } else if line.hasPrefix("Pair invite: ") {
                directInvite = String(line.dropFirst("Pair invite: ".count))
            } else if line.hasPrefix("Pair request: ") {
                let fields = line.dropFirst("Pair request: ".count).split(separator: " ", maxSplits: 2)
                if fields.count >= 2 {
                    var request = PendingPair(number: String(fields[0]), fingerprint: String(fields[1]))
                    if fields.count == 3,
                       let peer = try? JSONDecoder().decode(PeerDescription.self, from: Data(fields[2].utf8)) {
                        request.kind = peer.kind
                        request.name = peer.name
                        request.scope = peer.scope
                    }
                    pendingPair = request
                }
            } else if line.hasPrefix("Agent approval: ") {
                struct Approval: Decodable { let reason: String }
                if let approval = try? JSONDecoder().decode(Approval.self, from: Data(line.dropFirst("Agent approval: ".count).utf8)) {
                    agentApproval = approval.reason
                    NSApp.requestUserAttention(.criticalRequest)
                }
            } else if line.hasPrefix("Agent help: ") {
                struct Help: Decodable { let reason: String }
                if let help = try? JSONDecoder().decode(Help.self, from: Data(line.dropFirst("Agent help: ".count).utf8)) {
                    agentHelp = help.reason
                    NSApp.requestUserAttention(.criticalRequest)
                }
            } else if line.hasPrefix("Agent session: ") {
                let fields = line.dropFirst("Agent session: ".count).split(separator: " ", maxSplits: 1)
                if fields.count == 2,
                   let peer = try? JSONDecoder().decode(PeerDescription.self, from: Data(fields[1].utf8)) {
                    agentSession = AgentSession(fingerprint: String(fields[0]), name: peer.name, scope: peer.scope)
                    agentAllowances = []
                    pendingPair = nil
                    hostNotice = ConnectionNotice(message: "Agent「\(peer.name)」已连接，可\(agentAccessTitle(peer.scope))。")
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
        guard isSharingWithPeer || agentSession != nil else { return }
        let reply = await executeDesktopAction(action)
        guard !stopRequested, hostCommand?.process === command.process else { return }
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = .withoutEscapingSlashes
            let json = try encoder.encode(reply)
            try await command.send("desktop-reply \(request) \(String(decoding: json, as: UTF8.self))")
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    private func executeDesktopAction(_ action: RemoteAction) async -> RemoteReply {
        if action.actor == "agent", action.approved != true, ["pointer", "text"].contains(action.action) {
            let policy: AgentPolicy
            do {
                policy = try AgentPolicy.load(from: agentPolicyFile)
            } catch {
                policy = AgentPolicy()
                hostNotice = ConnectionNotice(message: "agent-policy.json 格式有误，已改用默认的高危操作规则。", isError: true)
            }
            if let hit = policy.check(action, frontmost: NSWorkspace.shared.frontmostApplication),
               !agentAllowances.contains(hit.rule) {
                pendingApprovalRule = hit.rule
                return RemoteReply(status: "approval_required", reason: hit.summary)
            }
        }
        do {
            switch action.action {
            case "clipboard_read":
                // Press Copy in the frontmost app, then report what it put on the pasteboard.
                guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
                let board = NSPasteboard.general
                let before = board.changeCount
                try postRemoteKey("copy")
                for _ in 0..<50 where board.changeCount == before {
                    try await Task.sleep(for: .milliseconds(40))
                }
                guard board.changeCount != before else { throw ClipboardError.unchanged }
                let items = try ClipboardItems.read(from: board)
                return RemoteReply(status: "clipboard_items", text: items.text, paths: items.paths)
            case "clipboard_target":
                do {
                    return RemoteReply(status: "clipboard_folder", path: try makeClipboardFolder().path)
                } catch {
                    throw ClipboardError.from(error)
                }
            case "clipboard_publish":
                guard hasInputPermission(request: false) else { throw DesktopError.inputPermissionRequired }
                try ClipboardItems(text: action.text, paths: action.paths ?? []).publish(on: NSPasteboard.general)
                try postRemoteKey("paste")
            case "displays":
                // Renew only while an authorized viewer is asking for screens.
                IOPMAssertionDeclareUserActivity("KoKo remote desktop" as CFString, kIOPMUserActiveRemote, &displayActivity)
                guard CGPreflightScreenCaptureAccess() else { throw ScreenCaptureError.permissionRequired }
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                let main = CGMainDisplayID()
                let screens = NSScreen.screens
                let displays = content.displays.map { display in
                    let id = display.displayID
                    let screen = screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id }
                    let mode = CGDisplayCopyDisplayMode(id)
                    return RemoteDisplay(id: id, name: screen?.localizedName ?? "显示器 \(id)",
                                         width: mode?.pixelWidth ?? display.width, height: mode?.pixelHeight ?? display.height)
                }
                let ordered = displays.filter { $0.id == main } + displays.filter { $0.id != main }
                return RemoteReply(status: "displays", clipboard: true, hostName: localHostName, displays: ordered, video: true,
                                   clipboardStream: true)
            case "video":
                guard let display = action.display, let edge = action.edge, let quality = action.quality,
                      let fps = action.fps, let socket = action.videoSocket else { throw DesktopError.invalidCommand }
                let settings = VideoSettings(display: display, edge: edge, quality: quality, fps: fps)
                Task { await streamDesktopVideo(settings, socket: socket) }
            case "screenshot", "capture":
                guard !isCapturingScreen else { return RemoteReply(status: "error", code: "capture_busy") }
                isCapturingScreen = true
                defer { isCapturingScreen = false }
                let png = action.action == "screenshot" || action.format == "png"
                let shot = try await captureScreenImage(
                    displayID: action.display, edge: action.edge ?? 1600,
                    quality: png ? nil : action.quality,
                    showsCursor: action.cursor ?? (action.action == "screenshot"),
                    excludingWindows: [agentIndicator.windowID].compactMap { $0 }
                )
                screenPermission = .granted
                let image = shot.data.base64EncodedString()
                return action.action == "screenshot"
                    ? RemoteReply(status: "frame", png: image)
                    : RemoteReply(status: "image", image: image, width: shot.width, height: shot.height,
                                  format: png ? "png" : "jpeg")
            case "ui_tree":
                guard let maxNodes = action.maxNodes, (1...1000).contains(maxNodes) else { throw DesktopError.invalidCommand }
                guard AXIsProcessTrusted() else { throw DesktopError.inputPermissionRequired }
                guard let app = NSWorkspace.shared.frontmostApplication else { throw ScreenCaptureError.captureFailed }
                let pid = app.processIdentifier
                // Accessibility calls block on the target app, so keep them off the main thread.
                let tree = await Task.detached { readUITree(pid: pid, maxNodes: maxNodes) }.value
                return RemoteReply(status: "ui_tree", app: app.localizedName ?? "App", nodes: tree.nodes,
                                   truncated: tree.truncated ? true : nil)
            case "pointer":
                guard let display = action.display, let events = action.events else { throw DesktopError.invalidCommand }
                let mapKeys = action.sourceOS == "windows" && (UserDefaults.standard.object(forKey: "mapWindowsModifiers") as? Bool ?? true)
                try hostPointer.apply(events, display: display, mapWindowsModifiers: mapKeys)
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
        } catch let error as ClipboardError {
            return RemoteReply(status: "error", code: error.rawValue)
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

    func respondToPairing(_ request: PendingPair, allow: Bool, remember: Bool = false) async {
        do {
            let action = allow ? (remember ? "remember" : "approve") : "decline"
            try await hostCommand?.send("\(action) \(request.number)")
            pendingPair = nil
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    private var trustedDeviceFile: URL {
        identityDirectory.appendingPathComponent("trusted-controllers.json")
    }

    // Written by releases before agents could pair. The daemon migrates it on its next start.
    private var legacyTrustedDeviceFile: URL {
        identityDirectory.appendingPathComponent("trusted-controllers")
    }

    func endAgentSession() async {
        await sendHost("end-agent")
    }

    func stopAgent() async {
        await sendHost("stop-agent")
    }

    func resumeAgent() async {
        await sendHost("resume-agent")
    }

    private func sendHost(_ line: String) async {
        do {
            try await hostCommand?.send(line)
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    /// Answers the agent's request for help: resolved, taken_over or declined, with an optional note.
    func answerAgentHelp(_ result: String, note: String = "") async {
        let note = note.components(separatedBy: .newlines).joined(separator: " ").trimmingCharacters(in: .whitespaces)
        await sendHost(note.isEmpty ? "agent-help \(result)" : "agent-help \(result) \(note)")
    }

    /// Decides a risky agent input: once, session (allow this kind until the agent disconnects) or deny.
    func decideAgentApproval(_ choice: String) async {
        if choice == "session", let rule = pendingApprovalRule { agentAllowances.insert(rule) }
        pendingApprovalRule = nil
        await sendHost(choice == "deny" ? "approval deny" : "approval allow")
    }

    private func updateAgentIndicator() {
        agentIndicator.update(control: agentControl, agentName: agentSession?.name,
                              help: agentControl == "awaiting_approval" ? agentApproval : agentHelp)
    }

    /// While an agent operates, someone using this Mac's own mouse or keyboard takes control back.
    private func watchLocalInput() {
        localInputWatch?.cancel()
        localInputWatch = nil
        guard agentControl == "active" else { return }
        let start = DispatchTime.now().uptimeNanoseconds
        localInputWatch = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let self, self.agentControl == "active" else { return }
                if localInputSinceInjection(within: 0.5, since: start) {
                    await self.sendHost("local-input")
                    return
                }
            }
        }
    }

    func forgetTrustedDevices() async {
        do {
            if let hostCommand {
                try await hostCommand.send("forget-trusted")
            } else {
                for file in [trustedDeviceFile, legacyTrustedDeviceFile]
                where FileManager.default.fileExists(atPath: file.path) {
                    try FileManager.default.removeItem(at: file)
                }
                trustedDeviceCount = 0
            }
        } catch {
            hostNotice = ConnectionNotice(message: error.localizedDescription, isError: true)
        }
    }

    var selectedNearbyHost: NearbyHost? {
        nearbyHosts.first(where: { $0.id == selectedNearbyHostID })
    }

    func name(for host: NearbyHost) -> String {
        deviceNames[host.id] ?? host.label
    }

    func rename(_ host: NearbyHost, to name: String) {
        let label = String(name.trimmingCharacters(in: .whitespacesAndNewlines).prefix(64))
        deviceNames[host.id] = label.isEmpty ? nil : label
    }

    func hide(_ host: NearbyHost) {
        hiddenDevices.append(host.id)
        if selectedNearbyHostID == host.id { selectedNearbyHostID = nil }
    }

    func restoreHiddenDevices() {
        hiddenDevices.removeAll()
    }

    func switchSessionMode(viewOnly: Bool) {
        if viewOnly {
            cancelClipboardTransfer()
            clipboardNotice = nil
            pointerQueue.removeAll()
            if let display = selectedDisplay, pointerChannel != nil {
                queuePointer(.init(kind: .release), display: display.id)
            }
        }
        isViewOnly = viewOnly
    }

    private func endHostDesktop() {
        if displayActivity != 0 {
            IOPMAssertionRelease(displayActivity)
            displayActivity = 0
        }
    }

    func stopHosting() {
        endHostDesktop()
        stopRequested = true
        UserDefaults.standard.set(false, forKey: Self.sharingKey)
        isSharingWithPeer = false
        agentSession = nil
        agentControl = "idle"
        hostPointer.release()
        connectionCode = ""
        directInvite = ""
        pendingPair = nil
        hostNotice = ConnectionNotice(message: "正在停止监听…")
        if let process = hostCommand?.process, process.isRunning { process.interrupt() }
    }

    func verify(code: String, viewOnly: Bool = false) async {
        isChecking = true
        cancelRequested = false
        controllerNotice = ConnectionNotice(message: code.hasPrefix("koko://request/")
            ? "正在连接局域网设备…" : "正在使用授权码连接…")
        defer {
            checkProcess = nil
            isChecking = false
        }

        do {
            let defaults = UserDefaults.standard
            var arguments = ["pair", keyPath(for: .controller), code]
            if defaults.object(forKey: "preferLAN") as? Bool ?? true { arguments.append("--prefer-lan") }
            if defaults.object(forKey: "enhancedConnections") as? Bool ?? true { arguments.append("--enhanced") }
            let command = try LocalCommand(.controller, arguments: arguments)
            checkProcess = command.process
            let reply = try await command.result()
            let host = try JSONDecoder().decode(PairedHost.self, from: Data(reply.utf8))
            let defaultFPS = UserDefaults.standard.integer(forKey: "defaultFrameRate")
            targetFPS = Self.frameRates.contains(defaultFPS) ? defaultFPS : 60
            isViewOnly = viewOnly
            session = host
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
        guard !isScanning else { return }
        isScanning = true
        defer { isScanning = false }
        do {
            let command = try LocalCommand(.controller, arguments: ["discover"])
            let result = try await command.result()
            discoveredHosts = try JSONDecoder().decode([NearbyHost].self, from: Data(result.utf8))
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
            try? await Task.sleep(for: supportsVideo ? .milliseconds(300) : changed ? .milliseconds(90) : .seconds(1))
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
                    if reply.code == "desktop_unavailable", videoTask != nil, frameDisplayID != nil {
                        displayRefresh = .now
                        return true
                    }
                    frameDisplayID = nil
                    stopVideo()
                    screenNotice = remoteError(reply.code ?? "display_unavailable")
                    return false
                }
                if self.displays != displays {
                    self.displays = displays
                    let available = Set(displays.map(\.id))
                    cachedFrames = cachedFrames.filter { available.contains($0.key) }
                }
                if let selectedDisplay, let current = displays.first(where: { $0.id == selectedDisplay.id }) {
                    if selectedDisplay != current { self.selectedDisplay = current }
                } else {
                    selectDisplay(displays[0])
                }
                let name = reply.hostName ?? "远程 Mac"
                if remoteHostName != name { remoteHostName = name }
                supportsClipboardStream = reply.clipboardStream == true
                supportsVideo = reply.video == true
                let control = reply.control ?? "idle"
                if remoteAgentControl != control { remoteAgentControl = control }
                displayRefresh = .now
            }
            guard let selectedDisplay else { return false }
            let adaptive = UserDefaults.standard.object(forKey: "adaptiveStreaming") as? Bool ?? true
            if supportsVideo {
                let constrained = adaptive && videoLatency.constrained
                let edge = frameResolution == .automatic ? max(480, min(constrained ? 1280 : 4096, (viewportEdge / 128) * 128)) : frameResolution.rawValue
                let quality = frameQuality == .automatic ? (constrained ? 45 : 70) : frameQuality.rawValue
                let settings = VideoSettings(display: selectedDisplay.id, edge: edge, quality: quality, fps: targetFPS)
                if videoSettings != settings || (videoTask == nil && ContinuousClock.now >= videoRetryAfter) {
                    startVideo(session, settings: settings)
                } else if let lastVideoFrameAt, ContinuousClock.now - lastVideoFrameAt > .seconds(1),
                          var idle = networkStats, idle.framesPerSecond != 0 {
                    idle.framesPerSecond = 0
                    idle.kilobitsPerSecond = 0
                    networkStats = idle
                }
                return true
            }
            let responseTime = adaptive ? captureTime : 0
            let edge = frameResolution == .automatic
                ? max(480, min(4096, Int(Double(viewportEdge) * (responseTime > 0.35 ? 0.60 : responseTime > 0.20 ? 0.85 : 1))))
                : frameResolution.rawValue
            let quality = frameQuality == .automatic
                ? (responseTime > 0.35 ? 45 : responseTime > 0.20 ? 65 : 82)
                : frameQuality.rawValue
            let start = ContinuousClock.now
            let reply = try await channel.send(.init(action: "capture", display: selectedDisplay.id, edge: edge, quality: quality))
            guard frameChannel === channel else { return false }
            guard self.selectedDisplay?.id == selectedDisplay.id else { return true }
            if reply.status == "image", let image = reply.image, let bytes = Data(base64Encoded: image),
               let frame = NSImage(data: bytes) {
                let now = ContinuousClock.now
                let seconds = (now - start).secondsValue
                captureTime = captureTime * 0.80 + seconds * 0.20
                let interval = lastFrameAt.map { (now - $0).secondsValue }
                networkStats = RemoteNetworkStats(
                    rttMS: reply.rttMS,
                    frameMS: Int(seconds * 1000),
                    framesPerSecond: interval.map { 1 / $0 },
                    kilobitsPerSecond: interval.map { Double(bytes.count) * 8 / $0 / 1000 },
                    direct: reply.direct
                )
                lastFrameAt = now
                cachedFrames[selectedDisplay.id] = frame
                remoteFrame = frame
                if frameDisplayID != selectedDisplay.id { frameDisplayID = selectedDisplay.id }
                if screenNotice != nil { screenNotice = nil }
                return true
            }
            if frameDisplayID != nil { frameDisplayID = nil }
            networkStats = nil
            if reply.code == "display_unavailable" { displayRefresh = .distantPast }
            screenNotice = remoteError(reply.code)
        } catch {
            if self.session == session && !Task.isCancelled {
                frameChannel?.close()
                frameChannel = nil
                // A display-list timeout can occur while the independent video stream is still delivering frames.
                if videoTask != nil, frameDisplayID != nil, !error.localizedDescription.contains("peer not authorized") {
                    displayRefresh = .now
                    return true
                }
                frameDisplayID = nil
                networkStats = nil
                lastFrameAt = nil
                screenNotice = remoteError(error.localizedDescription)
                stopVideo()
            }
        }
        return false
    }

    private func startVideo(_ session: PairedHost, settings: VideoSettings) {
        stopVideo()
        videoSettings = settings
        do {
            let receiver = try VideoReceiver(keyPath: keyPath(for: .controller), host: session, settings: settings)
            videoReceiver = receiver
            videoTask = Task { await receiveVideo(receiver, settings: settings) }
        } catch {
            screenNotice = remoteError("capture_failed")
            videoRetryAfter = .now.advanced(by: .seconds(1))
        }
    }

    private func receiveVideo(_ receiver: VideoReceiver, settings: VideoSettings) async {
        let started = ContinuousClock.now
        var measurementStarted = started
        var frameCount = 0
        var byteCount = 0
        var firstFrameMS: Int?
        var size: CGSize?
        var rtt: Int?
        var direct: Bool?
        var failure = "capture_failed"
        do {
            records: while let (kind, bytes) = try await receiver.next() {
                guard videoReceiver === receiver, !Task.isCancelled else { return }
                switch kind {
                case .path:
                    if bytes.count == 9 {
                        rtt = bytes.prefix(8).reduce(0) { ($0 << 8) | Int($1) }
                        if let rtt { videoLatency.observe(rtt) }
                        direct = bytes.last == 1
                    }
                case .error:
                    failure = String(decoding: bytes, as: UTF8.self)
                    break records
                case .format:
                    size = try videoSurface.setFormat(bytes)
                case .frame:
                    guard selectedDisplay?.id == settings.display else { continue }
                    while !videoSurface.isReadyForMoreMediaData {
                        if videoSurface.hasFailed { throw CommandError(errorDescription: "video_codec_failed") }
                        try await Task.sleep(for: .milliseconds(2))
                        guard videoReceiver === receiver else { return }
                    }
                    guard selectedDisplay?.id == settings.display else { continue }
                    try videoSurface.enqueue(bytes)
                    let now = ContinuousClock.now
                    lastVideoFrameAt = now
                    if firstFrameMS == nil {
                        let frameMS = Int((now - started).secondsValue * 1000)
                        firstFrameMS = frameMS
                        measurementStarted = now
                        frameDisplayID = settings.display
                        remoteVideoSize = size
                        screenNotice = nil
                        networkStats = RemoteNetworkStats(rttMS: rtt, frameMS: frameMS,
                                                          framesPerSecond: nil, kilobitsPerSecond: nil,
                                                          direct: direct, isVideo: true)
                    }
                    frameCount += 1
                    byteCount += bytes.count
                    let seconds = (now - measurementStarted).secondsValue
                    if seconds >= 1, let firstFrameMS {
                        networkStats = RemoteNetworkStats(
                            rttMS: rtt, frameMS: firstFrameMS, framesPerSecond: Double(frameCount) / seconds,
                            kilobitsPerSecond: Double(byteCount) * 8 / seconds / 1000, direct: direct, isVideo: true
                        )
                        measurementStarted = now
                        frameCount = 0
                        byteCount = 0
                    }
                }
            }
        } catch {
            failure = (error as? VideoError)?.rawValue ?? error.localizedDescription
        }
        guard videoReceiver === receiver, !Task.isCancelled else { return }
        frameDisplayID = nil
        networkStats = nil
        screenNotice = remoteError(failure)
        stopVideo()
        videoRetryAfter = .now.advanced(by: failure == "video_busy" ? .milliseconds(300) : .seconds(1))
        if failure == "display_unavailable" { displayRefresh = .distantPast }
    }

    private func stopVideo() {
        videoTask?.cancel()
        videoTask = nil
        videoReceiver?.close()
        videoReceiver = nil
        lastVideoFrameAt = nil
    }

    func selectDisplay(_ display: RemoteDisplay) {
        guard selectedDisplay?.id != display.id else { return }
        if let selectedDisplay, pointerChannel != nil || pointerTask != nil {
            queuePointer(.init(kind: .release), display: selectedDisplay.id)
        }
        if let cached = cachedFrames[display.id] { remoteFrame = cached }
        frameDisplayID = nil
        screenNotice = nil
        selectedDisplay = display
    }

    func refreshDisplays() {
        displayRefresh = .distantPast
    }

    func setViewportEdge(_ edge: Int) {
        viewportEdge = edge
    }

    func queuePointer(_ event: RemotePointerEvent, display: UInt32) {
        guard let session else { return }
        guard !isViewOnly || event.kind == .release else { return }
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
                let reply = try await channel.send(.init(action: "pointer", display: next.display, events: events, sourceOS: "macos"))
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

    func cancelClipboardTransfer() {
        guard let command = clipboardCommand else { return }
        clipboardCommand = nil
        clipboardTransferID = nil
        clipboardNotice = nil
        if command.process.isRunning { command.process.terminate() }
    }

    /// Copies from or pastes to the host with ⌘C or ⌘V. `koko-agent` streams the files directly,
    /// so their size does not matter; this side only reads or fills the local pasteboard.
    func clipboardShortcut(_ key: String) async {
        guard supportsClipboardStream else {
            // Hosts before streaming keep using their own clipboard.
            await sendInput(.init(action: "key", key: key))
            return
        }
        guard !isViewOnly, let session, clipboardCommand == nil else { return }
        let copying = key == "copy"
        let board = NSPasteboard.general
        let before = board.changeCount
        var folder: URL?
        var command: LocalCommand?
        defer {
            // Anything not handed to the pasteboard is removed, including after a cancel.
            if clipboardCommand?.process === command?.process { clipboardCommand = nil; clipboardTransferID = nil }
        }
        do {
            var arguments = [copying ? "clipboard-pull" : "clipboard-push", keyPath(for: .controller),
                             session.hostID, session.address, UUID().uuidString]
            var request: Data?
            if copying {
                let target = try makeClipboardFolder()
                folder = target
                arguments.append(target.path)
            } else {
                request = try JSONEncoder().encode(ClipboardItems.read(from: board))
            }
            let running = try LocalCommand(.controller, arguments: arguments, interactive: request != nil)
            command = running
            clipboardCommand = running
            clipboardTransferID = arguments[4]
            clipboardNotice = ConnectionNotice(message: copying ? "正在复制到本机…" : "正在粘贴到被控端…")
            if let request { try await running.writeRequest(request) }
            var moved = false
            let waiting = Task { @MainActor [weak self] in
                // Before the first byte, macOS may be asking on either Mac whether KoKo may read a folder.
                try? await Task.sleep(for: .seconds(3))
                guard let self, !moved, self.clipboardCommand?.process === running.process else { return }
                self.clipboardNotice = ConnectionNotice(message: "正在等待文件访问。若 macOS 弹出提示，请在弹窗所在的 Mac 上选择允许或拒绝。")
            }
            defer { waiting.cancel() }
            var outcome: ClipboardProgressLine?
            _ = try await running.result(collectOutput: false) { line in
                guard let update = try? JSONDecoder().decode(ClipboardProgressLine.self, from: Data(line.utf8)) else { return }
                if update.status != nil {
                    outcome = update
                } else if let done = update.progress, let total = update.total, total > 0 {
                    moved = true
                    let size = { ByteCountFormatter.string(fromByteCount: $0, countStyle: .file) }
                    self.clipboardNotice = ConnectionNotice(
                        message: "\(copying ? "已接收" : "已发送") \(size(done)) / \(size(total))（\(done * 100 / total)%）…")
                }
            }
            guard clipboardCommand?.process === running.process else {
                if let folder { try? FileManager.default.removeItem(at: folder) }
                return
            }
            guard let outcome, outcome.status == "done" else {
                if let folder { try? FileManager.default.removeItem(at: folder) }
                clipboardNotice = clipboardError(outcome?.code)
                return
            }
            if copying {
                // Something else copied on this Mac meanwhile; that newer copy wins.
                guard board.changeCount == before else {
                    if let folder { try? FileManager.default.removeItem(at: folder) }
                    clipboardNotice = remoteError(ClipboardError.unchanged.rawValue)
                    return
                }
                try ClipboardItems(text: outcome.text, paths: outcome.paths ?? []).publish(on: board)
                if outcome.text != nil, let folder { try? FileManager.default.removeItem(at: folder) }
            }
            clipboardNotice = nil
        } catch {
            if let folder { try? FileManager.default.removeItem(at: folder) }
            // A cancelled transfer already cleared its notice.
            guard command == nil || clipboardCommand?.process === command?.process else { return }
            clipboardNotice = remoteError((error as? ClipboardError)?.rawValue ?? error.localizedDescription)
        }
    }

    private func clipboardError(_ code: String?) -> ConnectionNotice {
        if code == ClipboardError.permission.rawValue {
            return ConnectionNotice(message: "没有文件夹访问权限。请在对应 Mac 的「隐私与安全性 → 文件与文件夹」中允许 KoKo 访问该目录，再重新复制。", isError: true)
        }
        return remoteError(code)
    }

    func sendInput(_ action: RemoteAction) async {
        guard !isViewOnly, !isSendingInput, let session else { return }
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
        try await command.writeRequest(request)
        let result = try await command.result()
        return try JSONDecoder().decode(RemoteReply.self, from: Data(result.utf8))
    }

    private func remoteError(_ reason: String?) -> ConnectionNotice {
        let message: String
        switch reason {
        case "clipboard_file_permission":
            message = "本机没有文件夹访问权限。请在「隐私与安全性 → 文件与文件夹」中允许 KoKo 访问对应目录，再重新复制。"
        case "clipboard_timeout":
            message = "文件传输等待超时。请检查两台 Mac 上的文件访问提示，处理后重新复制。"
        case "clipboard_unsupported":
            message = "可共享文件、文件夹或 1 MiB 以内的纯文本，不支持替身、链接等特殊文件。"
        case "clipboard_unchanged":
            message = "剪贴板已变化或远端没有复制到新内容。请重新选择并复制。"
        case "clipboard_invalid", "clipboard_failed":
            message = "剪贴板传输未完成，请检查源文件、接收目录和连接后重试。"
        case "screen_permission_required":
            message = "请在被控端允许 Koko 录屏并重新打开 Koko，然后重新连接。"
        case "input_permission_required":
            message = "被控端需要授予辅助功能权限，才能接收键鼠。"
        case "capture_busy":
            message = "被控端正在采集画面，正在自动重试。"
        case "display_unavailable":
            message = "正在唤醒并重新检测显示器。若外接屏已断开或关闭电源，请选择其他屏幕。"
        case "invalid_command":
            message = "请让两端使用同一版本的 Koko。"
        case "capture_failed":
            message = "等待被控端画面。锁屏时可从控制中心打开 macOS 屏幕共享。"
        case "video_codec_failed", "video_packet_invalid":
            message = "无法解码当前画面，请试试降低分辨率或目标帧率。"
        case "video_busy":
            message = "正在切换视频流…"
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
        isViewOnly = false
        remoteFrame = nil
        remoteVideoSize = nil
        videoSurface.clear()
        stopVideo()
        videoSettings = nil
        videoLatency = StreamLatency()
        supportsClipboardStream = false
        cancelClipboardTransfer()
        clipboardNotice = nil
        supportsVideo = false
        frameDisplayID = nil
        cachedFrames.removeAll()
        networkStats = nil
        lastFrameAt = nil
        showDisplayControls = false
        showNetworkStats = false
        showControlCenter = false
        screenNotice = nil
        inputNotice = nil
        remoteText = ""
        displays = []
        selectedDisplay = nil
        remoteHostName = ""
        remoteAgentControl = "idle"
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

    var permissionRestartRequested = false

    func permissionGuideDidDismiss() {
        guard permissionRestartRequested else { return }
        permissionRestartRequested = false
        // AppKit vetoes termination while the SwiftUI permission sheet is attached.
        DispatchQueue.main.async {
            let message: String
            do {
                let waiter = try PermissionRelaunch.schedule()
                NSApp.terminate(nil)
                // A cancelled quit must not start another copy when the user eventually exits.
                if waiter.isRunning { waiter.terminate() }
                message = "退出被取消。请先关闭其他 Koko 弹窗，再重开并复查。"
            } catch {
                message = error.localizedDescription
            }
            let alert = NSAlert()
            alert.messageText = "未能重开 Koko"
            alert.informativeText = message
            alert.runModal()
        }
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

private extension Duration {
    var secondsValue: Double {
        Double(components.seconds) + Double(components.attoseconds) / 1e18
    }
}
