import AppKit
import AVFoundation
import SwiftUI

struct RemoteControlView: View {
    @ObservedObject var connection: LocalConnection
    @ViewState private var toolbarHeight = WindowButtonAlignment.defaultHeight
    @AppStorage("forwardPhysicalKeyboard") private var forwardPhysicalKeyboard = true

    private let keys = [
        ("Esc", "escape"), ("Tab", "tab"), ("回车", "enter"), ("删除", "backspace"),
        ("空格", "space"), ("←", "left"), ("↑", "up"), ("↓", "down"), ("→", "right"),
        ("⌘A", "select_all"), ("⌘C", "copy"), ("⌘V", "paste"), ("⌘Z", "undo"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            ZStack {
                Color(red: 0.08, green: 0.10, blue: 0.14)
                RemoteCanvas(
                    image: connection.remoteFrame,
                    videoSize: connection.remoteVideoSize,
                    videoSurface: connection.videoSurface,
                    display: connection.isViewOnly ? nil : connection.frameDisplayID,
                    acceptsKeyboard: forwardPhysicalKeyboard,
                    clipboard: { key in Task { await connection.clipboardShortcut(key) } },
                    pointer: connection.queuePointer,
                    resize: connection.setViewportEdge
                )
                if connection.displayedSize == nil {
                    Label(
                        connection.screenNotice?.message ?? "正在等待远端画面…",
                        systemImage: connection.screenNotice == nil ? "display" : "exclamationmark.triangle"
                    )
                    .foregroundStyle(.white.opacity(0.85))
                    .multilineTextAlignment(.center)
                    .padding(24)
                    .allowsHitTesting(false)
                } else if let notice = connection.screenNotice {
                    screenStatus(notice.message, icon: "exclamationmark.triangle")
                } else if connection.frameDisplayID == nil, let screen = connection.selectedDisplay {
                    screenStatus("正在切换到 \(screen.name)", icon: "display")
                }
            }
            .overlay(alignment: .bottomTrailing) {
              VStack(alignment: .trailing, spacing: 0) {
                if !connection.receivedFiles.isEmpty {
                    HStack(spacing: 8) {
                        Label("已收到被控端发来的 \(connection.receivedFiles.count) 项", systemImage: "tray.and.arrow.down")
                        Button("在访达中显示") {
                            NSWorkspace.shared.activateFileViewerSelecting(connection.receivedFiles)
                            connection.dismissReceivedFiles()
                        }
                        Button("关闭", action: connection.dismissReceivedFiles)
                            .buttonStyle(.borderless)
                    }
                    .font(.callout)
                    .padding(8)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                    .padding(.horizontal, 10)
                    .padding(.top, 10)
                }
                if let notice = connection.clipboardNotice ?? connection.inputNotice {
                    HStack(spacing: 8) {
                        Label(notice.message, systemImage: connection.clipboardNotice == nil ? "cursorarrow.rays" : "doc.on.clipboard")
                        if connection.clipboardTransferID != nil {
                            Button("取消传输", action: connection.cancelClipboardTransfer)
                                .buttonStyle(.borderless)
                        }
                    }
                    .font(.callout)
                    .padding(8)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                    .padding(10)
                }
              }
            }
        }
        .frame(minWidth: 670, idealWidth: 1060, minHeight: 410, idealHeight: 650)
        .ignoresSafeArea(.container, edges: .top)
        .task { await connection.watchRemoteScreen() }
        .sheet(isPresented: $connection.showTextControls) {
            VStack(alignment: .leading, spacing: 12) {
                Text("发送文本").font(.headline)
                HStack {
                    TextField("输入要发送到被控端的文本", text: $connection.remoteText)
                        .textFieldStyle(.roundedBorder)
                    Button("发送") {
                        Task { await connection.sendInput(.init(action: "text", text: connection.remoteText)) }
                    }
                    .disabled(connection.isSendingInput || connection.remoteText.isEmpty || connection.remoteText.utf8.count > 2048)
                    Button("关闭") { connection.showTextControls = false }
                        .keyboardShortcut(.cancelAction)
                }
            }
            .padding(20)
            .frame(width: 460)
        }
    }

    private func screenStatus(_ text: String, icon: String) -> some View {
        Label(text, systemImage: icon)
            .font(.callout)
            .padding(.vertical, 6)
            .padding(.horizontal, 10)
            .background(.regularMaterial, in: Capsule())
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
            .allowsHitTesting(false)
    }

    private var toolbar: some View {
        let tabs = HStack(spacing: 4) {
            ForEach(Array(connection.displays.enumerated()), id: \.element.id) { index, display in
                let selected = display.id == connection.selectedDisplay?.id
                Button {
                    connection.selectDisplay(display)
                } label: {
                    Label("\(index + 1) \(display.name)", systemImage: "display")
                        .lineLimit(1)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .foregroundStyle(selected ? Color.accentColor : .secondary)
                        .background(selected ? Color.accentColor.opacity(0.12) : .clear, in: RoundedRectangle(cornerRadius: 6))
                }
                .buttonStyle(.plain)
                .help("\(display.name) · \(display.width) × \(display.height)")
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        return HStack(spacing: 9) {
            Text(connection.session.flatMap { connection.deviceNames[$0.hostID] }
                 ?? (connection.remoteHostName.isEmpty ? "远程 Mac" : connection.remoteHostName))
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 184, alignment: .leading)
                .fixedSize(horizontal: true, vertical: false)
                .allowsHitTesting(false)
            Divider().frame(height: 18)
            ViewThatFits(in: .horizontal) {
                tabs.fixedSize(horizontal: true, vertical: false)
                ScrollView(.horizontal, showsIndicators: false) {
                    tabs
                }
            }
            displayControls
            audioControls
            networkControls
            Spacer(minLength: 0)
            switch connection.remoteAgentControl {
            case "active":
                Label("Agent 正在操作", systemImage: "sparkles")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize()
                    .help("你在画面上的键鼠操作会让 Agent 暂停")
            case "awaiting_approval":
                Label("Agent 等被控端确认高危操作", systemImage: "exclamationmark.shield")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize()
            case "awaiting_human":
                Label("Agent 请求被控端协助", systemImage: "hand.raised")
                    .font(.caption)
                    .foregroundStyle(.blue)
                    .fixedSize()
            case "paused":
                Label("Agent 已暂停", systemImage: "pause.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize()
            default:
                EmptyView()
            }
            if connection.isViewOnly {
                Label("观看模式", systemImage: "eye")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }
            controlCenter.fixedSize()
        }
        .padding(.leading, WindowButtonAlignment.leadingInset)
        .padding(.trailing, 12)
        .frame(height: toolbarHeight)
        .background(WindowButtonAlignment(height: $toolbarHeight))
        .background(.regularMaterial)
    }

    private var displayControls: some View {
        Button { connection.showDisplayControls.toggle() } label: {
            Image(systemName: "plus")
        }
        .buttonStyle(.borderless)
        .accessibilityLabel("显示器列表")
        .help("管理显示器")
        .popover(isPresented: $connection.showDisplayControls, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("可用显示器").font(.headline)
                ForEach(connection.displays) { display in
                    choice(display.name, selected: display.id == connection.selectedDisplay?.id) {
                        connection.selectDisplay(display)
                        connection.showDisplayControls = false
                    }
                }
                Divider()
                Button("重新检测显示器") {
                    connection.refreshDisplays()
                }
            }
            .padding(14)
            .frame(width: 236)
            .background(Color(nsColor: .windowBackgroundColor))
        }
    }

    private var audioControls: some View {
        let icon = !connection.isRemoteAudioEnabled || connection.isAudioMuted || connection.hostAudioVolume == 0
            ? "speaker.slash"
            : connection.hostAudioVolume < 50 ? "speaker.wave.1" : "speaker.wave.2"
        return Button { connection.showAudioControls.toggle() } label: {
            Image(systemName: icon)
                .foregroundStyle(connection.isAudioMuted ? Color.orange : Color.primary)
        }
        .buttonStyle(.borderless)
        .accessibilityLabel("声音控制")
        .help("声音与静音")
        .popover(isPresented: $connection.showAudioControls, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text("远程声音").font(.headline)
                    Spacer()
                    Button {
                        connection.isAudioMuted.toggle()
                    } label: {
                        Label(connection.isAudioMuted ? "取消静音" : "本机静音",
                              systemImage: connection.isAudioMuted ? "speaker.slash.fill" : "speaker.wave.2")
                            .font(.caption.weight(.medium))
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("被控端音量")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Text("\(connection.hostAudioVolume)%")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    Slider(
                        value: Binding(
                            get: { Double(connection.hostAudioVolume) },
                            set: { connection.setHostAudioVolume(Int($0.rounded())) }
                        ),
                        in: 0...100
                    )
                }
                Toggle("接收被控端声音", isOn: $connection.isRemoteAudioEnabled)
                    .toggleStyle(.switch)
                    .controlSize(.small)
                if connection.hostAudioMuted {
                    Text("被控端当前处于系统静音状态。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(14)
            .frame(width: 248)
            .background(Color(nsColor: .windowBackgroundColor))
        }
    }

    /// Round trips are reported in whole milliseconds, so a local link reads 0.
    private static func latencyText(_ milliseconds: Int) -> String {
        milliseconds < 1 ? "<1 ms" : "\(milliseconds) ms"
    }

    private var networkControls: some View {
        let rtt = connection.networkStats?.rttMS
        return Button { connection.showNetworkStats.toggle() } label: {
            Label(rtt.map(Self.latencyText) ?? "网络", systemImage: "waveform.path")
                .font(.caption.monospacedDigit())
                .foregroundStyle(rtt.map { $0 < 100 ? Color.green : Color.orange } ?? .secondary)
                .fixedSize()
        }
        .buttonStyle(.borderless)
        .accessibilityLabel("会话网络状态")
        .help("查看延迟与画面传输")
        .popover(isPresented: $connection.showNetworkStats, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 10) {
                Text("会话网络").font(.headline)
                networkRow("网络延迟", rtt.map(Self.latencyText) ?? "—")
                networkRow(connection.networkStats?.isVideo == true ? "首帧耗时" : "帧请求耗时",
                           connection.networkStats.map { "\($0.frameMS) ms" } ?? "—")
                networkRow("接收帧率", connection.networkStats?.framesPerSecond.map { String(format: "%.1f fps", $0) } ?? "—")
                networkRow("画面数据速率", connection.networkStats?.kilobitsPerSecond.map { String(format: "%.0f Kbps", $0) } ?? "—")
                Divider()
                networkRow("连接路径", connection.networkStats?.direct.map { $0 ? "QUIC 直连" : "QUIC 中继" } ?? "—")
                networkRow("被控端地址", connection.session?.address ?? "—")
            }
            .padding(16)
            .frame(width: 274)
            .background(Color(nsColor: .windowBackgroundColor))
        }
    }

    private func networkRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).foregroundStyle(.secondary)
            Spacer()
            Text(value).monospacedDigit().textSelection(.enabled)
        }
        .font(.callout)
    }

    private var controlCenter: some View {
        Button { connection.showControlCenter.toggle() } label: {
            Label("控制中心", systemImage: "slider.horizontal.3")
        }
        .buttonStyle(.borderless)
        .popover(isPresented: $connection.showControlCenter, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("画面设置").font(.headline)
                    Spacer()
                    if let size = connection.displayedSize {
                        Text("\(Int(size.width)) × \(Int(size.height))")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
                Text("传输分辨率").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(FrameResolution.allCases) { resolution in
                    choice(resolution.title(for: connection.selectedDisplay), selected: resolution == connection.frameResolution) {
                        connection.frameResolution = resolution
                    }
                }
                Divider()
                Text("画质").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(FrameQuality.allCases) { quality in
                    choice(quality.title, selected: quality == connection.frameQuality) {
                        connection.frameQuality = quality
                    }
                }
                Divider()
                Picker("目标帧率", selection: $connection.targetFPS) {
                    ForEach(LocalConnection.frameRates, id: \.self) { fps in
                        Text("\(fps) FPS").tag(fps)
                    }
                }
                .pickerStyle(.menu)
                .disabled(!connection.supportsVideo)
                Divider()
                HStack {
                    Menu("快捷键") {
                        ForEach(keys.indices, id: \.self) { index in
                            Button(keys[index].0) {
                                let key = keys[index].1
                                Task {
                                    if ["copy", "paste"].contains(key) { await connection.clipboardShortcut(key) }
                                    else { await connection.sendInput(.init(action: "key", key: key)) }
                                }
                            }
                        }
                    }
                    .fixedSize()
                    .disabled(connection.isViewOnly)
                    Button("发送文本") {
                        connection.showControlCenter = false
                        connection.showTextControls = true
                    }
                    .disabled(connection.isViewOnly)
                    Button("全屏") {
                        connection.showControlCenter = false
                        NSApp.keyWindow?.toggleFullScreen(nil)
                    }
                    Spacer()
                    Button("结束", role: .destructive) { connection.disconnect() }
                }
                if let ip = connection.session?.address.split(separator: ":").first,
                   let url = URL(string: "vnc://\(ip)"),
                   let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.ScreenSharing") {
                    Button("在 macOS 屏幕共享中打开") {
                        connection.showControlCenter = false
                        NSWorkspace.shared.open(
                            [url], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil
                        )
                    }
                    .help("被控 Mac 已开启系统屏幕共享时，可在系统应用中用允许访问的账户登录。")
                }
            }
            .padding(14)
            .frame(width: 312)
            .background(Color(nsColor: .windowBackgroundColor))
        }
    }

    private func choice(_ title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(title)
                Spacer(minLength: 12)
                if selected { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(selected ? Color.accentColor.opacity(0.1) : .clear, in: RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct RemoteCanvas: NSViewRepresentable {
    let image: NSImage?
    let videoSize: CGSize?
    let videoSurface: VideoSurface
    let display: UInt32?
    let acceptsKeyboard: Bool
    let clipboard: (String) -> Void
    let pointer: (RemotePointerEvent, UInt32) -> Void
    let resize: (Int) -> Void

    func makeNSView(context: Context) -> CanvasView {
        let canvas = CanvasView()
        canvas.onClipboard = clipboard
        canvas.onPointer = pointer
        canvas.onResize = resize
        canvas.wantsLayer = true
        videoSurface.layer.videoGravity = .resizeAspect
        canvas.layer?.addSublayer(videoSurface.layer)
        canvas.videoLayer = videoSurface.layer
        return canvas
    }

    func updateNSView(_ view: CanvasView, context: Context) {
        view.displayID = display
        view.acceptsKeyboard = acceptsKeyboard
        view.videoSize = videoSize
        view.image = image
        view.needsDisplay = true
    }
}

@MainActor
private final class CanvasView: NSView {
    var image: NSImage?
    var videoLayer: AVSampleBufferDisplayLayer?
    var acceptsKeyboard = true
    var videoSize: CGSize? {
        didSet {
            if videoSize != oldValue {
                videoLayer?.isHidden = videoSize == nil
                needsLayout = true
            }
        }
    }
    var displayID: UInt32? {
        didSet {
            if oldValue != displayID {
                pressed.removeAll()
                pressedKeys.removeAll()
            }
        }
    }
    var onClipboard: ((String) -> Void)?
    var onPointer: ((RemotePointerEvent, UInt32) -> Void)?
    var onResize: ((Int) -> Void)?
    private var pressed = Set<RemoteMouseButton>()
    private var pressedKeys = Set<UInt16>()
    private var tracking: NSTrackingArea?

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    private var imageRect: CGRect {
        guard let size = videoSize ?? image?.size, size.width > 0, size.height > 0 else { return .zero }
        let scale = min(bounds.width / size.width, bounds.height / size.height)
        let width = size.width * scale
        let height = size.height * scale
        return CGRect(x: bounds.midX - width / 2, y: bounds.midY - height / 2, width: width, height: height)
    }

    override func draw(_ dirtyRect: NSRect) {
        if videoSize == nil {
            image?.draw(in: imageRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        videoLayer?.frame = bounds
        CATransaction.commit()
        let pixels = convertToBacking(bounds).size
        let fitted = imageRect
        guard fitted.width > 0 && fitted.height > 0 else {
            onResize?(Int(max(pixels.width, pixels.height)))
            return
        }
        let imagePixels = convertToBacking(fitted).size
        onResize?(Int(max(imagePixels.width, imagePixels.height)))
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        needsLayout = true
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        NotificationCenter.default.removeObserver(self, name: NSWindow.didResignKeyNotification, object: nil)
        if let window {
            window.acceptsMouseMovedEvents = true
            NotificationCenter.default.addObserver(
                self, selector: #selector(windowResigned), name: NSWindow.didResignKeyNotification, object: window
            )
        }
    }

    @objc private func windowResigned() {
        guard !pressed.isEmpty || !pressedKeys.isEmpty, let displayID else { return }
        pressed.removeAll()
        pressedKeys.removeAll()
        onPointer?(.init(kind: .release), displayID)
    }

    override func resignFirstResponder() -> Bool {
        windowResigned()
        return super.resignFirstResponder()
    }

    override func keyDown(with event: NSEvent) {
        if !forwardKey(event, down: true) { super.keyDown(with: event) }
    }

    override func keyUp(with event: NSEvent) {
        if !forwardKey(event, down: false) { super.keyUp(with: event) }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.firstResponder === self, event.type == .keyDown,
              event.modifierFlags.contains(.command) else { return super.performKeyEquivalent(with: event) }
        if ["q", "w"].contains(event.charactersIgnoringModifiers?.lowercased() ?? "") {
            return super.performKeyEquivalent(with: event)
        }
        return forwardKey(event, down: true) || super.performKeyEquivalent(with: event)
    }

    private func forwardKey(_ event: NSEvent, down: Bool) -> Bool {
        guard acceptsKeyboard, let displayID, event.keyCode < 128 else { return false }
        let modifiers = event.modifierFlags.intersection([.command, .option, .control, .shift])
        if down, modifiers == .command, let character = event.charactersIgnoringModifiers?.lowercased(),
           character == "c" || character == "v" {
            if !event.isARepeat { onClipboard?(character == "c" ? "copy" : "paste") }
            return true
        }
        let code = event.keyCode
        if down { pressedKeys.insert(code) } else if pressedKeys.remove(code) == nil { return false }
        let flags = UInt8(truncatingIfNeeded: event.modifierFlags.rawValue >> 16) & 0xbf
        onPointer?(.init(kind: down ? .keyDown : .keyUp, code: code, flags: flags, repeat: down ? event.isARepeat : nil), displayID)
        return true
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(
            rect: .zero, options: [.inVisibleRect, .activeInKeyWindow, .mouseMoved, .mouseEnteredAndExited],
            owner: self, userInfo: nil
        )
        addTrackingArea(area)
        tracking = area
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseEntered(with event: NSEvent) { handle(event, kind: .move) }
    override func mouseMoved(with event: NSEvent) { handle(event, kind: .move) }
    override func mouseDown(with event: NSEvent) { handle(event, kind: .down) }
    override func mouseUp(with event: NSEvent) { handle(event, kind: .up) }
    override func mouseDragged(with event: NSEvent) { handle(event, kind: .move) }
    override func rightMouseDown(with event: NSEvent) { handle(event, kind: .down) }
    override func rightMouseUp(with event: NSEvent) { handle(event, kind: .up) }
    override func rightMouseDragged(with event: NSEvent) { handle(event, kind: .move) }
    override func otherMouseDown(with event: NSEvent) { handle(event, kind: .down) }
    override func otherMouseUp(with event: NSEvent) { handle(event, kind: .up) }
    override func otherMouseDragged(with event: NSEvent) { handle(event, kind: .move) }
    override func scrollWheel(with event: NSEvent) {
        if window?.isKeyWindow == true { handle(event, kind: .scroll) }
    }

    private func handle(_ event: NSEvent, kind: RemotePointerEvent.Kind) {
        guard let displayID else { return }
        let button: RemoteMouseButton? = switch event.buttonNumber {
        case 0: .left
        case 1: .right
        case 2: .middle
        case 3: .back
        case 4: .forward
        default: nil
        }
        if kind == .up {
            guard let button, pressed.remove(button) != nil else { return }
        }

        let rect = imageRect
        guard rect.width > 0, rect.height > 0 else {
            if kind == .up { onPointer?(.init(kind: .release), displayID) }
            return
        }
        let point = convert(event.locationInWindow, from: nil)
        guard rect.contains(point) || kind == .up || (kind == .move && !pressed.isEmpty) else { return }

        var action = RemotePointerEvent(
            kind: kind,
            x: min(9999, max(0, Int((point.x - rect.minX) / rect.width * 10_000))),
            y: min(9999, max(0, Int((point.y - rect.minY) / rect.height * 10_000)))
        )
        switch kind {
        case .down:
            guard let button else { return }
            window?.makeFirstResponder(self)
            pressed.insert(button)
            action.button = button
            action.clicks = min(3, max(1, event.clickCount))
        case .up:
            action.button = button
        case .scroll:
            let scale = event.hasPreciseScrollingDeltas ? 1.0 : 12.0
            action.dx = Int(Int16(clamping: Int((event.scrollingDeltaX * scale).rounded(.awayFromZero))))
            action.dy = Int(Int16(clamping: Int((event.scrollingDeltaY * scale).rounded(.awayFromZero))))
            if action.dx == 0 && action.dy == 0 { return }
        default:
            break
        }
        onPointer?(action, displayID)
    }
}
