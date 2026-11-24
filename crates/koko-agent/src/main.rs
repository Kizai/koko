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
    HumanPairing, PairingOptions, VerificationCode, device_id, discover_hosts, find_device,
    lan_request, parse_device_id, request_human_pairing, resolve_invite,
};
use koko_p2p::remote::{RemoteClient, send_command};
use koko_p2p::{check_host, load_identity, load_or_create_identity};
use koko_proto::{
    MAX_REMOTE_REQUEST, MAX_VIDEO_PACKET, RemoteCommand, VIDEO_PATH, feedback_length,
};

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
                PairArgs::default(),
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
        [command, file, target, flags @ ..] if command == "pair" => {
            let mut args = PairArgs::default();
            // Secrets come on stdin, one per line, so other processes cannot read them from
            // the command line: first the target when it is `-`, then the code for `--code-stdin`.
            let mut secrets = std::io::stdin().lines();
            let mut secret = |what: &str| -> Result<String, Box<dyn Error>> {
                let line = secrets
                    .next()
                    .ok_or(format!("stdin ended before the {what}"))??;
                Ok(line.trim().to_owned())
            };
            let target = if target == "-" {
                secret("pairing target")?
            } else {
                target.clone()
            };
            let mut flags = flags.iter();
            while let Some(flag) = flags.next() {
                match flag.as_str() {
                    "--prefer-lan" => args.options.prefer_lan = true,
                    "--enhanced" => args.options.enhanced = true,
                    "--quiet" => args.quiet = true,
                    "--code" => {
                        let value = flags.next().ok_or("--code needs a value")?;
                        args.code = Some(value.parse()?);
                    }
                    "--code-stdin" => args.code = Some(secret("verification code")?.parse()?),
                    "--name" => {
                        let value = flags.next().ok_or("--name needs a value")?;
                        args.name = value.trim().to_owned();
                    }
                    "--known" => {
                        let value = flags.next().ok_or("--known needs host_id/ip:port")?;
                        args.known = Some(parse_known(value)?);
                    }
                    _ => return Err(format!("unknown pairing option: {flag}").into()),
                }
            }
            pair(Path::new(file), &target, args).await?;
        }
        [command] if command == "discover" => {
            let devices: Vec<_> = discover_hosts()
                .await?
                .into_iter()
                .map(|device| {
                    let target = device.target;
                    serde_json::json!({
                        "id": target.id.to_string(),
                        "device_id": device_id(target.id),
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
            // The app writes feedback records to stdin and closes it to end the stream.
            let (feedback, feedback_records) = tokio::sync::mpsc::channel(64);
            std::thread::spawn(move || {
                read_feedback(&mut io::stdin().lock(), &feedback);
                let _ = io::copy(&mut io::stdin(), &mut io::sink());
                let _ = closed.send(());
            });
            let client = RemoteClient::connect(key, target).await?;
            let result = tokio::select! {
                _ = input_closed => Ok(()),
                result = stream_video(&client, &request, feedback_records) => result,
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

struct PairArgs {
    options: PairingOptions,
    code: Option<VerificationCode>,
    name: String,
    quiet: bool,
    known: Option<EndpointAddr>,
}

impl Default for PairArgs {
    fn default() -> Self {
        Self {
            options: PairingOptions::default(),
            code: None,
            name: "KoKo".into(),
            quiet: false,
            known: None,
        }
    }
}

fn parse_known(value: &str) -> Result<EndpointAddr, Box<dyn Error>> {
    let (host, addresses) = value.split_once('/').ok_or("invalid --known target")?;
    let mut target = EndpointAddr::new(host.parse()?);
    for address in addresses.split(',').filter(|item| !item.is_empty()) {
        target = target.with_ip_addr(address.parse()?);
    }
    Ok(target)
}

async fn pair(path: &Path, input: &str, args: PairArgs) -> Result<(), Box<dyn Error>> {
    let key = load_or_create_identity(path)?;
    let (invite_code, target, name, os) = if input.trim().starts_with("koko://") {
        let (code, target) = resolve_invite(input)?;
        (code, target, None, None)
    } else if let Some(id) = parse_device_id(input) {
        let discovered = find_device(&id, args.known).await?;
        (None, discovered.target, discovered.name, discovered.os)
    } else {
        return Err("expected a 9-digit device ID or KoKo direct invite".into());
    };
    let host = target.id;
    let code = args.code.or(invite_code);
    let pairing = HumanPairing {
        name: &args.name,
        code: code.as_ref(),
        quiet: args.quiet,
    };
    let address = request_human_pairing(key, target, pairing, args.options).await?;
    println!(
        "{}",
        serde_json::json!({
            "status": "paired",
            "host_id": host.to_string(),
            "device_id": device_id(host),
            "address": address.to_string(),
            "name": name,
            "os": os,
        })
    );
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

/// Passes whole feedback records from the app on, until stdin closes or holds something else.
/// A record is dropped rather than queued when the stream is behind; the next ack replaces it.
fn read_feedback(input: &mut impl Read, records: &tokio::sync::mpsc::Sender<Vec<u8>>) {
    loop {
        let mut record = vec![0u8; 5];
        if input.read_exact(&mut record).is_err() {
            return;
        }
        let length = u32::from_be_bytes([record[0], record[1], record[2], record[3]]);
        if feedback_length(record[4]) != Some(length) {
            return;
        }
        record.resize(4 + length as usize, 0);
        if input.read_exact(&mut record[5..]).is_err() {
            return;
        }
        let _ = records.try_send(record);
    }
}

async fn stream_video(
    client: &RemoteClient,
    request: &RemoteCommand,
    mut feedback: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn Error>> {
    let want_audio = matches!(request, RemoteCommand::Video { audio: true, .. });
    let video_recv = if matches!(request, RemoteCommand::Video { feedback: true, .. }) {
        // The request stream stays open so acknowledgements reach the host on it.
        let (mut send, recv) = client.open_stream(request).await?;
        tokio::spawn(async move {
            while let Some(record) = feedback.recv().await {
                if send.write_all(&record).await.is_err() {
                    return;
                }
            }
        });
        recv
    } else {
        client.open_command(request).await?
    };
    let audio_recv = if want_audio {
        client.open_command(&RemoteCommand::Audio).await.ok()
    } else {
        None
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let video_tx = tx.clone();
    let video_task =
        tokio::spawn(async move { read_media_records(video_recv, video_tx, false).await });
    let audio_task = audio_recv.map(|recv| {
        let audio_tx = tx.clone();
        tokio::spawn(async move {
            let _ = read_media_records(recv, audio_tx, true).await;
        })
    });
    drop(tx);

    let mut output = io::BufWriter::new(io::stdout().lock());
    let mut last_stats = None::<Instant>;
    while let Some(frame) = rx.recv().await {
        if last_stats.is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
            && let Some((rtt, direct)) = client.selected_path()
        {
            output.write_all(&10u32.to_be_bytes())?;
            output.write_all(&[VIDEO_PATH])?;
            output.write_all(&rtt.to_be_bytes())?;
            output.write_all(&[u8::from(direct)])?;
            last_stats = Some(Instant::now());
        }
        output.write_all(&frame)?;
        output.flush()?;
    }
    if let Some(task) = audio_task {
        task.abort();
    }
    video_task.await??;
    Ok(())
}

async fn read_media_records(
    mut recv: iroh::endpoint::RecvStream,
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ignore_error_record: bool,
) -> Result<(), String> {
    let mut length = [0u8; 4];
    while recv
        .read(&mut length[..1])
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        recv.read_exact(&mut length[1..])
            .await
            .map_err(|e| e.to_string())?;
        let size = u32::from_be_bytes(length) as usize;
        if !(1..=MAX_VIDEO_PACKET).contains(&size) {
            return Err("invalid media packet size".into());
        }
        let mut frame = vec![0u8; 4 + size];
        frame[..4].copy_from_slice(&length);
        recv.read_exact(&mut frame[4..])
            .await
            .map_err(|e| e.to_string())?;
        if ignore_error_record && frame[4] == koko_proto::VIDEO_ERROR {
            return Ok(());
        }
        if tx.send(frame).await.is_err() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use koko_proto::{FEEDBACK_ACK, FEEDBACK_KEY_FRAME};

    #[test]
    fn feedback_is_forwarded_as_whole_records_until_one_is_malformed() {
        let mut input = Vec::new();
        input.extend_from_slice(&5u32.to_be_bytes());
        input.push(FEEDBACK_ACK);
        input.extend_from_slice(&7u32.to_be_bytes());
        input.extend_from_slice(&1u32.to_be_bytes());
        input.push(FEEDBACK_KEY_FRAME);
        input.extend_from_slice(&1u32.to_be_bytes());
        input.push(0xff);
        input.extend_from_slice(&1u32.to_be_bytes());
        input.push(FEEDBACK_KEY_FRAME);
        let (records, mut received) = tokio::sync::mpsc::channel(8);
        read_feedback(&mut input.as_slice(), &records);
        assert_eq!(received.try_recv().unwrap(), input[..9]);
        assert_eq!(received.try_recv().unwrap(), input[9..14]);
        assert!(received.try_recv().is_err());
    }
}
