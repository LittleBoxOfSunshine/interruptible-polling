use std::cell::RefCell;
use std::num::TryFromIntError;
use std::time::Duration;
use crate::PollingTask;

pub struct SelfUpdatingPollingTask {
    polling_task: Option<PollingTask>
}

pub type PollingIntervalSetter = dyn Fn(Duration) -> Result<(), TryFromIntError>;

impl SelfUpdatingPollingTask {
    pub fn new(interval: Duration, task: Box<dyn Fn(&PollingIntervalSetter) + Send>) -> Result<Self, TryFromIntError> {
        let mut outer_task = SelfUpdatingPollingTask{ polling_task: None };
        let cell = RefCell::new(outer_task);

        let setter = move |duration: Duration| cell.borrow().polling_task.as_ref().unwrap().set_polling_rate(duration);
        let wrapper = move || (*task)(&setter);

        outer_task.polling_task = Some(PollingTask::new(interval, Box::new(wrapper))?);

        Ok(outer_task)
    }
}

#[cfg(test)]
mod tests {
    //use tokio;
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
