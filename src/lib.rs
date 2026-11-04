//! teer: the essence of every revision.
//!
//! Library crate behind the `teer` binary. `units`, `graph`, `review` and
//! `session` hold no terminal code, so reviews can be prepared headless
//! (`teer --json`) and driven by another front end. The terminal front
//! end (`tui`, and the `cli` that opens it) is the `tui` feature, on by
//! default.

pub mod agent;
pub mod approve;
#[cfg(feature = "tui")]
pub mod cli;
pub mod clipboard;
pub mod commit;
pub mod config;
pub mod editor;
pub mod gh;
pub mod git;
pub mod graph;
pub mod keymap;
pub mod log;
pub mod notes;
pub mod pr;
pub mod pr_list;
pub mod review;
pub mod sandbox;
pub mod session;
pub mod setup;
pub mod start;
pub mod syntax;
#[cfg(feature = "tui")]
pub mod tui;
pub mod units;
pub mod util;
pub mod watch;
