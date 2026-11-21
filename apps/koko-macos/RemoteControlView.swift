import AppKit
import SwiftUI

struct RemoteControlView: View {
    @ObservedObject var connection: LocalConnection

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
                    display: connection.selectedDisplay?.id,
                    pointer: connection.queuePointer,
                    resize: connection.setViewportEdge
                )
                if connection.remoteFrame == nil {
                    Label(
                        connection.screenNotice?.message ?? "正在等待远端画面…",
                        systemImage: connection.screenNotice == nil ? "display" : "exclamationmark.triangle"
                    )
                    .foregroundStyle(.white.opacity(0.85))
                    .multilineTextAlignment(.center)
                    .padding(24)
                    .allowsHitTesting(false)
                }
            }
            if let notice = connection.inputNotice {
                Label(notice.message, systemImage: "cursorarrow.rays")
                    .font(.callout)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(.orange.opacity(0.15))
            }
        }
        .frame(minWidth: 670, idealWidth: 1060, minHeight: 410, idealHeight: 650)
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

    private var toolbar: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(connection.remoteHostName.isEmpty ? "远程 Mac" : connection.remoteHostName)
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Label("远程会话", systemImage: "link")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .frame(minWidth: 104, idealWidth: 164, maxWidth: 200, alignment: .leading)
            Divider().frame(height: 25)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Array(connection.displays.enumerated()), id: \.element.id) { index, display in
                        let selected = display.id == connection.selectedDisplay?.id
                        Button {
                            connection.selectDisplay(display)
                        } label: {
                            Label("\(index + 1)  \(display.name)", systemImage: "display")
                                .lineLimit(1)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .foregroundStyle(selected ? Color.accentColor : .secondary)
                                .background(selected ? Color.accentColor.opacity(0.12) : .clear, in: RoundedRectangle(cornerRadius: 6))
                        }
                        .buttonStyle(.plain)
                        .help("\(display.name) · \(display.width) × \(display.height)")
                        .accessibilityAddTraits(selected ? .isSelected : [])
                    }
                }
            }
            controlCenter.fixedSize()
        }
        .padding(.horizontal, 14)
        .frame(height: 55)
        .background(.regularMaterial)
    }

    private var controlCenter: some View {
        Menu {
            if let image = connection.remoteFrame {
                Text("当前画面 \(Int(image.size.width)) × \(Int(image.size.height))")
            }
            Picker("传输分辨率", selection: $connection.frameResolution) {
                ForEach(FrameResolution.allCases) { resolution in
                    Text(resolution.title(for: connection.selectedDisplay)).tag(resolution)
                }
            }
            Picker("画质", selection: $connection.frameQuality) {
                ForEach(FrameQuality.allCases) { quality in Text(quality.title).tag(quality) }
            }
            Divider()
            Menu("快捷键") {
                ForEach(keys.indices, id: \.self) { index in
                    Button(keys[index].0) {
                        Task { await connection.sendInput(.init(action: "key", key: keys[index].1)) }
                    }
                }
            }
            Button("发送文本…") { connection.showTextControls = true }
            Button("切换全屏") { NSApp.keyWindow?.toggleFullScreen(nil) }
            Divider()
            Button("结束远控", role: .destructive) { connection.disconnect() }
        } label: {
            Label("控制中心", systemImage: "slider.horizontal.3")
        }
        .menuStyle(.borderlessButton)
    }
}

private struct RemoteCanvas: NSViewRepresentable {
    let image: NSImage?
    let display: UInt32?
    let pointer: (RemotePointerEvent, UInt32) -> Void
    let resize: (Int) -> Void

    func makeNSView(context: Context) -> CanvasView {
        let canvas = CanvasView()
        canvas.onPointer = pointer
        canvas.onResize = resize
        return canvas
    }

    func updateNSView(_ view: CanvasView, context: Context) {
        view.displayID = display
        view.image = image
        view.needsDisplay = true
    }
}

@MainActor
private final class CanvasView: NSView {
    var image: NSImage?
    var displayID: UInt32? {
        didSet {
            if oldValue != displayID { pressed.removeAll() }
        }
    }
    var onPointer: ((RemotePointerEvent, UInt32) -> Void)?
    var onResize: ((Int) -> Void)?
    private var pressed = Set<RemoteMouseButton>()
    private var tracking: NSTrackingArea?

    override var isFlipped: Bool { true }

    private var imageRect: CGRect {
        guard let size = image?.size, size.width > 0, size.height > 0 else { return .zero }
        let scale = min(bounds.width / size.width, bounds.height / size.height)
        let width = size.width * scale
        let height = size.height * scale
        return CGRect(x: bounds.midX - width / 2, y: bounds.midY - height / 2, width: width, height: height)
    }

    override func draw(_ dirtyRect: NSRect) {
        image?.draw(in: imageRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }

    override func layout() {
        super.layout()
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
        guard !pressed.isEmpty, let displayID else { return }
        pressed.removeAll()
        onPointer?(.init(kind: .release), displayID)
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
