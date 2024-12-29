//! [![github]](https://github.com/LittleBoxOfSunshine/interruptible-polling)&ensp;[![crates-io]](https://crates.io/crates/interruptible_polling)&ensp;[![docs-rs]](https://docs.rs/interruptible_polling)
//!
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//! [crates-io]: https://img.shields.io/badge/crates.io-fc8d62?style=for-the-badge&labelColor=555555&logo=rust
//! [docs-rs]: https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs
//!
//! <br>
//!
//! This library provides [`PollingTaskBuilder`][`sync::PollingTaskBuilder`] and [`PollingTaskHandle`][`sync::PollingTaskHandle`] for scheduling a closure
//! to execute as a recurring task. The returned handle operates as a RAII handle, meaning it will
//! signal the background task to exit and clean up any pending work. The signal is low latency,
//! even if the thread is sleeping when it is sent.
//!
//! It is common for a service to have long-lived polling operations for the life of the process.
//! The intended use case is to offer a RAII container for a polled operation that will interrupt
//! pending sleeps to allow a low-latency clean exit.
//!
//! There handle can be configured signal the thread to exit then any of:
//!     - Move on without waiting (default, to match rust std conventions)
//!     - Wait for the thread to reply it is done running the current proc (i.e. iteration) using [`wait_for_clean_exit`][`sync::PollingTaskBuilder::wait_for_clean_exit`] with `None` passed
//!     - Wait for the thread to reply it is done with a timeout using [`wait_for_clean_exit`][`sync::PollingTaskBuilder::wait_for_clean_exit`] with `Some(Duration)` passed
//!
//! ## Cancellation timeouts and panics
//!
//! Any handle can be canceled directly using its `cancel` function. This allows you to decide how
//! to handle timeouts (if applicable). Cancellation occurs at drop time if `cancel` isn't called.
//! If a timeout occurs during a drop, a panic is raised.
//!
//! ## Examples
//!
//! - Use [`task`][`sync::PollingTaskBuilder::task`] to emit a heart beat every 30 seconds without an exit timeout. The returned
//!   handle send a cancel signal when dropped, then block until the background thread indicates it
//!   is done.
//!
//!   ```
//!   # use std::time::Duration;
//!   use interruptible_polling::sync::PollingTaskBuilder;
//!
//!   let handle = PollingTaskBuilder::new()
//!       .wait_for_clean_exit(None)
//!       .task(Duration::from_secs(30), || {
//!           println!("BeatBeat");
//!       });
//!   ```
//!
//! - If your poll operation is time-intensive or internally iterative, there are opportunities to assert
//!   if the task is still active to allow the blocked clean exit to occur faster. If you create the
//!   task with [`task_with_checker`][`sync::PollingTaskBuilder::task_with_checker`] or and other `_with_checker` suffixed
//!   function, your closure will receive a lookup function to peek if the managed task is still active.
//!
//!   ```
//!   # use std::time::Duration;
//!   use interruptible_polling::sync::PollingTaskBuilder;
//!   let files = vec!["foo.txt", "bar.txt", "cow.txt"];
//!   let handle = PollingTaskBuilder::new()
//!       .wait_for_clean_exit(None)
//!       .task_with_checker(Duration::from_secs(30), move |checker| {
//!           for file in files.iter() {
//!               // Do things with file
//!
//!               if !checker.is_running() {
//!                   break
//!               }
//!           }
//!       });
//!   ```
//!
//! - If the polling rate is sourced from a dynamic source, using [`variable_task`][`sync::PollingTaskBuilder::variable_task`]
//!   allows providing a closure to source the interval from each iteration.
//!
//!   ```
//!   # use std::time::Duration;
//!   use interruptible_polling::sync::PollingTaskBuilder;
//!   let interval_fetcher = || Duration::from_secs(30);
//!   let handle = PollingTaskBuilder::new()
//!       .wait_for_clean_exit(None)
//!       .variable_task(interval_fetcher, || {
//!           println!("BeatBeat");
//!       });
//!   ```
//!
//! - Some polled operations such as configuration updates contain the updated rate at which the
//!   service should continue to poll for future updates. [`self_updating_task`][`sync::PollingTaskBuilder::self_updating_task`] passes a
//!   callback to the poll task that allows it to conveniently apply the new state to future polls.
//!
//!   ```
//!   # use std::time::Duration;
//!   # use serde_json::Value;
//!   # use std::io::Read;
//!   # use std::fs::File;
//!   use interruptible_polling::sync::PollingTaskBuilder;
//!   let handle = PollingTaskBuilder::new()
//!       .wait_for_clean_exit(None)
//!       .self_updating_task(|| {
//!           let mut file = File::open("config.json").unwrap();
//!           let mut contents = String::new();
//!           file.read_to_string(&mut contents).unwrap();
//!           let config: Value = serde_json::from_str(&contents).unwrap();
//!           // Do things with config
//!
//!           // Return the portion of the config that determines polling rate
//!           Duration::from_secs(config["pollingRateSeconds"].as_u64().unwrap())
//!       });
//!   ```
//!
//! ## Fire and Forget
//!
//! For convenience, if you also need to run polling threads that don't require clean exits, fire
//! and forget versions of each polling task is offered with the same semantics for interval updates
//! and early exits. See the functions in the [`sync`] module. These are supported as distinct
//! functions rather than allowing the handle to detach to improve efficiency.
//!
//! ## Async / Tokio
//!
//! Async variants are available for use, currently only the `tokio` runtime is supported. Enable
//! the `tokio` feature to use them.
//!
//! ### Distinctions
//!
//! Rust doesn't have an async drop. If the handle attempts to wait until the background task signals
//! it has finished, this while become a blocking operation in the runtime. If monitoring is requested,
//! a new tokio task will be spawned on drop to await the exit signal. Same as with the sync variant,
//! it will wait indefinitely or with a timeout. If a timeout occurs, the task will panic.
//!
//! If [`cancel`][`tokio::PollingTaskHandle::cancel`] is called, no new task is spawned. Since we're in an
//! async context it's sufficient to yield to the runtime like normal.
//!
//! For these reasons, `wait_for_clean_exit` isn't offered. Its closest
//! equivalent is [`track_for_clean_exit_within`][`tokio::PollingTaskBuilder::track_for_clean_exit_within`].
//!
//! ### Example
//!
//! Async polling task, on drop or cancel will spawn a task to confirm the background task exited
//! within 5 seconds of being notified. Uses variable interval + checker to show a full feature set
//! example.
//!
//! ```
//! # use std::time::Duration;
//! # use serde_json::Value;
//! # use std::io::Read;
//! # use std::fs::File;
//! # use std::sync::Arc;
//!   use interruptible_polling::tokio::PollingTaskBuilder;
//!
//! # #[tokio::main]
//! # async fn main() {
//!   let files = Arc::new(vec!["foo.txt", "bar.txt", "cow.txt"]);
//!   let interval_fetcher = || async { Duration::from_secs(30) };
//!   let handle = PollingTaskBuilder::new()
//!       .track_for_clean_exit_within(Duration::from_secs(5))
//!       .variable_task_with_checker(interval_fetcher, move |checker| {
//!           let files_clone = files.clone();
//!           async move {
//!               for file in files_clone.iter() {
//!                   // Do things with file
//!
//!                   if !checker.is_running() {
//!                       break
//!                   }
//!               }
//!           }
//!       });
//! # }
//! ```

pub mod sync;

#[cfg(feature = "tokio")]
pub mod tokio;

mod error;
