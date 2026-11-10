//! The security lens over a review (see `docs/security-plan.md`): facts
//! teer finds in the change without an agent, which raise levels; the
//! attack-surface card and the queue's security order; the Security tile,
//! where the agent's findings must show a path the call graph confirms;
//! proving a finding old against new; and findings imported from other
//! tools (SARIF, GitHub code scanning).

pub mod audit;
pub mod catalog;
pub mod config;
pub mod facts;
pub mod model;
pub mod order;
pub mod prove;
pub mod sarif;
pub mod surface;

pub use config::SecurityConfig;
pub use model::*;
