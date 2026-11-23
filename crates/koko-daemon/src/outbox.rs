use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use koko_proto::OutboxOffer;
use tokio::sync::Notify;

/// The most batches waiting at once; older ones are dropped first.
const MAX_OFFERS: usize = 20;

/// Files the person at the host dropped to send to the controller. Controllers learn about
/// them by waiting on this queue, because only controllers open connections.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    offers: Mutex<Vec<(OutboxOffer, Vec<PathBuf>)>>,
    changed: Notify,
    next: Mutex<u64>,
}

impl Outbox {
    pub(crate) fn add(&self, paths: Vec<PathBuf>) -> String {
        let id = {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            format!("o{next}")
        };
        let names = paths
            .iter()
            .map(|path| {
                path.file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            })
            .collect();
        let mut offers = self.offers.lock().unwrap();
        if offers.len() == MAX_OFFERS {
            offers.remove(0);
        }
        offers.push((
            OutboxOffer {
                id: id.clone(),
                names,
            },
            paths,
        ));
        drop(offers);
        self.changed.notify_waiters();
        id
    }

    /// Lists waiting batches, waiting up to `limit` for one to arrive.
    pub(crate) async fn wait(&self, limit: Duration) -> Vec<OutboxOffer> {
        let changed = self.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let current = self.list();
        if !current.is_empty() {
            return current;
        }
        let _ = tokio::time::timeout(limit, changed).await;
        self.list()
    }

    fn list(&self) -> Vec<OutboxOffer> {
        let offers = self.offers.lock().unwrap();
        offers.iter().map(|(offer, _)| offer.clone()).collect()
    }

    /// Hands a batch to the one controller that pulls it.
    pub(crate) fn take(&self, id: &str) -> Option<Vec<PathBuf>> {
        let mut offers = self.offers.lock().unwrap();
        let index = offers.iter().position(|(offer, _)| offer.id == id)?;
        Some(offers.remove(index).1)
    }

    /// Batches belong to the session they were dropped in.
    pub(crate) fn clear(&self) {
        self.offers.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_waiting_controller_hears_about_new_files_at_once() {
        let outbox = std::sync::Arc::new(Outbox::default());
        let waiting = tokio::spawn({
            let outbox = outbox.clone();
            async move { outbox.wait(Duration::from_secs(10)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let id = outbox.add(vec![PathBuf::from("/tmp/Report.pdf")]);
        let offers = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(offers[0].names, ["Report.pdf"]);
        assert_eq!(
            outbox.take(&id),
            Some(vec![PathBuf::from("/tmp/Report.pdf")])
        );
        assert_eq!(outbox.take(&id), None, "a batch is pulled once");
        assert!(outbox.wait(Duration::from_millis(10)).await.is_empty());
    }
}
