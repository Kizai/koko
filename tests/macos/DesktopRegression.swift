import AppKit
import AudioToolbox
import CoreMedia
import CoreVideo
import Metal
import SwiftUI

@main
struct DesktopRegression {
    @MainActor static func main() async throws {
        _ = NSApplication.shared
        try checkPermissionUpgrade()
        try checkPermissionRefresh()
        try checkAdvancedSettings()
        try await checkReleaseUpdates()
        if let version = ProcessInfo.processInfo.environment["KOKO_TEST_LIVE_UPDATES"] {
            try await checkLiveReleaseUpdates(installedVersion: version)
        }
        try checkRemoteTitleBar()
        try await checkHelperPipes()
        try checkClipboardItems()
        try checkAgentPolicy()
        try checkUpdateProgress()
        try checkMediaPipeline()
        try checkVideoFlow()
        for chroma444 in [true, false] { try await checkVideoRendering(chroma444: chroma444) }
        print("通过发布更新检查、升级权限检测、高级设置、顶栏拖动、双击最大化与还原、管道背压、辅助进程退出、剪贴板读写、更新进度、音视频编解码回路、视频流控、Metal 解码直出和 Agent 高危操作规则测试")
    }

    struct Failure: LocalizedError {
        let errorDescription: String?
        init(_ message: String) { errorDescription = message }
    }

    private static func checkPermissionUpgrade() throws {
        let name = "koko.tests.permissions." + UUID().uuidString
        guard let defaults = UserDefaults(suiteName: name) else { throw Failure("无法隔离测试偏好") }
        defer { defaults.removePersistentDomain(forName: name) }
        func lost(_ version: String, screen: Bool = true, input: Bool = true) -> Bool {
            DesktopPermissionHistory.record(version: version, screen: screen, input: input, in: defaults)
        }
        guard !lost("0.1.15"), !lost("0.1.16"),
              lost("0.1.17", screen: false), !lost("0.1.17", screen: false),
              lost("0.1.18", screen: false, input: false),
              !lost("0.1.19", screen: false, input: false)
        else { throw Failure("仅在升级后丢失已授权能力时应提醒一次") }
    }

    @MainActor private static func checkPermissionRefresh() throws {
        let connection = LocalConnection()
        connection.refreshDesktopPermissions()
        var changes = 0
        let observer = connection.objectWillChange.sink { changes += 1 }
        defer { observer.cancel() }
        connection.refreshDesktopPermissions()
        guard changes == 0 else { throw Failure("权限未变化时，复查不应触发界面刷新") }
    }

    private static func checkAdvancedSettings() throws {
        var latency = StreamLatency()
        for sample in [180, 30, 185, 190] { latency.observe(sample) }
        guard !latency.constrained else { throw Failure("单次延迟波动不应切换画质") }
        latency.observe(151)
        guard latency.constrained else { throw Failure("持续高延迟时应降低自动画质") }

        // A host missing from one LAN scan stays online; one silent for longer goes offline.
        let start = ContinuousClock.now
        var seen: [String: ContinuousClock.Instant] = [:]
        let mac = NearbyHost(id: "a", label: "Mac", request: "", os: "macos")
        var hosts = LocalConnection.mergeScan([mac], previous: [], lastSeen: &seen, now: start)
        hosts = LocalConnection.mergeScan([], previous: hosts, lastSeen: &seen, now: start + .seconds(8))
        guard hosts.map(\.id) == ["a"] else { throw Failure("单次扫描漏报不应立即显示离线") }
        hosts = LocalConnection.mergeScan([], previous: hosts, lastSeen: &seen, now: start + LocalConnection.offlineAfter)
        guard hosts.isEmpty, seen.isEmpty else { throw Failure("长时间未响应的设备应显示离线") }
        // Background scans answer in any order; the list must not reshuffle.
        let air = NearbyHost(id: "b", label: "Air", request: "", os: "macos")
        hosts = LocalConnection.mergeScan([mac, air], previous: [], lastSeen: &seen, now: start)
        hosts = LocalConnection.mergeScan([air, mac], previous: hosts, lastSeen: &seen, now: start + .seconds(8))
        guard hosts.map(\.id) == ["a", "b"] else { throw Failure("重新扫描不应打乱设备顺序") }
        seen.removeAll()

        // Overlay-network devices answer direct probes; one missed probe keeps them online.
        var reached: [String: ContinuousClock.Instant] = [:]
        var online = LocalConnection.mergeReach(["tail"], lastReached: &reached, now: start)
        online = LocalConnection.mergeReach([], lastReached: &reached, now: start + .seconds(8))
        guard online == ["tail"] else { throw Failure("组网设备单次探测失败不应立即显示离线") }
        online = LocalConnection.mergeReach([], lastReached: &reached, now: start + LocalConnection.offlineAfter)
        guard online.isEmpty, reached.isEmpty else { throw Failure("组网设备长时间无法连通应显示离线") }

        // A 60 Hz display delivers frames about 16.7 ms apart with jitter; none may be lost.
        var pacer = FramePacer(fps: 60)
        let jittered = (0..<60).map { Double($0) / 60 + ($0.isMultiple(of: 2) ? 0.0015 : -0.0015) }
        guard jittered.filter({ pacer.accepts($0) }).count == 60 else { throw Failure("60 Hz 抖动下不应丢帧") }
        // A 120 Hz display must still be paced down to the target rate.
        var fast = FramePacer(fps: 60)
        guard (0..<120).map({ Double($0) / 120 }).filter({ fast.accepts($0) }).count == 60
        else { throw Failure("120 Hz 屏幕应按目标帧率降到 60 fps") }
        guard FramePacer.captureInterval(fps: 60) < CMTime(value: 1, timescale: 60)
        else { throw Failure("采集间隔应短于目标帧间隔，避免系统丢帧") }
        for sample in [70, 95, 72, 72, 72, 72] { latency.observe(sample) }
        guard latency.constrained else { throw Failure("网络恢复前应维持低带宽档位") }
        latency.observe(72)
        guard !latency.constrained else { throw Failure("延迟持续恢复后应回到正常画质") }

        guard windowsShortcut(code: 8, flags: 4) == (8, 16), // Ctrl+C → ⌘C
              windowsShortcut(code: 55, flags: 16) == (59, 4),
              windowsShortcut(code: 62, flags: 4) == (54, 16),
              windowsShortcut(code: 58, flags: 8) == (58, 8),
              windowsShortcut(code: 8, flags: 30) == (8, 30)
        else { throw Failure("Windows 修饰键映射错误") }
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

    /// Encodes a red frame, decodes it with VideoToolbox and draws it with the Metal shader, then
    /// reads the middle pixel back: catches wrong planes, range or color matrix in the renderer.
    private static func checkVideoRendering(chroma444: Bool) async throws {
        let encoder = try VideoEncoder(width: 320, height: 180, fps: 60, quality: 90,
                                       preferHEVC: VideoCapabilities.supportsHEVC,
                                       prefer444: chroma444 && VideoCapabilities.supportsHEVC444)
        var created: CVPixelBuffer?
        let attributes = [kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary] as CFDictionary
        guard CVPixelBufferCreate(nil, 320, 180, encoder.pixelFormat, attributes, &created) == kCVReturnSuccess,
              let pixels = created else { throw Failure("无法创建测试画面") }
        // Rec. 709 full range red: Y 54, Cb 99, Cr 255.
        CVPixelBufferLockBaseAddress(pixels, [])
        for plane in 0..<2 {
            let base = CVPixelBufferGetBaseAddressOfPlane(pixels, plane)!.assumingMemoryBound(to: UInt8.self)
            let rows = CVPixelBufferGetHeightOfPlane(pixels, plane), stride = CVPixelBufferGetBytesPerRowOfPlane(pixels, plane)
            let width = CVPixelBufferGetWidthOfPlane(pixels, plane)
            for row in 0..<rows {
                for column in 0..<width {
                    if plane == 0 {
                        base[row * stride + column] = 54
                    } else {
                        base[row * stride + column * 2] = 99
                        base[row * stride + column * 2 + 1] = 255
                    }
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(pixels, [])
        CVBufferSetAttachment(pixels, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, .shouldPropagate)
        var description: CMVideoFormatDescription?
        CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: pixels, formatDescriptionOut: &description)
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 60), presentationTimeStamp: .zero,
                                        decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard let description, CMSampleBufferCreateReadyWithImageBuffer(
            allocator: nil, imageBuffer: pixels, formatDescription: description, sampleTiming: &timing, sampleBufferOut: &sample
        ) == noErr, let sample else { throw Failure("无法创建测试帧") }
        guard let encoded = try await encoder.encode(sample, keyFrame: true), let format = encoded.formatDescription
        else { throw Failure("测试帧编码失败") }

        let decoder = try VideoDecoder(format: format)
        let (status, decoded) = decoder.decode(encoded)
        guard status == noErr, let decoded else { throw Failure("VideoToolbox 解码失败：\(status)") }
        guard decoder.isHardware || !VideoCapabilities.supportsHEVC else { throw Failure("支持硬件解码的机器应使用硬解") }

        let drawing = try VideoDrawing()
        let target = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: 160, height: 90, mipmapped: false)
        target.usage = [.renderTarget]
        target.storageMode = .managed
        guard let texture = drawing.device.makeTexture(descriptor: target),
              let queue = drawing.device.makeCommandQueue(), let commands = queue.makeCommandBuffer()
        else { throw Failure("无法创建 Metal 目标") }
        let textures = drawing.encode(decoded, into: texture, commands: commands)
        guard !textures.isEmpty else { throw Failure("Metal 无法读取解码后的画面") }
        let blit = commands.makeBlitCommandEncoder()!
        blit.synchronize(resource: texture)
        blit.endEncoding()
        commands.commit()
        await commands.completed()
        var bgra = [UInt8](repeating: 0, count: 4)
        texture.getBytes(&bgra, bytesPerRow: 160 * 4, from: MTLRegionMake2D(80, 45, 1, 1), mipmapLevel: 0)
        guard bgra[2] > 225, bgra[1] < 30, bgra[0] < 30 else {
            throw Failure("Metal 颜色转换不正确（\(chroma444 ? "4:4:4" : "4:2:0")）：BGRA \(bgra)")
        }
    }

    private static func checkVideoFlow() throws {
        // On a LAN the window covers a round trip plus two frames, so frames never queue up.
        var flow = VideoFlow(fps: 60, bitrate: 50_000_000)
        var now = 0.0
        var sent: [UInt32] = []
        while flow.admits(now: now) { sent.append(flow.send(now: now)) }
        guard (2...8).contains(sent.count) else { throw Failure("未确认的帧数应受窗口限制：\(sent.count)") }
        flow.acknowledge(sent.last!, now: now + 0.004)
        guard flow.admits(now: now + 0.004) else { throw Failure("确认后应继续发送") }
        guard flow.window <= 4 else { throw Failure("局域网下在途帧应只有几帧：\(flow.window)") }

        // A link that keeps answering slower than its best round trip is congested.
        for step in 1...40 {
            now = Double(step) * 0.05
            let frame = flow.send(now: now)
            flow.acknowledge(frame, now: now + 0.004 + min(0.12, Double(step) * 0.01))
        }
        guard let lowered = flow.takeBitrateChange(), lowered < 50_000_000, lowered >= flow.minimumBitrate
        else { throw Failure("排队延迟增长时应降低码率") }
        // Once quiet again, the bitrate climbs back toward the start.
        for step in 41...200 {
            now = Double(step) * 0.05
            let frame = flow.send(now: now)
            flow.acknowledge(frame, now: now + 0.004)
        }
        guard flow.bitrate > lowered else { throw Failure("网络恢复后应逐步提高码率") }

        // One slow frame among quick ones is not congestion.
        var steady = VideoFlow(fps: 60, bitrate: 50_000_000)
        for step in 0..<120 {
            let at = Double(step) / 60
            let frame = steady.send(now: at)
            steady.acknowledge(frame, now: at + (step == 60 ? 0.08 : 0.003))
        }
        guard steady.bitrate == 50_000_000 else { throw Failure("单个慢帧不应降低码率：\(steady.bitrate)") }

        // A still screen sends almost nothing; the first frame after it is back at full bitrate.
        for step in 201...240 {
            now = Double(step) * 0.05
            let frame = flow.send(now: now)
            flow.acknowledge(frame, now: now + 0.004 + min(0.12, Double(step - 200) * 0.01))
        }
        guard flow.bitrate < 50_000_000 else { throw Failure("再次拥塞时应降低码率") }
        _ = flow.takeBitrateChange()
        guard flow.admits(now: now + 1.5), flow.takeBitrateChange() == 50_000_000 else {
            throw Failure("画面静止一段时间后应恢复满码率")
        }

        // A controller without feedback is never held back for long.
        var silent = VideoFlow(fps: 60, bitrate: 10_000_000)
        for _ in 0..<10 { _ = silent.send(now: 0) }
        guard !silent.admits(now: 1), silent.admits(now: 2.5) else { throw Failure("不回确认的控制端不应一直被限流") }

        // Key frame requests close together are served by one key frame.
        silent.requestKeyFrame()
        guard silent.takeKeyFrame(now: 3) else { throw Failure("应响应关键帧请求") }
        silent.requestKeyFrame()
        guard !silent.takeKeyFrame(now: 3.2), !silent.takeKeyFrame(now: 3.8) else { throw Failure("关键帧请求应去抖") }

        // Feedback records split across reads are parsed whole; garbage stops the parser.
        var buffer = Data([0, 0, 0, 5, 1, 0, 0, 1, 2, 0, 0, 0, 1])
        guard let first = takeVideoFeedback(from: &buffer), first.count == 1, first[0].1 == 258, buffer.count == 4
        else { throw Failure("应解析完整的确认记录并保留不完整部分") }
        buffer.append(2)
        guard let second = takeVideoFeedback(from: &buffer), second.count == 1, second[0].0 == .keyFrame, buffer.isEmpty
        else { throw Failure("应解析关键帧请求") }
        var garbage = Data([0, 0, 0, 9, 1])
        guard takeVideoFeedback(from: &garbage) == nil else { throw Failure("长度不符的记录应被拒绝") }
    }

    @MainActor private static func checkMediaPipeline() throws {
        let encoder = try LowDelayAACEncoder()
        guard let decoder = LowDelayAACDecoder() else { throw Failure("无法初始化音频解码器") }
        var packets: [Data] = []
        var phase = 0.0
        let step = 2.0 * Double.pi * 440.0 / 48_000.0
        for _ in 0..<500 {
            var left = [Float](repeating: 0, count: 960)
            var right = [Float](repeating: 0, count: 960)
            for i in 0..<960 {
                let s = Float(sin(phase) * 0.4)
                left[i] = s
                right[i] = s
                phase += step
            }
            packets.append(contentsOf: encoder.encodePlanar(left: left, right: right))
        }
        guard let header = encoder.formatHeader, decoder.configure(header: header) else {
            throw Failure("AAC 编码器未生成有效的格式头")
        }
        let expectedPackets = (500 * 960) / encoder.framesPerPacket
        guard packets.count == expectedPackets else {
            throw Failure("音频包数量不连续：期望 \(expectedPackets)，实际 \(packets.count)")
        }
        var decodedSamples = 0
        for packet in packets {
            guard let pcm = decoder.decode(frame: packet) else {
                throw Failure("AAC 音频包解码失败")
            }
            decodedSamples += Int(pcm.frameLength)
        }
        guard decodedSamples == packets.count * encoder.framesPerPacket else {
            throw Failure("解码采样数与编码采样数不一致：\(decodedSamples)")
        }

        let video = try VideoEncoder(
            width: 1280, height: 720, fps: 60, quality: 82,
            preferHEVC: VideoCapabilities.supportsHEVC,
            prefer444: VideoCapabilities.supportsHEVC444
        )
        if VideoCapabilities.supportsHEVC {
            guard video.isHEVC else { throw Failure("支持硬件 HEVC 的机器未启用 HEVC 编码") }
        }
        print("媒体管线验证通过：音频格式=\(encoder.formatID == kAudioFormatMPEG4AAC_ELD ? "AAC-ELD(10ms)" : "AAC-LC") 包数=\(packets.count)，视频编码=\(video.isHEVC ? (video.pixelFormat == kCVPixelFormatType_444YpCbCr8BiPlanarFullRange ? "HEVC 4:4:4 FullRange" : "HEVC 4:2:0 FullRange") : "H.264")")
    }

    /// The pasteboard is read as paths or text and filled back from received items; the bytes are streamed in Rust.
    @MainActor private static func checkClipboardItems() throws {
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }

        board.clearContents()
        board.writeObjects([folder as NSURL])
        guard try ClipboardItems.read(from: board).paths == [folder.path] else { throw Failure("复制的文件夹应作为路径交给传输") }

        try ClipboardItems(text: "你好 KoKo").publish(on: board)
        guard try ClipboardItems.read(from: board) == ClipboardItems(text: "你好 KoKo") else { throw Failure("文字应原样往返") }

        // An image without a file (a screenshot to the clipboard) travels as a PNG file and
        // arrives as both the file and the picture.
        let pixel = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8,
                                     samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                     colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        board.clearContents()
        board.setData(pixel.tiffRepresentation!, forType: .tiff)
        let image = try ClipboardItems.read(from: board)
        guard image.text == nil, image.paths.count == 1, image.paths[0].hasSuffix(".png") else {
            throw Failure("剪贴板图片应保存为 PNG 文件再传输")
        }
        defer { try? FileManager.default.removeItem(at: URL(fileURLWithPath: image.paths[0]).deletingLastPathComponent()) }
        try image.publish(on: board)
        guard board.types?.contains(.fileURL) == true, board.data(forType: .png) != nil else {
            throw Failure("收到的图片应同时以文件和图片形式粘贴")
        }

        // Copied items wait in a cache, not the download folder, and expire after a day.
        guard !image.paths[0].hasPrefix(defaultDownloadDirectory),
              image.paths[0].hasPrefix(clipboardCacheRoot().path) else { throw Failure("剪贴板图片不应保存到下载文件夹") }
        let cache = folder.appendingPathComponent("cache", isDirectory: true)
        let stale = try makeClipboardFolder(root: cache)
        try FileManager.default.setAttributes([.creationDate: Date(timeIntervalSinceNow: -clipboardCacheLifetime - 60)],
                                              ofItemAtPath: stale.path)
        let recent = try makeClipboardFolder(root: cache)
        _ = try makeClipboardFolder(root: cache)
        guard !FileManager.default.fileExists(atPath: stale.path), FileManager.default.fileExists(atPath: recent.path)
        else { throw Failure("剪贴板缓存应只清理超过一天的内容") }

        board.clearContents()
        do {
            _ = try ClipboardItems.read(from: board)
            throw Failure("空的剪贴板不应开始传输")
        } catch ClipboardError.unsupported {}
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
