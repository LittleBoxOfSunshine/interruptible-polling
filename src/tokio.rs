use crate::error::CancelPollingTaskTimeout;
use std::{cell::UnsafeCell, future::Future, sync::Arc, time::Duration};
use tokio::{select, sync::Notify};
use tokio_util::sync::CancellationToken;

#[must_use = "Dropping this handle will cancel the background task"]
/// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit
/// at the first available opportunity. If the thread is currently sleeping, this will occur almost
/// immediately. If the closure is still running, it will happen immediately after the closure
/// finishes. The task joins on the background thread as a best-effort clean exit. Whether this is a
/// blocking operation depends on how the handle was configured by [`PollingTaskBuilder`].
///
/// If the handle is configured to track exit with timeout and a timeout occurs, the drop will panic!
///
/// The tokio variant here is none blocking. Any monitoring is performed by spawning and detaching a
/// new tokio task.
pub struct PollingTaskHandle {
    signal: Arc<Notify>,
    cancellation_token: CancellationToken,
    timeout: Option<Duration>,
}

impl PollingTaskHandle {
    /// Cancel the task now, allowing the caller to decide how to handle timeout errors instead of
    /// panicking at drop time. If you haven't configured the handle to monitor for exit with
    /// timeout, it's unlikely that this function is useful.
    pub async fn cancel(self) -> Result<(), CancelPollingTaskTimeout> {
        Self::cancel_impl(
            self.cancellation_token.clone(),
            self.signal.clone(),
            self.timeout,
        )
        .await
    }

    async fn cancel_impl(
        cancellation_token: CancellationToken,
        signal: Arc<Notify>,
        timeout: Option<Duration>,
    ) -> Result<(), CancelPollingTaskTimeout> {
        cancellation_token.cancel();

        if let Some(timeout) = timeout {
            if let Err(_) = tokio::time::timeout(timeout, async {
                // If Err, the thread died before it could signal. There's nothing to handle
                // here, we're just waiting until the thread exits.
                let _ = signal.notified().await;
            })
            .await
            {
                return Err(CancelPollingTaskTimeout);
            }
        }

        Ok(())
    }
}

impl Drop for PollingTaskHandle {
    fn drop(&mut self) {
        match self.timeout {
            Some(timeout) => {
                let cancellation_token = self.cancellation_token.clone();
                let signal = self.signal.clone();

                tokio::task::spawn(async move {
                    Self::cancel_impl(cancellation_token, signal, Some(timeout))
                        .await
                        .expect("Polling task didn't signal exit within timeout");
                });
            }
            None => self.cancellation_token.cancel(),
        }
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
pub struct TaskChecker {
    cancellation_token: CancellationToken,
}

impl TaskChecker {
    fn new(cancellation_token: CancellationToken) -> Self {
        Self { cancellation_token }
    }

    /// Checks if the polling task is still in a running state. Returns false when the owner has
    /// requested a clean exit. This should be checked periodically through iterative work. When
    /// false is returned, exit your closure as soon as possible.
    pub fn is_running(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

pub struct PollingTaskBuilder {
    timeout: Option<Duration>,
}

impl PollingTaskBuilder {
    pub fn new() -> Self {
        Self { timeout: None }
    }

    /// Configures the final handle to track the exit of the background task when cancelled or
    /// dropped. See [`PollingTaskHandle`] for full details. In the async variant, it doesn't make
    /// sense to track without a timeout because the tracking is non-blocking. A timeout must be
    /// provided.
    pub fn track_for_clean_exit_within(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    fn into_polling_task_handle<D, F, Dfut, Ffut>(
        self,
        interval_fetcher: D,
        task: F,
    ) -> PollingTaskHandle
    where
        D: Fn() -> Dfut + Send + 'static,
        Dfut: Future<Output = Duration> + Send,
        F: Fn(TaskChecker) -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        let signal = Arc::new(Notify::new());
        let signal_clone = signal.clone();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let cancellation_token_clone2 = cancellation_token.clone();

        let _thread_handle = tokio::task::spawn(async move {
            let checker = TaskChecker::new(cancellation_token_clone2);
            loop {
                let checker_clone = checker.clone();
                task(checker_clone).await;

                select! {
                    _ = cancellation_token_clone.cancelled() => {
                        break;
                    }
                    _ = tokio::time::sleep(interval_fetcher().await) => {
                        // Nothing to do, move on to next iteration.
                    }
                }
            }

            let _ = signal_clone.notify_one();
        });

        PollingTaskHandle {
            signal,
            timeout: self.timeout,
            cancellation_token,
        }
    }

    /// General purpose RAII polling task that executes a closure with a given frequency.
    pub fn task<F, Fut>(self, interval: Duration, task: F) -> PollingTaskHandle
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        self.task_with_checker(interval, move |_checker| task())
    }

    /// General purpose RAII polling task that executes a closure capable of early exiting with a
    /// given frequency. If your task is long-running or has iterations (say updating 10 cache
    /// entries sequentially), you can assert the managed task is still active to early exit during
    /// a clean exit.
    pub fn task_with_checker<F, Fut>(self, interval: Duration, task: F) -> PollingTaskHandle
    where
        F: Fn(TaskChecker) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let interval_fetcher = move || async move { interval };
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }

    /// Executes a closure with a given frequency where the closure knows the next interval period.
    /// An example would be a polling thread monitoring for updates to a config file that includes a
    /// setting for how often to poll the file.
    pub fn self_updating_task<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = Duration> + Send,
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
    pub fn self_updating_task_with_checker<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(TaskChecker) -> Fut + Send + 'static,
        Fut: Future<Output = Duration> + Send,
    {
        // Initial value is never used, just put in default.
        let interval = Arc::new(IntervalCell(UnsafeCell::new(Default::default())));
        let interval_clone = interval.clone();

        // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
        // determine how long to sleep for. This happens in the same thread (and function call, the
        // poll 'proc'). This means that the access and potential mutation can't race.
        let interval_fetcher = move || {
            let interval = unsafe { (*interval_clone.0.get()).to_owned() };
            async move { interval }
        };

        self.into_polling_task_handle(interval_fetcher, move |checker| {
            let interval_clone = interval.clone();
            let task_future = task(checker);

            async move {
                // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
                // determine how long to sleep for. This happens in the same thread (and function call, the
                // poll 'proc'). This means that the access and potential mutation can't race.
                unsafe {
                    let interval_ref = &mut *interval_clone.0.get();
                    *interval_ref = task_future.await;
                }
            }
        })
    }

    /// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
    /// rate is retrieved from the given closure on every iteration.
    pub fn variable_task<D, F, Dfut, Ffut>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Dfut + Send + 'static,
        Dfut: Future<Output = Duration> + Send,
        F: Fn() -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        self.into_polling_task_handle(interval_fetcher, move |_| task())
    }

    /// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
    /// rate is retrieved from the given closure on every iteration.
    ///
    /// If your task is long-running or has iterations (say updating 10 cache
    /// entries sequentially), you can assert the managed task is still active to early exit during
    /// a clean exit.
    pub fn variable_task_with_checker<D, F, Dfut, Ffut>(
        self,
        interval_fetcher: D,
        task: F,
    ) -> PollingTaskHandle
    where
        D: Fn() -> Dfut + Send + 'static,
        Dfut: Future<Output = Duration> + Send,
        F: Fn(TaskChecker) -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }
}

/// General purpose RAII polling task that executes a closure with a given frequency.
pub fn fire_and_forget_polling_task<F>(interval: Duration, task: F)
where
    F: Fn() + Send + 'static,
{
    tokio::task::spawn(async move {
        loop {
            task();

            // There's no need to support a fast exit here, because there is no handle head for this
            // thread. Instead, we rely on that rust will kill the thread when the exe exits. This is
            // safe, because if a clean exit was needed the other offerings of the crate would be used.
            tokio::time::sleep(interval).await;
        }
    });
}

/// Executes a closure with a given frequency where the closure knows the next interval period.
/// An example would be a polling thread monitoring for updates to a config file that includes a
/// setting for how often to poll the file.
pub fn self_updating_fire_and_forget_polling_task<F>(task: F)
where
    F: Fn() -> Duration + Send + 'static,
{
    tokio::task::spawn(async move {
        loop {
            tokio::time::sleep(task()).await;
        }
    });
}

/// Executes a closure with a remotely sourced, potentially variable interval rate. The interval
/// rate is retrieved from the given closure on every iteration.
pub fn variable_fire_and_forget_polling_task<D, F>(interval_fetcher: D, task: F)
where
    D: Fn() -> Duration + Send + 'static,
    F: Fn() + Send + 'static,
{
    tokio::task::spawn(async move {
        loop {
            task();

            tokio::time::sleep(interval_fetcher()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    mod fire_and_forget;
    mod self_updating_task;
    mod task;
    mod variable_task;
}
