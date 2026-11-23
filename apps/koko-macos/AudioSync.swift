@preconcurrency import AVFoundation
import AudioToolbox
import CoreAudio
import CoreMedia
import Foundation

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

final class AACStreamEncoder {
    private var converter: AVAudioConverter?
    private var inputFormat: AVAudioFormat?
    private let outputFormat: AVAudioFormat
    private(set) var formatHeader: Data?

    init() throws {
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 48_000,
            AVNumberOfChannelsKey: 2,
            AVEncoderBitRateKey: 128_000,
        ]
        guard let format = AVAudioFormat(settings: settings) else {
            throw VideoError.codec
        }
        self.outputFormat = format
    }

    func encode(_ sample: CMSampleBuffer, basePTS: CMTime) -> [(ptsUs: Int64, durationUs: Int64, data: Data)] {
        guard let desc = CMSampleBufferGetFormatDescription(sample) else { return [] }
        let sourceFormat = AVAudioFormat(cmAudioFormatDescription: desc)
        if inputFormat != sourceFormat {
            inputFormat = sourceFormat
            converter = AVAudioConverter(from: sourceFormat, to: outputFormat)
            converter?.bitRate = 128_000
            formatHeader = nil
        }
        guard let converter else { return [] }

        let frameCount = AVAudioFrameCount(CMSampleBufferGetNumSamples(sample))
        guard frameCount > 0, let pcm = AVAudioPCMBuffer(pcmFormat: sourceFormat, frameCapacity: frameCount) else {
            return []
        }
        pcm.frameLength = frameCount

        var blockBuffer: CMBlockBuffer?
        var listSize = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: &listSize, bufferListOut: nil, bufferListSize: 0,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &blockBuffer
        )
        let rawList = UnsafeMutableRawPointer.allocate(byteCount: listSize, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { rawList.deallocate() }
        let listPtr = rawList.bindMemory(to: AudioBufferList.self, capacity: 1)
        let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: nil, bufferListOut: listPtr, bufferListSize: listSize,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil,
            flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, blockBufferOut: &blockBuffer
        )
        guard status == noErr else { return [] }
        let srcBuffers = UnsafeMutableAudioBufferListPointer(listPtr)
        let dstBuffers = UnsafeMutableAudioBufferListPointer(pcm.mutableAudioBufferList)
        for index in 0..<min(srcBuffers.count, dstBuffers.count) {
            if let src = srcBuffers[index].mData, let dst = dstBuffers[index].mData {
                let bytes = min(Int(srcBuffers[index].mDataByteSize), Int(dstBuffers[index].mDataByteSize))
                memcpy(dst, src, bytes)
                dstBuffers[index].mDataByteSize = UInt32(bytes)
            }
        }

        let startUs = Int64(((sample.presentationTimeStamp - basePTS).seconds * 1_000_000).rounded())
        var packets: [(Int64, Int64, Data)] = []
        var consumed = false
        var offsetUs: Int64 = 0

        while true {
            let compressed = AVAudioCompressedBuffer(format: outputFormat, packetCapacity: 4, maximumPacketSize: 1024)
            var error: NSError?
            let outStatus = converter.convert(to: compressed, error: &error) { _, statusOut in
                if !consumed {
                    consumed = true
                    statusOut.pointee = .haveData
                    return pcm
                }
                statusOut.pointee = .noDataNow
                return nil
            }
            if error != nil || outStatus == .error || compressed.packetCount == 0 {
                break
            }
            if formatHeader == nil, let cookie = converter.magicCookie ?? outputFormat.magicCookie, !cookie.isEmpty {
                formatHeader = Self.makeFormatHeader(sampleRate: 48_000, channels: 2, framesPerPacket: 1024, cookie: cookie)
            }
            if let descs = compressed.packetDescriptions {
                let basePtr = compressed.data.assumingMemoryBound(to: UInt8.self)
                for index in 0..<Int(compressed.packetCount) {
                    let desc = descs[index]
                    let packetData = Data(bytes: basePtr.advanced(by: Int(desc.mStartOffset)), count: Int(desc.mDataByteSize))
                    let durUs: Int64 = 21_333 // 1024 samples at 48 kHz
                    packets.append((startUs + offsetUs, durUs, packetData))
                    offsetUs += durUs
                }
            }
            if outStatus == .inputRanDry || outStatus == .endOfStream {
                break
            }
        }
        return packets
    }

    private static func makeFormatHeader(sampleRate: UInt32, channels: UInt16, framesPerPacket: UInt32, cookie: Data) -> Data {
        var data = Data()
        var sr = sampleRate.bigEndian
        var ch = channels.bigEndian
        var fpp = framesPerPacket.bigEndian
        Swift.withUnsafeBytes(of: &sr) { data.append(contentsOf: $0) }
        Swift.withUnsafeBytes(of: &ch) { data.append(contentsOf: $0) }
        Swift.withUnsafeBytes(of: &fpp) { data.append(contentsOf: $0) }
        data.append(cookie)
        return data
    }
}

enum AudioPacketCodec {
    static func makeFormatDescription(from bytes: Data) -> CMAudioFormatDescription? {
        guard bytes.count >= 10 else { return nil }
        let sampleRate = bytes.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 0, as: UInt32.self).bigEndian }
        let channels = bytes.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 4, as: UInt16.self).bigEndian }
        let framesPerPacket = bytes.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: 6, as: UInt32.self).bigEndian }
        let cookie = bytes.subdata(in: 10..<bytes.count)

        var asbd = AudioStreamBasicDescription(
            mSampleRate: Float64(sampleRate),
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: UInt32(MPEG4ObjectID.AAC_LC.rawValue),
            mBytesPerPacket: 0,
            mFramesPerPacket: framesPerPacket,
            mBytesPerFrame: 0,
            mChannelsPerFrame: UInt32(channels),
            mBitsPerChannel: 0,
            mReserved: 0
        )
        var description: CMAudioFormatDescription?
        let status = cookie.withUnsafeBytes { raw in
            CMAudioFormatDescriptionCreate(
                allocator: nil,
                asbd: &asbd,
                layoutSize: 0,
                layout: nil,
                magicCookieSize: cookie.count,
                magicCookie: raw.baseAddress,
                extensions: nil,
                formatDescriptionOut: &description
            )
        }
        return status == noErr ? description : nil
    }

    static func decodeTimedPayload(_ bytes: Data) -> (ptsUs: Int64, durationUs: Int64, payload: Data)? {
        guard bytes.count > 12 else { return nil }
        let ptsUs = bytes.withUnsafeBytes { Int64(bitPattern: $0.loadUnaligned(fromByteOffset: 0, as: UInt64.self).bigEndian) }
        let durationUs = bytes.withUnsafeBytes { Int64($0.loadUnaligned(fromByteOffset: 8, as: UInt32.self).bigEndian) }
        return (ptsUs, durationUs, bytes.subdata(in: 12..<bytes.count))
    }

    static func encodeTimedPayload(ptsUs: Int64, durationUs: Int64, payload: Data) -> Data {
        var data = Data(capacity: 12 + payload.count)
        var pts = UInt64(bitPattern: ptsUs).bigEndian
        var dur = UInt32(clamping: max(0, durationUs)).bigEndian
        Swift.withUnsafeBytes(of: &pts) { data.append(contentsOf: $0) }
        Swift.withUnsafeBytes(of: &dur) { data.append(contentsOf: $0) }
        data.append(payload)
        return data
    }

    static func decodeVideoTimedPayload(_ bytes: Data) -> (ptsUs: Int64, payload: Data)? {
        guard bytes.count > 8 else { return nil }
        let ptsUs = bytes.withUnsafeBytes { Int64(bitPattern: $0.loadUnaligned(fromByteOffset: 0, as: UInt64.self).bigEndian) }
        return (ptsUs, bytes.subdata(in: 8..<bytes.count))
    }

    static func encodeVideoTimedPayload(ptsUs: Int64, payload: Data) -> Data {
        var data = Data(capacity: 8 + payload.count)
        var pts = UInt64(bitPattern: ptsUs).bigEndian
        Swift.withUnsafeBytes(of: &pts) { data.append(contentsOf: $0) }
        data.append(payload)
        return data
    }

    static func makeSampleBuffer(
        payload: Data,
        format: CMAudioFormatDescription,
        ptsUs: Int64,
        durationUs: Int64
    ) -> CMSampleBuffer? {
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: nil, memoryBlock: nil, blockLength: payload.count,
            blockAllocator: nil, customBlockSource: nil, offsetToData: 0, dataLength: payload.count,
            flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &block
        ) == noErr, let block else { return nil }

        let written = payload.withUnsafeBytes {
            CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: payload.count)
        }
        guard written == noErr else { return nil }

        var packetDesc = AudioStreamPacketDescription(
            mStartOffset: 0,
            mVariableFramesInPacket: 0,
            mDataByteSize: UInt32(payload.count)
        )
        var sample: CMSampleBuffer?
        let status = CMAudioSampleBufferCreateReadyWithPacketDescriptions(
            allocator: nil,
            dataBuffer: block,
            formatDescription: format,
            sampleCount: 1,
            presentationTimeStamp: CMTime(value: ptsUs, timescale: 1_000_000),
            packetDescriptions: &packetDesc,
            sampleBufferOut: &sample
        )
        return status == noErr ? sample : nil
    }
}
