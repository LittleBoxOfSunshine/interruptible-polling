use crate::task::{new_task, wait_with_timeout, PollingTaskInnerState, StillActiveChecker, Task};
use std::num::TryFromIntError;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

/// Executes a closure with a given frequency where the closure also apply changes to the polling rate.
///
/// When [`SelfUpdatingPollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best effort clean exit.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid execution will still occur.
pub struct SelfUpdatingPollingTask {
    shared_state: Arc<PollingTaskInnerState>,
    background_thread: Option<JoinHandle<()>>,
}

/// Alias for the callback that allows the poll operation to apply the new polling rate back into
/// the [`SelfUpdatingPollingTask`]
pub type PollingIntervalSetter = dyn Fn(Duration) -> Result<(), TryFromIntError>;

macro_rules! new_interval_setter {
    ($shared_state:ident) => {{
        let copy = $shared_state.clone();
        let setter = move |duration: Duration| {
            copy.interval
                .store(u64::try_from(duration.as_millis())?, Relaxed);
            Ok(())
        };

        setter
    }};
}

impl SelfUpdatingPollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new(
        interval: Duration,
        task: Box<dyn Fn(&PollingIntervalSetter) + Send>,
    ) -> Result<Self, TryFromIntError> {
        let shared_state = PollingTaskInnerState::new(interval)?;
        let setter = new_interval_setter!(shared_state);
        let task = Box::new(move || (task)(&setter));

        let task = Task::Unit(task);
        new_task!(Self, shared_state, task)
    }

    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    /// * `task` The closure to execute at every poll. This closure gets access to another function that can assert if the managed task is still active.
    ///
    /// If your task is long running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn new_with_checker(
        interval: Duration,
        task: Box<dyn Fn(&PollingIntervalSetter, &StillActiveChecker) + Send>,
    ) -> Result<Self, TryFromIntError> {
        let shared_state = PollingTaskInnerState::new(interval)?;
        let setter = new_interval_setter!(shared_state);
        let task = Box::new(move |checker: &StillActiveChecker| (task)(&setter, checker));

        let task = Task::WithChecker(task);
        new_task!(Self, shared_state, task)
    }

    fn poll(shared_state: &Arc<PollingTaskInnerState>, task: &Box<dyn Fn() + Send>) {
        wait_with_timeout!(shared_state, task);
    }
}

impl Drop for SelfUpdatingPollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        crate::task::drop_task!(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn update_observed_on_next_poll_with_early_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));

        let _task = SelfUpdatingPollingTask::new(
            Duration::from_millis(0),
            Box::new(move |setter: &PollingIntervalSetter| {
                counter_clone.fetch_add(1, SeqCst);
                setter(Duration::from_secs(5000)).unwrap();
                if let Some(tx) = tx.lock().unwrap().take() {
                    tx.send(true).unwrap();
                }
            }),
        )
        .unwrap();

        rx.await.unwrap();
        assert_eq!(counter.load(SeqCst), 1);
    }

    #[tokio::test]
    async fn slow_poll_exits_early() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
        let tx_exit = Mutex::new(Some(tx_exit));

        {
            let _task = SelfUpdatingPollingTask::new_with_checker(
                Duration::from_millis(0),
                Box::new(
                    move |setter: &PollingIntervalSetter, checker: &StillActiveChecker| {
                        tx.lock().unwrap().take().unwrap().send(true).unwrap();

                        loop {
                            if !checker() {
                                break;
                            }
                        }

                        // Prevent issues cause by cycling second time.
                        setter(Duration::from_secs(5000)).unwrap();
                        tx_exit.lock().unwrap().take().unwrap().send(true).unwrap();
                    },
                ),
            )
            .unwrap();

            // Guarantee we polled at least once
            rx.await.unwrap();
        }

        // Ensure the long poll exits by signal, not by test going out of scope.
        rx_exit.await.unwrap();
    }
}
