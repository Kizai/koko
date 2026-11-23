//! Shared protocol types, request validation and size limits for Koko.

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"koko/1";
pub const PAIR_ALPN: &[u8] = b"koko/pair/1";
pub const REMOTE_ALPN: &[u8] = b"koko/remote/1";
pub const CONTROL_REQUEST: &[u8] = b"request";
pub const MAX_PAIR_REQUEST: usize = 1024;
pub const MAX_PAIR_REPLY: usize = 256;
pub const PAIR_VERSION: u8 = 2;
pub const MAX_PEER_NAME: usize = 64;
pub const MAX_REMOTE_REQUEST: usize = 128 * 1024;
pub const MAX_REMOTE_REPLY: usize = 16 * 1024 * 1024;
pub const MAX_VIDEO_PACKET: usize = 8 * 1024 * 1024;
pub const VIDEO_ERROR: u8 = 3;
pub const VIDEO_PATH: u8 = 4;

// The first byte is the Control channel, then a big-endian u32 and its JSON payload.
pub const CONTROL_HELLO: &[u8] = b"\x01\x00\x00\x00\x07\"hello\"";
pub const CONTROL_READY: &[u8] = b"\x01\x00\x00\x00\x07\"ready\"";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerKind {
    Human,
    Agent,
}

/// Ordered from least to most capable, so an agent's request can be compared with what it was granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Observer,
    AgentSafe,
    Human,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::AgentSafe => "agent_safe",
            Self::Human => "human",
        }
    }
}

impl std::str::FromStr for Scope {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "observer" => Ok(Self::Observer),
            "agent_safe" => Ok(Self::AgentSafe),
            "human" => Ok(Self::Human),
            _ => Err("unknown scope"),
        }
    }
}

/// A pairing request from a client that declares who it is. Older clients send only a code or `request`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PairHello {
    pub v: u8,
    pub kind: PeerKind,
    pub name: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl PairHello {
    pub fn valid(&self) -> bool {
        self.v == PAIR_VERSION
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= MAX_PEER_NAME
            && !self.name.chars().any(char::is_control)
            && match self.kind {
                PeerKind::Human => self.scope == Scope::Human,
                PeerKind::Agent => self.scope != Scope::Human,
            }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PairGranted {
    pub v: u8,
    pub scope: Scope,
}

/// Who may send input to the host right now, as decided by the host's arbiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    /// No agent holds control. An agent's next input takes it.
    Idle,
    /// An agent holds control.
    Active,
    /// A person took over or pressed stop. Only a person can let the agent continue.
    Paused,
    /// The agent asked a person for help and waits for the answer.
    AwaitingHuman,
    /// A risky input waits for the person at the host to allow or decline it.
    AwaitingApproval,
}

/// How long a risky agent input waits for the host's decision before it counts as declined.
pub const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How a person answered an agent's request for help.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanResult {
    /// The person did what was needed. The agent continues.
    Resolved,
    /// The person said no. The agent is paused.
    Declined,
    /// The person took over. The agent is paused.
    TakenOver,
    /// Nobody answered in time. The agent continues.
    Timeout,
}

impl std::str::FromStr for HumanResult {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "resolved" => Ok(Self::Resolved),
            "declined" => Ok(Self::Declined),
            "taken_over" => Ok(Self::TakenOver),
            _ => Err("unknown result"),
        }
    }
}

pub const MAX_HELP_REASON: usize = 500;
pub const MAX_UI_NODES: u16 = 1000;

/// One element of the frontmost app's accessibility tree, flattened in depth-first order.
/// The frame is normalized to 0-9999 on the display that holds the element's center.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Omitted for password fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Index of the nearest included ancestor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    pub display: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

fn enabled() -> bool {
    true
}

fn is_enabled(enabled: &bool) -> bool {
    *enabled
}
pub const HELP_TIMEOUT_SECONDS: std::ops::RangeInclusive<u16> = 10..=600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Jpeg,
    Png,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RemoteCommand {
    Clipboard {
        transfer: String,
        op: ClipboardOp,
        packet: Option<ClipboardPacket>,
    },
    Disconnect,
    Hello,
    ReleaseControl,
    RequestHuman {
        reason: String,
        timeout_s: u16,
    },
    Screenshot,
    Displays,
    UiTree {
        max_nodes: u16,
    },
    Capture {
        display: u32,
        edge: u16,
        quality: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<ImageFormat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<bool>,
    },
    Video {
        display: u32,
        edge: u16,
        quality: u8,
        fps: u16,
    },
    Pointer {
        display: u32,
        events: Vec<PointerEvent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_os: Option<KeyboardPlatform>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardOp {
    Copy,
    Begin,
    Next,
    Write,
    Cancel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipboardPacket {
    Text { bytes: String },
    File { name: String, size: u64 },
    Data { bytes: String },
    End,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardPlatform {
    Macos,
    Windows,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    KeyDown {
        code: u8,
        flags: u8,
        repeat: bool,
    },
    KeyUp {
        code: u8,
        flags: u8,
    },
    Release,
}

impl PointerEvent {
    fn valid(&self) -> bool {
        let (x, y) = match *self {
            Self::Move { x, y } | Self::Up { x, y, .. } => (x, y),
            Self::Down { x, y, clicks, .. } if (1..=3).contains(&clicks) => (x, y),
            Self::Scroll { x, y, dx, dy } if dx != 0 || dy != 0 => (x, y),
            Self::KeyDown { code, flags, .. } | Self::KeyUp { code, flags } => {
                return code <= 127 && flags & !0xbf == 0;
            }
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
    Clipboard {
        packet: ClipboardPacket,
    },
    Ok,
    /// Sent by the host app to the daemon only: this agent input needs the person's approval.
    ApprovalRequired {
        reason: String,
    },
    Human {
        result: HumanResult,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Hello {
        kind: PeerKind,
        scope: Scope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<ControlState>,
    },
    Frame {
        png: String,
    },
    Image {
        image: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<ImageFormat>,
    },
    Displays {
        host_name: String,
        displays: Vec<RemoteDisplay>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        video: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clipboard: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<ControlState>,
    },
    UiTree {
        app: String,
        nodes: Vec<UiNode>,
        /// More elements matched than `max_nodes` allowed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
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

impl RemoteCommand {
    pub fn desktop_timeout(&self) -> std::time::Duration {
        // Clipboard file access may wait for a person to answer a macOS privacy prompt.
        std::time::Duration::from_secs(match self {
            Self::Clipboard { .. } => 120,
            // The host holds the request open until a person answers.
            Self::RequestHuman { timeout_s, .. } => u64::from(*timeout_s) + 5,
            _ => 9,
        })
    }

    pub fn allowed_for(&self, scope: Scope) -> bool {
        match scope {
            Scope::Human => true,
            Scope::AgentSafe => {
                self.is_viewing()
                    || self.is_agent_input()
                    || matches!(self, Self::ReleaseControl | Self::RequestHuman { .. })
            }
            Scope::Observer => self.is_viewing(),
        }
    }

    fn is_viewing(&self) -> bool {
        matches!(
            self,
            Self::Hello
                | Self::Displays
                | Self::UiTree { .. }
                | Self::Screenshot
                | Self::Capture { .. }
                | Self::Video { .. }
                | Self::Disconnect
        )
    }

    /// Input an agent may send while it holds control. The older `click` and `key` commands are
    /// left to people because they bypass the host app's checks when no app is running.
    pub fn is_agent_input(&self) -> bool {
        matches!(self, Self::Pointer { .. } | Self::Text { .. })
    }

    /// Whether a person sending this takes control away from an agent. A bare release is not input.
    pub fn is_human_input(&self) -> bool {
        match self {
            Self::Pointer { events, .. } => events
                .iter()
                .any(|event| !matches!(event, PointerEvent::Release)),
            Self::Clipboard { .. } | Self::Click { .. } | Self::Key { .. } => true,
            _ => self.is_agent_input(),
        }
    }
}

pub fn valid_remote_command(command: &RemoteCommand) -> bool {
    match command {
        RemoteCommand::Clipboard {
            transfer,
            op,
            packet,
        } => {
            transfer.len() == 36
                && matches!(
                    (op, packet),
                    (ClipboardOp::Write, Some(_))
                        | (
                            ClipboardOp::Begin
                                | ClipboardOp::Copy
                                | ClipboardOp::Next
                                | ClipboardOp::Cancel,
                            None
                        )
                )
        }
        RemoteCommand::Click { x, y } => *x < 10_000 && *y < 10_000,
        RemoteCommand::Capture {
            display,
            edge,
            quality,
            ..
        } => valid_capture(*display, *edge, *quality),
        RemoteCommand::Video {
            display,
            edge,
            quality,
            fps,
        } => {
            valid_capture(*display, *edge, *quality)
                && matches!(fps, 30 | 60 | 90 | 120 | 144 | 165 | 240)
        }
        RemoteCommand::Pointer {
            display, events, ..
        } => {
            *display != 0
                && (1..=32).contains(&events.len())
                && events.iter().all(PointerEvent::valid)
        }
        RemoteCommand::UiTree { max_nodes } => (1..=MAX_UI_NODES).contains(max_nodes),
        RemoteCommand::RequestHuman { reason, timeout_s } => {
            !reason.trim().is_empty()
                && reason.chars().count() <= MAX_HELP_REASON
                && !reason.contains('\0')
                && HELP_TIMEOUT_SECONDS.contains(timeout_s)
        }
        RemoteCommand::Text { text } => {
            !text.is_empty() && text.len() <= 2048 && !text.contains('\0')
        }
        RemoteCommand::Disconnect
        | RemoteCommand::Hello
        | RemoteCommand::ReleaseControl
        | RemoteCommand::Screenshot
        | RemoteCommand::Displays
        | RemoteCommand::Key { .. } => true,
    }
}

fn valid_capture(display: u32, edge: u16, quality: u8) -> bool {
    display != 0 && (320..=4096).contains(&edge) && (20..=95).contains(&quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_gate_agent_input() {
        let pointer = RemoteCommand::Pointer {
            display: 1,
            events: vec![PointerEvent::Move { x: 1, y: 1 }],
            source_os: None,
        };
        assert!(!pointer.allowed_for(Scope::Observer));
        assert!(pointer.allowed_for(Scope::AgentSafe));
        assert!(RemoteCommand::Displays.allowed_for(Scope::Observer));
        assert!(!RemoteCommand::ReleaseControl.allowed_for(Scope::Observer));
        let clipboard = RemoteCommand::Clipboard {
            transfer: "x".into(),
            op: ClipboardOp::Copy,
            packet: None,
        };
        assert!(!clipboard.allowed_for(Scope::AgentSafe));
        assert!(clipboard.is_human_input());
        let release = RemoteCommand::Pointer {
            display: 1,
            events: vec![PointerEvent::Release],
            source_os: None,
        };
        assert!(!release.is_human_input());
    }

    #[test]
    fn decodes_the_tree_the_mac_app_sends() {
        // Shaped like Swift's JSONEncoder output: every field present, keys in any order.
        let reply: RemoteReply = serde_json::from_str(
            r#"{"status":"ui_tree","app":"System Settings","nodes":[
                {"display":1,"enabled":true,"focused":false,"role":"Window","label":"Privacy","y":519,"x":2896,"height":6364,"width":4894},
                {"focused":true,"width":658,"role":"SecureTextField","parent":0,"enabled":false,"display":1,"height":386,"x":3002,"y":1558}
            ]}"#,
        )
        .unwrap();
        let RemoteReply::UiTree {
            app,
            nodes,
            truncated,
        } = reply
        else {
            panic!("expected a tree");
        };
        assert_eq!((app.as_str(), truncated), ("System Settings", false));
        assert_eq!(nodes[1].parent, Some(0));
        assert!(nodes[1].focused && !nodes[1].enabled && nodes[1].value.is_none());
    }
}
