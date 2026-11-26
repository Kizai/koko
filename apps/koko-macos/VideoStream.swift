@preconcurrency import AVFoundation
import CoreGraphics
import Foundation
import Network
import os
@preconcurrency import ScreenCaptureKit
@preconcurrency import VideoToolbox

enum VideoCapabilities {
    static let supportsHEVC: Bool = VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC)

    static let supportsHEVC444: Bool = {
        #if arch(arm64)
        guard supportsHEVC else { return false }
        var session: VTCompressionSession?
        let spec: [CFString: Any] = [
            kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: true,
        ]
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_444YpCbCr8BiPlanarFullRange,
        ]
        let status = VTCompressionSessionCreate(
            allocator: nil, width: 256, height: 256, codecType: kCMVideoCodecType_HEVC,
            encoderSpecification: spec as CFDictionary, imageBufferAttributes: attrs as CFDictionary,
            compressedDataAllocator: nil, outputCallback: nil, refcon: nil, compressionSessionOut: &session
        )
        guard status == noErr, let session else { return false }
        defer { VTCompressionSessionInvalidate(session) }
        let profileSet = VTSessionSetProperty(
            session, key: kVTCompressionPropertyKey_ProfileLevel, value: "HEVC_Main444_AutoLevel" as CFString
        )
        guard profileSet == noErr, VTCompressionSessionPrepareToEncodeFrames(session) == noErr else {
            return false
        }
        return true
        #else
        return false
        #endif
    }()
}

struct VideoSettings: Equatable {
    let display: UInt32
    let edge: Int
    let quality: Int
    let fps: Int
    let audio: Bool
    let hevc: Bool
    let chroma444: Bool
    /// Whether the controller acknowledges frames and asks for key frames; see `VideoFlow`.
    var feedback = false
}

// Separate entry and recovery thresholds keep a borderline link from restarting the encoder every second.
struct StreamLatency {
    private(set) var constrained = false
    private var consecutive = 0

    mutating func observe(_ milliseconds: Int) {
        let shouldChange = constrained ? milliseconds < 85 : milliseconds >= 150
        consecutive = shouldChange ? consecutive + 1 : 0
        if consecutive >= (constrained ? 5 : 3) {
            constrained.toggle()
            consecutive = 0
        }
    }
}

// Each record is a big-endian length followed by a kind byte and its payload.
enum VideoRecord: UInt8 {
    case format = 1, frame, error, path, audioFormat, audioFrame, syncFrame, hevcFormat, sequencedFrame
    /// JSON describing the host's encoder and capture, sent to controllers with feedback.
    case streamInfo
}

/// A `streamInfo` record: the first names the encoder and capture, later ones only the bitrate.
struct VideoStreamInfo: Codable {
    var encoder: String? = nil
    var capture: String? = nil
    var bitrate: Int? = nil
}

/// Records a controller writes back on a video stream that has feedback.
enum VideoFeedback: UInt8 {
    case ack = 1, keyFrame
}

/// The host's side of a video stream with feedback. The controller acknowledges every frame it
/// receives, which tells the host how many frames are still on their way and how long a frame
/// takes to arrive. A frame is only captured while few enough are unacknowledged to cover one
/// round trip, so frames never pile up in the socket, QUIC or the controller and the picture
/// stays current even when the link slows. Delay growing beyond the quietest round trip means a
/// queue is forming, so the bitrate goes down; it climbs back while the link stays quiet.
struct VideoFlow {
    private let interval: Double
    private let maxBitrate: Int
    private(set) var bitrate: Int
    private var unacknowledged: [(sequence: UInt32, sentAt: Double)] = []
    private var nextSequence: UInt32 = 0
    private var startedAt: Double?
    private var acknowledged = false
    private var fastest = Double.infinity
    private var fastestAt = 0.0
    private var queueing = 0.0
    private var adjustedAt = 0.0
    private var heldFrames = 0
    private var congestedChecks = 0
    private var congestedAt = -Double.infinity
    private var raisedAt = -Double.infinity
    private var sentAt = -Double.infinity
    private var bitrateChanged = false
    private var keyFrameRequested = false
    private var keyFrameAt = -Double.infinity

    init(fps: Int, bitrate: Int) {
        interval = 1 / Double(max(1, fps))
        maxBitrate = bitrate
        self.bitrate = bitrate
    }

    /// The lowest bitrate congestion may push the stream to.
    var minimumBitrate: Int { max(1_000_000, maxBitrate / 4) }

    /// Frames allowed on their way: one round trip plus two frame intervals, plus one frame.
    var window: Int {
        let roundTrip = fastest.isFinite ? fastest : 0.05
        return max(2, Int(((roundTrip + 2 * interval) / interval).rounded(.up)) + 1)
    }

    /// Whether to capture another frame now. A controller that never acknowledges within two
    /// seconds is treated as one without feedback and is never held back.
    mutating func admits(now: Double) -> Bool {
        // A still screen sends almost nothing, so nothing could show the link recovering; the
        // first frame after a quiet second starts again at full quality.
        if now - sentAt > 1, now - congestedAt > 1 { setBitrate(maxBitrate) }
        if !acknowledged, let startedAt, now - startedAt > 2 { return true }
        guard unacknowledged.count >= window else { return true }
        heldFrames += 1
        return false
    }

    /// Records a frame about to be sent and returns its sequence number.
    mutating func send(now: Double) -> UInt32 {
        if startedAt == nil { startedAt = now }
        sentAt = now
        let sequence = nextSequence
        nextSequence &+= 1
        unacknowledged.append((sequence, now))
        return sequence
    }

    mutating func acknowledge(_ sequence: UInt32, now: Double) {
        // Sequence numbers wrap; anything up to 2^31 behind the acknowledged one is covered.
        guard let index = unacknowledged.lastIndex(where: { sequence &- $0.sequence < 1 << 31 }) else { return }
        acknowledged = true
        let delay = now - unacknowledged[index].sentAt
        unacknowledged.removeFirst(index + 1)
        if delay < fastest || now - fastestAt > 10 {
            fastest = delay
            fastestAt = now
        }
        queueing += (max(0, delay - fastest) - queueing) * 0.2
        guard now - adjustedAt >= 0.25 else { return }
        adjustedAt = now
        // One large frame or a busy moment is not congestion: it takes half a second of frames
        // arriving late, or several held back, before the bitrate goes down.
        let congested = heldFrames >= 3 || queueing > max(0.03, 3 * interval)
        heldFrames = 0
        congestedChecks = congested ? congestedChecks + 1 : 0
        if congestedChecks >= 2 {
            setBitrate(max(minimumBitrate, Int(Double(bitrate) * 0.85)))
            congestedAt = now
            congestedChecks = 0
        } else if !congested, now - congestedAt >= 1, now - raisedAt >= 0.5, bitrate < maxBitrate {
            setBitrate(min(maxBitrate, Int(Double(bitrate) * 1.15)))
            raisedAt = now
        }
    }

    private mutating func setBitrate(_ value: Int) {
        guard value != bitrate else { return }
        bitrate = value
        bitrateChanged = true
    }

    /// A new bitrate for the encoder, once per change.
    mutating func takeBitrateChange() -> Int? {
        guard bitrateChanged else { return nil }
        bitrateChanged = false
        return bitrate
    }

    mutating func requestKeyFrame() { keyFrameRequested = true }

    /// Whether the next frame should be a key frame. Requests within half a second of the last
    /// forced key frame are served by that one.
    mutating func takeKeyFrame(now: Double) -> Bool {
        guard keyFrameRequested else { return false }
        keyFrameRequested = false
        guard now - keyFrameAt >= 0.5 else { return false }
        keyFrameAt = now
        return true
    }
}

enum VideoError: String, Error {
    case codec = "video_codec_failed"
    case packet = "video_packet_invalid"
}

private extension Data {
    mutating func appendLength(_ length: Int) {
        var bigEndian = UInt32(length).bigEndian
        Swift.withUnsafeBytes(of: &bigEndian) { append(contentsOf: $0) }
    }

    func uint32(at index: Int) -> Int {
        withUnsafeBytes { Int(UInt32(bigEndian: $0.loadUnaligned(fromByteOffset: index, as: UInt32.self))) }
    }
}

// ScreenCaptureKit and VideoToolbox own the buffers. The pipeline only reads completed samples.
private struct VideoSample: @unchecked Sendable {
    let buffer: CMSampleBuffer
}

/// Paces captured frames to the target rate. ScreenCaptureKit drops any frame that arrives even
/// slightly sooner than `minimumFrameInterval`, so asking it for exactly 1/60 s on a 60 Hz display
/// loses frames to timing jitter and settles near 50 fps. Capture asks for a shorter interval and
/// this gate keeps frames whose spacing is close enough to the target instead.
struct FramePacer {
    private let minimumSpacing: Double
    private var last: Double?

    init(fps: Int) {
        minimumSpacing = 0.75 / Double(max(1, fps))
    }

    /// The interval to ask ScreenCaptureKit for: short enough that display jitter never drops a frame.
    static func captureInterval(fps: Int) -> CMTime {
        CMTime(value: 1, timescale: CMTimeScale(max(1, fps) * 2))
    }

    mutating func accepts(_ seconds: Double) -> Bool {
        if let last, seconds - last < minimumSpacing, seconds >= last { return false }
        last = seconds
        return true
    }
}

// The pacer is only touched on the capture queue, which ScreenCaptureKit calls serially.
private final class ScreenVideoOutput: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let video: AsyncThrowingStream<VideoSample, Error>.Continuation
    private var pacer: FramePacer

    init(_ video: AsyncThrowingStream<VideoSample, Error>.Continuation, fps: Int) {
        self.video = video
        self.pacer = FramePacer(fps: fps)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
                    as? [[SCStreamFrameInfo: Any]],
              let status = attachments.first?[.status] as? Int,
              status == SCFrameStatus.complete.rawValue,
              pacer.accepts(sample.presentationTimeStamp.seconds) else { return }
        video.yield(VideoSample(buffer: sample))
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        video.finish(throwing: error)
    }
}

final class VideoEncoder {
    private let session: VTCompressionSession
    let isHEVC: Bool
    let pixelFormat: OSType
    /// The average bitrate the encoder was created with.
    let bitrate: Int

    /// `keyFrameSeconds` is the longest gap between key frames. Controllers with feedback ask for
    /// one when they need it, so their streams can space them out and avoid regular size spikes.
    init(width: Int, height: Int, fps: Int, quality: Int, preferHEVC: Bool, prefer444: Bool,
         keyFrameSeconds: Double = 2) throws {
        if preferHEVC, prefer444,
           let created = Self.makeSession(
               width: width, height: height, fps: fps, quality: quality,
               codec: kCMVideoCodecType_HEVC,
               profile: "HEVC_Main444_AutoLevel" as CFString,
               pixelFormat: kCVPixelFormatType_444YpCbCr8BiPlanarFullRange,
               requireHardware: true, keyFrameSeconds: keyFrameSeconds
           ) {
            self.session = created
            self.isHEVC = true
            self.pixelFormat = kCVPixelFormatType_444YpCbCr8BiPlanarFullRange
            self.bitrate = Self.bitrate(width: width, height: height, fps: fps, quality: quality,
                                        pixelFormat: kCVPixelFormatType_444YpCbCr8BiPlanarFullRange)
            return
        }
        if preferHEVC,
           let created = Self.makeSession(
               width: width, height: height, fps: fps, quality: quality,
               codec: kCMVideoCodecType_HEVC,
               profile: kVTProfileLevel_HEVC_Main_AutoLevel,
               pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
               requireHardware: false, keyFrameSeconds: keyFrameSeconds
           ) {
            self.session = created
            self.isHEVC = true
            self.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            self.bitrate = Self.bitrate(width: width, height: height, fps: fps, quality: quality,
                                        pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange)
            return
        }
        guard let fallback = Self.makeSession(
            width: width, height: height, fps: fps, quality: quality,
            codec: kCMVideoCodecType_H264,
            profile: kVTProfileLevel_H264_High_AutoLevel,
            pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            requireHardware: false, keyFrameSeconds: keyFrameSeconds
        ) else {
            throw VideoError.codec
        }
        self.session = fallback
        self.isHEVC = false
        self.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        self.bitrate = Self.bitrate(width: width, height: height, fps: fps, quality: quality,
                                    pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange)
    }

    /// Screen text needs far more bits than camera video: about 50 Mbps for 2560 × 1600 at 60 fps
    /// and quality 82, more for 4:4:4, which a gigabit LAN carries easily. With feedback the
    /// stream starts here and `VideoFlow` lowers it while the link cannot keep up.
    static func bitrate(width: Int, height: Int, fps: Int, quality: Int, pixelFormat: OSType) -> Int {
        let chroma = pixelFormat == kCVPixelFormatType_444YpCbCr8BiPlanarFullRange ? 13 : 10
        return max(1_500_000, min(150_000_000, width * height * fps * quality / 400 * chroma / 10))
    }

    /// Whether VideoToolbox runs this session on the media engine.
    var isHardware: Bool {
        var value: CFTypeRef?
        VTSessionCopyProperty(session, key: kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                              allocator: nil, valueOut: &value)
        return (value as? Bool) ?? false
    }

    /// Changes the bitrate of the running session; the next frames follow it.
    func setBitrate(_ bitrate: Int) {
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AverageBitRate, value: bitrate as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_DataRateLimits,
                             value: [Int(Double(bitrate) / 8.0 * 2.5), 1] as CFArray)
    }

    private static func makeSession(
        width: Int, height: Int, fps: Int, quality: Int,
        codec: CMVideoCodecType, profile: CFString, pixelFormat: OSType, requireHardware: Bool,
        keyFrameSeconds: Double
    ) -> VTCompressionSession? {
        var session: VTCompressionSession?
        let spec: [CFString: Any] = requireHardware
            ? [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: true]
            : [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true]
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: pixelFormat,
        ]
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height), codecType: codec,
            encoderSpecification: spec as CFDictionary,
            imageBufferAttributes: attrs as CFDictionary,
            compressedDataAllocator: nil, outputCallback: nil, refcon: nil,
            compressionSessionOut: &session
        )
        guard status == noErr, let session else { return nil }

        let avgBitrate = bitrate(width: width, height: height, fps: fps, quality: quality, pixelFormat: pixelFormat)
        let peakBytesPerSec = Int(Double(avgBitrate) / 8.0 * 2.5)
        let properties: [CFString: Any] = [
            kVTCompressionPropertyKey_RealTime: true,
            kVTCompressionPropertyKey_AllowFrameReordering: false,
            kVTCompressionPropertyKey_ProfileLevel: profile,
            kVTCompressionPropertyKey_ExpectedFrameRate: fps,
            kVTCompressionPropertyKey_AverageBitRate: avgBitrate,
            kVTCompressionPropertyKey_DataRateLimits: [peakBytesPerSec, 1] as CFArray,
            kVTCompressionPropertyKey_Quality: Float(max(20, min(95, quality))) / 100.0,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: keyFrameSeconds,
            kVTCompressionPropertyKey_MaxFrameDelayCount: 0,
            kVTCompressionPropertyKey_ColorPrimaries: kCVImageBufferColorPrimaries_ITU_R_709_2,
            kVTCompressionPropertyKey_TransferFunction: kCVImageBufferTransferFunction_ITU_R_709_2,
            kVTCompressionPropertyKey_YCbCrMatrix: kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        ]
        guard VTSessionSetProperties(session, propertyDictionary: properties as CFDictionary) == noErr,
              VTCompressionSessionPrepareToEncodeFrames(session) == noErr else {
            VTCompressionSessionInvalidate(session)
            return nil
        }
        return session
    }

    deinit { VTCompressionSessionInvalidate(session) }

    func encode(_ sample: CMSampleBuffer, keyFrame: Bool = false) async throws -> CMSampleBuffer? {
        guard let pixels = CMSampleBufferGetImageBuffer(sample) else { return nil }
        let properties = keyFrame ? [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary : nil
        return try await withCheckedThrowingContinuation { continuation in
            let status = VTCompressionSessionEncodeFrame(
                session, imageBuffer: pixels, presentationTimeStamp: sample.presentationTimeStamp,
                duration: sample.duration, frameProperties: properties, infoFlagsOut: nil
            ) { status, _, encoded in
                if status == noErr {
                    continuation.resume(returning: encoded.map { VideoSample(buffer: $0) }?.buffer)
                } else {
                    continuation.resume(throwing: VideoError.codec)
                }
            }
            if status != noErr { continuation.resume(throwing: VideoError.codec) }
        }
    }
}

func streamDesktopVideo(_ settings: VideoSettings, socket: String) async {
    let connection = NWConnection(to: .unix(path: socket), using: .tcp)
    let queue = DispatchQueue(label: "KoKo.video.capture")
    let (frames, output) = AsyncThrowingStream<VideoSample, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
    let flow = OSAllocatedUnfairLock<VideoFlow?>(initialState: nil)
    // The newest frame the window held back. ScreenCaptureKit sends nothing while the screen is
    // still, so without it the controller could keep showing an older picture.
    let held = OSAllocatedUnfairLock<VideoSample?>(initialState: nil)
    connection.start(queue: queue)
    // The daemon relays the controller's feedback on the socket and closes it when the stream ends.
    readVideoFeedback(from: connection, into: flow, acknowledged: {
        if let frame = held.withLock({ frame in defer { frame = nil }; return frame }) { output.yield(frame) }
    }, ended: { output.finish() })
    defer { connection.cancel() }
    do {
        let (filter, config) = try await screenCaptureSource(displayID: settings.display, edge: settings.edge)
        let videoWidth = max(2, config.width & ~1)
        let videoHeight = max(2, config.height & ~1)
        let encoder = try VideoEncoder(
            width: videoWidth, height: videoHeight, fps: settings.fps, quality: settings.quality,
            preferHEVC: settings.hevc, prefer444: settings.chroma444,
            keyFrameSeconds: settings.feedback ? 10 : 2
        )
        if settings.feedback {
            let start = VideoFlow(fps: settings.fps, bitrate: encoder.bitrate)
            flow.withLock { $0 = start }
            // Older controllers would reject a record kind they do not know.
            let info = VideoStreamInfo(encoder: encoder.isHardware ? "hardware" : "software",
                                       capture: "ScreenCaptureKit", bitrate: encoder.bitrate)
            sendVideoFast(.streamInfo, bytes: try JSONEncoder().encode(info), to: connection)
        }
        config.width = videoWidth
        config.height = videoHeight
        config.pixelFormat = encoder.pixelFormat
        config.colorSpaceName = CGColorSpace.sRGB
        config.showsCursor = false
        config.capturesAudio = false
        config.minimumFrameInterval = FramePacer.captureInterval(fps: settings.fps)
        // Room for the encoder to hold a frame while the next one is captured.
        config.queueDepth = 5

        let delegate = ScreenVideoOutput(output, fps: settings.fps)
        let stream = SCStream(filter: filter, configuration: config, delegate: delegate)
        try stream.addStreamOutput(delegate, type: .screen, sampleHandlerQueue: queue)
        try await stream.startCapture()
        do {
            var format: CMFormatDescription?
            var newest = CMTime.negativeInfinity
            for try await frame in frames {
                // A held frame handed back late must not replace a newer one already sent.
                guard frame.buffer.presentationTimeStamp > newest else { continue }
                // A frame skipped here is never encoded, so the controller's picture stays
                // decodable and only shows fewer frames while the link catches up.
                let (admitted, bitrate, keyFrame) = flow.withLock { state -> (Bool, Int?, Bool) in
                    guard state != nil else { return (true, nil, false) }
                    let now = ProcessInfo.processInfo.systemUptime
                    guard state!.admits(now: now) else { return (false, nil, false) }
                    return (true, state!.takeBitrateChange(), state!.takeKeyFrame(now: now))
                }
                guard admitted else {
                    held.withLock { $0 = frame }
                    continue
                }
                held.withLock { $0 = nil }
                newest = frame.buffer.presentationTimeStamp
                if let bitrate {
                    encoder.setBitrate(bitrate)
                    // The controller's network panel shows the bitrate the host is aiming for.
                    if let update = try? JSONEncoder().encode(VideoStreamInfo(bitrate: bitrate)) {
                        sendVideoFast(.streamInfo, bytes: update, to: connection)
                    }
                }
                guard let encoded = try await encoder.encode(frame.buffer, keyFrame: keyFrame),
                      let block = encoded.dataBuffer, let nextFormat = encoded.formatDescription else { continue }
                if format != nextFormat {
                    if encoder.isHEVC {
                        sendVideoFast(.hevcFormat, bytes: try hevcParameterSets(nextFormat), to: connection)
                    } else {
                        sendVideoFast(.format, bytes: try h264ParameterSets(nextFormat), to: connection)
                    }
                    format = nextFormat
                }
                let size = CMBlockBufferGetDataLength(block)
                guard size > 0 else { continue }
                let sequence = flow.withLock { $0?.send(now: ProcessInfo.processInfo.systemUptime) }
                let header = sequence == nil ? 0 : 5
                var bytes = Data(count: header + size)
                if let sequence {
                    bytes.replaceSubrange(0..<4, with: withUnsafeBytes(of: sequence.bigEndian) { Data($0) })
                    bytes[4] = isKeyFrame(encoded) ? 1 : 0
                }
                let status = bytes.withUnsafeMutableBytes {
                    CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: size, destination: $0.baseAddress! + header)
                }
                guard status == noErr else { throw VideoError.codec }
                sendVideoFast(sequence == nil ? .frame : .sequencedFrame, bytes: bytes, to: connection)
            }
        } catch {
            try? await stream.stopCapture()
            throw error
        }
        try? await stream.stopCapture()
    } catch {
        let code = (error as? ScreenCaptureError)?.rawValue ?? (error as? VideoError)?.rawValue ?? "capture_failed"
        sendVideoFast(.error, bytes: Data(code.utf8), to: connection)
    }
}

private func isKeyFrame(_ sample: CMSampleBuffer) -> Bool {
    guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
            as? [[CFString: Any]], let first = attachments.first else { return true }
    return (first[kCMSampleAttachmentKey_NotSync] as? Bool) != true
}

/// Splits complete feedback records off the front of `buffer`. Returns nil for a record that
/// cannot be one, after which the rest of the socket is ignored.
func takeVideoFeedback(from buffer: inout Data) -> [(VideoFeedback, UInt32)]? {
    var records: [(VideoFeedback, UInt32)] = []
    while buffer.count >= 5 {
        let start = buffer.startIndex
        let length = buffer.uint32(at: 0)
        guard let kind = VideoFeedback(rawValue: buffer[start + 4]),
              length == (kind == .ack ? 5 : 1) else { return nil }
        guard buffer.count >= 4 + length else { break }
        let value = kind == .ack ? UInt32(buffer.uint32(at: 5)) : 0
        records.append((kind, value))
        buffer = Data(buffer.dropFirst(4 + length))
    }
    return records
}

private func readVideoFeedback(
    from connection: NWConnection, into flow: OSAllocatedUnfairLock<VideoFlow?>,
    acknowledged: @escaping @Sendable () -> Void, ended: @escaping @Sendable () -> Void
) {
    final class Reader: @unchecked Sendable {
        // Only touched from the connection's queue, which calls back serially.
        var buffer = Data()
        var valid = true
    }
    let reader = Reader()
    func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { data, _, complete, error in
            if let data, reader.valid {
                reader.buffer.append(data)
                if let records = takeVideoFeedback(from: &reader.buffer) {
                    let now = ProcessInfo.processInfo.systemUptime
                    flow.withLock { state in
                        for (kind, sequence) in records {
                            switch kind {
                            case .ack: state?.acknowledge(sequence, now: now)
                            case .keyFrame: state?.requestKeyFrame()
                            }
                        }
                    }
                    if records.contains(where: { $0.0 == .ack }) { acknowledged() }
                } else {
                    reader.valid = false
                    reader.buffer = Data()
                }
            }
            if complete || error != nil {
                ended()
            } else {
                receive()
            }
        }
    }
    receive()
}

private func sendVideoFast(_ kind: VideoRecord, bytes: Data, to connection: NWConnection) {
    var packet = Data(capacity: 5 + bytes.count)
    packet.appendLength(bytes.count + 1)
    packet.append(kind.rawValue)
    packet.append(bytes)
    connection.send(content: packet, completion: .idempotent)
}

func h264ParameterSets(_ format: CMFormatDescription) throws -> Data {
    var data = Data()
    for index in 0..<2 {
        var buffer: UnsafePointer<UInt8>?
        var size = 0
        let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format, parameterSetIndex: index, parameterSetPointerOut: &buffer,
            parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil
        )
        guard status == noErr, let buffer else { throw VideoError.codec }
        data.appendLength(size)
        data.append(buffer, count: size)
    }
    return data
}

func hevcParameterSets(_ format: CMFormatDescription) throws -> Data {
    var count = 0
    let countStatus = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        format, parameterSetIndex: 0, parameterSetPointerOut: nil,
        parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: nil
    )
    guard countStatus == noErr, count >= 3 else { throw VideoError.codec }
    var data = Data()
    data.appendLength(count)
    for index in 0..<count {
        var buffer: UnsafePointer<UInt8>?
        var size = 0
        let status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            format, parameterSetIndex: index, parameterSetPointerOut: &buffer,
            parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil
        )
        guard status == noErr, let buffer else { throw VideoError.codec }
        data.appendLength(size)
        data.append(buffer, count: size)
    }
    return data
}

@MainActor
final class VideoSurface {
    private let renderer = MetalVideoRenderer()
    let audioPlayer = LiveAudioPlayer()
    private var format: CMVideoFormatDescription?

    var layer: CAMetalLayer { renderer.layer }
    var info: VideoPipelineInfo { renderer.info }

    var isMuted: Bool = false {
        didSet { audioPlayer.setMuted(isMuted) }
    }

    func setFormat(_ bytes: Data) throws -> CGSize {
        guard bytes.count >= 8 else { throw VideoError.packet }
        let sps = bytes.uint32(at: 0)
        guard sps > 0, sps <= bytes.count - 8 else { throw VideoError.packet }
        let pps = bytes.uint32(at: sps + 4)
        guard pps > 0, pps == bytes.count - sps - 8 else { throw VideoError.packet }

        var description: CMVideoFormatDescription?
        let status = bytes.withUnsafeBytes { raw in
            let base = raw.bindMemory(to: UInt8.self).baseAddress!
            return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                allocator: nil, parameterSetCount: 2, parameterSetPointers: [base + 4, base + sps + 8],
                parameterSetSizes: [sps, pps], nalUnitHeaderLength: 4, formatDescriptionOut: &description
            )
        }
        guard status == noErr, let description else { throw VideoError.codec }
        format = description
        renderer.setFormat(description)
        let size = CMVideoFormatDescriptionGetDimensions(description)
        return CGSize(width: Int(size.width), height: Int(size.height))
    }

    func setHEVCFormat(_ bytes: Data) throws -> CGSize {
        guard bytes.count >= 16 else { throw VideoError.packet }
        let count = bytes.uint32(at: 0)
        guard (3...8).contains(count) else { throw VideoError.packet }
        var offsets: [(start: Int, size: Int)] = []
        var cursor = 4
        for _ in 0..<count {
            guard cursor + 4 <= bytes.count else { throw VideoError.packet }
            let size = bytes.uint32(at: cursor)
            cursor += 4
            guard size > 0, cursor + size <= bytes.count else { throw VideoError.packet }
            offsets.append((cursor, size))
            cursor += size
        }
        guard cursor == bytes.count else { throw VideoError.packet }

        var description: CMVideoFormatDescription?
        let status = bytes.withUnsafeBytes { raw in
            let base = raw.bindMemory(to: UInt8.self).baseAddress!
            let pointers = offsets.map { base + $0.start }
            let sizes = offsets.map(\.size)
            return pointers.withUnsafeBufferPointer { ptrBuf in
                sizes.withUnsafeBufferPointer { sizeBuf in
                    CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        allocator: nil,
                        parameterSetCount: count,
                        parameterSetPointers: ptrBuf.baseAddress!,
                        parameterSetSizes: sizeBuf.baseAddress!,
                        nalUnitHeaderLength: 4,
                        extensions: nil,
                        formatDescriptionOut: &description
                    )
                }
            }
        }
        guard status == noErr, let description else { throw VideoError.codec }
        format = description
        renderer.setFormat(description)
        let size = CMVideoFormatDescriptionGetDimensions(description)
        return CGSize(width: Int(size.width), height: Int(size.height))
    }

    func enqueue(_ bytes: Data) throws {
        guard let format, !bytes.isEmpty else { throw VideoError.packet }
        var buffer: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: nil, memoryBlock: nil, blockLength: bytes.count,
            blockAllocator: nil, customBlockSource: nil, offsetToData: 0, dataLength: bytes.count,
            flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &buffer
        ) == noErr, let buffer else { throw VideoError.codec }

        let written = bytes.withUnsafeBytes {
            CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: buffer, offsetIntoDestination: 0, dataLength: bytes.count)
        }
        var sample: CMSampleBuffer?
        guard written == noErr,
              CMSampleBufferCreateReady(
                allocator: nil, dataBuffer: buffer, formatDescription: format, sampleCount: 1,
                sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1,
                sampleSizeArray: [bytes.count], sampleBufferOut: &sample
              ) == noErr, let sample else { throw VideoError.codec }
        renderer.decode(sample)
    }

    /// The host's encoder, capture and current bitrate, from its `streamInfo` records.
    func setStreamInfo(_ bytes: Data) {
        guard let info = try? JSONDecoder().decode(VideoStreamInfo.self, from: bytes) else { return }
        renderer.setStreamInfo(info)
    }

    /// Called by the view when its size or backing scale changes.
    func resize(to size: CGSize, scale: CGFloat) {
        renderer.resize(to: size, scale: scale)
    }

    /// Whether a frame failed to decode since the last call, so a key frame is needed.
    func takeKeyFrameNeed() -> Bool { renderer.takeKeyFrameNeed() }

    var isReadyForMoreMediaData: Bool { renderer.canAccept }
    var hasFailed: Bool { renderer.hasFailed }

    func clear() {
        renderer.clear()
        audioPlayer.stop()
        format = nil
    }
}

@MainActor
final class VideoReceiver {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let reader = DispatchQueue(label: "KoKo.video.decode")
    /// Feedback writes and closing stdin happen in order on this queue, off the main thread.
    private let writer = DispatchQueue(label: "KoKo.video.feedback")
    /// Only read and written on `writer`.
    nonisolated(unsafe) private var inputClosed = false
    /// Audio records are decoded and played on the reader queue, never on the main actor.
    private let audio: LiveAudioPlayer

    init(keyPath: String, host: PairedHost, settings: VideoSettings, audio: LiveAudioPlayer) throws {
        self.audio = audio
        let action = RemoteAction(
            action: "video", display: settings.display, edge: settings.edge,
            quality: settings.quality, fps: settings.fps, audio: settings.audio,
            hevc: settings.hevc ? true : nil, chroma444: settings.chroma444 ? true : nil,
            feedback: settings.feedback ? true : nil
        )
        let request = try JSONEncoder().encode(action)
        process.executableURL = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/koko-agent")
        process.arguments = ["remote-video", keyPath, host.hostID, host.address, String(decoding: request, as: UTF8.self)]
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
    }

    func next() async throws -> (VideoRecord, Data)? {
        let file = output.fileHandleForReading
        let audio = self.audio
        return try await withCheckedThrowingContinuation { continuation in
            reader.async {
                do {
                    while true {
                        guard let prefix = try Self.read(4, from: file) else {
                            continuation.resume(returning: nil)
                            return
                        }
                        let count = prefix.uint32(at: 0)
                        guard (1...8 * 1024 * 1024).contains(count),
                              let data = try Self.read(count, from: file),
                              let kind = VideoRecord(rawValue: data[0]) else {
                            throw VideoError.packet
                        }
                        let payload = data.subdata(in: 1..<data.count)
                        // Audio goes straight to its own queue. Passing it through the caller
                        // put 10 ms packets behind video work on the main thread, which showed
                        // up as growing delay and stutter.
                        switch kind {
                        case .audioFormat:
                            audio.setFormat(payload)
                        case .audioFrame:
                            audio.enqueue(payload)
                        default:
                            continuation.resume(returning: (kind, payload))
                            return
                        }
                    }
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Closing stdin lets the helper close its QUIC connection, which tells the host at once
    /// that this stream ended. Killing it first would leave the host waiting for a timeout and
    /// refusing the next stream meanwhile, so the helper is only killed if it does not exit.
    /// Asks the host for a key frame after this Mac had to skip one it could not show.
    func requestKeyFrame() {
        write(Data([0, 0, 0, 1, VideoFeedback.keyFrame.rawValue]))
    }

    private func write(_ record: Data) {
        let handle = input.fileHandleForWriting
        writer.async { [weak self] in
            guard let self, !self.inputClosed else { return }
            try? handle.write(contentsOf: record)
        }
    }

    func close() {
        let handle = input.fileHandleForWriting
        writer.async { [weak self] in
            self?.inputClosed = true
            try? handle.close()
        }
        let process = self.process
        DispatchQueue.global().asyncAfter(deadline: .now() + 1.5) {
            if process.isRunning { process.terminate() }
        }
    }

    private nonisolated static func read(_ length: Int, from file: FileHandle) throws -> Data? {
        var buffer = Data()
        buffer.reserveCapacity(length)
        while buffer.count < length {
            guard let chunk = try file.read(upToCount: length - buffer.count), !chunk.isEmpty else {
                if buffer.isEmpty { return nil }
                throw VideoError.packet
            }
            buffer.append(chunk)
        }
        return buffer
    }
}
