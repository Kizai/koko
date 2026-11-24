@preconcurrency import AVFoundation
import AudioToolbox
import CoreAudio
import CoreMedia
import Foundation
import Network
@preconcurrency import ScreenCaptureKit

enum HostAudioVolume {
    static func read() -> (level: Int, muted: Bool) {
        guard let device = defaultOutputDevice() else { return (50, false) }
        var volume = Float32(0.5)
        var size = UInt32(MemoryLayout<Float32>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwareServiceDeviceProperty_VirtualMainVolume,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
        if AudioObjectHasProperty(device, &address) {
            _ = AudioObjectGetPropertyData(device, &address, 0, nil, &size, &volume)
        } else {
            var scalar = AudioObjectPropertyAddress(
                mSelector: kAudioDevicePropertyVolumeScalar,
                mScope: kAudioObjectPropertyScopeOutput,
                mElement: 1
            )
            if AudioObjectHasProperty(device, &scalar) {
                _ = AudioObjectGetPropertyData(device, &scalar, 0, nil, &size, &volume)
            }
        }
        var muted = UInt32(0)
        var muteSize = UInt32(MemoryLayout<UInt32>.size)
        var muteAddress = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyMute,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
        if AudioObjectHasProperty(device, &muteAddress) {
            _ = AudioObjectGetPropertyData(device, &muteAddress, 0, nil, &muteSize, &muted)
        }
        return (max(0, min(100, Int((volume * 100).rounded()))), muted != 0)
    }

    static func write(level: Int) -> (level: Int, muted: Bool) {
        let clamped = max(0, min(100, level))
        guard let device = defaultOutputDevice() else { return (clamped, false) }
        var volume = Float32(clamped) / 100.0
        let size = UInt32(MemoryLayout<Float32>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwareServiceDeviceProperty_VirtualMainVolume,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
        if AudioObjectHasProperty(device, &address) {
            _ = AudioObjectSetPropertyData(device, &address, 0, nil, size, &volume)
        } else {
            for channel: UInt32 in [1, 2] {
                var scalar = AudioObjectPropertyAddress(
                    mSelector: kAudioDevicePropertyVolumeScalar,
                    mScope: kAudioObjectPropertyScopeOutput,
                    mElement: channel
                )
                if AudioObjectHasProperty(device, &scalar) {
                    _ = AudioObjectSetPropertyData(device, &scalar, 0, nil, size, &volume)
                }
            }
        }
        return read()
    }

    private static func defaultOutputDevice() -> AudioObjectID? {
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        let status = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device)
        guard status == noErr, device != kAudioObjectUnknown else { return nil }
        return device
    }
}

/// Low-delay AAC encoder using exact frame slicing so chunk boundaries never drift or tear.
final class LowDelayAACEncoder {
    private let converter: AVAudioConverter
    private let pcmFormat: AVAudioFormat
    private let aacFormat: AVAudioFormat
    let framesPerPacket: Int
    let formatID: AudioFormatID
    private var leftFIFO: [Float] = []
    private var rightFIFO: [Float] = []
    private var sequence: UInt32 = 0
    private(set) var formatHeader: Data?

    init() throws {
        guard let pcm = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2) else {
            throw VideoError.codec
        }
        self.pcmFormat = pcm

        var eldDesc = AudioStreamBasicDescription(
            mSampleRate: 48_000,
            mFormatID: kAudioFormatMPEG4AAC_ELD,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 480,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 2,
            mBitsPerChannel: 0,
            mReserved: 0
        )
        if let eld = AVAudioFormat(streamDescription: &eldDesc),
           let conv = AVAudioConverter(from: pcm, to: eld) {
            // The highest rate up to 192 kbps the encoder offers; 128 kbps dulled music.
            let rates = conv.applicableEncodeBitRates?.map(\.intValue) ?? []
            conv.bitRate = rates.filter { $0 <= 192_000 }.max() ?? 128_000
            self.aacFormat = eld
            self.converter = conv
            self.framesPerPacket = 480
            self.formatID = kAudioFormatMPEG4AAC_ELD
        } else {
            let lcSettings: [String: Any] = [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 2,
                AVEncoderBitRateKey: 128_000,
            ]
            guard let lc = AVAudioFormat(settings: lcSettings),
                  let conv = AVAudioConverter(from: pcm, to: lc) else {
                throw VideoError.codec
            }
            conv.bitRate = 128_000
            self.aacFormat = lc
            self.converter = conv
            self.framesPerPacket = 1024
            self.formatID = kAudioFormatMPEG4AAC
        }
        leftFIFO.reserveCapacity(4096)
        rightFIFO.reserveCapacity(4096)
    }

    func encode(_ sample: CMSampleBuffer) -> [Data] {
        guard let channels = Self.extractStereoFloat32(from: sample) else { return [] }
        return encodePlanar(left: channels.left, right: channels.right)
    }

    func encodePlanar(left: [Float], right: [Float]) -> [Data] {
        let count = min(left.count, right.count)
        guard count > 0 else { return [] }
        leftFIFO.append(contentsOf: left.prefix(count))
        rightFIFO.append(contentsOf: right.prefix(count))

        var output: [Data] = []
        while leftFIFO.count >= framesPerPacket {
            guard let chunk = AVAudioPCMBuffer(pcmFormat: pcmFormat, frameCapacity: AVAudioFrameCount(framesPerPacket)),
                  let channelData = chunk.floatChannelData else { break }
            chunk.frameLength = AVAudioFrameCount(framesPerPacket)
            leftFIFO.withUnsafeBufferPointer { ptr in
                _ = memcpy(channelData[0], ptr.baseAddress!, framesPerPacket * MemoryLayout<Float>.size)
            }
            rightFIFO.withUnsafeBufferPointer { ptr in
                _ = memcpy(channelData[1], ptr.baseAddress!, framesPerPacket * MemoryLayout<Float>.size)
            }
            leftFIFO.removeFirst(framesPerPacket)
            rightFIFO.removeFirst(framesPerPacket)

            let compressed = AVAudioCompressedBuffer(format: aacFormat, packetCapacity: 2, maximumPacketSize: 1024)
            var provided = false
            var error: NSError?
            let status = converter.convert(to: compressed, error: &error) { _, outStatus in
                if !provided {
                    provided = true
                    outStatus.pointee = .haveData
                    return chunk
                }
                outStatus.pointee = .noDataNow
                return nil
            }
            guard error == nil, status != .error, compressed.packetCount > 0 else { continue }
            if formatHeader == nil, let cookie = converter.magicCookie ?? aacFormat.magicCookie, !cookie.isEmpty {
                formatHeader = Self.packHeader(
                    formatID: formatID, sampleRate: 48_000, channels: 2,
                    framesPerPacket: UInt32(framesPerPacket), cookie: cookie
                )
            }
            let base = compressed.data.assumingMemoryBound(to: UInt8.self)
            if let descs = compressed.packetDescriptions {
                for index in 0..<Int(compressed.packetCount) {
                    let desc = descs[index]
                    let raw = Data(bytes: base.advanced(by: Int(desc.mStartOffset)), count: Int(desc.mDataByteSize))
                    output.append(Self.packFrame(sequence: sequence, payload: raw))
                    sequence &+= 1
                }
            } else if compressed.byteLength > 0 {
                let raw = Data(bytes: base, count: Int(compressed.byteLength))
                output.append(Self.packFrame(sequence: sequence, payload: raw))
                sequence &+= 1
            }
        }
        return output
    }

    static func packHeader(
        formatID: UInt32, sampleRate: UInt32, channels: UInt16, framesPerPacket: UInt32, cookie: Data
    ) -> Data {
        var header = Data(capacity: 14 + cookie.count)
        for value in [formatID.bigEndian, sampleRate.bigEndian] {
            var v = value
            Swift.withUnsafeBytes(of: &v) { header.append(contentsOf: $0) }
        }
        var ch = channels.bigEndian
        Swift.withUnsafeBytes(of: &ch) { header.append(contentsOf: $0) }
        var fpp = framesPerPacket.bigEndian
        Swift.withUnsafeBytes(of: &fpp) { header.append(contentsOf: $0) }
        header.append(cookie)
        return header
    }

    static func packFrame(sequence: UInt32, payload: Data) -> Data {
        var data = Data(capacity: 4 + payload.count)
        var seq = sequence.bigEndian
        Swift.withUnsafeBytes(of: &seq) { data.append(contentsOf: $0) }
        data.append(payload)
        return data
    }

    private static func extractStereoFloat32(from sample: CMSampleBuffer) -> (left: [Float], right: [Float])? {
        guard let desc = CMSampleBufferGetFormatDescription(sample),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc)?.pointee else { return nil }
        let frames = CMSampleBufferGetNumSamples(sample)
        guard frames > 0 else { return nil }

        var block: CMBlockBuffer?
        var listSize = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: &listSize, bufferListOut: nil, bufferListSize: 0,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &block
        )
        let raw = UnsafeMutableRawPointer.allocate(byteCount: listSize, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { raw.deallocate() }
        let listPtr = raw.bindMemory(to: AudioBufferList.self, capacity: 1)
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: nil, bufferListOut: listPtr, bufferListSize: listSize,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil,
            flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, blockBufferOut: &block
        ) == noErr else { return nil }

        let buffers = UnsafeMutableAudioBufferListPointer(listPtr)
        let isNonInterleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        if isNonInterleaved, buffers.count >= 1, let ch0 = buffers[0].mData {
            let p0 = ch0.assumingMemoryBound(to: Float.self)
            let left = Array(UnsafeBufferPointer(start: p0, count: frames))
            if buffers.count >= 2, let ch1 = buffers[1].mData {
                let p1 = ch1.assumingMemoryBound(to: Float.self)
                let right = Array(UnsafeBufferPointer(start: p1, count: frames))
                return (left, right)
            }
            return (left, left)
        } else if let data = buffers.first?.mData {
            let channels = max(1, Int(asbd.mChannelsPerFrame))
            let ptr = data.assumingMemoryBound(to: Float.self)
            var left = [Float](repeating: 0, count: frames)
            var right = [Float](repeating: 0, count: frames)
            for i in 0..<frames {
                left[i] = ptr[i * channels]
                right[i] = ptr[i * channels + (channels > 1 ? 1 : 0)]
            }
            return (left, right)
        }
        return nil
    }
}

/// Decodes `LowDelayAACEncoder` packets back into standard non-interleaved Float32 PCM buffers.
final class LowDelayAACDecoder {
    private var converter: AVAudioConverter?
    private var aacFormat: AVAudioFormat?
    let pcmFormat: AVAudioFormat
    private(set) var framesPerPacket: AVAudioFrameCount = 480

    init?() {
        guard let pcm = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2) else { return nil }
        self.pcmFormat = pcm
    }

    func configure(header: Data) -> Bool {
        guard header.count > 14 else { return false }
        let formatID = header.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 0, as: UInt32.self).bigEndian }
        let sampleRate = header.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 4, as: UInt32.self).bigEndian }
        let channels = header.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 8, as: UInt16.self).bigEndian }
        let fpp = header.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 10, as: UInt32.self).bigEndian }
        let cookie = header.subdata(in: 14..<header.count)

        var asbd = AudioStreamBasicDescription(
            mSampleRate: Float64(sampleRate),
            mFormatID: formatID,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: fpp,
            mBytesPerFrame: 0,
            mChannelsPerFrame: UInt32(channels),
            mBitsPerChannel: 0,
            mReserved: 0
        )
        var desc: CMAudioFormatDescription?
        let status = cookie.withUnsafeBytes { raw in
            CMAudioFormatDescriptionCreate(
                allocator: nil, asbd: &asbd, layoutSize: 0, layout: nil,
                magicCookieSize: cookie.count, magicCookie: raw.baseAddress,
                extensions: nil, formatDescriptionOut: &desc
            )
        }
        guard status == noErr, let desc else { return false }
        let source = AVAudioFormat(cmAudioFormatDescription: desc)
        guard let conv = AVAudioConverter(from: source, to: pcmFormat) else { return false }
        self.aacFormat = source
        self.converter = conv
        self.framesPerPacket = AVAudioFrameCount(max(256, fpp))
        return true
    }

    func decode(frame: Data) -> AVAudioPCMBuffer? {
        guard frame.count > 4, let converter, let aacFormat else { return nil }
        let payload = frame.subdata(in: 4..<frame.count)
        let compressed = AVAudioCompressedBuffer(
            format: aacFormat, packetCapacity: 1, maximumPacketSize: max(1024, payload.count)
        )
        payload.withUnsafeBytes { raw in
            _ = memcpy(compressed.data, raw.baseAddress!, payload.count)
        }
        compressed.byteLength = UInt32(payload.count)
        compressed.packetCount = 1
        compressed.packetDescriptions?.pointee = AudioStreamPacketDescription(
            mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: UInt32(payload.count)
        )

        guard let outPCM = AVAudioPCMBuffer(pcmFormat: pcmFormat, frameCapacity: framesPerPacket * 2) else {
            return nil
        }
        var supplied = false
        var error: NSError?
        let status = converter.convert(to: outPCM, error: &error) { _, outStatus in
            if !supplied {
                supplied = true
                outStatus.pointee = .haveData
                return compressed
            }
            outStatus.pointee = .noDataNow
            return nil
        }
        guard error == nil, status != .error, outPCM.frameLength > 0 else { return nil }
        return outPCM
    }
}

/// Independent low-latency audio player (~30 ms jitter target) decoupled from video rendering.
@MainActor
final class LiveAudioPlayer {
    private let engine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let decoder = LowDelayAACDecoder()
    private var started = false
    private var queued = 0

    var isMuted: Bool = false {
        didSet { engine.mainMixerNode.outputVolume = isMuted ? 0 : 1 }
    }

    init() {
        if let decoder {
            engine.attach(node)
            engine.connect(node, to: engine.mainMixerNode, format: decoder.pcmFormat)
        }
    }

    func setFormat(_ header: Data) {
        stop()
        _ = decoder?.configure(header: header)
    }

    func enqueue(_ frame: Data) {
        guard let decoder, let pcm = decoder.decode(frame: frame) else { return }
        // Cap queued latency at ~90 ms (9 x 10 ms packets) so network bursts never cause permanent lag.
        if queued > 9 { return }
        if !engine.isRunning {
            do {
                try engine.start()
                engine.mainMixerNode.outputVolume = isMuted ? 0 : 1
            } catch {
                return
            }
        }
        queued += 1
        node.scheduleBuffer(pcm) { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                self.queued = max(0, self.queued - 1)
            }
        }
        // Start playback once 3 packets (~30 ms) are buffered to absorb micro-jitter cleanly.
        if !started, queued >= 3 {
            node.play()
            started = true
        } else if started, !node.isPlaying, queued >= 2 {
            node.play()
        }
    }

    func stop() {
        started = false
        queued = 0
        if engine.isRunning {
            node.stop()
            engine.stop()
        }
    }
}

private final class AudioOnlyOutput: NSObject, SCStreamOutput, SCStreamDelegate, Sendable {
    private struct Sample: @unchecked Sendable { let buffer: CMSampleBuffer }
    private let continuation: AsyncStream<CMSampleBuffer>.Continuation

    init(_ continuation: AsyncStream<CMSampleBuffer>.Continuation) {
        self.continuation = continuation
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, CMSampleBufferDataIsReady(sample) else { return }
        let wrapped = Sample(buffer: sample)
        continuation.yield(wrapped.buffer)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        continuation.finish()
    }
}

/// Captures system audio and writes `AAC-ELD` records to its own dedicated Unix socket -> QUIC stream.
func streamDesktopAudio(socket: String) async {
    let connection = NWConnection(to: .unix(path: socket), using: .tcp)
    let queue = DispatchQueue(label: "KoKo.audio.capture")
    let (samples, continuation) = AsyncStream<CMSampleBuffer>.makeStream(bufferingPolicy: .bufferingNewest(32))
    connection.start(queue: queue)
    connection.receive(minimumIncompleteLength: 1, maximumLength: 1) { _, _, _, _ in
        continuation.finish()
    }
    defer { connection.cancel() }

    do {
        guard CGPreflightScreenCaptureAccess() else { throw ScreenCaptureError.permissionRequired }
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
            throw ScreenCaptureError.displayUnavailable
        }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.showsCursor = false
        config.capturesAudio = true
        config.sampleRate = 48_000
        config.channelCount = 2

        let delegate = AudioOnlyOutput(continuation)
        let stream = SCStream(filter: filter, configuration: config, delegate: delegate)
        try stream.addStreamOutput(delegate, type: .audio, sampleHandlerQueue: queue)
        try await stream.startCapture()
        do {
            let encoder = try LowDelayAACEncoder()
            var sentHeader = false
            for await sample in samples {
                let packets = encoder.encode(sample)
                if !sentHeader, let header = encoder.formatHeader {
                    sendMediaRecord(.audioFormat, bytes: header, over: connection)
                    sentHeader = true
                }
                for packet in packets {
                    sendMediaRecord(.audioFrame, bytes: packet, over: connection)
                }
            }
        } catch {
            try? await stream.stopCapture()
            throw error
        }
        try? await stream.stopCapture()
    } catch {
        let code = (error as? ScreenCaptureError)?.rawValue ?? "capture_failed"
        sendMediaRecord(.error, bytes: Data(code.utf8), over: connection)
    }
}

private func sendMediaRecord(_ kind: VideoRecord, bytes: Data, over connection: NWConnection) {
    var packet = Data(capacity: 5 + bytes.count)
    var bigEndian = UInt32(bytes.count + 1).bigEndian
    Swift.withUnsafeBytes(of: &bigEndian) { packet.append(contentsOf: $0) }
    packet.append(kind.rawValue)
    packet.append(bytes)
    connection.send(content: packet, completion: .idempotent)
}
