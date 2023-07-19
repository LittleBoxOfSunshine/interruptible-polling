use std::num::TryFromIntError;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::Duration;
use crate::task::{new_task, PollingTaskInnerState, wait_with_timeout};

/// Executes a closure with a given frequency where the closure also apply changes to the polling rate.
///
/// When [`SelfUpdatingPollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid execution will still occur.
pub struct SelfUpdatingPollingTask {
    shared_state: Arc<PollingTaskInnerState>,
}

/// Alias for the callback that allows the poll operation to apply the new polling rate back into
/// the [`SelfUpdatingPollingTask`]
pub type PollingIntervalSetter = dyn Fn(Duration) -> Result<(), TryFromIntError>;

impl SelfUpdatingPollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new(interval: Duration, task: Box<dyn Fn(&PollingIntervalSetter) + Send>) -> Result<Self, TryFromIntError> {
        new_task!(Self, interval, task)
    }

    fn poll(shared_state: &Arc<PollingTaskInnerState>, task: &Box<dyn Fn(&PollingIntervalSetter) + Send>) {
        let copy = shared_state.clone();
        let setter = move |duration: Duration| {
            copy.interval.store(u64::try_from(duration.as_millis())?, Relaxed);
            Ok(())
        };

        wait_with_timeout! (shared_state, (task)(&setter));
    }
}

impl Drop for SelfUpdatingPollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::SeqCst;
    use super::*;

    #[tokio::test]
    async fn update_observed_on_next_poll_with_early_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));

        let _task = SelfUpdatingPollingTask::new(Duration::from_millis(0), Box::new(move |setter: &PollingIntervalSetter| {
            counter_clone.fetch_add(1, SeqCst);
            setter(Duration::from_secs(5000)).unwrap();
            tx.lock().unwrap().take().unwrap().send(true).unwrap();
        })).unwrap();

        rx.await.unwrap();
        assert_eq!(counter.load(SeqCst), 1);
    }
}
