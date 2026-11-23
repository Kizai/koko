@preconcurrency import AVFoundation
import Foundation
import Network
@preconcurrency import ScreenCaptureKit
@preconcurrency import VideoToolbox

struct VideoSettings: Equatable {
    let display: UInt32
    let edge: Int
    let quality: Int
    let fps: Int
    let audio: Bool
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
    case format = 1, frame, error, path, audioFormat, audioFrame, syncFrame
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

private final class ScreenVideoOutput: NSObject, SCStreamOutput, SCStreamDelegate, Sendable {
    private let video: AsyncThrowingStream<VideoSample, Error>.Continuation
    private let audio: AsyncStream<VideoSample>.Continuation?

    init(
        _ video: AsyncThrowingStream<VideoSample, Error>.Continuation,
        audio: AsyncStream<VideoSample>.Continuation?
    ) {
        self.video = video
        self.audio = audio
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        if type == .audio {
            guard CMSampleBufferDataIsReady(sample) else { return }
            audio?.yield(VideoSample(buffer: sample))
            return
        }
        guard type == .screen, let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
                    as? [[SCStreamFrameInfo: Any]],
              let status = attachments.first?[.status] as? Int,
              status == SCFrameStatus.complete.rawValue else { return }
        video.yield(VideoSample(buffer: sample))
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        video.finish(throwing: error)
        audio?.finish()
    }
}

private final class AVCEncoder {
    private let session: VTCompressionSession

    init(width: Int, height: Int, fps: Int, quality: Int) throws {
        var session: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_H264,
            encoderSpecification: [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true] as CFDictionary,
            imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: nil, refcon: nil,
            compressionSessionOut: &session
        )
        guard status == noErr, let session else { throw VideoError.codec }
        self.session = session

        let bitrate = max(700_000, min(45_000_000, width * height * fps * quality / 1400))
        let properties: [CFString: Any] = [
            kVTCompressionPropertyKey_RealTime: true,
            kVTCompressionPropertyKey_AllowFrameReordering: false,
            kVTCompressionPropertyKey_ProfileLevel: kVTProfileLevel_H264_High_AutoLevel,
            kVTCompressionPropertyKey_ExpectedFrameRate: fps,
            kVTCompressionPropertyKey_AverageBitRate: bitrate,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: 2,
            kVTCompressionPropertyKey_MaxFrameDelayCount: 0,
        ]
        guard VTSessionSetProperties(session, propertyDictionary: properties as CFDictionary) == noErr,
              VTCompressionSessionPrepareToEncodeFrames(session) == noErr else { throw VideoError.codec }
    }

    deinit { VTCompressionSessionInvalidate(session) }

    func encode(_ sample: VideoSample) async throws -> VideoSample? {
        guard let pixels = CMSampleBufferGetImageBuffer(sample.buffer) else { return nil }
        return try await withCheckedThrowingContinuation { continuation in
            let status = VTCompressionSessionEncodeFrame(
                session, imageBuffer: pixels, presentationTimeStamp: sample.buffer.presentationTimeStamp,
                duration: sample.buffer.duration, frameProperties: nil, infoFlagsOut: nil
            ) { status, _, encoded in
                if status == noErr {
                    continuation.resume(returning: encoded.map { VideoSample(buffer: $0) })
                } else {
                    continuation.resume(throwing: VideoError.codec)
                }
            }
            if status != noErr { continuation.resume(throwing: VideoError.codec) }
        }
    }
}

private actor SocketRecordWriter {
    private let connection: NWConnection

    init(_ connection: NWConnection) {
        self.connection = connection
    }

    func send(_ kind: VideoRecord, bytes: Data) async throws {
        try await sendVideo(kind, bytes: bytes, to: connection)
    }
}

func streamDesktopVideo(_ settings: VideoSettings, socket: String) async {
    let connection = NWConnection(to: .unix(path: socket), using: .tcp)
    let queue = DispatchQueue(label: "KoKo.video.capture")
    let (frames, output) = AsyncThrowingStream<VideoSample, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
    let (audioSamples, audioOutput) = AsyncStream<VideoSample>.makeStream(bufferingPolicy: .bufferingNewest(24))
    connection.start(queue: queue)
    connection.receive(minimumIncompleteLength: 1, maximumLength: 1) { _, _, _, _ in
        output.finish()
        audioOutput.finish()
    }
    defer { connection.cancel() }
    do {
        let (filter, config) = try await screenCaptureSource(displayID: settings.display, edge: settings.edge)
        config.width = max(2, config.width & ~1)
        config.height = max(2, config.height & ~1)
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.showsCursor = false
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(settings.fps))
        config.capturesAudio = settings.audio
        if settings.audio {
            config.sampleRate = 48_000
            config.channelCount = 2
        }

        let videoWidth = config.width
        let videoHeight = config.height
        let delegate = ScreenVideoOutput(output, audio: settings.audio ? audioOutput : nil)
        let stream = SCStream(filter: filter, configuration: config, delegate: delegate)
        try stream.addStreamOutput(delegate, type: .screen, sampleHandlerQueue: queue)
        if settings.audio {
            try stream.addStreamOutput(delegate, type: .audio, sampleHandlerQueue: queue)
        }
        try await stream.startCapture()
        let writer = SocketRecordWriter(connection)
        let clockBase = CMClockGetTime(CMClockGetHostTimeClock())
        do {
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask {
                    let encoder = try AVCEncoder(width: videoWidth, height: videoHeight, fps: settings.fps, quality: settings.quality)
                    var format: CMFormatDescription?
                    for try await frame in frames {
                        let ptsUs = Int64(((frame.buffer.presentationTimeStamp - clockBase).seconds * 1_000_000).rounded())
                        guard let encoded = try await encoder.encode(frame)?.buffer,
                              let block = encoded.dataBuffer, let nextFormat = encoded.formatDescription else { continue }
                        if format != nextFormat {
                            try await writer.send(.format, bytes: h264ParameterSets(nextFormat))
                            format = nextFormat
                        }
                        let size = CMBlockBufferGetDataLength(block)
                        guard size > 0 else { continue }
                        var bytes = Data(count: size)
                        let status = bytes.withUnsafeMutableBytes {
                            CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: size, destination: $0.baseAddress!)
                        }
                        guard status == noErr else { throw VideoError.codec }
                        if settings.audio {
                            try await writer.send(.syncFrame, bytes: AudioPacketCodec.encodeVideoTimedPayload(ptsUs: ptsUs, payload: bytes))
                        } else {
                            try await writer.send(.frame, bytes: bytes)
                        }
                    }
                }
                if settings.audio {
                    group.addTask {
                        let aac = try AACStreamEncoder()
                        var sentHeader = false
                        for await sample in audioSamples {
                            let packets = aac.encode(sample.buffer, basePTS: clockBase)
                            if !sentHeader, let header = aac.formatHeader {
                                try await writer.send(.audioFormat, bytes: header)
                                sentHeader = true
                            }
                            for packet in packets {
                                let payload = AudioPacketCodec.encodeTimedPayload(
                                    ptsUs: packet.ptsUs, durationUs: packet.durationUs, payload: packet.data
                                )
                                try await writer.send(.audioFrame, bytes: payload)
                            }
                        }
                    }
                }
                try await group.waitForAll()
            }
        } catch {
            try? await stream.stopCapture()
            throw error
        }
        try? await stream.stopCapture()
    } catch {
        let code = (error as? ScreenCaptureError)?.rawValue ?? (error as? VideoError)?.rawValue ?? "capture_failed"
        try? await sendVideo(.error, bytes: Data(code.utf8), to: connection)
    }
}

private func sendVideo(_ kind: VideoRecord, bytes: Data, to connection: NWConnection) async throws {
    var packet = Data()
    packet.appendLength(bytes.count + 1)
    packet.append(kind.rawValue)
    packet.append(bytes)
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
        connection.send(content: packet, completion: .contentProcessed { error in
            if let error { continuation.resume(throwing: error) } else { continuation.resume() }
        })
    }
}

private func h264ParameterSets(_ format: CMFormatDescription) throws -> Data {
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

@MainActor
final class VideoSurface {
    let layer = AVSampleBufferDisplayLayer()
    private let audioRenderer = AVSampleBufferAudioRenderer()
    private let synchronizer = AVSampleBufferRenderSynchronizer()
    private var format: CMVideoFormatDescription?
    private var audioFormat: CMAudioFormatDescription?
    private var needsFlush = false
    private var needsAudioFlush = false

    var isMuted: Bool = false {
        didSet { audioRenderer.isMuted = isMuted }
    }

    init() {
        synchronizer.addRenderer(layer)
        synchronizer.addRenderer(audioRenderer)
        synchronizer.rate = 0
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

    func setAudioFormat(_ bytes: Data) {
        guard let description = AudioPacketCodec.makeFormatDescription(from: bytes) else { return }
        audioFormat = description
        needsAudioFlush = true
    }

    func enqueue(_ bytes: Data, ptsUs: Int64? = nil) throws {
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
        if let ptsUs {
            let pts = CMTime(value: ptsUs, timescale: 1_000_000)
            var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: pts, decodeTimeStamp: .invalid)
            guard written == noErr,
                  CMSampleBufferCreateReady(
                    allocator: nil, dataBuffer: buffer, formatDescription: format, sampleCount: 1,
                    sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 1,
                    sampleSizeArray: [bytes.count], sampleBufferOut: &sample
                  ) == noErr else { throw VideoError.codec }
            syncClockIfNeeded(to: pts)
        } else {
            guard written == noErr,
                  CMSampleBufferCreateReady(
                    allocator: nil, dataBuffer: buffer, formatDescription: format, sampleCount: 1,
                    sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1,
                    sampleSizeArray: [bytes.count], sampleBufferOut: &sample
                  ) == noErr else { throw VideoError.codec }
            if let sample, let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) {
                let first = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
                CFDictionarySetValue(first, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                     Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
            }
        }
        guard let sample else { throw VideoError.codec }

        if needsFlush {
            layer.flush()
            needsFlush = false
        }
        layer.enqueue(sample)
    }

    func enqueueAudio(_ bytes: Data) {
        guard let audioFormat,
              let packet = AudioPacketCodec.decodeTimedPayload(bytes),
              let sample = AudioPacketCodec.makeSampleBuffer(
                payload: packet.payload, format: audioFormat, ptsUs: packet.ptsUs, durationUs: packet.durationUs
              ) else { return }
        if needsAudioFlush {
            audioRenderer.flush()
            needsAudioFlush = false
        }
        let pts = CMTime(value: packet.ptsUs, timescale: 1_000_000)
        syncClockIfNeeded(to: pts)
        if audioRenderer.isReadyForMoreMediaData {
            audioRenderer.enqueue(sample)
        }
    }

    private func syncClockIfNeeded(to pts: CMTime) {
        if synchronizer.rate == 0 {
            synchronizer.setRate(1.0, time: pts)
            return
        }
        let drift = abs((synchronizer.currentTime() - pts).seconds)
        if drift > 0.25 {
            synchronizer.setRate(1.0, time: pts)
        }
    }

    var isReadyForMoreMediaData: Bool { needsFlush || layer.isReadyForMoreMediaData }
    var hasFailed: Bool { layer.status == .failed }

    func clear() {
        synchronizer.rate = 0
        layer.flushAndRemoveImage()
        audioRenderer.flush()
        format = nil
        audioFormat = nil
    }
}

@MainActor
final class VideoReceiver {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let reader = DispatchQueue(label: "KoKo.video.decode")

    init(keyPath: String, host: PairedHost, settings: VideoSettings) throws {
        let action = RemoteAction(action: "video", display: settings.display, edge: settings.edge,
                                  quality: settings.quality, fps: settings.fps, audio: settings.audio)
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
                    continuation.resume(returning: (kind, Data(data.dropFirst())))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    nonisolated private static func read(_ count: Int, from file: FileHandle) throws -> Data? {
        var data = Data()
        while data.count < count {
            guard let bytes = try file.read(upToCount: count - data.count), !bytes.isEmpty else {
                if data.isEmpty { return nil }
                throw VideoError.packet
            }
            data.append(bytes)
        }
        return data
    }

    func close() {
        try? input.fileHandleForWriting.close()
        try? output.fileHandleForReading.close()
    }
}
