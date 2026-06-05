//! IR method body / value → Elixir.
//!
//! Phase 2 walker, grown to cover `json_builder.rb`. Elixir is
//! expression-oriented and immutable, so two Ruby constructs need real
//! transformation rather than 1:1 syntax mapping:
//!
//! - **`return` elimination.** Elixir has no `return`. A guard-clause
//!   sequence (`return X if c1; return Y if c2; Z`) folds into nested
//!   `if c1, do: X, else: (if c2 …)`. `emit_stmts` does this by putting
//!   the rest of the block in the `else` branch of each guard.
//! - **Conditional local reassignment.** A variable reassigned inside an
//!   `if` body doesn't leak out in Elixir. `ms = "000"; if c do … ms = X
//!   end` becomes `ms = if c do … X else ms end`.
//!
//! Everything else is per-construct mapping: `is_a?`/`nil?`/`to_s`/
//! `length`/`gsub` Sends, `[]` slicing → `String.slice`, Regex/Range/
//! Hash literals, string interpolation (syntax matches Ruby).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::expr::{BoolOpKind, Expr, ExprNode, InterpPart, IrHint, LValue, Literal};

thread_local! {
    /// Simple class name → emitted `V2.*` module name, across ALL runtime
    /// units in the overlay (populated up front by a pre-registration
    /// pass). Elixir doesn't resolve a bare reference (`MatchResult`
    /// inside `V2.ActionDispatch.Router`, or `Session` from another file)
    /// the way Ruby's lexical scoping does, so `emit_const` rewrites such
    /// refs to the fully-qualified module name using this map.
    static MODULE_NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());

    /// `"{simple_module}#{elixir_fn}"` → that function's declared params,
    /// across ALL registered modules in the overlay. The cross-module
    /// analogue of `METHOD_PARAMS` (which holds only the CURRENT class):
    /// a qualified call like `ActionView::ViewHelpers.truncate(body,
    /// length: 100)` reaches a callee in ANOTHER module, so its params
    /// can't come from `METHOD_PARAMS`. Lets the Const-receiver call arm
    /// spread a trailing keyword-args hash into the callee's defaulted
    /// positionals (`truncate(body, 100)`) instead of passing it as a
    /// literal map (which would land in the `length` slot and silently
    /// defeat the helper — `String.length(s) <= %{length: 100}` is always
    /// true in Elixir term ordering). Populated by `register_modules`.
    static MODULE_METHOD_PARAMS: RefCell<HashMap<String, Vec<crate::dialect::Param>>> =
        RefCell::new(HashMap::new());

    /// Elixir function names of the CURRENT class's instance methods that
    /// thread a leading `record` param. A self-call (`record.foo(args)`)
    /// routes to `foo(record, args)` only when `foo` is in here; a pure
    /// instance method (e.g. `resolve_status`, which reads only a module
    /// constant) is NOT, so its self-call stays arity-correct as
    /// `foo(args)`. Set per class by `emit_library_class`.
    static RECORD_METHODS: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Set the current class's record-threading instance-method names (see
/// `RECORD_METHODS`). Called by `emit_library_class` before emitting a
/// class's methods.
pub(super) fn set_record_methods(names: std::collections::HashSet<String>) {
    RECORD_METHODS.with(|m| *m.borrow_mut() = names);
}

thread_local! {
    /// Elixir fn name → its declared params (name + default), for the
    /// CURRENT class. A call site whose trailing arg is a keyword-args
    /// hash (`render(body, status: :x)`) spreads it into the callee's
    /// defaulted positionals by name (`render(record, body, :x)`) — Elixir
    /// has no Ruby keyword args. Set per class by `emit_library_class`.
    static METHOD_PARAMS: RefCell<HashMap<String, Vec<crate::dialect::Param>>> =
        RefCell::new(HashMap::new());
}

/// Set the current class's per-method declared params (see `METHOD_PARAMS`).
pub(super) fn set_method_params(params: HashMap<String, Vec<crate::dialect::Param>>) {
    METHOD_PARAMS.with(|m| *m.borrow_mut() = params);
}

thread_local! {
    /// Param names of the method currently being emitted. A recv-less
    /// 0-arg call (`article()`) whose name is in here is a *local read*
    /// of that param (Ruby resolves a bareword to an in-scope local
    /// before a method), not a call — emit the bare name. Needed for
    /// view partials whose param (`article`) collides with a same-named
    /// view function (`Views::Articles.article`). Set by `emit_fn`.
    static CURRENT_PARAMS: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Set the param names in scope for the method being emitted.
pub(super) fn set_current_params(names: std::collections::HashSet<String>) {
    CURRENT_PARAMS.with(|p| *p.borrow_mut() = names);
}

thread_local! {
    /// Whether the method currently being emitted threads a `record` (an
    /// instance method whose synthetic first arg is the renamed `self`).
    /// `is_record_var` gates on this so a *module-singleton* function with
    /// a genuine param named `record` (e.g. `ViewHelpers.dom_id(record,
    /// …)`) treats `record.foo` as an ordinary method-on-local, NOT a
    /// same-module self-call that would drop the receiver.
    static THREADS_RECORD: RefCell<bool> = const { RefCell::new(false) };
}

/// Set whether the method being emitted threads `record` (see
/// `THREADS_RECORD`). Called by `emit_fn` before emitting the body.
pub(super) fn set_threads_record(yes: bool) {
    THREADS_RECORD.with(|t| *t.borrow_mut() = yes);
}

thread_local! {
    /// The accumulator names of the `Enum.reduce` folds currently being
    /// emitted (a stack — folds nest). A Ruby `next` inside a fold means
    /// "yield the accumulator unchanged for this element", so the top of
    /// the stack is what a `Next` node (or a `next if cond` guard) emits.
    static FOLD_ACC: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The accumulator of the innermost fold currently being emitted, if any.
fn current_fold_acc() -> Option<String> {
    FOLD_ACC.with(|s| s.borrow().last().cloned())
}

/// True when `name` is a param of the method currently being emitted.
fn is_current_param(name: &str) -> bool {
    CURRENT_PARAMS.with(|p| p.borrow().contains(name))
}

thread_local! {
    /// The Ruby name of the class currently being emitted (e.g.
    /// `ActiveRecord::Base`), used to resolve `Module#name` reflection
    /// (`#{name}` in a class method) to a static string — Elixir module
    /// functions have no class-name reflection.
    static CURRENT_CLASS_NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Set the Ruby name of the class currently being emitted. Called by
/// `emit_library_class`.
pub(super) fn set_current_class_name(name: &str) {
    CURRENT_CLASS_NAME.with(|n| *n.borrow_mut() = name.to_string());
}

thread_local! {
    /// Module-level constant names DECLARED in the runtime files (e.g.
    /// `HTML_ESCAPES`, `ESCAPES`, `STATUS_CODES`), registered up front by
    /// the overlay pre-pass. `emit_const` rewrites a SCREAMING_SNAKE name
    /// to a module attribute (`@html_escapes`) only when it's in here —
    /// so an all-caps *module* reference (`JSON`, `IO`, `URI`) is NOT
    /// mistaken for a constant and stays a module name.
    static DECLARED_CONSTANTS: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Reset the declared-constant registry (start of an overlay emit).
pub(super) fn clear_declared_constants() {
    DECLARED_CONSTANTS.with(|c| c.borrow_mut().clear());
}

/// Register a declared module-level constant name (see `DECLARED_CONSTANTS`).
pub(super) fn register_declared_constant(name: &str) {
    DECLARED_CONSTANTS.with(|c| {
        c.borrow_mut().insert(name.to_string());
    });
}

thread_local! {
    /// Ruby class name → (struct field name → field `Ty`), across all
    /// units (registered up front). Two uses: (1) method-on-typed-local
    /// distinguishes a struct FIELD read (`record.id` → `record.id`) from
    /// a METHOD call (`instance.save` → `instance.__struct__.save(…)`) by
    /// key presence; (2) field reads carry the recorded `Ty` so emit can
    /// dispatch on it (`record.title.empty?` → `== ""` when `title: Str`).
    /// Runtime classes register names only (`Ty::Untyped`); model + Row
    /// classes register schema-derived column types.
    static FIELD_TYPES: RefCell<HashMap<String, HashMap<String, crate::ty::Ty>>> =
        RefCell::new(HashMap::new());
}

/// Reset the field registry (start of an overlay emit).
pub(super) fn clear_field_names() {
    FIELD_TYPES.with(|f| f.borrow_mut().clear());
}

/// Register a class's struct field names with unknown types (runtime
/// classes — their bare-ivar fields carry no schema type).
pub(super) fn register_field_names(class: &str, fields: &[String]) {
    FIELD_TYPES.with(|f| {
        f.borrow_mut().insert(
            class.to_string(),
            fields.iter().map(|n| (n.clone(), crate::ty::Ty::Untyped)).collect(),
        );
    });
}

/// Register a class's struct fields with their `Ty` (model + `<Model>Row`
/// classes — column types from the schema).
pub(super) fn register_field_types(class: &str, fields: &[(String, crate::ty::Ty)]) {
    FIELD_TYPES.with(|f| {
        f.borrow_mut().insert(class.to_string(), fields.iter().cloned().collect());
    });
}

/// True when `field` is a known struct field of class `id` — so
/// `value.field` is a field read rather than a 0-arg method call.
fn is_struct_field(class_id: &str, field: &str) -> bool {
    FIELD_TYPES.with(|f| f.borrow().get(class_id).is_some_and(|m| m.contains_key(field)))
}

/// The recorded `Ty` of `class_id`'s `field`, if registered with one.
fn field_type(class_id: &str, field: &str) -> Option<crate::ty::Ty> {
    FIELD_TYPES.with(|f| f.borrow().get(class_id).and_then(|m| m.get(field).cloned()))
}

thread_local! {
    /// Method-param name → its `Ty`, keyed by the enclosing class name
    /// (e.g. `Views::Articles` → {"article": Class{Article}, "articles":
    /// Array{Article}}). A view partial's record param carries NO type in
    /// the IR — the view lowering emits a bare positional `Param`, and the
    /// functionalize passes drop the body-typer's annotations — so
    /// `article.errors` can't resolve `errors` to `Array` (→ `Enum.count`/
    /// `Enum.empty?`) without it. Registered up front from the
    /// resource→model mapping; read by `effective_recv_ty` for a `Var`
    /// matching a param of the method being emitted.
    static PARAM_TYPES: RefCell<HashMap<String, HashMap<String, crate::ty::Ty>>> =
        RefCell::new(HashMap::new());
}

/// Reset the param-type registry (start of an overlay emit).
pub(super) fn clear_param_types() {
    PARAM_TYPES.with(|p| p.borrow_mut().clear());
}

/// Register a class's method-param types (see `PARAM_TYPES`). Accumulates
/// across calls so multiple resources' params land under the same view
/// module.
pub(super) fn register_param_types(class: &str, params: &[(String, crate::ty::Ty)]) {
    PARAM_TYPES.with(|p| {
        p.borrow_mut().entry(class.to_string()).or_default().extend(params.iter().cloned());
    });
}

/// The recorded `Ty` of a `param` in class `class_id`, if registered.
fn param_type(class_id: &str, param: &str) -> Option<crate::ty::Ty> {
    PARAM_TYPES.with(|p| p.borrow().get(class_id).and_then(|m| m.get(param).cloned()))
}

/// True when `name` is a record-threading instance method of the current
/// class — so a self-call to it takes a leading `record` arg.
fn threads_record(name: &str) -> bool {
    RECORD_METHODS.with(|m| m.borrow().contains(name))
}

/// Clear the module-name registry. Called once at the start of an
/// overlay emit, before the cross-file pre-registration pass, so a
/// prior emit (e.g. another app in the same test process) doesn't leak.
pub(super) fn clear_modules() {
    MODULE_NAMES.with(|m| m.borrow_mut().clear());
    MODULE_METHOD_PARAMS.with(|m| m.borrow_mut().clear());
}

/// Register the `V2.*` names of `classes` into the registry, accumulating
/// (does NOT clear — the overlay pre-registers every unit's modules up
/// front so cross-file constant references resolve). Idempotent: the
/// per-unit transform may re-register the same names harmlessly.
pub(super) fn register_modules<'a>(classes: impl IntoIterator<Item = &'a crate::dialect::LibraryClass>) {
    MODULE_NAMES.with(|names| {
        MODULE_METHOD_PARAMS.with(|mparams| {
            let mut names = names.borrow_mut();
            let mut mparams = mparams.borrow_mut();
            for c in classes {
                let full = super::library::v2_module_name(c.name.0.as_str());
                let simple = c
                    .name
                    .0
                    .as_str()
                    .rsplit("::")
                    .next()
                    .unwrap_or_else(|| c.name.0.as_str())
                    .to_string();
                // Record each method's params under `{simple}#{fn}` so a
                // cross-module qualified call can spread its kwargs (see
                // MODULE_METHOD_PARAMS). Only methods with at least one
                // defaulted param are spread candidates, but store all —
                // `unpack_kwargs_with` no-ops when there's no kwargs hash.
                for method in &c.methods {
                    let fname = super::library::elixir_fn_name(method.name.as_str());
                    mparams.insert(format!("{simple}#{fname}"), method.params.clone());
                }
                names.insert(simple, full);
            }
        });
    });
}

/// Register a single `simple → full` module-name mapping — for a
/// hand-written module that isn't a parsed `LibraryClass` (e.g. the
/// `V2.Db` runtime primitive, so the lowered model emit's bare `Db.…`
/// references resolve).
pub(super) fn register_module(simple: &str, full: &str) {
    MODULE_NAMES.with(|m| {
        m.borrow_mut().insert(simple.to_string(), full.to_string());
    });
}

/// Emit a method body as Elixir (indent level 0; the caller indents).
pub(super) fn emit_method_body(body: &Expr) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => emit_stmts(exprs),
        _ => emit_tail(body),
    }
}

/// Render an expression for use as a top-level module constant value
/// (the `value` in `@name value`). `.freeze` is stripped (Elixir is
/// immutable). Used by `library::format_constant`.
pub(super) fn emit_const_value(value: &Expr) -> String {
    emit_expr(value)
}

// ---- statement-list emit (return-elim + cond-rebind) ----------------

fn emit_stmts(stmts: &[Expr]) -> String {
    let Some((head, rest)) = stmts.split_first() else {
        return "nil".to_string();
    };
    if rest.is_empty() {
        return emit_tail(head);
    }

    // Fold guard: `next if cond` (an `if` whose then-branch is a bare
    // `next` and else is empty) inside a reduce → `if cond do <acc> else
    // <rest> end` (skip the element: yield the accumulator unchanged).
    // Only fires inside a fold (a `next` elsewhere has no accumulator).
    if let ExprNode::If { cond, then_branch, else_branch } = &*head.node {
        if is_empty(else_branch) && ends_in_next(then_branch) {
            if let Some(acc) = current_fold_acc() {
                let cond_s = emit_expr(cond);
                let else_s = emit_stmts(rest);
                return format!(
                    "if {cond_s} do\n{}\nelse\n{}\nend",
                    indent(&acc, 1),
                    indent(&else_s, 1),
                );
            }
        }
    }

    // Guard clause: `return X if cond` → `if cond do X else <rest> end`.
    if let ExprNode::If { cond, then_branch, else_branch } = &*head.node {
        if is_empty(else_branch) && ends_in_return(then_branch) {
            let cond_s = emit_expr(cond);
            let then_s = emit_return_value(then_branch);
            let else_s = emit_stmts(rest);
            return format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            );
        }

        // The `unless` mirror: `return X unless cond` → an `if` whose
        // *else* returns and *then* is empty. When cond holds, fall
        // through to the rest; otherwise return X. (`save`'s `return
        // false unless ok`.)
        if is_empty(then_branch) && ends_in_return(else_branch) {
            let cond_s = emit_expr(cond);
            let then_s = emit_stmts(rest);
            let else_s = emit_return_value(else_branch);
            return format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            );
        }

        // Conditional reassignment: an `if`/`elsif` chain where every
        // branch yields the same reassigned local `v` (reassigns it, is
        // empty, or is a nested chain yielding it). Elixir scoping
        // discards a rebind inside a branch, so lift the whole chain to
        // `v = if cond do <new> else <…> end`. Covers the single `if`
        // (`@x = v unless c`) and `if/elsif` chains (`[]=`/case-on-key).
        if let Some(v) = chain_reassigned_var(then_branch, else_branch) {
            let lifted = format!("{v} = {}", render_chain(cond, then_branch, else_branch, &v));
            return format!("{lifted}\n{}", emit_stmts(rest));
        }
    }

    format!("{}\n{}", emit_stmt(head), emit_stmts(rest))
}

/// String-accumulator hint consumer — the view/jbuilder lowerer's
/// `io = String.new; io << "..."; io` triple, tagged with
/// `IrHint::StringBuilder*`. Rendered as Elixir's iolist idiom so the
/// inner appends stay O(1) and the function returns a proper binary:
/// - `Init`   → `io = []` (an empty iolist; bypasses the `String.new`
///   value emit).
/// - `Append` → `io = [io, <chunk>]` (nested iodata — order-preserving;
///   a chunk may itself be a sub-view binary or a string interpolation).
/// - `Result` → `IO.iodata_to_binary(io)` (flatten to the returned
///   binary).
/// Safe-by-construction: every `io` reference flows through one of the
/// three tagged sites, so nothing else observes its iolist shape.
fn try_string_builder(e: &Expr) -> Option<String> {
    match e.hint? {
        IrHint::StringBuilderInit => match &*e.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, .. } => Some(format!("{name} = []")),
            _ => None,
        },
        IrHint::StringBuilderAppend => match &*e.node {
            ExprNode::Send { recv: Some(r), args, .. } if args.len() == 1 => {
                if let ExprNode::Var { name, .. } = &*r.node {
                    Some(format!("{name} = [{name}, {}]", emit_expr(&args[0])))
                } else {
                    None
                }
            }
            _ => None,
        },
        IrHint::StringBuilderResult => match &*e.node {
            ExprNode::Var { name, .. } => Some(format!("IO.iodata_to_binary({name})")),
            _ => None,
        },
        _ => None,
    }
}

/// The outer local a `StringBuilderAppend`-hinted send appends to
/// (`io << chunk` → `"io"`). The emitter renders that send as
/// `io = [io, chunk]`, so for accumulator detection and the cond-rebind
/// lift it counts as a rebind of `io` — even though the IR node is a
/// `Send`, not an `Assign`. This is what lets a string-builder append
/// inside an `each` block (`articles.each { io << render(a) }`) or an
/// `if` branch thread `io` through `Enum.reduce` / `io = if … end`
/// rather than emit a dead, unused-variable rebind.
fn string_builder_append_local(e: &Expr) -> Option<&str> {
    if e.hint != Some(IrHint::StringBuilderAppend) {
        return None;
    }
    match &*e.node {
        ExprNode::Send { recv: Some(r), args, .. } if args.len() == 1 => match &*r.node {
            ExprNode::Var { name, .. } => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The iolist value a `StringBuilderAppend` yields (`io << chunk` →
/// `"[io, chunk]"`) — the *next* accumulator value, used when the append
/// is the trailing statement of a cond-rebind branch and must yield that
/// value (not the `io = …` rebind, whose binding the branch scope drops).
fn string_builder_append_value(e: &Expr) -> Option<String> {
    let name = string_builder_append_local(e)?;
    match &*e.node {
        ExprNode::Send { args, .. } => Some(format!("[{name}, {}]", emit_expr(&args[0]))),
        _ => None,
    }
}

/// A single non-terminal statement (a `let` binding or a bare expr).
fn emit_stmt(e: &Expr) -> String {
    // String-builder `Init` (`io = String.new`) → `io = []`; intercept
    // before the generic `Assign` arm renders the `String.new` value.
    if let Some(s) = try_string_builder(e) {
        return s;
    }
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value))
        }
        // A module-singleton's `@ivar = value` is mutable module state →
        // the process dictionary (instance-method ivar mutation is already
        // threaded to struct fields by the functionalize passes, so an
        // Ivar target here is always module-singleton state).
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("Process.put({}, {})", ivar_pd_key(name.as_str()), emit_expr(value))
        }
        ExprNode::MultiAssign { targets, value } => emit_multi_assign(targets, value),
        _ => emit_expr(e),
    }
}

/// `{t1, t2, …} = value` — the dual-return tuple destructure (a
/// `{record, ok} = save(record)` call site). Targets render as plain
/// local names (`_` for a discard).
fn emit_multi_assign(targets: &[LValue], value: &Expr) -> String {
    let lhs = targets
        .iter()
        .map(|t| match t {
            LValue::Var { name, .. } | LValue::Ivar { name } => name.to_string(),
            _ => "_".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{lhs}}} = {}", emit_expr(value))
}

/// The trailing (value-producing) position of a block. A bare `return X`
/// here is just `X`.
fn emit_tail(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Return { value } => emit_expr(value),
        ExprNode::Seq { exprs } if !exprs.is_empty() => emit_stmts(exprs),
        _ => emit_expr(e),
    }
}

/// True when `e` is an empty/absent branch (modifier-`if` has no else).
fn is_empty(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
        || matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

/// The local a statement rebinds when emitted: a `v = …` Assign, a
/// string-builder `v << chunk` append (renders `v = [v, chunk]`), or an
/// accumulating block-call `coll.each { v << … }` (renders `v =
/// Enum.reduce(…)`). Unifies the three forms for the cond-rebind lift.
fn rebound_local(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
            Some(name.as_str().to_string())
        }
        ExprNode::Send { block: Some(blk), .. } => match &*blk.node {
            ExprNode::Lambda { params, body, .. } => {
                block_accumulators(body, params).into_iter().next()
            }
            _ => None,
        },
        _ => string_builder_append_local(e).map(|s| s.to_string()),
    }
}

/// A local `v` reassigned across an `if`/`elsif` chain where *every*
/// branch yields it — the signal to lift the chain to `v = if … end`.
fn chain_reassigned_var(then_branch: &Expr, else_branch: &Expr) -> Option<String> {
    let v = reassigned_var(then_branch).or_else(|| reassigned_var(else_branch))?;
    (branch_yields(then_branch, &v) && branch_yields(else_branch, &v)).then_some(v)
}

/// A branch "yields `v`" if it leaves `v` as its value: empty (unchanged),
/// a trailing rebind of `v` (assign / string-builder append / accumulating
/// block-call), or a nested chain whose branches all yield.
fn branch_yields(b: &Expr, v: &str) -> bool {
    if is_empty(b) {
        return true;
    }
    match &*b.node {
        ExprNode::Seq { exprs } => exprs.last().is_some_and(|l| branch_yields(l, v)),
        ExprNode::If { then_branch, else_branch, .. } => {
            branch_yields(then_branch, v) && branch_yields(else_branch, v)
        }
        _ => rebound_local(b).as_deref() == Some(v),
    }
}

/// Render a chain as `if cond do <yields v> else <yields v> end`, where
/// each branch produces `v`'s next value (unchanged `v` for empty, the
/// rebind's RHS for a `v = …`, a nested chain recursively).
fn render_chain(cond: &Expr, then_branch: &Expr, else_branch: &Expr, v: &str) -> String {
    format!(
        "if {} do\n{}\nelse\n{}\nend",
        emit_expr(cond),
        indent(&branch_render(then_branch, v), 1),
        indent(&branch_render(else_branch, v), 1),
    )
}

fn branch_render(b: &Expr, v: &str) -> String {
    if is_empty(b) {
        return v.to_string();
    }
    match &*b.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            render_chain(cond, then_branch, else_branch, v)
        }
        // A sequence whose last statement yields `v` (a trailing rebind, or
        // a nested chain): emit the leading statements verbatim and
        // recurse on the last so a nested `if` is rendered as a chain that
        // yields `v` (not emitted as-is, which would drop the rebind).
        ExprNode::Seq { exprs } if exprs.len() > 1 => {
            let (last, leading) = exprs.split_last().unwrap();
            let mut lines: Vec<String> = leading.iter().map(emit_stmt).collect();
            lines.push(branch_render(last, v));
            lines.join("\n")
        }
        _ => emit_block_with_value(b),
    }
}

/// True when the then-branch of a guard ends in a `return`.
fn ends_in_return(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Return { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(ends_in_return),
        _ => false,
    }
}

/// True when a branch is (or ends in) a bare `next` — the fold-skip guard.
fn ends_in_next(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Next { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().is_some_and(ends_in_next),
        _ => false,
    }
}

/// Emit the value a guard's `return` yields.
fn emit_return_value(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Return { value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            // Lets before the return stay; the trailing return becomes
            // the block's value.
            emit_stmts(exprs)
        }
        _ => emit_expr(e),
    }
}

/// If the block's last statement reassigns an (already-bound) local
/// variable, return its name — the signal for the cond-rebind lift. A
/// trailing `if` whose branches all rebind the same local counts too (a
/// nested mutating conditional, e.g. the length-validation
/// `unless attr.nil? do len = …; if len < n do record = … end end`):
/// delegate to `chain_reassigned_var`, which finds + validates the var
/// across the inner branches. `branch_yields` already accepts this shape;
/// without this, the enclosing `if` isn't lifted and the rebind is lost.
fn reassigned_var(e: &Expr) -> Option<String> {
    let last = match &*e.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => e,
    };
    if let ExprNode::If { then_branch, else_branch, .. } = &*last.node {
        return chain_reassigned_var(then_branch, else_branch);
    }
    rebound_local(last)
}

/// The value a trailing rebind-statement yields (the `X` of `v = X`, the
/// `[v, chunk]` of a string-builder append, or the bare `Enum.reduce(…)`
/// of an accumulating block-call) — used inside a cond-rebind `if` where
/// the branch must produce `v`'s next value, not the `v = …` statement.
fn rebound_value(e: &Expr) -> Option<String> {
    string_builder_append_value(e)
        .or_else(|| try_reduce_value(e))
        .or_else(|| match &*e.node {
            ExprNode::Assign { value, .. } => Some(emit_expr(value)),
            _ => None,
        })
}

/// Emit a block whose trailing rebind is rewritten to yield its value
/// (used inside a cond-rebind `if`). Leading lets are preserved.
fn emit_block_with_value(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let (last, leading) = exprs.split_last().unwrap();
            let mut lines: Vec<String> = leading.iter().map(emit_stmt).collect();
            lines.push(rebound_value(last).unwrap_or_else(|| emit_stmt(last)));
            lines.join("\n")
        }
        _ => rebound_value(e).unwrap_or_else(|| emit_tail(e)),
    }
}

// ---- expression emit ------------------------------------------------

pub(super) fn emit_expr(e: &Expr) -> String {
    // String-builder hint sites (`io = String.new; io << "..."; io`,
    // tagged by the view/jbuilder lowerer) → the iolist idiom. One hook
    // covers the Append + terminal-Result sites; Init is intercepted in
    // `emit_stmt` (an `Assign` handled before it reaches here).
    if let Some(s) = try_string_builder(e) {
        return s;
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => emit_const(path),
        ExprNode::Var { name, .. } => name.to_string(),
        // A bare module-state `@ivar` read → the whole process-dictionary
        // store. (Element/method access is intercepted in `emit_send`.)
        ExprNode::Ivar { name } => ivar_pd_get(name.as_str()),
        ExprNode::Send { recv, method, args, block, .. } => match block {
            Some(blk) => emit_block_call(recv.as_ref(), method.as_str(), args, blk),
            None => emit_send(recv.as_ref(), method.as_str(), args),
        },
        ExprNode::Return { value } => emit_expr(value),
        // `next` inside a fold yields the accumulator unchanged (`next v`
        // yields `v`). Outside a fold there's no accumulator to yield —
        // fall back to the value (or nil), which the surrounding emit
        // handles. (`next` as a loop-skip is lowered earlier by
        // while_to_recursion; what reaches here is a block `next`.)
        ExprNode::Next { value } => match value {
            Some(v) => emit_expr(v),
            None => current_fold_acc().unwrap_or_else(|| "nil".to_string()),
        },
        ExprNode::Raise { value } => format!("raise {}", emit_expr(value)),
        // `yield a, b` → call the block passed as the trailing `block_fn`
        // param (added by emit_fn when the body yields).
        ExprNode::Yield { args } => format!("block_fn.({})", emit_args(args)),
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::MultiAssign { targets, value } => emit_multi_assign(targets, value),
        ExprNode::Seq { exprs } => emit_stmts(exprs),
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = emit_tail(then_branch);
            let else_s = if is_empty(else_branch) {
                "nil".to_string()
            } else {
                emit_tail(else_branch)
            };
            format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            // Ruby `||`/`&&` (and the `or`/`and` keyword forms) are truthy
            // operators returning an operand — Elixir `||`/`&&` match that
            // (nil/false are falsy, anything else truthy). NOT Elixir
            // `or`/`and`, which demand a strict boolean and raise on a
            // `nil` left (`attrs[:id] || 0`, `content_for_get(:x) || ""`).
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        // Atom-key shorthand `name: v`, but a symbol that
                        // isn't a bare Elixir atom (e.g. the hyphenated
                        // `"data-turbo-track":` HTML data attr) must use the
                        // quoted-atom form `"name": v` — still an atom key,
                        // so `render_attrs`'s `to_string/1` sees it uniformly.
                        if is_bare_atom(value.as_str()) {
                            format!("{value}: {}", emit_expr(v))
                        } else {
                            format!("{:?}: {}", value.as_str(), emit_expr(v))
                        }
                    } else {
                        format!("{} => {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        ExprNode::Range { begin, end, exclusive } => {
            let b = begin.as_ref().map(emit_expr).unwrap_or_default();
            let e = end.as_ref().map(emit_expr).unwrap_or_default();
            // Elixir ranges are inclusive `b..e`; exclusive Ruby `b...e`
            // → `b..(e - 1)//1` is awkward, so use `Range` only for the
            // inclusive/endless forms json_builder needs (`20..`).
            if *exclusive {
                format!("{b}..{e}//1")
            } else {
                format!("{b}..{e}")
            }
        }
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::Cast { value, .. } => emit_expr(value),
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms),
        other => crate::emit::diagnostics::report_unsupported("elixir2", other.kind_str(), ""),
    }
}

/// Whether a symbol renders as a bare Elixir atom in `name: v` map
/// shorthand. Bare atoms are identifier-like — leading letter/underscore,
/// then word chars, with an optional trailing `?`/`!`. Anything else (a
/// hyphenated HTML data attr like `data-turbo-track`, leading digit, etc.)
/// needs the quoted-atom form `"name": v`.
fn is_bare_atom(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    let body: String = chars.collect();
    let core = body.strip_suffix(['?', '!']).unwrap_or(&body);
    core.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `case scrutinee do <pat> -> <body> … end` — the per-column index
/// dispatch (`[]`/`[]=`). Patterns are column-name atoms (`Lit::Sym`);
/// a wildcard/bind renders `_`/name.
fn emit_case(scrutinee: &Expr, arms: &[crate::expr::Arm]) -> String {
    let body = arms
        .iter()
        .map(|arm| {
            let pat = emit_pattern(&arm.pattern);
            format!("{pat} ->\n{}", indent(&emit_tail(&arm.body), 1))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("case {} do\n{}\nend", emit_expr(scrutinee), indent(&body, 1))
}

fn emit_pattern(p: &crate::expr::Pattern) -> String {
    use crate::expr::Pattern;
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Bind { name } => name.to_string(),
        Pattern::Lit { value } => emit_literal(value),
        Pattern::Expr { expr } => emit_expr(expr),
        // Array/Record destructure patterns aren't produced by the
        // column indexer; surface a diagnostic if one ever reaches here.
        other => crate::emit::diagnostics::report_unsupported("elixir2", "case pattern", &format!("{other:?}")),
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // A module-singleton's mutable `@ivar` state (e.g. ViewHelpers'
    // `@slots` content_for store) → the process dictionary. Raw `Ivar`
    // nodes only reach emit in `def self.` methods — instance/constructor
    // ivars are already threaded to struct fields by the functionalize
    // passes — so an Ivar receiver here is always module state.
    if let Some(r) = recv {
        if let ExprNode::Ivar { name } = &*r.node {
            if let Some(s) = emit_ivar_state_send(name.as_str(), method, args) {
                return s;
            }
        }
    }

    // Ruby stdlib module calls (`Base64`, `JSON`) → their Elixir
    // equivalents (`Base.encode64`, native `JSON.encode!`).
    if let Some(s) = try_stdlib_const_call(recv, method, args) {
        return s;
    }

    // A recv-less 0-arg call whose name is an in-scope param is a local
    // read, not a call — Ruby resolves a bareword to a local before a
    // method. (A view partial's param `article` collides with the
    // same-named view fn `Views::Articles.article`; without this, the
    // param read emits as a recursive `article()` call.)
    if recv.is_none() && args.is_empty() && is_current_param(method) {
        return method.to_string();
    }
    // A `self.foo(...)` call inside a module is just a same-module
    // bareword call in Elixir — collapse the receiver.
    let recv = match recv {
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => None,
        other => other,
    };

    // A recv-less (or self-recv, collapsed above) 0-arg call whose name is
    // a struct field of the current (record-threading) class is an
    // implicit-self accessor read, not a call — `attr_accessor
    // :request_format` reads emit as `self.request_format` in an action
    // body, a 0-arg self-call to a method that doesn't exist (the accessor
    // is a struct field). Route it to `record.request_format`. Gated on
    // THREADS_RECORD so a module-singleton function isn't given a phantom
    // `record`. (Distinct from `@ivar` reads, which
    // mutation_to_struct_return already bridges to `record.__field__`.)
    if recv.is_none() && args.is_empty() && is_record_threading_context() {
        let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
        if is_struct_field(&class, method) {
            return format!("record.{method}");
        }
    }

    // `self.class.foo(args)` → same-module `foo(args)`: Elixir has no
    // class reflection, and the defining module IS the class. (Real
    // subclass dispatch is handled by the lowerer linearizing these
    // methods per-model; on Base itself the same-module call lands on
    // the stub.) A bare `self.class` → `__MODULE__`.
    if let Some(r) = recv {
        if is_self_class(r) {
            return format!("{}({})", super::library::elixir_fn_name(method), emit_args(args));
        }
    }
    if method == "class" && args.is_empty() && recv.is_none() {
        return "__MODULE__".to_string();
    }

    // `recv.__index_put__(k, v)` (from local_accumulation's `x[k]=v`)
    // rendered by receiver type: a struct routes to its `put` setter,
    // a map (or unknown) to `Map.put`.
    if method == "__index_put__" && args.len() == 2 {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            let (k, v) = (emit_expr(&args[0]), emit_expr(&args[1]));
            if let Some(crate::ty::Ty::Class { id, .. }) = r.ty.as_ref() {
                let module = super::library::v2_module_name(id.0.as_str());
                return format!("{module}.put({r_s}, {k}, {v})");
            }
            return format!("Map.put({r_s}, {k}, {v})");
        }
    }

    // `record.__field__(:x)` → `record.x` (struct field read).
    if method == "__field__" && args.len() == 1 {
        if let Some(r) = recv {
            let field = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                _ => emit_expr(&args[0]),
            };
            return format!("{}.{field}", emit_expr(r));
        }
    }

    // `__tuple__(a, b)` → `{a, b}` — the dual-return bridge (a `save`/
    // `valid?` method returning `{record, value}`; see lower::
    // functionalize::mutation_to_struct_return).
    if method == "__tuple__" && recv.is_none() {
        return format!("{{{}}}", emit_args(args));
    }

    // `record.__struct_put__(:field, value)` → `%{record | field: value}`
    // (the mutation-threading bridge — see lower::functionalize::
    // mutation_to_struct_return).
    if method == "__struct_put__" && args.len() == 2 {
        if let Some(r) = recv {
            let field = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                _ => emit_expr(&args[0]),
            };
            return format!("%{{{} | {field}: {}}}", emit_expr(r), emit_expr(&args[1]));
        }
    }

    // Unary `!x` (Ruby `CallNode` with method `!`, no args) → Elixir
    // `!(x)`. Parenthesized so it binds tighter than any operator in the
    // receiver (`!(a and b)`, `!(is_nil(x))`).
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!({})", emit_expr(r));
        }
    }

    // `raise Class, msg` / `raise msg`. Ruby exception classes (e.g.
    // `NotImplementedError`) mostly have no Elixir module — emitting
    // `raise NotImplementedError, msg` is an undefined-module error. Keep
    // the 2-arg form only for an exception that exists in Elixir;
    // otherwise raise the message string (a `RuntimeError`), preserving
    // the message and staying compile-clean.
    if method == "raise" && recv.is_none() {
        match args {
            [msg] => return format!("raise {}", emit_expr(msg)),
            [class, msg] => {
                if let ExprNode::Const { path } = &*class.node {
                    if let Some(name) = path.last().map(|s| s.to_string()) {
                        if is_elixir_exception(&name) {
                            return format!("raise {name}, {}", emit_expr(msg));
                        }
                    }
                }
                return format!("raise {}", emit_expr(msg));
            }
            _ => {}
        }
    }

    // `.freeze` — Elixir is immutable; the receiver is the value.
    if method == "freeze" && args.is_empty() {
        if let Some(r) = recv {
            return emit_expr(r);
        }
    }

    // `recv.nil?` → `is_nil(recv)`.
    if method == "nil?" && args.is_empty() {
        if let Some(r) = recv {
            return format!("is_nil({})", emit_expr(r));
        }
    }

    // `recv.is_a?(Class)` → Elixir type guard / equality.
    if method == "is_a?" && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            if let Some(class) = path.last() {
                let r_s = emit_expr(r);
                let mapped = match class.as_str() {
                    "TrueClass" => Some(format!("{r_s} == true")),
                    "FalseClass" => Some(format!("{r_s} == false")),
                    "NilClass" => Some(format!("is_nil({r_s})")),
                    "Integer" => Some(format!("is_integer({r_s})")),
                    "Float" => Some(format!("is_float({r_s})")),
                    "Numeric" => Some(format!("is_number({r_s})")),
                    "String" => Some(format!("is_binary({r_s})")),
                    "Array" => Some(format!("is_list({r_s})")),
                    "Hash" => Some(format!("is_map({r_s})")),
                    _ => None,
                };
                if let Some(s) = mapped {
                    return s;
                }
            }
        }
    }

    // `s.gsub(regex, hash)` → `Regex.replace(regex, s, fn m -> Map.get(hash, m) end)`.
    // `s.gsub(needle, repl)` (string args) → `String.replace(s, needle, repl)`.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            let a0 = emit_expr(&args[0]);
            let a1 = emit_expr(&args[1]);
            let hash_repl = matches!(
                &*args[1].node,
                ExprNode::Hash { .. } | ExprNode::Const { .. }
            );
            if hash_repl {
                return format!(
                    "Regex.replace({a0}, {r_s}, fn m -> Map.get({a1}, m, \"\") end)"
                );
            }
            return format!("String.replace({r_s}, {a0}, {a1})");
        }
    }

    // `recv.to_s` → `to_string(recv)`.
    if method == "to_s" && args.is_empty() {
        if let Some(r) = recv {
            return format!("to_string({})", emit_expr(r));
        }
    }

    // `recv.to_i` — Ruby's lenient string→int (`"12abc"` → 12, garbage →
    // 0). Elixir's `String.to_integer/1` raises on anything non-numeric,
    // so route through `Integer.parse/1` with a `0` fallback to preserve
    // the Ruby semantics the params code relies on (`params["id"].to_i`).
    if method == "to_i" && args.is_empty() {
        if let Some(r) = recv {
            return format!(
                "(case Integer.parse({}) do {{n, _}} -> n; :error -> 0 end)",
                emit_expr(r)
            );
        }
    }
    // `recv.to_f` → lenient string→float (`0.0` on failure).
    if method == "to_f" && args.is_empty() {
        if let Some(r) = recv {
            return format!(
                "(case Float.parse({}) do {{n, _}} -> n; :error -> 0.0 end)",
                emit_expr(r)
            );
        }
    }

    // Bareword `name` / `self.name` (receiver collapsed to None above) in
    // a class method is `Module#name` reflection — Elixir module
    // functions have no class-name reflection, so resolve it to the
    // defining class's name string at emit time. Used only in these
    // runtime files' contract-marker raises (`"#{name}.table_name must
    // be overridden"`); no runtime class defines a `name` method/local.
    if method == "name" && args.is_empty() && recv.is_none() {
        let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
        if !class.is_empty() {
            return format!("{class:?}");
        }
    }

    // `recv.length` / `recv.size` — lists use `Kernel.length/1`, strings
    // use `String.length/1`. Driven by the analyzer's `Ty` on the
    // receiver; defaults to `String.length` when the type is unknown.
    // `Kernel.length` is fully qualified so it survives in a module that
    // defines its own `length/1` (e.g. session's HWIA `length` shim),
    // where a bare `length(list)` would be an ambiguous-import error.
    if (method == "length" || method == "size") && args.is_empty() {
        if let Some(r) = recv {
            let r_s = emit_expr(r);
            return if recv_is_array(r) {
                format!("Kernel.length({r_s})")
            } else if recv_is_hash(r) {
                format!("map_size({r_s})")
            } else {
                format!("String.length({r_s})")
            };
        }
    }

    // `recv.empty?` — Array/list → `Enum.empty?`, Hash → `map_size == 0`.
    // An unknown/struct receiver falls through (a struct's own `empty?`
    // method routes via the self-call / method-on-record paths).
    if method == "empty?" && args.is_empty() {
        if let Some(r) = recv {
            if recv_is_array(r) {
                return format!("Enum.empty?({})", emit_expr(r));
            }
            if recv_is_hash(r) {
                return format!("map_size({}) == 0", emit_expr(r));
            }
            if recv_is_string(r) {
                return format!("{} == \"\"", emit_expr(r));
            }
        }
    }

    // `arr.count` → `Enum.count(arr)` for an Array receiver (a list has
    // no `.count` field; `Enum.count` is the size). `count(&block)` /
    // `count(x)` aren't used by the runtime/views, so only the 0-arg form
    // is mapped — anything else falls through.
    if method == "count" && args.is_empty() {
        if let Some(r) = recv {
            if recv_is_array(r) {
                return format!("Enum.count({})", emit_expr(r));
            }
        }
    }

    // `arr.include?(x)` → `Enum.member?(arr, x)` for an Array receiver.
    // (Hash `include?` is key-membership — handled in the Map block.)
    if method == "include?" && args.len() == 1 {
        if let Some(r) = recv {
            if recv_is_array(r) {
                return format!("Enum.member?({}, {})", emit_expr(r), emit_expr(&args[0]));
            }
        }
    }

    // `recv.to_h` — a map is already its own hash in Elixir, so this is
    // the identity (`conditions.to_h` → `conditions`).
    if method == "to_h" && args.is_empty() {
        if let Some(r) = recv {
            return emit_expr(r);
        }
    }

    // `Time.now` → `DateTime.utc_now()` (Elixir's UTC clock). The Ruby
    // idiom `Time.now.utc.iso8601` maps to
    // `DateTime.to_iso8601(DateTime.utc_now())`: `.utc` is the identity
    // (already UTC) and `.iso8601` wraps in `DateTime.to_iso8601`.
    if method == "now" && args.is_empty() {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("Time") {
                    return "DateTime.utc_now()".to_string();
                }
            }
        }
    }
    if method == "utc" && args.is_empty() {
        if let Some(r) = recv {
            return emit_expr(r);
        }
    }
    if method == "iso8601" && args.is_empty() {
        if let Some(r) = recv {
            return format!("DateTime.to_iso8601({})", emit_expr(r));
        }
    }

    // Ruby Hash methods → Elixir `Map.*` (gated on a Hash-typed receiver,
    // so a struct's `key?`/`keys` route to its own methods instead).
    if let Some(r) = recv {
        if recv_is_hash(r) {
            let r_s = emit_expr(r);
            match (method, args.len()) {
                ("keys", 0) => return format!("Map.keys({r_s})"),
                ("values", 0) => return format!("Map.values({r_s})"),
                ("empty?", 0) => return format!("map_size({r_s}) == 0"),
                ("key?" | "has_key?" | "include?", 1) => {
                    return format!("Map.has_key?({r_s}, {})", emit_expr(&args[0]))
                }
                ("fetch", 2) => {
                    return format!("Map.get({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]))
                }
                ("fetch", 1) => return format!("Map.fetch!({r_s}, {})", emit_expr(&args[0])),
                ("delete", 1) => return format!("Map.delete({r_s}, {})", emit_expr(&args[0])),
                _ => {}
            }
        }
    }

    // `self[k]` / `self[k] = v` on the threaded `record` → the renamed
    // same-module accessor (`def []` → `get`, `def []=` → `put`). Threads
    // `record` only when that accessor does (the per-model indexer reads/
    // writes columns → record-threaded; Base's stub raises → pure, so
    // `self[:x]` lands on `get/1`, arity-correct).
    if (method == "[]" || method == "[]=") && recv.is_some_and(is_record_var) {
        let fname = super::library::elixir_fn_name(method);
        if threads_record(&fname) {
            return format!("{fname}(record, {})", emit_args(args));
        }
        return format!("{fname}({})", emit_args(args));
    }
    // `self.foo(args)` / `self.foo` on the threaded `record` is a method
    // call (field reads come through `__field__`, handled above) → the
    // same-module `foo(record, …)`, including 0-arg `self.to_h`. A self-
    // call to a PURE instance method (one that doesn't thread record —
    // e.g. `resolve_status`, which reads only a module constant) drops
    // the record arg to stay arity-correct (`foo(args)`).
    if recv.is_some_and(is_record_var)
        && method.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        // A 0-arg call whose name is a struct field is an `attr_accessor`
        // read, not a method — `self.request_format` → `record.request_format`
        // (field read), not the `request_format()` self-call routing below
        // (which would drop `record` and land on an undefined function,
        // since the accessor has no method body). Distinct from `@ivar`
        // reads, already bridged to `__field__` above.
        if args.is_empty() {
            let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
            if is_struct_field(&class, method) {
                return format!("record.{method}");
            }
        }
        let fname = super::library::elixir_fn_name(method);
        // Spread a trailing keyword-args hash into the callee's defaulted
        // positionals (`render(body, status: :x)` → `render(record, body,
        // :x)`) — render/redirect_to/head are reached here (self-receiver).
        let arg_strs = unpack_kwargs(&fname, args);
        if !threads_record(&fname) {
            return format!("{fname}({})", arg_strs.join(", "));
        }
        return if arg_strs.is_empty() {
            format!("{fname}(record)")
        } else {
            format!("{fname}(record, {})", arg_strs.join(", "))
        };
    }

    // `recv[...]` indexing.
    if method == "[]" && recv.is_some() {
        let r = recv.unwrap();
        let r_s = emit_expr(r);
        // `flash[:notice]` on a struct-typed field (Flash/Session) → the
        // renamed accessor `<Module>.get(recv, key)`. A bare `recv[key]`
        // is `Access.get`, which structs don't implement → a runtime
        // raise. (The `[]` def on those modules was renamed to `get`.)
        if args.len() == 1 {
            // Prefer the explicitly-registered field type (the response-
            // state `flash`/`session` structs) over the node's own ty: the
            // body-typer infers `@flash` as `Hash` from the `[:notice]`
            // indexing, which would route to raw `Access` and raise (a
            // struct isn't Access). The registry knows it's a Flash struct.
            let recv_ty = field_bridge_name(r)
                .and_then(|f| field_type(&CURRENT_CLASS_NAME.with(|n| n.borrow().clone()), &f))
                .filter(|t| matches!(t, crate::ty::Ty::Class { .. }))
                .or_else(|| effective_recv_ty(r));
            if let Some(crate::ty::Ty::Class { id, .. }) = recv_ty {
                let module = super::library::v2_module_name(id.0.as_str());
                return format!("{module}.get({r_s}, {})", emit_expr(&args[0]));
            }
        }
        // List indexing: `list[i]` raises in Elixir (lists aren't Access
        // by integer), so route through `Enum`.
        if recv_is_array(r) {
            if args.len() == 1 {
                if let ExprNode::Range { .. } = &*args[0].node {
                    return format!("Enum.slice({r_s}, {})", emit_expr(&args[0]));
                }
                return format!("Enum.at({r_s}, {})", emit_expr(&args[0]));
            }
            if args.len() == 2 {
                return format!("Enum.slice({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]));
            }
        }
        // Two-arg `recv[start, len]` → string slice.
        if args.len() == 2 {
            return format!("String.slice({r_s}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1]));
        }
        if args.len() == 1 {
            // Range index → string slice. Elixir has no endless-range
            // literal (`20..` is a syntax error), so an open end uses
            // the `start, length` form; bounded ranges slice directly.
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                let b = begin.as_ref().map(emit_expr).unwrap_or_else(|| "0".into());
                return match end {
                    None => format!("String.slice({r_s}, {b}, String.length({r_s}))"),
                    Some(e) if *exclusive => {
                        format!("String.slice({r_s}, {b}..({} - 1)//1)", emit_expr(e))
                    }
                    Some(e) => format!("String.slice({r_s}, {b}..{})", emit_expr(e)),
                };
            }
            // Otherwise a map/keyword access: `map[key]`.
            return format!("{r_s}[{}]", emit_expr(&args[0]));
        }
        return format!("{r_s}[{}]", emit_args(args));
    }

    // Binary operators ride the Send channel. Comparisons map 1:1;
    // `+`/`-`/`*`/`/`/`%`/`**` dispatch on operand type because Elixir's
    // arithmetic operators are numeric-only (strings use `<>`, lists
    // `++`/`--`, etc.). Ported from the legacy elixir emitter.
    if let (Some(r), [arg]) = (recv, args) {
        // `== nil` / `!= nil` → `is_nil/1` guard.
        if method == "==" || method == "!=" {
            use crate::emit::shared::eq::{classify_eq, EqCase};
            if let EqCase::NilCheck { subject } = classify_eq(r, arg) {
                let s = emit_expr(subject);
                return if method == "==" {
                    format!("is_nil({s})")
                } else {
                    format!("not is_nil({s})")
                };
            }
        }
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            // String concatenation is `<>` in Elixir, never `+`. The
            // classifier needs BOTH operands typed `Str`, but the
            // functionalize passes build fresh IR nodes that drop the
            // `.ty` the body-typer set — so a SQL-building chain
            // (`"INSERT…" + Db.escape_string(x) + …`) loses its types.
            // Fall back to a structural check: a `+` whose left spine
            // bottoms out in a string literal/interpolation (or a
            // Str-typed node) IS string concatenation.
            if is_string_rooted(r) || is_string_rooted(arg) {
                return format!("{ls} <> {rs}");
            }
            return match classify_add(r, arg) {
                AddCase::StringConcat => format!("{ls} <> {rs}"),
                AddCase::ArrayConcat { .. } => format!("{ls} ++ {rs}"),
                AddCase::Incompatible => {
                    r#"raise "roundhouse: + with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} + {rs}"),
            };
        }
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => format!("{ls} -- {rs}"),
                SubCase::Incompatible => {
                    r#"raise "roundhouse: - with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} - {rs}"),
            };
        }
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_mul(r, arg) {
                MulCase::StringRepeat => format!("String.duplicate({ls}, {rs})"),
                MulCase::ArrayRepeat { .. } => format!("List.duplicate({ls}, {rs}) |> List.flatten()"),
                MulCase::ArrayJoin { .. } => format!("Enum.join({ls}, {rs})"),
                MulCase::Incompatible => {
                    r#"raise "roundhouse: * with incompatible operand types""#.to_string()
                }
                _ => format!("{ls} * {rs}"),
            };
        }
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            let (ls, rs) = (emit_expr(r), emit_expr(arg));
            return match classify_modulo(r, arg) {
                ModuloCase::NumericPromote => format!(":math.fmod({ls}, {rs})"),
                ModuloCase::StringFormat => {
                    r#"raise "roundhouse: String % (sprintf) not yet supported for Elixir target""#.to_string()
                }
                ModuloCase::Incompatible => {
                    r#"raise "roundhouse: % with incompatible operand types""#.to_string()
                }
                // Numeric/Unknown: rem/2 (integer) or fmod (float recv).
                _ if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Float)) => {
                    format!(":math.fmod({ls}, {rs})")
                }
                _ => format!("rem({ls}, {rs})"),
            };
        }
        if method == "**" {
            return format!(":math.pow({}, {})", emit_expr(r), emit_expr(arg));
        }
        if is_infix(method) {
            return format!("{} {method} {}", emit_expr(r), emit_expr(arg));
        }
    }

    // Ruby String / Hash methods → Elixir module-function calls (Elixir
    // has no `recv.method` dispatch on builtins).
    if let Some(r) = recv {
        let r_s = emit_expr(r);
        match (method, args.len()) {
            ("upcase", 0) => return format!("String.upcase({r_s})"),
            ("downcase", 0) => return format!("String.downcase({r_s})"),
            ("strip", 0) => return format!("String.trim({r_s})"),
            ("split", 1) => return format!("String.split({r_s}, {})", emit_expr(&args[0])),
            ("start_with?", 1) => {
                return format!("String.starts_with?({r_s}, {})", emit_expr(&args[0]))
            }
            ("end_with?", 1) => {
                return format!("String.ends_with?({r_s}, {})", emit_expr(&args[0]))
            }
            // `acc.merge({k => v})` — the threaded-accumulator update
            // emitted by while_to_recursion.
            ("merge", 1) => return format!("Map.merge({r_s}, {})", emit_expr(&args[0])),
            // `arr.join` / `arr.join(sep)` → `Enum.join`. Array-only in
            // Ruby (String has no `join`), so safe to route unconditionally
            // — covers `pins.map {…}.join(",\n")` where the map result is
            // an untyped list.
            ("join", 0) => return format!("Enum.join({r_s})"),
            ("join", 1) => return format!("Enum.join({r_s}, {})", emit_expr(&args[0])),
            // `s.tr(from, to)` → `String.replace` (single-char translation,
            // the only form these runtime files use — `tr("_", "-")`).
            ("tr", 2) => {
                return format!(
                    "String.replace({r_s}, {}, {})",
                    emit_expr(&args[0]),
                    emit_expr(&args[1])
                )
            }
            _ => {}
        }
    }

    // Method/field access on a typed record value — a local/param typed
    // as a model class (`instance.save`, `r.destroy`, `record.id`), as
    // opposed to the threaded `record` self (handled above). A known
    // struct field is a field read (`x.field`); anything else is an
    // instance-method call, dispatched polymorphically through the
    // struct's module so the actual subclass's implementation runs:
    // `x.__struct__.m(x, args)` (record threaded as the first arg).
    if let Some(r) = recv {
        // A `Const` receiver (`Session.new`, `Article.find`) is a CLASS
        // reference — a static call `Module.method(args)`, handled by the
        // default form below — NOT an instance value, even though the
        // class const carries `Ty::Class`. Only route a value receiver
        // (local/param/expression) through the instance dispatch.
        let is_class_ref = matches!(&*r.node, ExprNode::Const { .. });
        // `effective_recv_ty` (not just `r.ty`) so a `record.__field__(:article)`
        // bridge whose body-typer ty was dropped by functionalize still
        // resolves to its registered model type — `record.article.save`
        // routes to a method call, not a `.save` field access.
        if let Some(crate::ty::Ty::Class { id, .. }) = effective_recv_ty(r) {
            if !is_record_var(r) && !is_class_ref {
                let r_s = emit_expr(r);
                if args.is_empty() && is_struct_field(id.0.as_str(), method) {
                    return format!("{r_s}.{method}");
                }
                let fname = super::library::elixir_fn_name(method);
                return if args.is_empty() {
                    format!("{r_s}.__struct__.{fname}({r_s})")
                } else {
                    format!("{r_s}.__struct__.{fname}({r_s}, {})", emit_args(args))
                };
            }
        }
    }

    // Default call forms.
    match recv {
        None => {
            // Bareword — a 0-or-more-arg call to a function in the
            // enclosing module (`encode_string(v)`, `table_name()`). A
            // recv-less `Send` is always a CALL (a bare local read is a
            // `Var` node), so 0-arity gets `()` too — Elixir reads
            // `table_name` without parens as an undefined variable.
            //
            // An implicit-self call to a same-class INSTANCE method
            // threads `record` (we're inside an instance method, so it's
            // in scope) — e.g. `save` inside `save!` → `save(record)` —
            // keeping arity in step with explicit `record.m`/`x.__struct__
            // .m` call sites. Class methods / module functions aren't in
            // the set, so they stay bare.
            //
            // A `*__loop` recursion helper is excluded: `while_to_recursion`
            // already builds its entry/recurse calls with `record` as the
            // explicit first arg, so auto-threading would double it.
            let fname = super::library::elixir_fn_name(method);
            // Spread a trailing keyword-args hash (`render(body, status:
            // :x)`) into the callee's defaulted positionals by name; a
            // plain per-arg render otherwise.
            let arg_strs = unpack_kwargs(&fname, args);
            if threads_record(&fname) && !fname.ends_with("__loop") {
                return if arg_strs.is_empty() {
                    format!("{fname}(record)")
                } else {
                    format!("{fname}(record, {})", arg_strs.join(", "))
                };
            }
            format!("{}({})", method, arg_strs.join(", "))
        }
        Some(r) => {
            let r_s = emit_expr(r);
            // A predicate/bang method (`persisted?`, `save!`) is NEVER a
            // struct field — Elixir field names can't end in `?`/`!`. So
            // even when the receiver's type was dropped (e.g. a recv
            // synthesized by the form lowering, untyped → not caught by the
            // typed method-on-local dispatch above), route it through the
            // struct's module rather than emitting a `.persisted?` field
            // access (a runtime KeyError). A `Const` receiver is excluded:
            // `Db.step?(stmt)` is a static `V2.Db.step?(stmt)` module call,
            // not instance dispatch. `record`-self calls are handled above.
            if (method.ends_with('?') || method.ends_with('!'))
                && !matches!(&*r.node, ExprNode::Const { .. })
            {
                let fname = super::library::elixir_fn_name(method);
                return if args.is_empty() {
                    format!("{r_s}.__struct__.{fname}({r_s})")
                } else {
                    format!("{r_s}.__struct__.{fname}({r_s}, {})", emit_args(args))
                };
            }
            if args.is_empty() {
                return format!("{r_s}.{method}");
            }
            // A qualified cross-module call (`ActionView::ViewHelpers
            // .truncate(body, length: 100)`) whose callee has registered
            // params spreads a trailing keyword-args hash into the callee's
            // defaulted positionals — Elixir has no Ruby kwargs, so passing
            // the hash whole lands it in the first optional's slot. (See
            // MODULE_METHOD_PARAMS; `unpack_kwargs_with` no-ops on a plain
            // arg list, so this is a transparent pass-through otherwise.)
            if let ExprNode::Const { path } = &*r.node {
                if let Some(simple) = path.last() {
                    let fname = super::library::elixir_fn_name(method);
                    let params = module_method_params(simple.as_str(), &fname);
                    let arg_strs = unpack_kwargs_with(params.as_deref(), args);
                    return format!("{r_s}.{method}({})", arg_strs.join(", "));
                }
            }
            format!("{r_s}.{method}({})", emit_args(args))
        }
    }
}

fn emit_args(args: &[Expr]) -> String {
    args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
}

/// Render a bareword call's args, spreading a trailing keyword-args hash
/// into the callee's defaulted positionals BY NAME. Ruby keyword args
/// (`render :new, status: :x`) reach a method whose Elixir signature
/// makes them defaulted positionals (`render(body, status \\ :ok, …)`);
/// the call must place `:x` in `status`'s slot, filling earlier omitted
/// optionals with their defaults. Falls back to a plain per-arg render
/// when the callee is unknown, the last arg isn't a kwargs hash, or a key
/// doesn't match a param (so nothing is silently dropped).
fn unpack_kwargs(fname: &str, args: &[Expr]) -> Vec<String> {
    let params = METHOD_PARAMS.with(|m| m.borrow().get(fname).cloned());
    unpack_kwargs_with(params.as_deref(), args)
}

/// The params of `fname` on the module with simple name `module` (a
/// qualified cross-module callee), if that module was registered. See
/// `MODULE_METHOD_PARAMS`.
fn module_method_params(module: &str, fname: &str) -> Option<Vec<crate::dialect::Param>> {
    MODULE_METHOD_PARAMS.with(|m| m.borrow().get(&format!("{module}#{fname}")).cloned())
}

/// As `unpack_kwargs`, but with the callee's params supplied explicitly —
/// used for qualified cross-module calls (`ViewHelpers.truncate`), whose
/// params live in `MODULE_METHOD_PARAMS`, not the current class's
/// `METHOD_PARAMS`. With `params: None` (callee unknown) it falls back to
/// a plain per-arg render, so nothing is dropped.
fn unpack_kwargs_with(params: Option<&[crate::dialect::Param]>, args: &[Expr]) -> Vec<String> {
    let plain = || args.iter().map(emit_expr).collect::<Vec<_>>();
    let Some((last, head)) = args.split_last() else { return plain() };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else { return plain() };
    let Some(params) = params else {
        return plain();
    };
    if head.len() > params.len() {
        return plain();
    }
    let remaining = &params[head.len()..];
    // Every kwarg key must name one of the remaining params — otherwise
    // unpacking would drop it; fall back to a plain render instead.
    let key_name = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    let all_keys_known = entries.iter().all(|(k, _)| {
        key_name(k).is_some_and(|n| remaining.iter().any(|p| p.name.as_str() == n))
    });
    if !all_keys_known {
        return plain();
    }
    let lookup = |name: &str| -> Option<&Expr> {
        entries.iter().find_map(|(k, v)| (key_name(k).as_deref() == Some(name)).then_some(v))
    };
    let mut out: Vec<String> = head.iter().map(emit_expr).collect();
    let mut tail: Vec<String> = Vec::new();
    let mut last_provided = 0;
    for (i, p) in remaining.iter().enumerate() {
        if let Some(v) = lookup(p.name.as_str()) {
            tail.push(emit_expr(v));
            last_provided = i + 1;
        } else if let Some(d) = &p.default {
            tail.push(emit_expr(d));
        } else {
            // A required param the hash doesn't supply — can't safely
            // reorder; fall back.
            return plain();
        }
    }
    // Drop trailing params left at their defaults (Elixir fills them).
    tail.truncate(last_provided);
    out.extend(tail);
    out
}

/// `recv.each do |x| body end` → an `Enum.*` call with the block as an
/// anonymous function.
///
/// If the block reassigns a single outer local (an accumulator, e.g.
/// `result = result.merge(...)`), it lowers to `Enum.reduce`, threading
/// that local as the accumulator and rebinding it at the call site —
/// the Elixir answer to block-local mutation not leaking. A
/// non-accumulating block uses the directly-mapped `Enum.*` (each/map/
/// filter/…). Multi-accumulator blocks (tuple reduce) aren't covered.
fn emit_block_call(recv: Option<&Expr>, method: &str, args: &[Expr], block: &Expr) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return crate::emit::diagnostics::report_unsupported(
            "elixir2",
            "block",
            &format!("non-lambda block on `{method}`"),
        );
    };
    let recv_s = recv.map(emit_expr).unwrap_or_default();
    let body_s = emit_method_body(body);
    let block_params = params.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    // The `Enum.*` callback receives ONE element per item; a Ruby block
    // with multiple params (`each do |k, v|`) is iterating pairs, so the
    // element destructures as a tuple `{k, v}`.
    let element = match block_params.len() {
        0 => "_".to_string(),
        1 => block_params[0].clone(),
        _ => format!("{{{}}}", block_params.join(", ")),
    };

    let accs = block_accumulators(body, params);
    if accs.len() > 1 {
        return crate::emit::diagnostics::report_unsupported(
            "elixir2",
            "block",
            "block reassigns multiple outer locals (tuple reduce)",
        );
    }
    if let Some(acc) = accs.first() {
        // Accumulating block → reduce, rebinding the acc at the call site.
        return format!("{acc} = {}", emit_reduce(&recv_s, &element, body, acc));
    }

    // Non-accumulating: directly-mapped Enum.* with the block as a fn.
    let enum_fn = enum_method(method);
    let lead = if args.is_empty() {
        String::new()
    } else {
        format!("{}, ", emit_args(args))
    };
    format!(
        "Enum.{enum_fn}({recv_s}, {lead}fn {element} ->\n{}\nend)",
        indent(&body_s, 1),
    )
}

/// `Enum.reduce(recv, acc, fn elem, acc -> <body>; acc end)` — the bare
/// fold expression (no outer `acc =` rebind). The body emits with `acc`
/// pushed as the current fold accumulator (so a `next` inside yields it),
/// and a trailing `acc` appended so the last rebind is preserved as a
/// statement rather than collapsing to its value.
fn emit_reduce(recv_s: &str, element: &str, body: &Expr, acc: &str) -> String {
    let fn_params = format!("{element}, {acc}");
    let acc_var = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
        id: crate::ident::VarId(0),
        name: crate::ident::Symbol::from(acc),
    });
    let mut stmts: Vec<Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![body.clone()],
    };
    stmts.push(acc_var);
    let seq = Expr::new(crate::span::Span::synthetic(), ExprNode::Seq { exprs: stmts });
    FOLD_ACC.with(|s| s.borrow_mut().push(acc.to_string()));
    let threaded = emit_method_body(&seq);
    FOLD_ACC.with(|s| {
        s.borrow_mut().pop();
    });
    format!("Enum.reduce({recv_s}, {acc}, fn {fn_params} ->\n{}\nend)", indent(&threaded, 1))
}

/// If `e` is an accumulating block-call (`coll.each { acc << … }`), the
/// bare fold expression (no `acc =` rebind) — for use as a branch/return
/// value, where the fold's *result* is what the branch yields.
fn try_reduce_value(e: &Expr) -> Option<String> {
    let ExprNode::Send { recv, block: Some(blk), .. } = &*e.node else { return None };
    let ExprNode::Lambda { params, body, .. } = &*blk.node else { return None };
    let acc = block_accumulators(body, params).into_iter().next()?;
    let recv_s = recv.as_ref().map(emit_expr).unwrap_or_default();
    let element = match params.len() {
        0 => "_".to_string(),
        1 => params[0].to_string(),
        _ => format!("{{{}}}", params.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")),
    };
    Some(emit_reduce(&recv_s, &element, body, &acc))
}

/// Outer locals the block reassigns to a value derived from themselves
/// (`v = …v…`) — i.e. accumulators threaded through a fold. Block params
/// and block-local lets (`tmp = x`) are excluded. Recurses through `if`
/// branches and nested accumulating block-calls (the inner fold's `acc =
/// Enum.reduce(…)` rebinds the acc at this level), so an accumulator
/// reassigned only inside nested control flow is still detected.
fn block_accumulators(body: &Expr, params: &[crate::ident::Symbol]) -> Vec<String> {
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => std::slice::from_ref(body),
    };
    let mut out: Vec<String> = Vec::new();
    for s in stmts {
        collect_block_rebinds(s, params, &mut out);
    }
    out
}

/// Collect the locals statement `s` rebinds to a self-derived value (its
/// accumulators), descending into `if` branches, `Seq`s, and nested
/// accumulating block-calls. See [`block_accumulators`].
fn collect_block_rebinds(s: &Expr, params: &[crate::ident::Symbol], out: &mut Vec<String>) {
    let mut add = |name: &str, out: &mut Vec<String>| {
        if !params.iter().any(|p| p.as_str() == name) && !out.iter().any(|a| a == name) {
            out.push(name.to_string());
        }
    };
    match &*s.node {
        // `v = …v…` — a self-referential rebind (excludes a plain `tmp = x`
        // block-local let, which doesn't reference itself).
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if super::library::references_token(&emit_expr(value), name.as_str()) =>
        {
            add(name.as_str(), out);
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            collect_block_rebinds(then_branch, params, out);
            collect_block_rebinds(else_branch, params, out);
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_block_rebinds(e, params, out);
            }
        }
        // A nested accumulating block-call rebinds its accumulator HERE
        // (it emits `acc = Enum.reduce(…)`).
        ExprNode::Send { block: Some(blk), .. } => {
            if let ExprNode::Lambda { params: bp, body, .. } = &*blk.node {
                for acc in block_accumulators(body, bp) {
                    add(&acc, out);
                }
            }
        }
        _ => {
            if let Some(name) = string_builder_append_local(s) {
                add(name, out);
            }
        }
    }
}

/// Map a Ruby Enumerable method to its `Enum` counterpart. Renamed
/// forms are listed; everything else passes through (most names match).
fn enum_method(method: &str) -> &str {
    match method {
        "collect" => "map",
        "select" | "find_all" => "filter",
        "detect" => "find",
        "inject" => "reduce",
        other => other,
    }
}

/// True when the analyzer typed `e` as an `Array` — the signal to use
/// `Enum`/`Kernel.length` rather than the `String`/`Access` forms.
/// The effective type of a receiver — its `.ty` when concrete, else (for
/// a self-record field read `record.__field__(:f)`, whose `.ty` the
/// functionalize passes dropped) the field's recorded type resolved
/// against the class currently being emitted. Lets field reads dispatch
/// on their schema type (`record.title.empty?` → `== ""`) without the
/// body-typer's annotations surviving lowering.
fn effective_recv_ty(e: &Expr) -> Option<crate::ty::Ty> {
    // Field reads resolve via the field-type REGISTRY first — it's
    // authoritative for struct fields, whereas the body-typer's `ty` on a
    // field chain is unreliable (infers `Hash` for `flash`, drops `Array`
    // for `errors`). Only fall back to the node's own `ty` when the field
    // isn't registered.
    // Only a CONCRETE registered type wins over the node's `ty` — a field
    // registered `Untyped` (the default for unclassified struct fields,
    // e.g. a controller's `params`) must fall through to the body-typer's
    // `ty` (which knows `params: Hash`), not clobber it.
    let concrete = |t: crate::ty::Ty| (!matches!(t, crate::ty::Ty::Untyped)).then_some(t);
    // (a) a `record.__field__(:f)` self-bridge → the current class's field.
    if let Some(field) = field_bridge_name(e) {
        let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
        if let Some(t) = field_type(&class, &field).and_then(concrete) {
            return Some(t);
        }
    }
    // (b) a 0-arg field read on a typed receiver (`article.errors` where
    // `article: Article`) → the field's registered type, so a downstream
    // `article.errors.empty?` knows it's an Array. Recurses on the receiver.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if args.is_empty() {
            if let Some(crate::ty::Ty::Class { id, .. }) = effective_recv_ty(r) {
                if let Some(t) = field_type(id.0.as_str(), method.as_str()).and_then(concrete) {
                    return Some(t);
                }
            }
        }
    }
    // (c) a param reference matching a param of the method being emitted →
    // its registered type. View partials carry no IR type on their record
    // param (a bare positional `Param`; functionalize drops body-typer
    // types), so `article: Class{Article}` is recorded in `PARAM_TYPES`
    // and resolved here — letting `article.errors` reach `Array`. A
    // bareword param reads as EITHER a `Var` (when a pass bound it) OR a
    // recv-less 0-arg `Send` (Ruby parses an unbound bareword as a method
    // call) — accept both, matching `set_current_params`' emit handling.
    let param_ref = match &*e.node {
        ExprNode::Var { name, .. } => Some(name.as_str()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.as_str())
        }
        _ => None,
    };
    if let Some(name) = param_ref {
        if is_current_param(name) {
            let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
            if let Some(t) = param_type(&class, name).and_then(concrete) {
                return Some(t);
            }
        }
    }
    // Fall back to the node's own ty.
    match e.ty.as_ref() {
        Some(t) if !matches!(t, crate::ty::Ty::Untyped) => Some(t.clone()),
        _ => None,
    }
}

/// The field name of a `record.__field__(:f)` bridge (a self-record field
/// read), if `e` is one.
fn field_bridge_name(e: &Expr) -> Option<String> {
    if let ExprNode::Send { method, args, .. } = &*e.node {
        if method.as_str() == "__field__" && args.len() == 1 {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn recv_is_array(e: &Expr) -> bool {
    matches!(effective_recv_ty(e), Some(crate::ty::Ty::Array { .. }))
}

/// True when `e` is a `String` (or a string literal) — its `empty?`
/// becomes `== ""` (Elixir has no `String.empty?`).
fn recv_is_string(e: &Expr) -> bool {
    matches!(effective_recv_ty(e), Some(crate::ty::Ty::Str))
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } })
}

/// True when `e` is the threaded `record` var (self) — its `[]`/`[]=`
/// route to the same-module renamed accessor.
fn is_record_var(e: &Expr) -> bool {
    is_record_threading_context()
        && matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "record")
}

/// True when the method currently being emitted threads `record` (an
/// instance method touching self) — so an implicit-self accessor read can
/// route to `record.<field>`.
fn is_record_threading_context() -> bool {
    THREADS_RECORD.with(|t| *t.borrow())
}

/// Process-dictionary key (an atom literal) for a module-singleton's
/// mutable `@ivar` state. Namespaced by the current module so two
/// modules' same-named ivars don't collide in the shared dictionary.
fn ivar_pd_key(name: &str) -> String {
    let class = CURRENT_CLASS_NAME.with(|n| n.borrow().clone());
    let prefix = class.replace("::", "_").to_lowercase();
    if prefix.is_empty() {
        format!(":{name}")
    } else {
        format!(":{prefix}_{name}")
    }
}

/// Read a module-singleton's `@ivar` state — the whole stored value,
/// defaulting to an empty map (the only mutable-state shape these runtime
/// modules use is a hash store).
fn ivar_pd_get(name: &str) -> String {
    format!("Process.get({}, %{{}})", ivar_pd_key(name))
}

/// Ruby stdlib module calls → Elixir equivalents. `Base64.strict_encode64`
/// → `Base.encode64` (same standard-alphabet, padded, single-line output);
/// `JSON.generate`/`dump` → Elixir 1.18's native `JSON.encode!`. `None`
/// for anything else (falls through to the default `Module.method` form).
fn try_stdlib_const_call(recv: Option<&Expr>, method: &str, args: &[Expr]) -> Option<String> {
    let ExprNode::Const { path } = &*recv?.node else { return None };
    let module = path.last()?.as_str();
    match (module, method, args.len()) {
        ("Base64", "strict_encode64" | "encode64", 1) => {
            Some(format!("Base.encode64({})", emit_expr(&args[0])))
        }
        ("Base64", "urlsafe_encode64", 1) => {
            Some(format!("Base.url_encode64({})", emit_expr(&args[0])))
        }
        ("JSON", "generate" | "dump", 1) => {
            Some(format!("JSON.encode!({})", emit_expr(&args[0])))
        }
        _ => None,
    }
}

/// Render a `Send` whose receiver is a module-state `@ivar` (hash store)
/// as the equivalent process-dictionary operation. `None` for an
/// unrecognized shape (falls through to the generic emit).
fn emit_ivar_state_send(name: &str, method: &str, args: &[Expr]) -> Option<String> {
    let key = ivar_pd_key(name);
    let get = ivar_pd_get(name);
    match (method, args.len()) {
        // `@h[k] = v` → put back the updated map.
        ("[]=", 2) => Some(format!(
            "Process.put({key}, Map.put({get}, {}, {}))",
            emit_expr(&args[0]),
            emit_expr(&args[1])
        )),
        // `@h[k]` → `Map.get(store, k)`.
        ("[]", 1) => Some(format!("Map.get({get}, {})", emit_expr(&args[0]))),
        // `@h.fetch(k, default)` → `Map.get(store, k, default)`.
        ("fetch", 2) => {
            Some(format!("Map.get({get}, {}, {})", emit_expr(&args[0]), emit_expr(&args[1])))
        }
        ("fetch", 1) => Some(format!("Map.fetch!({get}, {})", emit_expr(&args[0]))),
        _ => None,
    }
}

/// True when `e` is structurally a String — a string literal,
/// interpolation, a `Str`-typed node, or a `+` concatenation whose left
/// operand is itself string-rooted. Used to recognize string concat
/// (`<>`) when the body-typer's `.ty` was dropped by the functionalize
/// passes (which rebuild IR nodes): a SQL-building `+` chain always
/// roots in a string literal at its leftmost segment.
fn is_string_rooted(e: &Expr) -> bool {
    if matches!(e.ty.as_ref(), Some(crate::ty::Ty::Str)) {
        return true;
    }
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { .. } } => true,
        ExprNode::StringInterp { .. } => true,
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "+" => {
            is_string_rooted(r)
        }
        _ => false,
    }
}

/// True when `e` is `self.class` — a no-arg `class` Send whose receiver
/// is `self`, already collapsed to none, or the threaded `record` var
/// (mutation-threading rewrites `self` → `record`). `self.class.foo` is
/// the only `class` call shape these runtime files use.
fn is_self_class(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv, method, args, .. }
            if method.as_str() == "class"
                && args.is_empty()
                && recv.as_ref().is_none_or(|r| {
                    matches!(&*r.node, ExprNode::SelfRef) || is_record_var(r)
                })
    )
}

/// True when `e` is a `Hash` — route its methods to `Map.*`.
fn recv_is_hash(e: &Expr) -> bool {
    matches!(effective_recv_ty(e), Some(crate::ty::Ty::Hash { .. }))
}

/// Ruby infix operators whose Elixir spelling is identical.
/// Operators whose Elixir spelling is a plain infix (comparisons, plus
/// `/` which is numeric-only in both languages). `+`/`-`/`*`/`%`/`**`
/// are handled by the type-dispatching arms above.
fn is_infix(method: &str) -> bool {
    matches!(
        method,
        // `++`/`--` are generated by local_accumulation (list append /
        // difference); the rest are comparisons + numeric `/`.
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "/" | "and" | "or" | "++" | "--"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{Symbol, VarId};
    use crate::ty::Ty;

    fn var_t(name: &str, ty: Ty) -> Expr {
        let mut e = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(name),
        });
        e.ty = Some(ty);
        e
    }
    fn binop(l: Expr, op: &str, r: Expr) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(l),
            method: Symbol::from(op),
            args: vec![r],
            block: None,
            parenthesized: false,
        })
    }
    fn arr() -> Ty {
        Ty::Array { elem: Box::new(Ty::Untyped) }
    }

    fn unary(method: &str, recv: Expr) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    }

    fn block_send(recv: &str, method: &str, params: &[&str], body: Expr) -> Expr {
        let lambda = Expr::new(crate::span::Span::synthetic(), ExprNode::Lambda {
            params: params.iter().map(|p| Symbol::from(*p)).collect(),
            block_param: None,
            body,
            block_style: Default::default(),
        });
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(var_t(recv, Ty::Untyped)),
            method: Symbol::from(method),
            args: vec![],
            block: Some(lambda),
            parenthesized: false,
        })
    }

    #[test]
    fn block_calls_map_to_enum() {
        let each = block_send("items", "each", &["x"], var_t("x", Ty::Untyped));
        assert_eq!(emit_expr(&each), "Enum.each(items, fn x ->\n  x\nend)");
        // Renames: collect→map, select→filter, detect→find.
        let mapped = block_send("items", "collect", &["x"], var_t("x", Ty::Untyped));
        assert!(emit_expr(&mapped).starts_with("Enum.map(items, fn x ->"));
        let filtered = block_send("items", "select", &["x"], var_t("x", Ty::Untyped));
        assert!(emit_expr(&filtered).starts_with("Enum.filter(items, fn x ->"));
        // Two-param block (`each do |k, v|`) destructures the element tuple.
        let kv = block_send("h", "each", &["k", "v"], var_t("k", Ty::Untyped));
        assert!(emit_expr(&kv).starts_with("Enum.each(h, fn {k, v} ->"));
    }

    #[test]
    fn hash_sym_keys_quote_non_bare_atoms() {
        fn sym(s: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: crate::expr::Literal::Sym { value: Symbol::from(s) },
            })
        }
        fn str_lit(s: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: crate::expr::Literal::Str { value: s.to_string() },
            })
        }
        let hash = Expr::new(crate::span::Span::synthetic(), ExprNode::Hash {
            entries: vec![
                // bare atom → shorthand
                (sym("class"), str_lit("btn")),
                // hyphenated HTML data attr → quoted-atom form (a bare
                // `data-turbo-track:` is a syntax error in Elixir)
                (sym("data-turbo-track"), str_lit("reload")),
            ],
            kwargs: false,
        });
        assert_eq!(
            emit_expr(&hash),
            r#"%{class: "btn", "data-turbo-track": "reload"}"#
        );
    }

    #[test]
    fn accumulating_block_lowers_to_reduce() {
        // items.each do |x| total = total + x end
        let acc_assign = Expr::new(crate::span::Span::synthetic(), ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("total") },
            value: binop(var_t("total", Ty::Int), "+", var_t("x", Ty::Int)),
        });
        let e = block_send("items", "each", &["x"], acc_assign);
        let out = emit_expr(&e);
        assert_eq!(
            out,
            "total = Enum.reduce(items, total, fn x, total ->\n  total = total + x\n  total\nend)",
            "got: {out}"
        );
    }

    #[test]
    fn string_builder_append_in_block_threads_through_reduce() {
        // articles.each do |a| io << render(a) end
        // The append is hinted `StringBuilderAppend` — the emitter renders
        // it as `io = [io, a]`, so `io` must be recognized as the block
        // accumulator (it's a `Send`, not an `Assign`) and the `each`
        // lowered to `Enum.reduce`, not a dead `Enum.each` rebind.
        let mut append = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(var_t("io", Ty::Untyped)),
            method: Symbol::from("<<"),
            args: vec![var_t("a", Ty::Untyped)],
            block: None,
            parenthesized: false,
        });
        append.hint = Some(IrHint::StringBuilderAppend);
        let e = block_send("articles", "each", &["a"], append);
        let out = emit_expr(&e);
        assert_eq!(
            out,
            "io = Enum.reduce(articles, io, fn a, io ->\n  io = [io, a]\n  io\nend)",
            "got: {out}"
        );
    }

    #[test]
    fn string_builder_append_in_if_branch_lifts_to_rebind() {
        // `io << "x" if cond` — a hinted append as the sole then-branch
        // statement (else empty). The cond-rebind lift must hoist it to
        // `io = if cond do [io, "x"] else io end` so the append isn't
        // discarded by the branch scope.
        let mut append = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(var_t("io", Ty::Untyped)),
            method: Symbol::from("<<"),
            args: vec![Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: Literal::Str { value: "x".to_string() },
            })],
            block: None,
            parenthesized: false,
        });
        append.hint = Some(IrHint::StringBuilderAppend);
        let if_stmt = Expr::new(crate::span::Span::synthetic(), ExprNode::If {
            cond: var_t("cond", Ty::Bool),
            then_branch: append,
            else_branch: Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: Literal::Nil,
            }),
        });
        let body = Expr::new(crate::span::Span::synthetic(), ExprNode::Seq {
            exprs: vec![if_stmt, var_t("io", Ty::Untyped)],
        });
        let out = emit_method_body(&body);
        assert!(
            out.contains("io = if cond do\n  [io, \"x\"]\nelse\n  io\nend"),
            "if-branch append should lift to an `io =` rebind, got:\n{out}"
        );
    }

    #[test]
    fn record_param_in_module_fn_keeps_receiver() {
        use crate::ident::ClassId;
        // A genuine `record` param of a module-singleton function (e.g.
        // ViewHelpers.dom_id) is NOT the threaded self — `record.dom_prefix()`
        // must dispatch on the value (`record.__struct__.dom_prefix(record)`),
        // not collapse to a same-module self-call `dom_prefix()` that drops
        // the receiver.
        let record = var_t(
            "record",
            Ty::Class { id: ClassId(Symbol::from("ActiveRecord::Base")), args: vec![] },
        );
        let call = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(record),
            method: Symbol::from("dom_prefix"),
            args: vec![],
            block: None,
            parenthesized: true,
        });
        set_threads_record(false);
        assert_eq!(emit_expr(&call), "record.__struct__.dom_prefix(record)");
        // Inside an instance method `record` is the threaded self, so the
        // same shape collapses to a same-module self-call (receiver folded
        // into the module), not a value dispatch — the behavior the
        // module-fn case must NOT inherit.
        set_threads_record(true);
        assert_eq!(emit_expr(&call), "dom_prefix()");
        set_threads_record(false);
    }

    #[test]
    fn next_in_block_skips_to_accumulator_in_reduce() {
        // coll.each do |x|
        //   next if x                  # If{x, Next, nil}
        //   acc = acc ++ [x]           # accumulator rebind (post local_accum)
        // end
        // → reduce where `next` yields the accumulator unchanged.
        let sp = crate::span::Span::synthetic;
        let next_guard = Expr::new(sp(), ExprNode::If {
            cond: var_t("x", Ty::Untyped),
            then_branch: Expr::new(sp(), ExprNode::Next { value: None }),
            else_branch: Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }),
        });
        let acc_push = Expr::new(sp(), ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("acc") },
            value: binop(
                var_t("acc", arr()),
                "++",
                Expr::new(sp(), ExprNode::Array {
                    elements: vec![var_t("x", Ty::Untyped)],
                    style: Default::default(),
                }),
            ),
        });
        let body = Expr::new(sp(), ExprNode::Seq { exprs: vec![next_guard, acc_push] });
        let out = emit_expr(&block_send("coll", "each", &["x"], body));
        assert!(out.starts_with("acc = Enum.reduce(coll, acc, fn x, acc ->"), "reduce:\n{out}");
        assert!(out.contains("if x do\n    acc\n  else"), "next yields acc unchanged:\n{out}");
        assert!(out.contains("acc = acc ++ [x]"), "accumulation:\n{out}");
    }

    #[test]
    fn nested_if_accumulator_detected_and_threaded() {
        // coll.each do |x|
        //   if x
        //     acc = acc ++ [x]
        //   else
        //     acc = acc ++ [0]
        //   end
        // end
        // The accumulator is rebound only INSIDE the if (not a top-level
        // block statement) — detection must recurse into the branches, and
        // the if lifts to `acc = if x do acc ++ [x] else acc ++ [0] end`.
        let sp = crate::span::Span::synthetic;
        let push = |elem: Expr| {
            Expr::new(sp(), ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from("acc") },
                value: binop(
                    var_t("acc", arr()),
                    "++",
                    Expr::new(sp(), ExprNode::Array { elements: vec![elem], style: Default::default() }),
                ),
            })
        };
        let inner_if = Expr::new(sp(), ExprNode::If {
            cond: var_t("x", Ty::Untyped),
            then_branch: push(var_t("x", Ty::Untyped)),
            else_branch: push(Expr::new(sp(), ExprNode::Lit { value: Literal::Int { value: 0 } })),
        });
        let out = emit_expr(&block_send("coll", "each", &["x"], inner_if));
        assert!(out.starts_with("acc = Enum.reduce(coll, acc, fn x, acc ->"), "outer reduce:\n{out}");
        assert!(out.contains("acc = if x do"), "nested if lifted to acc rebind:\n{out}");
        assert!(out.contains("acc ++ [x]") && out.contains("acc ++ [0]"), "both branches:\n{out}");
    }

    #[test]
    fn module_ivar_state_maps_to_process_dictionary() {
        // A module-singleton's mutable `@slots` hash store → the process
        // dictionary, keyed by the module name so it can't collide.
        set_current_class_name("ActionView::ViewHelpers");
        let ivar = Expr::new(crate::span::Span::synthetic(), ExprNode::Ivar {
            name: Symbol::from("slots"),
        });
        let set = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(ivar.clone()),
            method: Symbol::from("[]="),
            args: vec![var_t("k", Ty::Untyped), var_t("v", Ty::Untyped)],
            block: None,
            parenthesized: false,
        });
        assert_eq!(
            emit_expr(&set),
            "Process.put(:actionview_viewhelpers_slots, \
             Map.put(Process.get(:actionview_viewhelpers_slots, %{}), k, v))"
        );
        let fetch = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(ivar.clone()),
            method: Symbol::from("fetch"),
            args: vec![
                var_t("k", Ty::Untyped),
                Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            ],
            block: None,
            parenthesized: false,
        });
        assert_eq!(
            emit_expr(&fetch),
            "Map.get(Process.get(:actionview_viewhelpers_slots, %{}), k, nil)"
        );
        // Bare read → the whole store.
        assert_eq!(emit_expr(&ivar), "Process.get(:actionview_viewhelpers_slots, %{})");
        set_current_class_name("");
    }

    #[test]
    fn hash_methods_map_to_elixir_map() {
        let hash = || {
            var_t("h", Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) })
        };
        let no_args = |m: &str| Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(hash()),
            method: Symbol::from(m),
            args: vec![],
            block: None,
            parenthesized: false,
        });
        assert_eq!(emit_expr(&no_args("keys")), "Map.keys(h)");
        assert_eq!(emit_expr(&no_args("values")), "Map.values(h)");
        assert_eq!(emit_expr(&no_args("length")), "map_size(h)");
        assert_eq!(emit_expr(&no_args("empty?")), "map_size(h) == 0");
        // key?(k)
        let key_q = Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(hash()),
            method: Symbol::from("key?"),
            args: vec![var_t("k", Ty::Str)],
            block: None,
            parenthesized: false,
        });
        assert_eq!(emit_expr(&key_q), "Map.has_key?(h, k)");
    }

    #[test]
    fn unary_bang_negates() {
        // `!(@notice.nil?)` shape: `!` over a `nil?` call.
        let inner = unary("nil?", var_t("x", Ty::Str));
        assert_eq!(emit_expr(&inner), "is_nil(x)");
        assert_eq!(emit_expr(&unary("!", inner)), "!(is_nil(x))");
    }

    #[test]
    fn operator_dispatch_by_operand_type() {
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Str), "+", var_t("b", Ty::Str))), "a <> b");
        assert_eq!(emit_expr(&binop(var_t("a", arr()), "+", var_t("b", arr()))), "a ++ b");
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Int), "+", var_t("b", Ty::Int))), "a + b");
        assert_eq!(emit_expr(&binop(var_t("a", arr()), "-", var_t("b", arr()))), "a -- b");
        assert_eq!(emit_expr(&binop(var_t("a", Ty::Int), "<", var_t("b", Ty::Int))), "a < b");
    }

    #[test]
    fn cross_file_const_resolution_accumulates() {
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;

        fn lib(name: &str) -> LibraryClass {
            LibraryClass {
                name: ClassId(Symbol::from(name)),
                is_module: false,
                parent: None,
                includes: vec![],
                methods: vec![],
                origin: None,
            }
        }
        fn const_ref(path: &[&str]) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Const {
                path: path.iter().map(|s| Symbol::from(*s)).collect(),
            })
        }

        // Mimic the overlay's pre-registration pass: two SEPARATE units
        // registered in turn (no clear between them).
        clear_modules();
        register_modules([lib("ActionDispatch::Session")].iter());
        register_modules([lib("ActionController::Base")].iter());

        // A reference from the second unit to the first's module resolves
        // — both bare and fully qualified, by last segment. The old
        // clear-on-register behavior would have wiped Session here.
        assert_eq!(emit_expr(&const_ref(&["Session"])), "V2.ActionDispatch.Session");
        assert_eq!(
            emit_expr(&const_ref(&["ActionDispatch", "Session"])),
            "V2.ActionDispatch.Session"
        );
        assert_eq!(emit_expr(&const_ref(&["Base"])), "V2.ActionController.Base");
        // An unregistered const passes through dotted, unchanged.
        assert_eq!(emit_expr(&const_ref(&["SomeUnknown"])), "SomeUnknown");
        clear_modules();
    }

    #[test]
    fn raise_maps_ruby_exception_classes() {
        fn raise_call(args: Vec<Expr>) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
                recv: None,
                method: Symbol::from("raise"),
                args,
                block: None,
                parenthesized: true,
            })
        }
        fn const_ref(name: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Const {
                path: vec![Symbol::from(name)],
            })
        }
        fn str_lit(s: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: Literal::Str { value: s.to_string() },
            })
        }
        clear_modules();
        // A Ruby-only exception class drops to a message-only raise.
        assert_eq!(
            emit_expr(&raise_call(vec![const_ref("NotImplementedError"), str_lit("nope")])),
            r#"raise "nope""#
        );
        // An Elixir-valid exception keeps its class.
        assert_eq!(
            emit_expr(&raise_call(vec![const_ref("ArgumentError"), str_lit("bad")])),
            r#"raise ArgumentError, "bad""#
        );
        // Single-arg `raise msg` is unchanged.
        assert_eq!(emit_expr(&raise_call(vec![str_lit("boom")])), r#"raise "boom""#);
    }

    #[test]
    fn pure_instance_method_self_call_drops_record() {
        use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param, AccessorKind};
        use crate::effect::EffectSet;
        use crate::ident::ClassId;

        fn sym(s: &str) -> Symbol { Symbol::from(s) }
        fn syn(node: ExprNode) -> Expr { Expr::new(crate::span::Span::synthetic(), node) }
        fn m(name: &str, params: &[&str], body: Expr) -> MethodDef {
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
        // def render(s); @status = resolve_status(s); end  (threads record)
        // def resolve_status(s); STATUS_CODES.fetch(s, 200); end  (pure)
        let self_call = syn(ExprNode::Send {
            recv: Some(syn(ExprNode::SelfRef)),
            method: sym("resolve_status"),
            args: vec![syn(ExprNode::Var { id: VarId(0), name: sym("s") })],
            block: None,
            parenthesized: true,
        });
        let render = m("render", &["s"], syn(ExprNode::Assign {
            target: crate::expr::LValue::Ivar { name: sym("status") },
            value: self_call,
        }));
        let resolve = m("resolve_status", &["s"], syn(ExprNode::Send {
            recv: Some(syn(ExprNode::Const { path: vec![sym("STATUS_CODES")] })),
            method: sym("fetch"),
            args: vec![
                syn(ExprNode::Var { id: VarId(0), name: sym("s") }),
                syn(ExprNode::Lit { value: Literal::Int { value: 200 } }),
            ],
            block: None,
            parenthesized: true,
        }));
        let class = LibraryClass {
            name: ClassId(sym("ActionController::Base")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![render, resolve],
            origin: None,
        };
        let class = crate::lower::functionalize::functionalize(vec![class]).pop().unwrap();
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        // Uniform record threading: EVERY instance method takes a leading
        // `record` param (a pure one — `resolve_status` reads only a
        // constant — names it `_record`), and the self-call passes it. So
        // self-calls, bareword implicit-self, and `x.__struct__.m` dispatch
        // all agree on arity.
        assert!(ex.contains("def resolve_status(_record, s)"), "pure method takes _record:\n{ex}");
        assert!(ex.contains("resolve_status(record, s)"), "self-call threads record:\n{ex}");
        assert!(ex.contains("def render(record, s)"), "render threads record:\n{ex}");
    }

    #[test]
    fn method_on_typed_local_dispatches_via_struct() {
        use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param, AccessorKind};
        use crate::effect::EffectSet;
        use crate::ident::ClassId;
        fn sym(s: &str) -> Symbol { Symbol::from(s) }
        fn syn(node: ExprNode) -> Expr { Expr::new(crate::span::Span::synthetic(), node) }

        // `instance.save` where `instance: Ty::Class{Foo}` (a value, not
        // the threaded record) → polymorphic `instance.__struct__.save(
        // instance)`. `instance.id` (a struct field) → field read.
        let mut instance = syn(ExprNode::Var { id: VarId(0), name: sym("instance") });
        instance.ty = Some(Ty::Class { id: ClassId(sym("Foo")), args: vec![] });
        let save_call = call(instance.clone(), "save", vec![]);
        let id_read = call(instance, "id", vec![]);

        // Register Foo's struct fields so `id` is a field, `save` a method.
        super::register_field_names("Foo", &["id".to_string()]);
        assert_eq!(emit_expr(&save_call), "instance.__struct__.save(instance)");
        assert_eq!(emit_expr(&id_read), "instance.id");
        super::clear_field_names();
        // After clearing, `id` (unknown field) routes as a method call.
        assert_eq!(
            emit_expr(&call(
                {
                    let mut v = syn(ExprNode::Var { id: VarId(0), name: sym("instance") });
                    v.ty = Some(Ty::Class { id: ClassId(sym("Foo")), args: vec![] });
                    v
                },
                "id",
                vec![]
            )),
            "instance.__struct__.id(instance)"
        );
    }

    fn call(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }
    fn const_path(name: &str) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Const {
            path: vec![Symbol::from(name)],
        })
    }

    #[test]
    fn container_query_methods_dispatch_on_type() {
        // Array receiver → Enum.*; Hash receiver → Map.* / map_size.
        let arr = || var_t("xs", arr());
        let hsh = || var_t("h", Ty::Hash { key: Box::new(Ty::Untyped), value: Box::new(Ty::Untyped) });
        assert_eq!(emit_expr(&call(arr(), "empty?", vec![])), "Enum.empty?(xs)");
        assert_eq!(emit_expr(&call(hsh(), "empty?", vec![])), "map_size(h) == 0");
        assert_eq!(
            emit_expr(&call(arr(), "include?", vec![var_t("y", Ty::Untyped)])),
            "Enum.member?(xs, y)"
        );
        // Hash include? is key membership.
        assert_eq!(
            emit_expr(&call(hsh(), "include?", vec![var_t("k", Ty::Untyped)])),
            "Map.has_key?(h, k)"
        );
        // String receiver → `== ""` (Elixir has no String.empty?).
        assert_eq!(emit_expr(&call(var_t("s", Ty::Str), "empty?", vec![])), "s == \"\"");
    }

    #[test]
    fn field_type_registry_drives_field_read_dispatch() {
        use crate::ident::ClassId;
        fn field_bridge(field: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
                recv: Some(Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
                    id: VarId(0),
                    name: Symbol::from("record"),
                })),
                method: Symbol::from("__field__"),
                args: vec![Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                    value: Literal::Sym { value: Symbol::from(field) },
                })],
                block: None,
                parenthesized: true,
            })
        }
        clear_field_names();
        register_field_types("Article", &[("title".to_string(), Ty::Str)]);
        register_field_types("ArticleRow", &[("id".to_string(), Ty::Int)]);
        set_current_class_name("Article");

        // A self-record field read resolves its type via the registry +
        // current class (the body-typer's `.ty` didn't survive lowering):
        // `record.title.empty?` → `record.title == ""`.
        assert_eq!(emit_expr(&call(field_bridge("title"), "empty?", vec![])), "record.title == \"\"");

        // A registered field on a typed-Class value reads as a field, not
        // a method dispatch: `row.id` → `row.id`.
        let mut row = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
            id: VarId(0),
            name: Symbol::from("row"),
        });
        row.ty = Some(Ty::Class { id: ClassId(Symbol::from("ArticleRow")), args: vec![] });
        assert_eq!(emit_expr(&call(row, "id", vec![])), "row.id");

        set_current_class_name("");
        clear_field_names();
    }

    #[test]
    fn kwargs_hash_spreads_into_defaulted_positionals() {
        use crate::dialect::Param;
        let sym_lit = |s: &str| {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
                value: Literal::Sym { value: Symbol::from(s) },
            })
        };
        let kwargs = |pairs: Vec<(&str, Expr)>| {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Hash {
                entries: pairs.into_iter().map(|(k, v)| (sym_lit(k), v)).collect(),
                kwargs: true,
            })
        };
        // render(body, status \\ :ok, content_type \\ nil, location \\ nil)
        let nil = || Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
        let params = vec![
            Param::positional(Symbol::from("body")),
            Param::with_default(Symbol::from("status"), sym_lit("ok")),
            Param::with_default(Symbol::from("content_type"), nil()),
            Param::with_default(Symbol::from("location"), nil()),
        ];
        let mut map = HashMap::new();
        map.insert("render".to_string(), params);
        set_method_params(map);

        // Only `status:` given → trailing defaulted params dropped.
        let only_status =
            vec![var_t("body", Ty::Untyped), kwargs(vec![("status", sym_lit("unprocessable_content"))])];
        assert_eq!(unpack_kwargs("render", &only_status), vec!["body", ":unprocessable_content"]);

        // `status:` + `location:` (skipping content_type) → the skipped
        // optional is filled with its default so `location` lands right.
        let status_and_loc = vec![
            var_t("body", Ty::Untyped),
            kwargs(vec![("status", sym_lit("created")), ("location", var_t("loc", Ty::Untyped))]),
        ];
        assert_eq!(
            unpack_kwargs("render", &status_and_loc),
            vec!["body", ":created", "nil", "loc"]
        );

        // An unknown callee falls back to a plain per-arg render.
        assert_eq!(unpack_kwargs("unknown_fn", &only_status).len(), 2);
        set_method_params(HashMap::new());
    }

    #[test]
    fn qualified_cross_module_call_spreads_kwargs() {
        use crate::dialect::{
            AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param,
        };
        use crate::effect::EffectSet;
        use crate::ident::ClassId;
        let sp = || crate::span::Span::synthetic();
        let int = |n: i64| Expr::new(sp(), ExprNode::Lit { value: Literal::Int { value: n } });
        let str_lit =
            |s: &str| Expr::new(sp(), ExprNode::Lit { value: Literal::Str { value: s.into() } });
        let sym_lit = |s: &str| Expr::new(sp(), ExprNode::Lit {
            value: Literal::Sym { value: Symbol::from(s) },
        });
        // `def truncate(s, length = 30, omission = "...")` on ViewHelpers.
        let truncate = MethodDef {
            name: Symbol::from("truncate"),
            receiver: MethodReceiver::Class,
            params: vec![
                Param::positional(Symbol::from("s")),
                Param::with_default(Symbol::from("length"), int(30)),
                Param::with_default(Symbol::from("omission"), str_lit("...")),
            ],
            block_param: None,
            body: Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }),
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        };
        let vh = LibraryClass {
            name: ClassId(Symbol::from("ActionView::ViewHelpers")),
            is_module: true,
            parent: None,
            includes: vec![],
            methods: vec![truncate],
            origin: None,
        };
        clear_modules();
        register_modules(std::iter::once(&vh));

        // `ActionView::ViewHelpers.truncate(body, length: 100)` — the kwargs
        // hash spreads into the `length` positional (`omission` trails at
        // its default → dropped), NOT passed as a literal map.
        let kwargs = Expr::new(sp(), ExprNode::Hash {
            entries: vec![(sym_lit("length"), int(100))],
            kwargs: true,
        });
        let call_expr = call(
            const_path("ViewHelpers"),
            "truncate",
            vec![var_t("body", Ty::Untyped), kwargs],
        );
        assert_eq!(
            emit_expr(&call_expr),
            "V2.ActionView.ViewHelpers.truncate(body, 100)"
        );
        clear_modules();
    }

    #[test]
    fn param_type_registry_types_view_partial_record() {
        use crate::ident::ClassId;
        // A view partial's record param (`article`) carries no IR type, so
        // `PARAM_TYPES` records it. `article.errors` (errors: Array) then
        // routes its container queries to `Enum.*` rather than a struct
        // dispatch on a list. The param reads as a recv-less bareword Send
        // (Ruby parses an unbound bareword as a method call) — the shape
        // the view lowering actually produces.
        fn bareword_field(param: &str, field: &str) -> Expr {
            call(bare_call(param, vec![]), field, vec![])
        }
        clear_field_names();
        clear_param_types();
        register_field_types(
            "Article",
            &[("errors".to_string(), Ty::Array { elem: Box::new(Ty::Str) })],
        );
        register_param_types(
            "Views::Articles",
            &[(
                "article".to_string(),
                Ty::Class { id: ClassId(Symbol::from("Article")), args: vec![] },
            )],
        );
        set_current_class_name("Views::Articles");
        set_current_params(["article".to_string()].into_iter().collect());

        let errors = || bareword_field("article", "errors");
        assert_eq!(emit_expr(&call(errors(), "empty?", vec![])), "Enum.empty?(article.errors)");
        assert_eq!(emit_expr(&call(errors(), "count", vec![])), "Enum.count(article.errors)");

        // A name that ISN'T a param of the current method doesn't resolve
        // (no phantom typing of an unrelated local).
        set_current_params(std::collections::HashSet::new());
        assert_ne!(emit_expr(&call(errors(), "count", vec![])), "Enum.count(article.errors)");

        set_current_params(std::collections::HashSet::new());
        set_current_class_name("");
        clear_param_types();
        clear_field_names();
    }

    #[test]
    fn to_h_is_identity() {
        assert_eq!(emit_expr(&call(var_t("conditions", Ty::Untyped), "to_h", vec![])), "conditions");
    }

    #[test]
    fn time_now_utc_iso8601_maps_to_datetime() {
        // Time.now.utc.iso8601 → DateTime.to_iso8601(DateTime.utc_now())
        let now = call(const_path("Time"), "now", vec![]);
        let utc = call(now, "utc", vec![]);
        let iso = call(utc, "iso8601", vec![]);
        assert_eq!(emit_expr(&iso), "DateTime.to_iso8601(DateTime.utc_now())");
    }

    fn bare_call(method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv: None,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }
    fn self_ref() -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::SelfRef)
    }

    #[test]
    fn zero_arg_bareword_call_gets_parens() {
        // A recv-less Send is a CALL; 0-arity needs `()` (bare `table_name`
        // would be an undefined variable in Elixir).
        assert_eq!(emit_expr(&bare_call("table_name", vec![])), "table_name()");
        assert_eq!(
            emit_expr(&bare_call("foo", vec![var_t("x", Ty::Untyped)])),
            "foo(x)"
        );
    }

    #[test]
    fn self_class_routes_to_same_module() {
        // `self.class.schema_columns` → `schema_columns()` (Elixir's
        // module IS the class). Covers both the `self`- and threaded-
        // `record`-receiver forms.
        // `record` is the threaded self only inside an instance method.
        set_threads_record(true);
        let via_self = call(call(self_ref(), "class", vec![]), "schema_columns", vec![]);
        assert_eq!(emit_expr(&via_self), "schema_columns()");
        let via_record =
            call(call(var_t("record", Ty::Untyped), "class", vec![]), "schema_columns", vec![]);
        assert_eq!(emit_expr(&via_record), "schema_columns()");
        set_threads_record(false);
    }

    #[test]
    fn name_reflection_resolves_to_class_string() {
        set_current_class_name("ActiveRecord::Base");
        assert_eq!(emit_expr(&bare_call("name", vec![])), "\"ActiveRecord::Base\"");
        set_current_class_name("");
    }

    #[test]
    fn array_join_and_string_tr() {
        let xs = || var_t("xs", arr());
        assert_eq!(emit_expr(&call(xs(), "join", vec![])), "Enum.join(xs)");
        assert_eq!(
            emit_expr(&call(xs(), "join", vec![str_lit_e(", ")])),
            "Enum.join(xs, \", \")"
        );
        assert_eq!(
            emit_expr(&call(var_t("s", Ty::Str), "tr", vec![str_lit_e("_"), str_lit_e("-")])),
            "String.replace(s, \"_\", \"-\")"
        );
    }

    #[test]
    fn declared_constant_is_attr_but_module_ref_is_not() {
        fn const_ref(name: &str) -> Expr {
            Expr::new(crate::span::Span::synthetic(), ExprNode::Const {
                path: vec![Symbol::from(name)],
            })
        }
        clear_declared_constants();
        register_declared_constant("ESCAPES");
        // A declared SCREAMING_SNAKE constant → module attribute.
        assert_eq!(emit_expr(&const_ref("ESCAPES")), "@escapes");
        // An all-caps MODULE reference (not declared) stays a module name,
        // not a bogus `@json` attribute.
        assert_eq!(emit_expr(&const_ref("JSON")), "JSON");
        clear_declared_constants();
    }

    fn str_lit_e(s: &str) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Lit {
            value: Literal::Str { value: s.to_string() },
        })
    }

    #[test]
    fn string_rooted_plus_chain_emits_concat() {
        // A `+` chain rooted in a string literal with UNTYPED operands
        // (the functionalize passes drop the body-typer's `.ty`) still
        // emits `<>`, not `+` — the SQL-building concat shape.
        let inner = call(str_lit_e("SELECT "), "+", vec![var_t("col", Ty::Untyped)]);
        let outer = call(inner, "+", vec![var_t("tail", Ty::Untyped)]);
        assert_eq!(emit_expr(&outer), "\"SELECT \" <> col <> tail");
        // A genuinely numeric `+` (no string root) stays `+`.
        assert_eq!(emit_expr(&call(var_t("m", Ty::Int), "+", vec![var_t("n", Ty::Int)])), "m + n");
    }
}

/// Exception modules that exist in Elixir's standard library, so a
/// Ruby `raise Class, msg` keeps its class. Anything else (Ruby-only
/// `NotImplementedError`, Rails' `RecordNotFound`, …) falls back to a
/// message-only `raise` (a `RuntimeError`).
fn is_elixir_exception(name: &str) -> bool {
    matches!(
        name,
        "ArgumentError" | "RuntimeError" | "KeyError" | "ArithmeticError"
    )
}

fn emit_const(path: &[crate::ident::Symbol]) -> String {
    // A DECLARED SCREAMING_SNAKE constant → module attribute (`ESCAPES`
    // → `@escapes`). Gated on the declared-constant registry so an
    // all-caps *module* reference (`JSON`, `IO`, `URI`) — which has the
    // same shape but isn't a constant — stays a module name rather than
    // becoming a bogus `@json` attribute.
    if path.len() == 1
        && is_screaming_snake(path[0].as_str())
        && DECLARED_CONSTANTS.with(|c| c.borrow().contains(path[0].as_str()))
    {
        return format!("@{}", path[0].as_str().to_lowercase());
    }
    // A reference to a sibling module in the same unit — whether bare
    // (`MatchResult`) or fully qualified (`ActionDispatch::Router::
    // MatchResult`) — resolves by its last segment to the emitted
    // `V2.*` name.
    if let Some(last) = path.last() {
        if let Some(full) = MODULE_NAMES.with(|m| m.borrow().get(last.as_str()).cloned()) {
            return full;
        }
    }
    path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("\"");
    for p in parts {
        match p {
            InterpPart::Text { value } => push_escaped(&mut out, value),
            InterpPart::Expr { expr } => {
                out.push_str("#{");
                out.push_str(&emit_expr(expr));
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}

/// Escape a string for an Elixir double-quoted literal body. Handles
/// the quote/backslash/`#` (interpolation) cases plus the control
/// characters json_builder's `ESCAPES` keys carry as raw bytes
/// (backspace `\b` 0x08, form feed `\f` 0x0c, …). Other control chars
/// fall back to `\xHH`.
fn push_escaped(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            other => out.push(other),
        }
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => format!("{value:?}"),
        Literal::Str { value } => emit_str_literal(value),
        Literal::Sym { value } => format!(":{value}"),
        Literal::Regex { pattern, flags } => emit_regex(pattern, flags),
    }
}

fn emit_str_literal(value: &str) -> String {
    let mut out = String::from("\"");
    push_escaped(&mut out, value);
    out.push('"');
    out
}

/// Ruby `/pat/flags` → Elixir `~r/pat/flags`. `pattern` is the regex
/// source text (backslash escapes preserved). Ruby's `m` flag (dotall)
/// maps to Elixir's `s`; `i`/`x` carry over.
fn emit_regex(pattern: &str, flags: &str) -> String {
    let escaped = pattern.replace('/', "\\/");
    let ex_flags: String = flags
        .chars()
        .filter_map(|f| match f {
            'i' => Some('i'),
            'm' => Some('s'),
            'x' => Some('x'),
            _ => None,
        })
        .collect();
    format!("~r/{escaped}/{ex_flags}")
}

fn is_screaming_snake(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.chars().any(|c| c.is_ascii_uppercase())
}

/// Indent every non-empty line by `levels * 2` spaces.
pub(super) fn indent(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
