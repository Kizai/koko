//! Peer-to-peer transport and connection management for Koko.

pub mod clipboard;
pub mod pairing;
pub mod remote;

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::Path;

use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use koko_proto::{ALPN, CONTROL_HELLO, CONTROL_READY, PAIR_ALPN, REMOTE_ALPN};
use tempfile::NamedTempFile;

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
