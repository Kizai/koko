use std::collections::HashMap;
use std::time::Duration;
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt};

use iroh::{
    EndpointId,
    endpoint::{Connection, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use koko_proto::{
    MAX_REMOTE_REPLY, MAX_REMOTE_REQUEST, RemoteCommand, RemoteReply, VIDEO_ERROR,
    valid_remote_command,
};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot, watch};

pub(crate) struct DesktopRequest {
    command: RemoteCommand,
    reply: oneshot::Sender<RemoteReply>,
    video_socket: Option<String>,
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
        self.request(command, None).await
    }

    async fn request(&self, command: RemoteCommand, video_socket: Option<String>) -> RemoteReply {
        let (reply, response) = oneshot::channel();
        let clipboard = matches!(command, RemoteCommand::Clipboard { .. });
        match tokio::time::timeout(command.desktop_timeout(), async {
            self.0
                .send(DesktopRequest {
                    command,
                    reply,
                    video_socket,
                })
                .await
                .ok()?;
            response.await.ok()
        })
        .await
        {
            Ok(Some(reply)) => reply,
            Err(_) if clipboard => RemoteReply::error("clipboard_timeout"),
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
        let mut command =
            serde_json::to_value(&request.command).expect("desktop command is serializable");
        if let Some(socket) = request.video_socket {
            command["video_socket"] = socket.into();
        }
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
    #[cfg(unix)]
    pub(crate) video: Mutex<()>,
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
        let authorization = self.allowed_peer.subscribe();
        if *authorization.borrow() != Some(connection.remote_id()) {
            connection.close(1u32.into(), b"peer not authorized");
            return Ok(());
        }
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            let bytes = recv
                .read_to_end(MAX_REMOTE_REQUEST)
                .await
                .map_err(AcceptError::from_err)?;
            if authorization.has_changed().unwrap_or(true)
                || *authorization.borrow() != Some(connection.remote_id())
            {
                connection.close(1u32.into(), b"peer not authorized");
                return Ok(());
            }
            let command = serde_json::from_slice::<RemoteCommand>(&bytes);
            let reply = match command {
                Ok(command @ RemoteCommand::Video { .. }) => {
                    let mut authorization = authorization.clone();
                    tokio::select! {
                        biased;
                        _ = authorization.changed() => {
                            connection.close(1u32.into(), b"peer not authorized");
                            return Ok(());
                        }
                        _ = send.stopped() => return Ok(()),
                        result = self.serve_video(command, &mut send) => result?,
                    }
                    send.finish()?;
                    continue;
                }
                Ok(RemoteCommand::Disconnect) => {
                    self.allowed_peer.send_if_modified(|peer| {
                        if *peer != Some(connection.remote_id()) {
                            return false;
                        }
                        println!("Session closed");
                        *peer = None;
                        true
                    });
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
                            let mut access = authorization.clone();
                            tokio::select! {
                                biased;
                                _ = access.changed() => {
                                    connection.close(1u32.into(), b"peer not authorized");
                                    return Ok(());
                                }
                                reply = app.run(command) => reply,
                            }
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

    #[cfg(unix)]
    async fn serve_video(
        &self,
        command: RemoteCommand,
        send: &mut SendStream,
    ) -> Result<(), AcceptError> {
        if !valid_remote_command(&command) {
            return send_video_error(send, "invalid_command").await;
        }
        let Some(app) = &self.app_desktop else {
            return send_video_error(send, "desktop_unavailable").await;
        };
        let Ok(_active) = self.video.try_lock() else {
            return send_video_error(send, "video_busy").await;
        };
        let directory = tempfile::Builder::new()
            .prefix("kv")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .map_err(AcceptError::from_err)?;
        let path = directory.path().join("v");
        let listener = UnixListener::bind(&path).map_err(AcceptError::from_err)?;
        if let RemoteReply::Error { code } = app
            .request(command, Some(path.to_string_lossy().into_owned()))
            .await
        {
            return send_video_error(send, &code).await;
        }
        let (mut source, _) = tokio::time::timeout(Duration::from_secs(9), listener.accept())
            .await
            .map_err(AcceptError::from_err)?
            .map_err(AcceptError::from_err)?;
        tokio::io::copy(&mut source, send)
            .await
            .map_err(AcceptError::from_err)?;
        Ok(())
    }

    #[cfg(not(unix))]
    async fn serve_video(
        &self,
        _command: RemoteCommand,
        send: &mut SendStream,
    ) -> Result<(), AcceptError> {
        send_video_error(send, "desktop_unavailable").await
    }
}

async fn send_video_error(send: &mut SendStream, code: &str) -> Result<(), AcceptError> {
    let size = (code.len() as u32 + 1).to_be_bytes();
    send.write_all(&size).await.map_err(AcceptError::from_err)?;
    send.write_all(&[VIDEO_ERROR])
        .await
        .map_err(AcceptError::from_err)?;
    send.write_all(code.as_bytes())
        .await
        .map_err(AcceptError::from_err)
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
        RemoteCommand::Capture { .. }
        | RemoteCommand::Displays
        | RemoteCommand::Clipboard { .. }
        | RemoteCommand::Pointer { .. }
        | RemoteCommand::Video { .. } => {
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
                    #[cfg(unix)]
                    video: Mutex::new(()),
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
    async fn clipboard_chunk_requires_current_session_authorization() -> Result<(), Box<dyn Error>>
    {
        use koko_proto::{ClipboardOp, ClipboardPacket};

        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let controller = SecretKey::generate();
        let (allowed_peer, _) = watch::channel(Some(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    allowed_peer: allowed_peer.clone(),
                    app_desktop: Some(app_desktop),
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(controller, host).await?;
        let command = RemoteCommand::Clipboard {
            transfer: "01234567-89ab-cdef-0123-456789abcdef".into(),
            op: ClipboardOp::Write,
            packet: Some(ClipboardPacket::Data {
                bytes: "/".repeat(87380) + "/w==",
            }),
        };
        let respond = async {
            let request = requests.recv().await.unwrap();
            let RemoteCommand::Clipboard {
                packet: Some(ClipboardPacket::Data { bytes }),
                ..
            } = request.command
            else {
                panic!("expected a file chunk");
            };
            assert_eq!(bytes.len(), 87384);
            request.reply.send(RemoteReply::Ok).unwrap();
        };
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(client.send(&command), respond)
        })
        .await?;
        assert!(matches!(reply?, RemoteReply::Ok));
        allowed_peer.send_replace(None);
        assert!(client.send(&command).await.is_err());
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        client.close().await;
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
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(controller, host).await?;
        let respond = async {
            for kind in ["down", "move", "up"] {
                let request = requests.recv().await.unwrap();
                let RemoteCommand::Pointer {
                    display, events, ..
                } = request.command
                else {
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
                        source_os: None,
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
