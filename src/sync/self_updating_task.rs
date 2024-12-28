use crate::sync::common::{InnerTaskState, JoinError};
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

/// Executes a closure with a given frequency where the closure also apply changes to the polling rate.
///
/// When [`SelfUpdatingPollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best-effort clean exit.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid-execution will still occur.
pub struct SelfUpdatingPollingTask {
    inner_state: Arc<InnerTaskState>,
    background_thread: Option<JoinHandle<()>>,
    shutdown_rx: Receiver<()>,
}

impl SelfUpdatingPollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new<F>(timeout: Option<Duration>, mut interval: Duration, task: F) -> Self
    where
        F: Fn(&mut Duration) + Send + 'static,
    {
        let shared_state = InnerTaskState::new(timeout);
        let shared_state_clone = shared_state.clone();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                Self::poll_task_forever(&mut interval, &shared_state_clone, task, shutdown_tx)
            })),
            shutdown_rx,
        }
    }

    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll. This closure gets access to another function that can assert if the managed task is still active.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn new_with_checker<F>(timeout: Option<Duration>, mut interval: Duration, task: F) -> Self
    where
        F: Fn(&mut Duration, &dyn Fn() -> bool) + Send + 'static,
    {
        let shared_state = InnerTaskState::new(timeout);
        let shared_state_clone = shared_state.clone();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                let shared_state_checker_clone = shared_state_clone.clone();

                let checker = move || shared_state_checker_clone.active.lock().unwrap().to_owned();

                Self::poll_task_forever(
                    &mut interval,
                    &shared_state_clone,
                    move |interval: &mut Duration| task(interval, &checker),
                    shutdown_tx,
                )
            })),
            shutdown_rx,
        }
    }

    fn poll_task_forever<F>(
        interval: &mut Duration,
        inner_state: &Arc<InnerTaskState>,
        task: F,
        shutdown_tx: Sender<()>,
    ) where
        F: Fn(&mut Duration) + Send + 'static,
    {
        loop {
            task(interval);

            let result = inner_state
                .signal
                .wait_timeout_while(
                    inner_state.active.lock().unwrap(),
                    *interval,
                    |&mut active| active,
                )
                .unwrap();

            if !result.1.timed_out() {
                break;
            }
        }

        // If the parent thread is down, there's nothing to report back to. Channel shutdown errors
        // can be ignored.
        let _ = shutdown_tx.send(());
    }

    pub fn join(mut self) -> Result<(), JoinError> {
        self.join_impl()
    }

    fn join_impl(&mut self) -> Result<(), JoinError> {
        if let Some(handle) = self.background_thread.take() {
            return self.inner_state.join_sync(&self.shutdown_rx);
        }

        Ok(())
    }
}

impl Drop for SelfUpdatingPollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        self.join_impl().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SelfUpdatingPollingTask;
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
            None,
            Duration::from_millis(0),
            move |interval: &mut Duration| {
                counter_clone.fetch_add(1, SeqCst);
                *interval = Duration::from_secs(5000);
                if let Some(tx) = tx.lock().unwrap().take() {
                    tx.send(true).unwrap();
                }
            },
        );

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
                None,
                Duration::from_millis(0),
                move |interval: &mut Duration, checker: &dyn Fn() -> bool| {
                    tx.lock().unwrap().take().unwrap().send(true).unwrap();

                    loop {
                        if !checker() {
                            break;
                        }
                    }

                    // Prevent issues caused by cycling a second time.
                    *interval = Duration::from_secs(5000);
                    tx_exit.lock().unwrap().take().unwrap().send(true).unwrap();
                },
            );

            // Guarantee we polled at least once
            rx.await.unwrap();
        }

        // Ensure the long poll exits by signal, not by test going out of scope.
        rx_exit.await.unwrap();
    }
}
