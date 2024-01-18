use std::cell::{RefCell, UnsafeCell};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::{PollingIntervalSetter, UnitTask};

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

struct Wrapper {
    ptr: *mut Duration
}

struct Wrapper2 {
    f: Box<dyn Fn(Duration)>
}

unsafe impl Send for Wrapper{}
unsafe impl Send for Wrapper2{}


pub fn self_updating_fire_and_forget_polling_task<F>(interval: Duration, task: F)
    where
        F: Fn(&dyn Fn(Duration)) + Send + 'static
{
    let mut interval = Box::new(interval);
    let ptr= &mut *interval as *mut Duration;
    let ptr = Wrapper{ptr};

    let setter = move |duration: Duration| unsafe {
        *(ptr.ptr) = duration;
    };

    let setter = Wrapper2 {f: Box::new(setter) };

        thread::spawn(move || {
            let setter = setter;
            loop {
                task(&*setter.f);

                thread::sleep(*interval);
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

    #[test]
    fn self_updating_polls_and_can_exit() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        self_updating_fire_and_forget_polling_task(Duration::from_millis(10), move |setter: &dyn Fn(Duration)| {
            counter_clone.fetch_add(1, SeqCst);
            setter(Duration::from_millis(0));

            if counter_clone.load(SeqCst) == 100 {
                setter(Duration::from_secs(1000000));
            }
        });

        sleep(Duration::from_millis(500));

        assert_eq!(100, counter.load(SeqCst));
    }

}
