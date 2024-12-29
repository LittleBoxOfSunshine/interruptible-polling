use crate::tokio::{
    fire_and_forget_polling_task, self_updating_fire_and_forget_polling_task,
    variable_fire_and_forget_polling_task,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc,
    },
    time::Duration,
};
use tokio::time::sleep;

#[tokio::test]
async fn polls_and_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    fire_and_forget_polling_task(Duration::from_millis(10), move || {
        counter_clone.fetch_add(1, SeqCst);
    });

    sleep(Duration::from_millis(50)).await;

    assert!(counter.load(SeqCst) > 1);
}

#[tokio::test]
async fn self_updating_polls_and_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    self_updating_fire_and_forget_polling_task(
        Duration::from_millis(10),
        move |interval: &mut Duration| {
            counter_clone.fetch_add(1, SeqCst);
            *interval = Duration::from_millis(0);

            if counter_clone.load(SeqCst) == 100 {
                *interval = Duration::from_secs(1000000);
            }
        },
    );

    sleep(Duration::from_millis(500)).await;

    assert_eq!(100, counter.load(SeqCst));
}

#[tokio::test]
async fn variable_polling_multiple_calls_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    variable_fire_and_forget_polling_task(
        || Duration::from_millis(10),
        move || {
            counter_clone.fetch_add(1, SeqCst);
        },
    );

    sleep(Duration::from_millis(50)).await;

    assert!(counter.load(SeqCst) > 1);
}
