use crate::sync2::PollingTaskBuilder;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn update_observed_on_next_poll_with_early_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));

    let _task = PollingTaskBuilder::new(Duration::from_millis(0))
        .wait_for_clean_exit(None)
        .self_updating_task(move |interval: &mut Duration| {
            counter_clone.fetch_add(1, SeqCst);
            *interval = Duration::from_secs(5000);
            if let Some(tx) = tx.lock().unwrap().take() {
                tx.send(true).unwrap();
            }
        });

    rx.await.unwrap();
    assert_eq!(counter.load(SeqCst), 1);
}

#[tokio::test]
async fn slow_poll_exits_early() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
    let tx_exit = Mutex::new(Some(tx_exit));

    {
        let _task = PollingTaskBuilder::new(Duration::from_millis(0))
            .wait_for_clean_exit(None)
            .self_updating_task_with_checker(move |interval: &mut Duration, checker: &dyn Fn() -> bool| {
                tx.lock().unwrap().take().unwrap().send(true).unwrap();

                loop {
                    if !checker() {
                        break;
                    }
                }

                // Prevent issues caused by cycling a second time.
                *interval = Duration::from_secs(5000);
                tx_exit.lock().unwrap().take().unwrap().send(true).unwrap();
            });

        // Guarantee we polled at least once
        rx.await.unwrap();
    }

    // Ensure the long poll exits by signal, not by test going out of scope.
    rx_exit.await.unwrap();
}
