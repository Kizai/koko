//! Peer-to-peer transport and connection management for Koko.

pub mod clipboard;
pub mod pairing;
pub mod remote;

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use koko_proto::{ALPN, CONTROL_HELLO, CONTROL_READY, PAIR_ALPN, REMOTE_ALPN};
use tempfile::NamedTempFile;

/// Resolves Koko's data directory under `USERPROFILE` on Windows and `HOME` on Unix.
/// Callers create the directory as needed.
pub fn data_directory() -> io::Result<PathBuf> {
    let home_variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(home_variable).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{home_variable} is missing"),
        )
    })?;
    Ok(PathBuf::from(home).join(".koko"))
}

pub fn load_identity(path: &Path) -> io::Result<SecretKey> {
    let bytes = fs::read(path)?
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "expected a 32-byte device key"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub fn load_or_create_identity(path: &Path) -> io::Result<SecretKey> {
    match load_identity(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        result => return result,
    }

    let key = SecretKey::generate();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = NamedTempFile::new_in(directory)?;
    file.write_all(&key.to_bytes())?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(key),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => load_identity(path),
        Err(error) => Err(error.error),
    }
}

pub async fn bind_host(key: SecretKey, address: SocketAddr) -> Result<Endpoint, Box<dyn Error>> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .clear_ip_transports()
        .bind_addr(address)?
        .alpns(vec![
            ALPN.to_vec(),
            PAIR_ALPN.to_vec(),
            REMOTE_ALPN.to_vec(),
        ])
        .bind()
        .await?;
    Ok(endpoint)
}

/// Binds the requested port, or steps forward / falls back to an OS-assigned port if already in use.
pub async fn bind_host_available(
    key: SecretKey,
    address: SocketAddr,
) -> Result<Endpoint, Box<dyn Error>> {
    let preferred = address.port();
    if preferred != 0 {
        for offset in 0..=16u16 {
            if let Some(port) = preferred.checked_add(offset) {
                let candidate = SocketAddr::new(address.ip(), port);
                match bind_host(key.clone(), candidate).await {
                    Ok(endpoint) => return Ok(endpoint),
                    Err(error) if is_addr_in_use(error.as_ref()) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    bind_host(key, SocketAddr::new(address.ip(), 0)).await
}

fn is_addr_in_use(mut error: &(dyn Error + 'static)) -> bool {
    loop {
        if let Some(io_error) = error.downcast_ref::<io::Error>()
            && io_error.kind() == io::ErrorKind::AddrInUse
        {
            return true;
        }
        let Some(source) = error.source() else {
            return false;
        };
        error = source;
    }
}

pub async fn check_host(
    key: SecretKey,
    host_id: iroh::EndpointId,
    address: SocketAddr,
) -> Result<(), Box<dyn Error>> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .bind()
        .await?;
    let target = EndpointAddr::new(host_id).with_ip_addr(address);
    let connection = endpoint.connect(target, ALPN).await?;

    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(CONTROL_HELLO).await?;
    send.finish()?;
    let response = recv.read_to_end(CONTROL_READY.len()).await?;
    if response != CONTROL_READY {
        return Err(
            io::Error::new(io::ErrorKind::InvalidData, "invalid Koko control response").into(),
        );
    }

    connection.close(0u32.into(), b"check complete");
    endpoint.close().await;
    Ok(())
}

/// Returns the hosts among `targets` that complete a QUIC handshake within `wait`.
///
/// LAN discovery uses mDNS, which Tailscale, ZeroTier and most VPNs do not carry, so a device
/// remembered from an overlay network is probed at its known addresses instead. The probe uses a
/// throwaway key: the handshake alone proves the host's identity, the host refuses the
/// unauthorized key at once, and the probe gains no access.
pub async fn reachable_hosts(
    targets: Vec<EndpointAddr>,
    wait: std::time::Duration,
) -> Result<Vec<iroh::EndpointId>, Box<dyn Error>> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .bind()
        .await?;
    let mut probes = tokio::task::JoinSet::new();
    for target in targets {
        let endpoint = endpoint.clone();
        probes.spawn(async move {
            let id = target.id;
            match tokio::time::timeout(wait, endpoint.connect(target, ALPN)).await {
                Ok(Ok(connection)) => {
                    connection.close(0u32.into(), b"probe complete");
                    Some(id)
                }
                _ => None,
            }
        });
    }
    let mut reachable = Vec::new();
    while let Some(result) = probes.join_next().await {
        if let Some(id) = result? {
            reachable.push(id);
        }
    }
    // The answer is known; do not hold the caller while closing waits on silent peers.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), endpoint.close()).await;
    Ok(reachable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct Refuse;

    impl iroh::protocol::ProtocolHandler for Refuse {
        async fn accept(
            &self,
            connection: iroh::endpoint::Connection,
        ) -> Result<(), iroh::protocol::AcceptError> {
            connection.close(1u32.into(), b"peer not authorized");
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_host_that_refuses_the_probe_is_still_reachable() -> Result<(), Box<dyn Error>> {
        let host = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let address = host.bound_sockets()[0];
        let host_id = host.id();
        let router = iroh::protocol::Router::builder(host)
            .accept(ALPN, Refuse)
            .spawn();
        // Nothing listens at this address, so the second target must time out.
        let silent = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let silent_target = EndpointAddr::new(silent.id()).with_ip_addr(silent.bound_sockets()[0]);
        silent.close().await;

        let reachable = reachable_hosts(
            vec![
                EndpointAddr::new(host_id).with_ip_addr(address),
                silent_target,
            ],
            std::time::Duration::from_secs(2),
        )
        .await?;
        assert_eq!(reachable, vec![host_id]);
        router.shutdown().await?;
        Ok(())
    }

    #[test]
    fn data_directory_resolves_platform_home() {
        let home_variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        match std::env::var_os(home_variable) {
            Some(home) => assert_eq!(data_directory().unwrap(), Path::new(&home).join(".koko")),
            None => assert_eq!(
                data_directory().unwrap_err().kind(),
                io::ErrorKind::NotFound
            ),
        }
    }

    #[tokio::test]
    async fn bind_host_available_falls_back_when_port_is_occupied() -> Result<(), Box<dyn Error>> {
        let first = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let occupied = first.bound_sockets()[0];
        let second = bind_host_available(SecretKey::generate(), occupied).await?;
        assert_ne!(second.bound_sockets()[0].port(), occupied.port());
        second.close().await;
        first.close().await;
        Ok(())
    }
}
