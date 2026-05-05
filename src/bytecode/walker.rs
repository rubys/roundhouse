//! IR → bytecode walker (M3a).
//!
//! Given a typed [`Expr`], emit a sequence of [`Op`]s that leaves the
//! expression's value on the operand stack. The walker maintains the
//! constant pools and local-variable scope; the caller owns the
//! resulting [`Program`].
//!
//! M3a scope: the subset of `ExprNode` whose bytecode maps cleanly to
//! the M2 VM's supported opcodes —
//!
//! - `Lit` (all `Literal` variants)
//! - `Var` (load local)
//! - `Let` (bind local, emit body)
//! - `Seq` (emit each, pop intermediates)
//! - `If` (conditional with forward branches patched after emit)
//! - `Assign` to `LValue::Var` (update local, leave value on stack)
//! - `Send` where the receiver is typed `Int` and the method is one of
//!   `+` `-` `*` `/` (emits the corresponding `*I64` opcode) or
//!   `==` `!=` `<` `<=` `>` `>=` (emits the corresponding comparison)
//!
//! Everything else returns [`WalkError::NotYetSupported`] with enough
//! context for diagnostics, keeping the "tests catch accidental
//! reliance" discipline established in M2. BoolOp, string concat,
//! Send to user methods, Ivar/Const access, collections,
//! interpolation, Lambda/Apply, Case, and exception handling all wait
//! for M3b and later.
//!
//! Forward branches use patch-after-emit: emit `JumpIfFalse { offset: 0 }`
//! as a placeholder, emit the branch target, then rewrite the offset
//! once the target position is known.

use std::collections::HashMap;

use crate::bytecode::format::{Op, Program, StrId, SymId, UserFn, UserFnId};
use crate::expr::{BoolOpKind, Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::ty::Ty;

/// Errors the walker can surface. `NotYetSupported` carries an
/// owned string describing what was encountered — more useful than a
/// static label here since the walker frequently needs to identify
/// *which* method or node variant tripped the rejection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkError {
    NotYetSupported(String),
    UnboundVariable(VarId),
}

/// Walker state — constant pools accumulated during emission, local
/// variable scope (`VarId → slot`), the growing code buffer, and the
/// user-function table being assembled as `declare_user_fn` /
/// `begin_user_fn_body` / `end_user_fn_body` are called.
pub struct Walker {
    string_pool: Vec<String>,
    symbol_pool: Vec<String>,
    code: Vec<Op>,
    locals: HashMap<VarId, u16>,
    next_slot: u16,
    user_fns: Vec<UserFn>,
    /// Saved (locals, next_slot) pairs for nested `begin_user_fn_body`
    /// calls. Pushed on enter, popped on exit; restores the caller's
    /// scope cleanly without assuming top-level-only emission.
    saved_states: Vec<(HashMap<VarId, u16>, u16)>,
}

impl Walker {
    pub fn new() -> Self {
        Self {
            string_pool: Vec::new(),
            symbol_pool: Vec::new(),
            code: Vec::new(),
            locals: HashMap::new(),
            next_slot: 0,
            user_fns: Vec::new(),
            saved_states: Vec::new(),
        }
    }

    /// Emit a raw opcode. Exposed so callers (tests, higher-level
    /// emitters) can splice in control flow or calls that don't
    /// correspond to a single `Expr` node.
    pub fn emit(&mut self, op: Op) {
        self.code.push(op);
    }

    /// Reserve a slot in the user-function table and return its id.
    /// `code_offset` and `locals_count` are filled in later by the
    /// matching `begin_user_fn_body` / `end_user_fn_body` pair. This
    /// split lets top-level code emit `CallUser` against a function
    /// whose body hasn't been emitted yet — the common pattern when
    /// the entry point calls functions that are defined further down
    /// in the code section.
    pub fn declare_user_fn(&mut self, name: String, arity: u8) -> UserFnId {
        let id = self.user_fns.len() as u16;
        self.user_fns.push(UserFn {
            name,
            code_offset: 0,
            arity,
            locals_count: 0,
        });
        UserFnId(id)
    }

    /// Begin emitting a function body. Records the current code
    /// position as the function's `code_offset`, saves the caller's
    /// local scope, and binds each `param_id` to a fresh slot
    /// (0..arity). Pair every call with `end_user_fn_body`.
    pub fn begin_user_fn_body(&mut self, id: UserFnId, param_ids: &[VarId]) {
        let idx = id.0 as usize;
        self.user_fns[idx].code_offset = self.code.len() as u32;

        // Save caller's scope, reset to empty for the function body.
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_next_slot = std::mem::replace(&mut self.next_slot, 0);
        self.saved_states.push((saved_locals, saved_next_slot));

        // Bind params to slots 0..arity.
        for (i, vid) in param_ids.iter().enumerate() {
            self.locals.insert(*vid, i as u16);
        }
        self.next_slot = param_ids.len() as u16;
    }

    /// Finalize a function body. Records the `locals_count` (the
    /// number of slots allocated during body emission) and restores
    /// the caller's local scope.
    pub fn end_user_fn_body(&mut self, id: UserFnId) {
        let idx = id.0 as usize;
        self.user_fns[idx].locals_count = self.next_slot;

        let (prev_locals, prev_next_slot) = self
            .saved_states
            .pop()
            .expect("end_user_fn_body without matching begin_user_fn_body");
        self.locals = prev_locals;
        self.next_slot = prev_next_slot;
    }

    /// Total locals allocated during this walk — the count a VM frame
    /// (or M2's `Vm::with_locals`) should pre-allocate.
    pub fn locals_count(&self) -> u16 {
        self.next_slot
    }

    /// Consume the walker and produce a `Program` holding the
    /// accumulated code, pools, and user-function table.
    pub fn into_program(self) -> Program {
        let mut p = Program::new();
        p.string_pool = self.string_pool;
        p.symbol_pool = self.symbol_pool;
        p.code = self.code;
        p.user_fns = self.user_fns;
        p
    }

    /// Walk an expression, emitting instructions that leave its value
    /// on the operand stack at the walker's current position.
    pub fn walk(&mut self, expr: &Expr) -> Result<(), WalkError> {
        match &*expr.node {
            ExprNode::Lit { value } => self.walk_lit(value),
            ExprNode::Var { id, .. } => self.walk_var(*id),
            ExprNode::Let {
                id, value, body, ..
            } => self.walk_let(*id, value, body),
            ExprNode::Seq { exprs } => self.walk_seq(exprs),
            ExprNode::If {
                cond,
                then_branch,
                else_branch,
            } => self.walk_if(cond, then_branch, else_branch),
            ExprNode::Assign { target, value } => self.walk_assign(target, value),
            ExprNode::Send {
                recv,
                method,
                args,
                ..
            } => self.walk_send(recv.as_ref(), method, args),
            ExprNode::BoolOp {
                op, left, right, ..
            } => self.walk_bool_op(*op, left, right),

            // Everything below waits for M3c+.
            other => Err(WalkError::NotYetSupported(format!(
                "ExprNode variant: {}",
                node_kind(other)
            ))),
        }
    }

    // ── Leaf walkers ─────────────────────────────────────────────

    fn walk_lit(&mut self, lit: &Literal) -> Result<(), WalkError> {
        match lit {
            Literal::Nil => self.emit(Op::LoadNil),
            Literal::Bool { value } => self.emit(Op::LoadBool { value: *value }),
            Literal::Int { value } => self.emit(Op::LoadI64 { value: *value }),
            Literal::Float { value } => self.emit(Op::LoadF64 { value: *value }),
            Literal::Str { value } => {
                let id = self.intern_string(value.clone());
                self.emit(Op::LoadStr { id });
            }
            Literal::Sym { value } => {
                let id = self.intern_symbol(value.as_str().to_string());
                self.emit(Op::LoadSym { id });
            }
            Literal::Regex { .. } => {
                return Err(WalkError::NotYetSupported(
                    "Literal::Regex in bytecode walker".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn walk_var(&mut self, id: VarId) -> Result<(), WalkError> {
        let slot = self
            .locals
            .get(&id)
            .copied()
            .ok_or(WalkError::UnboundVariable(id))?;
        self.emit(Op::LoadLocal { slot });
        Ok(())
    }

    fn walk_let(&mut self, id: VarId, value: &Expr, body: &Expr) -> Result<(), WalkError> {
        // Emit value (pushes V on stack).
        self.walk(value)?;
        // Bind the name to a fresh slot, store V there (consumes V).
        let slot = self.alloc_slot();
        self.locals.insert(id, slot);
        self.emit(Op::StoreLocal { slot });
        // The body's value becomes the let-expression's value.
        self.walk(body)
    }

    fn walk_seq(&mut self, exprs: &[Expr]) -> Result<(), WalkError> {
        if exprs.is_empty() {
            // Empty sequence: push nil so callers always see one value.
            self.emit(Op::LoadNil);
            return Ok(());
        }
        let last = exprs.len() - 1;
        for (i, e) in exprs.iter().enumerate() {
            self.walk(e)?;
            if i < last {
                // Every walked expression leaves one value on the stack;
                // non-final results are discarded.
                self.emit(Op::Pop);
            }
        }
        Ok(())
    }

    fn walk_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
    ) -> Result<(), WalkError> {
        // Emit the condition.
        self.walk(cond)?;

        // Placeholder: jump to the else branch if false.
        let jif_pos = self.code.len();
        self.emit(Op::JumpIfFalse { offset: 0 });

        // Then branch.
        self.walk(then_branch)?;

        // Placeholder: after the then branch, jump past the else branch.
        let jend_pos = self.code.len();
        self.emit(Op::Jump { offset: 0 });

        // Else branch starts here — patch the JumpIfFalse to land here.
        let else_start = self.code.len();
        self.patch_branch(jif_pos, else_start);

        self.walk(else_branch)?;

        // End of if — patch the unconditional jump to land here.
        let end = self.code.len();
        self.patch_branch(jend_pos, end);

        Ok(())
    }

    fn walk_assign(&mut self, target: &LValue, value: &Expr) -> Result<(), WalkError> {
        match target {
            LValue::Var { id, .. } => {
                self.walk(value)?;
                // Ruby: assignment evaluates to the assigned value, so
                // we leave a copy on the stack via `Dup` before `StoreLocal`
                // consumes one.
                self.emit(Op::Dup);
                let slot = match self.locals.get(id) {
                    Some(s) => *s,
                    None => {
                        let s = self.alloc_slot();
                        self.locals.insert(*id, s);
                        s
                    }
                };
                self.emit(Op::StoreLocal { slot });
                Ok(())
            }
            LValue::Ivar { .. } => Err(WalkError::NotYetSupported("assign to ivar".into())),
            LValue::Attr { .. } => Err(WalkError::NotYetSupported("assign to attr".into())),
            LValue::Index { .. } => Err(WalkError::NotYetSupported("assign to index[]".into())),
        }
    }

    /// Short-circuit `&&` / `||` via the `Dup` + conditional-jump +
    /// `Pop` idiom. The VM's `JumpIfFalse` / `JumpIfTrue` pop their
    /// operand, so we `Dup` first to keep a copy of the left-hand
    /// value around as the result when short-circuit fires.
    ///
    /// For M3b we require both operands to be `Bool`-typed (the VM's
    /// conditional jumps `pop_bool()`, so a non-`Bool` left-hand side
    /// would produce `TypeMismatch` at runtime). Ruby's truthy-aware
    /// `&&`/`||` over mixed-type operands waits for a `Truthy`
    /// opcode or a lifted comparison form — lifting opportunity, not
    /// a walker responsibility.
    ///
    /// Stack trace for `a && b`:
    ///
    /// ```text
    ///   <a>                    stack: [a]
    ///   Dup                    stack: [a, a]
    ///   JumpIfFalse end        pops top; if !a, jump — stack: [a]
    ///   Pop                    (only runs when a is true) — stack: []
    ///   <b>                    stack: [b]
    ///   end:                   result is a (if short-circuit) or b
    /// ```
    ///
    /// `||` is the dual via `JumpIfTrue`.
    fn walk_bool_op(
        &mut self,
        op: BoolOpKind,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), WalkError> {
        self.require_bool(left, "BoolOp left")?;
        self.require_bool(right, "BoolOp right")?;

        self.walk(left)?;
        self.emit(Op::Dup);
        let branch_pos = self.code.len();
        self.emit(match op {
            BoolOpKind::And => Op::JumpIfFalse { offset: 0 },
            BoolOpKind::Or => Op::JumpIfTrue { offset: 0 },
        });
        self.emit(Op::Pop);
        self.walk(right)?;
        let end = self.code.len();
        self.patch_branch(branch_pos, end);
        Ok(())
    }

    /// Assert an expression's type is `Bool`. Used by `walk_bool_op`
    /// to surface missing lifts (e.g., Ruby-truthy `1 && 2` shapes)
    /// rather than fail at runtime with a VM `TypeMismatch`.
    fn require_bool(&self, e: &Expr, context: &str) -> Result<(), WalkError> {
        match e.ty.as_ref() {
            Some(Ty::Bool) => Ok(()),
            Some(other) => Err(WalkError::NotYetSupported(format!(
                "{}: non-Bool operand (got {:?}) — needs truthy-aware lift",
                context, other
            ))),
            None => Err(WalkError::NotYetSupported(format!(
                "{}: operand has no type (analyzer didn't type it)",
                context
            ))),
        }
    }

    fn walk_send(
        &mut self,
        recv: Option<&Expr>,
        method: &Symbol,
        args: &[Expr],
    ) -> Result<(), WalkError> {
        // M3a handles exactly one shape: typed integer binary ops.
        // Anything else defers to M3b+.

        let method_str = method.as_str();
        let recv = recv.ok_or_else(|| {
            WalkError::NotYetSupported(format!("Send without receiver: method={}", method_str))
        })?;
        let recv_ty = recv.ty.as_ref().ok_or_else(|| {
            WalkError::NotYetSupported(format!(
                "Send whose receiver has no type (analyzer didn't type it): method={}",
                method_str
            ))
        })?;

        // Binary op on Int receiver with one Int arg.
        if matches!(recv_ty, Ty::Int) && args.len() == 1 {
            if let Some(op) = integer_arithmetic_op(method_str) {
                self.walk(recv)?;
                self.walk(&args[0])?;
                self.emit(op);
                return Ok(());
            }
            if let Some(op) = integer_comparison_op(method_str) {
                self.walk(recv)?;
                self.walk(&args[0])?;
                self.emit(op);
                return Ok(());
            }
        }

        Err(WalkError::NotYetSupported(format!(
            "Send: recv_ty={:?}, method={}, argc={}",
            recv_ty,
            method_str,
            args.len()
        )))
    }

    // ── Helpers ──────────────────────────────────────────────────

    fn alloc_slot(&mut self) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        slot
    }

    fn intern_string(&mut self, s: String) -> StrId {
        // Simple intern: linear search. Pools are small in practice
        // and this keeps the walker allocation-light. Optimize if it
        // ever matters.
        if let Some(idx) = self.string_pool.iter().position(|existing| existing == &s) {
            return StrId(idx as u32);
        }
        let idx = self.string_pool.len() as u32;
        self.string_pool.push(s);
        StrId(idx)
    }

    fn intern_symbol(&mut self, s: String) -> SymId {
        if let Some(idx) = self.symbol_pool.iter().position(|existing| existing == &s) {
            return SymId(idx as u32);
        }
        let idx = self.symbol_pool.len() as u32;
        self.symbol_pool.push(s);
        SymId(idx)
    }

    /// Rewrite the branch op at `branch_pos` so its offset lands at
    /// `target_pc`. Offsets are relative to the instruction *after*
    /// the branch, matching the VM's convention.
    fn patch_branch(&mut self, branch_pos: usize, target_pc: usize) {
        let offset = (target_pc as i64) - (branch_pos as i64 + 1);
        let offset = offset as i32;
        self.code[branch_pos] = match &self.code[branch_pos] {
            Op::Jump { .. } => Op::Jump { offset },
            Op::JumpIfFalse { .. } => Op::JumpIfFalse { offset },
            Op::JumpIfTrue { .. } => Op::JumpIfTrue { offset },
            other => panic!("patch_branch on non-branch op {:?}", other),
        };
    }
}

impl Default for Walker {
    fn default() -> Self {
        Self::new()
    }
}

fn integer_arithmetic_op(method: &str) -> Option<Op> {
    match method {
        "+" => Some(Op::AddI64),
        "-" => Some(Op::SubI64),
        "*" => Some(Op::MulI64),
        "/" => Some(Op::DivI64),
        _ => None,
    }
}

fn integer_comparison_op(method: &str) -> Option<Op> {
    match method {
        "==" => Some(Op::EqI64),
        "!=" => Some(Op::NeI64),
        "<" => Some(Op::LtI64),
        "<=" => Some(Op::LeI64),
        ">" => Some(Op::GtI64),
        ">=" => Some(Op::GeI64),
        _ => None,
    }
}

/// Human-readable `ExprNode` kind name for error messages.
fn node_kind(node: &ExprNode) -> &'static str {
    match node {
        ExprNode::Lit { .. } => "Lit",
        ExprNode::Var { .. } => "Var",
        ExprNode::Ivar { .. } => "Ivar",
        ExprNode::Const { .. } => "Const",
        ExprNode::Hash { .. } => "Hash",
        ExprNode::Array { .. } => "Array",
        ExprNode::StringInterp { .. } => "StringInterp",
        ExprNode::BoolOp { .. } => "BoolOp",
        ExprNode::Let { .. } => "Let",
        ExprNode::Lambda { .. } => "Lambda",
        ExprNode::Apply { .. } => "Apply",
        ExprNode::Send { .. } => "Send",
        ExprNode::If { .. } => "If",
        ExprNode::Case { .. } => "Case",
        ExprNode::Seq { .. } => "Seq",
        ExprNode::Assign { .. } => "Assign",
        ExprNode::Yield { .. } => "Yield",
        ExprNode::Raise { .. } => "Raise",
        ExprNode::RescueModifier { .. } => "RescueModifier",
        ExprNode::SelfRef => "SelfRef",
        ExprNode::Return { .. } => "Return",
        ExprNode::Super { .. } => "Super",
        ExprNode::BeginRescue { .. } => "BeginRescue",
        ExprNode::Next { .. } => "Next",
        ExprNode::MultiAssign { .. } => "MultiAssign",
        ExprNode::While { .. } => "While",
        ExprNode::Range { .. } => "Range",
        ExprNode::Cast { .. } => "Cast",
    }
}
