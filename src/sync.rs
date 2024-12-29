use crate::error::CancelPollingTaskTimeout;
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

#[must_use = "Dropping this handle will cancel the background task"]
/// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit
/// at the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best-effort clean exit. Whether this is a
/// blocking operation depends on how the handle was configured by [`PollingTaskBuilder`].
///
/// If the handle is configured to track exit with timeout and a timeout occurs, the drop will panic!
///
/// Any tracking is blocking, including when activated at drop time.
pub struct PollingTaskHandle {
    receiver: Receiver<()>,
    shared_state: Arc<SharedState>,
    timeout: Option<Duration>,
}

impl PollingTaskHandle {
    /// Cancel the task now, allowing the caller to decide how to handle timeout errors instead of
    /// panicking at drop time. If you haven't configured the handle to monitor for exit with
    /// timeout, it's unlikely that this function is useful.
    pub fn cancel(mut self) -> Result<(), CancelPollingTaskTimeout> {
        self.cancel_impl()
    }

    fn cancel_impl(&mut self) -> Result<(), CancelPollingTaskTimeout> {
        if *self.shared_state.active.lock().unwrap() {
            *self.shared_state.active.lock().unwrap() = false;
            self.shared_state.signal.notify_one();

            match self.timeout {
                None => {
                    // If Err, the thread died before it could signal. There's nothing to handle
                    // here, we're just waiting until the thread exits.
                    let _ = self.receiver.recv();
                }
                Some(timeout) => {
                    if self.receiver.recv_timeout(timeout).is_err() {
                        return Err(CancelPollingTaskTimeout);
                    }
                }
            }
        }

        Ok(())
    }
}

impl Drop for PollingTaskHandle {
    fn drop(&mut self) {
        self.cancel_impl()
            .expect("Polling thread didn't signal exit within timeout");
    }
}

/// We use an [`UnsafeCell`] to provide interior mutability where closure usage precludes the compiler
/// from seeing the usage is safe. Unsafe blocks aren't enough though, as the Arc won't implement
/// Sync. This wrapper type addresses that.
struct IntervalCell(UnsafeCell<Duration>);

unsafe impl Sync for IntervalCell {}

#[derive(Clone)]
/// Allows the polling task to check if a request to exit was received mid-execution. The task
/// abstraction handles the signal promptly by interrupting sleeps, but it can't safely interrupt
/// your arbitrary lambda. If the lambda is long-running / iterative in some way, it should check
/// if the task is still running as needed to decrease cancellation latency.
pub struct TaskChecker<'a> {
    shared_state: &'a Arc<SharedState>,
}

impl<'a> TaskChecker<'a> {
    fn new(shared_state: &'a Arc<SharedState>) -> Self {
        Self { shared_state }
    }

    /// Checks if the polling task is still in a running state. Returns false when the owner has
    /// requested a clean exit. This should be checked periodically through iterative work. When
    /// false is returned, exit your closure as soon as possible.
    pub fn is_running(&self) -> bool {
        self.shared_state.active.lock().unwrap().to_owned()
    }
}

#[derive(Default)]
pub struct PollingTaskBuilder {
    wait_for_clean_exit: bool,
    timeout: Option<Duration>,
}

impl PollingTaskBuilder {
    pub fn new() -> Self {
        Self {
            wait_for_clean_exit: false,
            timeout: None,
        }
    }

    /// Configures the final handle to track the exit of the background task when cancelled or
    /// dropped. See [`PollingTaskHandle`] for full details. Specifying a timeout is optional.
    /// Either way the operation in the handle is blocking.
    pub fn wait_for_clean_exit(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self.wait_for_clean_exit = true;
        self
    }

    fn into_polling_task_handle<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn(&TaskChecker) + Send + 'static,
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        let signal = Arc::new(Condvar::new());
        let shared_state = Arc::new(SharedState {
            active: Mutex::new(true),
            signal: signal.clone(),
        });
        let shared_state_clone = shared_state.clone();

        let _thread_handle = thread::spawn(move || {
            let checker = TaskChecker::new(&shared_state_clone);
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
            receiver,
            shared_state,
            timeout: self.timeout,
        }
    }

    /// General purpose RAII polling task that executes a closure with a given frequency.
    pub fn task<F>(self, interval: Duration, task: F) -> PollingTaskHandle
    where
        F: Fn() + Send + 'static,
    {
        self.task_with_checker(interval, move |_checker| task())
    }

    /// General purpose RAII polling task that executes a closure capable of early exiting with a
    /// given frequency. If your task is long-running or has iterations (say updating 10 cache
    /// entries sequentially), you can assert the managed task is still active to early exit during
    /// a clean exit.
    pub fn task_with_checker<F>(self, interval: Duration, task: F) -> PollingTaskHandle
    where
        F: Fn(&TaskChecker) + Send + 'static,
    {
        let interval_fetcher = move || interval;
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }

    /// Executes a closure with a given frequency where the closure knows the next interval period.
    /// An example would be a polling thread monitoring for updates to a config file that includes a
    /// setting for how often to poll the file.
    pub fn self_updating_task<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn() -> Duration + Send + 'static,
    {
        self.self_updating_task_with_checker(move |_checker| task())
    }

    /// Executes a closure with a given frequency where the closure knows the next interval period.
    /// An example would be a polling thread monitoring for updates to a config file that includes a
    /// setting for how often to poll the file.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache
    /// entries sequentially), you can assert the managed task is still active to early exit during
    /// a clean exit.
    pub fn self_updating_task_with_checker<F>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&TaskChecker) -> Duration + Send + 'static,
    {
        // Initial value is never used, just put in default.
        let interval = Arc::new(IntervalCell(UnsafeCell::new(Default::default())));
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
                *interval_ref = task(checker);
            }
        })
    }

    /// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
    /// rate is retrieved from the given closure on every iteration.
    pub fn variable_task<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn() + Send + 'static,
    {
        self.into_polling_task_handle(interval_fetcher, move |_| task())
    }

    /// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
    /// rate is retrieved from the given closure on every iteration.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache
    /// entries sequentially), you can assert the managed task is still active to early exit during
    /// a clean exit.
    pub fn variable_task_with_checker<D, F>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Duration + Send + 'static,
        F: Fn(&TaskChecker) + Send + 'static,
    {
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }
}

/// General purpose RAII polling task that executes a closure with a given frequency.
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

/// Executes a closure with a given frequency where the closure knows the next interval period.
/// An example would be a polling thread monitoring for updates to a config file that includes a
/// setting for how often to poll the file.
pub fn self_updating_fire_and_forget_polling_task<F>(task: F)
where
    F: Fn() -> Duration + Send + 'static,
{
    thread::spawn(move || loop {
        thread::sleep(task());
    });
}

/// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
/// rate is retrieved from the given closure on every iteration.
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
