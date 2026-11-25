import AppKit

/// The menu bar version of the KoKo logo: the door and the K drawn as one template image, so
/// macOS tints it for light and dark menu bars. A dot beside it means this Mac is being
/// controlled by a person or an agent.
enum MenuBarIcon {
    static let height: CGFloat = 18

    static func image(active: Bool) -> NSImage {
        let size = NSSize(width: active ? 23 : height, height: height)
        let image = NSImage(size: size, flipped: true) { _ in
            draw(active: active)
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = active ? "KoKo，正在被控制" : "KoKo"
        return image
    }

    /// Shapes in a unit square, traced from the app icon and simplified for 18 points: the
    /// door is a trapezoid with a knob, and the K's two arms are kept apart by a gap.
    private static let door: [CGPoint] = [
        CGPoint(x: 0, y: 0), CGPoint(x: 0.38, y: 0.17), CGPoint(x: 0.38, y: 0.83), CGPoint(x: 0, y: 1),
    ]
    private static let upperArm: [CGPoint] = [
        CGPoint(x: 0.72, y: 0.02), CGPoint(x: 1, y: 0.02), CGPoint(x: 0.5, y: 0.6),
        CGPoint(x: 0.5, y: 0.3),
    ]
    private static let lowerArm: [CGPoint] = [
        CGPoint(x: 0.7, y: 0.53), CGPoint(x: 1, y: 0.98), CGPoint(x: 0.72, y: 0.98),
        CGPoint(x: 0.5, y: 0.76),
    ]
    private static let knob = (center: CGPoint(x: 0.24, y: 0.5), radius: 0.075)

    private static func draw(active: Bool) {
        // Rounded strokes soften the corners the way the logo does; the inset keeps them inside.
        let stroke: CGFloat = 1.4
        let inset = stroke / 2 + 0.5
        let side = height - inset * 2
        func point(_ p: CGPoint) -> CGPoint { CGPoint(x: inset + p.x * side, y: inset + p.y * side) }

        NSColor.black.set()
        for shape in [door, upperArm, lowerArm] {
            let path = NSBezierPath()
            path.move(to: point(shape[0]))
            shape.dropFirst().forEach { path.line(to: point($0)) }
            path.close()
            path.lineJoinStyle = .round
            path.lineWidth = stroke
            path.fill()
            path.stroke()
        }
        let context = NSGraphicsContext.current
        context?.compositingOperation = .clear
        let center = point(knob.center)
        let radius = knob.radius * side + 0.4
        NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2))
            .fill()
        context?.compositingOperation = .sourceOver
        if active {
            NSBezierPath(ovalIn: NSRect(x: height + 1, y: 1.5, width: 4, height: 4)).fill()
        }
    }
}
