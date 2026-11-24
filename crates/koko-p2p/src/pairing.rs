use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::str::FromStr;
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr, endpoint::presets};
use koko_proto::{
    MAX_PAIR_REPLY, PAIR_ALPN, PAIR_VERSION, PairGranted, PairHello, PeerKind, Scope,
};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::time::timeout;

const SERVICE: &str = "_koko-pair._udp.local.";
const DIRECT_INVITE_PREFIX: &str = "koko://pair/v1/";
const LAN_REQUEST_PREFIX: &str = "koko://request/v1/";
/// Letters and digits that cannot be mistaken for one another, for generated codes.
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
pub const TEMPORARY_CODE_LENGTH: usize = 8;
/// Lengths accepted for any code, including one the host owner chose.
pub const CODE_LENGTHS: RangeInclusive<usize> = 6..=16;
/// The shortest code a host owner may choose. It never expires, so it is longer than the least.
pub const CUSTOM_CODE_MIN: usize = 8;
/// How long a lookup by device ID listens for the host on the local network.
const FIND_TIMEOUT: Duration = Duration::from_secs(5);

/// A verification code: ASCII letters and digits, compared without case, spaces or dashes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationCode(String);

impl VerificationCode {
    pub fn generate() -> Self {
        let random = SecretKey::generate().to_bytes();
        Self(
            random[..TEMPORARY_CODE_LENGTH]
                .iter()
                .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
                .collect(),
        )
    }

    /// Compares hashes, so the time taken does not reveal how much of a guess was right.
    pub fn matches(&self, other: &Self) -> bool {
        blake3::hash(self.0.as_bytes()) == blake3::hash(other.0.as_bytes())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a host owner may keep this as a code that never expires: at least
    /// `CUSTOM_CODE_MIN` characters, mixing letters and digits.
    pub fn strong_enough_to_keep(&self) -> bool {
        self.0.len() >= CUSTOM_CODE_MIN
            && self.0.bytes().any(|byte| byte.is_ascii_alphabetic())
            && self.0.bytes().any(|byte| byte.is_ascii_digit())
    }

    /// Proves knowledge of this code to one host, for one controller, without sending the code.
    /// A device that only pretends to be the host learns nothing it can replay at the real one.
    pub fn proof(&self, host: EndpointId, controller: EndpointId) -> String {
        self.proof_hash(host, controller).to_hex().to_string()
    }

    /// Checks a proof from `controller`. Hash equality is constant time.
    pub fn verifies(&self, proof: &str, host: EndpointId, controller: EndpointId) -> bool {
        blake3::Hash::from_hex(proof).is_ok_and(|proof| proof == self.proof_hash(host, controller))
    }

    fn proof_hash(&self, host: EndpointId, controller: EndpointId) -> blake3::Hash {
        let key = blake3::derive_key("koko/pair-proof/1", self.0.as_bytes());
        let mut hasher = blake3::Hasher::new_keyed(&key);
        hasher.update(host.as_bytes());
        hasher.update(controller.as_bytes());
        hasher.finalize()
    }
}

impl fmt::Display for VerificationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for VerificationCode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let code: String = value
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '-')
            .map(|ch| ch.to_ascii_uppercase())
            .collect();
        if CODE_LENGTHS.contains(&code.len()) && code.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            Ok(Self(code))
        } else {
            Err("expected a 6-16 character verification code of letters and digits")
        }
    }
}

/// The 9-digit number people type to reach a host. It comes from the host key, so it never
/// changes and a controller can check it against the key the host proves in QUIC.
pub fn device_id(host: EndpointId) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"koko/device-id/1");
    hasher.update(host.as_bytes());
    let hash = hasher.finalize();
    let number = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
    (100_000_000 + number % 900_000_000).to_string()
}

/// Accepts a device ID written with or without spaces, such as `253 153 157`.
pub fn parse_device_id(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|ch| !ch.is_whitespace()).collect();
    (digits.len() == 9
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && !digits.starts_with('0'))
    .then_some(digits)
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

pub fn direct_invite(target: &EndpointAddr, code: &VerificationCode) -> Option<String> {
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

/// Reads a direct invite, which carries a code, or a LAN request, which does not.
pub fn resolve_invite(
    input: &str,
) -> Result<(Option<VerificationCode>, EndpointAddr), Box<dyn Error>> {
    let input = input.trim();
    if let Some(invite) = input.strip_prefix(DIRECT_INVITE_PREFIX) {
        let (code, target) = invite.split_once('/').ok_or("invalid direct invite")?;
        Ok((Some(code.parse()?), parse_target(target)?))
    } else if let Some(target) = input.strip_prefix(LAN_REQUEST_PREFIX) {
        Ok((None, parse_target(target)?))
    } else {
        Err("expected a KoKo direct invite".into())
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

/// Announces the host on the local network. Codes are never published; the device ID is
/// derived from the announced key.
pub fn publish_host(
    host: EndpointId,
    bind: SocketAddr,
    label: Option<&str>,
) -> Result<ServiceDaemon, Box<dyn Error>> {
    let name = format!("koko-{}", device_id(host));
    let mut properties = vec![
        ("id", host.to_string()),
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

pub struct DiscoveredHost {
    pub target: EndpointAddr,
    pub name: Option<String>,
    pub os: Option<String>,
}

impl DiscoveredHost {
    fn add(&mut self, service: &ResolvedService, target: EndpointAddr) {
        self.target.addrs.extend(target.addrs);
        if let Some(os) = service.get_property_val_str("os") {
            self.os = Some(os.to_owned());
        }
        if let Some(label) = service.get_property_val_str("name") {
            self.name = Some(label.to_owned());
        }
    }
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
                hosts
                    .entry(target.id)
                    .or_insert_with(|| DiscoveredHost {
                        target: EndpointAddr::new(target.id),
                        name: None,
                        os: None,
                    })
                    .add(&service, target);
            }
        }
    })
    .await;
    let _ = daemon.shutdown();
    Ok(hosts.into_values().collect())
}

/// Finds the host with this device ID on the local network. When it does not answer there,
/// falls back to `known`, the addresses a controller remembers from an earlier connection.
///
/// A device ID is short enough that someone could make a key with the same one, so once a
/// controller knows the host's key it only accepts a host that has that key.
pub async fn find_device(
    device: &str,
    known: Option<EndpointAddr>,
) -> Result<DiscoveredHost, Box<dyn Error>> {
    let known = known.filter(|target| device_id(target.id) == device);
    let pinned = known.as_ref().map(|target| target.id);
    let daemon = ServiceDaemon::new()?;
    let events = daemon.browse(SERVICE)?;
    let found = timeout(FIND_TIMEOUT, async {
        while let Ok(event) = events.recv_async().await {
            if let ServiceEvent::ServiceResolved(service) = event
                && let Some(target) = discovered_target(&service)
                && device_id(target.id) == device
                && pinned.is_none_or(|key| key == target.id)
            {
                let mut host = DiscoveredHost {
                    target: EndpointAddr::new(target.id),
                    name: None,
                    os: None,
                };
                host.add(&service, target);
                return Some(host);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = daemon.shutdown();
    match (found, known) {
        (Some(mut host), Some(known)) => {
            host.target.addrs.extend(known.addrs);
            Ok(host)
        }
        (Some(host), None) => Ok(host),
        (None, Some(known)) => Ok(DiscoveredHost {
            target: known,
            name: None,
            os: None,
        }),
        (None, None) => Err("Koko device not found".into()),
    }
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

/// How a person's controller asks a host to let it in.
pub struct HumanPairing<'a> {
    /// This controller's name, shown to the host owner when they are asked.
    pub name: &'a str,
    pub code: Option<&'a VerificationCode>,
    /// Refuse with `code_required` instead of asking the host owner.
    pub quiet: bool,
}

pub async fn request_pairing(
    key: SecretKey,
    target: EndpointAddr,
    code: Option<&VerificationCode>,
) -> Result<SocketAddr, Box<dyn Error>> {
    let pairing = HumanPairing {
        name: "KoKo",
        code,
        quiet: false,
    };
    request_human_pairing(key, target, pairing, PairingOptions::default()).await
}

/// Pairs a person's controller. A valid code or the host's trust lets it in at once;
/// otherwise the host owner is asked, unless the request is quiet.
pub async fn request_human_pairing(
    key: SecretKey,
    target: EndpointAddr,
    pairing: HumanPairing<'_>,
    options: PairingOptions,
) -> Result<SocketAddr, Box<dyn Error>> {
    let hello = PairHello {
        v: PAIR_VERSION,
        kind: PeerKind::Human,
        name: pairing.name.to_owned(),
        scope: Scope::Human,
        proof: pairing.code.map(|code| code.proof(target.id, key.public())),
        quiet: pairing.quiet,
    };
    if !hello.valid() {
        return Err("invalid controller name".into());
    }
    let payload = serde_json::to_vec(&hello)?;
    let (address, reply) = exchange(key, target, &payload, MAX_PAIR_REPLY, options).await?;
    let granted: PairGranted =
        serde_json::from_slice(&reply).map_err(|_| "invalid Koko pairing response")?;
    if granted.v != PAIR_VERSION || granted.scope != Scope::Human {
        return Err("invalid Koko pairing response".into());
    }
    Ok(address)
}

/// Pairs an agent identity. The host confirms the first pairing even when the code is valid,
/// and may grant a narrower scope than requested.
pub async fn request_agent_pairing(
    key: SecretKey,
    target: EndpointAddr,
    code: Option<&VerificationCode>,
    name: &str,
    scope: Scope,
    options: PairingOptions,
) -> Result<(SocketAddr, Scope), Box<dyn Error>> {
    let hello = PairHello {
        v: PAIR_VERSION,
        kind: PeerKind::Agent,
        name: name.to_owned(),
        scope,
        proof: code.map(|code| code.proof(target.id, key.public())),
        quiet: false,
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
    use koko_proto::MAX_PAIR_REQUEST;

    use super::*;

    #[test]
    fn device_ids_are_stable_nine_digit_numbers() {
        let host = SecretKey::generate().public();
        let id = device_id(host);
        assert_eq!(id, device_id(host));
        assert_eq!(
            parse_device_id(&format!("{} {} {}", &id[..3], &id[3..6], &id[6..])),
            Some(id)
        );
        assert_eq!(parse_device_id("12345678"), None);
        assert_eq!(parse_device_id("012345678"), None);
    }

    #[test]
    fn codes_ignore_case_spaces_and_dashes() {
        let generated = VerificationCode::generate();
        assert_eq!(generated.as_str().len(), TEMPORARY_CODE_LENGTH);
        let typed: VerificationCode = "ab12-cd 34".parse().unwrap();
        assert!(typed.matches(&"AB12CD34".parse().unwrap()));
        assert!("abc12".parse::<VerificationCode>().is_err(), "too short");
        assert!("abc12!".parse::<VerificationCode>().is_err(), "punctuation");
        let host = SecretKey::generate().public();
        let controller = SecretKey::generate().public();
        let proof = generated.proof(host, controller);
        assert!(!proof.contains(generated.as_str()));
        assert!(generated.verifies(&proof, host, controller));
        let elsewhere = SecretKey::generate().public();
        assert!(
            !generated.verifies(&proof, elsewhere, controller),
            "a proof made for a look-alike host is useless at the real one"
        );
        assert!(!generated.verifies(&proof, host, elsewhere));
        assert!(!generated.verifies("not hex", host, controller));
        assert!(
            "KOKO2026"
                .parse::<VerificationCode>()
                .unwrap()
                .strong_enough_to_keep()
        );
        assert!(
            !"KOKO26"
                .parse::<VerificationCode>()
                .unwrap()
                .strong_enough_to_keep()
        );
        assert!(
            !"12345678"
                .parse::<VerificationCode>()
                .unwrap()
                .strong_enough_to_keep()
        );
        assert!(
            !"ABCDEFGH"
                .parse::<VerificationCode>()
                .unwrap()
                .strong_enough_to_keep()
        );
    }

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
        let pairing = HumanPairing {
            name: "Studio",
            code: None,
            quiet: false,
        };
        let options = PairingOptions {
            enhanced: true,
            ..Default::default()
        };
        let client = request_human_pairing(SecretKey::generate(), target, pairing, options);
        let server = async {
            host.accept().await.unwrap().refuse();
            let connection = host.accept().await.unwrap().await.unwrap();
            let (_send, mut recv) = connection.accept_bi().await.unwrap();
            let hello: PairHello =
                serde_json::from_slice(&recv.read_to_end(MAX_PAIR_REQUEST).await.unwrap()).unwrap();
            assert_eq!(
                (hello.kind, hello.name.as_str()),
                (PeerKind::Human, "Studio")
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
