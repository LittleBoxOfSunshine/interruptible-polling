use std::{
    cell::UnsafeCell,
    sync::{mpsc::Receiver, Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

struct SharedState {
    active: Mutex<bool>,
    signal: Arc<Condvar>,
}

pub struct PollingTaskHandle {
    wait_for_clean_exit: bool,
    receiver: Receiver<()>,
    shared_state: Arc<SharedState>,
    timeout: Option<Duration>,
}

impl Drop for PollingTaskHandle {
    fn drop(&mut self) {
        *self.shared_state.active.lock().unwrap() = false;
        self.shared_state.signal.notify_one();

        if self.wait_for_clean_exit {
            match self.timeout {
                None => {
                    // TODO: This whole mechanism needs test coverage
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

        let _thread_handle = thread::spawn(move || {
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
            timeout: self.timeout,
        }
    }

    /// General purpose RAII polling task that executes a closure with a given frequency.
    ///
    /// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit at
    /// the first available opportunity. If the thread is currently sleeping, this will occur almost
    /// immediately. If the closure is still running, it will happen immediately after the closure
    /// finishes. The task joins on the background thread as a best-effort clean exit.
    ///
    /// Note nothing special is done to try and keep the thread alive longer. If you terminate the
    /// program the default behavior of reaping the thread mid-execution will still occur.
    pub fn task<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn() + Send + 'static,
    {
        self.task_with_checker(move |_checker| task())
    }

    /// If your task is long-running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn task_with_checker<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&dyn Fn() -> bool) + Send + 'static,
    {
        let interval = self.interval;
        let interval_fetcher = move || interval;
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }

    /// Executes a closure with a given frequency where the closure can change the polling rate.
    ///
    /// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit at
    /// the first available opportunity. If the thread is currently sleeping, this will occur almost
    /// immediately. If the closure is still running, it will happen immediately after the closure
    /// finishes. The task joins on the background thread as a best-effort clean exit.
    ///
    /// Note nothing special is done to try and keep the thread alive longer. If you terminate the
    /// program the default behavior of reaping the thread mid-execution will still occur.
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
        let interval = Arc::new(IntervalCell(UnsafeCell::new(self.interval)));
        let interval_clone = interval.clone();
        // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
        // determine how long to sleep for. This happens in the same thread (and function call, the
        // poll 'proc'). This means that the access and potential mutation can't race.
        let interval_fetcher = move || unsafe { (*interval_clone.0.get()).to_owned() };

        self.into_polling_task_handle(interval_fetcher, move |checker| {
            // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
            // determine how long to sleep for. This happens in the same thread (and function call, the
            // poll 'proc'). This means that the access and potential mutation can't race.
            unsafe {
                let interval_ref = &mut *interval.0.get();
                task(interval_ref, checker);
            }
        })
    }

    /// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
    /// rate is retrieved from the given closure on every iteration.
    ///
    /// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit at
    /// the first available opportunity. If the thread is currently sleeping, this will occur almost
    /// immediately. If the closure is still running, it will happen immediately after the closure
    /// finishes. The task joins on the background thread as a best-effort clean exit.
    ///
    /// Note nothing special is done to try and keep the thread alive longer. If you terminate the
    /// program the default behavior of reaping the thread mid-execution will still occur.
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

pub fn variable_fire_and_forget_polling_task<D, F>(interval_fetcher: D, task: F)
where
    D: Fn() -> Duration + Send + 'static,
    F: Fn() + Send + 'static,
{
    thread::spawn(move || loop {
        task();

        thread::sleep(interval_fetcher());
    });
}

#[cfg(test)]
mod tests {
    mod fire_and_forget;
    mod self_updating_task;
    mod task;
    mod variable_task;
}
