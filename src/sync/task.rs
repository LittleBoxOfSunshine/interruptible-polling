use crate::sync::SelfUpdatingPollingTask;
use std::num::TryFromIntError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use crate::sync::common::JoinError;

/// General purpose RAII polling task that executes a closure with a given frequency.
///
/// When [`PollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best-effort clean exit.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid-execution will still occur.
pub struct PollingTask {
    inner_task: SelfUpdatingPollingTask,
    interval: Arc<AtomicU64>,
}

impl PollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new<F>(timeout: Option<Duration>, interval: Duration, task: F) -> Result<Self, TryFromIntError>
    where
        F: Fn() + Send + 'static,
    {
        let (shared_interval, shared_interval_clone) = Self::new_intervals(interval)?;

        let wrapped_task = move |interval: &mut Duration| {
            task();
            Self::apply_new_interval_if_exists(interval, &shared_interval_clone)
        };

        Ok(Self {
            inner_task: SelfUpdatingPollingTask::new(timeout, interval, wrapped_task),
            interval: shared_interval,
        })
    }

    fn new_intervals(
        interval: Duration,
    ) -> Result<(Arc<AtomicU64>, Arc<AtomicU64>), TryFromIntError> {
        let shared_interval = Arc::new(AtomicU64::new(u64::try_from(interval.as_millis())?));
        let shared_interval_clone = shared_interval.clone();

        Ok((shared_interval, shared_interval_clone))
    }

    fn apply_new_interval_if_exists(inner_interval: &mut Duration, task_interval: &Arc<AtomicU64>) {
        let task_interval = Duration::from_millis(task_interval.load(Ordering::Relaxed));

        // If the interval doesn't match that of the inner task, a set call was made since we
        // last polled and the new value needs to be propagated.
        if *inner_interval != task_interval {
            *inner_interval = task_interval;
        }
    }

    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    /// * `task` The closure to execute at every poll. This closure gets access to another function that can assert if the managed task is still active.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn new_with_checker<F>(timeout: Option<Duration>, interval: Duration, task: F) -> Result<Self, TryFromIntError>
    where
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let (shared_interval, shared_interval_clone) = Self::new_intervals(interval)?;

        let wrapped_task =
            move |interval: &mut Duration, still_alive_checker: &dyn Fn() -> bool| {
                task(still_alive_checker);
                Self::apply_new_interval_if_exists(interval, &shared_interval_clone)
            };

        Ok(Self {
            inner_task: SelfUpdatingPollingTask::new_with_checker(timeout, interval, wrapped_task),
            interval: shared_interval,
        })
    }

    pub fn join(&mut self) -> Result<(), JoinError> {
        self.inner_task.join()
    }

    /// Update the delay between poll events. Applied on the next iteration.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as an u64 in milliseconds.
    pub fn set_polling_rate(&self, interval: Duration) -> Result<(), TryFromIntError> {
        self.interval
            .store(u64::try_from(interval.as_millis())?, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Mutex;
    use std::thread::sleep;

    #[test]
    fn poll() {
        let (counter, _task) = get_task_and_timer();
        sleep(Duration::from_millis(50));

        assert!(counter.load(SeqCst) > 1);
    }

    fn get_task_and_timer() -> (Arc<AtomicU64>, PollingTask) {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        let task = PollingTask::new(
            None,
            Duration::from_millis(1),
            Box::new(move || {
                counter_clone.fetch_add(1, SeqCst);
            }),
        );
        (counter, task.unwrap())
    }

    #[test]
    fn set_polling_rate() {
        let (counter, task) = get_task_and_timer();

        // The sleeps here are not ideal, but given how simple the test is and small the crate is
        // in practice this sloppiness doesn't matter.

        // Wait for task to actually run
        sleep(Duration::from_millis(50));

        task.set_polling_rate(Duration::from_secs(5)).unwrap();

        // Wait for effect
        sleep(Duration::from_millis(50));

        // Observe no change in counter
        let counter_value = counter.load(SeqCst);
        sleep(Duration::from_millis(50));
        assert_eq!(counter_value, counter.load(SeqCst))
    }

    #[test]
    // This is implicitly covered through the set time tests having very large wait periods, but
    // keeping a dedicated test for clarity and to make the validation explicit.
    fn early_clean_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        let _task = PollingTask::new(
            None,
            Duration::from_secs(5000),
            Box::new(move || {
                counter_clone.fetch_add(1, SeqCst);
            }),
        )
        .unwrap();
        sleep(Duration::from_millis(50));

        assert_eq!(1, counter.load(SeqCst))
    }

    #[test]
    fn drop_while_running_blocks() {
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let start_tx = Mutex::new(Some(start_tx));
        let stop_tx = Mutex::new(Some(stop_tx));

        {
            let _task = PollingTask::new(
                None,
                Duration::from_secs(5000),
                Box::new(move || {
                    start_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                    // Lazy, give enough delay to allow signal to propagate.
                    sleep(Duration::from_millis(200));
                    stop_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                }),
            )
            .unwrap();

            // Wait until thread reports alive, then drop
            start_rx.blocking_recv().unwrap();
        }

        // Background thread will still be alive, send signal to allow blocked
        stop_rx.blocking_recv().unwrap();
    }

    #[tokio::test]
    async fn long_poll_exits_early() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
        let tx_exit = Mutex::new(Some(tx_exit));

        {
            let _task = PollingTask::new_with_checker(
                None,
                Duration::from_millis(0),
                Box::new(move |checker: &dyn Fn() -> bool| {
                    if let Some(tx) = tx.lock().unwrap().take() {
                        tx.send(true).unwrap();

                        loop {
                            if !checker() {
                                break;
                            }
                        }

                        tx_exit.lock().unwrap().take().unwrap().send(true).unwrap();
                    }
                }),
            )
            .unwrap();

            // Guarantee we polled at least once
            rx.await.unwrap();
        }

        // Ensure the long poll exits by signal, not by test going out of scope.
        rx_exit.await.unwrap();
    }
}
