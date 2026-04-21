//! Bytecode: roundhouse's seventh emission target.
//!
//! This module defines the typed, stack-based bytecode format that the
//! bytecode emitter will produce from analyzed IR, plus a minimal VM
//! that executes it.
//!
//! - [`format`] — opcode set and container layout (M1)
//! - [`vm`] — minimal stack-based interpreter (M2)
//!
//! Emission (IR → bytecode walker) and integration with the runtime
//! for calls / collections / string interpolation land in subsequent
//! milestones.

pub mod format;
pub mod vm;

pub use format::{FORMAT_VERSION, Op, Program, RtFnId, StrId, SymId, UserFn, UserFnId};
pub use vm::{Value, Vm, VmError};
