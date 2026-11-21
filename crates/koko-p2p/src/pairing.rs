use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr, endpoint::presets};
use koko_proto::{CONTROL_READY, PAIR_ALPN};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::time::timeout;

const SERVICE: &str = "_koko-pair._udp.local.";
const DIRECT_INVITE_PREFIX: &str = "koko://pair/v1/";
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
pub const CODE_LENGTH: usize = 10;
pub const INVITE_LIFETIME: Duration = Duration::from_secs(5 * 60);

pub struct InviteCode([u8; CODE_LENGTH]);

impl InviteCode {
    pub fn generate() -> Self {
        let random = SecretKey::generate().to_bytes();
        Self(std::array::from_fn(|index| {
            ALPHABET[usize::from(random[index] & 31)]
        }))
    }

    fn discovery_proof(&self, host: EndpointId) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"koko/lan-pair/1");
        hasher.update(host.as_bytes());
        hasher.update(&self.0);
        hasher.finalize().to_hex().to_string()
    }

    pub fn matches(&self, supplied: &[u8]) -> bool {
        blake3::hash(&self.0) == blake3::hash(supplied)
    }
}

impl fmt::Display for InviteCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, last) = self.0.split_at(5);
        write!(
            formatter,
            "{}-{}",
            std::str::from_utf8(first).unwrap(),
            std::str::from_utf8(last).unwrap()
        )
    }
}

impl FromStr for InviteCode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let code: [u8; CODE_LENGTH] = value
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'-')
            .map(|byte| byte.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| "expected a 10-character Koko connection code")?;
        if code.iter().all(|byte| ALPHABET.contains(byte)) {
            Ok(Self(code))
        } else {
            Err("invalid characters in Koko connection code")
        }
    }
}

pub fn listening_target(host: EndpointId, bind: SocketAddr) -> io::Result<EndpointAddr> {
    if !bind.ip().is_unspecified() {
        return Ok(EndpointAddr::new(host).with_ip_addr(bind));
    }

    let addresses = if_addrs::get_if_addrs()?
        .into_iter()
        .map(|interface| interface.ip())
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified())
        .map(|ip| TransportAddr::Ip(SocketAddr::new(ip, bind.port())));
    Ok(EndpointAddr::from_parts(host, addresses))
}

pub fn direct_invite(target: &EndpointAddr, code: &InviteCode) -> Option<String> {
    if target.is_empty() {
        return None;
    }
    let addresses = target
        .ip_addrs()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Some(format!(
        "{DIRECT_INVITE_PREFIX}{code}/{}/{}",
        target.id,
        addresses.join(",")
    ))
}

pub async fn resolve_invite(input: &str) -> Result<(InviteCode, EndpointAddr), Box<dyn Error>> {
    let input = input.trim();
    if let Some(invite) = input.strip_prefix(DIRECT_INVITE_PREFIX) {
        let (code, host) = invite.split_once('/').ok_or("invalid direct invite")?;
        let (host, addresses) = host.split_once('/').ok_or("invalid direct invite")?;
        let addresses = addresses
            .split(',')
            .map(str::parse::<SocketAddr>)
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| "invalid address in direct invite")?;
        if addresses.iter().any(|address| {
            !address.is_ipv4() || address.ip().is_unspecified() || address.port() == 0
        }) {
            return Err("invalid address in direct invite".into());
        }
        let target =
            EndpointAddr::from_parts(host.parse()?, addresses.into_iter().map(TransportAddr::Ip));
        Ok((code.parse()?, target))
    } else {
        let code = input.parse()?;
        let target = discover_invite(&code).await?;
        Ok((code, target))
    }
}

pub fn publish_invite(
    host: EndpointId,
    bind: SocketAddr,
    code: &InviteCode,
) -> Result<ServiceDaemon, Box<dyn Error>> {
    let proof = code.discovery_proof(host);
    let name = format!("koko-{}", &proof[..16]);
    let properties = [("id", host.to_string()), ("proof", proof)];
    let ip = if bind.ip().is_unspecified() {
        String::new()
    } else {
        bind.ip().to_string()
    };
    let mut info = ServiceInfo::new(
        SERVICE,
        &name,
        &format!("{name}.local."),
        ip,
        bind.port(),
        &properties[..],
    )?;
    if bind.ip().is_unspecified() {
        info = info.enable_addr_auto();
    }
    let daemon = ServiceDaemon::new()?;
    daemon.register(info)?;
    Ok(daemon)
}

async fn discover_invite(code: &InviteCode) -> Result<EndpointAddr, Box<dyn Error>> {
    let daemon = ServiceDaemon::new()?;
    let events = daemon.browse(SERVICE)?;
    let found = timeout(Duration::from_secs(8), async {
        while let Ok(event) = events.recv_async().await {
            if let ServiceEvent::ServiceResolved(service) = event
                && let Some(host) = service
                    .get_property_val_str("id")
                    .and_then(|id| id.parse().ok())
                && service.get_property_val_str("proof")
                    == Some(code.discovery_proof(host).as_str())
            {
                let addresses = service
                    .get_addresses_v4()
                    .into_iter()
                    .map(|ip| TransportAddr::Ip(SocketAddr::new(ip.into(), service.get_port())));
                let target = EndpointAddr::from_parts(host, addresses);
                if !target.is_empty() {
                    return Some(target);
                }
            }
        }
        None
    })
    .await;
    let _ = daemon.shutdown();
    found
        .ok()
        .flatten()
        .ok_or_else(|| "Koko host not found for this code".into())
}

pub async fn request_pairing(
    key: SecretKey,
    target: EndpointAddr,
    code: &InviteCode,
) -> Result<SocketAddr, Box<dyn Error>> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .bind()
        .await?;
    let connection = endpoint.connect(target, PAIR_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&code.0).await?;
    send.finish()?;
    let result = timeout(
        Duration::from_secs(90),
        recv.read_to_end(CONTROL_READY.len()),
    )
    .await??;
    if result != CONTROL_READY {
        return Err("invalid Koko pairing response".into());
    }
    let paths = connection.paths();
    let address = paths
        .iter()
        .filter(|path| path.is_selected())
        .find_map(|path| match path.remote_addr() {
            TransportAddr::Ip(address) => Some(*address),
            _ => None,
        })
        .ok_or("pairing completed without an active direct address")?;
    connection.close(0u32.into(), b"pairing complete");
    endpoint.close().await;
    Ok(address)
}
