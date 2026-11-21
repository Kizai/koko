use std::error::Error;
use std::path::Path;

use koko_p2p::{check_host, load_identity, load_or_create_identity};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, file] if command == "identity" => {
            let identity = load_or_create_identity(Path::new(file))?;
            println!("{}", identity.public());
        }
        [command, file, host, address] if command == "check" => {
            let identity = load_identity(Path::new(file))?;
            check_host(identity, host.parse()?, address.parse()?).await?;
            println!("Authorized by {host}");
        }
        _ => {
            return Err(
                "usage: koko-agent identity <key-file> | check <key-file> <host-id> <host-ip:port>"
                    .into(),
            );
        }
    }
    Ok(())
}
