use crate::sync::{PollingTaskBuilder, PollingTaskHandle};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc, Mutex,
    },
    thread::sleep,
    time::Duration,
};

#[test]
fn cancel() {
    let (_counter, task) = wait_case();
    let _test = task.cancel();
}

fn wait_case() -> (Arc<AtomicU64>, PollingTaskHandle) {
    let (counter, task) = get_task_and_timer(true);
    sleep(Duration::from_millis(50));

    assert!(counter.load(SeqCst) > 1);
    (counter, task)
}

#[test]
fn poll() {
    let _ = wait_case();
}

#[test]
fn poll_no_wait() {
    let (counter, _task) = get_task_and_timer(false);
    sleep(Duration::from_millis(50));

    assert!(counter.load(SeqCst) > 1);
}

fn get_task_and_timer(wait: bool) -> (Arc<AtomicU64>, PollingTaskHandle) {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    let mut task_builder = PollingTaskBuilder::new();

    if wait {
        task_builder = task_builder.wait_for_clean_exit(None)
    }

    let task = task_builder.task(Duration::from_millis(1), move || {
        counter_clone.fetch_add(1, SeqCst);
    });

    (counter, task)
}

#[test]
// This is implicitly covered through the set time tests having very large wait periods, but
// keeping a dedicated test for clarity and to make the validation explicit.
fn early_clean_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    let _task = PollingTaskBuilder::new().wait_for_clean_exit(None).task(
        Duration::from_millis(5000),
        move || {
            counter_clone.fetch_add(1, SeqCst);
        },
    );
    sleep(Duration::from_millis(50));

    assert_eq!(1, counter.load(SeqCst))
}

#[test]
fn drop_while_running_blocks() {
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let start_tx = Mutex::new(Some(start_tx));
    let stop_tx = Mutex::new(Some(stop_tx));

    {
        let _task = PollingTaskBuilder::new().wait_for_clean_exit(None).task(
            Duration::from_millis(5000),
            move || {
                start_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                // Lazy, give enough delay to allow signal to propagate.
                sleep(Duration::from_millis(200));
                stop_tx.lock().unwrap().take().unwrap().send(()).unwrap();
            },
        );

        // Wait until thread reports alive, then drop
        start_rx.blocking_recv().unwrap();
    }

    // Background thread will still be alive, send signal to allow blocked
    stop_rx.blocking_recv().unwrap();
}

#[tokio::test]
async fn long_poll_exits_early() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
    let tx_exit = Mutex::new(Some(tx_exit));

    {
        let _task = PollingTaskBuilder::new()
            .wait_for_clean_exit(None)
            .task_with_checker(Duration::from_millis(5000), move |checker| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    tx.send(true).unwrap();

                    loop {
                        if !checker.is_running() {
                            break;
                        }
                    }

                    tx_exit.lock().unwrap().take().unwrap().send(true).unwrap();
                }
            });

        // Guarantee we polled at least once
        rx.await.unwrap();
    }

    // Ensure the long poll exits by signal, not by test going out of scope.
    rx_exit.await.unwrap();
}
