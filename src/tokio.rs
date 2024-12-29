use crate::error::CancelPollingTaskTimeout;
use std::{cell::UnsafeCell, future::Future, sync::Arc, time::Duration};
use tokio::{select, sync::Notify};
use tokio_util::sync::CancellationToken;

pub struct PollingTaskHandle {
    signal: Arc<Notify>,
    cancellation_token: CancellationToken,
    timeout: Option<Duration>,
}

impl PollingTaskHandle {
    pub async fn cancel(mut self) -> Result<(), CancelPollingTaskTimeout> {
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

        // TODO: This whole mechanism needs test coverage
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

pub struct PollingTaskBuilder {
    track_for_clean_exit: bool,
    interval: Duration,
    timeout: Option<Duration>,
}

impl PollingTaskBuilder {
    pub fn new(interval: Duration) -> Self {
        Self {
            track_for_clean_exit: false,
            interval,
            timeout: None,
        }
    }

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
        F: Fn(&dyn Fn() -> bool) -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        let signal = Arc::new(Notify::new());
        let signal_clone = signal.clone();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let cancellation_token_clone2 = cancellation_token.clone();

        let _thread_handle = tokio::task::spawn(async move {
            let checker = || cancellation_token_clone2.is_cancelled();
            loop {
                task(&checker).await;

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
    ///
    /// When [`PollingTaskHandle`] is dropped, the background thread is signaled to perform a clean exit at
    /// the first available opportunity. If the thread is currently sleeping, this will occur almost
    /// immediately. If the closure is still running, it will happen immediately after the closure
    /// finishes. The task joins on the background thread as a best-effort clean exit.
    ///
    /// Note nothing special is done to try and keep the thread alive longer. If you terminate the
    /// program the default behavior of reaping the thread mid-execution will still occur.
    pub fn task<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        self.task_with_checker(move |_checker| task())
    }

    /// If your task is long-running or has iterations (say updating 10 cache entries sequentially),
    /// you can assert if the managed task is active to early exit during a clean exit.
    pub fn task_with_checker<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&dyn Fn() -> bool) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let interval = self.interval;
        let interval_fetcher = move || async move { interval };
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
    pub fn self_updating_task<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&mut Duration) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        self.self_updating_task_with_checker(move |duration, _checker| task(duration))
    }

    pub fn self_updating_task_with_checker<F, Fut>(self, task: F) -> PollingTaskHandle
    where
        F: Fn(&mut Duration, &dyn Fn() -> bool) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let interval = Arc::new(IntervalCell(UnsafeCell::new(self.interval)));
        let interval_clone = interval.clone();
        // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
        // determine how long to sleep for. This happens in the same thread (and function call, the
        // poll 'proc'). This means that the access and potential mutation can't race.
        let interval_fetcher = move || {
            let interval = unsafe { (*interval_clone.0.get()).to_owned() };
            async move { interval }
        };

        self.into_polling_task_handle(interval_fetcher, move |checker| {
            // SAFETY: The general implementation calls the passed task, *then* fetches the interval to
            // determine how long to sleep for. This happens in the same thread (and function call, the
            // poll 'proc'). This means that the access and potential mutation can't race.
            unsafe {
                let interval_ref = &mut *interval.0.get();
                task(interval_ref, checker)
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
    pub fn variable_task<D, F, Dfut, Ffut>(self, interval_fetcher: D, task: F) -> PollingTaskHandle
    where
        D: Fn() -> Dfut + Send + 'static,
        Dfut: Future<Output = Duration> + Send,
        F: Fn() -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        self.into_polling_task_handle(interval_fetcher, move |_| task())
    }

    pub fn variable_task_with_checker<D, F, Dfut, Ffut>(
        self,
        interval_fetcher: D,
        task: F,
    ) -> PollingTaskHandle
    where
        D: Fn() -> Dfut + Send + 'static,
        Dfut: Future<Output = Duration> + Send,
        F: Fn(&dyn Fn() -> bool) -> Ffut + Send + 'static,
        Ffut: Future<Output = ()> + Send,
    {
        self.into_polling_task_handle(interval_fetcher, move |checker| task(checker))
    }
}

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

pub fn self_updating_fire_and_forget_polling_task<F>(interval: Duration, task: F)
where
    F: Fn(&mut Duration) + Send + 'static,
{
    tokio::task::spawn(async move {
        let mut interval = interval;
        loop {
            task(&mut interval);

            tokio::time::sleep(interval).await;
        }
    });
}

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
