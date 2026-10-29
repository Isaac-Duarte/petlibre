//! The bench as one server: `wellfed-bench serve` is a foreground process
//! that holds the bench lease and the relay board for its whole life and
//! runs one job at a time, each a rig sequence or a suite run in-process
//! and unchanged. Every `wellfed-bench` hardware command is its client.
//!
//! - **Control:** a unix socket, `private/bench.sock`, mode 0600: the live
//!   state and its changes, jobs, their output streams and their cancels.
//! - **Jobs:** a client posts a command line's words, which the server
//!   parses with the CLI's own definitions. A second job is refused while
//!   one runs, never queued. A cancel signals the job's children (SIGINT to
//!   `wellfed-flash`, which stops itself terminally and records it; SIGTERM
//!   to the others' whole process groups) and the sequence runs its usual
//!   cleanup.
//! - **Output:** descriptors 1 and 2 go through one pipe that a thread
//!   drains without pause, into the console, the running job's stream and
//!   its log file.
//! - **Feeder:** one owner device API session for the server's life
//!   (`feeder`), which the jobs and the `wellfed` CLI ask through, and a
//!   receive-only UART capture while no job runs (`capture`); both feed
//!   the bank ledger (`banks`) what the feeder reports.
//!
//! `wellfed-flash` stays a child process, never linked: its pinned checks,
//! plan hashes and no-retry rule are its own.

#![forbid(unsafe_code)]

pub mod api;
mod banks;
mod board;
mod capture;
pub mod client;
mod device_routes;
mod feeder;
mod files;
pub mod jobs;
mod output;
mod routes;
mod server;
mod state;

pub use client::{Client, ClientError, Followed, Interrupt};
pub use feeder::DeviceOptions;
pub use jobs::{JobCommand, JobOptions, ToolPaths};
pub use server::{Options, ServeError, Server, restore_output};
pub use state::Store;

#[cfg(test)]
mod tests;
