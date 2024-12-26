//! [![github]](https://github.com/LittleBoxOfSunshine/interruptible-polling)&ensp;[![crates-io]](https://crates.io/crates/interruptible_polling)&ensp;[![docs-rs]](https://docs.rs/interruptible_polling)
//!
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//! [crates-io]: https://img.shields.io/badge/crates.io-fc8d62?style=for-the-badge&labelColor=555555&logo=rust
//! [docs-rs]: https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs
//!
//! <br>
//!
//! This library provides [`PollingTask`] and [`VariablePollingTask`] structs for scheduling a
//! closure to execute as a recurring task.
//!
//! It is common for a service to have long-lived polling operations for the life of the process.
//! The intended use case is to offer a RAII container for a polled operation that will interrupt
//! pending sleeps to allow a low-latency clean exit.
//!
//! If the poll operation is still running, the task drop will join the background thread which will
//! exit after the closure finishes.
//!
//! # Examples
//! - Use [`PollingTask`] to emit a heart beat every 30 seconds without an exit timeout.
//!
//!   ```
//!   use interruptible_polling::sync::PollingTask;
//!   use std::time::Duration;
//!
//!   let task = PollingTask::new(None, Duration::from_secs(30), || {
//!       println!("BeatBeat");
//!   });
//!   ```
//!
//! - Some polled operations such as configuration updates contain the updated rate at which the
//!   service should continue to poll for future updates. The [`VariablePollingTask`] passes a
//!   callback to the poll task that allows it to conveniently apply the new state to future polls.
//!
//!
//!
//! - If your poll operation is long-lived or internally iterative, there are opportunities to assert
//!   if the task is still active to allow the blocked clean exit to occur faster. If you create the
//!   task with [`PollingTask::new_with_checker`] or [`SelfUpdatingPollingTask::new_with_checker`]
//!   your closure will receive a lookup function to peek if the managed task is still active.
//!
//! ```
//!  use interruptible_polling::sync::PollingTask;
//!  use std::time::Duration;
//!
//!  let task = PollingTask::new_with_checker(
//!      None,
//!      Duration::from_secs(30),
//!      |checker: &dyn Fn() -> bool|
//!  {
//!      let keys = vec![1 ,2, 3];
//!
//!      for key in keys {
//!          // Early exit if signaled. The task will not poll again either way, but you have
//!          // returned control to the parent task earlier.
//!          if !checker() {
//!              break;
//!          }
//!
//!          // Some long or potentially long operation such as a synchronous web request.
//!      }
//!  });
//! ```
//!
//! # Fire and Forget
//!
//! For convenience, if you also need to run polling threads that don't require clean exits, fire and forget
//! versions of each polling task is offered with the same semantics for interval updates and early exit
//! checkers.
//!
//! # Tokio
//!
//! The `tokio` module offers async variants compatible with [`tokio`]. Thef
//!
//! # Generic Async
//!
//!

pub mod sync;
//pub mod async;
mod tokio;
