use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::path::Path;

use iroh::{
    EndpointId, SecretKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use koko_p2p::{
    bind_host,
    pairing::{
        CODE_LENGTH, INVITE_LIFETIME, InviteCode, direct_invite, listening_target, publish_invite,
    },
};
use koko_proto::{ALPN, CONTROL_READY, CONTROL_REQUEST, PAIR_ALPN, REMOTE_ALPN};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    Host,
    desktop::{AppDesktop, AppDesktopRequests, RemoteHost},
};

struct PairRequest {
    peer: EndpointId,
    by_code: bool,
    decision: oneshot::Sender<bool>,
}

struct Pairing {
    code: InviteCode,
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
        let code = recv
            .read_to_end(CODE_LENGTH)
            .await
            .map_err(AcceptError::from_err)?;
        let by_code = self.code.matches(&code);
        if !by_code && code != CONTROL_REQUEST {
            connection.close(1u32.into(), b"invalid pairing code");
            return Ok(());
        }

        let (decision, response) = oneshot::channel();
        let request = PairRequest {
            peer: connection.remote_id(),
            by_code,
            decision,
        };
        if self.requests.try_send(request).is_err() {
            connection.close(1u32.into(), b"pairing unavailable");
            return Ok(());
        }

        tokio::select! {
            _ = connection.closed() => return Ok(()),
            accepted = response => {
                if !matches!(accepted, Ok(true)) {
                    connection.close(1u32.into(), b"pairing declined or expired");
                    return Ok(());
                }
            }
        }
        send.write_all(CONTROL_READY)
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
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

    let trust_file = app.map(|(file, _)| file.with_file_name("trusted-controllers"));
    let mut trusted = match trust_file.as_deref() {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(contents) => contents
                .lines()
                .filter_map(|line| line.parse().ok())
                .collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error.into()),
        },
        None => BTreeSet::new(),
    };

    let endpoint = bind_host(key, address).await?;
    let address = endpoint.bound_sockets()[0];
    let code = InviteCode::generate();
    let display_code = code.to_string();
    let direct = direct_invite(&listening_target(endpoint.id(), address)?, &code);
    let advertisement =
        match publish_invite(endpoint.id(), address, &code, app.map(|(_, name)| name)) {
            Ok(advertisement) => Some(advertisement),
            Err(error) => {
                eprintln!("Discovery unavailable: {error}");
                None
            }
        };
    let (allow_peer, allowed_peer) = watch::channel(None);
    let (requests, mut incoming) = mpsc::channel(1);
    let (app_desktop, mut desktop_requests) = AppDesktop::channel();
    let router = Router::builder(endpoint)
        .accept(
            REMOTE_ALPN,
            RemoteHost {
                allowed_peer: allow_peer.clone(),
                app_desktop: app.is_some().then_some(app_desktop),
                pointer: tokio::sync::Mutex::new(()),
                #[cfg(unix)]
                video: tokio::sync::Mutex::new(()),
            },
        )
        .accept(ALPN, Host { allowed_peer })
        .accept(PAIR_ALPN, Pairing { code, requests })
        .spawn();

    println!("Listening on {address}");
    println!("Trusted devices: {}", trusted.len());
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

    let interrupt = tokio::signal::ctrl_c();
    let expires = tokio::time::sleep(INVITE_LIFETIME);
    tokio::pin!(interrupt, expires);
    let mut open = true;
    let mut pending: Option<(u64, PairRequest)> = None;
    let mut request_number = 0;
    let mut app_desktops = AppDesktopRequests::default();
    loop {
        tokio::select! {
            biased;
            result = &mut interrupt => {
                result?;
                break;
            }
            _ = &mut expires, if open => {
                open = false;
                println!("Pair code expired");
            }
            Some(request) = incoming.recv() => {
                if request.decision.is_closed() {
                    continue;
                }
                if allow_peer.borrow().is_some() {
                    let _ = request.decision.send(false);
                    continue;
                }
                if (open && request.by_code) || (!request.by_code && trusted.contains(&request.peer)) {
                    println!("Pair approved");
                    allow_peer.send_replace(Some(request.peer));
                    let _ = request.decision.send(true);
                    open = false;
                    pending = None;
                } else if !request.by_code
                    && pending.as_ref().is_none_or(|(_, current)| current.decision.is_closed())
                {
                    request_number += 1;
                    println!("Pair request: {request_number} {}", request.peer.fmt_short());
                    pending = Some((request_number, request));
                } else {
                    let _ = request.decision.send(false);
                }
            }
            Some(request) = desktop_requests.recv() => app_desktops.request(request),
            command = commands.recv() => {
                let Some(command) = command else { break; };
                if command == "forget-trusted" {
                    if let Some(path) = &trust_file {
                        if let Err(error) = save_trusted(path, &BTreeSet::new()) {
                            eprintln!("Trust storage failed: {error}");
                            continue;
                        }
                        trusted.clear();
                        allow_peer.send_if_modified(|peer| {
                            if peer.is_none() { return false; }
                            println!("Session closed");
                            *peer = None;
                            true
                        });
                        println!("Trusted devices: 0");
                    }
                    continue;
                }
                let Some((action, number)) = command.split_once(' ') else { continue; };
                if action == "desktop-reply" {
                    app_desktops.reply(number);
                    continue;
                }
                if !matches!(action, "approve" | "decline" | "remember")
                    || pending.as_ref().is_none_or(|(id, _)| number.parse::<u64>() != Ok(*id))
                {
                    continue;
                }
                let (_, request) = pending.take().unwrap();
                if request.decision.is_closed() {
                    println!("Pair request ended");
                } else if action == "decline" {
                    let _ = request.decision.send(false);
                    println!("Pair declined");
                } else {
                    if action == "remember" && let Some(path) = &trust_file {
                        trusted.insert(request.peer);
                        if let Err(error) = save_trusted(path, &trusted) {
                            trusted.remove(&request.peer);
                            eprintln!("Trust storage failed: {error}");
                        } else {
                            println!("Trusted devices: {}", trusted.len());
                        }
                    }
                    println!("Pair approved");
                    allow_peer.send_replace(Some(request.peer));
                    let _ = request.decision.send(true);
                    open = false;
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

fn save_trusted(path: &Path, trusted: &BTreeSet<EndpointId>) -> io::Result<()> {
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    for peer in trusted {
        writeln!(file, "{peer}")?;
    }
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use iroh::EndpointAddr;
    use koko_p2p::{
        check_host,
        pairing::{lan_request, request_pairing, resolve_invite},
    };
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn a_discovered_host_requires_approval() -> Result<(), Box<dyn Error>> {
        let host_key = SecretKey::generate();
        let host_id = host_key.public();
        let controller = SecretKey::generate();
        let endpoint = bind_host(host_key, "0.0.0.0:0".parse()?).await?;
        let bind = endpoint.bound_sockets()[0];
        assert!(bind.ip().is_unspecified());
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, bind.port()));
        let (allow_peer, allowed_peer) = watch::channel(None);
        let (requests, mut incoming) = mpsc::channel(1);
        let code = InviteCode::generate();
        let invitation = lan_request(
            &EndpointAddr::new(host_id)
                .with_ip_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)))
                .with_ip_addr(address),
        );
        let (try_code, target) = resolve_invite(&invitation).await?;
        assert!(try_code.is_none());
        let router = Router::builder(endpoint)
            .accept(ALPN, Host { allowed_peer })
            .accept(PAIR_ALPN, Pairing { code, requests })
            .spawn();

        timeout(Duration::from_secs(10), async {
            let approve = async {
                let request = incoming.recv().await.unwrap();
                assert_eq!(request.peer, controller.public());
                let denied = check_host(controller.clone(), host_id, address)
                    .await
                    .expect_err("a pending pair must not allow ordinary connections");
                assert!(format!("{denied:?}").contains("peer not authorized"));
                allow_peer.send_replace(Some(request.peer));
                request.decision.send(true).unwrap();
            };
            let (paired, ()) = tokio::join!(
                request_pairing(controller.clone(), target, try_code.as_ref()),
                approve,
            );
            assert_eq!(paired?, address);
            check_host(controller, host_id, address).await
        })
        .await??;
        router.shutdown().await?;
        Ok(())
    }
}
