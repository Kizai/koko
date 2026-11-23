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
    APPROVAL_TIMEOUT, HumanResult, MAX_REMOTE_REPLY, MAX_REMOTE_REQUEST, PeerKind, RemoteCommand,
    RemoteReply, Scope, VIDEO_ERROR, valid_remote_command,
};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot, watch};

use std::sync::Arc;

use crate::arbiter::Arbiter;
use crate::audit::Audit;
use crate::session::{Sessions, report_end};

pub(crate) struct DesktopRequest {
    command: RemoteCommand,
    reply: oneshot::Sender<RemoteReply>,
    video_socket: Option<String>,
    from: Origin,
}

/// Who sent a desktop request, so the app can apply the agent policy to agents only.
#[derive(Clone, Copy, PartialEq)]
enum Origin {
    Person,
    Agent,
    /// An agent input the person at the host already allowed.
    ApprovedAgent,
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
        self.request(command, None, Origin::Person).await
    }

    async fn request(
        &self,
        command: RemoteCommand,
        video_socket: Option<String>,
        from: Origin,
    ) -> RemoteReply {
        let (reply, response) = oneshot::channel();
        let clipboard = matches!(command, RemoteCommand::Clipboard { .. });
        match tokio::time::timeout(command.desktop_timeout(), async {
            self.0
                .send(DesktopRequest {
                    command,
                    reply,
                    video_socket,
                    from,
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
        if request.from != Origin::Person {
            command["actor"] = "agent".into();
        }
        if request.from == Origin::ApprovedAgent {
            command["approved"] = true.into();
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
    pub(crate) sessions: watch::Sender<Sessions>,
    pub(crate) app_desktop: Option<AppDesktop>,
    pub(crate) arbiter: Arc<Arbiter>,
    pub(crate) audit: Option<Audit>,
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
        let peer = connection.remote_id();
        let authorization = self.sessions.subscribe();
        let Some(scope) = authorization.borrow().scope(peer) else {
            connection.close(1u32.into(), b"peer not authorized");
            return Ok(());
        };
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            let bytes = recv
                .read_to_end(MAX_REMOTE_REQUEST)
                .await
                .map_err(AcceptError::from_err)?;
            if authorization.borrow().scope(peer) != Some(scope) {
                connection.close(1u32.into(), b"peer not authorized");
                return Ok(());
            }
            let command = serde_json::from_slice::<RemoteCommand>(&bytes);
            let audited = match (&self.audit, &command) {
                (Some(_), Ok(command)) if scope != Scope::Human => Some(command.clone()),
                _ => None,
            };
            let mut approved = false;
            let reply = match command {
                Ok(command) if !command.allowed_for(scope) => RemoteReply::error("scope_denied"),
                Ok(command @ RemoteCommand::Video { .. }) => {
                    tokio::select! {
                        biased;
                        _ = revoked(&authorization, peer, scope) => {
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
                    let mut ended = None;
                    self.sessions.send_if_modified(|sessions| {
                        ended = sessions.end(peer);
                        ended.is_some()
                    });
                    if let Some(kind) = ended {
                        report_end(kind);
                        if kind == PeerKind::Agent {
                            self.arbiter.reset();
                        }
                    }
                    RemoteReply::Ok
                }
                Ok(RemoteCommand::ReleaseControl) => {
                    self.arbiter.release();
                    RemoteReply::Ok
                }
                Ok(command @ RemoteCommand::RequestHuman { .. })
                    if valid_remote_command(&command) =>
                {
                    let RemoteCommand::RequestHuman { reason, timeout_s } = command else {
                        unreachable!("matched above")
                    };
                    match self.arbiter.request_help() {
                        Err(code) => RemoteReply::error(code),
                        Ok(answer) => {
                            println!(
                                "Agent help: {}",
                                serde_json::json!({"reason": reason.trim(), "timeout_s": timeout_s})
                            );
                            tokio::select! {
                                biased;
                                _ = revoked(&authorization, peer, scope) => {
                                    self.arbiter.reset();
                                    connection.close(1u32.into(), b"peer not authorized");
                                    return Ok(());
                                }
                                answer = tokio::time::timeout(
                                    Duration::from_secs(timeout_s.into()),
                                    answer,
                                ) => match answer {
                                    Ok(Ok((result, note))) => RemoteReply::Human { result, note },
                                    // The request was withdrawn because the session ended.
                                    Ok(Err(_)) => RemoteReply::error("desktop_unavailable"),
                                    Err(_) => {
                                        self.arbiter.help_timed_out();
                                        RemoteReply::Human {
                                            result: HumanResult::Timeout,
                                            note: None,
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
                Ok(RemoteCommand::Hello) => RemoteReply::Hello {
                    kind: if scope == Scope::Human {
                        PeerKind::Human
                    } else {
                        PeerKind::Agent
                    },
                    scope,
                    control: Some(self.arbiter.control()),
                },
                Ok(command) if valid_remote_command(&command) => 'run: {
                    let agent = scope != Scope::Human;
                    if agent
                        && command.is_agent_input()
                        && let Err(code) = self.arbiter.agent_input()
                    {
                        break 'run RemoteReply::error(code);
                    }
                    if !agent && command.is_human_input() {
                        self.arbiter.human_input();
                    }
                    let Some(app) = &self.app_desktop else {
                        // Without the app there is no policy check, so agents may not send input.
                        if agent && command.is_agent_input() {
                            break 'run RemoteReply::error("desktop_unavailable");
                        }
                        break 'run tokio::task::spawn_blocking(move || execute(command))
                            .await
                            .unwrap_or_else(|_| RemoteReply::error("desktop_unavailable"));
                    };
                    // A person's pointer connection keeps button and drag order. An agent sends each
                    // gesture complete in one request, so it does not take the per-connection lock.
                    if !agent
                        && matches!(command, RemoteCommand::Pointer { .. })
                        && pointer.is_none()
                    {
                        *pointer = self.pointer.try_lock().ok();
                        if pointer.is_none() {
                            break 'run RemoteReply::error("pointer_busy");
                        }
                    }
                    let mut reply = tokio::select! {
                        biased;
                        _ = revoked(&authorization, peer, scope) => {
                            connection.close(1u32.into(), b"peer not authorized");
                            return Ok(());
                        }
                        reply = async {
                            if agent && command.is_agent_input() {
                                self.run_agent_input(app, command).await
                            } else {
                                (app.run(command).await, false)
                            }
                        } => {
                            approved = reply.1;
                            reply.0
                        }
                    };
                    if let RemoteReply::Displays { control, .. } = &mut reply {
                        *control = Some(self.arbiter.control());
                    }
                    reply
                }
                _ => RemoteReply::error("invalid_command"),
            };
            if let (Some(audit), Some(command)) = (&self.audit, &audited) {
                audit.record(peer, command, &reply, approved);
            }
            send.write_all(&serde_json::to_vec(&reply).map_err(AcceptError::from_err)?)
                .await
                .map_err(AcceptError::from_err)?;
            send.finish()?;
        }
        Ok(())
    }

    /// Runs an agent's input through the app's policy. A risky input waits for the person at the
    /// host, then runs once more marked as approved. Returns whether a person approved it.
    async fn run_agent_input(
        &self,
        app: &AppDesktop,
        command: RemoteCommand,
    ) -> (RemoteReply, bool) {
        let reply = app.request(command.clone(), None, Origin::Agent).await;
        let RemoteReply::ApprovalRequired { reason } = reply else {
            return (reply, false);
        };
        let decision = match self.arbiter.request_approval() {
            Ok(decision) => decision,
            Err(code) => return (RemoteReply::error(code), false),
        };
        println!(
            "Agent approval: {}",
            serde_json::json!({ "reason": reason })
        );
        match tokio::time::timeout(APPROVAL_TIMEOUT, decision).await {
            Ok(Ok(true)) => (
                app.request(command, None, Origin::ApprovedAgent).await,
                true,
            ),
            Ok(_) => (RemoteReply::error("approval_declined"), false),
            Err(_) => {
                self.arbiter.approval_timed_out();
                (RemoteReply::error("approval_timeout"), false)
            }
        }
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
            .request(
                command,
                Some(path.to_string_lossy().into_owned()),
                Origin::Person,
            )
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

/// Resolves once this peer no longer holds the session it connected with.
async fn revoked(authorization: &watch::Receiver<Sessions>, peer: EndpointId, scope: Scope) {
    let _ = authorization
        .clone()
        .wait_for(|sessions| sessions.scope(peer) != Some(scope))
        .await;
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
        RemoteCommand::Disconnect
        | RemoteCommand::Hello
        | RemoteCommand::ReleaseControl
        | RemoteCommand::RequestHuman { .. } => {
            unreachable!("session commands are handled by the host")
        }
        RemoteCommand::Screenshot => native(&["screenshot"], None),
        RemoteCommand::Capture { .. }
        | RemoteCommand::UiTree { .. }
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
    use koko_proto::{ControlState, MouseButton, PointerEvent};

    use super::*;

    #[tokio::test]
    async fn forwards_only_authorized_screenshots_to_the_app() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let controller = SecretKey::generate();
        let (sessions, _) = watch::channel(Sessions::human(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: Some(app_desktop),
                    arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60))),
                    audit: None,
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
    async fn an_observer_agent_can_look_but_not_touch() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let person = SecretKey::generate();
        let agent = SecretKey::generate();
        let (sessions, _) = watch::channel(Sessions {
            human: Some(person.public()),
            agent: Some((agent.public(), Scope::Observer)),
        });
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions: sessions.clone(),
                    app_desktop: Some(app_desktop),
                    arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60))),
                    audit: None,
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(agent.clone(), host.clone()).await?;
        let person_client = RemoteClient::connect(person, host).await?;

        let hello = client.send(&RemoteCommand::Hello).await?;
        assert!(matches!(
            hello,
            RemoteReply::Hello {
                kind: PeerKind::Agent,
                scope: Scope::Observer,
                ..
            }
        ));
        let pointer = client
            .send(&RemoteCommand::Pointer {
                display: 1,
                events: vec![PointerEvent::Move { x: 1, y: 1 }],
                source_os: None,
            })
            .await?;
        assert!(matches!(pointer, RemoteReply::Error { code } if code == "scope_denied"));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let capture = RemoteCommand::Capture {
            display: 1,
            edge: 1280,
            quality: 80,
            format: Some(koko_proto::ImageFormat::Png),
            cursor: Some(true),
        };
        let respond = async {
            let request = requests.recv().await.unwrap();
            assert!(matches!(
                request.command,
                RemoteCommand::Capture {
                    format: Some(koko_proto::ImageFormat::Png),
                    ..
                }
            ));
            request.reply.send(RemoteReply::Ok).unwrap();
        };
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(client.send(&capture), respond)
        })
        .await?;
        assert!(matches!(reply?, RemoteReply::Ok));

        // Ending the agent's session leaves the person's session in place.
        assert!(matches!(
            client.send(&RemoteCommand::Disconnect).await?,
            RemoteReply::Ok
        ));
        assert_eq!(sessions.borrow().agent, None);
        assert!(client.send(&RemoteCommand::Hello).await.is_err());
        assert!(matches!(
            person_client.send(&RemoteCommand::Hello).await?,
            RemoteReply::Hello {
                scope: Scope::Human,
                ..
            }
        ));
        client.close().await;
        person_client.close().await;
        router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_person_preempts_an_operating_agent() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let person = SecretKey::generate();
        let agent = SecretKey::generate();
        let (sessions, _) = watch::channel(Sessions {
            human: Some(person.public()),
            agent: Some((agent.public(), Scope::AgentSafe)),
        });
        let arbiter = Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60)));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: Some(app_desktop),
                    arbiter: arbiter.clone(),
                    audit: None,
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let person_client = RemoteClient::connect(person, host.clone()).await?;
        let agent_client = RemoteClient::connect(agent, host).await?;
        let click = |x| RemoteCommand::Pointer {
            display: 1,
            events: vec![PointerEvent::Move { x, y: 10 }],
            source_os: None,
        };
        let answer = async |requests: &mut mpsc::Receiver<DesktopRequest>| {
            requests
                .recv()
                .await
                .unwrap()
                .reply
                .send(RemoteReply::Ok)
                .unwrap();
        };

        tokio::time::timeout(Duration::from_secs(10), async {
            // The person's pointer connection holds the pointer lock; the agent still gets through.
            let command = click(1);
            let (reply, ()) = tokio::join!(person_client.send(&command), answer(&mut requests));
            assert!(matches!(reply?, RemoteReply::Ok));
            let command = click(2);
            let (reply, ()) = tokio::join!(agent_client.send(&command), answer(&mut requests));
            assert!(matches!(reply?, RemoteReply::Ok));
            assert_eq!(arbiter.control(), ControlState::Active);

            let command = click(3);
            let (reply, ()) = tokio::join!(person_client.send(&command), answer(&mut requests));
            assert!(matches!(reply?, RemoteReply::Ok));
            assert_eq!(arbiter.control(), ControlState::Paused);
            let preempted = agent_client.send(&click(4)).await?;
            assert!(
                matches!(preempted, RemoteReply::Error { code } if code == "preempted_by_human")
            );
            assert!(matches!(
                requests.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            let hello = agent_client.send(&RemoteCommand::Hello).await?;
            assert!(matches!(
                hello,
                RemoteReply::Hello {
                    control: Some(ControlState::Paused),
                    ..
                }
            ));
            Ok::<_, Box<dyn Error>>(())
        })
        .await??;
        person_client.close().await;
        agent_client.close().await;
        router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn an_agent_waits_for_a_person_to_answer() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let agent = SecretKey::generate();
        let (sessions, _) = watch::channel(Sessions {
            human: None,
            agent: Some((agent.public(), Scope::AgentSafe)),
        });
        let arbiter = Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60)));
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: Some(AppDesktop::channel().0),
                    arbiter: arbiter.clone(),
                    audit: None,
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(agent, host).await?;
        let request = RemoteCommand::RequestHuman {
            reason: "Please sign in to the bank website".into(),
            timeout_s: 30,
        };
        let person = async {
            while arbiter.control() != ControlState::AwaitingHuman {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            arbiter.answer(HumanResult::Resolved, Some("Done".into()));
        };
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(client.send(&request), person)
        })
        .await?;
        assert!(matches!(
            reply?,
            RemoteReply::Human { result: HumanResult::Resolved, note: Some(note) } if note == "Done"
        ));
        assert_eq!(arbiter.control(), ControlState::Active);
        client.close().await;
        router.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_risky_agent_input_runs_only_after_approval() -> Result<(), Box<dyn Error>> {
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let agent = SecretKey::generate();
        let (sessions, _) = watch::channel(Sessions {
            human: None,
            agent: Some((agent.public(), Scope::AgentSafe)),
        });
        let arbiter = Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60)));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: Some(app_desktop),
                    arbiter: arbiter.clone(),
                    audit: None,
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let client = RemoteClient::connect(agent.clone(), host.clone()).await?;
        let quit = RemoteCommand::Pointer {
            display: 1,
            events: vec![PointerEvent::KeyDown {
                code: 12,
                flags: 0x10,
                repeat: false,
            }],
            source_os: None,
        };

        for allow in [true, false] {
            let app = async {
                let first = requests.recv().await.unwrap();
                assert!(first.from == Origin::Agent);
                first
                    .reply
                    .send(RemoteReply::ApprovalRequired {
                        reason: "Press ⌘Q".into(),
                    })
                    .unwrap();
                while arbiter.control() != ControlState::AwaitingApproval {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                arbiter.decide(allow);
                if allow {
                    let second = requests.recv().await.unwrap();
                    assert!(second.from == Origin::ApprovedAgent);
                    second.reply.send(RemoteReply::Ok).unwrap();
                }
            };
            let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(client.send(&quit), app)
            })
            .await?;
            match (allow, reply?) {
                (true, RemoteReply::Ok) => {}
                (false, RemoteReply::Error { code }) if code == "approval_declined" => {}
                (_, other) => panic!("unexpected reply {other:?}"),
            }
            assert_eq!(arbiter.control(), ControlState::Active);
        }
        client.close().await;
        router.shutdown().await?;

        // Without the app nothing could check the policy, so agent input is refused.
        let endpoint = bind_host(SecretKey::generate(), "127.0.0.1:0".parse()?).await?;
        let host = EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0]);
        let (sessions, _) = watch::channel(Sessions {
            human: None,
            agent: Some((agent.public(), Scope::AgentSafe)),
        });
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: None,
                    arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60))),
                    audit: None,
                    pointer: Mutex::new(()),
                    #[cfg(unix)]
                    video: Mutex::new(()),
                },
            )
            .spawn();
        let reply = send_command(
            agent,
            host,
            &RemoteCommand::Text {
                text: "rm -rf ~\n".into(),
            },
        )
        .await?;
        assert!(matches!(reply, RemoteReply::Error { code } if code == "desktop_unavailable"));
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
        let (sessions, _) = watch::channel(Sessions::human(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions: sessions.clone(),
                    app_desktop: Some(app_desktop),
                    arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60))),
                    audit: None,
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
        sessions.send_replace(Sessions::default());
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
        let (sessions, _) = watch::channel(Sessions::human(controller.public()));
        let (app_desktop, mut requests) = AppDesktop::channel();
        let router = Router::builder(endpoint)
            .accept(
                koko_proto::REMOTE_ALPN,
                RemoteHost {
                    sessions,
                    app_desktop: Some(app_desktop),
                    arbiter: Arc::new(Arbiter::new(Duration::ZERO, Duration::from_secs(60))),
                    audit: None,
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
