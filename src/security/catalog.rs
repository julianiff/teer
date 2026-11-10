//! The built-in catalog, per language: authorization checks, entry points
//! and sinks by class, matched by name in the syntax tree; plus the user's
//! additions from `security.toml` ([`super::config::Extra`]).
//!
//! Phase 1 (facts) fills this in: PHP, TypeScript/JavaScript and Go first.
