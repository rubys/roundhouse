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
            // Index assignment into a Hash<_, Str>: HashMap stores
            // `String` values, so RHS string must be Owned too. Other
            // LValues don't impose a string constraint today.
            let expect = match target {
                LValue::Ivar { .. } if is_str_ty(value.ty.as_ref()) => {
                    ParentExpect::Color(StrColor::Owned)
                }
                LValue::Index { recv, index } => {
                    // Walk the recv + index first (their own positions
                    // are independent of the assignment's RHS color),
                    // then derive the RHS expectation from the Hash's
                    // value type.
                    count += walk(recv, ParentExpect::None, ctx);
                    count += walk(index, ParentExpect::None, ctx);
                    match recv.ty.as_ref() {
                        Some(Ty::Hash { value: v_ty, .. })
                            if matches!(v_ty.as_ref(), Ty::Str | Ty::Sym) =>
                        {
                            ParentExpect::Color(StrColor::Owned)
                        }
                        _ => ParentExpect::None,
                    }
                }
                LValue::Attr { recv, .. } => {
                    count += walk(recv, ParentExpect::None, ctx);
                    ParentExpect::None
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
            //
            // Additionally, when the parent imposes no expectation
            // but the two branches' producer colors disagree, unify
            // them to the more-owned color. Without this, `let x = if
            // cond { lit } else { string_call() }` infers a Rust
            // type-mismatch error.
            count += walk(cond, ParentExpect::None, ctx);
            let branch_expect = match tail_expect {
                ParentExpect::Color(_) => tail_expect,
                ParentExpect::None => unify_branches_expect(then_branch, else_branch),
            };
            count += walk(then_branch, branch_expect, ctx);
            count += walk(else_branch, branch_expect, ctx);
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
            // Tuple-type unification: `HashMap::from([(k1, v1), ...])`
            // infers its key/value types from the FIRST tuple. If any
            // subsequent entry's value has a different ownership color
            // (e.g., first is `&str` literal, second is a String var),
            // the compiler rejects. Compute the "homogeneous color"
            // across all entries' values (and separately keys) and
            // propagate as expectation so literals get ToOwned'd into
            // the dominant Owned color when needed.
            let value_expect =
                hash_homogeneous_expect(entries.iter().map(|(_, v)| v));
            let key_expect =
                hash_homogeneous_expect(entries.iter().map(|(k, _)| k));
            for (k, v) in entries.iter_mut() {
                count += walk(k, key_expect, ctx);
                count += walk(v, value_expect, ctx);
            }
        }
        ExprNode::Array { elements, .. } => {
            // `vec![...]` infers element type from the first element.
            // Same homogeneity story as Hash above.
            let elem_expect = hash_homogeneous_expect(elements.iter());
            for el in elements.iter_mut() {
                count += walk(el, elem_expect, ctx);
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

/// Unify two If-branch producer colors. If both branches are
/// string-typed but produce different colors (e.g. then yields a
/// literal, else yields a String from a Send), Rust rejects the
/// resulting If as a type mismatch. Return the more-owned color as
/// the expectation so coercions land on the literal side.
fn unify_branches_expect(then_branch: &Expr, else_branch: &Expr) -> ParentExpect {
    let then_likely_owned = branch_likely_owned(then_branch);
    let else_likely_owned = branch_likely_owned(else_branch);
    let then_has_literal = branch_has_string_literal(then_branch);
    let else_has_literal = branch_has_string_literal(else_branch);
    if (then_likely_owned && else_has_literal) || (else_likely_owned && then_has_literal) {
        ParentExpect::Color(StrColor::Owned)
    } else {
        ParentExpect::None
    }
}

/// True when this branch's tail-position emit is likely String-shaped
/// (Send, StringInterp, Ivar, or a Var/Send chain). Conservative —
/// only inspects the immediate tail expression of a Seq, not deeper.
fn branch_likely_owned(e: &Expr) -> bool {
    matches!(
        tail_expr(e).node.as_ref(),
        ExprNode::Send { .. }
            | ExprNode::StringInterp { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Var { .. }
    )
}

fn branch_has_string_literal(e: &Expr) -> bool {
    matches!(
        tail_expr(e).node.as_ref(),
        ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
    )
}

/// Peel a Seq down to its tail expression (the value the Seq
/// produces). Other shapes are their own tail.
fn tail_expr(e: &Expr) -> &Expr {
    match e.node.as_ref() {
        ExprNode::Seq { exprs } => exprs.last().map_or(e, tail_expr),
        _ => e,
    }
}

/// Compute the homogeneous expectation for an iterator of sibling
/// string expressions (Hash entries, Array elements). Rust infers the
/// collection's element type from the first entry, so if ANY entry
/// is non-literal-string the collection settles as `String`-typed and
/// other entries (typically `&'static str` literals) need ToOwned.
///
/// Treats any non-literal `Ty::Str` expression as `Owned`-producing
/// regardless of its computed producer color — covers local `Var`s
/// bound from `format!()`/method calls (which `producer_color` would
/// mis-classify as `Borrowed` until Phase 2.6 lands a local-let
/// symbol table). The downside is over-coercion: a literal in a
/// hash whose other entries are all `&str`-typed local Vars will get
/// an unneeded `ToOwned`; Rust then infers `HashMap<_, String>` and
/// accepts it. Suboptimal vs. ideal `HashMap<_, &str>`, but the
/// alternative — over-strict `&str` inference — fails compilation,
/// which is strictly worse.
fn hash_homogeneous_expect<'a, I: Iterator<Item = &'a Expr>>(entries: I) -> ParentExpect {
    let mut has_string_literal = false;
    let mut has_likely_owned = false;
    for e in entries {
        match e.node.as_ref() {
            ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } } => {
                has_string_literal = true;
            }
            // Non-literal string-producing shapes — Var (which our
            // producer_color hardcodes as Borrowed but is usually a
            // local holding String at runtime), Send (Ty::Str return
            // emits as String), Ivar (String field read), StringInterp
            // (format!() yields String). Trigger the Owned expectation
            // whenever any of these appear alongside a literal entry,
            // even if the Var's Ty annotation is missing (the
            // body-typer doesn't always propagate Ty to Var reads
            // bound from non-let RHS shapes like `x = label || ...`).
            ExprNode::Var { .. }
            | ExprNode::Send { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::StringInterp { .. } => {
                has_likely_owned = true;
            }
            _ => {}
        }
    }
    if has_string_literal && has_likely_owned {
        ParentExpect::Color(StrColor::Owned)
    } else {
        ParentExpect::None
    }
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
