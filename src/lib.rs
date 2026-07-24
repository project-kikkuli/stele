//! stele — declare a rule once; enforce it across every AI coding agent harness.
//!
//! Design (empirical basis: conformance/RESULTS.md):
//! - A rule is a pure function of the measurement substrate (root, base,
//!   change-set), computed once in [`substrate`]; checkers never touch git.
//! - Delivery channels are layered restoring organs — prompt injection,
//!   stop blocks, tool gates, synthesized resume-loops, git hooks, CI — all
//!   running the same check. CI is the unbypassable floor.
//! - Local channels fail open but distinguish "green" from "couldn't
//!   measure"; CI fails loud on both.

pub mod ack;
pub mod compile;
pub mod config;
pub mod conformance;
pub mod devin;
pub mod doctor;
pub mod emit;
pub mod engine;
pub mod eval;
pub mod hook;
pub mod launch;
pub mod rules;
pub mod substrate;
pub mod wrap;
