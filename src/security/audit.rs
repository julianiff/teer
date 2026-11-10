//! The Security tile (`<space>x`): the agent audits one unit with the path
//! from its entry points (from teer's call graph) and the checks found
//! along it. Every finding must cite real lines, and every hop of its path
//! must be a call the graph knows; a second model then checks each one.
//!
//! Phase 2 fills this in.
