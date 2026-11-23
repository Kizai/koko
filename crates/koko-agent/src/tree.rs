//! Presents a host's accessibility tree to a model and maps elements back to click points.

use std::fmt::Write;

use koko_proto::UiNode;

/// Deeper levels are still listed, just without more indentation, to keep the text short.
const MAX_INDENT: usize = 8;

/// One line per element: `[id] Role "label" = "value" flags`. Top-level elements name their display.
pub fn format_tree(app: &str, nodes: &[UiNode], truncated: bool) -> String {
    let mut text = format!(
        "App: {app}. Use an element's [id] as element_id in koko_mouse until the next koko_get_ui_tree call.\n"
    );
    let mut depths = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let depth = node
            .parent
            .and_then(|parent| depths.get(parent as usize))
            .map_or(0, |depth: &usize| depth + 1);
        depths.push(depth);
        let _ = write!(
            text,
            "{}[{index}] {}",
            "  ".repeat(depth.min(MAX_INDENT)),
            node.role
        );
        if let Some(label) = &node.label {
            let _ = write!(text, " {label:?}");
        }
        if let Some(value) = &node.value {
            let _ = write!(text, " = {value:?}");
        }
        if node.focused {
            text.push_str(" focused");
        }
        if !node.enabled {
            text.push_str(" disabled");
        }
        if node.parent.is_none() {
            let _ = write!(text, " (display {})", node.display);
        }
        text.push('\n');
    }
    if nodes.is_empty() {
        text.push_str("No visible elements were found. The app may not support accessibility; use a screenshot instead.\n");
    }
    if truncated {
        text.push_str(
            "More elements were left out. Raise max_nodes (up to 1000) or close other windows.\n",
        );
    }
    text
}

/// The element's center in protocol coordinates on its display.
pub fn element_center(node: &UiNode) -> (u16, u16) {
    let center =
        |start: u16, length: u16| (u32::from(start) + u32::from(length) / 2).min(9_999) as u16;
    (center(node.x, node.width), center(node.y, node.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, label: Option<&str>, parent: Option<u32>) -> UiNode {
        UiNode {
            role: role.into(),
            label: label.map(Into::into),
            value: None,
            parent,
            enabled: true,
            focused: false,
            display: 2,
            x: 1000,
            y: 9000,
            width: 400,
            height: 2000,
        }
    }

    #[test]
    fn lists_elements_with_their_ids() {
        let mut field = node("TextField", Some("Address"), Some(0));
        field.value = Some("github.com".into());
        field.focused = true;
        let mut back = node("Button", Some("Back"), Some(0));
        back.enabled = false;
        let text = format_tree(
            "Safari",
            &[node("Window", Some("GitHub"), None), field, back],
            true,
        );
        assert!(text.contains("[0] Window \"GitHub\" (display 2)"), "{text}");
        assert!(
            text.contains("  [1] TextField \"Address\" = \"github.com\" focused"),
            "{text}"
        );
        assert!(text.contains("  [2] Button \"Back\" disabled"), "{text}");
        assert!(text.contains("max_nodes"), "{text}");
    }

    #[test]
    fn clicks_the_middle_of_an_element() {
        assert_eq!(element_center(&node("Button", None, None)), (1200, 9999));
    }
}
