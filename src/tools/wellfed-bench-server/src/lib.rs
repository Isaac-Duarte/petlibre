//! The bench as one server: `wellfed-bench serve` is a foreground process
//! that holds the bench lease and the relay board for its whole life and
//! runs one job at a time, each a rig sequence or a suite run in-process
//! and unchanged. Every `wellfed-bench` hardware command is its client.
//!
//! - **Control:** a unix socket, `private/bench.sock`, mode 0600: the live
//!   state and its changes, jobs, their output streams and their cancels.
//! - **Jobs:** a second job is refused while one runs, never queued. A
//!   cancel signals the job's children (SIGINT to `wellfed-flash`, which
//!   stops itself terminally and records it; SIGTERM to the rest) and the
//!   sequence runs its usual cleanup.
//! - **Output:** descriptors 1 and 2 go through one pipe that a thread
//!   drains without pause, into the console and the running job's stream.
//!
//! `wellfed-flash` stays a child process, never linked: its pinned checks,
//! plan hashes and no-retry rule are its own.

#![forbid(unsafe_code)]

pub mod api;
mod board;
pub mod client;
pub mod jobs;
mod output;
mod routes;
mod server;
mod state;

pub use client::{Client, ClientError, Followed, Interrupt};
pub use jobs::{JobCommand, JobSpec, ToolPaths};
pub use server::{Options, ServeError, Server};
pub use state::Store;

#[cfg(test)]
mod tests;
