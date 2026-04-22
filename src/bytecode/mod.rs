//! Bytecode: roundhouse's seventh emission target.
//!
//! This module defines the typed, stack-based bytecode format that the
//! bytecode emitter produces from analyzed IR, plus a minimal VM that
//! executes it and the IR → bytecode walker that fills in the format.
//!
//! - [`format`] — opcode set and container layout (M1)
//! - [`vm`] — minimal stack-based interpreter (M2)
//! - [`walker`] — IR → bytecode walker for a subset of `ExprNode` (M3a)
//!
//! Integration with the runtime for calls / collections / string
//! interpolation and full-app emission land in subsequent milestones.

pub mod format;
pub mod vm;
pub mod walker;

pub use format::{FORMAT_VERSION, Op, Program, RtFnId, StrId, SymId, UserFn, UserFnId};
pub use vm::{Value, Vm, VmError};
pub use walker::{WalkError, Walker};
