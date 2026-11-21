use std::error::Error;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use iroh::EndpointAddr;
use koko_p2p::pairing::{discover_hosts, lan_request, request_pairing, resolve_invite};
use koko_p2p::remote::{RemoteClient, send_command};
use koko_p2p::{check_host, load_identity, load_or_create_identity};
use koko_proto::{MAX_REMOTE_REQUEST, RemoteCommand};

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
            let home =
                std::env::var_os("HOME").ok_or("HOME is missing; pass a key file explicitly")?;
            let directory = PathBuf::from(home).join(".koko");
            std::fs::create_dir_all(&directory)?;
            pair(&directory.join("controller.key"), code).await?;
        }
        [command, file, code] if command == "pair" => {
            pair(Path::new(file), code).await?;
        }
        [command] if command == "discover" => {
            let devices: Vec<_> = discover_hosts()
                .await?
                .into_iter()
                .map(|target| {
                    serde_json::json!({
                        "id": target.id.to_string(),
                        "label": format!("Koko {}", target.id.fmt_short()),
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
                "usage: koko-agent --version | identity <key-file> | check <key-file> <host-id> <host-ip:port> | pair [key-file] <code-or-invite> | discover | remote <key-file> <host-id> <host-ip:port>"
                    .into(),
            );
        }
    }
    Ok(())
}

async fn pair(path: &Path, input: &str) -> Result<(), Box<dyn Error>> {
    let key = load_or_create_identity(path)?;
    let (code, target) = resolve_invite(input).await?;
    let host = target.id;
    let address = request_pairing(key, target, code.as_ref()).await?;
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
        let reply = client.as_ref().unwrap().send(&command).await?;
        serde_json::to_writer(&mut output, &reply)?;
        writeln!(output)?;
        output.flush()?;
    }
    if let Some(client) = client {
        client.close().await;
    }
    Ok(())
}
