//! `<params>.merge(k: v)` where the merge is NOT on the permit chain.
//!
//! `controller_to_library::params` recognizes `permit(...).merge(k: v)`
//! syntactically and folds the merged keys into the spec's field list.
//! That only reaches merges written directly on the chain. campfire
//! writes the merge a method and a class away:
//!
//! ```ruby
//! # app/controllers/first_runs_controller.rb
//! def create
//!   user = FirstRun.create!(user_params)          # → UserParams
//! end
//! def user_params
//!   params.require(:user).permit(:name, :avatar, :email_address, :password)
//! end
//!
//! # app/models/first_run.rb  (a plain class, not a model)
//! def self.create!(user_params)
//!   administrator = room.creator = User.new(user_params.merge(role: :administrator))
//! end
//! ```
//!
//! So `role` never joined the field set, and the emit called
//! `UserParams#merge`, which doesn't exist — the whole first-run signup
//! died on it.
//!
//! **Widening the spec is not the fix here.** `UserParams` is shared:
//! `UsersController` (plain public signup) permits the identical four
//! fields, so it holds the same class. Folding `role` into the field
//! list would make `UserParams.from_raw` read `role` from the request,
//! and a signup form could POST `user[role]=administrator`. The merge
//! has to stay CALL-SITE LOCAL, assigned on the model after
//! construction:
//!
//! ```ruby
//! _pm0 = User.from_params(user_params)
//! _pm0.role = 1
//! administrator = room.creator = _pm0
//! ```
//!
//! Two things this needs that a local rewrite can't supply:
//!
//! 1. **Knowing `user_params` holds a `UserParams`.** Its declared type
//!    is `untyped` — nothing infers a parameter's type from its call
//!    sites. This pass proves it narrowly instead: scan every
//!    controller for `<Const>.<method>(…, <x>_params, …)` where the
//!    helper resolves to a spec, and bind that spec to the callee's
//!    parameter. Every call site must agree; one disagreeing site
//!    poisons the binding and nothing is rewritten. The proven type is
//!    then stamped on the method's signature — REQUIRED, not a bonus:
//!    a strict target can't pass an `untyped` into the `UserParams`-typed
//!    factory this pass calls.
//!
//! 2. **Statement position.** The construction has to become several
//!    statements, and a `Seq` left in expression position renders as
//!    newline-joined statements — `administrator = room.creator = _pm0 =
//!    User.from_params(...)` followed by loose lines, which binds the
//!    wrong value. So the prelude is HOISTED above the enclosing
//!    statement and the matched node becomes a temp read.
//!
//! All-or-nothing per site, and it fails closed: a merged key with no
//! writer, or a model whose resource doesn't match the spec's, leaves
//! the source shape in place and files a residue diagnostic. Emitting a
//! setter that doesn't exist would trade a named gap for a silent one.

use std::collections::{BTreeMap, HashMap};

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{MethodDef, MethodReceiver, Model};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

use super::controller_to_library::params::{
    collect_specs, helper_spec_map, model_from_params_name, ParamsSpec, ParamsSpecs,
};

/// `(callee class, method, parameter index)`.
type BindKey = (Symbol, Symbol, usize);

pub fn apply_params_merge_lowering(app: &mut App) -> Vec<Diagnostic> {
    let specs = collect_specs(&app.controllers);
    if specs.iter().next().is_none() {
        return Vec::new();
    }
    let bindings = scan_bindings(app, &specs);
    if bindings.is_empty() {
        return Vec::new();
    }

    // Writer surface per model, so a merged key can be checked before
    // anything is rewritten. Same authority the permit-writer filter
    // uses — `model_to_library::writable_field_set`.
    let mut writers: HashMap<Symbol, WriterSet> = HashMap::new();
    let mut resource_of: HashMap<Symbol, Symbol> = HashMap::new();
    for m in &app.models {
        let Some(table) = app.schema.tables.get(&m.table.0) else { continue };
        writers.insert(
            m.name.0.clone(),
            super::model_to_library::writable_field_set(m, table),
        );
        resource_of.insert(
            m.name.0.clone(),
            Symbol::from(crate::naming::snake_case(m.name.0.as_str())),
        );
    }
    // `model_defines_writer` needs the Model, and the rewrite runs while
    // `app.models` is mutably borrowed — settle the per-key question now.
    let models: Vec<Model> = app.models.clone();

    let ctx = Ctx { specs: &specs, writers: &writers, resource_of: &resource_of, models: &models };
    let mut diags = Vec::new();

    for lc in &mut app.library_classes {
        let owner = lc.name.0.clone();
        for method in &mut lc.methods {
            rewrite_method(&owner, method, &bindings, &ctx, &mut diags);
        }
    }
    for model in &mut app.models {
        let owner = model.name.0.clone();
        for item in &mut model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                rewrite_method(&owner, method, &bindings, &ctx, &mut diags);
            }
        }
    }
    diags
}

struct Ctx<'a> {
    specs: &'a ParamsSpecs,
    writers: &'a HashMap<Symbol, WriterSet>,
    resource_of: &'a HashMap<Symbol, Symbol>,
    models: &'a [Model],
}

type WriterSet = std::collections::BTreeSet<Symbol>;

impl Ctx<'_> {
    fn can_assign(&self, model: &Symbol, field: &Symbol) -> bool {
        if self.writers.get(model).is_some_and(|w| w.contains(field)) {
            return true;
        }
        self.models
            .iter()
            .find(|m| &m.name.0 == model)
            .is_some_and(|m| super::model_to_library::model_defines_writer(m, field))
    }
}

// ---------------------------------------------------------------------------
// Scan: which parameter of which method holds which params class.
// ---------------------------------------------------------------------------

fn scan_bindings(app: &App, specs: &ParamsSpecs) -> HashMap<BindKey, ClassId> {
    // `None` = poisoned: two call sites disagreed, or one passed
    // something that isn't a params helper at all.
    let mut seen: HashMap<BindKey, Option<ClassId>> = HashMap::new();

    // EVERY call site has to be seen, not just the ones that could bind
    // — a site passing a plain Hash is exactly what proves the parameter
    // ISN'T uniformly a params object. Coverage mirrors
    // `for_each_hook_body` (models, library classes, controllers, seeds);
    // only controller actions can name a `<x>_params` helper, and only
    // their own controller's.
    for controller in &app.controllers {
        let actions: Vec<crate::dialect::Action> = controller.actions().cloned().collect();
        let helpers = helper_spec_map(&actions, specs);
        for action in &actions {
            scan_body(&action.body, &helpers, &mut seen);
        }
    }
    let none = BTreeMap::new();
    for model in &app.models {
        for item in &model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    scan_body(&method.body, &none, &mut seen)
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    scan_body(&scope.body, &none, &mut seen)
                }
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => {
                    scan_body(expr, &none, &mut seen)
                }
                _ => {}
            }
        }
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            scan_body(&method.body, &none, &mut seen);
        }
        for (_name, value) in &lc.constants {
            scan_body(value, &none, &mut seen);
        }
        for call in &lc.unknown_calls {
            scan_body(call, &none, &mut seen);
        }
    }
    if let Some(seeds) = &app.seeds {
        scan_body(seeds, &none, &mut seen);
    }

    seen.into_iter().filter_map(|(k, v)| v.map(|c| (k, c))).collect()
}

fn scan_body(
    body: &Expr,
    helpers: &BTreeMap<Symbol, &ParamsSpec>,
    seen: &mut HashMap<BindKey, Option<ClassId>>,
) {
    walk(body, &mut |e| {
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
            return;
        };
        let ExprNode::Const { path } = &*recv.node else { return };
        let Some(class) = path.last() else { return };
        for (i, arg) in args.iter().enumerate() {
            let key = (class.clone(), method.clone(), i);
            let bound = match &*arg.node {
                ExprNode::Send { recv: None, method: h, args, block: None, .. }
                    if args.is_empty() =>
                {
                    helpers.get(h).map(|s| s.class_id.clone())
                }
                _ => None,
            };
            match (seen.get(&key), bound) {
                (None, b) => {
                    seen.insert(key, b);
                }
                // Agreeing sites reinforce; anything else poisons.
                (Some(Some(prev)), Some(b)) if *prev == b => {}
                (Some(None), None) => {}
                _ => {
                    seen.insert(key, None);
                }
            }
        }
    });
}

fn walk<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(expr);
    expr.node.for_each_child(&mut |c| walk(c, f));
}

// ---------------------------------------------------------------------------
// Rewrite.
// ---------------------------------------------------------------------------

fn rewrite_method(
    owner: &Symbol,
    method: &mut MethodDef,
    bindings: &HashMap<BindKey, ClassId>,
    ctx: &Ctx<'_>,
    diags: &mut Vec<Diagnostic>,
) {
    // Only `<Const>.<method>` call sites are scanned, so only class-side
    // methods can carry a proven binding.
    if !matches!(method.receiver, MethodReceiver::Class) {
        return;
    }
    let bound: Vec<(usize, Symbol, &ParamsSpec)> = method
        .params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let class_id = bindings.get(&(owner.clone(), method.name.clone(), i))?;
            let spec = ctx.specs.by_class(class_id)?;
            Some((i, p.name.clone(), spec))
        })
        .collect();
    if bound.is_empty() {
        return;
    }

    let mut n = 0usize;
    let mut rewrote = false;
    hoist_in_statements(&mut method.body, &mut |stmt, prelude| {
        rewrite_stmt(stmt, prelude, &bound, ctx, &mut n, &mut rewrote, diags)
    });
    if !rewrote {
        return;
    }
    // The rewrite calls a `UserParams`-typed factory with this
    // parameter, so its declared type has to say so — an `untyped`
    // here doesn't compile on a strict target.
    stamp_param_types(method, &bound);
}

fn stamp_param_types(method: &mut MethodDef, bound: &[(usize, Symbol, &ParamsSpec)]) {
    // An app method ingested from source usually carries no signature at
    // all (the rbs emit then renders every param `untyped`), so build one
    // rather than declining — the whole point is that this parameter's
    // type is now known.
    let (mut params, block, ret, effects) = match method.signature.clone() {
        Some(Ty::Fn { params, block, ret, effects }) => (params, block, ret, effects),
        _ => (
            method
                .params
                .iter()
                .map(|p| crate::ty::Param {
                    name: p.name.clone(),
                    ty: Ty::Untyped,
                    kind: crate::ty::ParamKind::Required,
                })
                .collect(),
            None,
            Box::new(Ty::Untyped),
            method.effects.clone(),
        ),
    };
    for (i, _name, spec) in bound {
        if let Some(p) = params.get_mut(*i) {
            p.ty = Ty::Class { id: spec.class_id.clone(), args: vec![] };
        }
    }
    method.signature = Some(Ty::Fn { params, block, ret, effects });
}

/// Offer each STATEMENT in `body` to `f`, which may rewrite it and push
/// statements to run before it.
///
/// The distinction that matters is statement position vs expression
/// position. Recursing into every child would offer `User.new(...)`
/// itself as a "statement", and its prelude would be spliced into the
/// expression slot it came from — a `Seq` nested inside `administrator =
/// room.creator = …`, which renders as newline-joined lines and binds
/// the wrong value. So descend only where a statement list genuinely
/// lives (a Seq, an `if` branch, a block/lambda body, a `case` arm),
/// and hand `f` the whole enclosing statement otherwise — `replace_in`
/// finds the match anywhere inside it.
fn hoist_in_statements(body: &mut Expr, f: &mut impl FnMut(&mut Expr, &mut Vec<Expr>)) {
    if let ExprNode::Seq { exprs } = &mut *body.node {
        let mut out: Vec<Expr> = Vec::with_capacity(exprs.len());
        for mut stmt in std::mem::take(exprs) {
            visit_statement(&mut stmt, f, &mut out);
        }
        *exprs = out;
        return;
    }
    // A bare (non-Seq) body is itself the whole statement list.
    let mut out = Vec::new();
    let mut stmt =
        std::mem::replace(body, Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }));
    visit_statement(&mut stmt, f, &mut out);
    *body = if out.len() == 1 {
        out.pop().expect("checked")
    } else {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: out })
    };
}

/// Rewrite one statement into `out` — its prelude first, then itself.
fn visit_statement(
    stmt: &mut Expr,
    f: &mut impl FnMut(&mut Expr, &mut Vec<Expr>),
    out: &mut Vec<Expr>,
) {
    for nested in nested_statement_lists(stmt) {
        hoist_in_statements(nested, f);
    }
    let mut prelude = Vec::new();
    f(stmt, &mut prelude);
    out.extend(prelude);
    out.push(std::mem::replace(
        stmt,
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    ));
}

/// The sub-expressions of `stmt` that are themselves statement lists.
fn nested_statement_lists(stmt: &mut Expr) -> Vec<&mut Expr> {
    match &mut *stmt.node {
        ExprNode::If { then_branch, else_branch, .. } => vec![then_branch, else_branch],
        ExprNode::Case { arms, .. } => arms.iter_mut().map(|a| &mut a.body).collect(),
        ExprNode::Lambda { body, .. } => vec![body],
        ExprNode::Send { block: Some(block), .. } => vec![block],
        ExprNode::Apply { block: Some(block), .. } => vec![block],
        ExprNode::RescueModifier { expr, fallback } => vec![expr, fallback],
        _ => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_stmt(
    stmt: &mut Expr,
    prelude: &mut Vec<Expr>,
    bound: &[(usize, Symbol, &ParamsSpec)],
    ctx: &Ctx<'_>,
    n: &mut usize,
    rewrote: &mut bool,
    diags: &mut Vec<Diagnostic>,
) {
    replace_in(stmt, &mut |e| {
        let Some(site) = match_new_with_merge(e, bound) else { return None };
        match plan(&site, ctx) {
            Ok(plan) => {
                let tmp = Symbol::from(format!("_pm{n}"));
                *n += 1;
                let read = |sp| Expr::new(sp, ExprNode::Var { id: VarId(0), name: tmp.clone() });
                prelude.push(Expr::new(
                    e.span,
                    ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: tmp.clone() },
                        value: Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: Some(Expr::new(
                                    e.span,
                                    ExprNode::Const { path: vec![site.model.clone()] },
                                )),
                                method: plan.factory,
                                args: vec![site.params_read.clone()],
                                block: None,
                                parenthesized: true,
                            },
                        ),
                    },
                ));
                for (key, value) in &site.merged {
                    prelude.push(Expr::new(
                        e.span,
                        ExprNode::Assign {
                            target: LValue::Attr { recv: read(e.span), name: key.clone() },
                            value: value.clone(),
                        },
                    ));
                }
                *rewrote = true;
                Some(read(e.span))
            }
            Err(reason) => {
                diags.push(super::residue_diagnostic(
                    "params_merge",
                    "params-merge-across-boundary",
                    e.span,
                    reason,
                    format!(
                        "`{}.new(<params>.merge(...))` left in source shape ({reason}) — \
                         the synthesized params class has no `merge`, so this call site \
                         will not resolve",
                        site.model.as_str()
                    ),
                ));
                None
            }
        }
    });
}

/// One recognized `<Model>.new(<bound param>.merge(k: v, …))`.
struct Site<'a> {
    model: Symbol,
    /// The parameter read the merge hangs off, reused verbatim as the
    /// factory's argument.
    params_read: Expr,
    spec: &'a ParamsSpec,
    merged: Vec<(Symbol, Expr)>,
}

struct Plan {
    factory: Symbol,
}

fn match_new_with_merge<'a>(
    e: &Expr,
    bound: &[(usize, Symbol, &'a ParamsSpec)],
) -> Option<Site<'a>> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "new" || args.len() != 1 {
        return None;
    }
    let ExprNode::Const { path } = &*recv.node else { return None };
    let model = path.last()?.clone();

    let ExprNode::Send { recv: Some(inner), method: m, args: margs, block: None, .. } =
        &*args[0].node
    else {
        return None;
    };
    if m.as_str() != "merge" || margs.len() != 1 {
        return None;
    }
    let ExprNode::Var { name, .. } = &*inner.node else { return None };
    let spec = bound.iter().find(|(_, p, _)| p == name).map(|(_, _, s)| *s)?;

    let ExprNode::Hash { entries, .. } = &*margs[0].node else { return None };
    let mut merged = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
            return None;
        };
        merged.push((value.clone(), v.clone()));
    }
    Some(Site { model, params_read: inner.clone(), spec, merged })
}

fn plan(site: &Site<'_>, ctx: &Ctx<'_>) -> Result<Plan, &'static str> {
    // `Model.from_params(p)` exists only when the model is the one the
    // spec's resource names — the model lowerer sizes its factories off
    // its OWN resource's permit lists.
    match ctx.resource_of.get(&site.model) {
        Some(r) if *r == site.spec.resource => {}
        Some(_) => return Err("the model is not the one this permit list names"),
        None => return Err("receiver is not an app model"),
    }
    if site.merged.iter().any(|(k, _)| !ctx.can_assign(&site.model, k)) {
        return Err("a merged key has no writer on the model");
    }
    Ok(Plan { factory: model_from_params_name(site.spec) })
}

/// Post-order in-place replacement — `map_expr`'s mutating twin, kept
/// local because the callback needs `&mut` capture (it pushes prelude
/// statements) which `map_expr`'s `Fn` bound can't hold.
fn replace_in(expr: &mut Expr, f: &mut impl FnMut(&Expr) -> Option<Expr>) {
    expr.node.for_each_child_mut(&mut |c| replace_in(c, f));
    if let Some(replacement) = f(expr) {
        *expr = replacement;
    }
}
