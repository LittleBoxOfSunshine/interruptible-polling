use std::num::TryFromIntError;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) struct PollingTaskInnerState {
    pub(crate) active: Mutex<bool>,
    pub(crate) signal: Condvar,
    pub(crate) interval: AtomicU64,
}

impl PollingTaskInnerState {
    pub(crate) fn new(interval: Duration) -> Result<Arc<Self>, TryFromIntError> {
        Ok(Arc::new(PollingTaskInnerState {
            active: Mutex::new(true),
            signal: Condvar::new(),
            interval: AtomicU64::new(u64::try_from(interval.as_millis())?),
        }))
    }
}

/// General purpose RAII polling task that executes a closure with a given frequency.
///
/// When [`PollingTask`] is dropped, the background thread is signaled to perform a clean exit at
/// the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best effort clean exit.
///
/// Note nothing special is done to try and keep the thread alive longer. If you terminate the
/// program the default behavior of reaping the thread mid execution will still occur.
pub struct PollingTask {
    shared_state: Arc<PollingTaskInnerState>,
    background_thread: Option<JoinHandle<()>>,
}

/// Basic closure type for poll operation.
pub type UnitTask = dyn Fn() + Send;

/// Closure can periodically check if the task is still active to respect attempts to clean early
/// exit. Suitable for long running or iterative poll operations.
pub type CheckerTask = dyn Fn(&StillActiveChecker) + Send;

/// Closure to check active status without exposing the underlying mutex.
pub type StillActiveChecker = dyn Fn() -> bool;

pub(crate) enum Task {
    Unit(Box<UnitTask>),
    WithChecker(Box<CheckerTask>),
}

impl PollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new(interval: Duration, task: Box<UnitTask>) -> Result<Self, TryFromIntError> {
        let shared_state = PollingTaskInnerState::new(interval)?;
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
        task: Box<CheckerTask>,
    ) -> Result<Self, TryFromIntError> {
        let shared_state = PollingTaskInnerState::new(interval)?;
        let task = Task::WithChecker(task);
        new_task!(Self, shared_state, task)
    }

    /// Update the delay between poll events. Applied on the next iteration.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    pub fn set_polling_rate(&self, interval: Duration) -> Result<(), TryFromIntError> {
        self.shared_state
            .interval
            .store(u64::try_from(interval.as_millis())?, Relaxed);
        Ok(())
    }

    fn poll(shared_state: &Arc<PollingTaskInnerState>, task: &Box<dyn Fn() + Send>) {
        wait_with_timeout!(shared_state, task);
    }
}

impl Drop for PollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        drop_task!(self);
    }
}

macro_rules! drop_task {
    ($self:ident) => {
        *$self.shared_state.active.lock().unwrap() = false;
        $self.shared_state.signal.notify_one();
        $self.background_thread.take().unwrap().join().unwrap();
    };
}

pub(crate) use drop_task;

macro_rules! wait_with_timeout {
    ($shared_state:ident, $task:ident) => {
        loop {
            ($task)();

            let result = $shared_state
                .signal
                .wait_timeout_while(
                    $shared_state.active.lock().unwrap(),
                    Duration::from_millis($shared_state.interval.load(Relaxed)),
                    |&mut active| active,
                )
                .unwrap();

            if !result.1.timed_out() {
                break;
            }
        }
    };
}

pub(crate) use wait_with_timeout;

macro_rules! new_task {
    ($task_type:tt, $shared_state:ident, $task:ident) => {{
        let mut polling_task = $task_type {
            shared_state: $shared_state,
            background_thread: None,
        };

        let shared_state = polling_task.shared_state.clone();

        let task = match $task {
            Task::Unit(task) => task,
            Task::WithChecker(task) => {
                let checker_shared_state = shared_state.clone();
                let checker = move || *checker_shared_state.active.lock().unwrap();
                Box::new(move || task(&checker))
            }
        };

        polling_task.background_thread =
            Some(thread::spawn(move || Self::poll(&shared_state, &task)));

        Ok(polling_task)
    }};
}

pub(crate) use new_task;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
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
                Duration::from_millis(0),
                Box::new(move |checker: &StillActiveChecker| {
                    if let Some(tx

                    ) = tx.lock().unwrap().take() {
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
