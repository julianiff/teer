//! teer: the essence of every revision.
//!
//! Library crate behind the `teer` binary. `units`, `graph` and `review`
//! hold no TUI code, so reviews can be prepared headless (`teer --json`).

pub mod agent;
pub mod approve;
pub mod cli;
pub mod clipboard;
pub mod commit;
pub mod config;
pub mod gh;
pub mod git;
pub mod graph;
pub mod log;
pub mod notes;
pub mod pr;
pub mod pr_list;
pub mod review;
pub mod sandbox;
pub mod tui;
pub mod units;
pub mod util;
