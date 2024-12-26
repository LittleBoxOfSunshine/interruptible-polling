use std::sync::{Arc, Condvar, Mutex};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use thiserror::Error;

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
            timeout
        })
    }

    pub fn join_sync(&self, thread: std::thread::JoinHandle<()>, rx: &Receiver<()>) -> Result<(), JoinError> {
        *self.active.lock().unwrap() = false;
        self.signal.notify_one();

        match self.timeout {
            None => {
                // If the underlying thread panics, Err is returned. There's nothing useful we can
                // do with that here. The point is to wait until exit. If it panicked, it has already
                // exited and we can move on.
                let _ = thread.join();
                Ok(())
            }
            Some(timeout) => {
                Ok(rx.recv_timeout(timeout).map_err(|_| JoinError::Timeout)?)
            }
        }
    }
}