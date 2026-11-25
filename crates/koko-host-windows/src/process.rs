use crate::{Command, DesktopRequest, Event, parse_frame};
use koko_proto::RemoteReply;
use std::{
    io,
    net::SocketAddr,
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command as Process},
    sync::mpsc,
    time::timeout,
};

/// Implementations must apply the local input policy for requests from an agent.
pub trait DesktopBackend {
    fn handle(&mut self, request: &DesktopRequest) -> RemoteReply;
}

pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<io::Result<Event>>,
}
impl Host {
    pub fn spawn(executable: &Path, key: &Path, bind: SocketAddr, name: &str) -> io::Result<Self> {
        let mut child = Process::new(executable)
            .arg("serve-pair")
            .arg(key)
            .arg(bind.to_string())
            .arg("--app-desktop")
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (tx, events) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                let event = match stdout.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => parse_frame(&line),
                    Err(error) => Err(error),
                };
                let failed = event.is_err();
                // During shutdown the receiver closes first. Continue draining stdout
                // until the child exits, even if it emits a final status line.
                let _ = tx.send(event).await;
                if failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            events,
        })
    }
    pub async fn next_event(&mut self) -> io::Result<Option<Event>> {
        self.events.recv().await.transpose()
    }
    pub async fn send(&mut self, command: Command<'_>) -> io::Result<()> {
        let frame = command.encode()?;
        // An interrupted write can leave half a frame in the pipe. Closing that
        // pipe prevents the next command from being appended to the partial one.
        let mut stdin = self.stdin.take().ok_or(io::ErrorKind::BrokenPipe)?;
        stdin.write_all(frame.as_bytes()).await?;
        stdin.flush().await?;
        self.stdin = Some(stdin);
        Ok(())
    }
    /// Serial dispatch. A slow backend delays the caller's next event. Callers
    /// may instead handle requests off-thread and send their correlated replies.
    pub async fn dispatch(
        &mut self,
        backend: &mut impl DesktopBackend,
        request: &DesktopRequest,
    ) -> io::Result<()> {
        self.send(Command::DesktopReply(request.id, &backend.handle(request)))
            .await
    }
    pub async fn shutdown(mut self, grace: Duration) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        self.events.close();
        if let Ok(status) = timeout(grace, self.child.wait()).await {
            return status;
        }
        self.child.start_kill()?;
        timeout(grace, self.child.wait()).await?
    }
}
