//! Arel IR — query algebra evaluable at transpile time.
//!
//! See `project_arel_compile_time_first.md` for the architectural
//! direction this implements. Phase 1 contains:
//!
//! - `ir`       — type definitions for the algebra (`ArelOp`,
//!                `Predicate`, `Value`, …).
//! - `visitor`  — `ArelVisitor` trait + `SqliteVisitor`
//!                implementation that turns an `ArelOp` into the
//!                same kind of `Expr` today's per-shape adapter
//!                methods produce.
//! - `build`    — `try_build_arel`: pattern recognizer that maps
//!                a Send call site to an `ArelOp`. Returns None
//!                for shapes the lowerer can't statically resolve;
//!                those route to runtime fallback in Phase 2.

pub mod build;
pub mod ir;
pub mod visitor;
mod ruby_values;

pub use build::{try_build_arel, try_build_arel_with_assocs};
pub use ir::{
    ArelOp, Assignment, ColRef, ColumnSpec, Delete, Direction, Insert, Join, JoinKind, LimitSpec,
    Order, Predicate, PreloadDirective, Select, Update, Value, ValueType,
};
pub use visitor::{ArelVisitor, SqliteVisitor};

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::expr::{Expr, ExprNode, InterpPart};
use crate::ident::{ClassId, Symbol};
use crate::schema::Schema;
use crate::ty::Ty;

/// Rewrite an Expr tree in-place: every Send that `try_build_arel`
/// recognizes is replaced by the visitor-emitted Expr. Sends that
/// don't match are left intact; recursion continues into their
/// receiver, args, and block.
///
/// The replacement happens top-down: when an outer Send matches, we
/// don't recurse into its parts (they're consumed into the
/// visitor-built tree). Inner Sends inside the visitor's output
/// don't need re-inspection — they're target-runtime Db.* calls,
/// not user code.
pub fn rewrite_arel_in_expr(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
) {
    let _ = rewrite_arel_in_expr_with_assocs(expr, schema, registry, &[]);
}

/// As `rewrite_arel_in_expr`, but with the app's association graph so
/// `includes(:assoc)` chains lower to eager-load preloads (issue #27).
/// The 3-arg wrapper passes an empty graph → legacy drop-includes.
///
/// Returns whether any node was rewritten so callers can skip a
/// follow-up type pass on an unchanged body.
pub fn rewrite_arel_in_expr_with_assocs(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
) -> bool {
    let no_scopes = std::collections::HashSet::new();
    rewrite_arel_in_expr_with_ruby_values(expr, schema, registry, assocs, false, &no_scopes, SqliteVisitor::default())
}

/// Ruby-family values preserve SQL NULL without changing strict-target emit.
/// `scopes` is the app's relation-returning class methods (`scope`
/// declarations plus class methods whose body tail is a query chain) —
/// see `relation_refined_method_names` for why these license the same
/// treatment as a builtin refiner (#558).
pub(crate) fn rewrite_arel_in_expr_with_ruby_values(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
    ruby_read_values: bool,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
    visitor: SqliteVisitor,
) -> bool {
    // Names (ivars/locals) the body later refines with relation-chain
    // methods (`@moderations.where(...)` after `@moderations =
    // Moderation.all...`). Materializing the assigned chain here would
    // hand those refiners an Array — leave such statements on the
    // runtime Relation path.
    let mut refined = RefinedNames::default();
    collect_relation_refined_names(expr, scopes, &mut refined);
    let mut changed = rewrite_arel_inner(expr, schema, registry, assocs, &refined, ruby_read_values, scopes, visitor);
    // Both call sites hand us a METHOD BODY, and a body that is a
    // single statement is not a `Seq` — so the hoist post-pass inside
    // `rewrite_arel_inner`, which walks a Seq's statement list, had no
    // list to walk. A one-line action (`def counts; @counts =
    // Comment.group(:post_id).count; end` — the shape issues #77/#78
    // reproduce with) then kept the whole hydrate Seq in the assign's
    // value position, and the Ruby emitter rendered it as `@counts =
    // stmt = Db.prepare(…)` with the accumulator dangling on the last
    // line: the ivar bound to the statement handle. Same break for any
    // folded chain in that position (`pluck`, `count`, a hydrate) —
    // visible only because a second statement in the body made the
    // body a Seq and hid it. Give the root its own statement list when
    // there is something to hoist into it.
    if !matches!(&*expr.node, ExprNode::Seq { .. }) {
        let mut hoisted = Vec::new();
        let replaced = hoist_value_seqs(expr, &mut hoisted);
        if !hoisted.is_empty() {
            let span = expr.span;
            let placeholder = Expr::new(
                span,
                ExprNode::Lit { value: crate::expr::Literal::Nil },
            );
            hoisted.push(std::mem::replace(expr, placeholder));
            *expr = Expr::new(span, ExprNode::Seq { exprs: hoisted });
            changed = true;
        } else if replaced {
            changed = true;
        }
    }
    changed
}

const RELATION_REFINERS: &[&str] = &[
    "where", "not", "joins", "left_outer_joins", "left_joins", "order", "group", "having",
    "limit", "offset", "merge", "includes", "preload", "eager_load", "references", "distinct",
    "select", "where!", "order!", "reorder", "rewhere",
];

/// A finder terminates a relation but still needs that relation as its
/// receiver. If the whole call cannot lift, hydrating only its receiver
/// would strand `find_by`/`find_by!` on an Array, just as for a refiner.
/// `sole`/`find_sole_by` likewise: unlike `first`, Array answers neither.
/// Rails' per-column dynamic finders (`find_by_id`, `find_by_email!`, …)
/// are the same shape as `find_by` and need the same treatment (#558).
fn requires_relation_receiver(method: &str) -> bool {
    RELATION_REFINERS.contains(&method)
        || matches!(method, "find_by" | "find_by!" | "sole" | "find_sole_by")
        || is_dynamic_finder(method)
}

/// `find(id)` / `find(ids)`, the primary-key finder, is a finder in the
/// same sense: when `Model.includes(:x).find(id)` cannot lift, hydrating
/// only the receiver leaves `Array#find(ifnone)`, which ignores the id
/// and answers an Enumerator. Block-form `find { … }` is Enumerable and
/// stays out; callers check `block` themselves.
fn is_id_finder(method: &str, args: &[Expr]) -> bool {
    method == "find" && !args.is_empty()
}

/// `find_by_<attr>` / `find_by_<attr>!` — Rails synthesizes one of these
/// per column. `find_by_` alone (no attribute) isn't a real finder.
fn is_dynamic_finder(method: &str) -> bool {
    dynamic_finder_attr(method).is_some()
}

/// Parse `find_by_<attr>` / `find_by_<attr>!` into the attribute name and
/// whether it's the bang (raising) form. Shared by the predicate above and
/// the normalization below so the two can't drift on what counts as one.
fn dynamic_finder_attr(method: &str) -> Option<(&str, bool)> {
    let (base, bang) = match method.strip_suffix('!') {
        Some(base) => (base, true),
        None => (method, false),
    };
    let attr = base.strip_prefix("find_by_")?;
    if attr.is_empty() {
        None
    } else {
        Some((attr, bang))
    }
}

/// A per-column dynamic finder that reaches here is about to run
/// against the runtime `ActiveRecord::Relation` (its chain above
/// declined to lift to Arel SQL — typically after a prior `.includes`
/// or similar). That runtime class has a fixed, statically typed
/// method surface (invariant: every `runtime/ruby/` body is fully
/// typed and resolvable, with no dynamic catch-all dispatch), so it
/// cannot carry one method per column the way Rails' own dynamic
/// finders do.
/// Normalize to the spelling it DOES implement — `find_by(attr:
/// value)` / `find_by!(attr: value)` — the same transform `find_by`
/// itself needs none of, since it's already spelled that way (#558).
///
/// Column-validated against the receiver's own model's actual SCHEMA
/// TABLE (`ClassInfo::has_schema_column` — the same check the
/// type-checker's `BodyTyper::dynamic_finder_ty` makes), not the
/// broader `instance_methods` map, which also carries synthesized
/// readers that are not real columns (`has_secure_password` seeds a
/// `password_reset_token` reader alongside the class-side
/// `find_by_password_reset_token`/`find_by_password_reset_token!`
/// finders it actually dispatches to; an `instance_methods`-only check
/// would rename that call into a bogus `find_by(password_reset_token:
/// …)` querying a column that doesn't exist). A name that isn't a real
/// column is left exactly as written, so it keeps dispatching to
/// whatever method actually owns it. The model is resolved by walking
/// the chain down to its root Const rather than trusting the
/// receiver's stamped `ty`: this pass runs on the lowerer's OWN
/// re-typing of the body, whose registry is assembled fresh per
/// lowering call and doesn't carry the main analyze pass's full
/// class-method catalog, so a `Model.includes(...)` chain root can
/// still read back `Untyped` here even though the construct is fully
/// supported end to end.
fn normalize_dynamic_finder_send(expr: &mut Expr, registry: &HashMap<ClassId, ClassInfo>) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node
    else {
        return false;
    };
    let Some((attr, bang)) = dynamic_finder_attr(method.as_str()) else {
        return false;
    };
    let [value] = &args[..] else { return false };
    let owns_column = chain_root_class(recv, registry)
        .and_then(|of| registry.get(&of))
        .is_some_and(|ci| ci.has_schema_column(&Symbol::from(attr)));
    if !owns_column {
        return false;
    }
    let span = value.span;
    let key = Expr::new(
        span,
        ExprNode::Lit { value: crate::expr::Literal::Sym { value: Symbol::from(attr) } },
    );
    let hash = Expr::new(
        span,
        ExprNode::Hash { entries: vec![(key, value.clone())], kwargs: true },
    );
    *method = Symbol::from(if bang { "find_by!" } else { "find_by" });
    *args = vec![hash];
    true
}

/// Walk a chain receiver down to its root and resolve the model it
/// names — the same "the innermost recv is eventually a registered
/// Const" assumption `try_chain_recv` makes when lifting a chain to
/// Arel. Falls back to the stamped type (when present) for a root this
/// syntactic walk doesn't reach, such as an association read.
fn chain_root_class<'e>(
    mut expr: &'e Expr,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Option<ClassId> {
    loop {
        if let Some(id) = build::const_to_class_id(expr, registry) {
            return Some(id);
        }
        match expr.node.as_ref() {
            ExprNode::Send { recv: Some(r), .. } => expr = r,
            _ => break,
        }
    }
    match expr.ty.as_ref() {
        Some(Ty::Relation { of }) | Some(Ty::Class { id: of, .. }) => Some(of.clone()),
        _ => None,
    }
}

/// As `requires_relation_receiver`, plus the app's own relation-returning
/// class methods (`scope :named, …`, or a class method whose body tail is
/// a query chain). An app scope chained after a class-chain receiver
/// (`Part.includes(:widget).named("b")`) licenses keeping that receiver a
/// Relation exactly as a builtin refiner does — it's the spelling an app
/// actually uses (#558).
fn requires_relation_receiver_or_scope(
    method: &crate::ident::Symbol,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
) -> bool {
    requires_relation_receiver(method.as_str()) || scopes.contains(method)
}

/// Method names whose RESULT this class then refines with a relation
/// method — `users_scope.active`, where `users_scope` is a method on
/// the same class.
///
/// The sibling guard below asks the same question about a NAME inside
/// one body; this asks it across a CLASS, because a method's return
/// value is consumed by its callers and the arel pass sees one body at
/// a time. Materializing a chain is a decision only the CONSUMER can
/// license: campfire's `users_scope` answers `User.all` from one
/// branch, the pass lifted it to a `from_stmt` hydrate loop (an
/// Array), and the caller's `.active` then had no relation left to
/// chain — "undefined method 'active' for an instance of Array", from a
/// method whose source says `User.all`.
///
/// Receiver shapes: a bare self-call (`users_scope.active`) and the
/// explicit `self.users_scope.active` the controller lowering writes.
/// `scopes` names the app's own relation-returning class methods —
/// `User.active`, `User.ordered` — which refine a relation exactly as
/// `where` does and are the spelling an app actually uses. Reading them
/// off the analyzer's registry rather than a second list keeps the two
/// from drifting, and it is what makes this guard fire at all:
/// campfire's caller is `users_scope.active`, and `active` is a scope.
pub fn relation_refined_method_names(
    body: &Expr,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
    out: &mut std::collections::HashSet<crate::ident::Symbol>,
) {
    if let ExprNode::Send { recv: Some(r), method, .. } = body.node.as_ref() {
        if requires_relation_receiver_or_scope(method, scopes) {
            if let Some(name) = self_call_name(r) {
                out.insert(name);
            }
        }
    }
    body.node
        .for_each_child(&mut |c| relation_refined_method_names(c, scopes, out));
}

/// The method name of a no-argument call on the implicit or explicit
/// self — the two spellings a controller helper is reached by.
fn self_call_name(e: &Expr) -> Option<crate::ident::Symbol> {
    let ExprNode::Send { recv, method, args, block: None, .. } = e.node.as_ref() else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    match recv {
        None => Some(method.clone()),
        Some(r) if matches!(r.node.as_ref(), ExprNode::SelfRef) => Some(method.clone()),
        _ => None,
    }
}

fn collect_relation_refined_names(
    expr: &Expr,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
    out: &mut RefinedNames,
) {
    if let ExprNode::Send { recv: Some(r), method, args, block, .. } = expr.node.as_ref() {
        if let ExprNode::Ivar { name } | ExprNode::Var { name, .. } = r.node.as_ref() {
            if requires_relation_receiver(method.as_str())
                || (block.is_none() && is_id_finder(method.as_str(), args))
            {
                out.always.insert(name.clone());
            } else if scopes.contains(method) {
                out.by_scope.entry(name.clone()).or_default().push(method.clone());
            }
        }
    }
    expr.node
        .for_each_child(&mut |c| collect_relation_refined_names(c, scopes, out));
}

/// The ivars/locals a body later refines with a relation method, so
/// their assignments must stay on the runtime Relation path.
///
/// A builtin refiner or finder (`where`, `find_by`, `find(id)`) refines
/// whatever the name holds. An app scope does not: `scopes` holds every
/// model's scope NAMES, so `@widget.gadget` (a belongs_to reader) next
/// to some other model's `scope :gadget` marked `@widget` refined and
/// left `@widget = Widget.all.find { … }` on the runtime Relation, whose
/// `find` takes an id (#569). A scope call refines one ASSIGNMENT, the
/// one whose chain is rooted at a model declaring that scope; an
/// assignment whose root is not a known model keeps the name-only
/// reading.
#[derive(Default)]
struct RefinedNames {
    always: std::collections::HashSet<Symbol>,
    by_scope: HashMap<Symbol, Vec<Symbol>>,
}

impl RefinedNames {
    fn keeps_relation(&self, name: &Symbol, value: &Expr, registry: &HashMap<ClassId, ClassInfo>) -> bool {
        if self.always.contains(name) {
            return true;
        }
        let Some(scope_calls) = self.by_scope.get(name) else { return false };
        match chain_root_class(value, registry) {
            None => true,
            Some(model) => scope_calls.iter().any(|m| model_declares_scope(registry, &model, m)),
        }
    }
}

/// Whether `model` or one of its ancestors declares `method` as a
/// relation-returning class method.
fn model_declares_scope(registry: &HashMap<ClassId, ClassInfo>, model: &ClassId, method: &Symbol) -> bool {
    let mut current = Some(model.clone());
    for _ in 0..32 {
        let Some(ci) = current.and_then(|id| registry.get(&id)) else { return false };
        if ci.class_methods.get(method).is_some_and(returns_relation) {
            return true;
        }
        current = ci.parent.clone();
    }
    false
}

/// A relation-returning class method's type: a `Relation`, or a method
/// answering one.
pub(crate) fn returns_relation(ty: &Ty) -> bool {
    match ty {
        Ty::Relation { .. } => true,
        Ty::Fn { ret, .. } => returns_relation(ret),
        _ => false,
    }
}

fn rewrite_arel_inner(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
    refined: &RefinedNames,
    ruby_read_values: bool,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
    visitor: SqliteVisitor,
) -> bool {
    if let ExprNode::Assign { target, value } = expr.node.as_ref() {
        let name = match target {
            crate::expr::LValue::Ivar { name } => Some(name),
            crate::expr::LValue::Var { name, .. } => Some(name),
            _ => None,
        };
        if name.is_some_and(|n| refined.keeps_relation(n, value, registry)) {
            return false;
        }
    }
    if let ExprNode::Send { .. } = expr.node.as_ref() {
        if let Some((mut op, owner)) =
            build::try_build_arel_with_assocs(expr, schema, registry, assocs)
        {
            if ruby_read_values {
                ruby_values::normalize(&mut op, schema);
            }
            let mut replacement = visitor.visit(&op, schema, &owner);
            // The expansion replaces the recognized chain wholesale;
            // its provenance is the chain call site. Subtrees the
            // builder lifted out of the chain (predicate values, …)
            // keep their own, tighter spans.
            replacement.inherit_span(expr.span);
            *expr = replacement;
            return true;
        }
    }
    // Inline sibling of the refined-names guard above: this Send is a
    // relation consumer whose chain did NOT lift (a lifted chain was
    // replaced wholesale and returned before reaching here — string
    // `order("tag asc")`, a chained `.where`, `references(...)`, …).
    // Recursing into its receiver would materialize the liftable base
    // underneath (`Category.all`, the has_many FK query) and strand the
    // consumer on a hydrated Array — `results.order("tag asc")` or
    // `results.find_by(id: value)`,
    // NoMethodError on every lane and a hard compile stop under AOT.
    // Leave the whole chain to the runtime Relation (the scope-chain
    // normalizer re-roots surviving `Const`-headed chains onto
    // `ActiveRecord::Relation.new(Model)` at emit). Spine Sends' args
    // and blocks are ordinary value positions and still rewrite. A
    // refiner WITH a block (`.select { … }`) is an Enumerable call on
    // materialized rows, not a chain link — the claim stays.
    let unlifted_relation_consumer = matches!(
        expr.node.as_ref(),
        ExprNode::Send { recv: Some(_), block: None, method, args, .. }
            if requires_relation_receiver_or_scope(method, scopes)
                || is_id_finder(method.as_str(), args)
    );
    if unlifted_relation_consumer {
        let mut changed = normalize_dynamic_finder_send(expr, registry);
        let ExprNode::Send { recv: Some(recv), args, .. } = &mut *expr.node else {
            unreachable!("matched Send with recv above");
        };
        changed |= rewrite_arel_spine_args(recv, schema, registry, assocs, refined, ruby_read_values, scopes, visitor);
        for a in args {
            changed |= rewrite_arel_inner(a, schema, registry, assocs, refined, ruby_read_values, scopes, visitor);
        }
        return changed;
    }
    let mut changed = false;
    walk_subexprs_mut(expr, &mut |e| {
        changed |= rewrite_arel_inner(e, schema, registry, assocs, refined, ruby_read_values, scopes, visitor)
    });
    // Post-pass: when an Arel rewrite landed a multi-stmt hydrate Seq
    // in a *value* position — directly as an Assign value
    // (`@articles = <hydrate Seq>`) or nested inside a larger
    // expression (`@stories = period(<hydrate Seq>)`, where the
    // recognizer only matched the innermost `Story.includes(...)` and
    // left the Seq buried as a chain receiver) — hoist the Seq's
    // leading stmts out ahead of the enclosing statement and collapse
    // the Seq to its final expression. The Ruby emitter can't render an
    // inline multi-stmt value (`x = (a; b; c)`), so normalize
    // structurally.
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        changed |= hoist_value_seqs_in_stmts(exprs);
    }
    changed
}

/// Rewrite value positions inside a chain-receiver spine WITHOUT
/// claiming the spine itself: every Send along the receiver path feeds
/// its value to an unlifted relation refiner, so materializing one
/// would strand the chain on a hydrated Array. The spine Sends' args
/// and blocks are ordinary value positions and rewrite normally.
/// Non-Send spine roots (a Const, an Ivar, an If picking a branch, …)
/// stay untouched for the same reason — anything materialized inside
/// them still becomes the chain's receiver value.
fn rewrite_arel_spine_args(
    expr: &mut Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[crate::lower::model_associations::AssociationEdge],
    refined: &RefinedNames,
    ruby_read_values: bool,
    scopes: &std::collections::HashSet<crate::ident::Symbol>,
    visitor: SqliteVisitor,
) -> bool {
    let mut changed = false;
    if let ExprNode::Send { recv, args, block, .. } = &mut *expr.node {
        if let Some(r) = recv {
            changed |= rewrite_arel_spine_args(r, schema, registry, assocs, refined, ruby_read_values, scopes, visitor);
        }
        for a in args {
            changed |= rewrite_arel_inner(a, schema, registry, assocs, refined, ruby_read_values, scopes, visitor);
        }
        if let Some(b) = block {
            changed |= rewrite_arel_inner(b, schema, registry, assocs, refined, ruby_read_values, scopes, visitor);
        }
    }
    changed
}

/// For each statement in a Seq's stmt list, hoist any multi-stmt Seq an
/// Arel rewrite landed in one of its value positions (see
/// [`hoist_value_seqs`]), inserting the hoisted stmts ahead of it.
fn hoist_value_seqs_in_stmts(stmts: &mut Vec<Expr>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < stmts.len() {
        let mut hoisted = Vec::new();
        let replaced = hoist_value_seqs(&mut stmts[i], &mut hoisted);
        if hoisted.is_empty() {
            if replaced {
                changed = true;
            }
            i += 1;
            continue;
        }
        changed = true;
        let added = hoisted.len();
        for (j, stmt) in hoisted.into_iter().enumerate() {
            stmts.insert(i + j, stmt);
        }
        i += added + 1;
    }
    changed
}

/// Recurse through the *value* positions of `e` (call recv/args, assign
/// value, operands, array/hash values, …) and hoist every nested Seq
/// into `hoisted`, replacing it with its final expression. Statement-
/// context children (block bodies, if/while branches, nested stmt Seqs)
/// are NOT descended — their own enclosing Seq's post-pass handles them.
///
/// Note: two hydrate Seqs hoisted from one statement would both bind the
/// visitor's fixed `stmt`/`results` locals and collide; that multi-query-
/// per-statement case is a pre-existing visitor-naming limitation, not
/// introduced here (every recognized site uses the same var names).
fn hoist_value_seqs(e: &mut Expr, hoisted: &mut Vec<Expr>) -> bool {
    let mut changed = false;
    match &mut *e.node {
        ExprNode::Send { recv, args, .. } => {
            if let Some(r) = recv {
                changed |= hoist_value_child(r, hoisted);
            }
            for a in args {
                changed |= hoist_value_child(a, hoisted);
            }
        }
        ExprNode::Apply { fun, args, .. } => {
            changed |= hoist_value_child(fun, hoisted);
            for a in args {
                changed |= hoist_value_child(a, hoisted);
            }
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            changed |= hoist_value_child(value, hoisted);
        }
        ExprNode::BoolOp { left, right, .. } => {
            changed |= hoist_value_child(left, hoisted);
            changed |= hoist_value_child(right, hoisted);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                changed |= hoist_value_child(el, hoisted);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (_, v) in entries {
                changed |= hoist_value_child(v, hoisted);
            }
        }
        ExprNode::Return { value }
        | ExprNode::Raise { value }
        | ExprNode::Splat { value }
        | ExprNode::KeywordSplat { value } => {
            changed |= hoist_value_child(value, hoisted);
        }
        ExprNode::Yield { args } => {
            for a in args {
                changed |= hoist_value_child(a, hoisted);
            }
        }
        _ => {}
    }
    changed
}

/// Process one value-position child: recurse into its own value
/// positions, then — if the child is itself a Seq — move its leading
/// statements into `hoisted` and collapse it to its final expression.
fn hoist_value_child(child: &mut Expr, hoisted: &mut Vec<Expr>) -> bool {
    let mut changed = hoist_value_seqs(child, hoisted);
    if matches!(&*child.node, ExprNode::Seq { .. }) {
        let placeholder = Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Lit { value: crate::expr::Literal::Nil },
        );
        let seq = std::mem::replace(child, placeholder);
        if let ExprNode::Seq { exprs } = *seq.node {
            let mut exprs = exprs;
            if let Some(last) = exprs.pop() {
                hoisted.extend(exprs);
                *child = last;
            }
            // Empty Seq → keep the nil placeholder.
        }
        // Replacement itself is a tree change even when nothing is
        // hoisted (empty `begin; end` → nil). Callers retype on this.
        changed = true;
    }
    changed
}

/// Mutable visitor for every direct sub-Expr of `expr`. Caller
/// applies whatever transform via `f`; this only handles the
/// recursion shape so adding a new ExprNode variant updates one
/// place.
pub(crate) fn walk_subexprs_mut(expr: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut *expr.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::ForwardArgs
        | ExprNode::ForwardKeywords
        | ExprNode::Defined { .. }
        | ExprNode::SelfRef => {}
        ExprNode::ForwardKeywordsWithPairs { entries } => {
            for (key, value) in entries {
                f(key);
                f(value);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::Array { elements, .. } => {
            for e in elements {
                f(e);
            }
        }
        ExprNode::StringInterp { parts } => {
            for part in parts {
                if let InterpPart::Expr { expr } = part {
                    f(expr);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::MethodRef { recv, .. } => {
            if let Some(r) = recv {
                f(r);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                f(r);
            }
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        ExprNode::CaseMatch { scrutinee, arms, else_body } => {
            f(scrutinee);
            for arm in arms {
                arm.pattern.for_each_expr_mut(f);
                if let Some((_, g)) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
            if let Some(e) = else_body {
                f(e);
            }
        }
        ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
            f(value);
            pattern.for_each_expr_mut(f);
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                f(e);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue_mut(target, f);
            f(value);
        }
        ExprNode::Yield { args } => {
            for a in args {
                f(a);
            }
        }
        ExprNode::Raise { value } => f(value),
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr);
            f(fallback);
        }
        ExprNode::Return { value } => f(value),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    f(a);
                }
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                f(v);
            }
        }
        ExprNode::Splat { value } => {
            f(value);
        }
        ExprNode::KeywordSplat { value } => {
            f(value);
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue_mut(t, f);
            }
            f(value);
        }
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                f(b);
            }
            if let Some(e) = end {
                f(e);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body);
            for r in rescues {
                for c in &mut r.classes {
                    f(c);
                }
                f(&mut r.body);
            }
            if let Some(e) = else_branch {
                f(e);
            }
            if let Some(e) = ensure {
                f(e);
            }
        }
        ExprNode::Cast { value, .. } => f(value),
    }
}

fn walk_lvalue_mut(lv: &mut crate::expr::LValue, f: &mut dyn FnMut(&mut Expr)) {
    use crate::expr::LValue;
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } | LValue::Const { .. } => {}
        LValue::Attr { recv, .. } => f(recv),
        LValue::Index { recv, index } => {
            f(recv);
            f(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{LValue, Literal};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;

    fn var(n: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(n) })
    }
    fn lit(s: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s.into() } })
    }
    fn assign(name: &str, value: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
                value,
            },
        )
    }
    fn seq_node(exprs: Vec<Expr>) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
    }
    fn call(method: &str, args: Vec<Expr>) -> Expr {
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

    /// A hydrate-shaped Seq whose final expression is the `results` var.
    fn hydrate_seq() -> Expr {
        seq_node(vec![
            assign("stmt", lit("prepare")),
            assign("results", lit("[]")),
            var("results"),
        ])
    }

    #[test]
    fn hoists_query_seq_nested_in_call_arg() {
        // `@x = period(<hydrate Seq>)` — the Seq is buried in the call
        // arg; its leading stmts must hoist out and the call bind to the
        // Seq's final expr (`period(results)`).
        let mut stmts = vec![assign("x", call("period", vec![hydrate_seq()]))];
        hoist_value_seqs_in_stmts(&mut stmts);

        assert_eq!(stmts.len(), 3, "two leading stmts hoisted ahead of the assign");
        let ExprNode::Assign { value, .. } = &*stmts[2].node else { panic!("expected assign") };
        let ExprNode::Send { args, .. } = &*value.node else { panic!("expected period(...)") };
        assert!(
            matches!(&*args[0].node, ExprNode::Var { .. }),
            "the Seq arg collapsed to its `results` var"
        );
    }

    #[test]
    fn direct_assign_seq_still_hoists_unchanged() {
        // The original `@x = <hydrate Seq>` case must behave identically.
        let mut stmts = vec![assign("x", hydrate_seq())];
        hoist_value_seqs_in_stmts(&mut stmts);
        assert_eq!(stmts.len(), 3);
        let ExprNode::Assign { value, .. } = &*stmts[2].node else { panic!() };
        assert!(matches!(&*value.node, ExprNode::Var { .. }), "binds to the results var");
    }

    #[test]
    fn empty_value_seq_replacement_is_a_change() {
        // `x = begin; end` is an empty Seq in value position. Hoisting
        // replaces it with nil and adds no statements; that still has
        // to count as a rewrite so the controller retypes the body.
        let mut stmts = vec![assign("x", seq_node(vec![]))];
        assert!(
            hoist_value_seqs_in_stmts(&mut stmts),
            "Seq-to-nil must set changed even with an empty hoist list"
        );
        let ExprNode::Assign { value, .. } = &*stmts[0].node else { panic!("expected assign") };
        assert!(
            matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }),
            "empty Seq collapsed to nil"
        );
    }
}
