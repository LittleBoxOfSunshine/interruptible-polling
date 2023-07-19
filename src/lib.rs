//! [![github]](https://github.com/LittleBoxOfSunshine/interruptible_polling)&ensp;[![crates-io]](https://crates.io/crates/interruptible_polling)&ensp;[![docs-rs]](https://docs.rs/interruptible_polling)
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
//! It is common for a service to have long lived polling operations for the life of the process.
//! The intended use case is to offer a RAII container for a polled operation that will interrupt
//! pending sleeps to allow a low-latency clean exit.
//!
//! If the poll operation is still running, the thread will exit after the closure finishes.
//!
//! <br>
//!
//! # Examples
//! - Use [`PollingTask`] to emit a heart beat every 30 seconds.
//!
//!   ```
//!   use interruptible_polling::PollingTask;
//!   use std::time::Duration;
//!
//!   let task = PollingTask::new(Duration::from_secs(30), Box::new(|| {
//!       println!("BeatBeat");
//!   }));
//!   ```
//!
//! - Some polled operations such as configuration updates contain the updated rate at which the
//!   service should continue to poll for future updates. The [`SelfUpdatingPollingTask`] passes a
//!   callback to the poll task that allows it to conveniently apply the new state to future polls.
//!
//!   ```no_run
//!   use interruptible_polling::{PollingIntervalSetter, SelfUpdatingPollingTask};
//!   use std::time::Duration;
//!   use serde_json::{Value, from_reader};
//!   use std::fs::File;
//!   use std::io::BufReader;
//!
//!   let task = SelfUpdatingPollingTask::new(Duration::from_secs(30), Box::new(
//!       move |setter: &PollingIntervalSetter| {
//!           let file = File::open("app.config").unwrap();
//!           let reader = BufReader::new(file);
//!           let config: Value = from_reader(reader).expect("JSON was not well-formatted");
//!
//!           // Do other work with config
//!
//!           setter(Duration::from_secs(config["polling_interval"].as_u64().unwrap()))
//!               .expect("Polling interval isn't u64 convertable");
//!       }
//!   ));
//!   ```

mod self_updating_task;
mod task;

pub use self_updating_task::PollingIntervalSetter;
pub use self_updating_task::SelfUpdatingPollingTask;
pub use task::PollingTask;
