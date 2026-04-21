//! Bytecode: roundhouse's seventh emission target.
//!
//! This module defines the typed, stack-based bytecode format that the
//! bytecode emitter will produce from analyzed IR and that a VM in the
//! runtime crate will execute. Today (M1) it contains only the format
//! types; emission and execution land in subsequent milestones.
//!
//! See [`format`] for the opcode set and container layout.

pub mod format;

pub use format::{FORMAT_VERSION, Op, Program, RtFnId, StrId, SymId, UserFn, UserFnId};
