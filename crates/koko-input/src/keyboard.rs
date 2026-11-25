//! Physical keyboard translation for the Mac virtual key wire format.

mod table;

use table::{KEYS, LEFT_COMMAND, LEFT_CONTROL, RIGHT_COMMAND, RIGHT_CONTROL};

/// Set 1 make byte and whether its prefix is E0. E1 sequences are outside this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Set1ScanCode {
    pub make: u8,
    pub extended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardSource {
    MacOs,
    Windows,
    /// Includes an absent or unrecognized source OS.
    Other,
}

impl KeyboardSource {
    fn swaps_control_and_command(self, cross_system_shortcuts: bool) -> bool {
        self == Self::MacOs && cross_system_shortcuts
    }
}

/// Aggregate Windows modifier state. Handedness comes from individual key scans.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WindowsModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub win: bool,
    /// Lock state, distinct from a Caps Lock key press.
    pub caps_lock: bool,
}

// Core Graphics device-independent masks (IOLLEvent.h), packed by shifting right 16.
const CAPS_LOCK: u8 = 1 << 0;
const SHIFT: u8 = 1 << 1;
const CONTROL: u8 = 1 << 2;
const OPTION: u8 = 1 << 3;
const COMMAND: u8 = 1 << 4;

impl WindowsModifiers {
    /// Decode wire flags for a Windows receiver. NumericPad, Help and Fn have no
    /// corresponding aggregate Windows modifier state.
    pub fn from_mac_flags(flags: u8, source: KeyboardSource, cross_system_shortcuts: bool) -> Self {
        let mut modifiers = Self {
            shift: flags & SHIFT != 0,
            ctrl: flags & CONTROL != 0,
            alt: flags & OPTION != 0,
            win: flags & COMMAND != 0,
            caps_lock: flags & CAPS_LOCK != 0,
        };
        if source.swaps_control_and_command(cross_system_shortcuts) {
            std::mem::swap(&mut modifiers.ctrl, &mut modifiers.win);
        }
        modifiers
    }

    /// Encode a Windows controller's physical modifier state for transport.
    pub fn to_mac_flags(self) -> u8 {
        (u8::from(self.caps_lock) * CAPS_LOCK)
            | (u8::from(self.shift) * SHIFT)
            | (u8::from(self.ctrl) * CONTROL)
            | (u8::from(self.alt) * OPTION)
            | (u8::from(self.win) * COMMAND)
    }
}

/// Decode a wire key for a Windows receiver, applying the same Control/Command
/// exchange as the macOS receiver's `windowsShortcut` for the opposite direction.
pub fn mac_to_windows_scan(
    mac_key: u16,
    source: KeyboardSource,
    cross_system_shortcuts: bool,
) -> Option<Set1ScanCode> {
    let mac_key = if source.swaps_control_and_command(cross_system_shortcuts) {
        match mac_key {
            LEFT_CONTROL => LEFT_COMMAND,
            RIGHT_CONTROL => RIGHT_COMMAND,
            LEFT_COMMAND => LEFT_CONTROL,
            RIGHT_COMMAND => RIGHT_CONTROL,
            _ => mac_key,
        }
    } else {
        mac_key
    };

    let &(_, scan, _) = KEYS.iter().find(|&&(mac, _, _)| mac == mac_key)?;
    Some(Set1ScanCode {
        make: scan as u8,
        extended: scan >> 8 == 0xe0,
    })
}

/// Encode a Windows controller's key as a Mac virtual position. Supply the
/// normalized Set 1 make byte and E0 flag when available. Scan metadata takes
/// precedence over layout-sensitive VKs. Without a scan, ambiguous VKs are
/// rejected and alphanumeric VKs use their US ANSI positions.
pub fn windows_to_mac_key(vk: u16, scan: Option<Set1ScanCode>) -> Option<u16> {
    const VK_NUMLOCK: u16 = 0x90;
    // PrintScreen, Scroll Lock and Pause/Break have no distinct Mac wire key.
    // This also keeps the unprefixed legacy Pause byte from becoming Num Lock.
    if matches!(vk, 0x2c | 0x91 | 0x13 | 0x03) {
        return None;
    }

    if let Some(scan) = scan {
        let make = match (scan.make, scan.extended) {
            // Num Lock uses 45 in raw input and E045 in legacy messages.
            (0x45, _) if vk == VK_NUMLOCK => 0x0045,
            (0x45, _) => return None,
            (make, true) => 0xe000 | u16::from(make),
            (make, false) => u16::from(make),
        };
        return KEYS
            .iter()
            .find(|&&(_, code, _)| code == make)
            .map(|row| row.0);
    }

    // VK_RETURN, generic modifiers, and the Num Lock-off navigation VKs
    // each identify multiple positions. OEM punctuation also varies by layout.
    if matches!(
        vk,
        0x0d | 0x10..=0x12 | 0x21..=0x28 | 0x2d..=0x2e
            | 0x92 | 0xba..=0xc0 | 0xdb..=0xdf | 0xe2
    ) {
        return None;
    }
    KEYS.iter()
        .find(|&&(_, _, key)| key == Some(vk))
        .map(|row| row.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_events_h_key_except_documented_omissions() {
        // Events.h leaves the following positions unassigned in 00–7E.
        let unassigned = [0x34, 0x42, 0x44, 0x46, 0x4d, 0x6c, 0x70, 0x7f];
        // Available Apple constants without a mapping: Fn, volume, Eisu, Kana.
        let unsupported = [0x3f, 0x48, 0x49, 0x4a, 0x66, 0x68];

        for mac in 0..=0x7f {
            let mapped = mac_to_windows_scan(mac, KeyboardSource::Windows, false).is_some();
            assert_eq!(
                mapped,
                !unassigned.contains(&mac) && !unsupported.contains(&mac),
                "Mac key {mac:#04x}"
            );
        }
    }

    #[test]
    fn every_physical_mapping_round_trips() {
        for &(mac, _, vk) in KEYS {
            let scan = mac_to_windows_scan(mac, KeyboardSource::Windows, false).unwrap();
            // Some hardware positions have only a scan and no fixed layout-independent VK.
            let reverse = windows_to_mac_key(vk.unwrap_or(0), Some(scan));
            assert_eq!(reverse, Some(mac), "Mac key {mac:#04x}, scan {scan:?}");
        }
    }
}
