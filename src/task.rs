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
    background_thread: Option<JoinHandle<()>>
}

impl PollingTask {
    /// Creates a new background thread that immediately executes the given task.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    /// * `task` The closure to execute at every poll.
    pub fn new(interval: Duration, task: Box<dyn Fn() + Send>) -> Result<Self, TryFromIntError> {
        new_task!(Self, interval, task)
    }

    /// Update the delay between poll events. Applied on the next iteration.
    ///
    /// * `interval` The interval to poll at. Note it must be expressible as a u64 in milliseconds.
    pub fn set_polling_rate(&self, interval: Duration) -> Result<(), TryFromIntError> {
        self.shared_state.interval.store(u64::try_from(interval.as_millis())?, Relaxed);
        Ok(())
    }

    fn poll(shared_state: &Arc<PollingTaskInnerState>, task: &Box<dyn Fn() + Send>) {
        wait_with_timeout! (shared_state, (task)());
    }
}

impl Drop for PollingTask {
    /// Signals the background thread that it should exit at first available opportunity.
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();
        self.background_thread.take().unwrap().join().unwrap();
    }
}

macro_rules! wait_with_timeout {
    ($shared_state:ident, $task_invocation:expr) => {
        loop {
            $task_invocation;

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
    }
}

pub(crate) use wait_with_timeout;

macro_rules! new_task {
    ($task_type:tt, $interval:ident, $task:ident) => {
        {
            let mut polling_task = $task_type {
                shared_state: Arc::new(PollingTaskInnerState {
                    active: Mutex::new(true),
                    signal: Condvar::new(),
                    interval: AtomicU64::new(u64::try_from($interval.as_millis())?),
                }),
                background_thread: None
            };

            let shared_state = polling_task.shared_state.clone();

            polling_task.background_thread = Some(thread::spawn(move || {
                Self::poll(&shared_state, &$task);
            }));

            Ok(polling_task)
        }
    }
}

pub(crate) use new_task;

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread::sleep;
    use super::*;

    #[test]
    fn poll() {
        let (counter, _task) = get_task_and_timer();
        sleep(Duration::from_millis(50));

        assert!(counter.load(SeqCst) > 1);
    }

    fn get_task_and_timer() -> (Arc<AtomicU64>, PollingTask) {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        let task = PollingTask::new(Duration::from_millis(1), Box::new(move || { counter_clone.fetch_add(1, SeqCst); }));
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

        let _task = PollingTask::new(Duration::from_secs(5000), Box::new(move || { counter_clone.fetch_add(1, SeqCst); })).unwrap();
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
            let _task = PollingTask::new(Duration::from_secs(5000), Box::new(move || {
                start_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                // Lazy, give enough delay to allow signal to propagate.
                sleep(Duration::from_millis(200));
                stop_tx.lock().unwrap().take().unwrap().send(()).unwrap();
            })).unwrap();

            // Wait until thread reports alive, then drop
            start_rx.blocking_recv().unwrap();
        }

        // Background thread will still be alive, send signal to allow blocked
        stop_rx.blocking_recv().unwrap();
    }
}
