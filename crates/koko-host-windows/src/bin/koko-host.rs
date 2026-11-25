#[cfg(any(windows, test))]
#[path = "koko_host/desktop_worker.rs"]
mod desktop_worker;

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("koko-host requires an interactive Windows desktop");
    std::process::ExitCode::FAILURE
}

#[cfg(any(windows, test))]
fn pair_decision(line: &str, pending: Option<u64>) -> Option<bool> {
    let mut fields = line.split_whitespace();
    let (Some(action), Some(id), None) = (fields.next(), fields.next(), fields.next()) else {
        return None;
    };
    if Some(id.parse::<u64>().ok()?) != pending {
        return None;
    }
    match action {
        "approve" => Some(true),
        "decline" => Some(false),
        _ => None,
    }
}

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    console::run().await
}

#[cfg(windows)]
mod console {
    use super::{
        desktop_worker::{Action, DesktopWorker, next_action},
        pair_decision,
    };
    use koko_host_windows::{Command, DesktopBackend, Event, Host, WindowsDesktopBackend};
    use koko_proto::{RemoteCommand, RemoteReply};
    use std::{env, io, path::Path, thread, time::Duration};
    use tokio::sync::mpsc;

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = env::args().skip(1).collect();
        const USAGE: &str = "usage: koko-host <key-file> <bind-ip:port> <device-name>";
        if args == ["--help"] {
            println!("{USAGE}");
            return Ok(());
        }
        let [key, bind, name] = args.as_slice() else {
            return Err(USAGE.into());
        };
        let bind = bind.parse()?;
        let mut backend = WindowsDesktopBackend::new(name.clone())?;
        let mut desktop = DesktopWorker::new(move |request| backend.handle(request))?;
        let daemon = env::current_exe()?.with_file_name("koko-daemon.exe");
        let (sender, mut lines) = mpsc::channel(1);
        // A detached OS thread lets Ctrl+C stop the host even while stdin is idle.
        thread::Builder::new()
            .name("koko-console-input".into())
            .spawn(move || {
                for line in io::stdin().lines() {
                    if sender.blocking_send(line).is_err() {
                        break;
                    }
                }
            })?;
        let mut host = Host::spawn(&daemon, Path::new(key), bind, name)?;
        println!("Starting KoKo host {name:?} (PID {})", std::process::id());
        let result = tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            result = serve(&mut host, &mut desktop, &mut lines) => result,
        };
        drop(desktop);
        let status = host.shutdown(Duration::from_secs(3)).await;
        result?;
        let status = status?;
        if !status.success() {
            return Err(format!("koko-daemon exited with {status}").into());
        }
        Ok(())
    }

    async fn serve(
        host: &mut Host,
        desktop: &mut DesktopWorker,
        lines: &mut mpsc::Receiver<io::Result<String>>,
    ) -> io::Result<()> {
        let mut pending = None;
        // The daemon expires this class of requests at nine seconds. Leave time
        // to serialize and deliver the reply over the local pipe.
        const DESKTOP_LIMIT: Duration = Duration::from_secs(7);
        loop {
            match next_action(host.next_event(), lines, desktop).await {
                Action::Event(event) => match event? {
                    Some(Event::Listening(address)) => println!("Listening on {address}"),
                    Some(Event::DeviceId(id)) => println!("Device ID: {id}"),
                    Some(Event::PairCode(code)) => println!("Pair code: {}", code.expose()),
                    Some(Event::PairInvite(invite)) => println!("Pair invite: {}", invite.expose()),
                    Some(Event::PairRequest { id, peer, details }) => {
                        pending = Some(id);
                        // Quoted debug rendering keeps an untrusted peer name on one line.
                        println!(
                            "Pair request {id}: {:?} from {peer}, {:?}, scope {}",
                            details.name,
                            details.kind,
                            details.scope.as_str()
                        );
                        println!("Enter approve {id} or decline {id}");
                    }
                    Some(Event::PairApproved | Event::AgentSession { .. }) => {
                        pending = None;
                        println!("Pair approved");
                    }
                    Some(Event::PairDeclined | Event::PairRequestEnded) => {
                        pending = None;
                        println!("Pair request closed");
                    }
                    Some(Event::PairCodeLocked) => println!("Pair code locked"),
                    Some(Event::Desktop(request)) => {
                        let result = match request.command {
                            RemoteCommand::Displays
                            | RemoteCommand::Screenshot
                            | RemoteCommand::Capture { .. }
                            | RemoteCommand::Wallpaper => desktop.submit(request, DESKTOP_LIMIT),
                            _ => Err((request.id, RemoteReply::error("desktop_unavailable"))),
                        };
                        if let Err((id, reply)) = result {
                            host.send(Command::DesktopReply(id, &reply)).await?;
                        }
                    }
                    Some(_) => {}
                    None => return Ok(()),
                },
                Action::DesktopReply(Some((id, reply))) => {
                    host.send(Command::DesktopReply(id, &reply)).await?;
                }
                Action::DesktopReply(None) => {}
                Action::Input(input) => {
                    let Some(input) = input else { return Ok(()) };
                    let input = input?;
                    match pair_decision(&input, pending) {
                        Some(approved) => {
                            let id = pending.take().expect("validated pending request");
                            let command = if approved {
                                Command::Approve(id, None)
                            } else {
                                Command::Decline(id)
                            };
                            host.send(command).await?;
                        }
                        None => println!(
                            "Use approve <request-id> or decline <request-id> for the current request"
                        ),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ties_console_approval_to_the_shown_request() {
        assert_eq!(pair_decision("approve 12", Some(12)), Some(true));
        assert_eq!(pair_decision("decline 12", Some(12)), Some(false));
        assert_eq!(pair_decision("approve 12", Some(13)), None);
        assert_eq!(pair_decision("approve 12", None), None);
        assert_eq!(pair_decision("approve", Some(12)), None);
        assert_eq!(pair_decision("approve 12 extra", Some(12)), None);
    }
}
