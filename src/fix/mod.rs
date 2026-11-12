//! Proposed changes: when something important has to change in a unit (a
//! security finding that stands, the reviewer's note, the advice), the
//! agent proposes the code. It only reads: it answers with exact text to
//! replace, and teer checks every hunk applies once and still parses, has
//! a second model check the whole, and shows the hunks in the unit's text
//! like the change's own. The reviewer puts them (`p`, `P`) into the
//! working tree, or, on a pull request, suggests them in the review.

pub mod apply;
pub mod model;
pub mod propose;

pub use model::*;
