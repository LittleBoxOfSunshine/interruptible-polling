use std::sync::Arc;
use std::time::Duration;
use crate::PollingTask;

pub struct SelfUpdatingPollingTask {
    polling_task: PollingTask
}

pub type PollingIntervalSetter = dyn Fn(Duration);

// impl SelfUpdatingPollingTask {
//     pub fn new(interval: Duration, task: impl Fn(&PollingIntervalSetter)) -> Self {
//         let test: Arc<Self>;
//         let test2 = test.clone();
//
//         let wrapper = move |duration: Duration| test2.polling_task.set_polling_rate(duration);
//
//         Self {
//             polling_task: PollingTask::new(interval, move || task(&wrapper))
//         }
//     }
// }
