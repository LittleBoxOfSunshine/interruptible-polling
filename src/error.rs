use thiserror::Error;

#[derive(Debug, Error)]
#[error("Timed out waiting for polling task to signal exit")]
pub struct CancelPollingTaskTimeout;
