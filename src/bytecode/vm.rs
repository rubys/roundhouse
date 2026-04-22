//! Minimal VM executing roundhouse bytecode (M2).
//!
//! Scope: arithmetic (i64), locals, typed comparisons, conditional and
//! unconditional branches, stack manipulation, string literal loads.
//! Enough to run hand-written programs that exercise the core dispatch
//! shape.
//!
//! Deliberately out of scope for M2: function calls (`CallUser`,
//! `CallRt`), collections (`NewArray`, `NewHash`, `IndexLoad`,
//! `IndexStore`), string concatenation and interpolation. These
//! opcodes return `VmError::NotYetSupported` with the opcode name so
//! tests catch accidental reliance.
//!
//! Design choices baked in here:
//!
//! - **Tagged `Value` enum.** Each stack slot holds a typed `Value`.
//!   Simple and obviously correct; variant experiments between M2 and
//!   M3 can explore separate-stacks-per-type or universal-word layouts
//!   if perf numbers argue for them.
//! - **Vec-based operand stack and locals.** Grows as needed; no
//!   pre-allocated size. Stack overflow is bounded by process memory
//!   today — adding an explicit limit is a trivial follow-up when
//!   deployment matters.
//! - **`&mut self` dispatch loop with cloned ops.** Each `Op` enum
//!   value fits in a small fixed size (~16 bytes, no heap fields), so
//!   cloning per dispatch is essentially a memcpy. Avoids the borrow
//!   dance of matching on a reference while also writing to `self.pc`.
//!   Direct threading / computed-goto dispatch is one of the variant
//!   experiments to revisit in Phase B.

use crate::bytecode::format::{Op, Program};

/// A runtime value produced by or consumed by the VM. Every stack
/// slot, local slot, and constant pool load materializes as one of
/// these. Tagged enum with one variant per supported type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Nil,
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::Str(_) => "Str",
            Value::Nil => "Nil",
        }
    }
}

/// Errors the VM can surface. Static-strlen where possible to keep
/// the error type cheap to construct in dispatch-hot paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    StackUnderflow,
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
    },
    InvalidLocalSlot(u16),
    InvalidStringId(u32),
    InvalidSymbolId(u32),
    /// `CallUser` referenced a `UserFnId` outside `Program::user_fns`.
    InvalidUserFnId(u16),
    /// `CallUser` argc didn't match the target function's arity.
    ArityMismatch {
        expected: u8,
        got: u8,
    },
    DivisionByZero,
    PcOutOfBounds(i64),
    /// Opcode recognized by the format but not yet implemented by this
    /// VM. Carries the opcode's human-readable name for diagnostics.
    NotYetSupported(&'static str),
}

/// One activation record on the VM's call stack. Each user function
/// call pushes a `Frame` holding its own locals and the return address.
/// The operand stack is shared across frames — arguments flow in via
/// it, return values flow out the same way.
#[derive(Clone, Debug)]
struct Frame {
    /// Locals for this call. Parameters occupy the first `arity`
    /// slots; additional slots are initialized to `Nil`.
    locals: Vec<Value>,
    /// Where to resume execution in the caller after `Return`. Always
    /// points at the instruction *after* the caller's `CallUser`
    /// (matches the convention `pc` has already been advanced by the
    /// dispatch loop before the opcode body runs).
    return_pc: usize,
}

/// Minimal stack-based VM. Holds a borrow of the program and its own
/// mutable dispatch state (operand stack, call-frame stack, pc).
pub struct Vm<'p> {
    program: &'p Program,
    stack: Vec<Value>,
    /// Call-frame stack. The last entry is the current frame. Always
    /// contains at least one frame — the entry frame — after `new`.
    frames: Vec<Frame>,
    pc: usize,
}

impl<'p> Vm<'p> {
    pub fn new(program: &'p Program) -> Self {
        Self {
            program,
            stack: Vec::new(),
            // Start with an empty entry frame. `with_locals` can replace
            // its `locals` vector if the top-level code needs slots.
            frames: vec![Frame {
                locals: Vec::new(),
                return_pc: 0,
            }],
            pc: 0,
        }
    }

    /// Pre-allocate `count` local slots in the entry frame, initialized
    /// to `Nil`. For the M3c+ call-frame world, this sizes the slots
    /// used by top-level (pre-`CallUser`) code; subsequent calls get
    /// their own frame with its own locals sized from the target
    /// `UserFn::locals_count`.
    pub fn with_locals(mut self, count: usize) -> Self {
        self.frames[0].locals = vec![Value::Nil; count];
        self
    }

    /// Execute instructions from the current `pc` until `Return` or
    /// end-of-code. Returns the top of the operand stack at that
    /// point, or `None` if the stack was empty.
    pub fn run(&mut self) -> Result<Option<Value>, VmError> {
        loop {
            if self.pc >= self.program.code.len() {
                // Implicit end-of-code: no more instructions. Return
                // whatever's on top of the stack, if anything.
                return Ok(self.stack.pop());
            }
            // Clone is cheap: Op has no heap-allocated fields, so this
            // is a small memcpy. Keeping the clone lets us freely
            // mutate `self.pc` while dispatching.
            let op = self.program.code[self.pc].clone();
            self.pc += 1;

            match op {
                // ── Literal pushes ────────────────────────────────
                Op::LoadI64 { value } => self.stack.push(Value::Int(value)),
                Op::LoadF64 { value } => self.stack.push(Value::Float(value)),
                Op::LoadBool { value } => self.stack.push(Value::Bool(value)),
                Op::LoadNil => self.stack.push(Value::Nil),
                Op::LoadStr { id } => {
                    let s = self
                        .program
                        .string_pool
                        .get(id.0 as usize)
                        .ok_or(VmError::InvalidStringId(id.0))?;
                    self.stack.push(Value::Str(s.clone()));
                }
                Op::LoadSym { id } => {
                    // M2 doesn't distinguish Sym from Str at the value
                    // level; symbols render as their string name.
                    let s = self
                        .program
                        .symbol_pool
                        .get(id.0 as usize)
                        .ok_or(VmError::InvalidSymbolId(id.0))?;
                    self.stack.push(Value::Str(s.clone()));
                }

                // ── Locals (always in the current frame) ──────────
                Op::LoadLocal { slot } => {
                    let frame = self.current_frame()?;
                    let v = frame
                        .locals
                        .get(slot as usize)
                        .ok_or(VmError::InvalidLocalSlot(slot))?
                        .clone();
                    self.stack.push(v);
                }
                Op::StoreLocal { slot } => {
                    let v = self.pop()?;
                    let frame = self.current_frame_mut()?;
                    if (slot as usize) >= frame.locals.len() {
                        return Err(VmError::InvalidLocalSlot(slot));
                    }
                    frame.locals[slot as usize] = v;
                }

                // ── Integer arithmetic ────────────────────────────
                Op::AddI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Int(a.wrapping_add(b)));
                }
                Op::SubI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Int(a.wrapping_sub(b)));
                }
                Op::MulI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Int(a.wrapping_mul(b)));
                }
                Op::DivI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    if b == 0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.stack.push(Value::Int(a.wrapping_div(b)));
                }

                // ── Typed comparisons (i64) ───────────────────────
                Op::EqI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a == b));
                }
                Op::NeI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a != b));
                }
                Op::LtI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a < b));
                }
                Op::LeI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a <= b));
                }
                Op::GtI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a > b));
                }
                Op::GeI64 => {
                    let (a, b) = self.pop_two_i64()?;
                    self.stack.push(Value::Bool(a >= b));
                }

                // ── Control flow ──────────────────────────────────
                Op::Jump { offset } => self.branch(offset)?,
                Op::JumpIfFalse { offset } => {
                    if !self.pop_bool()? {
                        self.branch(offset)?;
                    }
                }
                Op::JumpIfTrue { offset } => {
                    if self.pop_bool()? {
                        self.branch(offset)?;
                    }
                }

                // ── Stack manipulation ────────────────────────────
                Op::Pop => {
                    self.pop()?;
                }
                Op::Dup => {
                    let top = self.stack.last().ok_or(VmError::StackUnderflow)?.clone();
                    self.stack.push(top);
                }

                Op::Return => {
                    // At the bottom frame this is the program's final
                    // return: yield the top of the operand stack as
                    // the run result. Otherwise pop the current frame
                    // and resume at the caller's `return_pc`.
                    if self.frames.len() <= 1 {
                        return Ok(self.stack.pop());
                    }
                    let popped = self.frames.pop().expect("frames nonempty");
                    self.pc = popped.return_pc;
                }

                // ── User-defined function calls ───────────────────
                Op::CallUser { fn_id, argc } => {
                    let user_fn = self
                        .program
                        .user_fns
                        .get(fn_id.0 as usize)
                        .ok_or(VmError::InvalidUserFnId(fn_id.0))?;
                    if user_fn.arity != argc {
                        return Err(VmError::ArityMismatch {
                            expected: user_fn.arity,
                            got: argc,
                        });
                    }
                    // Pop arguments off the operand stack in reverse
                    // push order, so `arg[0]` lands in local slot 0.
                    let mut args = Vec::with_capacity(argc as usize);
                    for _ in 0..argc {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    // Build the callee's frame: locals sized from the
                    // function's declaration, arguments written into
                    // slots 0..argc, remainder `Nil`.
                    let locals_count = user_fn.locals_count as usize;
                    let mut locals = vec![Value::Nil; locals_count.max(argc as usize)];
                    for (i, v) in args.into_iter().enumerate() {
                        locals[i] = v;
                    }

                    self.frames.push(Frame {
                        locals,
                        return_pc: self.pc, // already advanced past CallUser
                    });
                    self.pc = user_fn.code_offset as usize;
                }

                // ── Deferred to later milestones ──────────────────
                Op::CallRt { .. } => return Err(VmError::NotYetSupported("call_rt")),
                Op::ConcatStr => return Err(VmError::NotYetSupported("concat_str")),
                Op::NewArray { .. } => return Err(VmError::NotYetSupported("new_array")),
                Op::NewHash { .. } => return Err(VmError::NotYetSupported("new_hash")),
                Op::IndexLoad => return Err(VmError::NotYetSupported("index_load")),
                Op::IndexStore => return Err(VmError::NotYetSupported("index_store")),
                Op::InterpStr { .. } => return Err(VmError::NotYetSupported("interp_str")),
            }
        }
    }

    fn current_frame(&self) -> Result<&Frame, VmError> {
        self.frames.last().ok_or(VmError::StackUnderflow)
    }

    fn current_frame_mut(&mut self) -> Result<&mut Frame, VmError> {
        self.frames.last_mut().ok_or(VmError::StackUnderflow)
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn pop_i64(&mut self) -> Result<i64, VmError> {
        match self.pop()? {
            Value::Int(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "Int",
                got: other.type_name(),
            }),
        }
    }

    /// Pop two i64 values in operand order — the top of the stack was
    /// the right-hand operand, the one below it the left. Returns
    /// `(lhs, rhs)` for readable use sites (`a + b`, `a < b`, …).
    fn pop_two_i64(&mut self) -> Result<(i64, i64), VmError> {
        let rhs = self.pop_i64()?;
        let lhs = self.pop_i64()?;
        Ok((lhs, rhs))
    }

    fn pop_bool(&mut self) -> Result<bool, VmError> {
        match self.pop()? {
            Value::Bool(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "Bool",
                got: other.type_name(),
            }),
        }
    }

    /// Apply a branch offset to `self.pc`. `self.pc` has already been
    /// advanced past the branching instruction at this point, so the
    /// offset is relative to the next sequential instruction.
    fn branch(&mut self, offset: i32) -> Result<(), VmError> {
        let new_pc = self.pc as i64 + offset as i64;
        if new_pc < 0 {
            return Err(VmError::PcOutOfBounds(new_pc));
        }
        self.pc = new_pc as usize;
        Ok(())
    }
}
