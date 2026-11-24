//! Subscriptions belong to one scheduler run. Callbacks run outside the registry lock.
use super::views;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy)]
pub enum SchedulerEvent<'a> {
    Feed(&'a views::FeedUpdated),
    Status(&'a views::SchedulerSnapshot),
    VectorStale(&'a views::VectorSpaceStatusView),
}

type Listener = dyn Fn(SchedulerEvent<'_>) + Send + Sync;

#[derive(Default)]
pub(crate) struct Subscribers {
    inner: Mutex<Listeners>,
}

#[derive(Default)]
struct Listeners {
    next: u64,
    callbacks: BTreeMap<u64, Arc<Listener>>,
}

#[must_use = "dropping the subscription disconnects the listener"]
pub struct Subscription {
    subscribers: Weak<Subscribers>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(subscribers) = self.subscribers.upgrade() {
            let removed = subscribers.inner.lock().unwrap().callbacks.remove(&self.id);
            drop(removed);
        }
    }
}

impl Subscribers {
    pub(super) fn subscribe(self: &Arc<Self>, callback: Arc<Listener>) -> Subscription {
        let mut listeners = self.inner.lock().unwrap();
        listeners.next += 1;
        let id = listeners.next;
        listeners.callbacks.insert(id, callback);
        Subscription {
            subscribers: Arc::downgrade(self),
            id,
        }
    }

    pub(crate) fn publish(&self, event: SchedulerEvent<'_>) {
        let callbacks: Vec<_> = self
            .inner
            .lock()
            .unwrap()
            .callbacks
            .values()
            .cloned()
            .collect();
        for callback in callbacks {
            callback(event);
        }
    }

    pub(crate) fn has_listeners(&self) -> bool {
        !self.inner.lock().unwrap().callbacks.is_empty()
    }
}
