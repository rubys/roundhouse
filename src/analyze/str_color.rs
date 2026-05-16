//! String-ownership coloring — rust2-specific Phase 1 (inference pass).
//!
//! Annotates each `Ty::Str` expression with `Expr.str_coercion` so the
//! rust2 emitter can produce the right ownership shape at every site
//! (`&` for borrows, `.to_string()` for owned-from-borrow). Mirrors
//! `async_color`'s structure: a registry of callable signatures + an
//! IR-walk + per-node annotation. The analogy "async call from sync
//! caller ⇒ mark caller async" maps to "producer color ≠ consumer
//! color ⇒ insert coercion."
//!
//! Phase 1 = produced but not yet consumed: the pass annotates each
//! `Expr.str_coercion`; the rust2 emitter still relies on its
//! existing per-site peepholes (the `IVAR_TYPES`/`PARAM_TYPES`
//! thread-locals in `src/emit/rust2/expr.rs`). Phase 2 flips the
//! emitter to read `e.str_coercion` and drop the peepholes.
//!
//! Unlike async (which is universal across TS/Rust/Python), ownership
//! coloring is rust-only — TS/Crystal/Python don't distinguish
//! borrowed vs owned strings. The annotation lives on the shared
//! `Expr` (rather than a side-table) because synthetic-Span collisions
//! rule out Span-keyed storage; cost is one `Option<StrCoercion>` per
//! `Expr`.

use std::collections::HashMap;

use crate::dialect::{LibraryClass, LibraryFunction, MethodDef};
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal, StrCoercion};
use crate::ident::Symbol;
use crate::ty::{ParamKind, Ty};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Where a string lives at runtime once emitted by rust2.
///
/// Three-valued because `&'static str` is borrow-compatible everywhere
/// `&str` is, but additionally has no lifetime constraint — useful for
/// future const-promotion peepholes. Today the pass treats `Static`
/// and `Borrowed` as interchangeable consumers; the distinction is
/// kept for producer-side bookkeeping only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrColor {
    /// `&'static str` — produced by string literals.
    Static,
    /// `&str` — produced by reads of `&str`-typed parameters.
    Borrowed,
    /// `String` — produced by ivar reads of `Ty::Str` fields, by
    /// `format!()`-shaped string interpolations, and by calls
    /// returning `Ty::Str` (rust2 emits `-> String`).
    Owned,
}

/// Compute the coercion needed when a producer of `from` lands in a
/// position expecting `to`. `None` means no coercion (colors agree, or
/// producer subsumes consumer).
pub fn coercion_for(from: StrColor, to: StrColor) -> Option<StrCoercion> {
    match (from, to) {
        (StrColor::Static | StrColor::Borrowed, StrColor::Owned) => Some(StrCoercion::ToOwned),
        (StrColor::Owned, StrColor::Borrowed) => Some(StrCoercion::Borrow),
        // Same-family (Owned→Owned, &str→&str), or producer is
        // `Static` filling a `Borrowed` slot — no coercion needed.
        _ => None,
    }
}

/// Per-callable signature: string-position colors keyed by param index
/// plus the return color. `None` slots mean "this position isn't
/// `Ty::Str`" so the inference pass should leave the corresponding
/// argument expression alone.
#[derive(Clone, Debug, Default)]
pub struct CallableSig {
    pub param_str_colors: Vec<Option<StrColor>>,
    pub return_str_color: Option<StrColor>,
}

/// Maps callable names to their rust2 string-position colors.
///
/// Lookup is by **method name only**, matching `async_color`'s
/// over-approximation. A same-named method on a different class with
/// a different sig results in extra/missing coercions at the call
/// site, not incorrect emit — the Rust compiler still rejects an
/// actual type mismatch at build time, so the worst case is a build
/// failure that points at the right site, not silent miscompilation.
/// A future refinement can plug in the typer's callee→def lookup for
/// receiver-aware resolution.
#[derive(Default, Debug)]
pub struct CallableRegistry {
    methods: HashMap<Symbol, CallableSig>,
    functions: HashMap<Symbol, CallableSig>,
}

impl CallableRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sig_for_method(&self, name: &Symbol) -> Option<&CallableSig> {
        self.methods.get(name)
    }

    pub fn sig_for_function(&self, name: &Symbol) -> Option<&CallableSig> {
        self.functions.get(name)
    }
}

// ---------------------------------------------------------------------------
// Registry construction
// ---------------------------------------------------------------------------

/// Build a registry from the lowered IR (LibraryClass methods +
/// LibraryFunctions). Uses rust2's emit conventions:
/// - `Ty::Str` parameter → `Borrowed` (rust2 emits `&str`)
/// - `Ty::Str` return → `Owned` (rust2 emits `String`)
///
/// After IR-derived sigs are populated, augment with the small
/// hand-written runtime table (`runtime/rust/*.rs` modules whose
/// signatures live in Rust source, not in any LibraryClass we'd
/// otherwise walk).
pub fn build_registry(
    classes: &[LibraryClass],
    functions: &[LibraryFunction],
) -> CallableRegistry {
    let mut reg = CallableRegistry::new();
    for class in classes {
        for method in &class.methods {
            let sig = signature_from_method(method);
            reg.methods.entry(method.name.clone()).or_insert(sig);
        }
    }
    for func in functions {
        let sig = signature_from_function(func);
        reg.functions.entry(func.name.clone()).or_insert(sig);
    }
    register_hand_written_runtime(&mut reg);
    reg
}

fn signature_from_method(method: &MethodDef) -> CallableSig {
    from_sig(method.signature.as_ref(), method.params.len())
}

fn signature_from_function(func: &LibraryFunction) -> CallableSig {
    from_sig(func.signature.as_ref(), func.params.len())
}

fn from_sig(sig: Option<&Ty>, param_count: usize) -> CallableSig {
    let Some(Ty::Fn { params, ret, .. }) = sig else {
        // Untyped (no signature) — leave every position blank. The
        // pass treats `None` as "don't coerce here", which is the
        // conservative choice when we don't know the param types.
        return CallableSig {
            param_str_colors: vec![None; param_count],
            return_str_color: None,
        };
    };
    // Skip block params + keyword-rest in the color vector — those
    // don't show up as positional args in Send call sites.
    let positional: Vec<&crate::ty::Param> = params
        .iter()
        .filter(|p| !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest))
        .collect();
    let param_str_colors = (0..param_count)
        .map(|i| positional.get(i).and_then(|p| color_for_param_ty(&p.ty)))
        .collect();
    let return_str_color = color_for_return_ty(ret);
    CallableSig { param_str_colors, return_str_color }
}

/// A `Ty::Str` parameter emits as `&str` in rust2; everything else
/// either isn't a string position (None) or doesn't fit this pass's
/// model yet (Option<String>, Vec<String>, etc — left for later).
fn color_for_param_ty(ty: &Ty) -> Option<StrColor> {
    match ty {
        Ty::Str | Ty::Sym => Some(StrColor::Borrowed),
        _ => None,
    }
}

/// A `Ty::Str` return emits as `String` in rust2.
fn color_for_return_ty(ty: &Ty) -> Option<StrColor> {
    match ty {
        Ty::Str | Ty::Sym => Some(StrColor::Owned),
        _ => None,
    }
}

/// Hand-written `runtime/rust/*.rs` modules export functions whose
/// signatures don't live in any LibraryClass we'd walk. Declare them
/// here so call sites in transpiled code coerce correctly when calling
/// into the primitive runtime.
///
/// Kept intentionally tiny for Phase 1 — extend per concrete miscompile
/// observed once Phase 2 wires emit to consume the annotations. Don't
/// pre-populate hypothetical entries; the registry will grow against
/// real demand.
fn register_hand_written_runtime(_reg: &mut CallableRegistry) {
    // Intentionally empty for Phase 1. Anticipated entries (`html_escape`,
    // `AdapterInterface::*`, `Flash::*`, `Session::*`) land in Phase 2
    // when the emit-side switchover surfaces concrete miscompiles that
    // need them — adding them speculatively now risks the registry
    // drifting from what the runtime actually exposes.
}

// ---------------------------------------------------------------------------
// The pass — walk a method body, annotate Ty::Str expressions
// ---------------------------------------------------------------------------

/// Walk every method on every class, annotating string-typed
/// expressions with the right `StrCoercion` for their context. Run
/// once after lowering, before per-target emit.
pub fn color_classes(classes: &mut [LibraryClass], registry: &CallableRegistry) -> usize {
    let mut annotated = 0;
    for class in classes.iter_mut() {
        for method in class.methods.iter_mut() {
            annotated += color_method(method, registry);
        }
    }
    annotated
}

/// Same as `color_classes` but for a free-function (LibraryFunction)
/// slice.
pub fn color_functions(
    functions: &mut [LibraryFunction],
    registry: &CallableRegistry,
) -> usize {
    let mut annotated = 0;
    for func in functions.iter_mut() {
        let return_color = registry
            .sig_for_function(&func.name)
            .and_then(|s| s.return_str_color);
        let mut ctx = WalkCtx { registry, return_color };
        annotated += walk(&mut func.body, ParentExpect::None, &mut ctx);
    }
    annotated
}

/// Annotate a single method body. Returns the number of expressions
/// that received a coercion (0 if every site already agreed).
///
/// The body itself is in tail / value position — its result is the
/// method's return value, so we seed the walk with the return color
/// as the parent expectation. Tail-position propagation through
/// Seq / If / Case / BeginRescue arms picks up implicit-tail string
/// literals (`def name; "x"; end` shape) the same as an explicit
/// `Return { value: lit }`.
pub fn color_method(method: &mut MethodDef, registry: &CallableRegistry) -> usize {
    let return_color = registry
        .sig_for_method(&method.name)
        .and_then(|s| s.return_str_color);
    let mut ctx = WalkCtx { registry, return_color };
    let expect = return_color.map_or(ParentExpect::None, ParentExpect::Color);
    walk(&mut method.body, expect, &mut ctx)
}

/// Per-walk context — what the surrounding code expects at the
/// current Ty::Str position. Threaded as a function argument
/// (`ParentExpect`) rather than mutable state so branching cases
/// (If, BoolOp, Case) can propagate the same expectation into
/// each arm.
struct WalkCtx<'a> {
    registry: &'a CallableRegistry,
    return_color: Option<StrColor>,
}

/// What the parent of the current expression expects from a string
/// child. `None` = no constraint (the expression isn't in a
/// string-position slot, or the parent doesn't care about ownership).
#[derive(Clone, Copy, Debug)]
enum ParentExpect {
    None,
    Color(StrColor),
}

fn walk(e: &mut Expr, expect: ParentExpect, ctx: &mut WalkCtx<'_>) -> usize {
    let mut count = 0;
    // Annotate THIS node first (if it's string-typed and there's an
    // expectation that differs from what we'll produce), then recurse
    // with appropriate per-child expectations.
    if is_str_ty(e.ty.as_ref()) {
        if let (Some(producer), ParentExpect::Color(consumer)) = (producer_color(e), expect) {
            if let Some(c) = coercion_for(producer, consumer) {
                e.str_coercion = Some(c);
                count += 1;
            }
        }
    }
    // Recurse, passing the right per-position expectation to children.
    // `expect` is forwarded to tail/value-position children (Seq tail,
    // If both branches, Case arm bodies, BoolOp both sides,
    // BeginRescue body + rescue bodies) so an implicit-tail string
    // literal inside a method body inherits the function's return
    // color the same way an explicit `return` value does.
    count += walk_children(e, expect, ctx);
    count
}

/// Compute what color the EMIT of this expression will produce, before
/// any coercion is applied. Returns `None` for non-string positions or
/// shapes Phase 1 doesn't model yet (e.g. unification across branches).
fn producer_color(e: &Expr) -> Option<StrColor> {
    match e.node.as_ref() {
        ExprNode::Lit { value: Literal::Str { .. } } => Some(StrColor::Static),
        ExprNode::StringInterp { .. } => Some(StrColor::Owned),
        ExprNode::Ivar { .. } => Some(StrColor::Owned),
        ExprNode::Send { .. } => Some(StrColor::Owned),
        ExprNode::Var { .. } => {
            // Conservative for Phase 1: assume `Borrowed` (param-like).
            // A local `let x = ivar` where `x` is then used wants
            // `Owned` — Phase 2 can refine via a local symbol table
            // threaded through `WalkCtx`.
            Some(StrColor::Borrowed)
        }
        // Branching / sequencing shapes — Phase 1 leaves these blank.
        // A future pass can unify by walking both arms and emitting a
        // coercion on each side if they disagree.
        _ => None,
    }
}

/// Recurse into children with the right per-position expectation.
/// `tail_expect` is the expectation inherited from the parent — it
/// flows through to tail/value-position children (Seq tail, If/Case
/// arms, BoolOp sides, BeginRescue body/rescue bodies). Children in
/// non-value positions (Send args, While condition, Range bounds)
/// get their own per-position expectation (callee param color for
/// args; None elsewhere).
fn walk_children(e: &mut Expr, tail_expect: ParentExpect, ctx: &mut WalkCtx<'_>) -> usize {
    let mut count = 0;
    match e.node.as_mut() {
        ExprNode::Send { recv, args, block, method, .. } => {
            // Receiver: no string constraint we model today.
            if let Some(r) = recv {
                count += walk(r, ParentExpect::None, ctx);
            }
            // Args: look up the callee's per-param color and apply.
            let sig = ctx.registry.sig_for_method(method);
            for (i, arg) in args.iter_mut().enumerate() {
                let expect = sig
                    .and_then(|s| s.param_str_colors.get(i).copied().flatten())
                    .map_or(ParentExpect::None, ParentExpect::Color);
                count += walk(arg, expect, ctx);
            }
            if let Some(b) = block {
                count += walk(b, ParentExpect::None, ctx);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            count += walk(fun, ParentExpect::None, ctx);
            for arg in args.iter_mut() {
                count += walk(arg, ParentExpect::None, ctx);
            }
            if let Some(b) = block {
                count += walk(b, ParentExpect::None, ctx);
            }
        }
        ExprNode::Assign { target, value } => {
            // Ivar assignment: field is `String`, so RHS must be Owned.
            // Other LValues don't impose a string constraint today.
            let expect = match target {
                LValue::Ivar { .. } if is_str_ty(value.ty.as_ref()) => {
                    ParentExpect::Color(StrColor::Owned)
                }
                _ => ParentExpect::None,
            };
            count += walk(value, expect, ctx);
        }
        ExprNode::Return { value } => {
            // Return value must match the function's return color.
            let expect = ctx
                .return_color
                .map_or(ParentExpect::None, ParentExpect::Color);
            count += walk(value, expect, ctx);
        }
        ExprNode::Let { value, body, .. } => {
            // Let RHS has no constraint — the binding takes whatever
            // color the RHS produced. Body inherits the surrounding
            // expectation; for Phase 1 we don't propagate, leaving the
            // body's own expression-level annotations to handle it.
            count += walk(value, ParentExpect::None, ctx);
            count += walk(body, ParentExpect::None, ctx);
        }
        ExprNode::Seq { exprs } => {
            // The tail expression carries the surrounding expectation
            // (it's the value of the Seq); earlier expressions are
            // statement-position and have no string-color constraint.
            if let Some((last, rest)) = exprs.split_last_mut() {
                for sub in rest.iter_mut() {
                    count += walk(sub, ParentExpect::None, ctx);
                }
                count += walk(last, tail_expect, ctx);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // Cond is a boolean position. Both branches are
            // value-positions inheriting the surrounding expectation
            // (Rust requires `if` arms to agree on type, so applying
            // the same expectation to both is the right call).
            count += walk(cond, ParentExpect::None, ctx);
            count += walk(then_branch, tail_expect, ctx);
            count += walk(else_branch, tail_expect, ctx);
        }
        ExprNode::BoolOp { left, right, .. } => {
            // `a || b` evaluates to whichever side short-circuits;
            // both produce the value, so both inherit the expectation.
            count += walk(left, tail_expect, ctx);
            count += walk(right, tail_expect, ctx);
        }
        ExprNode::StringInterp { parts } => {
            for part in parts.iter_mut() {
                if let InterpPart::Expr { expr } = part {
                    count += walk(expr, ParentExpect::None, ctx);
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries.iter_mut() {
                count += walk(k, ParentExpect::None, ctx);
                count += walk(v, ParentExpect::None, ctx);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements.iter_mut() {
                count += walk(el, ParentExpect::None, ctx);
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            count += walk(scrutinee, ParentExpect::None, ctx);
            for arm in arms.iter_mut() {
                // Arm body is value-position; arm guard is boolean.
                count += walk(&mut arm.body, tail_expect, ctx);
                if let Some(g) = arm.guard.as_mut() {
                    count += walk(g, ParentExpect::None, ctx);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            count += walk(cond, ParentExpect::None, ctx);
            count += walk(body, ParentExpect::None, ctx);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                count += walk(b, ParentExpect::None, ctx);
            }
            if let Some(e) = end {
                count += walk(e, ParentExpect::None, ctx);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            // The block's value is whichever of body / rescue body /
            // else branch executes — all are value-position. `ensure`
            // runs for side effects, its value is discarded.
            count += walk(body, tail_expect, ctx);
            for rc in rescues.iter_mut() {
                count += walk(&mut rc.body, tail_expect, ctx);
            }
            if let Some(eb) = else_branch {
                count += walk(eb, tail_expect, ctx);
            }
            if let Some(en) = ensure {
                count += walk(en, ParentExpect::None, ctx);
            }
        }
        ExprNode::Lambda { body, .. } => {
            // Lambda body's color is the lambda's RESULT, not the
            // surrounding expression's value — block-return color
            // would need its own lookup. Phase 1 doesn't model it.
            count += walk(body, ParentExpect::None, ctx);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            // `expr rescue fallback` — both sides are value-position
            // and inherit the surrounding expectation.
            count += walk(expr, tail_expect, ctx);
            count += walk(fallback, tail_expect, ctx);
        }
        ExprNode::Yield { args } | ExprNode::Super { args: Some(args) } => {
            for arg in args.iter_mut() {
                count += walk(arg, ParentExpect::None, ctx);
            }
        }
        ExprNode::Raise { value } => {
            count += walk(value, ParentExpect::None, ctx);
        }
        ExprNode::MultiAssign { value, .. } => {
            count += walk(value, ParentExpect::None, ctx);
        }
        ExprNode::Cast { value, .. } => {
            count += walk(value, ParentExpect::None, ctx);
        }
        ExprNode::Next { value: Some(v) } => {
            count += walk(v, ParentExpect::None, ctx);
        }
        // Leaves and shapes that carry no string-typed children we
        // model today.
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef
        | ExprNode::Super { args: None }
        | ExprNode::Next { value: None } => {}
    }
    count
}

fn is_str_ty(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Str) | Some(Ty::Sym))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{
        AccessorKind, LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver, Param,
    };
    use crate::effect::EffectSet;
    use crate::expr::{Expr, ExprNode, Literal, LValue};
    use crate::ident::{ClassId, Symbol, VarId};
    use crate::span::Span;
    use crate::ty::{Param as TyParam, ParamKind, Ty};

    // ----- Construction helpers -------------------------------------------

    fn lit_str(s: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: s.to_string() } },
        );
        e.ty = Some(Ty::Str);
        e
    }

    fn var(name: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        );
        e.ty = Some(Ty::Str);
        e
    }

    fn ivar(name: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from(name) },
        );
        e.ty = Some(Ty::Str);
        e
    }

    fn send(method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        )
    }

    fn fn_sig(params: Vec<(&str, Ty)>, ret: Ty) -> Ty {
        Ty::Fn {
            params: params
                .into_iter()
                .map(|(n, ty)| TyParam {
                    name: Symbol::from(n),
                    ty,
                    kind: ParamKind::Required,
                })
                .collect(),
            block: None,
            ret: Box::new(ret),
            effects: EffectSet::pure(),
        }
    }

    fn class(name: &str, methods: Vec<MethodDef>) -> LibraryClass {
        LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: false,
            parent: None,
            includes: vec![],
            methods,
            origin: None,
        }
    }

    fn method(name: &str, params: Vec<&str>, signature: Ty, body: Expr) -> MethodDef {
        MethodDef {
            name: Symbol::from(name),
            receiver: MethodReceiver::Instance,
            params: params.iter().map(|p| Param::positional(Symbol::from(*p))).collect(),
            body,
            signature: Some(signature),
            effects: EffectSet::default(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
        }
    }

    // Convenience that suppresses the `dead_code` warning on
    // `LibraryClassOrigin` when the tests don't exercise it; touching
    // the enum keeps it in the linker graph for the test binary.
    #[allow(dead_code)]
    const _: Option<LibraryClassOrigin> = None;

    // ----- Coercion-table tests -------------------------------------------

    #[test]
    fn coercion_for_static_to_owned_is_to_owned() {
        assert_eq!(
            coercion_for(StrColor::Static, StrColor::Owned),
            Some(StrCoercion::ToOwned),
        );
    }

    #[test]
    fn coercion_for_borrowed_to_owned_is_to_owned() {
        assert_eq!(
            coercion_for(StrColor::Borrowed, StrColor::Owned),
            Some(StrCoercion::ToOwned),
        );
    }

    #[test]
    fn coercion_for_owned_to_borrowed_is_borrow() {
        assert_eq!(
            coercion_for(StrColor::Owned, StrColor::Borrowed),
            Some(StrCoercion::Borrow),
        );
    }

    #[test]
    fn coercion_for_same_color_is_none() {
        assert_eq!(coercion_for(StrColor::Owned, StrColor::Owned), None);
        assert_eq!(coercion_for(StrColor::Borrowed, StrColor::Borrowed), None);
        // Static into a Borrowed slot is a no-op — `&'static str` is
        // already borrow-compatible.
        assert_eq!(coercion_for(StrColor::Static, StrColor::Borrowed), None);
    }

    // ----- Registry construction -----------------------------------------

    #[test]
    fn registry_picks_up_str_param_as_borrowed() {
        let m = method(
            "greet",
            vec!["who"],
            fn_sig(vec![("who", Ty::Str)], Ty::Nil),
            // body irrelevant for sig-only test
            lit_str("hi"),
        );
        let c = class("X", vec![m]);
        let reg = build_registry(&[c], &[]);
        let sig = reg.sig_for_method(&Symbol::from("greet")).expect("sig");
        assert_eq!(sig.param_str_colors, vec![Some(StrColor::Borrowed)]);
        assert_eq!(sig.return_str_color, None);
    }

    #[test]
    fn registry_picks_up_str_return_as_owned() {
        let m = method(
            "name",
            vec![],
            fn_sig(vec![], Ty::Str),
            lit_str("a"),
        );
        let c = class("X", vec![m]);
        let reg = build_registry(&[c], &[]);
        let sig = reg.sig_for_method(&Symbol::from("name")).expect("sig");
        assert_eq!(sig.return_str_color, Some(StrColor::Owned));
    }

    // ----- The pass: coercions land at the right sites --------------------

    /// Literal returned from a `-> String` function: producer is Static,
    /// consumer is Owned, so the Return value's expr should be flagged
    /// with `ToOwned`.
    #[test]
    fn literal_returned_from_string_fn_gets_to_owned() {
        let return_node = Expr::new(
            Span::synthetic(),
            ExprNode::Return { value: lit_str("hello") },
        );
        let mut m = method(
            "render",
            vec![],
            fn_sig(vec![], Ty::Str),
            return_node,
        );
        let reg = build_registry(&[class("X", vec![m.clone()])], &[]);
        color_method(&mut m, &reg);

        // Drill into Return.value — should carry ToOwned.
        let ExprNode::Return { value } = m.body.node.as_ref() else {
            panic!("expected Return at top of body");
        };
        assert_eq!(value.str_coercion, Some(StrCoercion::ToOwned));
    }

    /// Ivar assigned a string literal: producer is Static, field is
    /// String → ToOwned.
    #[test]
    fn ivar_assign_string_literal_gets_to_owned() {
        let body = Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from("name") },
                value: lit_str("dave"),
            },
        );
        let mut m = method(
            "set",
            vec![],
            fn_sig(vec![], Ty::Nil),
            body,
        );
        let reg = build_registry(&[class("X", vec![m.clone()])], &[]);
        color_method(&mut m, &reg);

        let ExprNode::Assign { value, .. } = m.body.node.as_ref() else {
            panic!("expected Assign at top of body");
        };
        assert_eq!(value.str_coercion, Some(StrCoercion::ToOwned));
    }

    /// Calling a function whose param is `&str`, passing an ivar of
    /// Ty::Str (which emits as a `String` field read): producer Owned,
    /// consumer Borrowed → Borrow.
    #[test]
    fn ivar_passed_to_borrow_param_gets_borrow() {
        // Callee: def greet(who); end  with sig (Str) -> Nil
        let callee = method(
            "greet",
            vec!["who"],
            fn_sig(vec![("who", Ty::Str)], Ty::Nil),
            lit_str("ignored"),
        );
        // Caller: def call; greet(@name); end
        let body = send("greet", vec![ivar("name")]);
        let mut caller = method(
            "call",
            vec![],
            fn_sig(vec![], Ty::Nil),
            body,
        );
        let reg = build_registry(&[class("X", vec![callee, caller.clone()])], &[]);
        color_method(&mut caller, &reg);

        let ExprNode::Send { args, .. } = caller.body.node.as_ref() else {
            panic!("expected Send at top of body");
        };
        assert_eq!(args[0].str_coercion, Some(StrCoercion::Borrow));
    }

    /// Calling a function whose param is `&str`, passing a literal
    /// (`&'static str`): producer Static, consumer Borrowed → no
    /// coercion needed (`&'static str` is borrow-compatible).
    #[test]
    fn literal_passed_to_borrow_param_needs_no_coercion() {
        let callee = method(
            "greet",
            vec!["who"],
            fn_sig(vec![("who", Ty::Str)], Ty::Nil),
            lit_str("ignored"),
        );
        let body = send("greet", vec![lit_str("dave")]);
        let mut caller = method(
            "call",
            vec![],
            fn_sig(vec![], Ty::Nil),
            body,
        );
        let reg = build_registry(&[class("X", vec![callee, caller.clone()])], &[]);
        color_method(&mut caller, &reg);

        let ExprNode::Send { args, .. } = caller.body.node.as_ref() else {
            panic!("expected Send at top of body");
        };
        assert_eq!(args[0].str_coercion, None);
    }

    /// Ivar (Owned) returned directly from a `-> String` function:
    /// producer and consumer both Owned → no coercion.
    #[test]
    fn ivar_returned_from_string_fn_needs_no_coercion() {
        let body = Expr::new(
            Span::synthetic(),
            ExprNode::Return { value: ivar("name") },
        );
        let mut m = method(
            "name",
            vec![],
            fn_sig(vec![], Ty::Str),
            body,
        );
        let reg = build_registry(&[class("X", vec![m.clone()])], &[]);
        color_method(&mut m, &reg);

        let ExprNode::Return { value } = m.body.node.as_ref() else {
            panic!("expected Return at top of body");
        };
        assert_eq!(value.str_coercion, None);
    }
}
