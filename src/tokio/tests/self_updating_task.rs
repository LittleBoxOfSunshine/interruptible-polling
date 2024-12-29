use crate::tokio::PollingTaskBuilder;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc, Mutex,
    },
    time::Duration,
};

#[tokio::test]
async fn update_observed_on_next_poll_with_early_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let _task = PollingTaskBuilder::new(Duration::from_millis(0))
        .track_for_clean_exit_within(Duration::from_millis(300))
        .self_updating_task(move || {
            let counter = counter_clone.clone();
            let tx_clone = tx.clone();
            async move {
                counter.fetch_add(1, SeqCst);
                if let Some(tx) = tx_clone.lock().unwrap().take() {
                    tx.send(true).unwrap();
                }
                Duration::from_secs(5000)
            }
        });

    rx.await.unwrap();
    assert_eq!(counter.load(SeqCst), 1);
}

#[tokio::test]
async fn slow_poll_exits_early() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
    let tx_exit = Arc::new(Mutex::new(Some(tx_exit)));

    {
        let _task = PollingTaskBuilder::new(Duration::from_millis(0))
            .track_for_clean_exit_within(Duration::from_millis(300))
            .self_updating_task_with_checker(move |checker| {
                let tx_clone = tx.clone();
                let tx_exit_clone = tx_exit.clone();
                async move {
                    tx_clone.lock().unwrap().take().unwrap().send(true).unwrap();

                    loop {
                        if !checker.is_running() {
                            break;
                        }
                    }

                    // Prevent issues caused by cycling a second time.
                    tx_exit_clone
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(true)
                        .unwrap();
                    Duration::from_secs(5000)
                }
            });

        // Guarantee we polled at least once
        rx.await.unwrap();
    }

    // Ensure the long poll exits by signal, not by test going out of scope.
    rx_exit.await.unwrap();
}
