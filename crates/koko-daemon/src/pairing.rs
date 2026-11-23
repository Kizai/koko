use std::error::Error;
use std::fmt;
use std::io::{self, BufRead};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::{
    EndpointAddr, EndpointId, SecretKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use koko_p2p::{
    bind_host_available,
    pairing::{VerificationCode, device_id, direct_invite, listening_target, publish_host},
};
use koko_proto::{
    ALPN, CONTROL_READY, CONTROL_REQUEST, HumanResult, MAX_HELP_REASON, MAX_PAIR_REQUEST,
    PAIR_ALPN, PAIR_CODE_REQUIRED, PAIR_VERSION, PairGranted, PairHello, PeerKind, REMOTE_ALPN,
    Scope,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    Host,
    arbiter::{AGENT_IDLE, Arbiter, HUMAN_GRACE},
    audit::Audit,
    desktop::{AppDesktop, AppDesktopRequests, RemoteHost},
    outbox::Outbox,
    session::{Sessions, report_end},
    trust::{Trust, TrustRecord},
};

const DECLINED: &str = "pairing declined or expired";
const AGENT_BUSY: &str = "agent_busy";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeMode {
    Temporary,
    Custom,
    Both,
}

impl CodeMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "temporary" => Some(Self::Temporary),
            "custom" => Some(Self::Custom),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    fn allows_temporary(self) -> bool {
        matches!(self, Self::Temporary | Self::Both)
    }

    fn allows_custom(self) -> bool {
        matches!(self, Self::Custom | Self::Both)
    }
}

struct Codes {
    mode: CodeMode,
    temporary: VerificationCode,
    custom: Option<VerificationCode>,
}

impl Codes {
    fn new() -> Self {
        Self {
            mode: CodeMode::Both,
            temporary: VerificationCode::generate(),
            custom: None,
        }
    }

    fn matches(&self, supplied: &VerificationCode) -> bool {
        (self.mode.allows_temporary() && self.temporary.matches(supplied))
            || (self.mode.allows_custom()
                && self
                    .custom
                    .as_ref()
                    .is_some_and(|code| code.matches(supplied)))
    }

    fn refresh_temporary(&mut self) -> VerificationCode {
        self.temporary = VerificationCode::generate();
        self.temporary.clone()
    }
}

struct AgentRequest {
    name: String,
    scope: Scope,
}

struct PairRequest {
    peer: EndpointId,
    by_code: bool,
    quiet: bool,
    /// None for people, including every client that predates pairing version 2.
    agent: Option<AgentRequest>,
    human_name: String,
    decision: oneshot::Sender<Result<Scope, &'static str>>,
}

impl PairRequest {
    fn scope(&self) -> Scope {
        self.agent
            .as_ref()
            .map_or(Scope::Human, |agent| agent.scope)
    }

    fn describe(&self) -> String {
        let (kind, name) = match &self.agent {
            Some(agent) => (PeerKind::Agent, agent.name.as_str()),
            None => (PeerKind::Human, self.human_name.as_str()),
        };
        serde_json::json!({"kind": kind, "name": name, "scope": self.scope()}).to_string()
    }
}

struct Pairing {
    codes: Arc<Mutex<Codes>>,
    requests: mpsc::Sender<PairRequest>,
}

impl fmt::Debug for Pairing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Pairing").finish_non_exhaustive()
    }
}

impl ProtocolHandler for Pairing {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let bytes = recv
            .read_to_end(MAX_PAIR_REQUEST)
            .await
            .map_err(AcceptError::from_err)?;
        let (by_code, quiet, agent, human_name, versioned) = if bytes.first() == Some(&b'{') {
            let Some(hello) = serde_json::from_slice::<PairHello>(&bytes)
                .ok()
                .filter(PairHello::valid)
            else {
                connection.close(1u32.into(), b"invalid pairing request");
                return Ok(());
            };
            let by_code = match hello.code.as_deref().map(str::parse::<VerificationCode>) {
                None => false,
                Some(Ok(code)) if self.codes.lock().unwrap().matches(&code) => true,
                Some(_) => {
                    connection.close(1u32.into(), b"invalid pairing code");
                    return Ok(());
                }
            };
            let name = hello.name.trim().to_owned();
            let agent = (hello.kind == PeerKind::Agent).then(|| AgentRequest {
                name: name.clone(),
                scope: hello.scope,
            });
            (by_code, hello.quiet, agent, name, true)
        } else {
            let by_code = if bytes == CONTROL_REQUEST {
                false
            } else if let Ok(text) = std::str::from_utf8(&bytes)
                && let Ok(code) = text.parse::<VerificationCode>()
                && self.codes.lock().unwrap().matches(&code)
            {
                true
            } else {
                connection.close(1u32.into(), b"invalid pairing code");
                return Ok(());
            };
            (by_code, false, None, String::new(), false)
        };

        let (decision, response) = oneshot::channel();
        let request = PairRequest {
            peer: connection.remote_id(),
            by_code,
            quiet,
            agent,
            human_name,
            decision,
        };
        if self.requests.try_send(request).is_err() {
            connection.close(1u32.into(), b"pairing unavailable");
            return Ok(());
        }

        let scope = tokio::select! {
            _ = connection.closed() => return Ok(()),
            decision = response => match decision {
                Ok(Ok(scope)) => scope,
                Ok(Err(reason)) => {
                    connection.close(1u32.into(), reason.as_bytes());
                    return Ok(());
                }
                Err(_) => {
                    connection.close(1u32.into(), DECLINED.as_bytes());
                    return Ok(());
                }
            },
        };
        let reply = if versioned {
            serde_json::to_vec(&PairGranted {
                v: PAIR_VERSION,
                scope,
            })
            .map_err(AcceptError::from_err)?
        } else {
            CONTROL_READY.to_vec()
        };
        send.write_all(&reply)
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

/// Pairing decisions for one listening host. Every state change is reported on stdout for the app.
struct HostState {
    sessions: watch::Sender<Sessions>,
    arbiter: Arc<Arbiter>,
    outbox: Arc<Outbox>,
    trust: Trust,
    codes: Arc<Mutex<Codes>>,
    target: Option<EndpointAddr>,
    pending: Option<(u64, PairRequest)>,
    request_number: u64,
}

impl HostState {
    fn request(&mut self, request: PairRequest) {
        if request.decision.is_closed() {
            return;
        }
        let sessions = self.sessions.borrow().clone();
        let trusted = self.trust.get(request.peer);
        let approved = match &request.agent {
            None => {
                if sessions.human.is_some() {
                    let _ = request.decision.send(Err(DECLINED));
                    return;
                }
                request.by_code || trusted.is_some_and(|record| record.kind == PeerKind::Human)
            }
            Some(agent) => {
                if sessions
                    .agent
                    .is_some_and(|(current, _)| current != request.peer)
                {
                    let _ = request.decision.send(Err(AGENT_BUSY));
                    return;
                }
                if sessions.human == Some(request.peer) {
                    let _ = request.decision.send(Err(DECLINED));
                    return;
                }
                // A verification code only finds the host. An agent's first pairing is always confirmed.
                trusted.is_some_and(|record| {
                    record.kind == PeerKind::Agent && agent.scope <= record.scope
                })
            }
        };
        if approved {
            let scope = request.scope();
            self.approve(request, scope, false);
        } else if request.agent.is_none() && request.quiet {
            let _ = request.decision.send(Err(PAIR_CODE_REQUIRED));
        } else if (request.agent.is_some() || !request.by_code)
            && self
                .pending
                .as_ref()
                .is_none_or(|(_, current)| current.decision.is_closed())
        {
            self.request_number += 1;
            println!(
                "Pair request: {} {} {}",
                self.request_number,
                request.peer.fmt_short(),
                request.describe()
            );
            self.pending = Some((self.request_number, request));
        } else {
            let _ = request.decision.send(Err(DECLINED));
        }
    }

    fn approve(&mut self, request: PairRequest, scope: Scope, remember: bool) {
        if remember {
            let (kind, name) = match &request.agent {
                Some(agent) => (PeerKind::Agent, agent.name.as_str()),
                None => (PeerKind::Human, request.human_name.as_str()),
            };
            match self
                .trust
                .insert(TrustRecord::new(request.peer, kind, scope, name))
            {
                Ok(()) => println!("Trusted devices: {}", self.trust.len()),
                Err(error) => eprintln!("Trust storage failed: {error}"),
            }
        }
        match &request.agent {
            None => {
                self.sessions
                    .send_modify(|sessions| sessions.human = Some(request.peer));
                println!("Pair approved");
            }
            Some(agent) => {
                self.sessions
                    .send_modify(|sessions| sessions.agent = Some((request.peer, scope)));
                println!(
                    "Agent session: {} {}",
                    request.peer.fmt_short(),
                    serde_json::json!({"name": agent.name, "scope": scope})
                );
            }
        }
        self.pending = None;
        let _ = request.decision.send(Ok(scope));
    }

    fn refresh_temporary_code(&self) {
        let code = self.codes.lock().unwrap().refresh_temporary();
        println!("Pair code: {code}");
        if let Some(target) = &self.target
            && let Some(direct) = direct_invite(target, &code)
        {
            println!("Pair invite: {direct}");
        }
    }

    /// Handles a line from the app. `approve` and `remember` may name a lower scope for an agent.
    fn command(&mut self, line: &str) {
        match line {
            "forget-trusted" => {
                if let Err(error) = self.trust.clear() {
                    eprintln!("Trust storage failed: {error}");
                    return;
                }
                let ended = self.sessions.send_replace(Sessions::default());
                if ended.human.is_some() {
                    report_end(PeerKind::Human);
                    self.outbox.clear();
                }
                if ended.agent.is_some() {
                    report_end(PeerKind::Agent);
                    self.arbiter.reset();
                }
                println!("Trusted devices: 0");
                return;
            }
            "refresh-code" => {
                self.refresh_temporary_code();
                return;
            }
            "end-agent" => {
                if self
                    .sessions
                    .send_if_modified(|sessions| sessions.agent.take().is_some())
                {
                    report_end(PeerKind::Agent);
                    self.arbiter.reset();
                }
                return;
            }
            "stop-agent" => {
                self.arbiter.stop();
                return;
            }
            "resume-agent" => {
                self.arbiter.resume();
                return;
            }
            "approval allow" | "approval deny" => {
                self.arbiter.decide(line == "approval allow");
                return;
            }
            // The app saw someone use this Mac's own keyboard or mouse.
            "local-input" => {
                self.arbiter.human_input();
                return;
            }
            _ => {}
        }
        if let Some(mode) = line.strip_prefix("code-mode ") {
            if let Some(mode) = CodeMode::parse(mode.trim()) {
                self.codes.lock().unwrap().mode = mode;
            }
            return;
        }
        if let Some(raw) = line.strip_prefix("custom-code ") {
            let trimmed = raw.trim();
            let mut codes = self.codes.lock().unwrap();
            if trimmed.is_empty() {
                codes.custom = None;
            } else if let Ok(code) = trimmed.parse::<VerificationCode>() {
                codes.custom = Some(code);
            }
            return;
        }
        // The person at the host dropped files to send to the controller.
        if let Some(offer) = line.strip_prefix("outbox ") {
            #[derive(serde::Deserialize)]
            struct Offer {
                paths: Vec<std::path::PathBuf>,
            }
            match serde_json::from_str::<Offer>(offer) {
                Ok(offer) if !offer.paths.is_empty() && self.sessions.borrow().human.is_some() => {
                    println!("Outbox queued {}", self.outbox.add(offer.paths));
                }
                _ => println!("Outbox refused"),
            }
            return;
        }
        if let Some(answer) = line.strip_prefix("agent-help ") {
            let (result, note) = answer.split_once(' ').unwrap_or((answer, ""));
            if let Ok(result) = result.parse::<HumanResult>() {
                let note: String = note
                    .trim()
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(MAX_HELP_REASON)
                    .collect();
                self.arbiter
                    .answer(result, (!note.is_empty()).then_some(note));
            }
            return;
        }
        let mut fields = line.split(' ');
        let (Some(action), Some(number)) = (fields.next(), fields.next()) else {
            return;
        };
        if !matches!(action, "approve" | "decline" | "remember")
            || self
                .pending
                .as_ref()
                .is_none_or(|(id, _)| number.parse::<u64>() != Ok(*id))
        {
            return;
        }
        let requested = self.pending.as_ref().map(|(_, request)| request.scope());
        let scope = match (fields.next().map(str::parse::<Scope>), requested) {
            (None, Some(requested)) => requested,
            // A host can narrow an agent's access but never widen it.
            (Some(Ok(scope)), Some(requested)) if scope <= requested && scope != Scope::Human => {
                scope
            }
            _ => return,
        };
        let (_, request) = self.pending.take().unwrap();
        if request.decision.is_closed() {
            println!("Pair request ended");
        } else if action == "decline" {
            let _ = request.decision.send(Err(DECLINED));
            println!("Pair declined");
        } else {
            self.approve(request, scope, action == "remember");
        }
    }
}

pub(crate) async fn serve(
    key: SecretKey,
    address: SocketAddr,
    app: Option<(&Path, &str)>,
) -> Result<(), Box<dyn Error>> {
    if !address.is_ipv4() {
        return Err("pairing requires an IPv4 listen address".into());
    }

    let directory = app.map(|(file, _)| file.parent().unwrap_or_else(|| Path::new(".")));
    let trust = Trust::load(directory)?;
    let arbiter = Arc::new(Arbiter::new(HUMAN_GRACE, AGENT_IDLE));
    let outbox = Arc::new(Outbox::default());
    let endpoint = bind_host_available(key, address).await?;
    let address = endpoint.bound_sockets()[0];
    let target = listening_target(endpoint.id(), address)?;
    let codes = Arc::new(Mutex::new(Codes::new()));
    let display_code = codes.lock().unwrap().temporary.to_string();
    let direct = direct_invite(&target, &codes.lock().unwrap().temporary);
    let advertisement = match publish_host(endpoint.id(), address, app.map(|(_, name)| name)) {
        Ok(advertisement) => Some(advertisement),
        Err(error) => {
            eprintln!("Discovery unavailable: {error}");
            None
        }
    };
    let (sessions, authorized) = watch::channel(Sessions::default());
    let mut session_changes = sessions.subscribe();
    let (requests, mut incoming) = mpsc::channel(1);
    let (app_desktop, mut desktop_requests) = AppDesktop::channel();
    let host_device_id = device_id(endpoint.id());
    let router = Router::builder(endpoint)
        .accept(
            REMOTE_ALPN,
            RemoteHost {
                sessions: sessions.clone(),
                app_desktop: app.is_some().then_some(app_desktop),
                arbiter: arbiter.clone(),
                audit: directory.map(Audit::new),
                outbox: outbox.clone(),
                pointer: tokio::sync::Mutex::new(()),
                #[cfg(unix)]
                video: tokio::sync::Mutex::new(()),
            },
        )
        .accept(ALPN, Host { authorized })
        .accept(
            PAIR_ALPN,
            Pairing {
                codes: codes.clone(),
                requests,
            },
        )
        .spawn();

    println!("Listening on {address}");
    println!("Device ID: {host_device_id}");
    println!("Trusted devices: {}", trust.len());
    println!("Pair code: {display_code}");
    if let Some(direct) = direct {
        println!("Pair invite: {direct}");
    }

    let (input, mut commands) = mpsc::channel(1);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if input.blocking_send(line).is_err() {
                break;
            }
        }
    });

    let mut state = HostState {
        sessions,
        arbiter,
        outbox,
        trust,
        codes,
        target: Some(target),
        pending: None,
        request_number: 0,
    };
    let mut had_human = false;
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut app_desktops = AppDesktopRequests::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            biased;
            result = &mut interrupt => {
                result?;
                break;
            }
            _ = tick.tick() => state.arbiter.expire(),
            Ok(()) = session_changes.changed() => {
                let has_human = session_changes.borrow_and_update().human.is_some();
                if had_human && !has_human {
                    state.refresh_temporary_code();
                }
                had_human = has_human;
            }
            Some(request) = incoming.recv() => state.request(request),
            Some(request) = desktop_requests.recv() => app_desktops.request(request),
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command.strip_prefix("desktop-reply ") {
                    Some(reply) => app_desktops.reply(reply),
                    None => state.command(&command),
                }
            }
        }
    }
    if let Some(advertisement) = advertisement {
        let _ = advertisement.shutdown();
    }
    router.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use iroh::EndpointAddr;
    use koko_p2p::{
        bind_host, check_host,
        pairing::{
            HumanPairing, PairingOptions, lan_request, request_agent_pairing,
            request_human_pairing, request_pairing, resolve_invite,
        },
    };
    use tokio::time::timeout;

    use super::*;

    struct TestHost {
        router: Router,
        id: EndpointId,
        address: SocketAddr,
        code: String,
        incoming: mpsc::Receiver<PairRequest>,
        state: HostState,
        _trust: tempfile::TempDir,
    }

    async fn test_host() -> Result<TestHost, Box<dyn Error>> {
        let key = SecretKey::generate();
        let id = key.public();
        let endpoint = bind_host(key, "127.0.0.1:0".parse()?).await?;
        let address = endpoint.bound_sockets()[0];
        let (sessions, authorized) = watch::channel(Sessions::default());
        let (requests, incoming) = mpsc::channel(1);
        let codes = Arc::new(Mutex::new(Codes::new()));
        let display = codes.lock().unwrap().temporary.to_string();
        let router = Router::builder(endpoint)
            .accept(ALPN, Host { authorized })
            .accept(
                PAIR_ALPN,
                Pairing {
                    codes: codes.clone(),
                    requests,
                },
            )
            .spawn();
        let directory = tempfile::tempdir()?;
        Ok(TestHost {
            router,
            id,
            address,
            code: display,
            incoming,
            state: HostState {
                sessions,
                arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::ZERO)),
                outbox: Default::default(),
                trust: Trust::load(Some(directory.path()))?,
                codes,
                target: None,
                pending: None,
                request_number: 0,
            },
            _trust: directory,
        })
    }

    fn agent_pair(
        host: &TestHost,
        agent: &SecretKey,
        code: Option<&str>,
        scope: Scope,
    ) -> impl Future<Output = Result<(SocketAddr, Scope), Box<dyn Error>>> + use<> {
        let target = EndpointAddr::new(host.id).with_ip_addr(host.address);
        let code = code.map(|code| code.parse::<VerificationCode>().unwrap());
        let agent = agent.clone();
        async move {
            request_agent_pairing(
                agent,
                target,
                code.as_ref(),
                "Test Agent",
                scope,
                PairingOptions::default(),
            )
            .await
        }
    }

    #[tokio::test]
    async fn a_discovered_host_requires_approval() -> Result<(), Box<dyn Error>> {
        let host_key = SecretKey::generate();
        let host_id = host_key.public();
        let controller = SecretKey::generate();
        let endpoint = bind_host(host_key, "0.0.0.0:0".parse()?).await?;
        let bind = endpoint.bound_sockets()[0];
        assert!(bind.ip().is_unspecified());
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, bind.port()));
        let (sessions, authorized) = watch::channel(Sessions::default());
        let (requests, mut incoming) = mpsc::channel(1);
        let codes = Arc::new(Mutex::new(Codes::new()));
        let invitation = lan_request(
            &EndpointAddr::new(host_id)
                .with_ip_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)))
                .with_ip_addr(address),
        );
        let (try_code, target) = resolve_invite(&invitation)?;
        assert!(try_code.is_none());
        let router = Router::builder(endpoint)
            .accept(ALPN, Host { authorized })
            .accept(PAIR_ALPN, Pairing { codes, requests })
            .spawn();

        timeout(Duration::from_secs(10), async {
            let approve = async {
                let request = incoming.recv().await.unwrap();
                assert_eq!(request.peer, controller.public());
                let denied = check_host(controller.clone(), host_id, address)
                    .await
                    .expect_err("a pending pair must not allow ordinary connections");
                assert!(format!("{denied:?}").contains("peer not authorized"));
                sessions.send_modify(|sessions| sessions.human = Some(request.peer));
                request.decision.send(Ok(Scope::Human)).unwrap();
            };
            let (paired, ()) = tokio::join!(
                request_pairing(controller.clone(), target, try_code.as_ref()),
                approve,
            );
            // A host bound to 0.0.0.0 may answer on any local interface, so only the port is fixed.
            assert_eq!(paired?.port(), address.port());
            check_host(controller, host_id, address).await
        })
        .await??;
        router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn quiet_human_pairing_requires_a_code_and_supports_custom_codes()
    -> Result<(), Box<dyn Error>> {
        let mut host = test_host().await?;
        let controller = SecretKey::generate();
        let target = EndpointAddr::new(host.id).with_ip_addr(host.address);

        timeout(Duration::from_secs(10), async {
            let quiet_no_code = request_human_pairing(
                controller.clone(),
                target.clone(),
                HumanPairing {
                    name: "MacBook",
                    code: None,
                    quiet: true,
                },
                PairingOptions::default(),
            );
            let answer = async {
                host.state.request(host.incoming.recv().await.unwrap());
                assert!(host.state.pending.is_none());
            };
            let (result, ()) = tokio::join!(quiet_no_code, answer);
            assert!(format!("{:?}", result.unwrap_err()).contains(PAIR_CODE_REQUIRED));

            host.state.command("custom-code MYCODE88");
            host.state.command("code-mode custom");
            let custom: VerificationCode = "mycode88".parse().unwrap();
            let by_custom = request_human_pairing(
                controller.clone(),
                target.clone(),
                HumanPairing {
                    name: "MacBook",
                    code: Some(&custom),
                    quiet: true,
                },
                PairingOptions::default(),
            );
            let accept = async { host.state.request(host.incoming.recv().await.unwrap()) };
            let (paired, ()) = tokio::join!(by_custom, accept);
            assert_eq!(paired?.port(), host.address.port());
            Ok::<_, Box<dyn Error>>(())
        })
        .await??;
        host.router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn an_agent_is_confirmed_once_then_reconnects_by_trust() -> Result<(), Box<dyn Error>> {
        let mut host = test_host().await?;
        let agent = SecretKey::generate();

        timeout(Duration::from_secs(10), async {
            // A valid code does not skip the host's confirmation for an agent.
            let pairing = agent_pair(&host, &agent, Some(&host.code.clone()), Scope::Observer);
            let confirm = async {
                let request = host.incoming.recv().await.unwrap();
                assert!(request.by_code && request.agent.is_some());
                host.state.request(request);
                assert!(
                    host.state.pending.is_some(),
                    "the agent must wait for the host"
                );
                host.state.command("remember 1");
            };
            let (paired, ()) = tokio::join!(pairing, confirm);
            assert_eq!(paired?.1, Scope::Observer);
            assert_eq!(
                host.state.sessions.borrow().scope(agent.public()),
                Some(Scope::Observer)
            );

            // After the host restarts its session, the trusted agent reconnects without a prompt.
            host.state.sessions.send_replace(Sessions::default());
            let pairing = agent_pair(&host, &agent, None, Scope::Observer);
            let trusted = async {
                let request = host.incoming.recv().await.unwrap();
                host.state.request(request);
                assert!(host.state.pending.is_none());
            };
            let (paired, ()) = tokio::join!(pairing, trusted);
            assert_eq!(paired?.1, Scope::Observer);

            // Asking for more than the host granted needs a new confirmation.
            let pairing = agent_pair(&host, &agent, None, Scope::AgentSafe);
            let wider = async {
                let request = host.incoming.recv().await.unwrap();
                host.state.request(request);
                assert!(host.state.pending.is_some());
                host.state.command("decline 2");
            };
            let (paired, ()) = tokio::join!(pairing, wider);
            assert!(format!("{:?}", paired.unwrap_err()).contains(DECLINED));
            Ok::<_, Box<dyn Error>>(())
        })
        .await??;
        host.router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn one_agent_shares_the_host_with_one_person() -> Result<(), Box<dyn Error>> {
        let mut host = test_host().await?;
        let person = SecretKey::generate();
        let first = SecretKey::generate();
        let second = SecretKey::generate();

        timeout(Duration::from_secs(10), async {
            let pairing = agent_pair(&host, &first, None, Scope::AgentSafe);
            let confirm = async {
                host.state.request(host.incoming.recv().await.unwrap());
                host.state.command("approve 1 observer");
            };
            let (paired, ()) = tokio::join!(pairing, confirm);
            assert_eq!(paired?.1, Scope::Observer, "the host may narrow the scope");

            let pairing = agent_pair(&host, &second, None, Scope::Observer);
            let busy = async { host.state.request(host.incoming.recv().await.unwrap()) };
            let (paired, ()) = tokio::join!(pairing, busy);
            assert!(format!("{:?}", paired.unwrap_err()).contains(AGENT_BUSY));

            let target = EndpointAddr::new(host.id).with_ip_addr(host.address);
            let code = host.code.parse::<VerificationCode>()?;
            let reopen = async {
                host.state.request(host.incoming.recv().await.unwrap());
            };
            let (paired, ()) =
                tokio::join!(request_pairing(person.clone(), target, Some(&code)), reopen);
            paired?;
            let sessions = host.state.sessions.borrow().clone();
            assert_eq!(sessions.scope(person.public()), Some(Scope::Human));
            assert_eq!(sessions.scope(first.public()), Some(Scope::Observer));
            check_host(person, host.id, host.address).await?;
            check_host(first, host.id, host.address).await
        })
        .await??;
        host.router.shutdown().await?;
        Ok(())
    }
}
