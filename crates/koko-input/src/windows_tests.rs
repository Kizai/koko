// Pure protocol-event vectors. The sink records planned input and never calls Windows.
use super::*;
use std::collections::VecDeque;

#[derive(Default)]
struct TestSink {
    foreground: Option<&'static str>,
    attempted: Vec<Vec<Record>>,
    counts: VecDeque<usize>,
}

impl Sink for TestSink {
    fn check_foreground(&self) -> Result<(), String> {
        self.foreground.map_or(Ok(()), |error| Err(error.into()))
    }

    fn send(&mut self, records: &[Record]) -> usize {
        self.attempted.push(records.to_vec());
        self.counts.pop_front().unwrap_or(records.len())
    }
}

fn input(cross_system_shortcuts: bool) -> Input<TestSink> {
    Input::new(TestSink::default(), cross_system_shortcuts)
}

fn geometry() -> Geometry {
    Geometry {
        monitor: PhysicalRect::new(-1920, 0, 1920, 1080).unwrap(),
        desktop: PhysicalRect::new(-1920, 0, 4480, 1440).unwrap(),
    }
}

fn key(code: u8, flags: u8, down: bool) -> PointerEvent {
    if down {
        PointerEvent::KeyDown {
            code,
            flags,
            repeat: false,
        }
    } else {
        PointerEvent::KeyUp { code, flags }
    }
}

fn scan(make: u8, extended: bool, down: bool) -> Record {
    Record::Edge(Control::Scan(Set1ScanCode { make, extended }), down)
}

fn sent(input: &Input<TestSink>) -> Vec<Record> {
    input.sink.attempted.iter().flatten().copied().collect()
}

#[test]
fn maps_negative_origin_and_sends_one_edge_per_click_and_separate_wheels() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Other,
            &[
                PointerEvent::Move { x: 0, y: 0 },
                PointerEvent::Down {
                    x: 9999,
                    y: 9999,
                    button: MouseButton::Left,
                    clicks: 3,
                },
                PointerEvent::Move { x: 0, y: 0 },
                PointerEvent::Up {
                    x: 0,
                    y: 0,
                    button: MouseButton::Left,
                },
                PointerEvent::Scroll {
                    x: 0,
                    y: 0,
                    dx: 1,
                    dy: -12,
                },
            ],
        )
        .unwrap();
    assert_eq!(
        sent(&input),
        [
            Record::Move(0, 0),
            Record::Move(28078, 49140),
            Record::Edge(Control::Button(Button::Left), true),
            Record::Move(0, 0),
            Record::Move(0, 0),
            Record::Edge(Control::Button(Button::Left), false),
            Record::Move(0, 0),
            Record::Wheel {
                horizontal: true,
                delta: -10
            },
            Record::Wheel {
                horizontal: false,
                delta: -120
            },
        ]
    );
}

#[test]
fn keeps_explicit_right_modifier_and_remembers_scan_across_source_change() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::MacOs,
            &[
                key(0x36, 16, true),
                key(0x7c, 16, true),
                PointerEvent::KeyDown {
                    code: 0x7c,
                    flags: 16,
                    repeat: true,
                },
                key(0x7c, 16, false),
            ],
        )
        .unwrap();
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Windows,
            &[key(0x36, 0, false)],
        )
        .unwrap();
    assert_eq!(
        sent(&input),
        [
            scan(0x1d, true, true),
            scan(0x4d, true, true),
            scan(0x4d, true, true),
            scan(0x4d, true, false),
            scan(0x1d, true, false),
        ]
    );
}

#[test]
fn missing_display_still_cleans_up_and_release_needs_no_foreground() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Windows,
            &[key(0x37, 16, true)],
        )
        .unwrap();
    input.sink.foreground = Some("higher integrity");
    assert!(matches!(
        input.pointer(
            || None,
            KeyboardSource::Windows,
            &[PointerEvent::Move { x: 0, y: 0 }, PointerEvent::Release,]
        ),
        Err(InputError::DisplayUnavailable)
    ));
    assert_eq!(
        sent(&input),
        [scan(0x5b, true, true), scan(0x5b, true, false)]
    );
    input
        .pointer(|| None, KeyboardSource::Windows, &[PointerEvent::Release])
        .unwrap();
    assert_eq!(input.sink.attempted.len(), 2);
}

#[test]
fn semantic_short_write_cleans_only_inserted_edges_and_retries_cleanup() {
    let mut input = input(true);
    input.sink.counts.extend([2, 1]);
    assert!(matches!(
        input.key(RemoteKey::Copy),
        Err(InputError::Failed(_))
    ));
    assert_eq!(
        input.sink.attempted,
        [
            vec![
                scan(0x1d, false, true),
                scan(0x2e, false, true),
                scan(0x2e, false, false),
                scan(0x1d, false, false)
            ],
            vec![scan(0x2e, false, false), scan(0x1d, false, false)],
        ]
    );
    input.release().unwrap();
    assert_eq!(input.sink.attempted[2], [scan(0x1d, false, false)]);
}

#[test]
fn keyboard_mapping_off_and_unicode_packets() {
    let mut input = input(false);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::MacOs,
            &[key(0x37, 16, true)],
        )
        .unwrap();
    input.release().unwrap();
    input.text("中🙂").unwrap();
    let mut expected = vec![scan(0x5b, true, true), scan(0x5b, true, false)];
    for unit in [0x4e2d, 0xd83d, 0xde42] {
        expected.extend([
            Record::Edge(Control::Unicode(unit), true),
            Record::Edge(Control::Unicode(unit), false),
        ]);
    }
    assert_eq!(sent(&input), expected);
}

#[test]
fn refuses_new_input_when_foreground_is_higher_or_unknown() {
    for reason in ["higher integrity", "could not query foreground process"] {
        let mut input = input(true);
        input.sink.foreground = Some(reason);
        assert!(
            matches!(input.text("blocked"), Err(InputError::Failed(message)) if message == reason)
        );
        assert!(input.sink.attempted.is_empty());
    }
}

#[test]
fn primary_click_and_all_mouse_buttons_keep_exact_edges() {
    let mut input = input(true);
    input.click(Some(geometry()), (0, 0)).unwrap();
    assert_eq!(
        sent(&input),
        [
            Record::Move(0, 0),
            Record::Edge(Control::Button(Button::Left), true),
            Record::Edge(Control::Button(Button::Left), false),
        ]
    );
    input.sink.attempted.clear();
    for (button, expected) in [
        (MouseButton::Left, Button::Left),
        (MouseButton::Right, Button::Right),
        (MouseButton::Middle, Button::Middle),
        (MouseButton::Back, Button::Back),
        (MouseButton::Forward, Button::Forward),
    ] {
        input
            .pointer(
                || Some(geometry()),
                KeyboardSource::Other,
                &[
                    PointerEvent::Down {
                        x: 0,
                        y: 0,
                        button,
                        clicks: 1,
                    },
                    PointerEvent::Up { x: 0, y: 0, button },
                ],
            )
            .unwrap();
        assert_eq!(
            sent(&input),
            [
                Record::Move(0, 0),
                Record::Edge(Control::Button(expected), true),
                Record::Move(0, 0),
                Record::Edge(Control::Button(expected), false),
            ]
        );
        input.sink.attempted.clear();
    }
    input.release().unwrap();
    assert!(input.sink.attempted.is_empty());
}

#[test]
fn real_modifier_side_takes_over_aggregate_fallback_and_caps_lock_is_a_pulse() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Windows,
            &[
                key(0x00, 4, true),
                key(0x00, 4, false),
                key(0x3e, 4, true),
                key(0x7b, 4, true),
                key(0x7b, 4, false),
                key(0x3e, 0, false),
                key(0x39, 1, true),
                key(0x39, 1, false),
                key(0x00, 1, true),
                key(0x00, 1, false),
                key(0x3f, 0, true),
                key(0x66, 0, true),
                key(0x68, 0, true),
            ],
        )
        .unwrap();
    assert_eq!(
        sent(&input),
        [
            scan(0x1d, false, true),
            scan(0x1e, false, true),
            scan(0x1e, false, false),
            scan(0x1d, false, false),
            scan(0x1d, true, true),
            scan(0x4b, true, true),
            scan(0x4b, true, false),
            scan(0x1d, true, false),
            scan(0x3a, false, true),
            scan(0x3a, false, false),
            scan(0x1e, false, true),
            scan(0x1e, false, false),
        ]
    );
}

#[test]
fn held_control_is_borrowed_for_semantic_key_and_release_skips_display_lookup() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::MacOs,
            &[key(0x36, 16, true)],
        )
        .unwrap();
    input.key(RemoteKey::Paste).unwrap();
    input.sink.foreground = Some("higher integrity");
    input
        .pointer(
            || panic!("release must bypass display lookup"),
            KeyboardSource::Other,
            &[PointerEvent::Release],
        )
        .unwrap();
    assert_eq!(
        sent(&input),
        [
            scan(0x1d, true, true),
            scan(0x2f, false, true),
            scan(0x2f, false, false),
            scan(0x1d, true, false),
        ]
    );
}

#[test]
fn pointer_partial_send_does_not_forget_an_inserted_down_or_release_an_unsent_down() {
    for accepted in 0..2 {
        let mut input = input(true);
        input.sink.counts.push_back(accepted);
        assert!(matches!(
            input.pointer(
                || Some(geometry()),
                KeyboardSource::Other,
                &[PointerEvent::Down {
                    x: 0,
                    y: 0,
                    button: MouseButton::Right,
                    clicks: 2
                },]
            ),
            Err(InputError::Failed(_))
        ));
        assert_eq!(input.sink.attempted.len(), 1);
        assert!(input.held.is_empty());
    }
    let mut input = input(true);
    input.sink.counts.push_back(2);
    assert!(matches!(
        input.click(Some(geometry()), (0, 0)),
        Err(InputError::Failed(_))
    ));
    assert_eq!(
        input.sink.attempted[1],
        [Record::Edge(Control::Button(Button::Left), false)]
    );
    assert!(input.held.is_empty());
}

#[test]
fn repeated_key_down_and_drag_release_once_in_reverse_order() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Windows,
            &[
                key(0x3d, 8, true),
                PointerEvent::KeyDown {
                    code: 0x3d,
                    flags: 8,
                    repeat: true,
                },
                PointerEvent::Down {
                    x: 0,
                    y: 0,
                    button: MouseButton::Middle,
                    clicks: 1,
                },
            ],
        )
        .unwrap();
    input.release().unwrap();
    input.release().unwrap();
    assert_eq!(
        sent(&input),
        [
            scan(0x38, true, true),
            scan(0x38, true, true),
            Record::Move(0, 0),
            Record::Edge(Control::Button(Button::Middle), true),
            Record::Edge(Control::Button(Button::Middle), false),
            scan(0x38, true, false),
        ]
    );
}

#[test]
fn semantic_click_preserves_an_active_pointer_drag() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Other,
            &[PointerEvent::Down {
                x: 0,
                y: 0,
                button: MouseButton::Left,
                clicks: 1,
            }],
        )
        .unwrap();
    assert!(matches!(
        input.click(Some(geometry()), (5000, 5000)),
        Err(InputError::Failed(_))
    ));
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::Other,
            &[
                PointerEvent::Move { x: 9999, y: 9999 },
                PointerEvent::Up {
                    x: 0,
                    y: 0,
                    button: MouseButton::Left,
                },
            ],
        )
        .unwrap();
    assert_eq!(
        sent(&input),
        [
            Record::Move(0, 0),
            Record::Edge(Control::Button(Button::Left), true),
            Record::Move(28078, 49140),
            Record::Move(0, 0),
            Record::Edge(Control::Button(Button::Left), false),
        ]
    );
}

#[test]
fn explicit_up_releases_fallback_only_when_the_aggregate_modifier_clears() {
    for (left, right, flag, make, extended) in [
        (0x38, 0x3c, 2, 0x2a, false),
        (0x3b, 0x3e, 4, 0x1d, false),
        (0x3a, 0x3d, 8, 0x38, false),
        (0x37, 0x36, 16, 0x5b, true),
    ] {
        for (first_up, final_up) in [(left, right), (right, left)] {
            let mut input = input(true);
            input
                .pointer(
                    || Some(geometry()),
                    KeyboardSource::Windows,
                    &[
                        key(0x00, flag, true),
                        key(0x00, flag, false),
                        key(first_up, flag, false),
                    ],
                )
                .unwrap();
            assert_eq!(
                sent(&input),
                [
                    scan(make, extended, true),
                    scan(0x1e, false, true),
                    scan(0x1e, false, false),
                ]
            );
            input
                .pointer(
                    || Some(geometry()),
                    KeyboardSource::Windows,
                    &[key(final_up, 0, false)],
                )
                .unwrap();
            assert_eq!(
                input.sink.attempted.last().unwrap(),
                &[scan(make, extended, false)]
            );
        }
    }
}

#[test]
fn unmatched_up_does_not_release_explicitly_owned_modifier_on_other_side() {
    let mut input = input(true);
    input
        .pointer(
            || Some(geometry()),
            KeyboardSource::MacOs,
            &[key(0x36, 16, true), key(0x37, 0, false)],
        )
        .unwrap();
    assert_eq!(sent(&input), [scan(0x1d, true, true)]);
    input.release().unwrap();
    assert_eq!(
        sent(&input),
        [scan(0x1d, true, true), scan(0x1d, true, false)]
    );
}
