use std::num::TryFromIntError;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

struct PollingTask {
    inner_task: SelfUpdatingPollingTask,
    interval: Arc<AtomicU64>,
}

impl PollingTask {
    pub fn new<F>(interval: Duration, task: F) -> Result<Self, TryFromIntError>
        where F: Fn() + Send + 'static
    {
        let (shared_interval, shared_interval_clone) = Self::new_intervals(interval)?;

        let wrapped_task = move |interval: &mut Duration| {
            task();
            Self::apply_new_interval_if_exists(interval, &shared_interval_clone)
        };

        Ok(Self {
            inner_task: SelfUpdatingPollingTask::new(interval, wrapped_task),
            interval: shared_interval,
        })
    }

    fn new_intervals(interval: Duration) -> Result<(Arc<AtomicU64>, Arc<AtomicU64>), TryFromIntError> {
        let shared_interval = Arc::new(AtomicU64::new(u64::try_from(interval.as_millis())?));
        let shared_interval_clone = shared_interval.clone();

        Ok((shared_interval, shared_interval_clone))
    }

    fn apply_new_interval_if_exists(inner_interval: &mut Duration, task_interval: &Arc<AtomicU64>) {
        let task_interval = Duration::from_millis(task_interval.load(Ordering::Relaxed));

        // If the interval doesn't match that of the inner task, a set call was made since we
        // last polled and the new value needs to be propagated.
        if *inner_interval != task_interval
        {
            *inner_interval = task_interval;
        }
    }

    pub fn new_with_checker<F>(interval: Duration, task: F) -> Result<Self, TryFromIntError>
        where F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let (shared_interval, shared_interval_clone) = Self::new_intervals(interval)?;

        let wrapped_task = move |interval: &mut Duration, still_alive_checker: &dyn Fn() -> bool| {
            task(still_alive_checker);
            Self::apply_new_interval_if_exists(interval, &shared_interval_clone)
        };

        Ok(Self {
            inner_task: SelfUpdatingPollingTask::new_with_checker(interval, wrapped_task),
            interval: shared_interval,
        })
    }
}

struct InnerState {
    pub(crate) active: Mutex<bool>,
    pub(crate) signal: Condvar,
}

impl InnerState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(InnerState {
            active: Mutex::new(true),
            signal: Condvar::new(),
        })
    }
}

pub struct SelfUpdatingPollingTask {
    inner_state: Arc<InnerState>,
    background_thread: Option<JoinHandle<()>>,
}

impl SelfUpdatingPollingTask {
    pub fn new<F>(mut interval: Duration, task: F) -> Self
        where F: Fn(&mut Duration) + Send + 'static
    {
        let shared_state = InnerState::new();
        let shared_state_clone = shared_state.clone();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                Self::poll_task_forever(&mut interval, &shared_state_clone, task)
            }))
        }
    }

    pub fn new_with_checker<F>(mut interval: Duration, task: F) -> Self
        where F: Fn(&mut Duration, &dyn Fn() -> bool) + Send + 'static,
    {
        let shared_state = InnerState::new();
        let shared_state_clone = shared_state.clone();

        Self {
            inner_state: shared_state,
            background_thread: Some(thread::spawn(move || {
                let shared_state_checker_clone = shared_state_clone.clone();

                let checker = move || {
                    shared_state_checker_clone.active.lock().unwrap().to_owned()
                };

                Self::poll_task_forever(&mut interval, &shared_state_clone, move |interval: &mut Duration| {
                    task(interval, &checker)
                })
            }))
        }
    }

    fn poll_task_forever<F>(interval: &mut Duration, inner_state: &Arc<InnerState>, task: F)
        where F: Fn(&mut Duration) + Send + 'static
    {
        loop {
            task(interval);

            let result = inner_state.signal.wait_timeout_while(
                inner_state.active.lock().unwrap(),
                *interval,
                |&mut active| active,
            )
                .unwrap();

            if !result.1.timed_out() {
                break;
            }
        }
    }
}

impl Drop for SelfUpdatingPollingTask {
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
            Duration::from_millis(0), move |interval: &mut Duration| {
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
