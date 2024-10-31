use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use crate::common::InnerTaskState;

/// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
/// rate is retrieved from the given closure on every iteration.
///
/// When [`VariablePollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best-effort clean exit.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid-execution will still occur.
pub struct VariablePollingTask {
    inner_state: Arc<InnerTaskState>,
    background_thread: Option<JoinHandle<()>>,
}

impl VariablePollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new<D, F>(interval_fetcher: D, task: F) -> Self
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn() + Send + 'static,
    {
        let shared_state = InnerTaskState::new();
        let shared_state_clone = shared_state.clone();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                Self::poll_task_forever(&interval_fetcher, &shared_state_clone, task)
            })),
        }
    }

    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll. This closure gets access to another function that can assert if the managed task is still active.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn new_with_checker<D, F>(interval_fetcher: D, task: F) -> Self
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let shared_state = InnerTaskState::new();
        let shared_state_clone = shared_state.clone();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                let shared_state_checker_clone = shared_state_clone.clone();

                let checker = move || shared_state_checker_clone.active.lock().unwrap().to_owned();

                Self::poll_task_forever(
                    &interval_fetcher,
                    &shared_state_clone,
                    move || task(&checker),
                )
            })),
        }
    }

    fn poll_task_forever<D, F>(interval_fetcher: &D, inner_state: &Arc<InnerTaskState>, task: F)
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn() + Send + 'static,
    {
        loop {
            task();

            let result = inner_state
                .signal
                .wait_timeout_while(
                    inner_state.active.lock().unwrap(),
                    interval_fetcher(),
                    |&mut active| active,
                )
                .unwrap();

            if !result.1.timed_out() {
                break;
            }
        }
    }
}

impl Drop for VariablePollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        *self.inner_state.active.lock().unwrap() = false;
        self.inner_state.signal.notify_one();
        self.background_thread.take().unwrap().join().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VariablePollingTask;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn update_observed_on_next_poll_with_early_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));

        let counter_clone_2 = counter.clone();
        let increases_on_second_call = move || {
            if counter_clone_2.load(SeqCst) == 0 {
                Duration::from_millis(0)
            }
            else {
                Duration::from_secs(5000)
            }
        };

        let _task = VariablePollingTask::new(
            increases_on_second_call,
            move || {
                counter_clone.fetch_add(1, SeqCst);
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
            let _task = VariablePollingTask::new_with_checker(
                // Prevent issues caused by cycling a second time.
                || Duration::from_secs(5000),
                move |checker: &dyn Fn() -> bool| {
                    tx.lock().unwrap().take().unwrap().send(true).unwrap();

                    loop {
                        if !checker() {
                            break;
                        }
                    }

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
