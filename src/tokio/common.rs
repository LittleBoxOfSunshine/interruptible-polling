use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;

#[derive(Error, Debug)]
pub enum JoinError {
    #[error("Timed out waiting for the polling thread to exit")]
    Timeout,
}

pub(crate) struct InnerTaskState {
    pub(crate) active: Mutex<bool>,
    pub(crate) signal: Condvar,
    pub(crate) timeout: Option<Duration>,
}

impl InnerTaskState {
    pub(crate) fn new(timeout: Option<Duration>) -> Arc<Self> {
        Arc::new(InnerTaskState {
            active: Mutex::new(true),
            signal: Condvar::new(),
            timeout,
        })
    }

    pub async fn join_async(&self, rx: &mut Receiver<()>) -> Result<(), JoinError> {
        *self.active.lock().unwrap() = false;
        self.signal.notify_one();

        match self.timeout {
            None => {
                // If the underlying thread panics, Err is returned. There's nothing useful we can
                // do with that here. The point is to wait until exit. If it panicked, it has already
                // exited and we can move on.
                let _ = rx.recv().await;
                Ok(())
            }
            Some(timeout) => Ok(tokio::time::timeout(timeout, async {
                let _ = rx.recv().await;
                ()
            })
            .await
            .map_err(|_| JoinError::Timeout)?),
        }
    }
}
