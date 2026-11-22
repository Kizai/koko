mod desktop;
mod pairing;

use std::error::Error;
use std::path::Path;

use iroh::{
    EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use koko_p2p::{bind_host, load_identity, load_or_create_identity};
use koko_proto::{ALPN, CONTROL_HELLO, CONTROL_READY};

#[derive(Debug)]
struct Host {
    allowed_peer: tokio::sync::watch::Receiver<Option<EndpointId>>,
}

impl ProtocolHandler for Host {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if *self.allowed_peer.borrow() != Some(connection.remote_id()) {
            connection.close(1u32.into(), b"peer not authorized");
            return Ok(());
        }

        let (mut send, mut recv) = connection.accept_bi().await?;
        let request = recv
            .read_to_end(CONTROL_HELLO.len())
            .await
            .map_err(AcceptError::from_err)?;
        if request != CONTROL_HELLO {
            connection.close(2u32.into(), b"invalid Koko control request");
            return Ok(());
        }

        send.write_all(CONTROL_READY)
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [option] if option == "--version" => println!("koko-daemon {}", env!("CARGO_PKG_VERSION")),
        #[cfg(target_os = "macos")]
        [command, action] if matches!(command.as_str(), "screen-permission" | "input-permission") => {
            let request = match action.as_str() {
                "check" => false,
                "request" => true,
                _ => return Err("expected permission action check or request".into()),
            };
            let granted = match desktop::permission(command, request) {
                Ok(granted) => granted,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && command == "screen-permission" => {
                    koko_capture::check_screen_recording_permission(request)
                }
                Err(error) => return Err(error.into()),
            };
            println!("{}", if granted { "granted" } else { "required" });
        }
        [command, file] if command == "identity" => {
            let identity = load_or_create_identity(Path::new(file))?;
            println!("{}", identity.public());
        }
        [command, file, bind, peer] if command == "serve" => {
            let identity = load_identity(Path::new(file))?;
            let host = Host {
                allowed_peer: tokio::sync::watch::channel(Some(peer.parse()?)).1,
            };
            let endpoint = bind_host(identity, bind.parse()?).await?;
            let router = Router::builder(endpoint).accept(ALPN, host).spawn();
            println!("Host ID: {}", router.endpoint().id());
            for address in router.endpoint().bound_sockets() {
                println!("Listening on {address}");
            }
            tokio::signal::ctrl_c().await?;
            router.shutdown().await?;
        }
        [command, file, bind, option, name] if command == "serve-pair" && option == "--app-desktop" => {
            let path = Path::new(file);
            let identity = load_or_create_identity(path)?;
            pairing::serve(identity, bind.parse()?, Some((path, name))).await?;
        }
        [command, file, bind, option] if command == "serve-pair" && option == "--app-desktop" => {
            let path = Path::new(file);
            let identity = load_or_create_identity(path)?;
            pairing::serve(identity, bind.parse()?, Some((path, ""))).await?;
        }
        [command, file, bind] if command == "serve-pair" => {
            let identity = load_or_create_identity(Path::new(file))?;
            pairing::serve(identity, bind.parse()?, None).await?;
        }
        [command, file] if command == "serve-pair" => {
            let identity = load_or_create_identity(Path::new(file))?;
            pairing::serve(identity, ([0, 0, 0, 0], 4444).into(), None).await?;
        }
        _ => return Err("usage: koko-daemon --version | identity <key-file> | serve <key-file> <bind-ip:port> <allowed-peer-id> | serve-pair <key-file> [bind-ip:port] | screen-permission <check|request>".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::SecretKey;
    use koko_p2p::check_host;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn checks_persisted_identity_against_the_host_allowlist() -> Result<(), Box<dyn Error>> {
        let keys = tempfile::tempdir()?;
        let host_file = keys.path().join("host.key");
        let controller_file = keys.path().join("controller.key");
        let host_id = load_or_create_identity(&host_file)?.public();
        let controller_id = load_or_create_identity(&controller_file)?.public();

        let stored_host = load_or_create_identity(&host_file)?;
        assert_eq!(stored_host.public(), host_id);
        let stored_controller = load_identity(&controller_file)?;
        assert_eq!(stored_controller.public(), controller_id);
        assert_ne!(host_id, controller_id);

        let endpoint = bind_host(stored_host, "127.0.0.1:0".parse()?).await?;
        let address = endpoint.bound_sockets()[0];
        let router = Router::builder(endpoint)
            .accept(
                ALPN,
                Host {
                    allowed_peer: tokio::sync::watch::channel(Some(controller_id)).1,
                },
            )
            .spawn();

        let test_timeout = Duration::from_secs(10);
        timeout(
            test_timeout,
            check_host(stored_controller.clone(), host_id, address),
        )
        .await??;
        let denied = timeout(
            test_timeout,
            check_host(SecretKey::generate(), host_id, address),
        )
        .await?
        .expect_err("an unpaired controller must be denied");
        assert!(
            format!("{denied:?}").contains("peer not authorized"),
            "{denied:?}"
        );

        let wrong_host = SecretKey::generate().public();
        let result = timeout(
            test_timeout,
            check_host(stored_controller, wrong_host, address),
        )
        .await?;
        assert!(result.is_err(), "an incorrect host identity must fail");
        router.shutdown().await?;
        Ok(())
    }
}
