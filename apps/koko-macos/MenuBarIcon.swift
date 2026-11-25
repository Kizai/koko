import AppKit

/// The menu bar version of the KoKo logo: the door and the K drawn as one template image, so
/// macOS tints it for light and dark menu bars. A dot beside it means this Mac is being
/// controlled by a person or an agent.
enum MenuBarIcon {
    /// The image is as tall as other menu bar icons' canvas; the glyph inside is drawn smaller,
    /// matching the visual size of system symbols such as Wi‑Fi and Control Center.
    static let height: CGFloat = 18
    private static let glyph: CGFloat = 12
    private static let padding: CGFloat = 1

    static func image(active: Bool) -> NSImage {
        let width = glyph + padding * 2 + (active ? 4.5 : 0)
        let image = NSImage(size: NSSize(width: width, height: height), flipped: true) { _ in
            draw(active: active)
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = active ? "KoKo，正在被控制" : "KoKo"
        return image
    }

    /// Shapes in a unit square, traced from the app icon and simplified for small sizes: the
    /// door is a trapezoid with a knob, and the K's two arms are kept apart by a gap.
    private static let door: [CGPoint] = [
        CGPoint(x: 0, y: 0), CGPoint(x: 0.36, y: 0.17), CGPoint(x: 0.36, y: 0.83), CGPoint(x: 0, y: 1),
    ]
    private static let upperArm: [CGPoint] = [
        CGPoint(x: 0.74, y: 0), CGPoint(x: 1, y: 0), CGPoint(x: 0.5, y: 0.58),
        CGPoint(x: 0.5, y: 0.3),
    ]
    private static let lowerArm: [CGPoint] = [
        CGPoint(x: 0.71, y: 0.55), CGPoint(x: 1, y: 1), CGPoint(x: 0.74, y: 1),
        CGPoint(x: 0.5, y: 0.78),
    ]
    private static let knob = (center: CGPoint(x: 0.21, y: 0.5), radius: 0.1)

    private static func draw(active: Bool) {
        // A thin rounded stroke softens the corners the way the logo does; the inset keeps it inside.
        let stroke: CGFloat = 0.9
        let side = glyph - stroke
        let origin = CGPoint(x: padding + stroke / 2, y: (height - glyph) / 2 + stroke / 2)
        func point(_ p: CGPoint) -> CGPoint { CGPoint(x: origin.x + p.x * side, y: origin.y + p.y * side) }

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
        let radius = knob.radius * side
        NSBezierPath(ovalIn: NSRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2))
            .fill()
        context?.compositingOperation = .sourceOver
        if active {
            let x = padding * 2 + glyph + 0.5
            NSBezierPath(ovalIn: NSRect(x: x, y: origin.y - stroke / 2, width: 3.5, height: 3.5)).fill()
        }
    }
}
