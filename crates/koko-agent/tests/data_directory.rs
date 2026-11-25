use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rmcp::model::ProtocolVersion;
use serde_json::json;

#[cfg(not(windows))]
const HOME_VARIABLE: &str = "HOME";
#[cfg(windows)]
const HOME_VARIABLE: &str = "USERPROFILE";

fn agent(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_koko-agent"));
    command
        .current_dir(directory)
        .env("HOME", directory.join("Unix home"))
        .env("USERPROFILE", directory.join("Windows profile"));
    command
}

fn expected_data_directory(directory: &Path) -> PathBuf {
    let home = if cfg!(windows) {
        "Windows profile"
    } else {
        "Unix home"
    };
    directory.join(home).join(".koko")
}

fn run_mcp(command: &mut Command) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": ProtocolVersion::LATEST_WITH_INITIALIZE,
            "capabilities": {},
            "clientInfo": {"name": "data-directory-test", "version": "1"}
        }
    });
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    writeln!(child.stdin.take().unwrap(), "{initialize}\n{initialized}")?;
    child.wait_with_output()
}

#[test]
fn default_pair_creates_controller_key_in_data_directory() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let output = agent(directory.path()).args(["pair", "invalid"]).output()?;
    let stderr = String::from_utf8(output.stderr)?;

    // Pairing loads the key before rejecting the target, so no network access is needed.
    assert!(!output.status.success());
    assert!(stderr.contains("expected a 9-digit device ID"), "{stderr}");
    koko_p2p::load_identity(&expected_data_directory(directory.path()).join("controller.key"))?;
    Ok(())
}

#[test]
fn default_mcp_creates_agent_key_in_data_directory() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let output = run_mcp(agent(directory.path()).arg("mcp"))?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "{stderr}");
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(reply["result"]["serverInfo"].is_object(), "{reply}");
    koko_p2p::load_identity(&expected_data_directory(directory.path()).join("agent.key"))?;
    Ok(())
}

#[test]
fn explicit_mcp_identity_works_without_a_home_directory() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let key = directory.path().join("explicit.key");
    let output = run_mcp(
        agent(directory.path())
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .args(["mcp", "--identity"])
            .arg(&key),
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8(output.stderr)?
    );
    koko_p2p::load_identity(&key)?;
    Ok(())
}

#[test]
fn missing_home_directory_reports_the_platform_variable() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let output = agent(directory.path())
        .env_remove(HOME_VARIABLE)
        .args(["pair", "invalid"])
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;

    assert!(!output.status.success());
    assert!(
        stderr.contains(&format!(
            "{HOME_VARIABLE} is missing; pass a key file explicitly"
        )),
        "{stderr}"
    );
    Ok(())
}
