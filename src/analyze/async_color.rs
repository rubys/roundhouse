//! Async coloring — Phase 1 (seed pass).
//!
//! Marks methods on the active deployment profile's adapter class as
//! `is_async = true`. These are the **seeds** the Phase 2 propagation
//! pass grows from: every method whose body calls a seed-marked
//! method becomes async itself, transitively.
//!
//! Today's scope is the seed pass only. Phase 2 adds propagation in
//! this same module; Phase 3 wires the TS emitter to consume the
//! flag.
//!
//! Phase-1 invariant: under a sync profile (`node-sync`), the seed
//! list is empty (`SqliteAdapter::async_seed_methods()` returns
//! `&[]`), so `seed_from_adapter` mutates nothing and emit output
//! is unchanged. That's Gate 1 from `project_async_coloring_plan.md`.

use std::collections::HashSet;

use crate::adapter::DatabaseAdapter;
use crate::dialect::{AccessorKind, LibraryClass, LibraryFunction};
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Pattern};
use crate::ident::{ClassId, Symbol};

// Thread-local extern set for propagation -----------------------------
//
// Set by the public emit entrypoint (`emit_with_profile`) so that
// per-target emit pipelines can apply propagation to lowered class
// Vecs / runtime units without threading the extern list through
// every helper signature. Empty by default — the sync-profile path
// reads it and skips propagation entirely (matches pre-Phase-3
// behavior, satisfies Gate 1).

std::thread_local! {
    static EXTERN_ASYNC_NAMES: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with `extern_names` as the active extern-async list.
/// Restores the previous value on return.
pub fn with_extern_async_names<F, R>(extern_names: Vec<&'static str>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev =
        EXTERN_ASYNC_NAMES.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), extern_names));
    let r = f();
    EXTERN_ASYNC_NAMES.with(|cell| *cell.borrow_mut() = prev);
    r
}

/// Snapshot of the active extern-async list. Per-target emit
/// pipelines call this and pass the result to `propagate_with_externs`
/// for each LibraryClass Vec they have in hand.
pub fn active_extern_async_names() -> Vec<&'static str> {
    EXTERN_ASYNC_NAMES.with(|cell| cell.borrow().clone())
}

/// Walk `classes` and set `is_async = true` on methods named in
/// `seed_methods`, restricted to the class whose name matches
/// `adapter_class`. Returns the number of methods marked.
///
/// Restricting to a single class prevents false positives — a
/// user-defined `def all` on a model class shares the name with
/// `SqliteAdapter#all` but isn't itself a Promise-returning
/// driver call. The propagation pass (Phase 2) only marks via
/// transitive calls, so seed precision matters: each false
/// positive is a method we'll incorrectly flag, forcing every
/// caller to await something that isn't a Promise.
pub fn seed_async_methods(
    classes: &mut [LibraryClass],
    adapter_class: &str,
    seed_methods: &[&str],
) -> usize {
    let mut count = 0;
    for class in classes.iter_mut() {
        if class.name.0.as_str() != adapter_class {
            continue;
        }
        for method in class.methods.iter_mut() {
            if seed_methods.iter().any(|s| *s == method.name.as_str()) {
                method.is_async = true;
                count += 1;
            }
        }
    }
    count
}

/// Convenience: pull the seed list from an `adapter` and call
/// `seed_async_methods`. The adapter class name is still caller-
/// supplied because the Rust `DatabaseAdapter` impl doesn't yet
/// declare its corresponding Ruby/transpiled class name (the
/// mapping varies by target — `SqliteAdapter`, `LibsqlAdapter`,
/// `BetterSqlite3Adapter`, etc.).
pub fn seed_from_adapter(
    classes: &mut [LibraryClass],
    adapter_class: &str,
    adapter: &dyn DatabaseAdapter,
) -> usize {
    seed_async_methods(classes, adapter_class, adapter.async_seed_methods())
}

/// Result of a propagation run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropagationResult {
    /// How many methods became async during propagation (excluding
    /// the seed marks set before propagation ran).
    pub newly_marked: usize,
    /// Number of fixed-point iterations executed before convergence.
    /// `1` means the first sweep marked everything reachable.
    pub iterations: usize,
    /// Methods on structurally-sync slots (attribute readers, attribute
    /// writers, constructors) that got colored by propagation. Each
    /// is a target-time error candidate — sync slots can't be async
    /// in TypeScript (`get foo()`/`set foo(v)`/`constructor()` can't
    /// be marked `async`). Surfaced here so the emit stage can either
    /// elevate to a build error or rewrite the structurally-sync slot
    /// into a regular method.
    pub sync_slot_violations: Vec<SyncSlotViolation>,
}

/// A method that was async-colored despite being in a structurally
/// sync slot. The triple identifies the violation; the kind names
/// which slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncSlotViolation {
    pub class: ClassId,
    pub method: Symbol,
    pub kind: SyncSlotKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncSlotKind {
    /// `attr_reader` / `attr_accessor` getter — TS emits a
    /// property/getter, no `async` allowed.
    AttributeReader,
    /// `attr_writer` / `attr_accessor` setter — TS emits a
    /// property/setter, no `async` allowed.
    AttributeWriter,
    /// `def initialize` — TS emits a `constructor()`, no `async`
    /// allowed; an async constructor is a TypeScript error.
    Constructor,
}

/// Fixed-point propagation: any method whose body calls a
/// currently-async method becomes async itself. Iterate until no
/// new flags flip. Run AFTER `seed_async_methods` so the seed set
/// is in place; `propagate` only grows the set, never seeds it.
///
/// Resolution: name-based across all classes in `classes`. A
/// `Send { method: m, .. }` is treated as a call to an async
/// method iff some class has an async method named `m`. This
/// over-approximates when names collide across classes; the
/// downside is over-marking (extra `await`s emitted, runtime
/// no-op for non-Promise values), not incorrect emit. A future
/// refinement can plug in the typer's callee→def lookup for
/// receiver-aware resolution.
///
/// Block bodies color the **enclosing** method, not the block
/// itself: the walker descends through Lambda bodies as part of
/// the same method, so an async call inside `posts.each { |p|
/// p.save }` colors the enclosing method. This matches the TS
/// HOF strategy in Phase 3 (chain rewrites to `for...of` so the
/// awaits land in the enclosing async function).
pub fn propagate(classes: &mut [LibraryClass]) -> PropagationResult {
    propagate_with_externs(classes, &[])
}

/// Same as `propagate`, but seeds the async-name set with `extern_async_names`
/// in addition to the in-class set. Use when the adapter methods that
/// drive the seed are NOT in the IR — they live in hand-written
/// target code (TS `runtime/typescript/juntos.ts`, Crystal
/// `runtime/crystal/db.cr`, …) so there's no MethodDef to mark.
/// Method bodies that Send to a name in `extern_async_names` still
/// get propagated to as if the name resolved to an async MethodDef.
pub fn propagate_with_externs(
    classes: &mut [LibraryClass],
    extern_async_names: &[&str],
) -> PropagationResult {
    propagate_global_with_externs(classes, &mut [], extern_async_names)
}

/// Global propagation across multiple class Vecs **and** a slice of
/// free functions. All inputs share one async-name set so cross-Vec
/// chains (controller method → model helper → extern) and
/// class-to-function chains (controller method → route helper
/// function → extern) propagate to convergence in one pass.
///
/// `classes` and `functions` are both `&mut` because propagation
/// flips `is_async` in place. Pass empty slices when a category
/// doesn't apply at the call site.
pub fn propagate_global_with_externs(
    classes: &mut [LibraryClass],
    functions: &mut [LibraryFunction],
    extern_async_names: &[&str],
) -> PropagationResult {
    let extern_set: HashSet<Symbol> = extern_async_names
        .iter()
        .map(|s| Symbol::from(*s))
        .collect();
    let mut result = PropagationResult::default();
    loop {
        let mut async_names = collect_async_method_names(classes);
        for f in functions.iter() {
            if f.is_async {
                async_names.insert(f.name.clone());
            }
        }
        async_names.extend(extern_set.iter().cloned());
        let mut marked_this_pass = 0;
        for class in classes.iter_mut() {
            for method in class.methods.iter_mut() {
                if method.is_async {
                    continue;
                }
                if body_calls_async(&method.body, &async_names) {
                    method.is_async = true;
                    marked_this_pass += 1;
                }
            }
        }
        for func in functions.iter_mut() {
            if func.is_async {
                continue;
            }
            if body_calls_async(&func.body, &async_names) {
                func.is_async = true;
                marked_this_pass += 1;
            }
        }
        result.iterations += 1;
        result.newly_marked += marked_this_pass;
        if marked_this_pass == 0 {
            break;
        }
    }
    result.sync_slot_violations = find_sync_slot_violations(classes);
    result
}

/// Walk `classes` and collect violations: methods marked
/// `is_async = true` whose slot is structurally sync (attr
/// reader/writer or `initialize`). Used by `propagate`; exposed
/// publicly so callers can re-check after manual mutation.
pub fn find_sync_slot_violations(classes: &[LibraryClass]) -> Vec<SyncSlotViolation> {
    let mut out = Vec::new();
    for class in classes {
        for method in &class.methods {
            if !method.is_async {
                continue;
            }
            let kind = match method.kind {
                AccessorKind::AttributeReader => Some(SyncSlotKind::AttributeReader),
                AccessorKind::AttributeWriter => Some(SyncSlotKind::AttributeWriter),
                AccessorKind::Method if method.name.as_str() == "initialize" => {
                    Some(SyncSlotKind::Constructor)
                }
                AccessorKind::Method => None,
            };
            if let Some(kind) = kind {
                out.push(SyncSlotViolation {
                    class: class.name.clone(),
                    method: method.name.clone(),
                    kind,
                });
            }
        }
    }
    out
}

fn collect_async_method_names(classes: &[LibraryClass]) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for class in classes {
        for method in &class.methods {
            if method.is_async {
                out.insert(method.name.clone());
            }
        }
    }
    out
}

/// Returns true if `expr` (or any subexpression) contains a
/// `Send` whose method name is in `async_names`. Walks every
/// Expr-bearing variant of `ExprNode`. Lambda bodies are
/// traversed as part of the enclosing method.
fn body_calls_async(expr: &Expr, async_names: &HashSet<Symbol>) -> bool {
    walk_expr(expr, &mut |e| match &*e.node {
        ExprNode::Send { method, .. } => async_names.contains(method),
        _ => false,
    })
}

/// Returns true if `expr` (or any subexpression) contains a `Send`
/// whose method name passes the `is_async` predicate. Public
/// counterpart of `body_calls_async` — emit-side callers (e.g. the
/// TS lambda-emit and HOF-rewrite paths) consult their own
/// thread-local async set rather than rebuilding a `HashSet<Symbol>`
/// at every call. Lambda bodies are walked as part of the enclosing
/// expression, which is the right behavior for emit's per-Send
/// rewrite decisions: a HOF block that calls `(await ...)` inside a
/// nested lambda still needs the for-of rewrite at the outer HOF
/// site.
pub fn expr_contains_async_send(expr: &Expr, is_async: impl Fn(&str) -> bool) -> bool {
    walk_expr_with(expr, &mut |e| match &*e.node {
        ExprNode::Send { method, .. } => is_async(method.as_str()),
        _ => false,
    })
}

fn walk_expr_with<F: FnMut(&Expr) -> bool>(expr: &Expr, pred: &mut F) -> bool {
    walk_expr(expr, pred)
}

/// Pre-order walk over `expr`. Returns true as soon as `pred`
/// returns true on any visited node. Recurses into all Expr
/// children of every variant.
fn walk_expr<F: FnMut(&Expr) -> bool>(expr: &Expr, pred: &mut F) -> bool {
    if pred(expr) {
        return true;
    }
    match &*expr.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => false,
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .any(|(k, v)| walk_expr(k, pred) || walk_expr(v, pred)),
        ExprNode::Array { elements, .. } => elements.iter().any(|e| walk_expr(e, pred)),
        ExprNode::StringInterp { parts } => parts.iter().any(|p| match p {
            InterpPart::Text { .. } => false,
            InterpPart::Expr { expr } => walk_expr(expr, pred),
        }),
        ExprNode::BoolOp { left, right, .. } => walk_expr(left, pred) || walk_expr(right, pred),
        ExprNode::Let { value, body, .. } => walk_expr(value, pred) || walk_expr(body, pred),
        ExprNode::Lambda { body, .. } => walk_expr(body, pred),
        ExprNode::Apply { fun, args, block } => {
            walk_expr(fun, pred)
                || args.iter().any(|a| walk_expr(a, pred))
                || block.as_ref().map_or(false, |b| walk_expr(b, pred))
        }
        ExprNode::Send { recv, args, block, .. } => {
            recv.as_ref().map_or(false, |r| walk_expr(r, pred))
                || args.iter().any(|a| walk_expr(a, pred))
                || block.as_ref().map_or(false, |b| walk_expr(b, pred))
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_expr(cond, pred)
                || walk_expr(then_branch, pred)
                || walk_expr(else_branch, pred)
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_expr(scrutinee, pred)
                || arms.iter().any(|a| {
                    a.guard.as_ref().map_or(false, |g| walk_expr(g, pred))
                        || walk_expr(&a.body, pred)
                })
        }
        ExprNode::Seq { exprs } => exprs.iter().any(|e| walk_expr(e, pred)),
        ExprNode::Assign { target, value } => walk_lvalue(target, pred) || walk_expr(value, pred),
        ExprNode::Yield { args } => args.iter().any(|a| walk_expr(a, pred)),
        ExprNode::Raise { value } => walk_expr(value, pred),
        ExprNode::RescueModifier { expr, fallback } => {
            walk_expr(expr, pred) || walk_expr(fallback, pred)
        }
        ExprNode::Return { value } => walk_expr(value, pred),
        ExprNode::Super { args } => args
            .as_ref()
            .map_or(false, |xs| xs.iter().any(|a| walk_expr(a, pred))),
        ExprNode::Next { value } => value.as_ref().map_or(false, |v| walk_expr(v, pred)),
        ExprNode::MultiAssign { targets, value } => {
            targets.iter().any(|t| walk_lvalue(t, pred)) || walk_expr(value, pred)
        }
        ExprNode::While { cond, body, .. } => walk_expr(cond, pred) || walk_expr(body, pred),
        ExprNode::Range { begin, end, .. } => {
            begin.as_ref().map_or(false, |e| walk_expr(e, pred))
                || end.as_ref().map_or(false, |e| walk_expr(e, pred))
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk_expr(body, pred)
                || rescues.iter().any(|r| {
                    r.classes.iter().any(|c| walk_expr(c, pred))
                        || walk_expr(&r.body, pred)
                })
                || else_branch.as_ref().map_or(false, |e| walk_expr(e, pred))
                || ensure.as_ref().map_or(false, |e| walk_expr(e, pred))
        }
        ExprNode::Cast { value, .. } => walk_expr(value, pred),
    }
}

fn walk_lvalue<F: FnMut(&Expr) -> bool>(lv: &LValue, pred: &mut F) -> bool {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } => false,
        LValue::Attr { recv, .. } => walk_expr(recv, pred),
        LValue::Index { recv, index } => walk_expr(recv, pred) || walk_expr(index, pred),
    }
}

// Pattern is bound-name only — no Expr children — so it doesn't
// need a walker. Guards and arm bodies are Exprs, walked above.
#[allow(dead_code)]
fn _pattern_marker(_p: &Pattern) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{SqliteAdapter, SqliteAsyncAdapter};
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
    use crate::effect::EffectSet;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::{ClassId, Symbol};
    use crate::span::Span;

    fn synth_method(name: &str) -> MethodDef {
        MethodDef {
            name: Symbol::from(name),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
        }
    }

    fn synth_class(name: &str, methods: Vec<MethodDef>) -> LibraryClass {
        LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: false,
            parent: None,
            includes: Vec::new(),
            methods,
            origin: None,
        }
    }

    #[test]
    fn sync_adapter_marks_nothing() {
        // Gate 1 invariant: SqliteAdapter has no async seeds, so
        // running the seed pass under a sync profile mutates
        // nothing — emit output stays identical to pre-Phase-1.
        let mut classes = vec![synth_class(
            "SqliteAdapter",
            vec![
                synth_method("all"),
                synth_method("find"),
                synth_method("where"),
            ],
        )];
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAdapter);
        assert_eq!(count, 0);
        for m in &classes[0].methods {
            assert!(!m.is_async, "{} should not be async under sync adapter", m.name);
        }
    }

    #[test]
    fn async_adapter_marks_seed_methods() {
        let mut classes = vec![synth_class(
            "SqliteAdapter",
            vec![
                synth_method("all"),
                synth_method("find"),
                synth_method("where"),
                synth_method("count"),
                synth_method("exists?"),
                synth_method("insert"),
                synth_method("update"),
                synth_method("delete"),
                synth_method("not_a_seed"),
            ],
        )];
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        assert_eq!(count, 8);
        for m in &classes[0].methods {
            let expected = m.name.as_str() != "not_a_seed";
            assert_eq!(m.is_async, expected, "{}", m.name);
        }
    }

    #[test]
    fn restricts_to_named_class_only() {
        // Two classes both have a method named `find`. Only the
        // one matching `adapter_class` should be marked.
        let mut classes = vec![
            synth_class("SqliteAdapter", vec![synth_method("find")]),
            synth_class("Article", vec![synth_method("find")]),
        ];
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        assert_eq!(count, 1);
        assert!(classes[0].methods[0].is_async);
        assert!(!classes[1].methods[0].is_async);
    }

    #[test]
    fn missing_adapter_class_is_a_no_op() {
        // Passing an adapter_class that doesn't exist in the
        // class list shouldn't panic — it's a valid case during
        // partial transpilation (adapter not yet in IR).
        let mut classes = vec![synth_class(
            "Article",
            vec![synth_method("find")],
        )];
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        assert_eq!(count, 0);
        assert!(!classes[0].methods[0].is_async);
    }

    #[test]
    fn marking_is_idempotent() {
        // Running the seed pass twice on the same input is safe
        // — re-marking an already-async method is a no-op.
        let mut classes = vec![synth_class(
            "SqliteAdapter",
            vec![synth_method("all")],
        )];
        seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        assert_eq!(count, 1, "second run still 'marks' (idempotent reset)");
        assert!(classes[0].methods[0].is_async);
    }

    fn synth_send(method: &str, recv: Option<Expr>, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        )
    }

    fn synth_method_with_body(name: &str, body: Expr) -> MethodDef {
        let mut m = synth_method(name);
        m.body = body;
        m
    }

    #[test]
    fn propagate_marks_direct_caller() {
        // Class A has async method `seed`. Class B has method
        // `caller` whose body sends `seed`. Propagation marks
        // B#caller async.
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class(
                "B",
                vec![synth_method_with_body(
                    "caller",
                    synth_send("seed", None, vec![]),
                )],
            ),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.newly_marked, 1);
        assert!(classes[1].methods[0].is_async);
        // Convergence should take 2 iterations: 1 to mark, 1 to
        // detect no further changes.
        assert_eq!(result.iterations, 2);
    }

    #[test]
    fn propagate_is_transitive() {
        // A#seed (async) ← B#mid ← C#outer. Both B#mid and C#outer
        // should end up async after propagation.
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class(
                "B",
                vec![synth_method_with_body(
                    "mid",
                    synth_send("seed", None, vec![]),
                )],
            ),
            synth_class(
                "C",
                vec![synth_method_with_body(
                    "outer",
                    synth_send("mid", None, vec![]),
                )],
            ),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.newly_marked, 2);
        assert!(classes[1].methods[0].is_async);
        assert!(classes[2].methods[0].is_async);
    }

    #[test]
    fn propagate_is_idempotent_when_nothing_to_mark() {
        // No async methods anywhere — propagate is a no-op,
        // should return after one iteration with zero marks.
        let mut classes = vec![
            synth_class("A", vec![synth_method("foo")]),
            synth_class(
                "B",
                vec![synth_method_with_body(
                    "bar",
                    synth_send("foo", None, vec![]),
                )],
            ),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.newly_marked, 0);
        assert_eq!(result.iterations, 1);
    }

    #[test]
    fn block_bodies_color_enclosing_method() {
        // method `outer` calls `xs.each { |x| x.seed }` — the
        // enclosing method becomes async because its body
        // (transitively, through the block) calls `seed`. The
        // block lambda itself isn't a MethodDef, so there's no
        // separate flag to flip — the async-ness lives on
        // `outer`.
        let block_body = synth_send(
            "seed",
            Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var {
                    id: crate::ident::VarId(0),
                    name: Symbol::from("x"),
                },
            )),
            vec![],
        );
        let block = Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                params: vec![Symbol::from("x")],
                block_param: None,
                body: block_body,
                block_style: crate::expr::BlockStyle::Brace,
            },
        );
        let xs_each = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: crate::ident::VarId(0),
                        name: Symbol::from("xs"),
                    },
                )),
                method: Symbol::from("each"),
                args: vec![],
                block: Some(block),
                parenthesized: false,
            },
        );
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class("B", vec![synth_method_with_body("outer", xs_each)]),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.newly_marked, 1);
        assert!(classes[1].methods[0].is_async);
    }

    #[test]
    fn propagate_walks_into_if_branches() {
        // `if cond; seed; else; nil; end` — async call in the
        // then branch colors the enclosing method.
        let body = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Bool { value: true } },
                ),
                then_branch: synth_send("seed", None, vec![]),
                else_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Nil },
                ),
            },
        );
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class("B", vec![synth_method_with_body("outer", body)]),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.newly_marked, 1);
        assert!(classes[1].methods[0].is_async);
    }

    #[test]
    fn sync_slot_violation_on_attribute_reader() {
        // An attr_reader whose body somehow calls an async
        // method (e.g. a synthesized accessor that delegates)
        // should be flagged as a violation — TS can't have an
        // async getter.
        let mut reader = synth_method("title");
        reader.kind = AccessorKind::AttributeReader;
        reader.body = synth_send("seed", None, vec![]);
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class("Article", vec![reader]),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.sync_slot_violations.len(), 1);
        assert_eq!(result.sync_slot_violations[0].kind, SyncSlotKind::AttributeReader);
        assert_eq!(result.sync_slot_violations[0].method.as_str(), "title");
    }

    #[test]
    fn sync_slot_violation_on_attribute_writer() {
        let mut writer = synth_method("title=");
        writer.kind = AccessorKind::AttributeWriter;
        writer.body = synth_send("seed", None, vec![]);
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class("Article", vec![writer]),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.sync_slot_violations.len(), 1);
        assert_eq!(result.sync_slot_violations[0].kind, SyncSlotKind::AttributeWriter);
    }

    #[test]
    fn sync_slot_violation_on_constructor() {
        // `def initialize` doing async work — TS `constructor()`
        // can't be async.
        let init = synth_method_with_body("initialize", synth_send("seed", None, vec![]));
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class("Article", vec![init]),
        ];
        let result = propagate(&mut classes);
        assert_eq!(result.sync_slot_violations.len(), 1);
        assert_eq!(result.sync_slot_violations[0].kind, SyncSlotKind::Constructor);
    }

    #[test]
    fn no_sync_slot_violations_when_method_is_plain() {
        // A regular `Method`-kind def becoming async is the
        // intended outcome — no violation.
        let mut classes = vec![
            synth_class(
                "A",
                vec![{
                    let mut m = synth_method("seed");
                    m.is_async = true;
                    m
                }],
            ),
            synth_class(
                "B",
                vec![synth_method_with_body("plain", synth_send("seed", None, vec![]))],
            ),
        ];
        let result = propagate(&mut classes);
        assert!(result.sync_slot_violations.is_empty());
        assert!(classes[1].methods[0].is_async);
    }

    fn synth_function(name: &str, body: Expr) -> crate::dialect::LibraryFunction {
        crate::dialect::LibraryFunction {
            module_path: vec![],
            name: Symbol::from(name),
            params: vec![],
            body,
            signature: None,
            effects: crate::effect::EffectSet::default(),
            is_async: false,
        }
    }

    #[test]
    fn propagate_global_marks_libraryfunctions_calling_externs() {
        // A free function whose body sends an extern-async name
        // should pick up `is_async = true`.
        let mut funcs = vec![synth_function(
            "page_count",
            synth_send("count", None, vec![]),
        )];
        let result = propagate_global_with_externs(&mut [], &mut funcs, &["count"]);
        assert!(funcs[0].is_async, "function should be marked async");
        assert_eq!(result.newly_marked, 1);
    }

    #[test]
    fn propagate_global_propagates_function_to_class_caller() {
        // A class method calls a LibraryFunction; the function
        // calls extern → both get marked, in one global pass.
        let mut classes = vec![synth_class(
            "C",
            vec![synth_method_with_body(
                "use_helper",
                synth_send("helper", None, vec![]),
            )],
        )];
        let mut funcs = vec![synth_function(
            "helper",
            synth_send("count", None, vec![]),
        )];
        let result = propagate_global_with_externs(&mut classes, &mut funcs, &["count"]);
        assert!(funcs[0].is_async, "helper function should be async");
        assert!(classes[0].methods[0].is_async, "C#use_helper should be async");
        assert_eq!(result.newly_marked, 2);
    }

    #[test]
    fn propagate_global_propagates_class_to_function_caller() {
        // The reverse direction: a function calls a class method;
        // the class method calls extern → both marked.
        let mut classes = vec![synth_class(
            "C",
            vec![synth_method_with_body(
                "find",
                synth_send("count", None, vec![]),
            )],
        )];
        let mut funcs = vec![synth_function(
            "wrapper",
            synth_send("find", None, vec![]),
        )];
        let result = propagate_global_with_externs(&mut classes, &mut funcs, &["count"]);
        assert!(classes[0].methods[0].is_async, "C#find should be async");
        assert!(funcs[0].is_async, "wrapper function should be async");
        assert_eq!(result.newly_marked, 2);
    }

    #[test]
    fn propagate_global_cross_vec_chain_converges() {
        // Three classes A/B/C; A method calls B method calls C
        // method which calls extern. All three should be marked
        // after one fixed-point run.
        let mut classes = vec![
            synth_class(
                "A",
                vec![synth_method_with_body(
                    "outer",
                    synth_send("middle", None, vec![]),
                )],
            ),
            synth_class(
                "B",
                vec![synth_method_with_body(
                    "middle",
                    synth_send("inner", None, vec![]),
                )],
            ),
            synth_class(
                "C",
                vec![synth_method_with_body(
                    "inner",
                    synth_send("count", None, vec![]),
                )],
            ),
        ];
        let result = propagate_global_with_externs(&mut classes, &mut [], &["count"]);
        for c in &classes {
            assert!(
                c.methods[0].is_async,
                "{}#{} should be async",
                c.name.0,
                c.methods[0].name
            );
        }
        assert_eq!(result.newly_marked, 3);
        // Convergence: 3 marked + 1 stable iteration = 4 total
        // iterations is the worst case, but for a linear chain
        // a single pass marks everything via name-set growth.
        assert!(result.iterations <= 4);
    }

    #[test]
    fn propagate_with_externs_marks_callers_of_extern_names() {
        // No async MethodDef in the IR — the adapter is hand-written
        // TS, not transpiled. Method `Base#all` calls `adapter.all`;
        // because `all` is in the extern set, the caller `Base#all`
        // is colored async even though no class in the IR has a
        // method named `all` with `is_async = true`.
        let mut classes = vec![synth_class(
            "Base",
            vec![synth_method_with_body(
                "all",
                synth_send(
                    "all",
                    Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Var {
                            id: crate::ident::VarId(0),
                            name: Symbol::from("adapter"),
                        },
                    )),
                    vec![],
                ),
            )],
        )];
        let result = propagate_with_externs(&mut classes, &["all", "find"]);
        assert!(classes[0].methods[0].is_async);
        assert_eq!(result.newly_marked, 1);
    }

    #[test]
    fn propagate_with_externs_empty_is_equivalent_to_propagate() {
        // Empty extern set behaves identically to `propagate`. Used
        // by the `node-sync` profile to keep emit equivalent to
        // pre-Phase-3 output.
        let mut classes_a = vec![synth_class(
            "A",
            vec![synth_method_with_body(
                "x",
                synth_send("y", None, vec![]),
            )],
        )];
        let mut classes_b = classes_a.clone();
        let result_a = propagate(&mut classes_a);
        let result_b = propagate_with_externs(&mut classes_b, &[]);
        assert_eq!(result_a.newly_marked, result_b.newly_marked);
        assert_eq!(classes_a, classes_b);
    }

    #[test]
    fn end_to_end_seed_then_propagate() {
        // Realistic shape: seed `SqliteAdapter#all` via the
        // async adapter, then propagate. A user method that
        // calls `all` (e.g. `Article.all` on the AR base, or a
        // controller action) becomes async without further
        // intervention.
        let mut classes = vec![
            synth_class(
                "SqliteAdapter",
                vec![synth_method("all"), synth_method("find")],
            ),
            synth_class(
                "Base",
                vec![
                    synth_method_with_body(
                        "all",
                        synth_send(
                            "all",
                            Some(Expr::new(
                                Span::synthetic(),
                                ExprNode::Var {
                                    id: crate::ident::VarId(0),
                                    name: Symbol::from("adapter"),
                                },
                            )),
                            vec![],
                        ),
                    ),
                ],
            ),
        ];
        let seeded = seed_from_adapter(&mut classes, "SqliteAdapter", &SqliteAsyncAdapter);
        assert_eq!(seeded, 2);
        let result = propagate(&mut classes);
        // Base#all should be async after propagation (it calls
        // adapter.all, which is now async-seeded).
        let base_all = &classes[1].methods[0];
        assert!(base_all.is_async, "Base#all should be async after propagation");
        assert_eq!(result.newly_marked, 1);
    }

    #[test]
    fn empty_seed_list_marks_nothing() {
        // The Gate 1 path expressed as a property: any adapter
        // whose seed list is empty produces zero marks, even on
        // a class whose methods would otherwise match.
        struct NoSeedAdapter;
        impl DatabaseAdapter for NoSeedAdapter {
            fn classify_ar_method(&self, _m: &str) -> crate::adapter::ArMethodKind {
                crate::adapter::ArMethodKind::Unknown
            }
        }
        let mut classes = vec![synth_class(
            "SqliteAdapter",
            vec![synth_method("all"), synth_method("find")],
        )];
        let count = seed_from_adapter(&mut classes, "SqliteAdapter", &NoSeedAdapter);
        assert_eq!(count, 0);
    }
}
