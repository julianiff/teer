//! The review as the reviewer goes through it, whatever draws it: the
//! queue, the unit view, the flagged list, the `:w` sheet, what keys and
//! actions do to them, and the runtime that carries out what they ask
//! for. No terminal code: the terminal front end (`crate::tui`) draws an
//! [`App`] and feeds it keys; another front end can do the same.

pub mod app;
pub mod exposure;
pub mod finish;
pub mod fix;
pub mod flagged_list;
pub mod help;
pub mod jump;
pub mod mode;
pub mod names;
pub mod prove;
pub mod runtime;
pub mod send;
pub mod suggest;
pub mod tools;
pub mod unit;
pub mod view;

pub use app::{App, AppEvent, Effect, Overlay, Screen};
pub use runtime::Runtime;
