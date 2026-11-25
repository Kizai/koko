use koko_proto::{RemoteReply, Scope};
use std::{io, path::PathBuf};

pub enum CodeMode {
    Temporary,
    Custom,
    Both,
}
pub enum HelpAnswer {
    Resolved,
    Declined,
    TakenOver,
}
pub enum Command<'a> {
    Approve(u64, Option<Scope>),
    Remember(u64, Option<Scope>),
    Decline(u64),
    EndAgent,
    StopAgent,
    ResumeAgent,
    AgentHelp(HelpAnswer, &'a str),
    Approval(bool),
    LocalInput,
    Outbox(&'a [PathBuf]),
    ForgetTrusted,
    RefreshCode,
    CodeMode(CodeMode),
    CustomCode(&'a str),
    DesktopReply(u64, &'a RemoteReply),
}
impl Command<'_> {
    pub fn encode(&self) -> io::Result<String> {
        use Command::*;
        let mut result = match self {
            Approve(id, scope) | Remember(id, scope) => {
                let verb = if matches!(self, Approve(..)) {
                    "approve"
                } else {
                    "remember"
                };
                let mut s = format!("{verb} {id}");
                if let Some(scope) = scope {
                    s.push(' ');
                    s.push_str(scope.as_str())
                }
                s
            }
            Decline(id) => format!("decline {id}"),
            EndAgent => "end-agent".into(),
            StopAgent => "stop-agent".into(),
            ResumeAgent => "resume-agent".into(),
            AgentHelp(answer, note) => {
                let a = match answer {
                    HelpAnswer::Resolved => "resolved",
                    HelpAnswer::Declined => "declined",
                    HelpAnswer::TakenOver => "taken_over",
                };
                format!("agent-help {a} {note}")
            }
            Approval(true) => "approval allow".into(),
            Approval(false) => "approval deny".into(),
            LocalInput => "local-input".into(),
            Outbox(paths) => {
                #[derive(serde::Serialize)]
                struct Offer<'a> {
                    paths: &'a [PathBuf],
                }
                format!("outbox {}", serde_json::to_string(&Offer { paths })?)
            }
            ForgetTrusted => "forget-trusted".into(),
            RefreshCode => "refresh-code".into(),
            CodeMode(mode) => format!(
                "code-mode {}",
                match mode {
                    self::CodeMode::Temporary => "temporary",
                    self::CodeMode::Custom => "custom",
                    self::CodeMode::Both => "both",
                }
            ),
            CustomCode(code) => format!("custom-code {code}"),
            DesktopReply(id, reply) => {
                format!("desktop-reply {id} {}", serde_json::to_string(reply)?)
            }
        };
        if result.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command contains a line break",
            ));
        }
        result.push('\n');
        Ok(result)
    }
}
