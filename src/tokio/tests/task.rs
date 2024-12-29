use crate::tokio::{PollingTaskBuilder, PollingTaskHandle};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::sleep;

#[tokio::test]
async fn poll() {
    let (counter, _task) = get_task_and_timer(true);
    sleep(Duration::from_millis(50)).await;

    assert!(counter.load(SeqCst) > 1);
}

#[tokio::test]
async fn poll_no_wait() {
    let (counter, _task) = get_task_and_timer(false);
    sleep(Duration::from_millis(50)).await;

    assert!(counter.load(SeqCst) > 1);
}

fn get_task_and_timer(wait: bool) -> (Arc<AtomicU64>, PollingTaskHandle) {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    let mut task_builder = PollingTaskBuilder::new(Duration::from_millis(1));

    if wait {
        task_builder = task_builder.track_for_clean_exit_within(Duration::from_millis(100));
    }

    let task = task_builder.task(move || {
        let counter = counter_clone.clone();
        async move {
            counter.fetch_add(1, SeqCst);
        }
    });

    (counter, task)
}

#[tokio::test]
// This is implicitly covered through the set time tests having very large wait periods, but
// keeping a dedicated test for clarity and to make the validation explicit.
async fn early_clean_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    let _task = PollingTaskBuilder::new(Duration::from_millis(5000))
        .track_for_clean_exit_within(Duration::from_millis(100))
        .task(move || {
            let counter = counter_clone.clone();
            async move {
                counter.fetch_add(1, SeqCst);
            }
        });
    sleep(Duration::from_millis(50)).await;

    assert_eq!(1, counter.load(SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_while_running_blocks() {
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let start_tx = Arc::new(Mutex::new(Some(start_tx)));
    let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));

    {
        let _task = PollingTaskBuilder::new(Duration::from_millis(5000))
            .track_for_clean_exit_within(Duration::from_millis(500))
            .task(move || {
                let start_tx_clone = start_tx.clone();
                let stop_tx_clone = stop_tx.clone();
                async move {
                    start_tx_clone
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .unwrap();

                    // Lazy, give enough delay to allow signal to propagate. Intentionally using the
                    // wrong sleep here.
                    std::thread::sleep(Duration::from_millis(200));
                    stop_tx_clone
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .unwrap();
                    ()
                }
            });

        // Wait until thread reports alive, then drop
        start_rx.await.unwrap();
    }

    // Background thread will still be alive, send signal to allow blocked
    stop_rx.await.unwrap();
}

#[tokio::test]
async fn long_poll_exits_early() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
    let tx_exit = Arc::new(Mutex::new(Some(tx_exit)));

    {
        let _task = PollingTaskBuilder::new(Duration::from_millis(5000))
            .track_for_clean_exit_within(Duration::from_millis(100))
            .task_with_checker(move |checker| {
                let tx_clone = tx.clone();
                let tx_exit_clone = tx_exit.clone();

                async move {
                    if let Some(tx) = tx_clone.lock().unwrap().take() {
                        tx.send(true).unwrap();

                        loop {
                            if !checker.is_running() {
                                break;
                            }
                        }

                        tx_exit_clone
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(true)
                            .unwrap();
                    }
                }
            });

        // Guarantee we polled at least once
        rx.await.unwrap();
    }

    // Ensure the long poll exits by signal, not by test going out of scope.
    rx_exit.await.unwrap();
}
