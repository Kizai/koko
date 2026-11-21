use std::error::Error;
use std::path::{Path, PathBuf};

use koko_p2p::pairing::{request_pairing, resolve_invite};
use koko_p2p::{check_host, load_identity, load_or_create_identity};

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
        _ => {
            return Err(
                "usage: koko-agent --version | identity <key-file> | check <key-file> <host-id> <host-ip:port> | pair [key-file] <code-or-invite>"
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
    let address = request_pairing(key, target, &code).await?;
    println!(r#"{{"status":"paired","host_id":"{host}","address":"{address}"}}"#);
    Ok(())
}
