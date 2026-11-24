//! Shared protocol types, request validation and size limits for Koko.

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"koko/1";
pub const PAIR_ALPN: &[u8] = b"koko/pair/1";
pub const REMOTE_ALPN: &[u8] = b"koko/remote/1";
pub const DEFAULT_PORT: u16 = 27688;
pub const CONTROL_REQUEST: &[u8] = b"request";
pub const MAX_PAIR_REQUEST: usize = 1024;
pub const MAX_PAIR_REPLY: usize = 256;
pub const PAIR_VERSION: u8 = 3;
pub const MAX_PEER_NAME: usize = 64;
/// A request header; a clipboard push carries its file list here, so it allows long lists.
pub const MAX_REMOTE_REQUEST: usize = 4 * 1024 * 1024;
pub const MAX_REMOTE_REPLY: usize = 16 * 1024 * 1024;
pub const MAX_VIDEO_PACKET: usize = 8 * 1024 * 1024;
pub const VIDEO_ERROR: u8 = 3;
pub const VIDEO_PATH: u8 = 4;
/// A video frame with a big-endian u32 sequence number and a flags byte before its payload.
/// Hosts send it only to controllers that asked for `feedback`.
pub const VIDEO_SEQUENCED_FRAME: u8 = 9;
pub const VIDEO_FRAME_KEY: u8 = 1;

// Records a controller writes back on a video stream it opened with `feedback`. Each is a
// big-endian u32 length, a kind byte and its payload, like the video records themselves.
/// The newest frame the controller has received, as a big-endian u32 sequence number.
pub const FEEDBACK_ACK: u8 = 1;
/// The controller cannot decode until the next key frame.
pub const FEEDBACK_KEY_FRAME: u8 = 2;

/// The length field of a valid feedback record, or `None` for a kind that does not exist.
pub fn feedback_length(kind: u8) -> Option<u32> {
    match kind {
        FEEDBACK_ACK => Some(5),
        FEEDBACK_KEY_FRAME => Some(1),
        _ => None,
    }
}

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
    /// Proves the controller knows a verification code without sending it: a keyed hash of
    /// the host and controller keys, written as hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<String>,
    /// Asks the host to refuse at once with `PAIR_CODE_REQUIRED` instead of asking its owner,
    /// when neither a code nor trust lets this client in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quiet: bool,
}

/// The close reason for a quiet pairing request that needs a verification code.
pub const PAIR_CODE_REQUIRED: &str = "code_required";
/// The close reason when the controller speaks another pairing version, so the app can ask
/// for both sides to be updated instead of showing a generic error.
pub const PAIR_VERSION_MISMATCH: &str = "pair_version_mismatch";
/// The close reason when a host stops checking codes for a while after too many wrong ones.
pub const PAIR_LOCKED: &str = "too_many_attempts";

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
    /// Copies the host's clipboard to the controller. The reply stream carries a manifest record,
    /// then every file's bytes in manifest order.
    ClipboardPull {
        transfer: String,
    },
    /// Pastes onto the host. The request header is followed by every file's bytes in manifest order.
    ClipboardPush {
        transfer: String,
        manifest: ClipboardManifest,
    },
    /// Daemon to app only: press Copy and list what the pasteboard holds.
    ClipboardRead {
        transfer: String,
    },
    /// Daemon to app only: create an empty folder to receive pasted files.
    ClipboardTarget {
        transfer: String,
    },
    /// Daemon to app only: put received text or files on the pasteboard and press Paste.
    ClipboardPublish {
        transfer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        paths: Vec<String>,
    },
    /// Waits up to `OUTBOX_WAIT` for files the person at the host offers to the controller.
    OutboxWait,
    /// Streams one offered batch like `clipboard_pull`: a manifest record, then the bytes.
    OutboxPull {
        offer: String,
    },
    Disconnect,
    Hello,
    ReleaseControl,
    RequestHuman {
        reason: String,
        timeout_s: u16,
    },
    Screenshot,
    /// The desktop picture of the main display without any windows, as a JPEG `image` reply,
    /// so a controller can show it as the device's cover.
    Wallpaper,
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
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        audio: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hevc: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        chroma444: bool,
        /// The controller acknowledges frames and asks for key frames on the same stream, so the
        /// host can skip frames that would only queue up and adapt its bitrate to the link.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        feedback: bool,
    },
    /// Subscribes to live desktop audio on its own QUIC stream, avoiding video head-of-line blocking.
    Audio,
    /// Reads (`level: None`) or sets (`level: Some(0..=100)`) the host's system output volume.
    AudioVolume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level: Option<u8>,
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

/// What a clipboard transfer carries: text, or files and folders listed parent first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipboardManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ClipboardEntry>,
}

/// A file or folder, named by a `/`-separated path relative to the transfer's folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub path: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub size: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dir: bool,
}

fn is_zero(size: &u64) -> bool {
    *size == 0
}

/// A batch of files the person at the host dropped to send, named by its top-level items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutboxOffer {
    pub id: String,
    pub names: Vec<String>,
}

/// How long a controller's outbox wait stays open before it is answered with no offers.
pub const OUTBOX_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

pub const MAX_CLIPBOARD_TEXT: usize = 1024 * 1024;
pub const MAX_CLIPBOARD_ENTRIES: usize = 20_000;
/// Records on a clipboard pull stream share the video framing: big-endian length, kind, payload.
pub const CLIPBOARD_MANIFEST: u8 = 1;
pub const CLIPBOARD_ERROR: u8 = 3;

impl ClipboardManifest {
    /// Text alone, or entries alone, with every path relative and free of `.` and `..`.
    pub fn valid(&self) -> bool {
        let text_ok = self
            .text
            .as_ref()
            .is_none_or(|text| text.len() <= MAX_CLIPBOARD_TEXT && !text.contains('\0'));
        let one_kind = self.text.is_some() != !self.entries.is_empty();
        text_ok
            && one_kind
            && self.entries.len() <= MAX_CLIPBOARD_ENTRIES
            && self
                .entries
                .iter()
                .all(|entry| safe_relative_path(&entry.path) && !(entry.dir && entry.size != 0))
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }
}

/// Rejects absolute paths, empty or `.`/`..` components, NUL and over-long names.
pub fn safe_relative_path(path: &str) -> bool {
    let components: Vec<&str> = path.split('/').collect();
    !path.is_empty()
        && components.len() <= 64
        && components.iter().all(|component| {
            !component.is_empty()
                && *component != "."
                && *component != ".."
                && component.len() <= 255
                && !component.contains('\0')
        })
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
    Ok,
    /// Files waiting at the host for this controller; empty when the wait timed out.
    Outbox {
        offers: Vec<OutboxOffer>,
    },
    /// App to daemon only: what Copy put on the host's pasteboard, as absolute paths or text.
    ClipboardItems {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default)]
        paths: Vec<String>,
    },
    /// App to daemon only: the folder that receives pasted files.
    ClipboardFolder {
        path: String,
    },
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
    AudioVolume {
        level: u8,
        muted: bool,
    },
    Displays {
        host_name: String,
        displays: Vec<RemoteDisplay>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        video: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clipboard: bool,
        /// The host streams clipboard files, so any size can be copied and pasted.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clipboard_stream: bool,
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
            Self::ClipboardRead { .. }
            | Self::ClipboardTarget { .. }
            | Self::ClipboardPublish { .. } => 120,
            // The host holds the request open until a person answers.
            Self::RequestHuman { timeout_s, .. } => u64::from(*timeout_s) + 5,
            Self::OutboxWait => OUTBOX_WAIT.as_secs() + 5,
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

    /// What an agent may look at. Desktop audio is left to people: an agent asked to watch the
    /// screen should not also hear calls or media playing at the host.
    fn is_viewing(&self) -> bool {
        matches!(
            self,
            Self::Hello
                | Self::Displays
                | Self::UiTree { .. }
                | Self::Screenshot
                | Self::Wallpaper
                | Self::Capture { .. }
                | Self::Video { audio: false, .. }
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
            Self::ClipboardPull { .. }
            | Self::ClipboardPush { .. }
            | Self::Click { .. }
            | Self::Key { .. } => true,
            _ => self.is_agent_input(),
        }
    }
}

pub fn valid_remote_command(command: &RemoteCommand) -> bool {
    match command {
        RemoteCommand::ClipboardPull { transfer } => transfer.len() == 36,
        RemoteCommand::OutboxPull { offer } => (1..=64).contains(&offer.len()),
        RemoteCommand::ClipboardPush { transfer, manifest } => {
            transfer.len() == 36 && manifest.valid()
        }
        // Only the daemon sends these to its own app.
        RemoteCommand::ClipboardRead { .. }
        | RemoteCommand::ClipboardTarget { .. }
        | RemoteCommand::ClipboardPublish { .. } => false,
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
            ..
        } => {
            valid_capture(*display, *edge, *quality)
                && matches!(fps, 30 | 60 | 90 | 120 | 144 | 165 | 240)
        }
        RemoteCommand::AudioVolume { level } => level.is_none_or(|value| value <= 100),
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
        | RemoteCommand::OutboxWait
        | RemoteCommand::Hello
        | RemoteCommand::ReleaseControl
        | RemoteCommand::Screenshot
        | RemoteCommand::Wallpaper
        | RemoteCommand::Audio
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
        assert!(RemoteCommand::Wallpaper.allowed_for(Scope::Observer));
        assert!(!RemoteCommand::ReleaseControl.allowed_for(Scope::Observer));
        assert!(!RemoteCommand::Audio.allowed_for(Scope::AgentSafe));
        assert!(!RemoteCommand::AudioVolume { level: None }.allowed_for(Scope::Observer));
        let video = |audio| RemoteCommand::Video {
            display: 1,
            edge: 1280,
            quality: 70,
            fps: 30,
            audio,
            hevc: false,
            chroma444: false,
            feedback: false,
        };
        assert!(video(false).allowed_for(Scope::Observer));
        assert!(!video(true).allowed_for(Scope::AgentSafe));
        assert!(video(true).allowed_for(Scope::Human));
        let clipboard = RemoteCommand::ClipboardPull {
            transfer: "x".into(),
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
    fn clipboard_manifests_stay_inside_their_folder() {
        let entry = |path: &str| ClipboardEntry {
            path: path.into(),
            size: 1,
            dir: false,
        };
        let files = |paths: &[&str]| ClipboardManifest {
            text: None,
            entries: paths.iter().map(|path| entry(path)).collect(),
        };
        assert!(files(&["Report.pdf", "Photos/a.jpg"]).valid());
        for bad in ["/etc/passwd", "../x", "a/../b", "a//b", "", "a/./b"] {
            assert!(!files(&[bad]).valid(), "{bad}");
        }
        assert!(
            !ClipboardManifest::default().valid(),
            "a transfer carries something"
        );
        let both = ClipboardManifest {
            text: Some("hi".into()),
            entries: vec![entry("a")],
        };
        assert!(!both.valid());
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

    #[test]
    fn video_feedback_is_opt_in_and_records_have_fixed_sizes() {
        let old = r#"{"action":"video","display":1,"edge":1280,"quality":70,"fps":60}"#;
        let parsed: RemoteCommand = serde_json::from_str(old).unwrap();
        assert!(matches!(
            parsed,
            RemoteCommand::Video {
                feedback: false,
                ..
            }
        ));
        assert!(!serde_json::to_string(&parsed).unwrap().contains("feedback"));
        let new =
            r#"{"action":"video","display":1,"edge":1280,"quality":70,"fps":60,"feedback":true}"#;
        assert!(matches!(
            serde_json::from_str(new).unwrap(),
            RemoteCommand::Video { feedback: true, .. }
        ));
        assert_eq!(feedback_length(FEEDBACK_ACK), Some(5));
        assert_eq!(feedback_length(FEEDBACK_KEY_FRAME), Some(1));
        assert_eq!(feedback_length(0), None);
    }
}
