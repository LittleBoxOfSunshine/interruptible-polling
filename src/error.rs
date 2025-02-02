use thiserror::Error;

#[derive(Debug, Error)]
#[error("Timed out waiting for polling task to signal exit")]
/// Returned when the handle times out waiting for the background thread to signal it has exited.
pub struct CancelPollingTaskTimeout;
