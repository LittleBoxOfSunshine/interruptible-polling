//! [![github]](https://github.com/LittleBoxOfSunshine/interruptible-polling)&ensp;[![crates-io]](https://crates.io/crates/interruptible_polling)&ensp;[![docs-rs]](https://docs.rs/interruptible_polling)
//!
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//! [crates-io]: https://img.shields.io/badge/crates.io-fc8d62?style=for-the-badge&labelColor=555555&logo=rust
//! [docs-rs]: https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs
//!
//! <br>
//!
//! This library provides [`PollingTask`] and [`SelfUpdatingPollingTask`] structs for scheduling a
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
//! - Use [`PollingTask`] to emit a heart beat every 30 seconds.
//!
//!   ```
//!   use interruptible_polling::PollingTask;
//!   use std::time::Duration;
//!
//!   let task = PollingTask::new(Duration::from_secs(30), || {
//!       println!("BeatBeat");
//!   });
//!   ```
//!
//! - Some polled operations such as configuration updates contain the updated rate at which the
//!   service should continue to poll for future updates. The [`SelfUpdatingPollingTask`] passes a
//!   callback to the poll task that allows it to conveniently apply the new state to future polls.
//!
//!   ```no_run
//!   use interruptible_polling::SelfUpdatingPollingTask;
//!   use std::time::Duration;
//!   use serde_json::{Value, from_reader};
//!   use std::fs::File;
//!   use std::io::BufReader;
//!
//!   let task = SelfUpdatingPollingTask::new(Duration::from_secs(30),
//!       move |interval: &mut Duration| {
//!           let file = File::open("app.config").unwrap();
//!           let reader = BufReader::new(file);
//!           let config: Value = from_reader(reader).expect("JSON was not well-formatted");
//!
//!           // Do other work with config
//!
//!           *interval = Duration::from_secs(config["pollingInterval"].as_u64().expect("Polling interval isn't u64 convertable"));
//!       }
//!   );
//!   ```
//!
//! - If your poll operation is long-lived or internally iterative, there are opportunities to assert
//!   if the task is still active to allow the blocked clean exit to occur faster. If you create the
//!   task with [`PollingTask::new_with_checker`] or [`SelfUpdatingPollingTask::new_with_checker`]
//!   your closure will receive a lookup function to peek if the managed task is still active.
//!
//! ```
//!  use interruptible_polling::PollingTask;
//!  use std::time::Duration;
//!
//!  let task = PollingTask::new_with_checker(
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
//! For convenience, if you also need to run polling threads that don't require clean exits, fire and forget can be
//! enabled. This is gated behind feature [`fire-forget`] to encourage use of the primary abstractions. It's not hard to make a
//! polling thread, so typical crate users are here for the clean exit constructs. However, some projects need both.
//! If you need both, enable the feature to make both available. By default, it's disabled.
//!

#[cfg(feature = "fire-forget")]
mod fire_and_forget;
mod self_updating_task;
mod task;

#[cfg(feature = "fire-forget")]
pub use fire_and_forget::fire_and_forget_polling_task;
#[cfg(feature = "fire-forget")]
pub use fire_and_forget::self_updating_fire_and_forget_polling_task;

pub use self_updating_task::SelfUpdatingPollingTask;
pub use task::PollingTask;
