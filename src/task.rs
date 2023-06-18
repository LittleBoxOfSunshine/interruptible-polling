use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

struct InnerState {
    active: Mutex<bool>,
    signal: Condvar,
    interval: AtomicU64,
    task: fn(),
}

pub struct PollingTask {
    shared_state: Arc<InnerState>,
}

impl PollingTask {
    /// The interval must be expressible as a u64 in milliseconds.
    pub fn new(interval: Duration, task: fn()) -> Self {
        let task = Self {
            shared_state: Arc::new(InnerState {
                active: Mutex::new(true),
                signal: Condvar::new(),
                interval: AtomicU64::new(u64::try_from(interval.as_millis()).unwrap()),
                task,
            }),
        };

        let shared_state = task.shared_state.clone();

        thread::spawn(move || {
            Self::poll(&shared_state);
        });

        task
    }

    pub fn set_polling_rate(&mut self, interval: Duration) {
        self.shared_state.interval.store(u64::try_from(interval.as_millis()).unwrap(), Relaxed);
    }

    fn poll(shared_state: &Arc<InnerState>) {
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

            (shared_state.task)()
        }
    }
}

impl Drop for PollingTask {
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();
    }
}
