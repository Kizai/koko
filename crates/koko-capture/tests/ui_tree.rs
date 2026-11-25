use koko_capture::ui_tree::{Monitor, UiElement, flatten_ui_tree};
use koko_input::coordinates::PhysicalRect;
use koko_proto::MAX_UI_NODES;

const WINDOW: i32 = 50032;
const BUTTON: i32 = 50000;
const EDIT: i32 = 50004;
const TEXT: i32 = 50020;
const GROUP: i32 = 50026;

fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
    PhysicalRect::new(x, y, width, height).unwrap()
}

fn primary() -> Monitor {
    Monitor {
        id: 798,
        bounds: rect(0, 0, 100, 100),
    }
}

fn element(control_type_id: i32) -> UiElement {
    UiElement {
        bounds: Some(rect(10, 20, 30, 40)),
        ..UiElement::new(control_type_id)
    }
}

fn group(children: Vec<UiElement>) -> UiElement {
    UiElement {
        children,
        ..element(GROUP)
    }
}

#[test]
fn maps_supported_uia_ids_to_existing_protocol_roles() {
    let mappings = [
        (50000, "Button"),
        (50002, "CheckBox"),
        (50003, "ComboBox"),
        (50004, "TextField"),
        (50005, "Link"),
        (50006, "Image"),
        (50007, "Row"),
        (50011, "MenuItem"),
        (50013, "RadioButton"),
        (50015, "Slider"),
        (50016, "Incrementor"),
        (50019, "Tab"),
        (50020, "StaticText"),
        (50024, "Row"),
        (50029, "Row"),
        (50031, "MenuButton"),
        (50032, "Window"),
        (50035, "StaticText"),
    ];
    let root = group(
        mappings
            .into_iter()
            .map(|(id, _)| UiElement {
                name: Some("Named control".into()),
                ..element(id)
            })
            .collect(),
    );
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.role.as_str())
            .collect::<Vec<_>>(),
        mappings.map(|(_, role)| role)
    );
    assert!(!truncated);
}

#[test]
fn emits_depth_first_order_with_nearest_included_parents() {
    let root = UiElement {
        name: Some("Window".into()),
        children: vec![
            group(vec![
                UiElement {
                    name: Some("Unmapped control".into()),
                    children: vec![UiElement {
                        children: vec![element(EDIT)],
                        ..element(BUTTON)
                    }],
                    ..element(-1)
                },
                element(BUTTON),
            ]),
            UiElement {
                children: vec![element(BUTTON)],
                ..element(50010) // MenuBar
            },
            element(EDIT),
        ],
        ..element(WINDOW)
    };
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(
        nodes
            .iter()
            .map(|node| (node.role.as_str(), node.parent))
            .collect::<Vec<_>>(),
        [
            ("Window", None),
            ("Button", Some(0)),
            ("TextField", Some(1)),
            ("Button", Some(0)),
            ("TextField", Some(0)),
        ]
    );
    assert!(!truncated);
}

#[test]
fn omits_missing_and_offscreen_frames_but_keeps_their_children() {
    let root = UiElement {
        bounds: None,
        children: vec![UiElement {
            bounds: Some(rect(300, 100, 30, 30)),
            children: vec![UiElement {
                bounds: Some(rect(-25, 30, 40, 60)),
                enabled: false,
                focused: true,
                ..element(BUTTON)
            }],
            ..element(BUTTON)
        }],
        ..element(BUTTON)
    };
    let secondary = Monitor {
        id: 212,
        bounds: rect(-200, 0, 200, 150),
    };
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary(), secondary], MAX_UI_NODES);
    assert_eq!(nodes.len(), 1);
    let node = &nodes[0];
    assert_eq!(node.display, 212);
    assert_eq!(
        (node.x, node.y, node.width, node.height),
        (8750, 2000, 2000, 4000)
    );
    assert_eq!(node.parent, None);
    assert!(!node.enabled);
    assert!(node.focused);
    assert!(!truncated);
}

#[test]
fn password_values_do_not_leak_as_output_values_or_fallback_labels() {
    let root = group(vec![
        UiElement {
            name: Some("Password".into()),
            value: Some("secret sentinel".into()),
            is_password: true,
            ..element(EDIT)
        },
        UiElement {
            value: Some("secret sentinel".into()),
            is_password: true,
            ..element(TEXT)
        },
        UiElement {
            label: Some("Safe label".into()),
            value: Some("secret sentinel".into()),
            is_password: true,
            ..element(TEXT)
        },
        UiElement {
            value: Some("public".into()),
            ..element(EDIT)
        },
    ]);
    let (nodes, _) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(
        nodes
            .iter()
            .map(|node| (
                node.role.as_str(),
                node.label.as_deref(),
                node.value.as_deref()
            ))
            .collect::<Vec<_>>(),
        [
            ("SecureTextField", Some("Password"), None),
            ("StaticText", Some("Safe label"), None),
            ("TextField", None, Some("public")),
        ]
    );
}

#[test]
fn keeps_useful_controls_and_cleans_text() {
    let long = "é".repeat(200);
    let root = group(vec![
        element(50006), // Image
        element(50029), // DataItem
        UiElement {
            name: Some(" \t\n".into()),
            label: Some("  Account\r\nname  ".into()),
            value: Some(format!("{long}x")),
            ..element(EDIT)
        },
        UiElement {
            value: Some("  value\u{2028}as label  ".into()),
            ..element(TEXT)
        },
        element(BUTTON),
    ]);
    let (nodes, _) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].label.as_deref(), Some("Account name"));
    assert_eq!(nodes[0].value, Some(format!("{long}…")));
    assert_eq!(nodes[1].label.as_deref(), Some("value as label"));
    assert_eq!(nodes[1].value, None);
    assert_eq!(nodes[2].role, "Button");
}

#[test]
fn node_budget_reports_truncation_only_when_an_eligible_node_is_omitted() {
    let root = group(vec![element(BUTTON), element(EDIT), element(WINDOW)]);
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], 1);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].role, "Button");
    assert!(truncated);
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], 2);
    assert_eq!(nodes.len(), 2);
    assert!(!truncated);
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], 0);
    assert!(nodes.is_empty());
    assert!(truncated);
    let (nodes, truncated) = flatten_ui_tree(&element(WINDOW), &[primary()], 0);
    assert!(nodes.is_empty());
    assert!(!truncated);
}

#[test]
fn clamps_requested_node_budget_to_protocol_maximum() {
    let root = group((0..=MAX_UI_NODES).map(|_| element(BUTTON)).collect());
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], u16::MAX);
    assert_eq!(nodes.len(), usize::from(MAX_UI_NODES));
    assert!(truncated);
}

#[test]
fn limits_depth_even_for_skipped_elements() {
    let mut root = element(BUTTON);
    for _ in 0..39 {
        root = group(vec![root]);
    }
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(nodes.len(), 1);
    assert!(!truncated);
    let root = group(vec![root]);
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert!(nodes.is_empty());
    assert!(truncated);
}

#[test]
fn limits_total_visits_even_for_skipped_elements() {
    let mut children = (0..5998).map(|_| element(GROUP)).collect::<Vec<_>>();
    children.push(element(BUTTON));
    let mut root = group(children);
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(nodes.len(), 1);
    assert!(!truncated);
    root.children.push(element(EDIT));
    let (nodes, truncated) = flatten_ui_tree(&root, &[primary()], MAX_UI_NODES);
    assert_eq!(nodes.len(), 1);
    assert!(truncated);
}
