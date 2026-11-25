use koko_host_windows::{DesktopRequest, Event};
use koko_proto::RemoteReply;
use std::{
    future::{Future, pending},
    io, thread,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, timeout_at},
};

type Job = (DesktopRequest, oneshot::Sender<RemoteReply>);
type Reply = (u64, RemoteReply);

pub(super) struct DesktopWorker {
    jobs: mpsc::Sender<Job>,
    active: Option<Active>,
}

struct Active {
    id: u64,
    deadline: Option<Instant>,
    result: oneshot::Receiver<RemoteReply>,
}

impl DesktopWorker {
    pub(super) fn new(
        mut handle: impl FnMut(&DesktopRequest) -> RemoteReply + Send + 'static,
    ) -> io::Result<Self> {
        let (jobs, mut queued) = mpsc::channel::<Job>(1);
        // A GPU driver or shell call can stall. This thread owns no child-process
        // handles and is independent of Tokio's runtime shutdown.
        thread::Builder::new()
            .name("koko-desktop-capture".into())
            .spawn(move || {
                while let Some((request, result)) = queued.blocking_recv() {
                    let _ = result.send(handle(&request));
                }
            })?;
        Ok(Self { jobs, active: None })
    }

    pub(super) fn submit(&mut self, request: DesktopRequest, limit: Duration) -> Result<(), Reply> {
        let id = request.id;
        if self.active.is_some() {
            return Err((id, RemoteReply::error("capture_busy")));
        }
        let (reply, result) = oneshot::channel();
        let deadline = Some(Instant::now() + limit);
        if self.jobs.try_send((request, reply)).is_err() {
            return Err((id, RemoteReply::error("desktop_unavailable")));
        }
        self.active = Some(Active {
            id,
            deadline,
            result,
        });
        Ok(())
    }

    async fn next_reply(&mut self) -> Option<Reply> {
        let Some(active) = &mut self.active else {
            return pending().await;
        };
        let Some(deadline) = active.deadline else {
            // Keep the slot occupied until the old native call exits. Its
            // completion cannot be delivered as a second or miscorrelated reply.
            let _ = (&mut active.result).await;
            self.active = None;
            return None;
        };
        let id = active.id;
        match timeout_at(deadline, &mut active.result).await {
            Ok(result) => {
                self.active = None;
                let reply = if Instant::now() >= deadline {
                    RemoteReply::error("capture_failed")
                } else {
                    result.unwrap_or_else(|_| RemoteReply::error("capture_failed"))
                };
                Some((id, reply))
            }
            Err(_) => {
                active.deadline = None;
                Some((id, RemoteReply::error("capture_failed")))
            }
        }
    }
}

pub(super) enum Action {
    Event(io::Result<Option<Event>>),
    Input(Option<io::Result<String>>),
    DesktopReply(Option<Reply>),
}

pub(super) async fn next_action(
    event: impl Future<Output = io::Result<Option<Event>>>,
    lines: &mut mpsc::Receiver<io::Result<String>>,
    desktop: &mut DesktopWorker,
) -> Action {
    tokio::select! {
        input = lines.recv() => Action::Input(input),
        reply = desktop.next_reply() => Action::DesktopReply(reply),
        event = event => Action::Event(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koko_host_windows::Origin;
    use koko_proto::RemoteCommand;

    #[tokio::test]
    async fn stalled_capture_times_out_without_holding_console_eof() {
        let (release, blocked) = std::sync::mpsc::channel();
        let (entered, started) = oneshot::channel();
        let mut entered = Some(entered);
        let (last_entered, last_started) = oneshot::channel();
        let mut last_entered = Some(last_entered);
        let mut worker = DesktopWorker::new(move |request| {
            let entered = match request.id {
                42 => entered.take(),
                45 => last_entered.take(),
                _ => None,
            };
            if let Some(entered) = entered {
                let _ = entered.send(());
                let _ = blocked.recv();
            }
            RemoteReply::Ok
        })
        .unwrap();
        let request = |id| DesktopRequest {
            id,
            command: RemoteCommand::Screenshot,
            origin: Origin::Person,
            video_socket: None,
        };
        let short = Duration::from_millis(500);
        worker.submit(request(42), short).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started)
            .await
            .unwrap()
            .unwrap();

        let (input, mut lines) = mpsc::channel(1);
        assert!(matches!(
            next_action(
                std::future::ready(Ok(Some(Event::PairApproved))),
                &mut lines,
                &mut worker
            )
            .await,
            Action::Event(Ok(Some(Event::PairApproved)))
        ));
        input.send(Ok("approve 7".into())).await.unwrap();
        assert!(matches!(
            next_action(std::future::pending(), &mut lines, &mut worker).await,
            Action::Input(Some(Ok(line))) if line == "approve 7"
        ));
        assert!(matches!(
            next_action(std::future::pending(), &mut lines, &mut worker).await,
            Action::DesktopReply(Some((42, RemoteReply::Error { code }))) if code == "capture_failed"
        ));
        assert!(matches!(
            worker.submit(request(43), short),
            Err((43, RemoteReply::Error { code })) if code == "capture_busy"
        ));

        drop(input);
        assert!(matches!(
            tokio::time::timeout(
                Duration::from_secs(1),
                next_action(std::future::pending(), &mut lines, &mut worker)
            )
            .await
            .unwrap(),
            Action::Input(None)
        ));
        release.send(()).unwrap();
        assert!(worker.next_reply().await.is_none());

        worker.submit(request(44), Duration::from_secs(1)).unwrap();
        assert!(matches!(
            worker.next_reply().await,
            Some((44, RemoteReply::Ok))
        ));

        worker.submit(request(45), Duration::from_secs(1)).unwrap();
        tokio::time::timeout(Duration::from_secs(2), last_started)
            .await
            .unwrap()
            .unwrap();
        // Dropping the worker on console exit must not join a native call.
        let (dropped, shutdown) = oneshot::channel();
        thread::spawn(move || {
            drop(worker);
            let _ = dropped.send(());
        });
        tokio::time::timeout(Duration::from_secs(1), shutdown)
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
    }
}
