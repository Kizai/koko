#![cfg(windows)]

use koko_host_windows::{Event, Host};
use std::{io, path::PathBuf, time::Duration};
use tokio::{process::Command, time::timeout};

const LIMIT: Duration = Duration::from_secs(25);
const DAEMON: &str = env!("CARGO_BIN_EXE_koko-daemon");

#[tokio::test]
async fn windows_host_launches_daemon_by_path_and_by_name() -> io::Result<()> {
    const FIXTURE_DIR: &str = "KOKO_DAEMON_PATH_TEST";
    const HELPER: &str = "koko-stage-1-path-helper";
    if let Some(directory) = std::env::var_os(FIXTURE_DIR).map(PathBuf::from) {
        for executable in [PathBuf::from(HELPER), directory.join(HELPER)] {
            let mut host = Host::spawn(
                &executable,
                &directory.join("host key"),
                "127.0.0.1:0".parse().unwrap(),
                "PATH test host",
            )?;
            assert!(matches!(
                timeout(LIMIT, host.next_event()).await??,
                Some(Event::Listening(_))
            ));
            assert!(host.shutdown(Duration::from_secs(5)).await?.success());
        }
        return Ok(());
    }
    let dir = tempfile::Builder::new()
        .prefix("Koko PATH test ")
        .tempdir()?;
    std::fs::copy(DAEMON, dir.path().join(format!("{HELPER}.exe")))?;
    // The isolated test runner owns its environment. The helper executable
    // lives only in its PATH directory, separate from the runner and its CWD.
    let output = timeout(
        LIMIT,
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "windows_host_launches_daemon_by_path_and_by_name",
                "--nocapture",
            ])
            .env(FIXTURE_DIR, dir.path())
            .env(
                "PATH",
                std::env::join_paths([dir.path()]).map_err(io::Error::other)?,
            )
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
