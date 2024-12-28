use std::cell::{RefCell, UnsafeCell};
use std::ops::DerefMut;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

struct SharedState {
    active: Mutex<bool>,
    signal: Arc<Condvar>,
}

struct TaskState {
    interval: Duration,
    shutdown_sender: Sender<()>,
    shared_state: Arc<SharedState>,
}

pub struct PollingTaskHandle {
    wait_for_clean_exit: bool,
    receiver: Receiver<()>,
    shared_state: Arc<SharedState>,
    thread_handle: thread::JoinHandle<()>,
    timeout: Option<Duration>,
}

impl Drop for PollingTaskHandle {
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();

        if self.wait_for_clean_exit {
            match self.timeout {
                None => {
                    // If Err, the thread died before it could signal. There's nothing to handle
                    // here, we're just waiting until the thread exits.
                    let _ = self.receiver.recv();
                }
                Some(timeout) => self
                    .receiver
                    .recv_timeout(timeout)
                    .expect("Polling thread didn't signal exit within timeout"),
            }
        }
    }
}

/// We use an [`UnsafeCell`] to provide interior mutability where closure usage precludes the compiler
/// from seeing the usage is safe. Unsafe blocks aren't enough though, as the Arc won't implement
/// Sync. This wrapper type addresses that.
struct IntervalCell(UnsafeCell<Duration>);

unsafe impl Sync for IntervalCell {}

pub struct PollingTaskBuilder {
    wait_for_clean_exit: bool,
    interval: Duration,
    timeout: Option<Duration>,
}

impl PollingTaskBuilder {
    pub fn new(interval: Duration) -> Self {
        Self {
            wait_for_clean_exit: false,
            interval,
            timeout: None,
        }
    }

    pub fn wait_for_clean_exit(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self.wait_for_clean_exit = true;
        self
    }

    fn into_polling_task_handle<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        let signal = Arc::new(Condvar::new());
        let shared_state = Arc::new(SharedState {
            active: Mutex::new(true),
            signal: signal.clone(),
        });
        let shared_state_clone = shared_state.clone();

        let thread_handle = thread::spawn(move || {
            let checker = || shared_state_clone.active.lock().unwrap().to_owned();
            loop {
                task(&checker);

                let interval = interval_fetcher();
                let result = shared_state_clone
                    .signal
                    .wait_timeout_while(
                        shared_state_clone.active.lock().unwrap(),
                        interval,
                        |&mut active| active,
                    )
                    .unwrap();

                if !result.1.timed_out() {
                    break;
                }
            }

            // If owning thread is down, there's nothing we need to signal.
            let _ = sender.send(());
        });

        PollingTaskHandle {
            wait_for_clean_exit: self.wait_for_clean_exit,
            receiver,
            shared_state,
            thread_handle,
            timeout: self.timeout,
        }
    }

    pub fn task<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn() + Send + 'static,
    {
        self.task_with_checker(move |_checker| task())
    }

    pub fn task_with_checker<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let interval = self.interval.clone();
        let interval_fetcher = move || interval;
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }

    pub fn self_updating_task<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&mut Duration) + Send + 'static,
    {
        self.self_updating_task_with_checker(move |duration, _checker| task(duration))
    }

    pub fn self_updating_task_with_checker<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&mut Duration, &dyn Fn() -> bool) + Send + 'static,
    {
        let mut interval = Arc::new(IntervalCell {
            0: UnsafeCell::new(self.interval.clone()),
        });
        let interval_clone = interval.clone();
        // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
        // determine how long to sleep for. This happens in the same thread (and function call, the
        // poll 'proc'). This means that the access and potential mutation can't race.
        let interval_fetcher = move || unsafe { (*(*interval_clone).0.get()).to_owned() };

        self.into_polling_task_handle(interval_fetcher, move |checker| {
            // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
            // determine how long to sleep for. This happens in the same thread (and function call, the
            // poll 'proc'). This means that the access and potential mutation can't race.
            unsafe {
                let interval_ref = &mut *(*interval).0.get();
                task(interval_ref, checker);
            }
        })
    }

    pub fn variable_task<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn() + Send + 'static,
    {
        self.into_polling_task_handle(interval_fetcher, move |_| task())
    }

    pub fn variable_task_with_checker<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }
}

#[cfg(test)]
mod tests {
    mod self_updating_task;
    mod task;
    mod variable_task;
}
