use koko_host_windows::{DesktopBackend, DesktopRequest, Event, Host, Origin};
use koko_proto::{ImageFormat, RemoteCommand, RemoteDisplay, RemoteReply};
use serde_json::{Value, json};
use std::{ffi::OsStr, io, net::SocketAddr, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const LIMIT: Duration = Duration::from_secs(25);
const DAEMON: &str = env!("CARGO_BIN_EXE_koko-daemon");

// Hand-authored synthetic responses authorized by 0B.1 solely for testing
// the driver protocol. These bytes and display details are not real Mac fixtures.
#[derive(Default)]
struct SyntheticDesktop {
    displays: usize,
    captures: usize,
}
impl DesktopBackend for SyntheticDesktop {
    fn handle(&mut self, request: &DesktopRequest) -> RemoteReply {
        if request.origin != Origin::Person || request.video_socket.is_some() {
            return RemoteReply::error("unexpected_origin");
        }
        match request.command {
            RemoteCommand::Displays => {
                self.displays += 1;
                RemoteReply::Displays {
                    host_name: "Synthetic host".into(),
                    displays: vec![RemoteDisplay {
                        id: 7,
                        name: "Synthetic display".into(),
                        width: 640,
                        height: 480,
                    }],
                    video: false,
                    clipboard: false,
                    clipboard_stream: false,
                    control: None,
                }
            }
            RemoteCommand::Capture {
                display: 7,
                edge: 320,
                quality: 75,
                format: Some(ImageFormat::Png),
                cursor: Some(true),
            } => {
                self.captures += 1;
                RemoteReply::Image {
                    image: "dGVzdA==".into(),
                    width: Some(320),
                    height: Some(240),
                    format: Some(ImageFormat::Png),
                }
            }
            _ => RemoteReply::error("invalid_command"),
        }
    }
}

async fn agent(path: &Path, args: &[&OsStr], input: &str) -> io::Result<Vec<Value>> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let result = timeout(LIMIT, async {
        let mut bytes = Vec::new();
        tokio::try_join!(
            async {
                stdin.write_all(input.as_bytes()).await?;
                drop(stdin);
                Ok::<(), io::Error>(())
            },
            async {
                stdout.read_to_end(&mut bytes).await?;
                Ok(())
            }
        )?;
        if !child.wait().await?.success() {
            return Err(io::Error::other("agent exited unsuccessfully"));
        }
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).map_err(io::Error::from))
            .collect()
    })
    .await;
    if matches!(result, Ok(Ok(_))) {
        return result?;
    }
    if child.try_wait()?.is_none() {
        child.start_kill()?;
        timeout(LIMIT, child.wait()).await??;
    }
    result?
}

async fn event(host: &mut Host) -> io::Result<Event> {
    host.next_event()
        .await?
        .ok_or_else(|| io::Error::other("daemon exited before expected event"))
}

async fn exercise(
    host: &mut Host,
    agent_path: &Path,
    key: &OsStr,
) -> io::Result<(SyntheticDesktop, Vec<Value>)> {
    let mut listening = None;
    let mut device = None;
    let invite = timeout(LIMIT, async {
        loop {
            match event(host).await? {
                Event::Listening(addr) => listening = Some(addr),
                Event::DeviceId(id) => device = Some(id),
                Event::PairInvite(invite) => return Ok::<_, io::Error>(invite),
                _ => {}
            }
        }
    })
    .await??;
    let pair = agent(
        agent_path,
        &[
            "pair".as_ref(),
            key,
            "-".as_ref(),
            "--quiet".as_ref(),
            "--name".as_ref(),
            "0B.1 controller".as_ref(),
        ],
        &format!("{}\n", invite.expose()),
    )
    .await?;
    let [pair] = pair.as_slice() else {
        return Err(io::Error::other("expected a single pairing result"));
    };
    let address = pair["address"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing paired address"))?;
    let host_id = pair["host_id"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing paired host id"))?;
    if pair["status"] != "paired"
        || device.is_none()
        || pair["device_id"].as_str() != device.as_deref()
        || listening.is_none()
        || address.parse::<SocketAddr>().ok() != listening
    {
        return Err(io::Error::other(
            "agent paired result disagrees with daemon startup",
        ));
    }
    timeout(LIMIT, async {
        while !matches!(event(host).await?, Event::PairApproved) {}
        Ok::<_, io::Error>(())
    })
    .await??;
    let args = [
        "remote-stream".as_ref(),
        key,
        host_id.as_ref(),
        address.as_ref(),
    ];
    let mut desktop = SyntheticDesktop::default();
    let input = concat!(
        "{\"action\":\"displays\"}\n",
        "{\"action\":\"capture\",\"display\":7,\"edge\":320,\"quality\":75,\"format\":\"png\",\"cursor\":true}\n"
    );
    let (outputs, dispatched) = tokio::join!(
        agent(agent_path, &args, input),
        timeout(LIMIT, async {
            for _ in 0..2 {
                loop {
                    if let Event::Desktop(request) = event(host).await? {
                        host.dispatch(&mut desktop, &request).await?;
                        break;
                    }
                }
            }
            Ok::<_, io::Error>(())
        })
    );
    dispatched??;
    let outputs = outputs?;
    Ok((desktop, outputs))
}

fn check_outputs(desktop: SyntheticDesktop, outputs: Vec<Value>) {
    assert_eq!((desktop.displays, desktop.captures), (1, 1));
    assert_eq!(outputs.len(), 2);
    assert_eq!(
        outputs[0],
        json!({"status":"displays","host_name":"Synthetic host","displays":[{"id":7,"name":"Synthetic display","width":640,"height":480}],"control":"idle"})
    );
    for (field, expected) in
        json!({"status":"image","image":"dGVzdA==","width":320,"height":240,"format":"png"})
            .as_object()
            .unwrap()
    {
        assert_eq!(&outputs[1][field], expected)
    }
}

#[tokio::test]
async fn real_agent_pairs_and_reads_synthetic_desktop() -> io::Result<()> {
    let agent_path =
        Path::new(DAEMON).with_file_name(format!("koko-agent{}", std::env::consts::EXE_SUFFIX));
    assert!(
        agent_path.is_file(),
        "select both packages: cargo test -p koko-daemon -p koko-agent --locked --tests"
    );
    let dir = tempfile::tempdir()?;
    let mut host = Host::spawn(
        Path::new(DAEMON),
        &dir.path().join("host.key"),
        "127.0.0.1:0".parse().unwrap(),
        "0B.1 test",
    )?;
    let key = dir.path().join("controller.key");
    let result = exercise(&mut host, &agent_path, key.as_os_str()).await;
    assert!(host.shutdown(Duration::from_secs(5)).await?.success());
    let (desktop, outputs) = result?;
    check_outputs(desktop, outputs);
    Ok(())
}
