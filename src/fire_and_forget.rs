use std::thread;
use std::time::Duration;

pub fn fire_and_forget_polling_task<F>(interval: Duration, task: F)
where
    F: Fn() + Send + 'static,
{
    thread::spawn(move || loop {
        task();

        // There's no need to support a fast exit here, because there is no handle head for this
        // thread. Instead, we rely on that rust will kill the thread when the exe exits. This is
        // safe, because if a clean exit was needed the other offerings of the crate would be used.
        thread::sleep(interval);
    });
}

pub fn self_updating_fire_and_forget_polling_task<F>(interval: Duration, task: F)
where
    F: Fn(&mut Duration) + Send + 'static,
{
    thread::spawn(move || {
        let mut interval = interval;
        loop {
            task(&mut interval);

            thread::sleep(interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Arc;
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

    #[test]
    fn self_updating_polls_and_can_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        self_updating_fire_and_forget_polling_task(
            Duration::from_millis(10),
            move |interval: &mut Duration| {
                counter_clone.fetch_add(1, SeqCst);
                *interval = Duration::from_millis(0);

                if counter_clone.load(SeqCst) == 100 {
                    *interval = Duration::from_secs(1000000);
                }
            },
        );

        sleep(Duration::from_millis(500));

        assert_eq!(100, counter.load(SeqCst));
    }
}
