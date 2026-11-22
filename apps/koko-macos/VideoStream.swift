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
}

// Each record is a big-endian length followed by a kind byte and its payload.
enum VideoRecord: UInt8 {
    case format = 1, frame, error, path
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
    private let continuation: AsyncThrowingStream<VideoSample, Error>.Continuation

    init(_ continuation: AsyncThrowingStream<VideoSample, Error>.Continuation) {
        self.continuation = continuation
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
                    as? [[SCStreamFrameInfo: Any]],
              let status = attachments.first?[.status] as? Int,
              status == SCFrameStatus.complete.rawValue else { return }
        continuation.yield(VideoSample(buffer: sample))
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        continuation.finish(throwing: error)
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

func streamDesktopVideo(_ settings: VideoSettings, socket: String) async {
    let connection = NWConnection(to: .unix(path: socket), using: .tcp)
    let queue = DispatchQueue(label: "KoKo.video.capture")
    let (frames, output) = AsyncThrowingStream<VideoSample, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
    connection.start(queue: queue)
    connection.receive(minimumIncompleteLength: 1, maximumLength: 1) { _, _, _, _ in output.finish() }
    defer { connection.cancel() }
    do {
        let (filter, config) = try await screenCaptureSource(displayID: settings.display, edge: settings.edge)
        config.width = max(2, config.width & ~1)
        config.height = max(2, config.height & ~1)
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.showsCursor = false
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(settings.fps))

        let encoder = try AVCEncoder(width: config.width, height: config.height, fps: settings.fps, quality: settings.quality)
        let delegate = ScreenVideoOutput(output)
        let stream = SCStream(filter: filter, configuration: config, delegate: delegate)
        try stream.addStreamOutput(delegate, type: .screen, sampleHandlerQueue: queue)
        try await stream.startCapture()
        do {
            var format: CMFormatDescription?
            for try await frame in frames {
                guard let encoded = try await encoder.encode(frame)?.buffer,
                      let block = encoded.dataBuffer, let nextFormat = encoded.formatDescription else { continue }
                if format != nextFormat {
                    try await sendVideo(.format, bytes: h264ParameterSets(nextFormat), to: connection)
                    format = nextFormat
                }
                let size = CMBlockBufferGetDataLength(block)
                guard size > 0 else { continue }
                var bytes = Data(count: size)
                let status = bytes.withUnsafeMutableBytes {
                    CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: size, destination: $0.baseAddress!)
                }
                guard status == noErr else { throw VideoError.codec }
                try await sendVideo(.frame, bytes: bytes, to: connection)
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
    private var format: CMVideoFormatDescription?
    private var needsFlush = false

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

    var isReadyForMoreMediaData: Bool { needsFlush || layer.isReadyForMoreMediaData }
    var hasFailed: Bool { layer.status == .failed }

    func clear() {
        layer.flushAndRemoveImage()
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
        let action = RemoteAction(action: "video", display: settings.display, edge: settings.edge,
                                  quality: settings.quality, fps: settings.fps)
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
