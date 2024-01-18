use std::thread;
use std::time::Duration;
use crate::UnitTask;

pub fn fire_and_forget_polling_task<F>(interval: Duration, task: F)
    where
        F: Fn() + Send + 'static
{
    thread::spawn(move || {
        loop {
            task();

            thread::sleep(interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread::sleep;

    #[test]
    fn polls_and_can_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        fire_and_forget_polling_task(Duration::from_millis(10), move || {
            counter_clone.fetch_add(1, SeqCst);
        });

        sleep(Duration::from_millis(50));

        assert!(counter.load(SeqCst) > 1);
    }

}
