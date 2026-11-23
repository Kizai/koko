import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// While a controller is connected, a drop target appears at the screen edge whenever someone
/// starts dragging files on this Mac, including a controller dragging remotely. Files dropped on
/// it are offered to the controller, which downloads them.
@MainActor
final class SendPanel {
    enum Status: Equatable {
        case ready
        case waiting(count: Int)
        case sent
        case failed(String)
    }

    /// Called with the dropped files; the owner queues them and reports back through `status`.
    var onSend: ([URL]) -> Void = { _ in }
    private let model = SendPanelModel()
    private var panel: NSPanel?
    private var watcher: Task<Void, Never>?
    private var hider: Task<Void, Never>?

    var status: Status {
        get { model.status }
        set {
            model.status = newValue
            switch newValue {
            case .sent, .failed: hide(after: .seconds(3))
            case .waiting: hider?.cancel()
            case .ready: break
            }
        }
    }

    /// Watches the system drag pasteboard. No permission is needed to see that a file drag began.
    func startWatching() {
        guard watcher == nil else { return }
        watcher = Task { [weak self] in
            let board = NSPasteboard(name: .drag)
            var seen = board.changeCount
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(150))
                guard let self else { return }
                let pressed = NSEvent.pressedMouseButtons & 1 != 0
                if pressed, board.changeCount != seen {
                    seen = board.changeCount
                    if board.types?.contains(.fileURL) == true { self.show() }
                } else if !pressed, self.panel?.isVisible == true, self.model.status == .ready, !self.model.targeted {
                    self.hide(after: .milliseconds(800))
                }
            }
        }
    }

    func stopWatching() {
        watcher?.cancel()
        watcher = nil
        hider?.cancel()
        panel?.orderOut(nil)
    }

    private func show() {
        hider?.cancel()
        model.status = .ready
        let panel = self.panel ?? makePanel()
        self.panel = panel
        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main
        if let frame = screen?.visibleFrame {
            panel.setFrameOrigin(CGPoint(x: frame.maxX - panel.frame.width - 16, y: frame.midY - panel.frame.height / 2))
        }
        panel.orderFrontRegardless()
    }

    private func hide(after delay: Duration) {
        hider?.cancel()
        hider = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self, !self.model.targeted else { return }
            if case .waiting = self.model.status { return }
            self.panel?.orderOut(nil)
            self.model.status = .ready
        }
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 220, height: 170),
                            styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: true)
        panel.level = .floating
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.contentView = NSHostingView(rootView: SendPanelView(model: model) { [weak self] urls in
            self?.model.status = .waiting(count: urls.count)
            self?.onSend(urls)
        })
        return panel
    }
}

@MainActor
private final class SendPanelModel: ObservableObject {
    @Published var status = SendPanel.Status.ready
    @Published var targeted = false
}

private struct SendPanelView: View {
    @ObservedObject var model: SendPanelModel
    let drop: ([URL]) -> Void

    var body: some View {
        VStack(spacing: 10) {
            switch model.status {
            case .ready:
                Image(systemName: "arrow.up.doc.on.clipboard").font(.system(size: 34))
                    .foregroundStyle(model.targeted ? Color.accentColor : .secondary)
                Text("拖到这里发送给控制端").font(.callout.weight(.medium))
            case .waiting(let count):
                ProgressView().controlSize(.small)
                Text("正在发送 \(count) 项…").font(.callout)
            case .sent:
                Image(systemName: "checkmark.circle.fill").font(.system(size: 34)).foregroundStyle(.green)
                Text("已发送到控制端").font(.callout.weight(.medium))
            case .failed(let message):
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 30)).foregroundStyle(.orange)
                Text(message).font(.caption).multilineTextAlignment(.center)
            }
        }
        .padding(16)
        .frame(width: 220, height: 170)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
        .overlay(
            RoundedRectangle(cornerRadius: 16)
                .strokeBorder(model.targeted ? Color.accentColor : Color.secondary.opacity(0.3),
                              style: StrokeStyle(lineWidth: 2, dash: model.targeted ? [] : [6, 4]))
        )
        .onDrop(of: [.fileURL], isTargeted: $model.targeted) { providers in
            Task { @MainActor in
                var urls: [URL] = []
                for provider in providers {
                    if let data = try? await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? Data,
                       let url = URL(dataRepresentation: data, relativeTo: nil) {
                        urls.append(url)
                    }
                }
                if !urls.isEmpty { drop(urls) }
            }
            return true
        }
    }
}
