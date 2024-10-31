use std::sync::{Arc, Condvar, Mutex};

pub(crate) struct InnerTaskState {
    pub(crate) active: Mutex<bool>,
    pub(crate) signal: Condvar,
}

impl InnerTaskState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(InnerTaskState {
            active: Mutex::new(true),
            signal: Condvar::new(),
        })
    }
}