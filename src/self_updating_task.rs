use std::num::TryFromIntError;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::Duration;
use crate::task::PollingTaskInnerState;

pub struct SelfUpdatingPollingTask {
    shared_state: Arc<PollingTaskInnerState>,
}

pub type PollingIntervalSetter = dyn Fn(Duration) -> Result<(), TryFromIntError>;

impl SelfUpdatingPollingTask {
    /// The interval must be expressible as a u64 in milliseconds.
    pub fn new(interval: Duration, task: Box<dyn Fn(&PollingIntervalSetter) + Send>) -> Result<Self, TryFromIntError> {
        let polling_task = Self {
            shared_state: Arc::new(PollingTaskInnerState {
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

    fn poll(shared_state: &Arc<PollingTaskInnerState>, task: &Box<dyn Fn(&PollingIntervalSetter) + Send>) {
        let copy = shared_state.clone();
        let setter = move |duration: Duration| {
            copy.interval.store(u64::try_from(duration.as_millis())?, Relaxed);
            Ok(())
        };

        loop {
            (task)(&setter);

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
        }
    }
}

impl Drop for SelfUpdatingPollingTask {
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
