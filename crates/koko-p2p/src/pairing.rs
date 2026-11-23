use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr, endpoint::presets};
use koko_proto::{
    CONTROL_READY, CONTROL_REQUEST, MAX_PAIR_REPLY, PAIR_ALPN, PAIR_VERSION, PairGranted,
    PairHello, PeerKind, Scope,
};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::time::timeout;

const SERVICE: &str = "_koko-pair._udp.local.";
const DIRECT_INVITE_PREFIX: &str = "koko://pair/v1/";
const LAN_REQUEST_PREFIX: &str = "koko://request/v1/";
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

    pub fn bytes(&self) -> &[u8] {
        &self.0
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
    (!target.is_empty()).then(|| format!("{DIRECT_INVITE_PREFIX}{code}/{}", target_path(target)))
}

pub fn lan_request(target: &EndpointAddr) -> String {
    format!("{LAN_REQUEST_PREFIX}{}", target_path(target))
}

fn target_path(target: &EndpointAddr) -> String {
    let addresses = target
        .ip_addrs()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    format!("{}/{}", target.id, addresses.join(","))
}

pub async fn resolve_invite(
    input: &str,
) -> Result<(Option<InviteCode>, EndpointAddr), Box<dyn Error>> {
    let input = input.trim();
    if let Some(invite) = input.strip_prefix(DIRECT_INVITE_PREFIX) {
        let (code, target) = invite.split_once('/').ok_or("invalid direct invite")?;
        Ok((Some(code.parse()?), parse_target(target)?))
    } else if let Some(target) = input.strip_prefix(LAN_REQUEST_PREFIX) {
        Ok((None, parse_target(target)?))
    } else {
        let code = input.parse()?;
        let target = discover_invite(&code).await?;
        Ok((Some(code), target))
    }
}

fn parse_target(input: &str) -> Result<EndpointAddr, Box<dyn Error>> {
    let (host, addresses) = input.split_once('/').ok_or("invalid direct invite")?;
    let addresses = addresses
        .split(',')
        .map(str::parse::<SocketAddr>)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| "invalid address in direct invite")?;
    if addresses
        .iter()
        .any(|address| !address.is_ipv4() || address.ip().is_unspecified() || address.port() == 0)
    {
        return Err("invalid address in direct invite".into());
    }
    Ok(EndpointAddr::from_parts(
        host.parse()?,
        addresses.into_iter().map(TransportAddr::Ip),
    ))
}

pub fn publish_invite(
    host: EndpointId,
    bind: SocketAddr,
    code: &InviteCode,
    label: Option<&str>,
) -> Result<ServiceDaemon, Box<dyn Error>> {
    let proof = code.discovery_proof(host);
    let name = format!("koko-{}", &proof[..16]);
    let mut properties = vec![
        ("id", host.to_string()),
        ("proof", proof),
        ("os", std::env::consts::OS.to_owned()),
    ];
    if let Some(label) = label {
        let display_name: String = label
            .trim()
            .chars()
            .filter(|ch| !ch.is_control())
            .take(54)
            .collect();
        if !display_name.is_empty() {
            properties.push(("name", display_name));
        }
    }
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
                && let Some(target) = discovered_target(&service)
                && service.get_property_val_str("proof")
                    == Some(code.discovery_proof(target.id).as_str())
            {
                return Some(target);
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

pub struct DiscoveredHost {
    pub target: EndpointAddr,
    pub name: Option<String>,
    pub os: Option<String>,
}

pub async fn discover_hosts() -> Result<Vec<DiscoveredHost>, Box<dyn Error>> {
    let daemon = ServiceDaemon::new()?;
    let events = daemon.browse(SERVICE)?;
    let mut hosts = BTreeMap::<EndpointId, DiscoveredHost>::new();
    let _ = timeout(Duration::from_secs(2), async {
        while let Ok(event) = events.recv_async().await {
            if let ServiceEvent::ServiceResolved(service) = event
                && let Some(target) = discovered_target(&service)
            {
                let host = hosts.entry(target.id).or_insert_with(|| DiscoveredHost {
                    target: EndpointAddr::new(target.id),
                    name: None,
                    os: None,
                });
                host.target.addrs.extend(target.addrs);
                if let Some(os) = service.get_property_val_str("os") {
                    host.os = Some(os.to_owned());
                }
                if let Some(label) = service.get_property_val_str("name") {
                    host.name = Some(label.to_owned());
                }
            }
        }
    })
    .await;
    let _ = daemon.shutdown();
    Ok(hosts.into_values().collect())
}

fn discovered_target(service: &ResolvedService) -> Option<EndpointAddr> {
    let host = service.get_property_val_str("id")?.parse().ok()?;
    let addresses = service
        .get_addresses_v4()
        .into_iter()
        .map(|ip| TransportAddr::Ip(SocketAddr::new(ip.into(), service.get_port())));
    let target = EndpointAddr::from_parts(host, addresses);
    (!target.is_empty() && service.get_port() != 0).then_some(target)
}

#[derive(Default)]
pub struct PairingOptions {
    pub prefer_lan: bool,
    pub enhanced: bool,
}

fn preferred_lan_target(
    target: &EndpointAddr,
    subnets: &[(Ipv4Addr, Ipv4Addr)],
) -> Option<EndpointAddr> {
    let local = target.ip_addrs().filter(|addr| {
        let SocketAddr::V4(addr) = addr else {
            return false;
        };
        subnets.iter().any(|(local, mask)| {
            (addr.ip().to_bits() & mask.to_bits()) == (local.to_bits() & mask.to_bits())
        })
    });
    let preferred = EndpointAddr::from_parts(target.id, local.copied().map(TransportAddr::Ip));
    (!preferred.is_empty() && preferred != *target).then_some(preferred)
}

impl PairingOptions {
    async fn connect(
        &self,
        endpoint: &Endpoint,
        target: EndpointAddr,
    ) -> Result<iroh::endpoint::Connection, Box<dyn Error>> {
        if self.prefer_lan
            && target.addrs.len() > 1
            && let Ok(interfaces) = if_addrs::get_if_addrs()
        {
            let subnets: Vec<_> = interfaces
                .into_iter()
                .filter(|interface| {
                    interface.is_oper_up() && !interface.is_p2p() && !interface.is_loopback()
                })
                .filter_map(|interface| match interface.addr {
                    if_addrs::IfAddr::V4(addr) if !addr.netmask.is_unspecified() => {
                        Some((addr.ip, addr.netmask))
                    }
                    _ => None,
                })
                .collect();
            if let Some(local) = preferred_lan_target(&target, &subnets)
                && let Ok(Ok(connection)) =
                    timeout(Duration::from_secs(2), endpoint.connect(local, PAIR_ALPN)).await
            {
                return Ok(connection);
            }
        }

        if self.enhanced
            && let Ok(Ok(connection)) = timeout(
                Duration::from_secs(4),
                endpoint.connect(target.clone(), PAIR_ALPN),
            )
            .await
        {
            return Ok(connection);
        }
        Ok(endpoint.connect(target, PAIR_ALPN).await?)
    }
}

pub async fn request_pairing(
    key: SecretKey,
    target: EndpointAddr,
    code: Option<&InviteCode>,
) -> Result<SocketAddr, Box<dyn Error>> {
    request_pairing_with_options(key, target, code, PairingOptions::default()).await
}

pub async fn request_pairing_with_options(
    key: SecretKey,
    target: EndpointAddr,
    code: Option<&InviteCode>,
    options: PairingOptions,
) -> Result<SocketAddr, Box<dyn Error>> {
    let payload = code.map_or(CONTROL_REQUEST, |code| &code.0);
    let (address, reply) = exchange(key, target, payload, CONTROL_READY.len(), options).await?;
    if reply != CONTROL_READY {
        return Err("invalid Koko pairing response".into());
    }
    Ok(address)
}

/// Pairs an agent identity. The host confirms the first pairing even when the code is valid,
/// and may grant a narrower scope than requested.
pub async fn request_agent_pairing(
    key: SecretKey,
    target: EndpointAddr,
    code: Option<&InviteCode>,
    name: &str,
    scope: Scope,
    options: PairingOptions,
) -> Result<(SocketAddr, Scope), Box<dyn Error>> {
    let hello = PairHello {
        v: PAIR_VERSION,
        kind: PeerKind::Agent,
        name: name.to_owned(),
        scope,
        code: code.map(ToString::to_string),
    };
    if !hello.valid() {
        return Err("invalid agent name or scope".into());
    }
    let payload = serde_json::to_vec(&hello)?;
    let (address, reply) = exchange(key, target, &payload, MAX_PAIR_REPLY, options).await?;
    let granted: PairGranted =
        serde_json::from_slice(&reply).map_err(|_| "invalid Koko pairing response")?;
    if granted.v != PAIR_VERSION || granted.scope > scope {
        return Err("invalid Koko pairing response".into());
    }
    Ok((address, granted.scope))
}

async fn exchange(
    key: SecretKey,
    target: EndpointAddr,
    payload: &[u8],
    reply_limit: usize,
    options: PairingOptions,
) -> Result<(SocketAddr, Vec<u8>), Box<dyn Error>> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .bind()
        .await?;
    let connection = options.connect(&endpoint, target).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(payload).await?;
    send.finish()?;
    let reply = timeout(Duration::from_secs(90), recv.read_to_end(reply_limit)).await??;
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
    Ok((address, reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_preference_uses_the_interface_netmask() {
        let host = SecretKey::generate().public();
        let local: SocketAddr = "172.16.7.9:4444".parse().unwrap();
        let other: SocketAddr = "172.16.8.9:4444".parse().unwrap();
        let overlay: SocketAddr = "100.64.1.1:4444".parse().unwrap();
        let target = EndpointAddr::new(host)
            .with_ip_addr(local)
            .with_ip_addr(other)
            .with_ip_addr(overlay);
        let subnet = [(
            "172.16.4.100".parse().unwrap(),
            "255.255.252.0".parse().unwrap(),
        )];
        assert_eq!(
            preferred_lan_target(&target, &subnet),
            Some(EndpointAddr::new(host).with_ip_addr(local))
        );
        assert!(preferred_lan_target(&target, &[]).is_none());
        assert!(
            preferred_lan_target(&EndpointAddr::new(host).with_ip_addr(local), &subnet).is_none()
        );
    }

    #[tokio::test]
    async fn enhanced_pairing_retries_handshake_but_honors_decline() -> Result<(), Box<dyn Error>> {
        let host = crate::bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let target = EndpointAddr::new(host.id()).with_ip_addr(host.bound_sockets()[0]);
        let client = request_pairing_with_options(
            SecretKey::generate(),
            target,
            None,
            PairingOptions {
                enhanced: true,
                ..Default::default()
            },
        );
        let server = async {
            host.accept().await.unwrap().refuse();
            let connection = host.accept().await.unwrap().await.unwrap();
            let (_send, mut recv) = connection.accept_bi().await.unwrap();
            assert_eq!(
                recv.read_to_end(CODE_LENGTH).await.unwrap(),
                CONTROL_REQUEST
            );
            connection.close(1u32.into(), b"pairing declined or expired");
        };
        let (result, ()) = timeout(Duration::from_secs(5), async {
            tokio::join!(client, server)
        })
        .await?;
        let error = format!("{:?}", result.unwrap_err());
        assert!(error.contains("pairing declined or expired"), "{error}");
        Ok(())
    }
}
