//! Turns an agent's gestures and key names into `pointer` events for the host.

use koko_proto::{MouseButton, PointerEvent};

/// Protocol coordinates run from 0 to 9999 across the display, independent of its scaling.
pub fn to_protocol(pixel: u32, size: u32) -> Option<u16> {
    (pixel < size).then(|| (u64::from(pixel) * 10_000 / u64::from(size)).min(9_999) as u16)
}

#[derive(Clone, Copy, Debug)]
pub enum Gesture {
    Move,
    Click(MouseButton),
    DoubleClick,
    Drag { to: (u16, u16) },
    Scroll { dx: i16, dy: i16 },
}

const DRAG_STEPS: u16 = 12;

/// Every gesture ends with its buttons released, so no state is left on the host between calls.
pub fn mouse_events((x, y): (u16, u16), gesture: Gesture) -> Vec<PointerEvent> {
    let down = |button, clicks| PointerEvent::Down {
        x,
        y,
        button,
        clicks,
    };
    let up = |button| PointerEvent::Up { x, y, button };
    let mut events = vec![PointerEvent::Move { x, y }];
    match gesture {
        Gesture::Move => {}
        Gesture::Click(button) => events.extend([down(button, 1), up(button)]),
        Gesture::DoubleClick => events.extend([
            down(MouseButton::Left, 1),
            up(MouseButton::Left),
            down(MouseButton::Left, 2),
            up(MouseButton::Left),
        ]),
        Gesture::Drag { to } => {
            events.push(down(MouseButton::Left, 1));
            let step = |from: u16, to: u16, index: u16| {
                (i32::from(from)
                    + (i32::from(to) - i32::from(from)) * i32::from(index) / i32::from(DRAG_STEPS))
                    as u16
            };
            events.extend((1..=DRAG_STEPS).map(|index| PointerEvent::Move {
                x: step(x, to.0, index),
                y: step(y, to.1, index),
            }));
            events.push(PointerEvent::Up {
                x: to.0,
                y: to.1,
                button: MouseButton::Left,
            });
        }
        Gesture::Scroll { dx, dy } => events.push(PointerEvent::Scroll { x, y, dx, dy }),
    }
    events
}

// Core Graphics modifier flags shifted right by 16, as the pointer protocol carries them.
const SHIFT: u8 = 0x02;
const CONTROL: u8 = 0x04;
const OPTION: u8 = 0x08;
const COMMAND: u8 = 0x10;
const FUNCTION: u8 = 0x80;

/// Builds key events for combos such as `cmd+shift+t`. Each combo presses and releases its key.
pub fn key_events(combos: &[String]) -> Result<Vec<PointerEvent>, String> {
    if combos.is_empty() || combos.len() > 16 {
        return Err("send 1 to 16 key combos per call".into());
    }
    let mut events = Vec::with_capacity(combos.len() * 2);
    for combo in combos {
        let lower = combo.trim().to_lowercase();
        let mut parts: Vec<&str> = lower.split('+').map(str::trim).collect();
        let key = parts
            .pop()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("`{combo}` has no key; write `+` as `shift+=`"))?;
        let mut flags = 0;
        for modifier in parts {
            flags |= match modifier {
                "cmd" | "command" => COMMAND,
                "ctrl" | "control" => CONTROL,
                "opt" | "option" | "alt" => OPTION,
                "shift" => SHIFT,
                "fn" => FUNCTION,
                other => return Err(format!("unknown modifier `{other}` in `{combo}`")),
            };
        }
        let code = key_code(key).ok_or_else(|| format!("unknown key `{key}` in `{combo}`"))?;
        events.push(PointerEvent::KeyDown {
            code,
            flags,
            repeat: false,
        });
        events.push(PointerEvent::KeyUp { code, flags });
    }
    Ok(events)
}

/// Mac virtual key codes for an ANSI keyboard. Letters follow key positions, not the host's layout.
fn key_code(key: &str) -> Option<u8> {
    Some(match key {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" => 24,
        "9" => 25,
        "7" => 26,
        "-" => 27,
        "8" => 28,
        "0" => 29,
        "]" => 30,
        "o" => 31,
        "u" => 32,
        "[" => 33,
        "i" => 34,
        "p" => 35,
        "enter" | "return" => 36,
        "l" => 37,
        "j" => 38,
        "'" => 39,
        "k" => 40,
        ";" => 41,
        "\\" => 42,
        "," => 43,
        "/" => 44,
        "n" => 45,
        "m" => 46,
        "." => 47,
        "tab" => 48,
        "space" => 49,
        "`" => 50,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f3" => 99,
        "f8" => 100,
        "f9" => 101,
        "f11" => 103,
        "f10" => 109,
        "f12" => 111,
        "home" => 115,
        "page_up" | "pageup" => 116,
        "forward_delete" => 117,
        "f4" => 118,
        "end" => 119,
        "f2" => 120,
        "page_down" | "pagedown" => 121,
        "f1" => 122,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_screenshot_pixels_to_protocol_coordinates() {
        assert_eq!(to_protocol(0, 1280), Some(0));
        assert_eq!(to_protocol(640, 1280), Some(5000));
        assert_eq!(to_protocol(1279, 1280), Some(9992));
        assert_eq!(to_protocol(1280, 1280), None);
    }

    #[test]
    fn gestures_release_their_buttons() {
        let events = mouse_events((100, 200), Gesture::Drag { to: (1300, 200) });
        assert_eq!(events.len(), 2 + DRAG_STEPS as usize + 1);
        assert!(matches!(events[1], PointerEvent::Down { clicks: 1, .. }));
        assert!(matches!(
            events.last(),
            Some(PointerEvent::Up {
                x: 1300,
                y: 200,
                ..
            })
        ));
        assert!(
            events.len() <= 32,
            "a pointer request carries at most 32 events"
        );

        let double = mouse_events((5, 5), Gesture::DoubleClick);
        assert!(matches!(double[3], PointerEvent::Down { clicks: 2, .. }));
    }

    #[test]
    fn parses_key_combos() {
        let events = key_events(&["Cmd+Shift+T".into(), "enter".into()]).unwrap();
        assert!(matches!(
            events[0],
            PointerEvent::KeyDown {
                code: 17,
                flags: 0x12,
                ..
            }
        ));
        assert!(matches!(
            events[3],
            PointerEvent::KeyUp { code: 36, flags: 0 }
        ));
        assert!(key_events(&["hyper+a".into()]).is_err());
        assert!(key_events(&["cmd+".into()]).is_err());
        assert!(key_events(&[]).is_err());
    }
}
