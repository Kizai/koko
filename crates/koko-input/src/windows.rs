//! Windows input planning and tracking. Only the accepted prefix of each
//! SendInput batch can change the set of controls owned by this host.

#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::WindowsInput;

use crate::{
    coordinates::PhysicalRect,
    keyboard::{KeyboardSource, Set1ScanCode, WindowsModifiers, mac_to_windows_scan},
    scroll::pixels_to_wheel,
};
use koko_proto::{MouseButton, PointerEvent, RemoteKey};

#[derive(Clone, Copy)]
pub struct Geometry {
    pub monitor: PhysicalRect,
    pub desktop: PhysicalRect,
}

#[derive(Debug)]
pub enum InputError {
    DisplayUnavailable,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Button(Button),
    Scan(Set1ScanCode),
    Unicode(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl From<MouseButton> for Button {
    fn from(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => Self::Left,
            MouseButton::Right => Self::Right,
            MouseButton::Middle => Self::Middle,
            MouseButton::Back => Self::Back,
            MouseButton::Forward => Self::Forward,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    Move(u16, u16),
    Edge(Control, bool),
    Wheel { horizontal: bool, delta: i32 },
}

#[derive(Clone, Copy)]
struct ForegroundIntegrity {
    host: u32,
    target: u32,
    process_id: u32,
}

trait Sink {
    /// None means there is no confirmed foreground integrity comparison.
    fn foreground_integrity(&self) -> Option<ForegroundIntegrity>;

    fn check_foreground(&self) -> Result<(), String> {
        if let Some(foreground) = self.foreground_integrity()
            && foreground.target > foreground.host
        {
            return Err(format!(
                "Windows input blocked: foreground process {} has higher integrity ({:#x} > {:#x})",
                foreground.process_id, foreground.target, foreground.host,
            ));
        }
        Ok(())
    }

    fn send(&mut self, records: &[Record]) -> usize;
}

struct Held {
    control: Control,
    source_key: Option<u8>,
}

struct Input<S: Sink> {
    sink: S,
    held: Vec<Held>,
    cleanup_pending: bool,
    cross_system_shortcuts: bool,
}

impl<S: Sink> Input<S> {
    fn new(sink: S, cross_system_shortcuts: bool) -> Self {
        Self {
            sink,
            held: Vec::new(),
            cleanup_pending: false,
            cross_system_shortcuts,
        }
    }

    fn pointer(
        &mut self,
        geometry: impl FnOnce() -> Option<Geometry>,
        source: KeyboardSource,
        events: &[PointerEvent],
    ) -> Result<(), InputError> {
        let mut checked = false;
        let mut lookup = Some(geometry);
        let mut geometry = None;
        for event in events {
            if matches!(event, PointerEvent::Release) {
                self.release()?;
                checked = false;
                continue;
            }
            if let Some(lookup) = lookup.take() {
                geometry = lookup();
            }
            let Some(geometry) = geometry else {
                return Err(self.abort(InputError::DisplayUnavailable));
            };
            if !checked {
                self.ready()?;
                checked = true;
            }
            match *event {
                PointerEvent::Move { x, y } => self.mouse(geometry, (x, y), &[])?,
                PointerEvent::Down { x, y, button, .. } => {
                    // The sender already emits each real edge of a double/triple click.
                    self.mouse(
                        geometry,
                        (x, y),
                        &[Record::Edge(Control::Button(button.into()), true)],
                    )?;
                }
                PointerEvent::Up { x, y, button } => {
                    let control = Control::Button(button.into());
                    let up = Record::Edge(control, false);
                    let edges = if self.owns(control) {
                        std::slice::from_ref(&up)
                    } else {
                        &[]
                    };
                    self.mouse(geometry, (x, y), edges)?;
                }
                PointerEvent::Scroll { x, y, dx, dy } => {
                    let (horizontal, vertical) = pixels_to_wheel((dx, dy));
                    let wheels: Vec<_> = [(true, horizontal), (false, vertical)]
                        .into_iter()
                        .filter(|(_, delta)| *delta != 0)
                        .map(|(horizontal, delta)| Record::Wheel { horizontal, delta })
                        .collect();
                    self.mouse(geometry, (x, y), &wheels)?;
                }
                PointerEvent::KeyDown {
                    code,
                    flags,
                    repeat,
                } => {
                    self.pointer_key(source, code, flags, true, repeat)?;
                }
                PointerEvent::KeyUp { code, flags } => {
                    self.pointer_key(source, code, flags, false, false)?;
                }
                PointerEvent::Release => unreachable!(),
            }
        }
        Ok(())
    }

    fn click(&mut self, geometry: Option<Geometry>, point: (u16, u16)) -> Result<(), InputError> {
        let button = Control::Button(Button::Left);
        if !self.cleanup_pending && self.owns(button) {
            return Err(InputError::Failed(
                "Windows click blocked: the remote left button is already held".into(),
            ));
        }
        let Some(geometry) = geometry else {
            return Err(self.abort(InputError::DisplayUnavailable));
        };
        self.ready()?;
        self.mouse(
            geometry,
            point,
            &[Record::Edge(button, true), Record::Edge(button, false)],
        )
    }

    fn mouse(
        &mut self,
        geometry: Geometry,
        point: (u16, u16),
        following: &[Record],
    ) -> Result<(), InputError> {
        let Some((x, y)) = geometry
            .monitor
            .protocol_to_pixel(point)
            .and_then(|pixel| geometry.desktop.pixel_to_absolute(pixel))
        else {
            return Err(self.abort(InputError::Failed("Invalid pointer coordinates".into())));
        };
        let mut records = Vec::with_capacity(following.len() + 1);
        records.push(Record::Move(x, y));
        records.extend_from_slice(following);
        self.submit(&records)
    }

    fn pointer_key(
        &mut self,
        source: KeyboardSource,
        code: u8,
        flags: u8,
        down: bool,
        repeat: bool,
    ) -> Result<(), InputError> {
        let key = self.held.iter().find(|key| key.source_key == Some(code));
        let already_down = key.is_some();
        // A release (or repeat) must target the exact scan originally posted.
        let control = if let Some(key) = key {
            key.control
        } else if let Some(scan) =
            mac_to_windows_scan(u16::from(code), source, self.cross_system_shortcuts)
        {
            Control::Scan(scan)
        } else {
            return Ok(());
        };

        let mut records = Vec::new();
        let flags = WindowsModifiers::from_mac_flags(flags, source, self.cross_system_shortcuts);
        if let Some(group) = Modifier::of(control) {
            if down || !group.is_set(flags) {
                // A physical down replaces a fallback on the other side. An
                // up with clear aggregate flags releases any remaining fallback.
                records.extend(
                    self.held
                        .iter()
                        .filter(|key| {
                            key.source_key.is_none()
                                && (!down || key.control != control)
                                && Modifier::of(key.control) == Some(group)
                        })
                        .map(|key| Record::Edge(key.control, false)),
                );
            }
        } else {
            for (group, left_code) in [
                (Modifier::Shift, 0x38),
                (Modifier::Ctrl, 0x3b),
                (Modifier::Alt, 0x3a),
                (Modifier::Win, 0x37),
            ] {
                let mut keys = self
                    .held
                    .iter()
                    .filter(|key| Modifier::of(key.control) == Some(group));
                if group.is_set(flags) {
                    if keys.next().is_none() {
                        records.push(Record::Edge(native_scan(left_code), true));
                    }
                } else {
                    records.extend(
                        keys.filter(|key| key.source_key.is_none())
                            .map(|key| Record::Edge(key.control, false)),
                    );
                }
            }
        }

        if (down && (repeat || !self.owns(control))) || (!down && already_down) {
            records.push(Record::Edge(control, down));
        }
        self.submit(&records)?;
        if down
            && let Some(key) = self.held.iter_mut().find(|key| key.control == control)
            && key.source_key.is_none()
        {
            key.source_key = Some(code);
        }
        Ok(())
    }

    fn key(&mut self, key: RemoteKey) -> Result<(), InputError> {
        self.ready()?;
        let (code, shortcut) = match key {
            RemoteKey::Enter => (0x24, false),
            RemoteKey::Escape => (0x35, false),
            RemoteKey::Tab => (0x30, false),
            RemoteKey::Backspace => (0x33, false),
            RemoteKey::Space => (0x31, false),
            RemoteKey::Up => (0x7e, false),
            RemoteKey::Down => (0x7d, false),
            RemoteKey::Left => (0x7b, false),
            RemoteKey::Right => (0x7c, false),
            RemoteKey::Copy => (0x08, true),
            RemoteKey::Paste => (0x09, true),
            RemoteKey::SelectAll => (0x00, true),
            RemoteKey::Undo => (0x06, true),
        };
        let ctrl = native_scan(0x3b);
        let temporary_ctrl = shortcut
            && !self
                .held
                .iter()
                .any(|key| Modifier::of(key.control) == Some(Modifier::Ctrl));
        let control = native_scan(code);
        let mut records = Vec::with_capacity(4);
        if temporary_ctrl {
            records.push(Record::Edge(ctrl, true));
        }
        records.push(Record::Edge(control, true));
        if !self.owns(control) {
            records.push(Record::Edge(control, false));
        }
        if temporary_ctrl {
            records.push(Record::Edge(ctrl, false));
        }
        self.submit(&records)
    }

    fn text(&mut self, text: &str) -> Result<(), InputError> {
        self.ready()?;
        let mut records = Vec::new();
        for unit in text.encode_utf16() {
            let control = Control::Unicode(unit);
            records.extend([Record::Edge(control, true), Record::Edge(control, false)]);
        }
        self.submit(&records)
    }

    fn ready(&mut self) -> Result<(), InputError> {
        if self.cleanup_pending {
            self.release()?;
        }
        self.sink
            .check_foreground()
            .map_err(|error| self.abort(InputError::Failed(error)))
    }

    fn submit(&mut self, records: &[Record]) -> Result<(), InputError> {
        let accepted = self.send(records);
        if accepted == records.len() {
            Ok(())
        } else {
            Err(self.abort(InputError::Failed(format!(
                "SendInput inserted {accepted} of {} events",
                records.len()
            ))))
        }
    }

    fn release(&mut self) -> Result<(), InputError> {
        let records: Vec<_> = self
            .held
            .iter()
            .rev()
            .map(|key| Record::Edge(key.control, false))
            .collect();
        let accepted = self.send(&records);
        self.cleanup_pending = accepted != records.len();
        if self.cleanup_pending {
            Err(InputError::Failed(format!(
                "SendInput cleanup released {accepted} of {} held controls",
                records.len()
            )))
        } else {
            Ok(())
        }
    }

    fn abort(&mut self, error: InputError) -> InputError {
        if let Err(InputError::Failed(message)) = self.release() {
            eprintln!("{message}");
        }
        error
    }

    fn owns(&self, control: Control) -> bool {
        self.held.iter().any(|key| key.control == control)
    }

    fn send(&mut self, records: &[Record]) -> usize {
        if records.is_empty() {
            return 0;
        }
        let accepted = self.sink.send(records);
        for record in &records[..accepted] {
            if let Record::Edge(control, down) = *record {
                if down {
                    if !self.owns(control) {
                        self.held.push(Held {
                            control,
                            source_key: None,
                        });
                    }
                } else {
                    self.held.retain(|key| key.control != control);
                }
            }
        }
        accepted
    }
}

impl<S: Sink> Drop for Input<S> {
    fn drop(&mut self) {
        if let Err(InputError::Failed(message)) = self.release() {
            eprintln!("{message}");
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Win,
}

impl Modifier {
    fn is_set(self, flags: WindowsModifiers) -> bool {
        match self {
            Self::Shift => flags.shift,
            Self::Ctrl => flags.ctrl,
            Self::Alt => flags.alt,
            Self::Win => flags.win,
        }
    }

    fn of(control: Control) -> Option<Self> {
        let Control::Scan(scan) = control else {
            return None;
        };
        match (scan.make, scan.extended) {
            (0x2a | 0x36, false) => Some(Self::Shift),
            (0x1d, _) => Some(Self::Ctrl),
            (0x38, _) => Some(Self::Alt),
            (0x5b | 0x5c, true) => Some(Self::Win),
            _ => None,
        }
    }
}

fn native_scan(code: u16) -> Control {
    Control::Scan(
        mac_to_windows_scan(code, KeyboardSource::Windows, false)
            .expect("known Mac physical position"),
    )
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
