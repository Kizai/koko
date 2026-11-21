use std::collections::HashMap;
use std::time::Duration;

use iroh::{
    EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use koko_proto::{
    MAX_REMOTE_REPLY, MAX_REMOTE_REQUEST, RemoteCommand, RemoteReply, valid_remote_command,
};
use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot, watch};

pub(crate) struct DesktopRequest {
    command: RemoteCommand,
    reply: oneshot::Sender<RemoteReply>,
}

#[derive(Debug)]
pub(crate) struct AppDesktop(mpsc::Sender<DesktopRequest>);

#[derive(Default)]
pub(crate) struct AppDesktopRequests {
    number: u64,
    pending: HashMap<u64, oneshot::Sender<RemoteReply>>,
}

impl AppDesktop {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<DesktopRequest>) {
        let (send, receive) = mpsc::channel(8);
        (Self(send), receive)
    }

    async fn run(&self, command: RemoteCommand) -> RemoteReply {
        let (reply, response) = oneshot::channel();
        match tokio::time::timeout(Duration::from_secs(9), async {
            self.0.send(DesktopRequest { command, reply }).await.ok()?;
            response.await.ok()
        })
        .await
        {
            Ok(Some(reply)) => reply,
            _ => RemoteReply::error("desktop_unavailable"),
        }
    }
}

impl AppDesktopRequests {
    pub(crate) fn request(&mut self, request: DesktopRequest) {
        self.pending.retain(|_, reply| !reply.is_closed());
        if request.reply.is_closed() {
            return;
        }
        self.number += 1;
        let command =
            serde_json::to_string(&request.command).expect("desktop command is serializable");
        println!("Desktop request: {} {command}", self.number);
        self.pending.insert(self.number, request.reply);
    }

    pub(crate) fn reply(&mut self, line: &str) {
        let Some((number, json)) = line.split_once(' ') else {
            return;
        };
        let Ok(number) = number.parse() else {
            return;
        };
        if let Some(request) = self.pending.remove(&number) {
            let reply = if json.len() <= MAX_REMOTE_REPLY {
                serde_json::from_str(json)
                    .unwrap_or_else(|_| RemoteReply::error("desktop_unavailable"))
            } else {
                RemoteReply::error("capture_failed")
            };
            let _ = request.send(reply);
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemoteHost {
    pub(crate) allowed_peer: watch::Sender<Option<EndpointId>>,
    pub(crate) app_desktop: Option<AppDesktop>,
    pub(crate) pointer: Mutex<()>,
}

impl ProtocolHandler for RemoteHost {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let mut pointer = None;
        let result = self.serve(&connection, &mut pointer).await;
        if pointer.is_some() {
            println!("Pointer closed");
        }
        result
    }
}

impl RemoteHost {
    async fn serve<'a>(
        &'a self,
        connection: &Connection,
        pointer: &mut Option<MutexGuard<'a, ()>>,
    ) -> Result<(), AcceptError> {
        if *self.allowed_peer.borrow() != Some(connection.remote_id()) {
            connection.close(1u32.into(), b"peer not authorized");
            return Ok(());
        }
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            let bytes = recv
                .read_to_end(MAX_REMOTE_REQUEST)
                .await
                .map_err(AcceptError::from_err)?;
            if *self.allowed_peer.borrow() != Some(connection.remote_id()) {
                connection.close(1u32.into(), b"peer not authorized");
                return Ok(());
            }
            let command = serde_json::from_slice::<RemoteCommand>(&bytes);
            let reply = match command {
                Ok(RemoteCommand::Disconnect) => {
                    self.allowed_peer.send_replace(None);
                    println!("Session closed");
                    RemoteReply::Ok
                }
                Ok(command) if valid_remote_command(&command) => {
                    if let Some(app) = &self.app_desktop {
                        if matches!(command, RemoteCommand::Pointer { .. }) && pointer.is_none() {
                            *pointer = self.pointer.try_lock().ok();
                        }
                        if matches!(command, RemoteCommand::Pointer { .. }) && pointer.is_none() {
                            RemoteReply::error("pointer_busy")
                        } else {
                            app.run(command).await
                        }
                    } else {
                        tokio::task::spawn_blocking(move || execute(command))
                            .await
                            .unwrap_or_else(|_| RemoteReply::error("desktop_unavailable"))
                    }
                }
                _ => RemoteReply::error("invalid_command"),
            };
            send.write_all(&serde_json::to_vec(&reply).map_err(AcceptError::from_err)?)
                .await
                .map_err(AcceptError::from_err)?;
            send.finish()?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn permission(kind: &str, request: bool) -> std::io::Result<bool> {
    let response = native(&[kind, if request { "request" } else { "check" }], None)?;
    match (response.status.success(), response.stdout.as_slice()) {
        (true, b"granted\n") => Ok(true),
        (true, b"required\n") => Ok(false),
        _ => Err(std::io::Error::other("could not check macOS permission")),
    }
}

#[cfg(target_os = "macos")]
fn execute(command: RemoteCommand) -> RemoteReply {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let screenshot = matches!(command, RemoteCommand::Screenshot);
    let result = match command {
        RemoteCommand::Disconnect => unreachable!("disconnect is handled by the session"),
        RemoteCommand::Screenshot => native(&["screenshot"], None),
        RemoteCommand::Capture { .. } | RemoteCommand::Displays | RemoteCommand::Pointer { .. } => {
            return RemoteReply::error("desktop_unavailable");
        }
        RemoteCommand::Click { x, y } => native(&["click", &x.to_string(), &y.to_string()], None),
        RemoteCommand::Key { key } => native(&["key", key.as_str()], None),
        RemoteCommand::Text { text } => native(&["text"], Some(text.as_bytes())),
    };
    match result {
        Ok(result)
            if result.status.success()
                && screenshot
                && result.stdout.starts_with(b"\x89PNG\r\n\x1a\n") =>
        {
            RemoteReply::Frame {
                png: STANDARD.encode(result.stdout),
            }
        }
        Ok(result) if result.status.success() && !screenshot => RemoteReply::Ok,
        Ok(result) => {
            let error = String::from_utf8_lossy(&result.stderr);
            if error
                .lines()
                .any(|line| line == "screen_permission_required")
            {
                RemoteReply::error("screen_permission_required")
            } else if error
                .lines()
                .any(|line| line == "input_permission_required")
            {
                RemoteReply::error("input_permission_required")
            } else {
                RemoteReply::error(if screenshot {
                    "capture_failed"
                } else {
                    "input_failed"
                })
            }
        }
        Err(_) => RemoteReply::error("desktop_unavailable"),
    }
}

#[cfg(not(target_os = "macos"))]
fn execute(_command: RemoteCommand) -> RemoteReply {
    RemoteReply::error("desktop_unavailable")
}

#[cfg(target_os = "macos")]
fn native(arguments: &[&str], input: Option<&[u8]>) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let executable = std::env::current_exe()?.with_file_name("koko-desktop");
    let mut command = Command::new(executable);
    command.args(arguments);
    if let Some(input) = input {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let write_result = child.stdin.take().unwrap().write_all(input);
        let output = child.wait_with_output()?;
        if output.status.success() {
            write_result?;
        }
        Ok(output)
    } else {
        command.stdin(Stdio::null()).output()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use iroh::{EndpointAddr, SecretKey, protocol::Router};
    use koko_p2p::{
        bind_host,
        remote::{RemoteClient, send_command},
    };
    use koko_proto::{MouseButton, PointerEvent};

    use super::*;

    #[tokio::test]
    async fn forwards_only_authorized_screenshots_to_the_app() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let controller = SecretKey::generate();
        let (allowed_peer, _) = watch::channel(Some(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    allowed_peer,
                    app_desktop: Some(app_desktop),
                    pointer: Mutex::new(()),
                },
            )
            .spawn();

        let denied = send_command(
            SecretKey::generate(),
            host.clone(),
            &RemoteCommand::Screenshot,
        )
        .await
        .expect_err("unpaired requests must be rejected");
        assert!(format!("{denied:?}").contains("peer not authorized"));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let respond = async {
            let mut app = AppDesktopRequests::default();
            app.request(requests.recv().await.unwrap());
            app.reply(r#"0 {"status":"error","code":"capture_failed"}"#);
            app.reply(r#"1 {"status":"error","code":"screen_permission_required"}"#);
        };
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                send_command(controller, host, &RemoteCommand::Screenshot),
                respond
            )
        })
        .await?;
        assert!(
            matches!(reply?, RemoteReply::Error { code } if code == "screen_permission_required")
        );
        router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_persistent_pointer_connection_preserves_button_order() -> Result<(), Box<dyn Error>>
    {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let controller = SecretKey::generate();
        let (allowed_peer, _) = watch::channel(Some(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    allowed_peer,
                    app_desktop: Some(app_desktop),
                    pointer: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(controller, host).await?;
        let respond = async {
            for kind in ["down", "move", "up"] {
                let request = requests.recv().await.unwrap();
                let RemoteCommand::Pointer { display, events } = request.command else {
                    panic!("expected a pointer request");
                };
                assert_eq!(display, 2);
                assert_eq!(serde_json::to_value(events).unwrap()[0]["kind"], kind);
                request.reply.send(RemoteReply::Ok).unwrap();
            }
        };
        let send = async {
            for event in [
                PointerEvent::Down {
                    x: 100,
                    y: 200,
                    button: MouseButton::Left,
                    clicks: 2,
                },
                PointerEvent::Move { x: 200, y: 250 },
                PointerEvent::Up {
                    x: 200,
                    y: 250,
                    button: MouseButton::Left,
                },
            ] {
                let reply = client
                    .send(&RemoteCommand::Pointer {
                        display: 2,
                        events: vec![event],
                    })
                    .await?;
                assert!(matches!(reply, RemoteReply::Ok));
            }
            Ok::<_, Box<dyn Error>>(())
        };
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(send, respond)
        })
        .await?;
        reply?;
        client.close().await;
        router.shutdown().await?;
        Ok(())
    }
}
