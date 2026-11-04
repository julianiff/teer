//! The review as the reviewer goes through it, whatever draws it: the
//! queue, the unit view, the flagged list, the `:w` sheet, what keys and
//! actions do to them, and the runtime that carries out what they ask
//! for. No terminal code: the terminal front end (`crate::tui`) draws an
//! [`App`] and feeds it keys; another front end can do the same.

pub mod app;
pub mod finish;
pub mod flagged_list;
pub mod jump;
pub mod names;
pub mod runtime;
pub mod tools;
pub mod unit;

pub use app::{App, AppEvent, Effect, Overlay, Screen};
pub use runtime::Runtime;
