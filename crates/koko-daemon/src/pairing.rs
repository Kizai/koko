use std::error::Error;
use std::fmt;
use std::io::{self, BufRead};
use std::net::SocketAddr;

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
use koko_proto::{ALPN, CONTROL_READY, PAIR_ALPN};
use tokio::sync::{mpsc, oneshot, watch};

use crate::Host;

struct PairRequest {
    peer: EndpointId,
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
        if !self.code.matches(&code) {
            connection.close(1u32.into(), b"invalid pairing code");
            return Ok(());
        }

        let (decision, response) = oneshot::channel();
        let request = PairRequest {
            peer: connection.remote_id(),
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

pub(crate) async fn serve(key: SecretKey, address: SocketAddr) -> Result<(), Box<dyn Error>> {
    if !address.is_ipv4() {
        return Err("pairing requires an IPv4 listen address".into());
    }

    let endpoint = bind_host(key, address).await?;
    let address = endpoint.bound_sockets()[0];
    let code = InviteCode::generate();
    let display_code = code.to_string();
    let direct = direct_invite(&listening_target(endpoint.id(), address)?, &code);
    let mut advertisement = match publish_invite(endpoint.id(), address, &code) {
        Ok(advertisement) => Some(advertisement),
        Err(error) => {
            eprintln!("Discovery unavailable: {error}");
            None
        }
    };
    let mut stop_discovery = || {
        if let Some(advertisement) = advertisement.take() {
            let _ = advertisement.shutdown();
        }
    };
    let (allow_peer, allowed_peer) = watch::channel(None);
    let (requests, mut incoming) = mpsc::channel(1);
    let router = Router::builder(endpoint)
        .accept(ALPN, Host { allowed_peer })
        .accept(PAIR_ALPN, Pairing { code, requests })
        .spawn();

    println!("Listening on {address}");
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
    loop {
        tokio::select! {
            biased;
            result = &mut interrupt => {
                result?;
                break;
            }
            _ = &mut expires, if open => {
                open = false;
                pending = None;
                incoming.close();
                stop_discovery();
                println!("Pair code expired");
            }
            Some(request) = incoming.recv() => {
                if open && pending.as_ref().is_none_or(|(_, current)| current.decision.is_closed()) {
                    request_number += 1;
                    println!("Pair request: {request_number} {}", request.peer.fmt_short());
                    pending = Some((request_number, request));
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let Some((action, number)) = command.split_once(' ') else { continue; };
                if !matches!(action, "approve" | "decline")
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
                    allow_peer.send_replace(Some(request.peer));
                    let _ = request.decision.send(true);
                    open = false;
                    incoming.close();
                    stop_discovery();
                    println!("Pair approved");
                }
            }
        }
    }
    stop_discovery();
    router.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use iroh::EndpointAddr;
    use koko_p2p::{
        check_host,
        pairing::{request_pairing, resolve_invite},
    };
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn a_direct_invite_to_a_wildcard_listener_still_requires_approval()
    -> Result<(), Box<dyn Error>> {
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
        let invitation = direct_invite(
            &EndpointAddr::new(host_id)
                .with_ip_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)))
                .with_ip_addr(address),
            &code,
        )
        .unwrap();
        let (try_code, target) = resolve_invite(&invitation).await?;
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
                request_pairing(controller.clone(), target, &try_code),
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
