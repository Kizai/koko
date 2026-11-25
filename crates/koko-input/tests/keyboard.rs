use koko_input::keyboard::{
    KeyboardSource, Set1ScanCode, WindowsModifiers, mac_to_windows_scan, windows_to_mac_key,
};

fn scan(make: u8, extended: bool) -> Set1ScanCode {
    Set1ScanCode { make, extended }
}

// Apple Events.h and the Microsoft Scan 1 Make table, linked in windows-plan.md.
#[test]
fn keeps_extended_physical_keys_distinct() {
    for (mac, vk, physical) in [
        (0x24, 0x0d, scan(0x1c, false)), // Return
        (0x4c, 0x0d, scan(0x1c, true)),  // Keypad Enter
        (0x77, 0x23, scan(0x4f, true)),  // End
        (0x53, 0x23, scan(0x4f, false)), // Keypad 1 with Num Lock off
        (0x56, 0x25, scan(0x4b, false)), // Keypad 4 with Num Lock off
        (0x57, 0x0c, scan(0x4c, false)), // Keypad 5 with Num Lock off
        (0x41, 0x2e, scan(0x53, false)), // Keypad decimal with Num Lock off
        (0x2c, 0xbf, scan(0x35, false)), // Slash
        (0x4b, 0x6f, scan(0x35, true)),  // Keypad Divide
        (0x3b, 0x11, scan(0x1d, false)), // Left Ctrl
        (0x3e, 0x11, scan(0x1d, true)),  // Right Ctrl
        (0x3a, 0x12, scan(0x38, false)), // Left Alt
        (0x3d, 0x12, scan(0x38, true)),  // Right Alt
        (0x38, 0x10, scan(0x2a, false)), // Left Shift
        (0x3c, 0x10, scan(0x36, false)), // Right Shift
        (0x47, 0x90, scan(0x45, false)), // Raw Num Lock
    ] {
        assert_eq!(
            mac_to_windows_scan(mac, KeyboardSource::Windows, true),
            Some(physical)
        );
        assert_eq!(windows_to_mac_key(vk, Some(physical)), Some(mac));
    }
    assert_eq!(windows_to_mac_key(0x90, Some(scan(0x45, true))), Some(0x47));
}

#[test]
fn requires_scan_metadata_for_ambiguous_virtual_keys() {
    for vk in [
        0x0d, 0x10, 0x11, 0x12, 0x23, 0x25, 0x2e, 0xba, 0xdc, 0xe2, 0x92,
    ] {
        assert_eq!(windows_to_mac_key(vk, None), None, "VK {vk:#04x}");
    }
    for (vk, mac) in [(0x41, 0x00), (0x61, 0x53), (0x90, 0x47), (0xa3, 0x3e)] {
        assert_eq!(windows_to_mac_key(vk, None), Some(mac));
    }
    assert_eq!(windows_to_mac_key(0x41, Some(scan(0x7f, false))), None);
}

#[test]
fn maps_iso_and_jis_physical_keys_without_using_character_labels() {
    // Apple IOHIDFamily hid_usb_2_adb_keymap and Microsoft Keyboard/Keypad usages.
    for (mac, vk, make) in [
        (0x0a, 0xe2, 0x56), // ISO Section, HID 0x64
        (0x5e, 0xe2, 0x73), // JIS Underscore / Ro, HID 0x87
        (0x5d, 0xdc, 0x7d), // JIS Yen, HID 0x89
        (0x5f, 0x6c, 0x7e), // JIS Keypad Comma, HID 0x85
        (0x2a, 0xdc, 0x2b), // ANSI backslash, HID 0x31
    ] {
        let physical = scan(make, false);
        assert_eq!(
            mac_to_windows_scan(mac, KeyboardSource::MacOs, true),
            Some(physical)
        );
        assert_eq!(windows_to_mac_key(vk, Some(physical)), Some(mac));
    }
}

#[test]
fn drops_unsupported_language_and_fn_keys() {
    for mac in [0x66, 0x68, 0x3f, 0x100] {
        assert_eq!(mac_to_windows_scan(mac, KeyboardSource::MacOs, true), None);
    }
    for (vk, physical) in [
        (0x15, scan(0x72, false)), // VK_KANA / VK_HANGUL, LANG1
        (0x15, scan(0xf2, false)), // Legacy LANG1
        (0x1a, scan(0x71, false)), // VK_IME_OFF, LANG2
        (0x1a, scan(0xf1, false)), // Legacy LANG2
    ] {
        assert_eq!(windows_to_mac_key(vk, Some(physical)), None);
        assert_eq!(windows_to_mac_key(vk, None), None);
    }
}

#[test]
fn distinguishes_windows_system_keys_from_f13_to_f15() {
    for (vk, physical) in [
        (0x2c, scan(0x37, true)),  // PrintScreen
        (0x2c, scan(0x54, false)), // Alt+PrintScreen
        (0x91, scan(0x46, false)), // Scroll Lock
        (0x13, scan(0x45, false)), // Legacy Pause
        (0x03, scan(0x46, true)),  // Ctrl+Pause / Break
    ] {
        assert_eq!(windows_to_mac_key(vk, Some(physical)), None);
        assert_eq!(windows_to_mac_key(vk, None), None);
    }
    for (mac, vk, make) in [(0x69, 0x7c, 0x64), (0x6b, 0x7d, 0x65), (0x71, 0x7e, 0x66)] {
        assert_eq!(
            mac_to_windows_scan(mac, KeyboardSource::Windows, false),
            Some(scan(make, false))
        );
        assert_eq!(windows_to_mac_key(vk, Some(scan(make, false))), Some(mac));
    }
}

#[test]
fn maps_windows_application_key_to_apple_contextual_menu() {
    assert_eq!(windows_to_mac_key(0x5d, None), Some(0x6e));
    assert_eq!(windows_to_mac_key(0x5d, Some(scan(0x5d, true))), Some(0x6e));
    assert_eq!(
        mac_to_windows_scan(0x6e, KeyboardSource::Windows, true),
        Some(scan(0x5d, true))
    );
}

#[test]
fn swaps_control_and_command_key_positions_only_for_macos_source_when_enabled() {
    for source in [
        KeyboardSource::MacOs,
        KeyboardSource::Windows,
        KeyboardSource::Other,
    ] {
        for enabled in [false, true] {
            for (mac, physical, swapped) in [
                (0x37, scan(0x5b, true), scan(0x1d, false)), // Left Command
                (0x36, scan(0x5c, true), scan(0x1d, true)),  // Right Command
                (0x3b, scan(0x1d, false), scan(0x5b, true)), // Left Control
                (0x3e, scan(0x1d, true), scan(0x5c, true)),  // Right Control
                (0x3a, scan(0x38, false), scan(0x38, false)), // Left Option
                (0x3d, scan(0x38, true), scan(0x38, true)),  // Right Option
            ] {
                let expected = if source == KeyboardSource::MacOs && enabled {
                    swapped
                } else {
                    physical
                };
                assert_eq!(mac_to_windows_scan(mac, source, enabled), Some(expected));
            }
        }
    }
}

#[test]
fn converts_packed_core_graphics_flags_in_both_directions() {
    // IOLLEvent.h device-independent flags, packed by shifting right 16.
    for flags in 0u8..=0x1f {
        for source in [
            KeyboardSource::MacOs,
            KeyboardSource::Windows,
            KeyboardSource::Other,
        ] {
            for enabled in [false, true] {
                let swap = source == KeyboardSource::MacOs && enabled;
                let ctrl = flags & if swap { 0x10 } else { 0x04 } != 0;
                let win = flags & if swap { 0x04 } else { 0x10 } != 0;
                let expected = WindowsModifiers {
                    shift: flags & 0x02 != 0,
                    alt: flags & 0x08 != 0,
                    caps_lock: flags & 0x01 != 0,
                    ctrl,
                    win,
                };
                assert_eq!(
                    WindowsModifiers::from_mac_flags(flags, source, enabled),
                    expected
                );
                assert_eq!(
                    WindowsModifiers::from_mac_flags(flags | 0xe0, source, enabled),
                    expected
                );
                assert_eq!(
                    expected.to_mac_flags(),
                    (flags & 0x0b) | (u8::from(ctrl) << 2) | (u8::from(win) << 4),
                );
            }
        }
    }
}

#[test]
fn translates_common_shortcuts_as_physical_keys_and_modifiers() {
    for (vk, mac, make) in [(0x43, 0x08, 0x2e), (0x56, 0x09, 0x2f)] {
        // Windows Ctrl+C/V and Mac Command+C/V both become Ctrl+C/V on Windows.
        assert_eq!(windows_to_mac_key(vk, None), Some(mac));
        for (source, flags) in [
            (KeyboardSource::Windows, 0x04),
            (KeyboardSource::MacOs, 0x10),
        ] {
            assert_eq!(
                mac_to_windows_scan(mac, source, true),
                Some(scan(make, false))
            );
            assert_eq!(
                WindowsModifiers::from_mac_flags(flags, source, true),
                WindowsModifiers {
                    ctrl: true,
                    ..WindowsModifiers::default()
                }
            );
        }
    }
    // Alt+Tab and the macOS Command+Tab chord retain their physical base keys.
    assert_eq!(
        mac_to_windows_scan(0x30, KeyboardSource::Windows, true),
        Some(scan(0x0f, false))
    );
    assert_eq!(
        WindowsModifiers::from_mac_flags(0x08, KeyboardSource::Windows, true),
        WindowsModifiers {
            alt: true,
            ..WindowsModifiers::default()
        }
    );
    assert_eq!(
        mac_to_windows_scan(0x30, KeyboardSource::MacOs, true),
        Some(scan(0x0f, false))
    );
    assert!(WindowsModifiers::from_mac_flags(0x10, KeyboardSource::MacOs, true).ctrl);

    // Win+Shift+S normalizes to Command+Shift+S for transport.
    assert_eq!(
        windows_to_mac_key(0x53, Some(scan(0x1f, false))),
        Some(0x01)
    );
    let screenshot = WindowsModifiers {
        win: true,
        shift: true,
        ..WindowsModifiers::default()
    };
    assert_eq!(screenshot.to_mac_flags(), 0x12);
    assert_eq!(
        WindowsModifiers::from_mac_flags(0x12, KeyboardSource::Windows, true),
        screenshot
    );

    // macOS Command+Shift+3 becomes Ctrl+Shift+3 under the shortcut setting.
    assert_eq!(
        mac_to_windows_scan(0x14, KeyboardSource::MacOs, true),
        Some(scan(0x04, false))
    );
    assert_eq!(
        WindowsModifiers::from_mac_flags(0x12, KeyboardSource::MacOs, true),
        WindowsModifiers {
            ctrl: true,
            shift: true,
            ..WindowsModifiers::default()
        }
    );
}
