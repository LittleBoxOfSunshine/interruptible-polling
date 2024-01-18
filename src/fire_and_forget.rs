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
    ptr: *mut u64,
    wut: i64
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
    let mut interval = Box::new(interval.as_secs());
    let ptr= &mut *interval as *mut u64;
    let ptr = Wrapper{ptr, wut: 0};
    //let test = UnsafeCell::new(interval);
    //let mut ptr = Wrapper{ptr};
    //let pin = Pin::new(interval);
    //let interval = Arc::new(RefCell::new(interval));
    //let mut interval_clone = interval.clone();
    let setter = move |duration: Duration| unsafe {
        let _ = ptr.wut;
        *(ptr.ptr) = duration.as_secs();

        //Ok(())
    };

    let setter = Wrapper2 {f: Box::new(setter) };

        thread::spawn(move || unsafe {
            let setter = setter;
            loop {
                task(&*setter.f);

                //let test = &mut *interval as *mut u64;

                thread::sleep(Duration::from_secs(*interval));
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
