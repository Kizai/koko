mod input;
mod mcp;
mod registry;
mod tree;

use std::error::Error;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iroh::EndpointAddr;
use koko_p2p::pairing::{
    PairingOptions, discover_hosts, lan_request, request_pairing_with_options, resolve_invite,
};
use koko_p2p::remote::{RemoteClient, send_command};
use koko_p2p::{check_host, load_identity, load_or_create_identity};
use koko_proto::{MAX_REMOTE_REQUEST, MAX_VIDEO_PACKET, RemoteCommand, VIDEO_PATH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [option] if option == "--version" => println!("koko-agent {}", env!("CARGO_PKG_VERSION")),
        [command, file] if command == "identity" => {
            let identity = load_or_create_identity(Path::new(file))?;
            println!("{}", identity.public());
        }
        [command, file, host, address] if command == "check" => {
            let identity = load_identity(Path::new(file))?;
            check_host(identity, host.parse()?, address.parse()?).await?;
            println!("Authorized by {host}");
        }
        [command, code] if command == "pair" => {
            pair(
                &koko_directory()?.join("controller.key"),
                code,
                Default::default(),
            )
            .await?;
        }
        [command, flags @ ..] if command == "mcp" => {
            let mut identity = None;
            let mut name = None;
            let mut flags = flags.iter();
            while let Some(flag) = flags.next() {
                let value = flags
                    .next()
                    .ok_or_else(|| format!("{flag} needs a value"))?;
                match flag.as_str() {
                    "--identity" => identity = Some(PathBuf::from(value)),
                    "--name" => name = Some(value.trim().to_owned()),
                    _ => return Err(format!("unknown mcp option: {flag}").into()),
                }
            }
            let name = name.unwrap_or_else(|| "KoKo Agent".into());
            if name.is_empty()
                || name.chars().count() > koko_proto::MAX_PEER_NAME
                || name.chars().any(char::is_control)
            {
                return Err("--name must be 1-64 printable characters".into());
            }
            let identity = match identity {
                Some(identity) => identity,
                None => koko_directory()?.join("agent.key"),
            };
            let key = load_or_create_identity(&identity)?;
            mcp::serve(key, &identity, name).await?;
        }
        [command, file, code, flags @ ..] if command == "pair" => {
            let mut options = PairingOptions::default();
            for flag in flags {
                match flag.as_str() {
                    "--prefer-lan" => options.prefer_lan = true,
                    "--enhanced" => options.enhanced = true,
                    _ => return Err(format!("unknown pairing option: {flag}").into()),
                }
            }
            pair(Path::new(file), code, options).await?;
        }
        [command] if command == "discover" => {
            let devices: Vec<_> = discover_hosts()
                .await?
                .into_iter()
                .map(|device| {
                    let target = device.target;
                    serde_json::json!({
                        "id": target.id.to_string(),
                        "os": device.os,
                        "label": device.name.unwrap_or_else(|| format!("Koko {}", target.id.fmt_short())),
                        "request": lan_request(&target),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string(&devices)?);
        }
        [command, file, host, address] if command == "remote-stream" => {
            let key = load_identity(Path::new(file))?;
            let target = EndpointAddr::new(host.parse()?).with_ip_addr(address.parse()?);
            stream_commands(key, target).await?;
        }
        [command, file, host, address, request] if command == "remote-video" => {
            let key = load_identity(Path::new(file))?;
            let target = EndpointAddr::new(host.parse()?).with_ip_addr(address.parse()?);
            let request = serde_json::from_str(request)?;
            if !matches!(request, RemoteCommand::Video { .. }) {
                return Err("expected a video subscription".into());
            }
            let (closed, input_closed) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let _ = io::copy(&mut io::stdin(), &mut io::sink());
                let _ = closed.send(());
            });
            let client = RemoteClient::connect(key, target).await?;
            let result = tokio::select! {
                _ = input_closed => Ok(()),
                result = stream_video(&client, &request) => result,
            };
            client.close().await;
            result?;
        }
        [command, file, host, address, transfer, folder] if command == "clipboard-pull" => {
            let request = RemoteCommand::ClipboardPull {
                transfer: transfer.clone(),
            };
            receive_into(file, host, address, &request, folder).await?;
        }
        [command, file, host, address, offer, folder] if command == "outbox-pull" => {
            let request = RemoteCommand::OutboxPull {
                offer: offer.clone(),
            };
            receive_into(file, host, address, &request, folder).await?;
        }
        [command, file, host, address, transfer] if command == "clipboard-push" => {
            #[derive(serde::Deserialize)]
            struct Items {
                text: Option<String>,
                #[serde(default)]
                paths: Vec<PathBuf>,
            }
            let mut input = Vec::new();
            std::io::stdin()
                .take((MAX_REMOTE_REQUEST + 1) as u64)
                .read_to_end(&mut input)?;
            let items: Items = serde_json::from_slice(&input)?;
            let key = load_identity(Path::new(file))?;
            let target = EndpointAddr::new(host.parse()?).with_ip_addr(address.parse()?);
            let client = RemoteClient::connect(key, target).await?;
            let mut report = Progress::default();
            let result = koko_p2p::clipboard::push(
                &client,
                transfer,
                items.text,
                items.paths,
                |done, total| report.print(done, total),
            )
            .await;
            client.close().await;
            print_line(&match result {
                Ok(()) => serde_json::json!({"status": "done"}),
                Err(code) => serde_json::json!({"status": "error", "code": code}),
            });
        }
        [command, file, host, address] if command == "remote" => {
            let key = load_identity(Path::new(file))?;
            let target = EndpointAddr::new(host.parse()?).with_ip_addr(address.parse()?);
            let mut input = Vec::new();
            std::io::stdin()
                .take((MAX_REMOTE_REQUEST + 1) as u64)
                .read_to_end(&mut input)?;
            if input.len() > MAX_REMOTE_REQUEST {
                return Err("remote command is too large".into());
            }
            let command: RemoteCommand = serde_json::from_slice(&input)?;
            let reply = send_command(key, target, &command).await?;
            println!("{}", serde_json::to_string(&reply)?);
        }
        _ => {
            return Err(
                "usage: koko-agent --version | identity <key-file> | check <key-file> <host-id> <host-ip:port> | pair <code-or-invite> | pair <key-file> <code-or-invite> [--prefer-lan] [--enhanced] | discover | mcp [--identity <key-file>] [--name <name>] | remote <key-file> <host-id> <host-ip:port> | clipboard-pull <key-file> <host-id> <host-ip:port> <transfer-id> <folder> | clipboard-push <key-file> <host-id> <host-ip:port> <transfer-id> | outbox-pull <key-file> <host-id> <host-ip:port> <offer-id> <folder> | remote-stream <key-file> <host-id> <host-ip:port> | remote-video <key-file> <host-id> <host-ip:port> <video-request-json>"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Receives copied or offered files into `folder`, printing progress and then the result as JSON.
async fn receive_into(
    file: &str,
    host: &str,
    address: &str,
    request: &RemoteCommand,
    folder: &str,
) -> Result<(), Box<dyn Error>> {
    let key = load_identity(Path::new(file))?;
    let target = EndpointAddr::new(host.parse()?).with_ip_addr(address.parse()?);
    let client = RemoteClient::connect(key, target).await?;
    let mut report = Progress::default();
    let result = koko_p2p::clipboard::pull(&client, request, Path::new(folder), |done, total| {
        report.print(done, total)
    })
    .await;
    client.close().await;
    print_line(&match result {
        Ok(received) => serde_json::json!({
            "status": "done",
            "text": received.text,
            "paths": received.paths,
        }),
        Err(code) => serde_json::json!({"status": "error", "code": code}),
    });
    Ok(())
}

/// Prints transfer progress as JSON lines, at most five times a second, plus the final count.
#[derive(Default)]
struct Progress {
    last: Option<Instant>,
}

impl Progress {
    fn print(&mut self, done: u64, total: u64) {
        if done == total
            || self
                .last
                .is_none_or(|time| time.elapsed() >= Duration::from_millis(200))
        {
            self.last = Some(Instant::now());
            print_line(&serde_json::json!({"progress": done, "total": total}));
        }
    }
}

fn print_line(value: &serde_json::Value) {
    let mut output = io::stdout().lock();
    let _ = writeln!(output, "{value}");
    let _ = output.flush();
}

fn koko_directory() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is missing; pass a key file explicitly")?;
    let directory = PathBuf::from(home).join(".koko");
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

async fn pair(path: &Path, input: &str, options: PairingOptions) -> Result<(), Box<dyn Error>> {
    let key = load_or_create_identity(path)?;
    let (code, target) = resolve_invite(input).await?;
    let host = target.id;
    let address = request_pairing_with_options(key, target, code.as_ref(), options).await?;
    println!(r#"{{"status":"paired","host_id":"{host}","address":"{address}"}}"#);
    Ok(())
}

async fn stream_commands(key: iroh::SecretKey, target: EndpointAddr) -> Result<(), Box<dyn Error>> {
    let mut lines = io::stdin().lock().lines();
    let mut output = io::stdout().lock();
    let mut client = None;
    for line in &mut lines {
        let line = line?;
        if line.len() > MAX_REMOTE_REQUEST {
            return Err("remote command is too large".into());
        }
        let command: RemoteCommand = serde_json::from_str(&line)?;
        if client.is_none() {
            client = Some(RemoteClient::connect(key.clone(), target.clone()).await?);
        }
        let client = client.as_ref().unwrap();
        let reply = client.send(&command).await?;
        let mut reply = serde_json::to_value(reply)?;
        if matches!(command, RemoteCommand::Capture { .. })
            && let Some((rtt, direct)) = client.selected_path()
        {
            reply["rtt_ms"] = rtt.into();
            reply["direct"] = direct.into();
        }
        serde_json::to_writer(&mut output, &reply)?;
        writeln!(output)?;
        output.flush()?;
    }
    if let Some(client) = client {
        client.close().await;
    }
    Ok(())
}

async fn stream_video(
    client: &RemoteClient,
    request: &RemoteCommand,
) -> Result<(), Box<dyn Error>> {
    let mut recv = client.open_command(request).await?;
    let mut output = io::BufWriter::new(io::stdout().lock());
    let mut packet = Vec::new();
    let mut last_stats = None::<Instant>;
    let mut length = [0; 4];
    while recv.read(&mut length[..1]).await?.is_some() {
        recv.read_exact(&mut length[1..]).await?;
        let size = u32::from_be_bytes(length) as usize;
        if !(1..=MAX_VIDEO_PACKET).contains(&size) {
            return Err("invalid video packet size".into());
        }
        packet.resize(size, 0);
        recv.read_exact(&mut packet).await?;
        if last_stats.is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
            && let Some((rtt, direct)) = client.selected_path()
        {
            output.write_all(&10u32.to_be_bytes())?;
            output.write_all(&[VIDEO_PATH])?;
            output.write_all(&rtt.to_be_bytes())?;
            output.write_all(&[u8::from(direct)])?;
            last_stats = Some(Instant::now());
        }
        output.write_all(&length)?;
        output.write_all(&packet)?;
        output.flush()?;
    }
    Ok(())
}
