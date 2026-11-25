import CoreMedia
import CoreVideo
import Metal
import os
import QuartzCore
@preconcurrency import VideoToolbox

/// What the controller knows about the stream it is showing, for the network panel.
struct VideoPipelineInfo: Equatable {
    var codec: String?
    var chroma: String?
    var hardwareDecode: Bool?
    /// Reported by hosts that support feedback.
    var hardwareEncode: Bool?
    var capture: String?
    /// The bitrate the host currently encodes at, which drops while the link is congested.
    var targetBitrate: Int?
}

/// A VideoToolbox decoder that hands back each decoded frame before returning.
final class VideoDecoder {
    private let session: VTDecompressionSession
    let isHardware: Bool

    init(format: CMVideoFormatDescription) throws {
        // Metal reads the planes directly; full-range 4:4:4 and 4:2:0 are what KoKo hosts send,
        // video range covers older H.264 hosts.
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: [
                kCVPixelFormatType_444YpCbCr8BiPlanarFullRange,
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            ],
            kCVPixelBufferMetalCompatibilityKey: true,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        let specification: [CFString: Any] = [
            kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: true,
        ]
        var created: VTDecompressionSession?
        guard VTDecompressionSessionCreate(
            allocator: nil, formatDescription: format, decoderSpecification: specification as CFDictionary,
            imageBufferAttributes: attributes as CFDictionary, outputCallback: nil, decompressionSessionOut: &created
        ) == noErr, let created else { throw VideoError.codec }
        session = created
        var hardware: CFTypeRef?
        VTSessionCopyProperty(created, key: kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                              allocator: nil, valueOut: &hardware)
        isHardware = (hardware as? Bool) ?? false
    }

    deinit { VTDecompressionSessionInvalidate(session) }

    /// Decodes one frame. Returns the decoder's status so a missing reference frame can be told
    /// apart from a session that must be recreated.
    func decode(_ sample: CMSampleBuffer) -> (OSStatus, CVPixelBuffer?) {
        final class Output: @unchecked Sendable {
            var status: OSStatus = noErr
            var pixels: CVPixelBuffer?
        }
        let output = Output()
        let status = VTDecompressionSessionDecodeFrame(
            session, sampleBuffer: sample, flags: [], infoFlagsOut: nil
        ) { status, _, image, _, _ in
            output.status = status
            output.pixels = image
        }
        // Without asynchronous decompression the handler has run by now; this makes sure of it.
        VTDecompressionSessionWaitForAsynchronousFrames(session)
        return (status != noErr ? status : output.status, output.pixels)
    }
}

/// Converts the decoder's YCbCr planes to RGB and draws them aspect-fit into a target.
final class VideoDrawing {
    let device: MTLDevice
    private let pipeline: MTLRenderPipelineState
    private let textures: CVMetalTextureCache

    private static let shader = """
    #include <metal_stdlib>
    using namespace metal;

    struct Vertex { float4 position [[position]]; float2 uv; };
    struct Colors { float videoRange; float rec601; };

    vertex Vertex koko_vertex(uint id [[vertex_id]]) {
        float2 p = float2((id & 1) ? 1.0 : -1.0, (id & 2) ? -1.0 : 1.0);
        Vertex out;
        out.position = float4(p, 0.0, 1.0);
        out.uv = float2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
        return out;
    }

    fragment float4 koko_fragment(Vertex in [[stage_in]],
                                  texture2d<float> luma [[texture(0)]],
                                  texture2d<float> chroma [[texture(1)]],
                                  constant Colors &colors [[buffer(0)]]) {
        constexpr sampler linear(filter::linear, address::clamp_to_edge);
        float y = luma.sample(linear, in.uv).r;
        float2 c = chroma.sample(linear, in.uv).rg - float2(128.0 / 255.0);
        if (colors.videoRange > 0.5) {
            y = (y - 16.0 / 255.0) * (255.0 / 219.0);
            c *= 255.0 / 224.0;
        }
        float3 rgb = colors.rec601 > 0.5
            ? float3(y + 1.402 * c.y, y - 0.344136 * c.x - 0.714136 * c.y, y + 1.772 * c.x)
            : float3(y + 1.5748 * c.y, y - 0.187324 * c.x - 0.468124 * c.y, y + 1.8556 * c.x);
        return float4(saturate(rgb), 1.0);
    }
    """

    init() throws {
        guard let device = MTLCreateSystemDefaultDevice() else { throw VideoError.codec }
        self.device = device
        let library = try device.makeLibrary(source: Self.shader, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "koko_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "koko_fragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        var cache: CVMetalTextureCache?
        guard CVMetalTextureCacheCreate(nil, nil, device, nil, &cache) == kCVReturnSuccess, let cache else {
            throw VideoError.codec
        }
        textures = cache
    }

    /// Encodes the drawing of `pixels` into `target`, letterboxed to keep its aspect ratio.
    /// Returns the textures the GPU reads, which must live until the command buffer completes.
    func encode(_ pixels: CVPixelBuffer?, into target: MTLTexture, commands: MTLCommandBuffer) -> [CVMetalTexture] {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { return [] }
        defer { encoder.endEncoding() }
        guard let pixels, let luma = texture(pixels, plane: 0, format: .r8Unorm),
              let chroma = texture(pixels, plane: 1, format: .rg8Unorm),
              let lumaTexture = CVMetalTextureGetTexture(luma), let chromaTexture = CVMetalTextureGetTexture(chroma)
        else { return [] }
        let width = Double(CVPixelBufferGetWidth(pixels)), height = Double(CVPixelBufferGetHeight(pixels))
        let scale = min(Double(target.width) / width, Double(target.height) / height)
        let fitted = (width * scale, height * scale)
        encoder.setViewport(MTLViewport(
            originX: ((Double(target.width) - fitted.0) / 2).rounded(), originY: ((Double(target.height) - fitted.1) / 2).rounded(),
            width: fitted.0.rounded(), height: fitted.1.rounded(), znear: 0, zfar: 1
        ))
        let format = CVPixelBufferGetPixelFormatType(pixels)
        let matrix = CVBufferCopyAttachment(pixels, kCVImageBufferYCbCrMatrixKey, nil) as? String
        var colors: (Float, Float) = (
            format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange ? 1 : 0,
            matrix == (kCVImageBufferYCbCrMatrix_ITU_R_601_4 as String) ? 1 : 0
        )
        encoder.setRenderPipelineState(pipeline)
        encoder.setFragmentTexture(lumaTexture, index: 0)
        encoder.setFragmentTexture(chromaTexture, index: 1)
        encoder.setFragmentBytes(&colors, length: MemoryLayout<(Float, Float)>.size, index: 0)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        return [luma, chroma]
    }

    private func texture(_ pixels: CVPixelBuffer, plane: Int, format: MTLPixelFormat) -> CVMetalTexture? {
        var texture: CVMetalTexture?
        guard CVMetalTextureCacheCreateTextureFromImage(
            nil, textures, pixels, nil, format,
            CVPixelBufferGetWidthOfPlane(pixels, plane), CVPixelBufferGetHeightOfPlane(pixels, plane), plane, &texture
        ) == kCVReturnSuccess else { return nil }
        return texture
    }
}

/// Decodes and draws remote video off the main thread. Every frame is decoded, since later ones
/// are predicted from it, and drawn the moment it is ready. AVSampleBufferDisplayLayer queued
/// frames for its own clock; here a frame that arrives while the display is still busy with the
/// previous one replaces it, so the picture follows the newest frame at any refresh rate.
final class MetalVideoRenderer: @unchecked Sendable {
    let layer = CAMetalLayer()
    private let queue = DispatchQueue(label: "KoKo.video.render", qos: .userInteractive)
    private let drawing: VideoDrawing?
    private let commands: MTLCommandQueue?

    private struct Shared {
        var waiting = 0
        var failed = false
        var needsKeyFrame = false
        var drawableSize = CGSize.zero
        var info = VideoPipelineInfo()
    }
    private let shared = OSAllocatedUnfairLock(initialState: Shared())

    // Only touched on `queue`.
    private var decoder: VideoDecoder?
    private var format: CMVideoFormatDescription?
    private var latest: CVPixelBuffer?
    private var drawsInFlight = 0
    private var lastDrawAt = 0.0
    private var stale = false

    init() {
        drawing = try? VideoDrawing()
        commands = drawing?.device.makeCommandQueue()
        layer.device = drawing?.device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.isOpaque = false
        layer.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        layer.maximumDrawableCount = 3
        if drawing == nil { shared.withLock { $0.failed = true } }
    }

    /// Frames waiting to be decoded; past a few the display cannot keep up.
    var canAccept: Bool { shared.withLock { $0.waiting < 6 && !$0.failed } }
    var hasFailed: Bool { shared.withLock { $0.failed } }
    var info: VideoPipelineInfo { shared.withLock { $0.info } }

    /// Whether a frame could not be decoded since the last call, so a key frame is needed.
    func takeKeyFrameNeed() -> Bool {
        shared.withLock { state in
            defer { state.needsKeyFrame = false }
            return state.needsKeyFrame
        }
    }

    func setFormat(_ description: CMVideoFormatDescription) {
        let codec = CMFormatDescriptionGetMediaSubType(description) == kCMVideoCodecType_HEVC ? "H.265" : "H.264"
        shared.withLock { $0.info.codec = codec }
        queue.async { [self] in
            format = description
            decoder = nil
        }
    }

    func setStreamInfo(_ update: VideoStreamInfo) {
        shared.withLock {
            if let encoder = update.encoder { $0.info.hardwareEncode = encoder == "hardware" }
            if let capture = update.capture { $0.info.capture = capture }
            if let bitrate = update.bitrate { $0.info.targetBitrate = bitrate }
        }
    }

    func decode(_ sample: CMSampleBuffer) {
        shared.withLock { $0.waiting += 1 }
        let box = SampleBox(sample)
        queue.async { [self] in
            defer { shared.withLock { $0.waiting -= 1 } }
            decodeNow(box.sample)
        }
    }

    /// Called on the main thread when the view's size or scale changes.
    func resize(to size: CGSize, scale: CGFloat) {
        let pixels = CGSize(width: max(1, (size.width * scale).rounded()), height: max(1, (size.height * scale).rounded()))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layer.contentsScale = scale
        CATransaction.commit()
        shared.withLock { $0.drawableSize = pixels }
        queue.async { [self] in draw() }
    }

    func clear() {
        shared.withLock {
            $0.failed = drawing == nil
            $0.needsKeyFrame = false
            $0.info = VideoPipelineInfo()
        }
        queue.async { [self] in
            decoder = nil
            format = nil
            latest = nil
            draw()
        }
    }

    private func decodeNow(_ sample: CMSampleBuffer) {
        guard let format else { return }
        if decoder == nil {
            do {
                decoder = try VideoDecoder(format: format)
            } catch {
                shared.withLock { $0.failed = true }
                return
            }
        }
        guard let decoder else { return }
        let (status, pixels) = decoder.decode(sample)
        if status == kVTInvalidSessionErr {
            // The session can be lost when the Mac sleeps; a new one starts at the next key frame.
            self.decoder = nil
            shared.withLock { $0.needsKeyFrame = true }
            return
        }
        guard status == noErr, let pixels else {
            shared.withLock { $0.needsKeyFrame = true }
            return
        }
        let chroma = CVPixelBufferGetPixelFormatType(pixels) == kCVPixelFormatType_444YpCbCr8BiPlanarFullRange ? "4:4:4" : "4:2:0"
        let hardware = decoder.isHardware
        shared.withLock {
            $0.info.chroma = chroma
            $0.info.hardwareDecode = hardware
        }
        latest = pixels
        draw()
    }

    /// Draws the newest frame, unless two drawings are still on their way to the display; the
    /// first of those to be shown then draws whatever is newest.
    private func draw() {
        guard let drawing, let commands else { return }
        let now = ProcessInfo.processInfo.systemUptime
        // A drawable that is never shown, such as in a hidden window, may never report back.
        if drawsInFlight >= 2, now - lastDrawAt > 0.1 { drawsInFlight = 0 }
        guard drawsInFlight < 2 else {
            stale = true
            return
        }
        let size = shared.withLock { $0.drawableSize }
        guard size.width >= 1, size.height >= 1 else { return }
        if layer.drawableSize != size { layer.drawableSize = size }
        guard let drawable = layer.nextDrawable(), let buffer = commands.makeCommandBuffer() else { return }
        let textures = drawing.encode(latest, into: drawable.texture, commands: buffer)
        drawsInFlight += 1
        lastDrawAt = now
        stale = false
        drawable.addPresentedHandler { [self] _ in
            queue.async { [self] in
                drawsInFlight = max(0, drawsInFlight - 1)
                if stale { draw() }
            }
        }
        buffer.addCompletedHandler { _ in _ = textures }
        buffer.present(drawable)
        buffer.commit()
    }
}

/// A sample buffer handed to the render queue; nothing else touches it afterwards.
private struct SampleBox: @unchecked Sendable {
    let sample: CMSampleBuffer
    init(_ sample: CMSampleBuffer) { self.sample = sample }
}
