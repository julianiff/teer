//! The words a unit goes by on every screen.

use std::path::Path;

use crate::review::model::{Unit, UnitKind};
use crate::units::extract::MODULE_SYMBOL;
use crate::util::sanitize;

/// The name a unit goes by in rows: its symbol, `(top level)` for the
/// code outside any function, and a whole file's file name.
pub fn row_name(u: &Unit) -> String {
    match u.kind {
        UnitKind::File => file_name(&u.file),
        _ if u.kind == UnitKind::Module || u.symbol == MODULE_SYMBOL => "(top level)".into(),
        _ => sanitize(&u.symbol),
    }
}

/// The last part of `path`.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}
