import AppKit
import SwiftUI

// The SwiftUI property wrapper is available when Command Line Tools omit the State macro plug-in.
typealias ViewState<Value> = SwiftUI.State<Value>

struct WindowButtonAlignment: NSViewRepresentable {
    static let leadingInset: CGFloat = 86
    static let defaultHeight: CGFloat = 32

    @Binding var height: CGFloat

    func makeNSView(context: Context) -> MeasurementView { MeasurementView() }

    func updateNSView(_ view: MeasurementView, context: Context) {
        let height = $height
        view.report = { value in
            if height.wrappedValue != value { height.wrappedValue = value }
        }
        view.measure()
    }

    final class MeasurementView: NSView {
        var report: ((CGFloat) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            measure()
        }

        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        private var dragOffset: CGPoint?
        override var mouseDownCanMoveWindow: Bool { false }

        override func mouseDown(with event: NSEvent) {
            guard let window, !window.styleMask.contains(.fullScreen) else { return }
            if event.clickCount == 2 {
                dragOffset = nil
                window.zoom(nil)
                return
            }
            dragOffset = event.locationInWindow
        }

        override func mouseDragged(with event: NSEvent) {
            guard let window, let dragOffset else { return }
            let pointer = window.convertPoint(toScreen: event.locationInWindow)
            window.setFrameOrigin(CGPoint(x: pointer.x - dragOffset.x, y: pointer.y - dragOffset.y))
        }

        override func mouseUp(with event: NSEvent) {
            dragOffset = nil
        }

        func measure() {
            DispatchQueue.main.async { [weak self] in
                guard let self, let window, !window.styleMask.contains(.fullScreen),
                      let content = window.contentView, let button = window.standardWindowButton(.closeButton) else { return }
                let center = button.convert(CGPoint(x: button.bounds.midX, y: button.bounds.midY), to: content)
                let offset = content.isFlipped ? center.y - content.bounds.minY : content.bounds.maxY - center.y
                if (24...56).contains(offset * 2) { report?(offset * 2) }
            }
        }
    }
}
