use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::EndpointId;
use koko_proto::{PeerKind, Scope};
use serde::{Deserialize, Serialize};

const TRUST_FILE: &str = "trusted-controllers.json";
// Releases up to 0.1.20 stored one human controller key per line.
const LEGACY_TRUST_FILE: &str = "trusted-controllers";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TrustRecord {
    pub(crate) id: EndpointId,
    pub(crate) kind: PeerKind,
    pub(crate) scope: Scope,
    pub(crate) name: String,
    pub(crate) approved_at: u64,
}

impl TrustRecord {
    pub(crate) fn new(id: EndpointId, kind: PeerKind, scope: Scope, name: &str) -> Self {
        Self {
            id,
            kind,
            scope,
            name: name.to_owned(),
            approved_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs()),
        }
    }
}

/// Remembered controllers. Without a directory nothing is persisted, matching the CLI host.
pub(crate) struct Trust {
    directory: Option<PathBuf>,
    records: Vec<TrustRecord>,
}

impl Trust {
    pub(crate) fn load(directory: Option<&Path>) -> io::Result<Self> {
        let Some(directory) = directory else {
            return Ok(Self {
                directory: None,
                records: Vec::new(),
            });
        };
        let mut trust = Self {
            directory: Some(directory.to_owned()),
            records: Vec::new(),
        };
        match std::fs::read(directory.join(TRUST_FILE)) {
            Ok(contents) => {
                trust.records = serde_json::from_slice(&contents).map_err(io::Error::other)?;
                return Ok(trust);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let legacy = directory.join(LEGACY_TRUST_FILE);
        match std::fs::read_to_string(&legacy) {
            Ok(contents) => {
                trust.records = contents
                    .lines()
                    .filter_map(|line| line.parse().ok())
                    .map(|id| TrustRecord {
                        approved_at: 0,
                        ..TrustRecord::new(id, PeerKind::Human, Scope::Human, "")
                    })
                    .collect();
                trust.save()?;
                std::fs::remove_file(legacy)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(trust)
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn get(&self, peer: EndpointId) -> Option<&TrustRecord> {
        self.records.iter().find(|record| record.id == peer)
    }

    /// Replaces any earlier record for the same key. The previous list is kept if saving fails.
    pub(crate) fn insert(&mut self, record: TrustRecord) -> io::Result<()> {
        if self.directory.is_none() {
            return Ok(());
        }
        let previous = self.records.clone();
        self.records.retain(|existing| existing.id != record.id);
        self.records.push(record);
        self.save().inspect_err(|_| self.records = previous)
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        let previous = std::mem::take(&mut self.records);
        self.save().inspect_err(|_| self.records = previous)
    }

    fn save(&self) -> io::Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(&mut file, &self.records).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.persist(directory.join(TRUST_FILE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    #[test]
    fn migrates_legacy_human_trust() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let peer = SecretKey::generate().public();
        std::fs::write(
            directory.path().join(LEGACY_TRUST_FILE),
            format!("{peer}\nnot-a-key\n"),
        )?;

        let trust = Trust::load(Some(directory.path()))?;
        assert_eq!(trust.len(), 1);
        let record = trust.get(peer).unwrap();
        assert_eq!((record.kind, record.scope), (PeerKind::Human, Scope::Human));
        assert!(!directory.path().join(LEGACY_TRUST_FILE).exists());

        let reloaded = Trust::load(Some(directory.path()))?;
        assert_eq!(reloaded.get(peer).unwrap().kind, PeerKind::Human);
        Ok(())
    }
}
