//! Shared protocol types and permission definitions for Koko.

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"koko/1";
pub const PAIR_ALPN: &[u8] = b"koko/pair/1";
pub const REMOTE_ALPN: &[u8] = b"koko/remote/1";
pub const CONTROL_REQUEST: &[u8] = b"request";
pub const MAX_REMOTE_REQUEST: usize = 8192;
pub const MAX_REMOTE_REPLY: usize = 16 * 1024 * 1024;

// The first byte is the Control channel, then a big-endian u32 and its JSON payload.
pub const CONTROL_HELLO: &[u8] = b"\x01\x00\x00\x00\x07\"hello\"";
pub const CONTROL_READY: &[u8] = b"\x01\x00\x00\x00\x07\"ready\"";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RemoteCommand {
    Disconnect,
    Screenshot,
    Displays,
    Capture {
        display: u32,
        edge: u16,
        quality: u8,
    },
    Pointer {
        display: u32,
        events: Vec<PointerEvent>,
    },
    Click {
        x: u16,
        y: u16,
    },
    Key {
        key: RemoteKey,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteDisplay {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointerEvent {
    Move {
        x: u16,
        y: u16,
    },
    Down {
        x: u16,
        y: u16,
        button: MouseButton,
        clicks: u8,
    },
    Up {
        x: u16,
        y: u16,
        button: MouseButton,
    },
    Scroll {
        x: u16,
        y: u16,
        dx: i16,
        dy: i16,
    },
    Release,
}

impl PointerEvent {
    fn valid(&self) -> bool {
        let (x, y) = match *self {
            Self::Move { x, y } | Self::Up { x, y, .. } => (x, y),
            Self::Down { x, y, clicks, .. } if (1..=3).contains(&clicks) => (x, y),
            Self::Scroll { x, y, dx, dy } if dx != 0 || dy != 0 => (x, y),
            Self::Release => return true,
            _ => return false,
        };
        x < 10_000 && y < 10_000
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    Space,
    Up,
    Down,
    Left,
    Right,
    Copy,
    Paste,
    SelectAll,
    Undo,
}

impl RemoteKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Escape => "escape",
            Self::Tab => "tab",
            Self::Backspace => "backspace",
            Self::Space => "space",
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::SelectAll => "select_all",
            Self::Undo => "undo",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RemoteReply {
    Ok,
    Frame {
        png: String,
    },
    Image {
        image: String,
    },
    Displays {
        host_name: String,
        displays: Vec<RemoteDisplay>,
    },
    Error {
        code: String,
    },
}

impl RemoteReply {
    pub fn error(code: &str) -> Self {
        Self::Error { code: code.into() }
    }
}

pub fn valid_remote_command(command: &RemoteCommand) -> bool {
    match command {
        RemoteCommand::Click { x, y } => *x < 10_000 && *y < 10_000,
        RemoteCommand::Capture {
            display,
            edge,
            quality,
        } => *display != 0 && (320..=4096).contains(edge) && (20..=95).contains(quality),
        RemoteCommand::Pointer { display, events } => {
            *display != 0
                && (1..=32).contains(&events.len())
                && events.iter().all(PointerEvent::valid)
        }
        RemoteCommand::Text { text } => {
            !text.is_empty() && text.len() <= 2048 && !text.contains('\0')
        }
        RemoteCommand::Disconnect
        | RemoteCommand::Screenshot
        | RemoteCommand::Displays
        | RemoteCommand::Key { .. } => true,
    }
}
