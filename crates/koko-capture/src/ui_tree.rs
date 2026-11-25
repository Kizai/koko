//! Convert a snapshot of a UI Automation tree into protocol nodes.

use koko_input::coordinates::{PhysicalRect, normalize_frame};
use koko_proto::{MAX_UI_NODES, UiNode};

const MAX_DEPTH: usize = 40;
const MAX_VISITED: usize = 6000;
const MAX_TEXT: usize = 200;

// UIAutomationClient.h ControlType IDs:
// https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controltype-ids
const EDIT: i32 = 50004;
const MENU_BAR: i32 = 50010;

fn protocol_role(control_type_id: i32, is_password: bool) -> Option<&'static str> {
    Some(match control_type_id {
        50000 => "Button",
        50002 => "CheckBox",
        50003 => "ComboBox",
        EDIT if is_password => "SecureTextField",
        EDIT => "TextField",
        50005 => "Link", // Hyperlink
        50006 => "Image",
        50007 | 50024 | 50029 => "Row", // ListItem, TreeItem, or DataItem
        50011 => "MenuItem",
        50013 => "RadioButton",
        50015 => "Slider",
        50016 => "Incrementor",        // Spinner
        50019 => "Tab",                // TabItem
        50020 | 50035 => "StaticText", // Text or HeaderItem
        50031 => "MenuButton",         // SplitButton
        50032 => "Window",
        _ => return None,
    })
}

/// The protocol display ID and physical desktop bounds for a monitor.
pub struct Monitor {
    pub id: u32,
    pub bounds: PhysicalRect,
}

/// UI Automation properties for one element in an owned tree.
pub struct UiElement {
    pub control_type_id: i32,
    pub name: Option<String>,
    /// Fallback label when the name is empty. Supply display text independent of the value.
    pub label: Option<String>,
    pub value: Option<String>,
    /// When true, the value is ignored for every control type.
    pub is_password: bool,
    pub enabled: bool,
    pub focused: bool,
    pub bounds: Option<PhysicalRect>,
    pub children: Vec<Self>,
}

impl UiElement {
    pub fn new(control_type_id: i32) -> Self {
        Self {
            control_type_id,
            name: None,
            label: None,
            value: None,
            is_password: false,
            enabled: true,
            focused: false,
            bounds: None,
            children: Vec::new(),
        }
    }
}

/// Returns protocol nodes in depth-first order and whether traversal hit a limit.
///
/// At most 40 levels and 6000 elements are visited. A node limit of zero emits
/// no nodes. Any requested limit above the protocol maximum is clamped.
/// Bounds must use the same physical desktop space as the monitors. When
/// monitors overlap, the first monitor containing the element center wins.
pub fn flatten_ui_tree(
    root: &UiElement,
    monitors: &[Monitor],
    max_nodes: u16,
) -> (Vec<UiNode>, bool) {
    let mut traversal = Traversal {
        monitors,
        max_nodes: usize::from(max_nodes.min(MAX_UI_NODES)),
        visited: 0,
        nodes: Vec::new(),
    };
    let truncated = traversal.visit(root, None, 0);
    (traversal.nodes, truncated)
}

struct Traversal<'a> {
    monitors: &'a [Monitor],
    max_nodes: usize,
    visited: usize,
    nodes: Vec<UiNode>,
}

impl Traversal<'_> {
    fn visit(&mut self, element: &UiElement, mut parent: Option<u32>, depth: usize) -> bool {
        if depth >= MAX_DEPTH || self.visited >= MAX_VISITED {
            return true;
        }
        self.visited += 1;
        if element.control_type_id == MENU_BAR {
            return false;
        }
        if let Some(node) = element.to_node(parent, self.monitors) {
            if self.nodes.len() >= self.max_nodes {
                return true;
            }
            parent = Some(self.nodes.len() as u32);
            self.nodes.push(node);
        }
        element
            .children
            .iter()
            .any(|child| self.visit(child, parent, depth + 1))
    }
}

impl UiElement {
    fn to_node(&self, parent: Option<u32>, monitors: &[Monitor]) -> Option<UiNode> {
        let role = protocol_role(self.control_type_id, self.is_password)?;
        let bounds = self.bounds?;
        let (display, frame) = monitors.iter().find_map(|monitor| {
            let (_, frame) = normalize_frame(bounds, std::slice::from_ref(&monitor.bounds))?;
            Some((monitor.id, frame))
        })?;
        let mut label = clean(self.name.as_deref()).or_else(|| clean(self.label.as_deref()));
        let mut value = if self.is_password {
            None
        } else {
            clean(self.value.as_deref())
        };
        if role == "StaticText" {
            label = label.or(value.take());
        }
        if matches!(role, "Image" | "Row" | "StaticText" | "Window")
            && label.is_none()
            && value.is_none()
        {
            return None;
        }
        Some(UiNode {
            role: role.to_owned(),
            label,
            value,
            parent,
            enabled: self.enabled,
            focused: self.focused,
            display,
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        })
    }
}

fn clean(text: Option<&str>) -> Option<String> {
    let line = text?.replace("\r\n", " ").replace(
        ['\n', '\r', '\x0b', '\x0c', '\u{85}', '\u{2028}', '\u{2029}'],
        " ",
    );
    let mut line = line.trim().to_owned();
    if line.is_empty() {
        return None;
    }
    // Rust counts Unicode scalar values here; Swift counts grapheme clusters.
    if let Some((index, _)) = line.char_indices().nth(MAX_TEXT) {
        line.truncate(index);
        line.push('…');
    }
    Some(line)
}
