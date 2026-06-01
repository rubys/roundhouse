//! Instance `@ivar` mutation → struct-update return-threading (#29 pass #4).
//!
//! Elixir has no mutable instance state, so an instance method that
//! mutates `@ivar`s must thread a `record` and return the updated copy:
//!
//! ```text
//!   def render(body, status: :ok, …)        def render(record, body, status \\ :ok, …)
//!     @body = body                            record = %{record | body: body}
//!     @status = resolve_status(status)  ──▶   record = %{record | status: resolve_status(status)}
//!     @x = v unless v.nil?                     record = if not is_nil(v), do: %{record | x: v}, else: record
//!     nil                                      record
//!   end                                      end
//! ```
//!
//! Mechanics (see [[project_mutation_threading_pass4]] for the design):
//! - `@x` read → `record.x` (a no-arg `Send` on the `record` var).
//! - `@x = v` write → `record = %{record | x: v}`, bridged through a
//!   synthetic `record.__struct_put__(:x, v)` `Send` that the elixir2
//!   emitter renders as the struct-update literal. (Conditional writes
//!   ride the emitter's existing cond-rebind lift — no work here.)
//! - the method returns `record` (a trailing `nil` is replaced).
//!
//! Pure instance methods (no `@ivar`) are left untouched and take no
//! `record` param (the emitter threads `record` only when the body
//! references it). `initialize` is skipped — the constructor path emits
//! it as `new` returning a struct literal.
//!
//! **Dual return.** A method that mutates `@ivar` AND yields a genuine
//! value (`save`/`valid?`: a `return false`, or an implicit
//! `@errors.empty?` tail) can't thread a single `record` return, so it
//! emits a `{record, value}` tuple. The transform threads `record`,
//! rebinds record-threading self-calls (`validate` → `record =
//! validate(record)`), lifts record-mutating `if`s so the mutations
//! leak past them (`save`'s `new_record?` insert/update split), and
//! wraps every exit value (`return v` → `return {record, v}`, trailing
//! `v` → `{record, v}`). Call sites destructure: `ok = save` → `{record,
//! ok} = save(record)`, `if save do …` → `{record, ok} = save(record);
//! if ok do …`, threading a typed local (`instance`) for a polymorphic
//! dispatch (`instance.save` in `create`).
//!
//! **Scope.** Flat + conditional `@ivar = scalar`. Bails (returns the
//! method unchanged) on `while`/`yield` outside the cases while→recursion
//! handles.

use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

const RECORD: &str = "record";

/// Per-class classification of which instance methods thread `record`,
/// so a self-call can be rebound at the call site. Computed once per
/// class (on the post-while-recursion method list) and threaded into
/// every `transform_method` call.
#[derive(Default, Clone)]
pub struct Registry {
    /// Methods that mutate `record` and return it (no value return) —
    /// a self-call statement `m` becomes `record = m(record)`.
    pub record_returning: std::collections::HashSet<String>,
    /// Mutate-AND-return-value methods (`save`/`valid?`) that emit a
    /// `{record, value}` tuple — a self-call destructures the tuple
    /// (`{record, ok} = m(record)` / `{record, _} = m(record)`).
    pub dual_return: std::collections::HashSet<String>,
}

impl Registry {
    fn is_record_returning(&self, name: &str) -> bool {
        self.record_returning.contains(name)
    }
    fn is_dual(&self, name: &str) -> bool {
        self.dual_return.contains(name)
    }
}

/// Classify a class's instance methods (post-while-recursion) into the
/// record-returning / dual-return registry. A method "returns record"
/// when it mutates `record` and has no genuine value return; it's
/// "dual" when it both mutates and returns a value (`save`/`valid?`).
/// `*__loop` helpers and `initialize` are excluded (handled elsewhere).
pub fn compute_registry(methods: &[MethodDef]) -> Registry {
    let mut reg = Registry::default();
    for m in methods {
        if m.receiver == MethodReceiver::Class
            || m.name.as_str() == "initialize"
            || m.name.as_str().ends_with("__loop")
            || !mutates_record(&m.body)
        {
            continue;
        }
        if returns_genuine_value(&m.body) {
            reg.dual_return.insert(m.name.to_string());
        } else {
            reg.record_returning.insert(m.name.to_string());
        }
    }
    reg
}

/// Rewrite one method for struct-return threading, or return it
/// unchanged when it isn't an applicable instance mutator.
pub fn transform_method(mut m: MethodDef, reg: &Registry) -> MethodDef {
    // A class method doesn't thread `record`, but its calls to dual
    // instance methods on a typed local (`instance.save` in `create`)
    // must still destructure the `{record, value}` tuple.
    if m.receiver == MethodReceiver::Class {
        m.body = rebind_record_calls(&m.body, reg);
        return m;
    }
    if !should_thread(&m) {
        return m;
    }
    // Mutate-and-return-value (`save`/`valid?`/flash#delete) — a single
    // struct return can't carry both the mutated record and the value,
    // so emit a `{record, value}` tuple (callers destructure it).
    if mutates_record(&m.body) && returns_genuine_value(&m.body) {
        // Emit a `{record, value}` tuple: thread `record` through the
        // body, lift record-mutating `if`s so the mutations leak past
        // them, and wrap each exit value. Callers destructure the tuple.
        return rewrite_dual_return(m, reg);
    }
    rewrite(m, reg)
}

/// True when a mutating method ALSO yields a genuine value — so it can't
/// thread to a single `record` return and must emit a `{record, value}`
/// tuple. Two sources: an explicit `return <value>` (`save`'s `return
/// false`), or a trailing value position that's a real computed value
/// rather than `self`/`nil`/a mutation (`valid?`'s implicit
/// `@errors.empty?`).
fn returns_genuine_value(e: &Expr) -> bool {
    has_value_return(e) || tail_is_genuine_value(e)
}

/// The trailing (value-producing) expression of a body — the last
/// statement of a `Seq`, recursively.
fn tail(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Seq { exprs } => exprs.last().map(tail).unwrap_or(e),
        _ => e,
    }
}

/// True when the body's terminal value is a genuine computed value — a
/// query call on a non-self receiver (`@errors.empty?`), a non-nil
/// literal (`true`), or a boolean expression. Conservatively false for
/// everything else: `self`/`nil`, a bare local read (`result` — a
/// record-ish carry), a self-call (`self.save` — threaded via
/// rebinding), a mutation (`@x =`, `errors <<`). Descends `if` tails.
fn tail_is_genuine_value(e: &Expr) -> bool {
    match &*tail(e).node {
        ExprNode::Lit { value: Literal::Nil } => false,
        ExprNode::Lit { .. } => true,
        ExprNode::BoolOp { .. } => true,
        ExprNode::Send { recv: Some(r), method, .. } => {
            // A query (`.empty?`/`.nil?`/`==`) on a real receiver is a
            // value; a self-call (`self.save`) or a mutation Send (`<<`,
            // `[]=`, attr-writer `x=`) is not.
            let m = method.as_str();
            !matches!(&*r.node, ExprNode::SelfRef)
                && m != "<<"
                && m != "[]="
                && !(m.ends_with('=') && !matches!(m, "==" | "!=" | ">=" | "<=" | "==="))
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            tail_is_genuine_value(then_branch) || tail_is_genuine_value(else_branch)
        }
        _ => false,
    }
}

/// True if the body has an explicit `return <value>` that's neither `nil`
/// nor `self`. `return self` / `return nil` are fine: `self` is the
/// record, `nil` is superseded by the threaded record.
fn has_value_return(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if let ExprNode::Return { value } = &*n.node {
            if !matches!(
                &*value.node,
                ExprNode::Lit { value: Literal::Nil } | ExprNode::SelfRef
            ) {
                found = true;
            }
        }
    });
    found
}

/// Thread a constructor (`initialize`) body for struct-update emit: the
/// `@field = v` writes become `record` updates and the body returns
/// `record`. The caller (elixir2's `emit_constructor`) seeds
/// `record = %Struct{}` ahead of this. Used only for non-flat
/// constructors (flat `@field = value` ones emit a struct literal
/// directly).
pub fn thread_constructor_body(body: &Expr) -> Expr {
    let rewritten = rewrite_expr(body);
    // A constructor always returns `record`. When the tail is a
    // control-flow `if` (e.g. session's `return if other.nil?` followed
    // by a record-threading populate loop), its value carries the
    // threaded struct — bind it back to `record`, mapping each branch's
    // trailing `nil` (a rewritten bare `return self`) to the unchanged
    // `record`, then return `record`.
    if let ExprNode::Seq { exprs } = &*rewritten.node {
        if let Some(last) = exprs.last() {
            if matches!(&*last.node, ExprNode::If { .. }) && !is_guard_if(last) {
                let mut head: Vec<Expr> = exprs[..exprs.len() - 1].to_vec();
                head.push(syn(ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                    value: tail_nil_to_record(last),
                }));
                head.push(var(RECORD));
                return syn(ExprNode::Seq { exprs: head });
            }
        }
    }
    append_record_return(rewritten)
}

/// Map an expression's trailing value position so a bare `nil` (a
/// rewritten bare `return self` in a constructor) yields `record`.
/// Descends `Seq` tails and both `If` branches; leaves any other tail
/// (e.g. a record-threading loop call) untouched.
fn tail_nil_to_record(e: &Expr) -> Expr {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => var(RECORD),
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            if let Some(last) = exprs.pop() {
                exprs.push(tail_nil_to_record(&last));
            }
            syn(ExprNode::Seq { exprs })
        }
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: tail_nil_to_record(then_branch),
            else_branch: tail_nil_to_record(else_branch),
        }),
        _ => e.clone(),
    }
}

fn should_thread(m: &MethodDef) -> bool {
    // `initialize` is the constructor (emitted as `new`), not a mutator.
    if m.name.as_str() == "initialize" {
        return false;
    }
    // Must touch instance state (`@ivar` or `self`), and stay in coverage.
    touches_self(&m.body) && !has_nested_mutation_or_loop(&m.body)
}

fn rewrite(mut m: MethodDef, reg: &Registry) -> MethodDef {
    let writes = mutates_record(&m.body);
    let body = rewrite_expr(&m.body);
    // Rebind self-calls to record-threading methods: a bare `validate`
    // statement becomes `record = validate(record)` so the mutation is
    // carried forward (a self-call's return is otherwise discarded).
    let body = rebind_record_calls(&body, reg);
    // A mutator returns the updated record; a read-only method keeps its
    // own value. A while→recursion helper (`*__loop`) already returns
    // `record` through its `if` branches (recurse / post-value), so it
    // must NOT get a trailing `record` appended.
    m.body = if writes && !m.name.as_str().ends_with("__loop") {
        append_record_return(body)
    } else {
        body
    };
    m
}

/// A mutate-and-return-value method (`save`/`valid?`) → a `{record,
/// value}` tuple. Thread `record` through the (linear) body, rebind
/// record-threading self-calls, then wrap every exit value: each
/// `return v` becomes `return {record, v}` and the trailing value `v`
/// becomes `{record, v}`. Callers destructure the tuple.
fn rewrite_dual_return(mut m: MethodDef, reg: &Registry) -> MethodDef {
    let body = rewrite_expr(&m.body);
    let body = rebind_record_calls(&body, reg);
    let body = thread_branches(&body);
    let body = wrap_returns(&body);
    m.body = wrap_tail(&body);
    m
}

/// Lift a non-tail `if` that mutates `record` in its branches into a
/// `record = if cond do <branch …; record> else <branch …; record> end`,
/// so the branch mutations leak past the `if` (Elixir branch scoping
/// otherwise discards a `record =` rebind made inside a branch — this is
/// `save`'s `new_record?` insert/update split). A guard branch (ending
/// in `return`) is left untouched — return-elim handles it at emit.
/// Recurses bottom-up so nested mutating `if`s are lifted first.
fn thread_branches(e: &Expr) -> Expr {
    match &*e.node {
        ExprNode::Seq { exprs } => {
            let n = exprs.len();
            let exprs = exprs
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let s = thread_branches(s);
                    // Only lift a NON-tail `if` (a tail `if` is the value
                    // position — wrap_tail handles it).
                    if i + 1 < n {
                        if let ExprNode::If { cond, then_branch, else_branch } = &*s.node {
                            if (branch_mutates_record(then_branch)
                                || branch_mutates_record(else_branch))
                                && !ends_in_return(then_branch)
                                && !ends_in_return(else_branch)
                            {
                                return syn(ExprNode::Assign {
                                    target: lvar(RECORD),
                                    value: syn(ExprNode::If {
                                        cond: cond.clone(),
                                        then_branch: append_record_yield(then_branch),
                                        else_branch: append_record_yield(else_branch),
                                    }),
                                });
                            }
                        }
                    }
                    s
                })
                .collect();
            syn(ExprNode::Seq { exprs })
        }
        _ => map_children(e, &thread_branches),
    }
}

/// True when a branch contains a `record = …` rebind (or a `{record, …}`
/// destructure) — the signal that it mutates the threaded record and so
/// must yield `record` when its enclosing `if` is lifted.
fn branch_mutates_record(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| match &*n.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name.as_str() == RECORD => {
            found = true
        }
        ExprNode::MultiAssign { targets, .. }
            if targets
                .iter()
                .any(|t| matches!(t, LValue::Var { name, .. } if name.as_str() == RECORD)) =>
        {
            found = true
        }
        _ => {}
    });
    found
}

/// Append a trailing `record` so a (lifted) `if` branch yields the
/// threaded record as its value.
fn append_record_yield(branch: &Expr) -> Expr {
    match &*branch.node {
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            exprs.push(var(RECORD));
            syn(ExprNode::Seq { exprs })
        }
        _ => syn(ExprNode::Seq { exprs: vec![branch.clone(), var(RECORD)] }),
    }
}

// ---- self-call rebinding + tuple wrapping ---------------------------

/// Rewrite the statements of a (threaded) body so self-calls to
/// record-threading methods rebind `record`. Recurses into `if`/`while`
/// branch sequences.
fn rebind_record_calls(e: &Expr, reg: &Registry) -> Expr {
    match &*e.node {
        ExprNode::Seq { exprs } => {
            syn(ExprNode::Seq { exprs: exprs.iter().map(|s| rebind_stmt(s, reg)).collect() })
        }
        _ => rebind_stmt(e, reg),
    }
}

/// Rebind a single statement so calls to record-threading methods carry
/// the receiver forward. A "threaded receiver" is the threaded `record`
/// (a self-call) or a typed local (`instance.save`); the rebind targets
/// that variable. Three positions:
/// - `y = dual(...)`          → `{recv, y} = dual(...)`
/// - `if dual(...) do …`       → `{recv, ok} = dual(...); if ok do …`
/// - bare `dual(...)`          → `{recv, _} = dual(...)`
/// - bare record-returning self → `record = m(...)`
/// Recurses into nested branches first.
fn rebind_stmt(s: &Expr, reg: &Registry) -> Expr {
    let s = rebind_into_children(s, reg);
    // `y = dual(...)` (value capture) → `{recv, y} = dual(...)`.
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*s.node {
        if let Some((recv, m)) = threaded_call(value) {
            if reg.is_dual(m) {
                return multi_assign(
                    vec![lvar(&recv), LValue::Var { id: VarId(0), name: name.clone() }],
                    value.clone(),
                );
            }
        }
    }
    // `if dual(...) do … end` (bool condition) → destructure, then test
    // the captured boolean: `{recv, ok} = dual(...); if ok do … end`.
    if let ExprNode::If { cond, then_branch, else_branch } = &*s.node {
        if let Some((recv, m)) = threaded_call(cond) {
            if reg.is_dual(m) {
                let lifted_if = syn(ExprNode::If {
                    cond: var("ok"),
                    then_branch: then_branch.clone(),
                    else_branch: else_branch.clone(),
                });
                return syn(ExprNode::Seq {
                    exprs: vec![
                        multi_assign(vec![lvar(&recv), lvar("ok")], cond.clone()),
                        lifted_if,
                    ],
                });
            }
        }
    }
    match threaded_call(&s) {
        // bare record-returning self-call statement → `record = m(...)`.
        Some((recv, m)) if recv == RECORD && reg.is_record_returning(m) => {
            syn(ExprNode::Assign { target: lvar(RECORD), value: s.clone() })
        }
        // bare dual call statement → `{recv, _} = m(...)`.
        Some((recv, m)) if reg.is_dual(m) => {
            multi_assign(vec![lvar(&recv), lvar("_")], s.clone())
        }
        _ => s,
    }
}

/// Recurse rebinding into a statement's nested statement-bearing
/// children (`if` branches, `while` body) — NOT into `Send` args or
/// value expressions, where a self-call is an operand, not a statement.
fn rebind_into_children(s: &Expr, reg: &Registry) -> Expr {
    match &*s.node {
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: rebind_record_calls(then_branch, reg),
            else_branch: rebind_record_calls(else_branch, reg),
        }),
        ExprNode::While { cond, body, until_form } => syn(ExprNode::While {
            cond: cond.clone(),
            body: rebind_record_calls(body, reg),
            until_form: *until_form,
        }),
        _ => s.clone(),
    }
}

/// For a `Send` whose receiver is a "threadable" record — implicit self
/// (no receiver), `self`, or a plain local var (the threaded `record`,
/// or a typed local like `instance` in `instance.save`) — return
/// `(receiver_var_name, method)`. The receiver name is what a dual /
/// record-returning rebind threads (`{record, ok} = …` for self,
/// `{instance, ok} = …` for a local). Excludes the `__field__`/… emit
/// bridges (a field read, not a call).
fn threaded_call(e: &Expr) -> Option<(String, &str)> {
    if let ExprNode::Send { recv, method, block: None, .. } = &*e.node {
        let m = method.as_str();
        if m.starts_with("__") {
            return None;
        }
        let recv_name = match recv {
            None => Some(RECORD.to_string()),
            Some(r) => match &*r.node {
                ExprNode::SelfRef => Some(RECORD.to_string()),
                ExprNode::Var { name, .. } => Some(name.to_string()),
                _ => None,
            },
        }?;
        return Some((recv_name, m));
    }
    None
}

/// Replace every `return v` in the body with `return {record, v}` (the
/// dual-return tuple), recursing through all nested positions.
fn wrap_returns(e: &Expr) -> Expr {
    if let ExprNode::Return { value } = &*e.node {
        return syn(ExprNode::Return { value: tuple_record(rewrite_expr(value)) });
    }
    map_children(e, &wrap_returns)
}

/// Wrap the trailing (value-producing) position of the body in a
/// `{record, v}` tuple, descending `Seq` tails and both `if` branches.
/// A tail that's already a `return` (wrapped by `wrap_returns`) is left
/// alone.
fn wrap_tail(e: &Expr) -> Expr {
    match &*e.node {
        ExprNode::Return { .. } => e.clone(),
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            if let Some(last) = exprs.pop() {
                exprs.push(wrap_tail(&last));
            }
            syn(ExprNode::Seq { exprs })
        }
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: wrap_tail(then_branch),
            else_branch: wrap_tail(else_branch),
        }),
        _ => tuple_record(e.clone()),
    }
}

/// `{record, value}` — the dual-return tuple, bridged through a synthetic
/// `__tuple__(record, value)` Send the emitter renders as `{record,
/// value}`.
fn tuple_record(value: Expr) -> Expr {
    syn(ExprNode::Send {
        recv: None,
        method: Symbol::from("__tuple__"),
        args: vec![var(RECORD), value],
        block: None,
        parenthesized: false,
    })
}

fn multi_assign(targets: Vec<LValue>, value: Expr) -> Expr {
    syn(ExprNode::MultiAssign { targets, value })
}

fn lvar(name: &str) -> LValue {
    LValue::Var { id: VarId(0), name: Symbol::from(name) }
}

/// Apply `f` to each statement-or-value child of `e`, rebuilding the
/// node — used by `wrap_returns` to reach `return`s in any position.
fn map_children(e: &Expr, f: &impl Fn(&Expr) -> Expr) -> Expr {
    match &*e.node {
        ExprNode::Seq { exprs } => syn(ExprNode::Seq { exprs: exprs.iter().map(f).collect() }),
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: cond.clone(),
            then_branch: f(then_branch),
            else_branch: f(else_branch),
        }),
        ExprNode::While { cond, body, until_form } => syn(ExprNode::While {
            cond: cond.clone(),
            body: f(body),
            until_form: *until_form,
        }),
        _ => e.clone(),
    }
}

// ---- the rewrite -----------------------------------------------------

fn rewrite_expr(e: &Expr) -> Expr {
    match &*e.node {
        // `@x` read → `record.x`; `self` → `record`.
        ExprNode::Ivar { name } => field_read(name),
        ExprNode::SelfRef => var(RECORD),
        // Rewrite into yielded args (they may read `@ivar`/`self`).
        ExprNode::Yield { args } => {
            syn(ExprNode::Yield { args: args.iter().map(rewrite_expr).collect() })
        }
        // `@x = v` write → `record = record.__struct_put__(:x, v)`.
        ExprNode::Assign { target: LValue::Ivar { name }, value } => syn(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
            value: struct_put(name, rewrite_expr(value)),
        }),
        // `@x[k] = v` → `record = %{record | x: Map.put(record.x, k, v)}`,
        // bridged: a struct-put of field `x` to (record.x with k→v). The
        // emitter renders __struct_put__ as `%{record | x: …}` and
        // __index_put__ on the (Hash) field as `Map.put`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Ivar { name } = &*r.node {
                let updated_field = syn(ExprNode::Send {
                    recv: Some(field_read(name)),
                    method: Symbol::from("__index_put__"),
                    args: vec![rewrite_expr(&args[0]), rewrite_expr(&args[1])],
                    block: None,
                    parenthesized: true,
                });
                return syn(ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                    value: struct_put(name, updated_field),
                });
            }
            // Non-ivar `[]=` (e.g. on a local) is left for local_accumulation.
            syn(ExprNode::Send {
                recv: Some(rewrite_expr(r)),
                method: method.clone(),
                args: args.iter().map(rewrite_expr).collect(),
                block: None,
                parenthesized: true,
            })
        }
        // `self.x = v` write (attr writer Send, e.g. a per-column setter
        // in a from-attrs constructor) → `record = record.__struct_put__(
        // :x, v)`, same as `@x = v`. Excludes `[]=` (handled above) and
        // the comparison operators that also end in `=`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str().ends_with('=')
                && args.len() == 1
                && !matches!(method.as_str(), "[]=" | "==" | "!=" | ">=" | "<=" | "===")
                && matches!(&*r.node, ExprNode::SelfRef) =>
        {
            let field = Symbol::from(method.as_str().trim_end_matches('='));
            syn(ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                value: struct_put(&field, rewrite_expr(&args[0])),
            })
        }
        // `errors << v` — `<<` on a bareword/`self` accessor `foo()` that
        // reads a list field → `record = %{record | foo: record.foo ++
        // [v]}`. On mutable targets the accessor returns the @errors Array
        // and `<<` mutates it in place; the functional equivalent threads
        // a struct-update append (the accessor name is the field name).
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "<<" && args.len() == 1 =>
        {
            if let ExprNode::Send { recv: ar, method: field, args: fargs, .. } = &*r.node {
                if fargs.is_empty()
                    && ar.as_ref().is_none_or(|x| matches!(&*x.node, ExprNode::SelfRef))
                {
                    let appended = syn(ExprNode::Send {
                        recv: Some(field_read(field)),
                        method: Symbol::from("++"),
                        args: vec![syn(ExprNode::Array {
                            elements: vec![rewrite_expr(&args[0])],
                            style: ArrayStyle::Brackets,
                        })],
                        block: None,
                        parenthesized: false,
                    });
                    return syn(ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: Symbol::from(RECORD) },
                        value: struct_put(field, appended),
                    });
                }
            }
            syn(ExprNode::Send {
                recv: Some(rewrite_expr(r)),
                method: method.clone(),
                args: args.iter().map(rewrite_expr).collect(),
                block: None,
                parenthesized: true,
            })
        }
        // Recurse through the container variants the runtime bodies use.
        // A `super` call (the constructor's chain to ActiveRecord::Base#
        // initialize) is dropped: the defstruct's field defaults +
        // `valid?`/`mark_persisted!` cover the base state, so there's
        // nothing to thread.
        ExprNode::Seq { exprs } => syn(ExprNode::Seq {
            exprs: exprs
                .iter()
                .filter(|x| !matches!(&*x.node, ExprNode::Super { .. }))
                .map(rewrite_expr)
                .collect(),
        }),
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: rewrite_expr(cond),
            then_branch: rewrite_expr(then_branch),
            else_branch: rewrite_expr(else_branch),
        }),
        ExprNode::Send { recv, method, args, block, parenthesized } => syn(ExprNode::Send {
            recv: recv.as_ref().map(rewrite_expr),
            method: method.clone(),
            args: args.iter().map(rewrite_expr).collect(),
            block: block.as_ref().map(rewrite_expr),
            parenthesized: *parenthesized,
        }),
        ExprNode::BoolOp { op, surface, left, right } => syn(ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_expr(left),
            right: rewrite_expr(right),
        }),
        ExprNode::Assign { target, value } => syn(ExprNode::Assign {
            target: target.clone(),
            value: rewrite_expr(value),
        }),
        // A bare `return` (return self/nil) in a threaded method returns
        // the updated `record`; `return expr` threads the expr.
        ExprNode::Return { value } => {
            if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                syn(ExprNode::Return { value: var(RECORD) })
            } else {
                syn(ExprNode::Return { value: rewrite_expr(value) })
            }
        }
        ExprNode::Array { elements, style } => syn(ExprNode::Array {
            elements: elements.iter().map(rewrite_expr).collect(),
            style: *style,
        }),
        ExprNode::Hash { entries, kwargs } => syn(ExprNode::Hash {
            entries: entries.iter().map(|(k, v)| (rewrite_expr(k), rewrite_expr(v))).collect(),
            kwargs: *kwargs,
        }),
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            syn(ExprNode::StringInterp {
                parts: parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite_expr(expr) },
                        InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    })
                    .collect(),
            })
        }
        ExprNode::Cast { value, target_ty } => syn(ExprNode::Cast {
            value: rewrite_expr(value),
            target_ty: target_ty.clone(),
        }),
        // Recurse into block bodies (a block may read `@ivar`/`self`).
        ExprNode::Lambda { params, block_param, body, block_style } => syn(ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_expr(body),
            block_style: *block_style,
        }),
        // Leaves and variants v1 doesn't expect inside a threaded body.
        _ => e.clone(),
    }
}

/// Replace a trailing `nil` with `record`, or append `record`, so the
/// method yields the updated struct.
fn append_record_return(body: Expr) -> Expr {
    let record = var(RECORD);
    // Already returns `record` (e.g. a rewritten trailing `self`)? Leave it.
    if is_record_var(&body) {
        return body;
    }
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut exprs = exprs.clone();
            match exprs.last() {
                // Already returns `record` (self) — leave it.
                Some(e) if is_record_var(e) => return body,
                // Tail is a guard `if` (`return X if c` → then-branch
                // returns) — it's the terminal value (both branches yield
                // record), so don't append. (Constructor-with-loop:
                // `if is_nil(other) do record else …__loop(record,…) end`.)
                Some(e) if is_guard_if(e) => return body,
                // Drop a trailing `nil` or a bare local-var read: in a
                // threaded mutator the method's own return value is
                // superseded by `record` (e.g. `[]=` returns the assigned
                // `value`, which is dead once we return the struct).
                Some(e)
                    if matches!(
                        &*e.node,
                        ExprNode::Lit { value: Literal::Nil } | ExprNode::Var { .. }
                    ) =>
                {
                    exprs.pop();
                }
                _ => {}
            }
            exprs.push(record);
            syn(ExprNode::Seq { exprs })
        }
        ExprNode::Lit { value: Literal::Nil } => record,
        _ => syn(ExprNode::Seq { exprs: vec![body, record] }),
    }
}

/// True when `e` is an `if` whose then-branch ends in a `return` — a
/// guard that produces the method's value (so it shouldn't get a
/// trailing `record` appended after it).
fn is_guard_if(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::If { then_branch, .. } if ends_in_return(then_branch))
}

fn ends_in_return(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Return { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(ends_in_return),
        _ => false,
    }
}

fn is_record_var(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == RECORD)
}

// ---- detection -------------------------------------------------------

fn touches_self(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::Ivar { .. } | ExprNode::SelfRef)
            || matches!(&*n.node, ExprNode::Assign { target: LValue::Ivar { .. }, .. })
        {
            found = true;
        }
    });
    found
}

/// True when the method mutates instance state — i.e. its body contains
/// any shape that `rewrite_expr` turns into a `record = …` struct
/// update. Broader than a plain `@ivar =` write: covers nested
/// `@x[k] = v`, the attr-writer `self.x = v` / `self[k] = v`, and a
/// `<<` append onto a (bareword/self) list accessor (`errors << msg`).
/// This is the signal both for appending a trailing `record` return and
/// for classifying a method as record-returning in the call-site
/// registry — so `validate` (only `errors <<`) and `fill_timestamps`
/// (only `self[:updated_at] =`) are correctly recognized as mutators.
fn mutates_record(e: &Expr) -> bool {
    let mut found = false;
    walk(e, &mut |n| match &*n.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, .. } => found = true,
        // `@x[k] = v` / `self[k] = v` — an ivar- or self-rooted `[]=`.
        ExprNode::Send { recv: Some(r), method, .. }
            if method.as_str() == "[]="
                && matches!(&*r.node, ExprNode::Ivar { .. } | ExprNode::SelfRef) =>
        {
            found = true
        }
        // `self.x = v` — an attr-writer Send on self (excluding `[]=`
        // and the comparison operators that also end in `=`).
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str().ends_with('=')
                && args.len() == 1
                && !matches!(method.as_str(), "[]=" | "==" | "!=" | ">=" | "<=" | "===")
                && matches!(&*r.node, ExprNode::SelfRef) =>
        {
            found = true
        }
        // `errors << v` — `<<` onto a bareword/self list accessor.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "<<" && args.len() == 1 =>
        {
            if let ExprNode::Send { recv: ar, method: _, args: fargs, .. } = &*r.node {
                if fargs.is_empty()
                    && ar.as_ref().is_none_or(|x| matches!(&*x.node, ExprNode::SelfRef))
                {
                    found = true;
                }
            }
        }
        _ => {}
    });
    found
}

/// Nested mutation (`@flash[:notice] = v` → `Send{recv: Ivar, "[]="}`)
/// or a loop — outside v1; bail. (`yield` IS supported.)
/// A `while` loop in an instance method — not yet threaded (the
/// recursion would need to carry `record`). `each`/`initialize` bail
/// here for now. (Nested `@x[k] = v` IS handled — see `rewrite_expr`.)
fn has_nested_mutation_or_loop(e: &Expr) -> bool {
    let mut bad = false;
    walk(e, &mut |n| {
        if matches!(&*n.node, ExprNode::While { .. }) {
            bad = true;
        }
    });
    bad
}

// ---- IR builders + walker -------------------------------------------

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn var(name: &str) -> Expr {
    syn(ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

/// `@field` read → `record.__field__(:field)`, a bridge the emitter
/// renders as `record.field` (struct field access). Using a bridge —
/// rather than a bare `record.field` Send — keeps field reads
/// unambiguous from method calls (`record.to_h`), which the emitter
/// routes to a same-module function.
fn field_read(field: &Symbol) -> Expr {
    syn(ExprNode::Send {
        recv: Some(var(RECORD)),
        method: Symbol::from("__field__"),
        args: vec![syn(ExprNode::Lit { value: Literal::Sym { value: field.clone() } })],
        block: None,
        parenthesized: true,
    })
}

/// `record.__struct_put__(:field, value)` — the emitter renders this as
/// `%{record | field: value}`.
fn struct_put(field: &Symbol, value: Expr) -> Expr {
    let field_sym = syn(ExprNode::Lit { value: Literal::Sym { value: field.clone() } });
    syn(ExprNode::Send {
        recv: Some(var(RECORD)),
        method: Symbol::from("__struct_put__"),
        args: vec![field_sym, value],
        block: None,
        parenthesized: true,
    })
}

fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &*e.node {
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            exprs.iter().for_each(|x| walk(x, f))
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, f);
            }
            args.iter().for_each(|a| walk(a, f));
            if let Some(b) = block {
                walk(b, f);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, f);
            walk(then_branch, f);
            walk(else_branch, f);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, f);
            walk(body, f);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, f);
            walk(right, f);
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => walk(value, f),
        ExprNode::Return { value } | ExprNode::Raise { value } | ExprNode::Cast { value, .. } => {
            walk(value, f)
        }
        ExprNode::Yield { args } => args.iter().for_each(|a| walk(a, f)),
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(k, v)| {
            walk(k, f);
            walk(v, f);
        }),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, f);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, LibraryClass, Param};
    use crate::effect::EffectSet;
    use crate::ident::ClassId;

    fn sym(s: &str) -> Symbol {
        Symbol::from(s)
    }
    fn vr(name: &str) -> Expr {
        var(name)
    }
    fn nil() -> Expr {
        syn(ExprNode::Lit { value: Literal::Nil })
    }
    fn assign_ivar(name: &str, value: Expr) -> Expr {
        syn(ExprNode::Assign { target: LValue::Ivar { name: sym(name) }, value })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        syn(ExprNode::Send {
            recv,
            method: sym(method),
            args,
            block: None,
            parenthesized: false,
        })
    }
    fn if_(cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
        syn(ExprNode::If { cond, then_branch, else_branch })
    }
    /// Transform a single method, classifying the registry from it alone
    /// (the test methods are self-contained).
    fn tx(m: MethodDef) -> MethodDef {
        let reg = compute_registry(std::slice::from_ref(&m));
        transform_method(m, &reg)
    }
    fn instance_method(name: &str, params: &[&str], body: Expr) -> MethodDef {
        MethodDef {
            name: sym(name),
            receiver: MethodReceiver::Instance,
            params: params.iter().map(|p| Param::positional(sym(p))).collect(),
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        }
    }
    fn render_via_elixir(methods: Vec<MethodDef>) -> String {
        let class = LibraryClass {
            name: ClassId(sym("Ctl")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods,
            origin: None,
        };
        crate::emit::elixir2::emit_library_class(&class).expect("emit")
    }

    /// Transform a whole method set through one shared registry (so
    /// cross-method classification — `validate` is record-returning,
    /// `save` is dual — drives the call-site rebinding) and render.
    fn render_all(methods: Vec<MethodDef>) -> String {
        let reg = compute_registry(&methods);
        let out = methods.into_iter().map(|m| transform_method(m, &reg)).collect();
        render_via_elixir(out)
    }
    fn class_method(name: &str, params: &[&str], body: Expr) -> MethodDef {
        let mut m = instance_method(name, params, body);
        m.receiver = MethodReceiver::Class;
        m
    }

    /// `def render(body, content_type)
    ///   @body = body
    ///   @content_type = content_type unless content_type.nil?
    ///   nil
    /// end`
    fn render_method() -> MethodDef {
        let cond_write = if_(
            send(Some(vr("content_type")), "nil?", vec![]),
            nil(),
            assign_ivar("content_type", vr("content_type")),
        );
        let body = syn(ExprNode::Seq {
            exprs: vec![assign_ivar("body", vr("body")), cond_write, nil()],
        });
        instance_method("render", &["body", "content_type"], body)
    }

    #[test]
    fn mutator_threads_record_and_returns_it() {
        let out = tx(render_method());
        let ex = render_via_elixir(vec![out]);
        eprintln!("--- render ---\n{ex}\n--------------");
        assert!(ex.contains("def render(record, body, content_type)"), "threads record:\n{ex}");
        assert!(ex.contains("record = %{record | body: body}"), "flat write:\n{ex}");
        // conditional write (`unless`) → else-side cond-rebind lift.
        assert!(
            ex.contains("record = if is_nil(content_type) do\n      record\n    else\n      %{record | content_type: content_type}"),
            "conditional write lifts:\n{ex}"
        );
        // returns the updated record (trailing nil replaced).
        let trimmed = ex.trim_end();
        assert!(trimmed.ends_with("record\n  end\nend"), "returns record:\n{ex}");
        assert!(!ex.contains("@"), "no raw ivars remain:\n{ex}");
    }

    #[test]
    fn constructor_drops_super_and_threads_attr_writers() {
        // def initialize(attrs = {}); super; self.title = attrs[:title]; end
        // → `new` with super dropped and the attr-writer as a struct update.
        let super_call = syn(ExprNode::Super { args: None });
        let write = send(
            Some(syn(ExprNode::SelfRef)),
            "title=",
            vec![send(Some(vr("attrs")), "[]", vec![syn(ExprNode::Lit {
                value: Literal::Sym { value: sym("title") },
            })])],
        );
        let body = syn(ExprNode::Seq { exprs: vec![super_call, write] });
        let init = instance_method("initialize", &["attrs"], body);
        let ex = render_via_elixir(vec![init]);
        eprintln!("--- ctor ---\n{ex}\n------------");
        assert!(ex.contains("def new("), "emits new:\n{ex}");
        assert!(ex.contains("record = %{record | title: attrs[:title]}"), "attr writer → struct update:\n{ex}");
        assert!(!ex.contains("set_title"), "no bare setter call:\n{ex}");
        assert!(!ex.to_lowercase().contains("super"), "super dropped:\n{ex}");
    }

    #[test]
    fn nested_ivar_index_assign_threads_via_map_put() {
        // def []=(key, value); @data[key] = value; end
        let assign = send(
            Some(syn(ExprNode::Ivar { name: sym("data") })),
            "[]=",
            vec![vr("key"), vr("value")],
        );
        let m = instance_method("[]=", &["key", "value"], syn(ExprNode::Seq { exprs: vec![assign] }));
        let ex = render_via_elixir(vec![tx(m)]);
        eprintln!("--- nested []= ---\n{ex}\n------------------");
        assert!(ex.contains("def put(record, key, value)"), "[]= → put, threaded:\n{ex}");
        assert!(
            ex.contains("record = %{record | data: Map.put(record.data, key, value)}"),
            "nested @data[k]=v → struct-update with Map.put:\n{ex}"
        );
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    #[test]
    fn yielding_method_takes_block_fn_and_returns_record() {
        // `def each; yield @notice; self; end` (Instance).
        let body = syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Yield { args: vec![syn(ExprNode::Ivar { name: sym("notice") })] }),
                syn(ExprNode::SelfRef),
            ],
        });
        let out = tx(instance_method("each", &[], body));
        let ex = render_via_elixir(vec![out]);
        eprintln!("--- each ---\n{ex}\n------------");
        assert!(ex.contains("def each(record, block_fn)"), "threads record + block_fn:\n{ex}");
        assert!(ex.contains("block_fn.(record.notice)"), "yield → block_fn call:\n{ex}");
        assert!(!ex.contains("self") && !ex.contains("@"), "self/@ rewritten:\n{ex}");
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    #[test]
    fn index_operator_def_and_self_call_route_to_get() {
        // def [](key); @notice; end   →  def get(record, key) do record.notice end
        // def fetch(key); self[key]; end → def fetch(record, key) do get(record, key) end
        let getter = instance_method(
            "[]",
            &["key"],
            syn(ExprNode::Seq { exprs: vec![syn(ExprNode::Ivar { name: sym("notice") })] }),
        );
        let self_index = send(
            Some(syn(ExprNode::SelfRef)),
            "[]",
            vec![vr("key")],
        );
        let fetch = instance_method("fetch", &["key"], syn(ExprNode::Seq { exprs: vec![self_index] }));
        let ex = render_via_elixir(vec![tx(getter), tx(fetch)]);
        eprintln!("--- index ops ---\n{ex}\n-----------------");
        assert!(ex.contains("def get(record, key)"), "[] def → get:\n{ex}");
        assert!(ex.contains("record.notice"), "getter reads field:\n{ex}");
        assert!(ex.contains("def fetch(record, key)"), "fetch threaded:\n{ex}");
        assert!(ex.contains("get(record, key)"), "self[key] → get(record, key):\n{ex}");
    }

    #[test]
    fn if_elsif_chain_reassignment_threads() {
        // def set(key, value)
        //   if key == "notice" then @notice = value
        //   elsif key == "alert" then @alert = value end
        // end
        let chain = if_(
            send(Some(vr("key")), "==", vec![str_lit_helper("notice")]),
            assign_ivar("notice", vr("value")),
            if_(
                send(Some(vr("key")), "==", vec![str_lit_helper("alert")]),
                assign_ivar("alert", vr("value")),
                nil(),
            ),
        );
        let m = instance_method("set", &["key", "value"], syn(ExprNode::Seq { exprs: vec![chain] }));
        let ex = render_via_elixir(vec![tx(m)]);
        eprintln!("--- if-chain ---\n{ex}\n----------------");
        assert!(ex.contains("record = if key == \"notice\" do"), "chain lifts to record =:\n{ex}");
        assert!(ex.contains("%{record | notice: value}"), "then update:\n{ex}");
        assert!(ex.contains("%{record | alert: value}"), "elsif update:\n{ex}");
        assert!(ex.trim_end().ends_with("record\n  end\nend"), "returns record:\n{ex}");
    }

    fn str_lit_helper(s2: &str) -> Expr {
        syn(ExprNode::Lit { value: Literal::Str { value: s2.to_string() } })
    }

    #[test]
    fn non_flat_constructor_threads_a_seeded_record() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        // def initialize(other = nil)
        //   @notice = nil
        //   return if other.nil?
        //   @notice = other
        // end
        let body = syn(ExprNode::Seq {
            exprs: vec![
                assign_ivar("notice", nil()),
                if_(send(Some(vr("other")), "nil?", vec![]), syn(ExprNode::Return { value: nil() }), nil()),
                assign_ivar("notice", vr("other")),
            ],
        });
        let init = MethodDef {
            name: sym("initialize"),
            receiver: MethodReceiver::Instance,
            params: vec![Param::with_default(sym("other"), nil())],
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        };
        // initialize isn't threaded by the pipeline; emit_constructor calls
        // thread_constructor_body itself. Render it through a class.
        let class = LibraryClass {
            name: ClassId(sym("Flash")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![init],
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- ctor ---\n{ex}\n------------");
        assert!(ex.contains("def new(other \\\\ nil)"), "default param:\n{ex}");
        assert!(ex.contains("record = %V2.Flash{}"), "seeds struct:\n{ex}");
        assert!(ex.contains("record = %{record | notice: nil}"), "threaded write:\n{ex}");
        assert!(ex.contains("if is_nil(other) do"), "guard:\n{ex}");
        assert!(!ex.contains("@"), "no raw ivars:\n{ex}");
    }

    #[test]
    fn pure_instance_method_takes_underscore_record() {
        // `def resolve_status(s); s; end` — no @ivar, but under uniform
        // record threading every instance method takes a leading record
        // param; a pure one names it `_record` (warning-clean) so call
        // sites can pass `record` uniformly.
        let m = instance_method("resolve_status", &["s"], syn(ExprNode::Seq { exprs: vec![vr("s")] }));
        let out = tx(m);
        let ex = render_via_elixir(vec![out]);
        assert!(ex.contains("def resolve_status(_record, s)"), "underscore record param:\n{ex}");
    }

    #[test]
    fn errors_shovel_threads_struct_append() {
        // `def validate; self.errors << "oops"; end` — the `<<` on the
        // @errors accessor threads to a struct-update append (the mutable
        // in-place push becomes a functional rebind).
        let push = send(
            Some(send(Some(syn(ExprNode::SelfRef)), "errors", vec![])),
            "<<",
            vec![syn(ExprNode::Lit { value: Literal::Str { value: "oops".to_string() } })],
        );
        let m = instance_method("validate", &[], syn(ExprNode::Seq { exprs: vec![push] }));
        let ex = render_via_elixir(vec![tx(m)]);
        assert!(
            ex.contains("%{record | errors: record.errors ++ [\"oops\"]}"),
            "errors << → struct append:\n{ex}"
        );
    }

    #[test]
    fn return_self_guard_threads_not_stubbed() {
        // `def destroy; return self unless persisted?; @destroyed = true;
        // self; end` — `return self` is a record return, not a value
        // return, so the method threads (returns record), not stub-raises.
        let guard = if_(
            send(Some(syn(ExprNode::SelfRef)), "persisted?", vec![]),
            nil(),
            syn(ExprNode::Return { value: syn(ExprNode::SelfRef) }),
        );
        let body = syn(ExprNode::Seq {
            exprs: vec![guard, assign_ivar("destroyed", syn(ExprNode::Lit { value: Literal::Bool { value: true } })), syn(ExprNode::SelfRef)],
        });
        let ex = render_via_elixir(vec![tx(instance_method("destroy", &[], body))]);
        assert!(!ex.contains("mutates instance state and returns a value"), "not stubbed:\n{ex}");
        assert!(ex.contains("%{record | destroyed: true}"), "threads the write:\n{ex}");
    }

    #[test]
    fn instance_method_with_while_bails() {
        // A `while` in an instance method isn't threaded yet (the
        // recursion would need to carry `record`) — left unchanged.
        let loop_ = syn(ExprNode::While {
            cond: send(Some(vr("i")), "<", vec![vr("n")]),
            body: syn(ExprNode::Seq { exprs: vec![assign_ivar("data", vr("i"))] }),
            until_form: false,
        });
        let m = instance_method("walk", &["n"], syn(ExprNode::Seq { exprs: vec![loop_, nil()] }));
        let out = tx(m.clone());
        assert_eq!(out.body, m.body, "instance-method while should bail (unchanged)");
    }

    #[test]
    fn dual_return_threads_record_returning_call_and_wraps_tuple() {
        // def validate; self.errors << "oops"; end      (record-returning)
        // def valid?; @errors = []; validate; @errors.empty?; end (dual)
        let validate = instance_method(
            "validate",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![send(
                    Some(send(Some(syn(ExprNode::SelfRef)), "errors", vec![])),
                    "<<",
                    vec![str_lit_helper("oops")],
                )],
            }),
        );
        let valid = instance_method(
            "valid?",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![
                    assign_ivar("errors", syn(ExprNode::Array { elements: vec![], style: ArrayStyle::Brackets })),
                    send(None, "validate", vec![]),
                    send(Some(syn(ExprNode::Ivar { name: sym("errors") })), "empty?", vec![]),
                ],
            }),
        );
        let ex = render_all(vec![validate, valid]);
        eprintln!("--- valid? ---\n{ex}\n--------------");
        assert!(ex.contains("record = validate(record)"), "record-returning self-call rebinds:\n{ex}");
        // dual tail wrapped as a tuple (the bool computed value, threaded
        // record). The `.empty?` rendering isn't asserted (field-type
        // stamping happens in the full pipeline, not this unit slice).
        assert!(ex.contains("{record, "), "tail wrapped as {{record, value}}:\n{ex}");
        assert!(!ex.contains("mutates instance state and returns a value"), "not stubbed:\n{ex}");
    }

    #[test]
    fn dual_call_in_bool_cond_lifts_to_destructure() {
        // def save; @id = 1; true; end          (dual: mutate + value)
        // def save!; raise "x" unless save; self; end  → bool-cond lift
        let save = instance_method(
            "save",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![
                    assign_ivar("id", syn(ExprNode::Lit { value: Literal::Int { value: 1 } })),
                    syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
                ],
            }),
        );
        // `raise "x" unless save` → if save do nil else raise end.
        let save_bang = instance_method(
            "save!",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![
                    if_(
                        send(None, "save", vec![]),
                        nil(),
                        syn(ExprNode::Raise { value: str_lit_helper("x") }),
                    ),
                    syn(ExprNode::SelfRef),
                ],
            }),
        );
        let ex = render_all(vec![save, save_bang]);
        eprintln!("--- save! ---\n{ex}\n-------------");
        assert!(ex.contains("{record, ok} = save(record)"), "bool-cond destructures:\n{ex}");
        assert!(ex.contains("if ok do"), "tests the captured boolean:\n{ex}");
    }

    #[test]
    fn class_method_destructures_dual_dispatch() {
        // def self.create; instance = new; instance.save; instance; end
        let save = instance_method(
            "save",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![
                    assign_ivar("id", syn(ExprNode::Lit { value: Literal::Int { value: 1 } })),
                    syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
                ],
            }),
        );
        let create = class_method(
            "create",
            &[],
            syn(ExprNode::Seq {
                exprs: vec![
                    syn(ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: sym("instance") },
                        value: send(None, "new", vec![]),
                    }),
                    send(Some(vr("instance")), "save", vec![]),
                    vr("instance"),
                ],
            }),
        );
        let ex = render_all(vec![save, create]);
        eprintln!("--- create ---\n{ex}\n--------------");
        // Threads the receiving local (`instance`), not `record`. (The
        // emitter's `instance.__struct__.save(instance)` polymorphic
        // routing needs `instance`'s `Ty::Class`, set in the full
        // pipeline — not asserted in this unit slice.)
        assert!(
            ex.contains("{instance, _} = instance.save"),
            "polymorphic dual call destructures, threading the local:\n{ex}"
        );
    }
}
