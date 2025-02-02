use crate::sync::{
    fire_and_forget_polling_task, self_updating_fire_and_forget_polling_task,
    variable_fire_and_forget_polling_task,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc,
    },
    thread::sleep,
    time::Duration,
};

#[test]
fn polls_and_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    fire_and_forget_polling_task(Duration::from_millis(10), move || {
        counter_clone.fetch_add(1, SeqCst);
    });

    sleep(Duration::from_millis(50));

    assert!(counter.load(SeqCst) > 1);
}

#[test]
fn self_updating_polls_and_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    self_updating_fire_and_forget_polling_task(move || {
        counter_clone.fetch_add(1, SeqCst);

        if counter_clone.load(SeqCst) == 100 {
            Duration::from_secs(1000000)
        } else {
            Duration::from_millis(0)
        }
    });

    sleep(Duration::from_millis(500));

    assert_eq!(100, counter.load(SeqCst));
}

#[test]
fn variable_polling_multiple_calls_can_exit() {
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = counter.clone();

    variable_fire_and_forget_polling_task(
        || Duration::from_millis(10),
        move || {
            counter_clone.fetch_add(1, SeqCst);
        },
    );

    sleep(Duration::from_millis(50));

    assert!(counter.load(SeqCst) > 1);
}
