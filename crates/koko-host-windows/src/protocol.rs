use koko_proto::{ControlState, PeerKind, RemoteCommand, Scope};
use serde::{Deserialize, de::DeserializeOwned};
use std::{fmt, io, net::SocketAddr};

pub struct Secret(String);
impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Origin {
    Person,
    Agent,
    ApprovedAgent,
}
#[derive(Debug)]
pub struct DesktopRequest {
    pub id: u64,
    pub command: RemoteCommand,
    pub origin: Origin,
    pub video_socket: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct PairDetails {
    pub kind: PeerKind,
    pub name: String,
    pub scope: Scope,
}
#[derive(Debug)]
pub enum Event {
    Listening(SocketAddr),
    TrustedDevices(usize),
    DeviceId(String),
    PairCode(Secret),
    PairInvite(Secret),
    PairCodeLocked,
    CustomCodeRejected,
    PairRequest {
        id: u64,
        peer: String,
        details: PairDetails,
    },
    PairApproved,
    PairDeclined,
    PairRequestEnded,
    SessionClosed,
    AgentSession {
        peer: String,
        name: String,
        scope: Scope,
    },
    AgentSessionClosed,
    AgentControl(ControlState),
    AgentHelp {
        reason: String,
        timeout_s: u16,
    },
    AgentApproval {
        reason: String,
    },
    OutboxQueued(String),
    OutboxSent(String),
    OutboxFailed {
        batch: String,
        code: String,
    },
    OutboxRefused,
    Desktop(DesktopRequest),
    PointerClosed,
    Other,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid daemon protocol frame")
}
fn json<T: DeserializeOwned>(s: &str) -> io::Result<T> {
    serde_json::from_str(s).map_err(|_| invalid())
}
fn split(s: &str) -> io::Result<(&str, &str)> {
    s.split_once(' ')
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .ok_or_else(invalid)
}
fn number(s: &str) -> io::Result<u64> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    s.parse().map_err(|_| invalid())
}

fn desktop(body: &str) -> io::Result<DesktopRequest> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(flatten)]
        command: RemoteCommand,
        #[serde(default, deserialize_with = "present")]
        actor: Option<PeerKind>,
        #[serde(default, deserialize_with = "present")]
        approved: Option<bool>,
        #[serde(default, deserialize_with = "present")]
        video_socket: Option<String>,
    }
    fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<T>, D::Error> {
        T::deserialize(d).map(Some)
    }
    let (id, data) = split(body)?;
    let Wire {
        command,
        actor,
        approved,
        video_socket,
    } = json(data)?;
    use RemoteCommand::*;
    let media = match &command {
        Video { .. } | Audio => true,
        ClipboardRead { .. }
        | ClipboardTarget { .. }
        | ClipboardPublish { .. }
        | Displays
        | AudioVolume { .. }
        | Wallpaper
        | Screenshot
        | Capture { .. }
        | UiTree { .. }
        | Pointer { .. }
        | Click { .. }
        | Key { .. }
        | Text { .. } => false,
        _ => return Err(invalid()),
    };
    if media != video_socket.is_some() || video_socket.as_deref() == Some("") {
        return Err(invalid());
    }
    let origin = match (actor, approved, &command) {
        (None, None, _) => Origin::Person,
        (Some(PeerKind::Agent), None, Pointer { .. } | Text { .. }) => Origin::Agent,
        (Some(PeerKind::Agent), Some(true), Pointer { .. } | Text { .. }) => Origin::ApprovedAgent,
        _ => return Err(invalid()),
    };
    Ok(DesktopRequest {
        id: number(id)?,
        command,
        origin,
        video_socket,
    })
}

pub fn parse_frame(frame: &str) -> io::Result<Event> {
    use Event::*;
    let line = frame.strip_suffix('\n').ok_or_else(invalid)?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains(['\r', '\n']) {
        return Err(invalid());
    }
    for (header, event) in [
        ("Pair code locked", PairCodeLocked),
        ("Custom code rejected", CustomCodeRejected),
        ("Pair approved", PairApproved),
        ("Pair declined", PairDeclined),
        ("Pair request ended", PairRequestEnded),
        ("Session closed", SessionClosed),
        ("Agent session closed", AgentSessionClosed),
        ("Outbox refused", OutboxRefused),
        ("Pointer closed", PointerClosed),
    ] {
        if let Some(suffix) = line.strip_prefix(header) {
            if suffix.is_empty() {
                return Ok(event);
            }
            if suffix.starts_with(|ch: char| ch == ':' || ch.is_whitespace()) {
                return Err(invalid());
            }
        }
    }
    // Split the two-word protocol header before parsing its payload: colons and
    // spaces inside a socket address, invite, or JSON string belong to the body.
    let Some((first, tail)) = line.split_once(' ') else {
        return Ok(Other);
    };
    let end = tail.find([':', ' ']).unwrap_or(tail.len());
    let tag = &line[..first.len() + 1 + end];
    let rest = &tail[end..];
    let body = || {
        let separator = match tag {
            "Listening on" | "Outbox queued" | "Outbox sent" | "Outbox failed" => " ",
            _ => ": ",
        };
        rest.strip_prefix(separator)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)
    };
    Ok(match tag {
        "Listening on" => Listening(body()?.parse().map_err(|_| invalid())?),
        "Outbox queued" => OutboxQueued(body()?.into()),
        "Outbox sent" => OutboxSent(body()?.into()),
        "Outbox failed" => {
            let (batch, code) = split(body()?)?;
            OutboxFailed {
                batch: batch.into(),
                code: code.into(),
            }
        }
        "Trusted devices" => TrustedDevices(body()?.parse().map_err(|_| invalid())?),
        "Device ID" => DeviceId(body()?.into()),
        "Pair code" => PairCode(Secret(body()?.into())),
        "Pair invite" => PairInvite(Secret(body()?.into())),
        "Desktop request" => Desktop(desktop(body()?)?),
        "Pair request" => {
            let (id, rest) = split(body()?)?;
            let (peer, details) = split(rest)?;
            PairRequest {
                id: number(id)?,
                peer: peer.into(),
                details: json(details)?,
            }
        }
        "Agent session" => {
            #[derive(Deserialize)]
            struct Session {
                name: String,
                scope: Scope,
            }
            let (peer, details) = split(body()?)?;
            let Session { name, scope } = json(details)?;
            AgentSession {
                peer: peer.into(),
                name,
                scope,
            }
        }
        "Agent control" => AgentControl(
            ControlState::deserialize(
                serde::de::value::StrDeserializer::<serde::de::value::Error>::new(body()?),
            )
            .map_err(|_| invalid())?,
        ),
        "Agent help" => {
            #[derive(Deserialize)]
            struct Help {
                reason: String,
                timeout_s: u16,
            }
            let Help { reason, timeout_s } = json(body()?)?;
            AgentHelp { reason, timeout_s }
        }
        "Agent approval" => {
            #[derive(Deserialize)]
            struct Approval {
                reason: String,
            }
            let Approval { reason } = json(body()?)?;
            AgentApproval { reason }
        }
        _ => Other,
    })
}
