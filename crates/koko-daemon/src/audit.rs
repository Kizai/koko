use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::EndpointId;
use koko_proto::{RemoteCommand, RemoteReply};

const MAX_LOG: u64 = 10 * 1024 * 1024;

/// Appends one JSON line per agent request to `audit.log`. Screen contents and typed text are never written.
#[derive(Debug)]
pub(crate) struct Audit {
    path: PathBuf,
}

impl Audit {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            path: directory.join("audit.log"),
        }
    }

    /// `approved` marks input that the person at the host allowed after a policy rule matched.
    pub(crate) fn record(
        &self,
        peer: EndpointId,
        command: &RemoteCommand,
        reply: &RemoteReply,
        approved: bool,
    ) {
        if let Err(error) = self.append(peer, command, reply, approved) {
            eprintln!("Audit log failed: {error}");
        }
    }

    fn append(
        &self,
        peer: EndpointId,
        command: &RemoteCommand,
        reply: &RemoteReply,
        approved: bool,
    ) -> io::Result<()> {
        let mut entry = serde_json::json!({
            "time": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs()),
            "peer": peer.fmt_short().to_string(),
            "action": serde_json::to_value(command)?["action"],
            "result": match reply {
                RemoteReply::Error { code } => code.as_str(),
                _ => "ok",
            },
        });
        match command {
            RemoteCommand::Pointer {
                display, events, ..
            } => {
                entry["display"] = (*display).into();
                entry["events"] = events.len().into();
            }
            RemoteCommand::Capture { display, .. } | RemoteCommand::Video { display, .. } => {
                entry["display"] = (*display).into();
            }
            RemoteCommand::Text { text } => entry["text_length"] = text.chars().count().into(),
            // The agent's own explanation of why it needs a person, kept for the host owner.
            RemoteCommand::RequestHuman { reason, .. } => entry["reason"] = reason.as_str().into(),
            _ => {}
        }
        if approved {
            entry["approved_by_person"] = true.into();
        }
        if fs::metadata(&self.path).is_ok_and(|metadata| metadata.len() > MAX_LOG) {
            fs::rename(&self.path, self.path.with_extension("log.1"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{entry}")
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    #[test]
    fn records_actions_without_typed_text() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let audit = Audit::new(directory.path());
        let peer = SecretKey::generate().public();
        audit.record(
            peer,
            &RemoteCommand::Text {
                text: "secret password".into(),
            },
            &RemoteReply::error("preempted_by_human"),
            false,
        );
        let log = fs::read_to_string(directory.path().join("audit.log"))?;
        assert!(log.contains(r#""action":"text""#), "{log}");
        assert!(log.contains(r#""text_length":15"#), "{log}");
        assert!(log.contains("preempted_by_human"), "{log}");
        assert!(!log.contains("secret"), "{log}");
        Ok(())
    }
}
