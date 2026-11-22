import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let iconset = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = points * scale
        let context = CGContext(
            data: nil, width: pixels, height: pixels, bitsPerComponent: 8,
            bytesPerRow: 0, space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        let factor = CGFloat(pixels) / 1024
        context.scaleBy(x: factor, y: factor)

        let tile = CGRect(x: 91, y: 91, width: 842, height: 842)
        context.addPath(CGPath(roundedRect: tile, cornerWidth: 190, cornerHeight: 190, transform: nil))
        context.setFillColor(red: 0.063, green: 0.127, blue: 0.179, alpha: 1)
        context.fillPath()

        context.setStrokeColor(red: 0.451, green: 0.894, blue: 0.778, alpha: 1)
        context.setLineWidth(82)
        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.move(to: CGPoint(x: 335, y: 335))
        context.addLine(to: CGPoint(x: 335, y: 689))
        context.move(to: CGPoint(x: 610, y: 682))
        context.addLines(between: [
            CGPoint(x: 610, y: 682), CGPoint(x: 445, y: 512), CGPoint(x: 610, y: 342),
        ])
        context.strokePath()

        context.setFillColor(red: 0.93, green: 0.98, blue: 0.96, alpha: 1)
        context.fillEllipse(in: CGRect(x: 659, y: 473, width: 78, height: 78))

        let suffix = scale == 1 ? "" : "@2x"
        let output = iconset.appendingPathComponent("icon_\(points)x\(points)\(suffix).png")
        let encoder = CGImageDestinationCreateWithURL(output as CFURL, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(encoder, context.makeImage()!, nil)
        precondition(CGImageDestinationFinalize(encoder))
    }
}
