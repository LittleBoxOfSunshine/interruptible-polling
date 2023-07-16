use std::num::TryFromIntError;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

struct InnerState {
    active: Mutex<bool>,
    signal: Condvar,
    interval: AtomicU64,
}

pub struct PollingTask {
    shared_state: Arc<InnerState>,
}

impl PollingTask {
    /// The interval must be expressible as a u64 in milliseconds.
    pub fn new(interval: Duration, task: Box<dyn Fn() + Send>) -> Result<Self, TryFromIntError> {
        let polling_task = Self {
            shared_state: Arc::new(InnerState {
                active: Mutex::new(true),
                signal: Condvar::new(),
                interval: AtomicU64::new(u64::try_from(interval.as_millis())?),
            }),
        };

        let shared_state = polling_task.shared_state.clone();

        thread::spawn(move || {
            Self::poll(&shared_state, &task);
        });

        Ok(polling_task)
    }

    pub fn set_polling_rate(&self, interval: Duration) -> Result<(), TryFromIntError> {
        self.shared_state.interval.store(u64::try_from(interval.as_millis())?, Relaxed);
        Ok(())
    }

    fn poll(shared_state: &Arc<InnerState>, task: &Box<dyn Fn() + Send>) {
        loop {
            let result = shared_state
                .signal
                .wait_timeout_while(
                    shared_state.active.lock().unwrap(),
                    Duration::from_millis(shared_state.interval.load(Relaxed)),
                    |&mut active| active,
                )
                .unwrap();

            if !result.1.timed_out() {
                break;
            }

            (task)()
        }
    }
}

impl Drop for PollingTask {
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();
    }
}

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
}
