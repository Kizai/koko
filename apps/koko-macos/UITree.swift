import AppKit
@preconcurrency import ApplicationServices

/// One accessibility element, as sent to an agent. Frames are 0-9999 on the element's display.
struct UINode: Codable, Sendable {
    let role: String
    var label: String?
    var value: String?
    var parent: Int?
    var enabled = true
    var focused = false
    let display: UInt32
    let x: Int
    let y: Int
    let width: Int
    let height: Int
}

private let interactiveRoles: Set<String> = [
    "AXButton", "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXMenuButton", "AXTextField",
    "AXTextArea", "AXComboBox", "AXSlider", "AXIncrementor", "AXLink", "AXMenuItem", "AXTab",
    "AXDisclosureTriangle", "AXCell", "AXRow", "AXColorWell", "AXDateField", "AXSegmentedControl",
]
private let textRoles: Set<String> = ["AXStaticText", "AXHeading", "AXImage", "AXWindow", "AXSheet", "AXDialog"]
private let maxVisited = 6000
private let maxText = 200

/// Reads the visible, useful part of an app's accessibility tree, depth first.
/// Menus are skipped because they are long and an agent can reach them with shortcuts.
/// Slow or unresponsive apps are cut off by a messaging timeout and an overall time budget.
func readUITree(pid: pid_t, maxNodes: Int) -> (nodes: [UINode], truncated: Bool) {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 0.5)
    let focused: AXUIElement? = attribute(app, kAXFocusedUIElementAttribute)
    var displayIDs = [CGDirectDisplayID](repeating: 0, count: 16)
    var displayCount: UInt32 = 0
    CGGetActiveDisplayList(16, &displayIDs, &displayCount)
    let displays = displayIDs.prefix(Int(displayCount)).map { ($0, CGDisplayBounds($0)) }
    let deadline = Date().addingTimeInterval(2)
    var nodes: [UINode] = []
    var visited = 0
    var truncated = false

    func visit(_ element: AXUIElement, parent: Int?, depth: Int) {
        guard !truncated else { return }
        guard visited < maxVisited, depth < 40, Date() < deadline else {
            truncated = true
            return
        }
        visited += 1
        let role: String = attribute(element, kAXRoleAttribute) ?? ""
        if role == "AXMenuBar" { return }
        var index = parent
        if let node = makeNode(element, role: role, parent: parent, focused: focused, displays: displays) {
            guard nodes.count < maxNodes else {
                truncated = true
                return
            }
            nodes.append(node)
            index = nodes.count - 1
        }
        let children: [AXUIElement] = attribute(element, kAXChildrenAttribute) ?? []
        for child in children { visit(child, parent: index, depth: depth + 1) }
    }

    visit(app, parent: nil, depth: 0)
    return (nodes, truncated)
}

private func makeNode(
    _ element: AXUIElement, role: String, parent: Int?, focused: AXUIElement?,
    displays: [(CGDirectDisplayID, CGRect)]
) -> UINode? {
    guard interactiveRoles.contains(role) || textRoles.contains(role) else { return nil }
    guard let frame = frame(of: element), frame.width > 0, frame.height > 0,
          let (display, bounds) = displays.first(where: { $0.1.contains(CGPoint(x: frame.midX, y: frame.midY)) })
    else { return nil }
    let subrole: String? = attribute(element, kAXSubroleAttribute)
    let secure = subrole == "AXSecureTextField"
    let title: String? = attribute(element, kAXTitleAttribute)
    let description: String? = attribute(element, kAXDescriptionAttribute)
    let placeholder: String? = attribute(element, kAXPlaceholderValueAttribute)
    var label = [title, description, placeholder].lazy.compactMap { clean($0) }.first
    var value = secure ? nil : textValue(element)
    if role == "AXStaticText" || role == "AXHeading" {
        label = label ?? value
        value = nil
    }
    // Containers, rows and pictures without words do not help an agent find anything;
    // their labelled children carry the same frame information.
    let wordless = label == nil && value == nil
    if wordless && (!interactiveRoles.contains(role) || role == "AXRow" || role == "AXCell") { return nil }
    let enabled: Bool = attribute(element, kAXEnabledAttribute) ?? true
    let normalize = { (offset: CGFloat, length: CGFloat) in
        min(9999, max(0, Int(offset / length * 10_000)))
    }
    let named = secure ? "SecureTextField" : (subrole == "AXSearchField" ? "SearchField" : role)
    return UINode(
        role: named.hasPrefix("AX") ? String(named.dropFirst(2)) : named,
        label: label, value: value, parent: parent, enabled: enabled,
        focused: focused.map { CFEqual($0, element) } ?? false, display: display,
        x: normalize(frame.minX - bounds.minX, bounds.width), y: normalize(frame.minY - bounds.minY, bounds.height),
        width: max(1, min(10_000, Int(frame.width / bounds.width * 10_000))),
        height: max(1, min(10_000, Int(frame.height / bounds.height * 10_000)))
    )
}

private func attribute<T>(_ element: AXUIElement, _ name: String) -> T? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value as? T
}

private func frame(of element: AXUIElement) -> CGRect? {
    guard let position: AXValue = attribute(element, kAXPositionAttribute),
          let size: AXValue = attribute(element, kAXSizeAttribute) else { return nil }
    var origin = CGPoint.zero
    var extent = CGSize.zero
    guard AXValueGetValue(position, .cgPoint, &origin), AXValueGetValue(size, .cgSize, &extent) else { return nil }
    return CGRect(origin: origin, size: extent)
}

private func textValue(_ element: AXUIElement) -> String? {
    var raw: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &raw) == .success,
          let raw else { return nil }
    if let text = raw as? String { return clean(text) }
    if let number = raw as? NSNumber { return number.stringValue }
    return nil
}

private func clean(_ text: String?) -> String? {
    guard let text else { return nil }
    let line = text.components(separatedBy: .newlines).joined(separator: " ").trimmingCharacters(in: .whitespaces)
    guard !line.isEmpty else { return nil }
    return line.count > maxText ? String(line.prefix(maxText)) + "…" : line
}
