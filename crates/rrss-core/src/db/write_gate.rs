//! FIFO admission for pooled explicit writes. SQLite still owns cross-process locking.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    next: u64,
    active: bool,
    queue: VecDeque<u64>,
}

#[derive(Default)]
pub(super) struct WriteGate {
    state: Mutex<State>,
    changed: Condvar,
}

pub(super) struct Permit(Arc<WriteGate>);

impl WriteGate {
    pub fn acquire(self: &Arc<Self>, timeout: Duration) -> rusqlite::Result<Permit> {
        let started = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = state.next;
        state.next = state.next.wrapping_add(1);
        state.queue.push_back(ticket);
        loop {
            if !state.active && state.queue.front() == Some(&ticket) {
                state.queue.pop_front();
                state.active = true;
                return Ok(Permit(self.clone()));
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                state.queue.retain(|waiting| *waiting != ticket);
                self.changed.notify_all();
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    None,
                ));
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active = false;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_writer_precedes_reacquisition_and_expired_waiter_is_removed() {
        let gate = Arc::new(WriteGate::default());
        let first = gate.acquire(Duration::ZERO).unwrap();
        assert!(gate.acquire(Duration::ZERO).is_err());
        let next = gate.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = next.acquire(Duration::from_secs(3)).unwrap();
            send.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while gate.state.lock().unwrap().queue.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(first);
        let _last = gate.acquire(Duration::from_secs(3)).unwrap();
        receive.try_recv().unwrap();
        worker.join().unwrap();
    }
}
