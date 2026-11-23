use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use iroh::{EndpointAddr, EndpointId};
use koko_proto::Scope;
use serde::{Deserialize, Serialize};

/// A host this agent identity has paired with.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostRecord {
    pub id: EndpointId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub addresses: Vec<SocketAddr>,
    pub scope: Scope,
    pub paired_at: u64,
}

impl HostRecord {
    pub fn target(&self) -> EndpointAddr {
        self.addresses
            .iter()
            .fold(EndpointAddr::new(self.id), |target, address| {
                target.with_ip_addr(*address)
            })
    }
}

/// Paired hosts, stored next to the agent key so each identity keeps its own list.
pub struct Registry {
    path: PathBuf,
    pub hosts: Vec<HostRecord>,
}

impl Registry {
    pub fn path_for(identity: &Path) -> PathBuf {
        let stem = identity
            .file_stem()
            .map_or_else(|| "agent".into(), |stem| stem.to_string_lossy());
        identity.with_file_name(format!("{stem}-hosts.json"))
    }

    pub fn load(path: PathBuf) -> io::Result<Self> {
        let hosts = match std::fs::read(&path) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, hosts })
    }

    pub fn get(&self, id: EndpointId) -> Option<&HostRecord> {
        self.hosts.iter().find(|host| host.id == id)
    }

    pub fn upsert(&mut self, record: HostRecord) -> io::Result<()> {
        self.hosts.retain(|host| host.id != record.id);
        self.hosts.push(record);
        self.save()
    }

    pub fn save(&self) -> io::Result<()> {
        let directory = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(&mut file, &self.hosts).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.persist(&self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    #[test]
    fn keeps_one_record_per_host() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = Registry::path_for(&directory.path().join("agent.key"));
        assert_eq!(path.file_name().unwrap(), "agent-hosts.json");
        let id = SecretKey::generate().public();
        let record = |name: &str| HostRecord {
            id,
            name: name.into(),
            os: None,
            addresses: vec!["192.168.1.10:4444".parse().unwrap()],
            scope: Scope::Observer,
            paired_at: 1,
        };

        let mut registry = Registry::load(path.clone())?;
        registry.upsert(record("Old"))?;
        registry.upsert(record("Studio"))?;
        let reloaded = Registry::load(path)?;
        assert_eq!(reloaded.hosts.len(), 1);
        assert_eq!(reloaded.get(id).unwrap().name, "Studio");
        assert_eq!(reloaded.get(id).unwrap().target().addrs.len(), 1);
        Ok(())
    }
}
