use koko_host_windows::agent_policy::{AgentPolicy, AppIdentity, PolicyHit, PolicyNotice};
use koko_proto::{KeyboardPlatform, MouseButton, PointerEvent, RemoteCommand};

fn press(code: u8, flags: u8) -> PointerEvent {
    PointerEvent::KeyDown {
        code,
        flags,
        repeat: false,
    }
}

fn click(x: u16, y: u16) -> PointerEvent {
    PointerEvent::Down {
        x,
        y,
        button: MouseButton::Left,
        clicks: 1,
    }
}

fn pointer(events: Vec<PointerEvent>) -> RemoteCommand {
    RemoteCommand::Pointer {
        display: 42,
        events,
        source_os: Some(KeyboardPlatform::Windows),
    }
}

fn text() -> RemoteCommand {
    RemoteCommand::Text {
        text: "echo hello".into(),
    }
}

fn app(process_name: &str) -> AppIdentity<'_> {
    AppIdentity {
        process_name: Some(process_name),
        aumid: None,
        display_name: None,
    }
}

fn rule(hit: Option<PolicyHit>) -> Option<String> {
    hit.map(|hit| hit.rule)
}

#[test]
fn native_windows_shortcuts_check_translated_modifiers_on_keydown() {
    let policy = AgentPolicy::default();
    for (code, flags, shortcut) in [
        (0x0f, 0x10, "Win+R"),
        (0x76, 0x08, "Alt+F4"),
        (0x35, 0x06, "Ctrl+Shift+Esc"),
        (0x75, 0x02, "Shift+Delete"),
        (0x07, 0x10, "Win+X"),
    ] {
        let expected = format!("shortcut:{shortcut}");
        for (source_os, enabled, wire_flags) in [
            (KeyboardPlatform::Windows, true, flags),
            (KeyboardPlatform::Windows, false, flags),
            (KeyboardPlatform::Macos, false, flags),
            (
                KeyboardPlatform::Macos,
                true,
                (flags & !0x14) | ((flags & 0x10) >> 2) | ((flags & 0x04) << 2),
            ),
        ] {
            let action = RemoteCommand::Pointer {
                display: 42,
                events: vec![press(code, wire_flags | 0xa1)],
                source_os: Some(source_os),
            };
            let hit = policy
                .check(&action, None, enabled, |_, _, _| panic!())
                .unwrap();
            assert_eq!(hit.rule, expected);
            assert!(hit.summary.contains(shortcut));
        }
        let action = pointer(vec![press(code, flags ^ 0x08)]);
        assert!(
            policy
                .check(&action, None, false, |_, _, _| panic!())
                .is_none()
        );
        let action = pointer(vec![PointerEvent::KeyUp { code, flags }]);
        assert!(
            policy
                .check(&action, None, false, |_, _, _| panic!())
                .is_none()
        );
    }
    let action = RemoteCommand::Pointer {
        display: 42,
        events: vec![press(0x0f, 0x10)],
        source_os: Some(KeyboardPlatform::Macos),
    };
    assert!(
        policy
            .check(&action, None, true, |_, _, _| panic!())
            .is_none()
    );
}

#[test]
fn terminal_text_and_both_enter_keys_require_approval() {
    let policy = AgentPolicy::default();
    for name in [
        "WindowsTerminal.exe",
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
    ] {
        let upper = name.to_uppercase();
        let app = AppIdentity {
            display_name: Some("Shell"),
            ..app(&upper)
        };
        let hit = policy
            .check(&text(), Some(app), false, |_, _, _| panic!())
            .unwrap();
        assert_eq!(hit.rule, format!("command:{name}"));
        assert!(hit.summary.contains("Shell"));
        assert!(hit.summary.contains("echo hello"));
        for code in [0x24, 0x4c] {
            assert_eq!(
                rule(policy.check(
                    &pointer(vec![press(code, 0)]),
                    Some(app),
                    true,
                    |_, _, _| panic!()
                )),
                Some(hit.rule.clone())
            );
        }
        for event in [
            press(0x00, 0),
            PointerEvent::KeyUp {
                code: 0x24,
                flags: 0,
            },
        ] {
            assert!(
                policy
                    .check(&pointer(vec![event]), Some(app), false, |_, _, _| panic!())
                    .is_none()
            );
        }
    }
    for name in ["cmd", r"C:\Windows\cmd.exe", "com.apple.Terminal"] {
        assert!(
            policy
                .check(&text(), Some(app(name)), false, |_, _, _| panic!())
                .is_none()
        );
    }
    assert!(
        policy
            .check(&text(), None, false, |_, _, _| panic!())
            .is_none()
    );
}

#[test]
fn protected_app_click_text_and_key_press_are_detected_but_navigation_is_ignored() {
    let policy = AgentPolicy::default();
    for name in [
        "regedit.exe",
        "mmc.exe",
        "Taskmgr.exe",
        "SystemSettings.exe",
    ] {
        let app = Some(app(name));
        for action in [
            text(),
            pointer(vec![click(5, 6)]),
            pointer(vec![press(0x38, 0x02)]),
        ] {
            assert_eq!(
                rule(policy.check(&action, app, false, |_, _, _| panic!())),
                Some(format!("app:{name}"))
            );
        }
        let navigation = pointer(vec![
            PointerEvent::Move { x: 5, y: 6 },
            PointerEvent::Up {
                x: 5,
                y: 6,
                button: MouseButton::Left,
            },
            PointerEvent::Scroll {
                x: 5,
                y: 6,
                dx: 0,
                dy: 25,
            },
            PointerEvent::KeyUp {
                code: 0x38,
                flags: 0,
            },
            PointerEvent::Release,
        ]);
        assert!(
            policy
                .check(&navigation, app, false, |_, _, _| panic!())
                .is_none()
        );
    }
}

#[test]
fn app_user_model_ids_match_exactly_in_both_app_lists() {
    let policy = AgentPolicy::parse(
        br#"{
        "command_apps":["Package_abc!Shell"],
        "protected_apps":["Package_abc!Settings"]
    }"#,
    )
    .policy;
    for (id, expected) in [
        (
            "Package_abc!Shell",
            Some("command:Package_abc!Shell".into()),
        ),
        (
            "Package_abc!Settings",
            Some("app:Package_abc!Settings".into()),
        ),
        ("package_abc!Settings", None),
        ("Package_abc!shell", None),
    ] {
        let app = AppIdentity {
            aumid: Some(id),
            ..app("surrogate.exe")
        };
        assert_eq!(
            rule(policy.check(&text(), Some(app), false, |_, _, _| panic!())),
            expected
        );
    }
}

#[test]
fn only_clicked_uia_names_are_used_and_each_lookup_keeps_its_display_and_position() {
    let policy = AgentPolicy::default();
    let action = pointer(vec![
        PointerEvent::Move { x: 1, y: 1 },
        click(12, 34),
        PointerEvent::Up {
            x: 12,
            y: 34,
            button: MouseButton::Left,
        },
        click(56, 78),
        click(90, 12),
    ]);
    let mut asked = Vec::new();
    let hit = policy
        .check(&action, None, false, |display, x, y| {
            asked.push((display, x, y));
            match (x, y) {
                (12, 34) => vec!["Save draft".into()],
                (56, 78) => vec!["Confirm DeLeTe account".into(), "付款".into()],
                _ => panic!("unexpected hit test"),
            }
        })
        .unwrap();
    assert_eq!(asked, [(42, 12, 34), (42, 56, 78)]);
    assert_eq!(hit.rule, "click:delete");
    assert_eq!(hit.summary, "点击「Confirm DeLeTe account」");
    assert_eq!(
        rule(
            policy.check(&pointer(vec![click(0, 0)]), None, false, |_, _, _| vec![
                "永久删除".into()
            ])
        ),
        Some("click:删除".into())
    );
    assert!(
        policy
            .check(&pointer(vec![click(0, 0)]), None, false, |_, _, _| vec![
                "Save".into()
            ])
            .is_none()
    );
}

#[test]
fn first_rule_class_wins_over_later_classes_in_the_entire_batch() {
    let policy = AgentPolicy::parse(
        br#"{
        "command_apps":["shell.exe","both.exe"],
        "protected_apps":["both.exe"]
    }"#,
    )
    .policy;
    for (name, events, expected) in [
        (
            "both.exe",
            vec![click(0, 0), press(0x0f, 0x10)],
            "shortcut:Win+R",
        ),
        (
            "both.exe",
            vec![click(0, 0), press(0x24, 0)],
            "app:both.exe",
        ),
        (
            "shell.exe",
            vec![click(0, 0), press(0x24, 0)],
            "command:shell.exe",
        ),
    ] {
        assert_eq!(
            rule(policy.check(&pointer(events), Some(app(name)), false, |_, _, _| panic!())),
            Some(expected.into())
        );
    }
}

#[test]
fn absent_null_and_unknown_config_keys_preserve_defaults() {
    for config in [
        "{}",
        r#"{"shortcuts":null,"command_apps":null,"protected_apps":null,"keywords":null}"#,
        r#"{"unknown":{"custom":true}}"#,
    ] {
        let loaded = AgentPolicy::parse(config.as_bytes());
        assert_eq!(loaded.notice, None);
        assert_eq!(loaded.policy, AgentPolicy::default());
    }
    let loaded = AgentPolicy::parse(br#"{"command_apps":[" custom.exe "],"keywords":[""]}"#);
    assert_eq!(loaded.notice, None);
    let default = AgentPolicy::default();
    assert_eq!(loaded.policy.command_apps, [" custom.exe "]);
    assert_eq!(loaded.policy.keywords, [""]);
    assert_eq!(loaded.policy.protected_apps, default.protected_apps);
    assert_eq!(loaded.policy.shortcuts, default.shortcuts);
}

#[test]
fn duplicate_known_keys_use_the_first_value_even_when_null() {
    for (config, expected) in [
        (r#"{"shortcuts":false,"shortcuts":true}"#, false),
        (r#"{"shortcuts":true,"shortcuts":false}"#, true),
        (r#"{"shortcuts":null,"shortcuts":false}"#, true),
        (r#"{"shortcuts":false,"shortcuts":"wrong"}"#, false),
    ] {
        let loaded = AgentPolicy::parse(config.as_bytes());
        assert_eq!(loaded.notice, None, "{config}");
        assert_eq!(loaded.policy.shortcuts, expected, "{config}");
    }
    let loaded = AgentPolicy::parse(
        br#"{"command_apps":[],"command_apps":["cmd.exe"],"protected_apps":null,"protected_apps":[],"keywords":["custom"],"keywords":42}"#,
    );
    assert_eq!(loaded.notice, None);
    assert!(loaded.policy.command_apps.is_empty());
    assert_eq!(
        loaded.policy.protected_apps,
        AgentPolicy::default().protected_apps
    );
    assert_eq!(loaded.policy.keywords, ["custom"]);
}

#[test]
fn accepts_a_utf8_bom_at_the_start_of_the_file() {
    let config = "\u{feff}{\"shortcuts\":false}";
    let loaded = AgentPolicy::parse(config.as_bytes());
    assert_eq!(loaded.notice, None);
    assert!(!loaded.policy.shortcuts);
}

#[test]
fn empty_keyword_matches_no_control_names() {
    let loaded = AgentPolicy::parse(br#"{"keywords":[""]}"#);
    let action = pointer(vec![click(0, 0)]);
    assert!(
        loaded
            .policy
            .check(&action, None, false, |_, _, _| vec!["Save".into()])
            .is_none()
    );
}

#[test]
fn empty_lists_and_false_disable_the_associated_rules() {
    let loaded = AgentPolicy::parse(
        br#"{"shortcuts":false,"command_apps":[],"protected_apps":[],"keywords":[]}"#,
    );
    assert_eq!(loaded.notice, None);
    for (action, app) in [
        (text(), app("cmd.exe")),
        (text(), app("regedit.exe")),
        (
            pointer(vec![click(0, 0), press(0x0f, 0x10)]),
            app("notepad.exe"),
        ),
    ] {
        assert!(
            loaded
                .policy
                .check(&action, Some(app), false, |_, _, _| panic!())
                .is_none()
        );
    }
}

#[test]
fn malformed_or_wrong_typed_json_falls_back_as_a_whole() {
    for config in [
        r#"{"shortcuts":false,"command_apps":["private value",42]}"#,
        r#"{"shortcuts":"false","keywords":[]}"#,
        r#"{"shortcuts":"wrong","shortcuts":false}"#,
        r#"{"protected_apps":{},"command_apps":[]}"#,
        r#"{"shortcuts":false,"keywords":[null]}"#,
        r#"{"shortcuts":false"#,
        "null",
        "[false,[],[],[]]",
        "42",
        r#""{}""#,
    ] {
        let loaded = AgentPolicy::parse(config.as_bytes());
        assert_eq!(loaded.notice, Some(PolicyNotice::InvalidFormat), "{config}");
        assert_eq!(loaded.policy, AgentPolicy::default(), "{config}");
    }
}

#[test]
fn file_loader_distinguishes_missing_invalid_and_unreadable_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-policy.json");
    let missing = AgentPolicy::load(&path);
    assert_eq!(missing.notice, None);
    assert_eq!(missing.policy, AgentPolicy::default());

    std::fs::write(&path, br#"{"shortcuts":false}"#).unwrap();
    let loaded = AgentPolicy::load(&path);
    assert_eq!(loaded.notice, None);
    assert!(!loaded.policy.shortcuts);

    std::fs::write(&path, [0xff]).unwrap();
    let invalid = AgentPolicy::load(&path);
    assert_eq!(invalid.notice, Some(PolicyNotice::InvalidFormat));
    assert_eq!(invalid.policy, AgentPolicy::default());

    let unreadable = AgentPolicy::load(directory.path());
    assert_eq!(unreadable.notice, Some(PolicyNotice::Unreadable));
    assert_eq!(unreadable.policy, AgentPolicy::default());
}
