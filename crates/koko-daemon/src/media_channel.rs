#[cfg(windows)]
mod windows;

use std::io;
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt};

#[cfg(unix)]
use tokio::net::{
    UnixListener,
    unix::{OwnedReadHalf, OwnedWriteHalf},
};

pub(crate) struct MediaChannel {
    pub(crate) name: String,
    #[cfg(windows)]
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(unix)]
    listener: UnixListener,
    // Keep the private directory and its socket alive until the stream ends.
    #[cfg(unix)]
    _directory: tempfile::TempDir,
}

#[cfg(unix)]
impl MediaChannel {
    pub(crate) fn bind(is_audio: bool) -> io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("kv")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let path = directory.path().join(if is_audio { "a" } else { "v" });
        let listener = UnixListener::bind(&path)?;
        Ok(Self {
            name: path.to_string_lossy().into_owned(),
            listener,
            _directory: directory,
        })
    }

    pub(crate) async fn accept(&mut self) -> io::Result<(OwnedReadHalf, OwnedWriteHalf)> {
        Ok(self.listener.accept().await?.0.into_split())
    }
}
