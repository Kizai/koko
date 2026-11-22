import AppKit
import SwiftUI

@main
struct DesktopRegression {
    @MainActor static func main() async throws {
        _ = NSApplication.shared
        try checkRemoteTitleBar()
        try await checkHelperPipes()
        try await checkDeniedClipboardFile()
        print("通过顶栏拖动、双击最大化与还原、管道背压、辅助进程退出和文件访问被拒绝测试")
    }

    struct Failure: LocalizedError {
        let errorDescription: String?
        init(_ message: String) { errorDescription = message }
    }

    @MainActor private static func checkRemoteTitleBar() throws {
        let window = NSWindow(
            contentRect: NSRect(x: 80, y: 80, width: 870, height: 600),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered, defer: false
        )
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.contentView = NSHostingView(rootView: RemoteControlView(connection: LocalConnection()))
        window.orderBack(nil)
        defer { window.orderOut(nil) }
        window.contentView?.layoutSubtreeIfNeeded()
        let before = window.frame.origin
        let anchor = window.convertPoint(toScreen: CGPoint(x: 550, y: window.frame.height - 16))
        let steps: [(NSEvent.EventType, CGFloat, CGFloat)] = [
            (.leftMouseDown, 0, 0), (.leftMouseDragged, 20, 10), (.leftMouseDragged, 60, 45), (.leftMouseUp, 60, 45)
        ]
        for (kind, dx, dy) in steps {
            let point = window.convertPoint(fromScreen: CGPoint(x: anchor.x + dx, y: anchor.y + dy))
            guard let event = NSEvent.mouseEvent(with: kind, location: point, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: 1) else { throw Failure("无法构造窗口事件") }
            window.sendEvent(event)
        }
        guard abs(window.frame.minX - before.x - 60) < 1, abs(window.frame.minY - before.y - 45) < 1 else {
            throw Failure("拖动标题栏后，窗口位置不正确")
        }

        let restoredFrame = window.frame
        for maximize in [true, false] {
            for count in 1...2 {
                for kind in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                    let point = CGPoint(x: 550, y: window.frame.height - 16)
                    guard let event = NSEvent.mouseEvent(with: kind, location: point, modifierFlags: [],
                        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                        context: nil, eventNumber: 0, clickCount: count, pressure: 1) else {
                        throw Failure("无法构造顶栏双击事件")
                    }
                    window.sendEvent(event)
                }
            }
            let expected = maximize ? window.screen?.visibleFrame : restoredFrame
            guard window.frame == expected else {
                throw Failure(maximize ? "双击顶栏没有最大化窗口" : "再次双击顶栏没有还原窗口")
            }
        }
    }

    @MainActor private static func checkDeniedClipboardFile() async throws {
        let files = FileManager.default
        let url = files.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data([1]).write(to: url)
        defer { try? files.removeItem(at: url) }
        try files.setAttributes([.posixPermissions: 0], ofItemAtPath: url.path)
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        guard board.writeObjects([url as NSURL]) else { throw Failure("无法准备剪贴板文件") }
        do {
            let source = try await ClipboardSource.read(from: board)
            _ = try await source.next()
        } catch {
            guard ClipboardError.from(error) == .permission else { throw error }
            return
        }
        throw Failure("读取被拒绝的文件应返回目录权限错误")
    }

    @MainActor private static func checkHelperPipes() async throws {
        let slow = try LocalCommand(executable: URL(fileURLWithPath: "/bin/sh"),
                                    arguments: ["-c", "sleep 1; cat >/dev/null"], interactive: true)
        let start = ContinuousClock.now
        let heartbeat = Task { @MainActor in ContinuousClock.now - start }
        try await slow.send(String(repeating: "x", count: 88_000))
        slow.finishInput()
        _ = try await slow.result()
        guard await heartbeat.value < .milliseconds(500) else { throw Failure("写入文件块阻塞了主线程") }

        let exited = try LocalCommand(executable: URL(fileURLWithPath: "/usr/bin/true"), arguments: [], interactive: true)
        _ = try await exited.result()
        do { try await exited.send("after exit") } catch { return }
        throw Failure("向已退出的辅助进程写入时，应返回错误")
    }
}
