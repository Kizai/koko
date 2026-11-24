@preconcurrency import AVFoundation
import CoreGraphics
import Foundation
import Network
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
    case format = 1, frame, error, path, audioFormat, audioFrame, syncFrame, hevcFormat
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

    init(width: Int, height: Int, fps: Int, quality: Int, preferHEVC: Bool, prefer444: Bool) throws {
        if preferHEVC, prefer444,
           let created = Self.makeSession(
               width: width, height: height, fps: fps, quality: quality,
               codec: kCMVideoCodecType_HEVC,
               profile: "HEVC_Main444_AutoLevel" as CFString,
               pixelFormat: kCVPixelFormatType_444YpCbCr8BiPlanarFullRange,
               requireHardware: true
           ) {
            self.session = created
            self.isHEVC = true
            self.pixelFormat = kCVPixelFormatType_444YpCbCr8BiPlanarFullRange
            return
        }
        if preferHEVC,
           let created = Self.makeSession(
               width: width, height: height, fps: fps, quality: quality,
               codec: kCMVideoCodecType_HEVC,
               profile: kVTProfileLevel_HEVC_Main_AutoLevel,
               pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
               requireHardware: false
           ) {
            self.session = created
            self.isHEVC = true
            self.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            return
        }
        guard let fallback = Self.makeSession(
            width: width, height: height, fps: fps, quality: quality,
            codec: kCMVideoCodecType_H264,
            profile: kVTProfileLevel_H264_High_AutoLevel,
            pixelFormat: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            requireHardware: false
        ) else {
            throw VideoError.codec
        }
        self.session = fallback
        self.isHEVC = false
        self.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    }

    private static func makeSession(
        width: Int, height: Int, fps: Int, quality: Int,
        codec: CMVideoCodecType, profile: CFString, pixelFormat: OSType, requireHardware: Bool
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

        // Screen text needs far more bits than camera video: about 50 Mbps for 2560 × 1600 at
        // 60 fps and quality 82, more for 4:4:4, which a gigabit LAN carries easily. A slow link
        // asks for a lower quality and size, which lowers this too.
        let chroma = pixelFormat == kCVPixelFormatType_444YpCbCr8BiPlanarFullRange ? 13 : 10
        let avgBitrate = max(1_500_000, min(150_000_000, width * height * fps * quality / 400 * chroma / 10))
        let peakBytesPerSec = Int(Double(avgBitrate) / 8.0 * 2.5)
        let properties: [CFString: Any] = [
            kVTCompressionPropertyKey_RealTime: true,
            kVTCompressionPropertyKey_AllowFrameReordering: false,
            kVTCompressionPropertyKey_ProfileLevel: profile,
            kVTCompressionPropertyKey_ExpectedFrameRate: fps,
            kVTCompressionPropertyKey_AverageBitRate: avgBitrate,
            kVTCompressionPropertyKey_DataRateLimits: [peakBytesPerSec, 1] as CFArray,
            kVTCompressionPropertyKey_Quality: Float(max(20, min(95, quality))) / 100.0,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: 2,
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

    func encode(_ sample: CMSampleBuffer) async throws -> CMSampleBuffer? {
        guard let pixels = CMSampleBufferGetImageBuffer(sample) else { return nil }
        return try await withCheckedThrowingContinuation { continuation in
            let status = VTCompressionSessionEncodeFrame(
                session, imageBuffer: pixels, presentationTimeStamp: sample.presentationTimeStamp,
                duration: sample.duration, frameProperties: nil, infoFlagsOut: nil
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
    connection.start(queue: queue)
    connection.receive(minimumIncompleteLength: 1, maximumLength: 1) { _, _, _, _ in
        output.finish()
    }
    defer { connection.cancel() }
    do {
        let (filter, config) = try await screenCaptureSource(displayID: settings.display, edge: settings.edge)
        let videoWidth = max(2, config.width & ~1)
        let videoHeight = max(2, config.height & ~1)
        let encoder = try VideoEncoder(
            width: videoWidth, height: videoHeight, fps: settings.fps, quality: settings.quality,
            preferHEVC: settings.hevc, prefer444: settings.chroma444
        )
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
            for try await frame in frames {
                guard let encoded = try await encoder.encode(frame.buffer),
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
                var bytes = Data(count: size)
                let status = bytes.withUnsafeMutableBytes {
                    CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: size, destination: $0.baseAddress!)
                }
                guard status == noErr else { throw VideoError.codec }
                sendVideoFast(.frame, bytes: bytes, to: connection)
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
    let layer = AVSampleBufferDisplayLayer()
    private let audioPlayer = LiveAudioPlayer()
    private var format: CMVideoFormatDescription?
    private var needsFlush = false

    var isMuted: Bool = false {
        didSet { audioPlayer.isMuted = isMuted }
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
        needsFlush = true
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
        needsFlush = true
        let size = CMVideoFormatDescriptionGetDimensions(description)
        return CGSize(width: Int(size.width), height: Int(size.height))
    }

    func setAudioFormat(_ bytes: Data) {
        audioPlayer.setFormat(bytes)
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
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) {
            let first = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(first, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }

        if needsFlush {
            layer.flush()
            needsFlush = false
        }
        layer.enqueue(sample)
    }

    func enqueueAudio(_ bytes: Data) {
        audioPlayer.enqueue(bytes)
    }

    var isReadyForMoreMediaData: Bool { needsFlush || layer.isReadyForMoreMediaData }
    var hasFailed: Bool { layer.status == .failed }

    func clear() {
        layer.flushAndRemoveImage()
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

    init(keyPath: String, host: PairedHost, settings: VideoSettings) throws {
        let action = RemoteAction(
            action: "video", display: settings.display, edge: settings.edge,
            quality: settings.quality, fps: settings.fps, audio: settings.audio,
            hevc: settings.hevc ? true : nil, chroma444: settings.chroma444 ? true : nil
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
        return try await withCheckedThrowingContinuation { continuation in
            reader.async {
                do {
                    guard let prefix = try Self.read(4, from: file) else {
                        continuation.resume(returning: nil)
                        return
                    }
                    let count = prefix.uint32(at: 0)
                    guard (1...8 * 1024 * 1024).contains(count),
                          let data = try Self.read(count, from: file), let kind = VideoRecord(rawValue: data[0]) else {
                        throw VideoError.packet
                    }
                    continuation.resume(returning: (kind, data.subdata(in: 1..<data.count)))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Closing stdin lets the helper close its QUIC connection, which tells the host at once
    /// that this stream ended. Killing it first would leave the host waiting for a timeout and
    /// refusing the next stream meanwhile, so the helper is only killed if it does not exit.
    func close() {
        try? input.fileHandleForWriting.close()
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
