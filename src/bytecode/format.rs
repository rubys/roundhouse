//! Bytecode format types.
//!
//! The typed, stack-based bytecode roundhouse will emit as its seventh
//! target. Everything in this module is data: the opcode enum, the
//! constant-pool layout, and the `Program` container. Emission (writing
//! a `Program` from IR) and execution (consuming one in a VM) live
//! elsewhere; this file just defines the contract between them.
//!
//! Design choices baked in here (subject to variant experiments later):
//!
//! - **Stack-based.** Instructions operate on an implicit operand stack.
//!   Easier to emit from the tree IR than a register machine; the
//!   register-vs-stack comparison is slotted as an M2/M3-era experiment.
//! - **Typed opcodes.** `AddI64` / `ConcatStr` rather than polymorphic
//!   `Add` with runtime type tags. The analyzer already resolves every
//!   expression's type, so the emitter can pick the right opcode and
//!   the VM never needs to check.
//! - **Direct runtime dispatch.** `CallRt` carries an `RtFnId` that
//!   indexes into `Program::runtime_fns`; the VM resolves each name to
//!   a Rust function pointer once at load time. No per-call hashmap
//!   lookup.
//!
//! Serialization format is deliberately not committed to here: the
//! types derive serde, so tests can roundtrip via any serde-compatible
//! format. M1 uses serde_json for legibility; switch to a binary
//! format (bincode, postcard, or hand-rolled) once the VM consumes
//! these in anger.

use serde::{Deserialize, Serialize};

/// Format version; bumped when the opcode set or container layout
/// changes incompatibly. The VM checks this at load time and refuses
/// bytecode produced by a different major version.
pub const FORMAT_VERSION: u32 = 1;

/// Index into [`Program::string_pool`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StrId(pub u32);

/// Index into [`Program::symbol_pool`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymId(pub u32);

/// Index into [`Program::runtime_fns`]. The VM resolves the name at
/// each index to a Rust function pointer once at load time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RtFnId(pub u16);

/// Index into [`Program::user_fns`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserFnId(pub u16);

/// A single bytecode instruction. Each opcode specifies the types of
/// values it consumes and produces; because the IR is fully typed at
/// analyze time the emitter can always pick the monomorphic form and
/// the VM never needs runtime type tags.
///
/// Branches carry signed offsets relative to the *next* instruction
/// (so `Jump { offset: 0 }` is a no-op fall-through), matching the
/// convention most stack VMs use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    // ── Literal pushes ────────────────────────────────────────────
    /// Push an inline 64-bit integer.
    LoadI64 { value: i64 },
    /// Push an inline 64-bit float.
    LoadF64 { value: f64 },
    /// Push the string at `string_pool[id]`.
    LoadStr { id: StrId },
    /// Push the symbol at `symbol_pool[id]`.
    LoadSym { id: SymId },
    /// Push a boolean.
    LoadBool { value: bool },
    /// Push nil.
    LoadNil,

    // ── Local variables (slot-indexed within a call frame) ────────
    LoadLocal { slot: u16 },
    StoreLocal { slot: u16 },

    // ── Integer arithmetic (pops 2 i64, pushes 1 i64) ─────────────
    AddI64,
    SubI64,
    MulI64,
    DivI64,

    // ── String concat (pops 2 Str, pushes 1 Str) ──────────────────
    ConcatStr,

    // ── Typed comparisons (pop 2 i64, push 1 bool) ────────────────
    EqI64,
    NeI64,
    LtI64,
    LeI64,
    GtI64,
    GeI64,

    // ── Control flow. `offset` is relative to the next instruction. ─
    Jump { offset: i32 },
    JumpIfFalse { offset: i32 },
    JumpIfTrue { offset: i32 },

    // ── Calls. `argc` arguments are on the stack below the call. ──
    CallUser { fn_id: UserFnId, argc: u8 },
    CallRt { rt_id: RtFnId, argc: u8 },
    Return,

    // ── Stack manipulation ────────────────────────────────────────
    Pop,
    Dup,

    // ── Collection constructors (pop N values, push collection) ───
    NewArray { len: u16 },
    /// Pops `entries` key/value pairs (so `2 * entries` stack values).
    NewHash { entries: u16 },
    /// Pops index, then collection; pushes element.
    IndexLoad,
    /// Pops value, then index, then collection; writes element. No push.
    IndexStore,

    // ── String interpolation. Pops `parts` values (already converted
    //    to Str); concatenates in order; pushes the result. ──────────
    InterpStr { parts: u16 },
}

/// A user-defined function: controller action, model method, view,
/// partial body, lambda body.
///
/// `code_offset` indexes into [`Program::code`] (instruction index,
/// not byte offset — the `Vec<Op>` is position-addressable).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserFn {
    pub name: String,
    pub code_offset: u32,
    pub arity: u8,
    pub locals_count: u16,
}

/// A complete roundhouse bytecode program — the artifact the bytecode
/// emitter produces from a Rails app and the VM consumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub format_version: u32,
    pub string_pool: Vec<String>,
    pub symbol_pool: Vec<String>,
    /// Names of runtime functions this program calls. The VM maps each
    /// name to a Rust function pointer at load time; `RtFnId(i)` then
    /// dispatches to `runtime_fns[i]` without a hashmap lookup per call.
    pub runtime_fns: Vec<String>,
    pub user_fns: Vec<UserFn>,
    /// Flat instruction stream. Branches and call offsets index into
    /// this vec.
    pub code: Vec<Op>,
}

impl Program {
    pub fn new() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            string_pool: Vec::new(),
            symbol_pool: Vec::new(),
            runtime_fns: Vec::new(),
            user_fns: Vec::new(),
            code: Vec::new(),
        }
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}
