import ApplicationServices
import CoreImage
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

enum ScreenCaptureError: String, Error {
    case permissionRequired = "screen_permission_required"
    case captureFailed = "capture_failed"
    case displayUnavailable = "display_unavailable"
}

private final class FrameOutput: NSObject, SCStreamOutput, @unchecked Sendable {
    let continuation: AsyncStream<CGImage>.Continuation
    private let context = CIContext()

    init(continuation: AsyncStream<CGImage>.Continuation) {
        self.continuation = continuation
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, buffer.isValid, let pixels = buffer.imageBuffer else { return }
        let frame = CIImage(cvPixelBuffer: pixels)
        if let image = context.createCGImage(frame, from: frame.extent) {
            continuation.yield(image)
            continuation.finish()
        }
    }
}

func screenCaptureSource(
    displayID: CGDirectDisplayID?, edge: Int
) async throws -> (SCContentFilter, SCStreamConfiguration) {
    guard CGPreflightScreenCaptureAccess() else { throw ScreenCaptureError.permissionRequired }
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    let screen = displayID ?? CGMainDisplayID()
    guard let display = content.displays.first(where: { $0.displayID == screen }) else {
        throw ScreenCaptureError.displayUnavailable
    }

    let filter = SCContentFilter(display: display, excludingWindows: [])
    let config = SCStreamConfiguration()
    let mode = CGDisplayCopyDisplayMode(screen)
    let width = mode?.pixelWidth ?? display.width
    let height = mode?.pixelHeight ?? display.height
    let scale = min(1, Double(edge) / Double(max(width, height)))
    config.width = max(1, Int(Double(width) * scale))
    config.height = max(1, Int(Double(height) * scale))
    config.pixelFormat = kCVPixelFormatType_32BGRA
    config.capturesAudio = false
    return (filter, config)
}

func captureScreen(
    displayID: CGDirectDisplayID? = nil, edge: Int = 1600, quality: Int? = nil, showsCursor: Bool = true
) async throws -> Data {
    let (filter, config) = try await screenCaptureSource(displayID: displayID, edge: edge)
    config.showsCursor = showsCursor

    let image: CGImage
    if #available(macOS 14, *) {
        image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
    } else {
        let (frames, continuation) = AsyncStream<CGImage>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let output = FrameOutput(continuation: continuation)
        defer { withExtendedLifetime(output) {} }
        let stream = SCStream(filter: filter, configuration: config, delegate: nil)
        try stream.addStreamOutput(output, type: .screen, sampleHandlerQueue: DispatchQueue(label: "Koko.capture"))
        try await stream.startCapture()
        do {
            image = try await withThrowingTaskGroup(of: CGImage.self) { group in
                group.addTask {
                    for await frame in frames { return frame }
                    throw ScreenCaptureError.captureFailed
                }
                group.addTask {
                    try await Task.sleep(for: .seconds(6))
                    throw ScreenCaptureError.captureFailed
                }
                defer { group.cancelAll() }
                guard let frame = try await group.next() else { throw ScreenCaptureError.captureFailed }
                return frame
            }
        } catch {
            try? await stream.stopCapture()
            throw error
        }
        try await stream.stopCapture()
    }

    let data = NSMutableData()
    let format = quality == nil ? UTType.png : UTType.jpeg
    guard let destination = CGImageDestinationCreateWithData(data, format.identifier as CFString, 1, nil)
    else { throw ScreenCaptureError.captureFailed }
    let properties = quality.map { [kCGImageDestinationLossyCompressionQuality: Double($0) / 100] as CFDictionary }
    CGImageDestinationAddImage(destination, image, properties)
    guard CGImageDestinationFinalize(destination) else { throw ScreenCaptureError.captureFailed }
    return data as Data
}

func hasScreenCapturePermission(request: Bool) -> Bool {
    if request && !CGPreflightScreenCaptureAccess() { _ = CGRequestScreenCaptureAccess() }
    return CGPreflightScreenCaptureAccess()
}
