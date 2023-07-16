use std::cell::Cell;
use std::num::TryFromIntError;
use std::time::Duration;
use crate::PollingTask;

pub struct SelfUpdatingPollingTask {
    polling_task: Option<PollingTask>
}

pub type PollingIntervalSetter = dyn FnMut(Duration) -> Result<(), TryFromIntError>;

impl SelfUpdatingPollingTask {
    pub fn new(interval: Duration, task: Box<dyn Fn(&PollingIntervalSetter) + Send>) -> Result<Self, TryFromIntError> {
        let outer_task = SelfUpdatingPollingTask{ polling_task: None };
        let mut cell = Cell::new(outer_task);

        let setter = move |duration: Duration| cell.get_mut().polling_task.as_ref().unwrap().set_polling_rate(duration);
        let wrapper = move || (*task)(&setter);

        Ok(Self {
            polling_task: Some(PollingTask::new(interval, Box::new(wrapper))?)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread::sleep;
    use super::*;

    #[test]
    fn update_observed_on_next_poll() {

    }
}
