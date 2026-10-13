//! `Model.where(k: v, …).first_or_create` / `.first_or_initialize`
//! grounding: macro-inline the find-else-build at the call site,
//! seeding the built record from the where-clause equality pairs —
//! Rails' contract (lobsters builds `ReadRibbon.where(user:,
//! story:).first_or_create` and reads the ribbon back; a blank-built
//! record would drop the keys). The heterogeneous conditions hash is
//! exactly the shape the macro-inline line says to expand rather than
//! push through a runtime helper: inlined, each pair lands as a typed
//! setter send.
//!
//!   _rec = Model.where(k: v).first
//!   if _rec.nil?
//!     _rec = Model.new
//!     _rec.k = v
//!     _rec.save            # first_or_create only
//!   end
//!   <original consumer of the value>
//!
//! Fires on statement positions (`Seq` elements): a bare call gains a
//! trailing `_rec` read (value-preserving), an `x = …` statement
//! reassigns from `_rec`. Gated on: the receiver chain being
//! `Const.where(HashLit)` with symbol keys and pure-read values (each
//! value is evaluated twice — once querying, once seeding), since the
//! runtime flattens conditions to SQL immediately and can't recover
//! the pairs later. Anything else keeps the runtime
//! `first_or_initialize` (blank-build residue) or fails resolution
//! honestly.
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form.

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue, Literal, Pattern};
use crate::ident::{ClassId, Symbol, VarId};
use crate::ty::Ty;

// A model remains in the map for the unsupported-call diagnostic even
// when its constructor declares an initialization callback not yet lowered.
type ModelColumns = std::collections::HashMap<ClassId, Option<std::collections::HashMap<Symbol, Ty>>>;

pub fn apply_first_or_create_lowering(app: &mut App) -> Vec<crate::diagnostic::Diagnostic> {
    let models = app.models.iter().map(|model| {
        let primary_key = model.primary_key.clone().unwrap_or_else(|| Symbol::from("id"));
        let fields = model.attributes.fields.iter().filter(|(key, _)| **key != primary_key)
            .map(|(key, ty)| (key.clone(), ty.clone())).collect();
        let unsupported_initializer = model.body.iter().any(|item| matches!(item,
            crate::dialect::ModelBodyItem::Unknown { expr, .. }
                if matches!(&*expr.node, ExprNode::Send { recv: None, method, block: None, .. }
                    if method.as_str() == "after_initialize")));
        (model.name.clone(), (!unsupported_initializer).then_some(fields))
    }).collect();
    let mut diagnostics = Vec::new();
    super::for_each_hook_body(app, &mut |body| {
        // A one-statement method body is not a Seq, so the statement
        // walk below never saw it — lobsters'
        // `def find_or_initialize_domain; @domain = Domain
        // .find_or_initialize_by(…); end`. Give it one to stand in.
        if is_claimable_stmt(body) || scoped_statement_model(body, &models).is_some() {
            let stmt = body.clone();
            *body = Expr::new(stmt.span, ExprNode::Seq { exprs: vec![stmt] });
        }
        rewrite(body, &models);
        ledger_scoped_creations(body, &models, &mut diagnostics);
    });
    diagnostics
}

fn is_claimable_stmt(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { .. } => claims(&as_where_first(e)).is_some(),
        ExprNode::Assign { value, .. } => claims(&as_where_first(value)).is_some(),
        _ => false,
    }
}

fn rewrite(expr: &mut Expr, models: &ModelColumns) {
    expr.node.for_each_child_mut(&mut |child| rewrite(child, models));
    let ExprNode::Seq { exprs } = &mut *expr.node else { return };
    for e in exprs {
        if let Some(model) = scoped_statement_model(e, models) {
            let (send, target) = match &*e.node {
                ExprNode::Assign { target, value } => (value, Some(target.clone())),
                _ => (&*e, None),
            };
            let mut statements = inline_scoped_creation(send, model);
            if let Some(target) = target {
                let record = statements.pop().expect("record result");
                statements.push(Expr::new(e.span, ExprNode::Assign { target, value: record }));
            }
            *e.node = ExprNode::Seq { exprs: statements };
            e.ty = None;
            continue;
        }
        // A bare call keeps its value via a trailing `_rec` read; an
        // assign statement rebuilds as inline + `target = _rec` (the
        // whole STATEMENT becomes the Seq — a Seq must never land in
        // the assign's value slot, the emitter renders that broken).
        let (save, reassign, send_expr) = match &*e.node {
            ExprNode::Send { .. } => {
                let e = as_where_first(e);
                match claims(&e) {
                    Some(save) => (save, None, e),
                    None => continue,
                }
            }
            ExprNode::Assign { target, value } => {
                let value = as_where_first(value);
                match claims(&value) {
                    Some(save) => (save, Some(target.clone()), value),
                    None => continue,
                }
            }
            _ => continue,
        };
        let mut stmts = inline(&send_expr, save);
        let span = send_expr.span;
        let rec_read = Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_rec") });
        stmts.push(match reassign {
            None => rec_read,
            Some(target) => Expr::new(span, ExprNode::Assign { target, value: rec_read }),
        });
        *e.node = ExprNode::Seq { exprs: stmts };
        e.ty = None;
    }
}

/// `Model.find_or_initialize_by(h)` IS `Model.where(h).first_or_initialize`
/// (and `find_or_create_by` the `_create` twin) — Rails defines them
/// that way. Restated in that shape so one inline serves all four;
/// anything else comes back unchanged. Const receivers only: an
/// association receiver keeps the runtime `Relation#find_or_create_by`,
/// whose scope merge is the point (campfire's `user.searches`).
fn as_where_first(e: &Expr) -> Expr {
    let ExprNode::Send { recv: Some(model), method, args, block: None, .. } = &*e.node else {
        return e.clone();
    };
    let first_or = match method.as_str() {
        "find_or_initialize_by" => "first_or_initialize",
        "find_or_create_by" => "first_or_create",
        _ => return e.clone(),
    };
    if args.len() != 1 || !matches!(&*model.node, ExprNode::Const { .. }) {
        return e.clone();
    }
    let mut where_send = Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(model.clone()),
            method: Symbol::from("where"),
            args: args.clone(),
            block: None,
            parenthesized: true,
        },
    );
    if let ExprNode::Const { path } = &*model.node {
        where_send.ty = Some(Ty::Relation {
            of: ClassId(Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
            )),
        });
    }
    Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(where_send),
            method: Symbol::from(first_or),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// A condition value is evaluated twice (query, then seed), so it must
/// be a pure read — or a literal-keyed index on one, the shape a
/// controller's `params[:id]` takes.
fn seedable(v: &Expr) -> bool {
    if super::case_lambda::is_pure_read(v) {
        return true;
    }
    matches!(
        &*v.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "[]"
                && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Lit { .. })
                && super::case_lambda::is_pure_read(r)
    )
}

/// Does this Send match the claimable shape? Returns `Some(save)` —
/// whether the built record saves (`first_or_create`) or stays
/// unsaved (`first_or_initialize`).
fn claims(e: &Expr) -> Option<bool> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    let save = match method.as_str() {
        "first_or_create" => true,
        "first_or_initialize" => false,
        _ => return None,
    };
    if !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(model), method: wm, args: wargs, block: None, .. } = &*r.node
    else {
        return None;
    };
    if wm.as_str() != "where" || wargs.len() != 1 || !matches!(&*model.node, ExprNode::Const { .. })
    {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*wargs[0].node else { return None };
    let ok = !entries.is_empty()
        && entries.iter().all(|(k, v)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } })
                && seedable(v)
        });
    ok.then_some(save)
}

fn inline(e: &Expr, save: bool) -> Vec<Expr> {
    let span = e.span;
    let ExprNode::Send { recv: Some(r), .. } = &*e.node else { unreachable!() };
    let ExprNode::Send { recv: Some(model), args: wargs, .. } = &*r.node else { unreachable!() };
    let ExprNode::Hash { entries, .. } = &*wargs[0].node else { unreachable!() };

    // This runs after analysis, so the nodes it builds are typed here
    // or not at all — and an untyped send on a typed receiver is what
    // the diagnostics walk reports as a dispatch failure (`no known
    // method \`new\` on ReadRibbon`). The model is known by construction.
    let ExprNode::Const { path } = &*model.node else { unreachable!() };
    let record = Ty::Class {
        id: ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        )),
        args: vec![].into(),
    };
    let typed = |mut e: Expr, ty: Ty| {
        e.ty = Some(ty);
        e
    };
    let rec = |()| {
        typed(Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_rec") }), record.clone())
    };
    let send = |recv: Expr, m: &str| {
        let ty = match m {
            "first" => Ty::Union { variants: vec![record.clone(), Ty::Nil].into() },
            "new" => record.clone(),
            _ => Ty::Bool,
        };
        typed(
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(recv),
                    method: Symbol::from(m),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            ty,
        )
    };

    let find = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_rec") },
            value: send(r.clone(), "first"),
        },
    );

    let mut build = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_rec") },
            value: send(model.clone(), "new"),
        },
    )];
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            unreachable!()
        };
        build.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: rec(()), name: name.clone() },
                value: v.clone(),
            },
        ));
    }
    if save {
        build.push(send(rec(()), "save"));
    }

    let guard = Expr::new(
        span,
        ExprNode::If {
            cond: send(rec(()), "nil?"),
            then_branch: Expr::new(span, ExprNode::Seq { exprs: build }),
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    vec![find, guard]
}

/// Find-or-create blocks follow the same concrete call-site rule as
/// `create_block`: a generic inherited yield would erase the model's type.
/// Query once with literal conditions, initialize only the miss branch,
/// and return the record independently of the block's result.
fn scoped_creation_model(e: &Expr, models: &ModelColumns) -> Option<ClassId> {
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*e.node else {
        return None;
    };
    if !matches!(method.as_str(), "find_or_create_by" | "find_or_create_by!")
        || args.len() != 1 || scalar_pairs(&args[0]).is_none()
        || !match block.as_ref().map(|block| &*block.node) {
            None => true,
            Some(ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. }) =>
                params.len() == 1 && extra_params.is_empty() && !has_block_control_flow(body) && !has_other_local_assignment(body, &params[0]),
            _ => false,
        }
    {
        return None;
    }
    let (id, scope) = scope_literals(recv)?;
    let columns = models.get(&id)?.as_ref()?;
    scope.iter().chain(scalar_pairs(&args[0])?.iter())
        .all(|(key, value)| columns.get(key).is_some_and(|ty| literal_fits(value, ty))).then_some(id)
}

fn scalar_pairs(hash: &Expr) -> Option<Vec<(Symbol, Expr)>> {
    let ExprNode::Hash { entries, .. } = &*hash.node else { return None; };
    entries.iter().map(|(key, value)| {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*key.node else { return None; };
        matches!(&*value.node, ExprNode::Lit { value: Literal::Nil | Literal::Bool { .. }
            | Literal::Int { .. } | Literal::Float { .. } | Literal::Str { .. } })
            .then(|| (key.clone(), value.clone()))
    }).collect()
}

// Only the syntactic scalar-equality subset is claimed here. A general
// relation loses its predicates when materialized, and a named scope or
// association needs scope metadata this pass does not have.
fn scope_literals(expr: &Expr) -> Option<(ClassId, Vec<(Symbol, Expr)>)> {
    match &*expr.node {
        ExprNode::Const { path } => Some((ClassId(Symbol::from(
            path.iter().map(|part| part.as_str()).collect::<Vec<_>>().join("::"))), vec![])),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "where" && args.len() == 1 => {
            let (model, mut pairs) = scope_literals(recv)?;
            pairs.extend(scalar_pairs(&args[0])?);
            Some((model, pairs))
        }
        _ => None,
    }
}

fn has_model_root(e: &Expr, models: &ModelColumns) -> bool {
    match &*e.node {
        ExprNode::Const { path } => models.contains_key(&ClassId(Symbol::from(
            path.iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("::"),
        ))),
        ExprNode::Send { recv: Some(recv), method, .. }
            if matches!(method.as_str(), "where" | "not" | "order" | "limit" | "offset" | "includes" | "preload" | "distinct") => has_model_root(recv, models),
        _ => false,
    }
}

fn inline_scoped_creation(e: &Expr, model_id: ClassId) -> Vec<Expr> {
    let span = e.span;
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*e.node else { unreachable!() };
    let record_ty = Ty::Class { id: model_id.clone(), args: vec![].into() };
    let relation_ty = Ty::Relation { of: model_id.clone() };
    let typed = |mut value: Expr, ty: Ty| { value.ty = Some(ty); value };
    let record = || typed(Expr::new(span, ExprNode::Var {
        id: VarId(0), name: Symbol::from(format!("_find_or_create_record_{}", span.start)),
    }), record_ty.clone());
    let assign = |target: Expr, value: Expr| {
        let ExprNode::Var { id, name } = *target.node else { unreachable!() };
        Expr::new(span, ExprNode::Assign { target: LValue::Var { id, name }, value })
    };
    let send = |receiver: Expr, name: &str, args: Vec<Expr>, ty: Ty| typed(Expr::new(span,
        ExprNode::Send { recv: Some(receiver), method: Symbol::from(name), args,
            block: None, parenthesized: true }), ty);
    let model = typed(Expr::new(span, ExprNode::Const {
        path: model_id.0.as_str().split("::").map(Symbol::from).collect(),
    }), record_ty.clone());
    let seed = typed(Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Const {
            path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")],
        })), method: Symbol::from("new"), args: vec![model.clone()],
        block: None, parenthesized: true,
    }), relation_ty.clone());
    let mut relation = recv.clone();
    seed_model_root(&mut relation, &model_id, &seed, &relation_ty);
    let find = send(relation, "find_by", args.clone(),
        Ty::Union { variants: vec![record_ty.clone(), Ty::Nil].into() });
    let (_, scope_pairs) = scope_literals(recv).expect("admitted literal scope");
    let mut pairs = Vec::new();
    for (key, value) in scope_pairs.into_iter().chain(scalar_pairs(&args[0]).expect("admitted literal conditions")) {
        pairs.retain(|(existing, _)| existing != &key);
        pairs.push((key, value));
    }
    let value_ty = pairs.iter().filter_map(|(_, value)| value.ty.clone()).fold(Ty::Bottom, crate::analyze::union_of);
    let attributes = typed(Expr::new(span, ExprNode::Hash {
        entries: pairs.into_iter().map(|(key, value)| {
            (typed(Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: key } }), Ty::Sym), value)
        }).collect(), kwargs: false,
    }), Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(value_ty) });
    let mut initialize = vec![assign(record(), send(model, "new", vec![attributes], record_ty.clone()))];
    if let Some(block) = block {
        let ExprNode::Lambda { extra_params, params, body, .. } = &*block.node else { unreachable!() };
        if !extra_params.is_empty() {
            unreachable!()
        }
        let block_id = super::create_block::find_var_id(body, &params[0]).unwrap_or(VarId(0));
        let block_name = Symbol::from(format!("_find_or_create_block_record_{}", span.start));
        let block_var = typed(Expr::new(span, ExprNode::Var { id: block_id, name: block_name.clone() }), record_ty.clone());
        let mut block_body = body.clone();
        rename_block_param(&mut block_body, block_id, &params[0], &block_name);
        initialize.push(assign(block_var, record()));
        initialize.push(block_body);
    }
    initialize.push(send(record(), if method.as_str().ends_with('!') { "save!" } else { "save" }, vec![], Ty::Bool));
    let branch = Expr::new(span, ExprNode::If {
        cond: send(record(), "nil?", vec![], Ty::Bool),
        then_branch: Expr::new(span, ExprNode::Seq { exprs: initialize }),
        else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
    });
    vec![assign(record(), find), branch, record()]
}

fn scoped_statement_model(
    e: &Expr,
    models: &ModelColumns,
) -> Option<ClassId> {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { .. } | LValue::Ivar { .. }, value } => scoped_creation_model(value, models),
        ExprNode::Assign { .. } => None,
        _ => scoped_creation_model(e, models),
    }
}

fn ledger_scoped_creations(e: &Expr, models: &ModelColumns, diagnostics: &mut Vec<crate::diagnostic::Diagnostic>) {
    if let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node {
        if matches!(method.as_str(), "find_or_create_by" | "find_or_create_by!")
            && (block.is_some() || method.as_str() == "find_or_create_by!" || has_model_root(recv, models))
            && (has_model_root(recv, models)
                || matches!(recv.ty.as_ref(), Some(Ty::Relation { of }) if models.contains_key(of))
                || matches!(recv.ty.as_ref(), Some(Ty::Array { elem }) if matches!(&**elem, Ty::Class { id, .. } if models.contains_key(id))))
        {
            // Name the cause: the ledger entry is the unsupported list,
            // and the three gaps close in different places.
            let cause = if block.is_some() {
                "an initialization block is inlined only on a statement-position call with scalar literal conditions, and only without control flow or new locals in its body"
            } else if method.as_str() == "find_or_create_by!" {
                "the runtime has no `find_or_create_by!`; only a statement-position call with scalar literal conditions is inlined"
            } else {
                "a `where` scope materializes before the call; only scalar literal predicates and conditions on a concrete model are inlined"
            };
            let mut diagnostic = super::residue_diagnostic("first_or_create", "scoped-find-or-create", e.span,
                "requires statement position, scalar literal Hash conditions, and a concrete model with literal where predicates",
                format!("scoped find-or-create cannot be lowered safely: {cause}"));
            diagnostic.severity = crate::diagnostic::Severity::Error;
            diagnostics.push(diagnostic);
        }
    }
    e.node.for_each_child(&mut |child| ledger_scoped_creations(child, models, diagnostics));
}

fn rename_block_param(expr: &mut Expr, id: VarId, name: &Symbol, replacement: &Symbol) {
    match &mut *expr.node {
        ExprNode::Var {
            id: var_id,
            name: var_name,
        } if *var_id == id && var_name == name => {
            *var_name = replacement.clone();
        }
        ExprNode::Assign {
            target:
                LValue::Var {
                    id: var_id,
                    name: var_name,
                },
            ..
        } if *var_id == id && var_name == name => {
            *var_name = replacement.clone();
        }
        _ => {}
    }
    expr.node
        .for_each_child_mut(&mut |child| rename_block_param(child, id, name, replacement));
}

fn has_block_control_flow(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Break { .. } | ExprNode::Next { .. } | ExprNode::Return { .. }
        | ExprNode::Redo | ExprNode::Retry | ExprNode::OpAssign { .. } | ExprNode::MultiAssign { .. }
        | ExprNode::Let { .. } | ExprNode::Lambda { .. }) {
        return true;
    }
    if matches!(&*expr.node, ExprNode::BeginRescue { rescues, .. } if rescues.iter().any(|clause| clause.binding.is_some()))
        || matches!(&*expr.node, ExprNode::Case { arms, .. } if arms.iter().any(|arm| pattern_has_binding(&arm.pattern))) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| { found |= has_block_control_flow(child); });
    found
}

// Keep the receiver lazy. A standalone model `where` otherwise emits an
// eagerly loaded Array, which cannot provide create defaults or find_by.
fn seed_model_root(expr: &mut Expr, model: &ClassId, seed: &Expr, relation_ty: &Ty) {
    match &mut *expr.node {
        ExprNode::Const { path } if path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::") == model.0.as_str() => {
            *expr = seed.clone();
        }
        ExprNode::Send { recv: Some(recv), .. } => {
            seed_model_root(recv, model, seed, relation_ty);
            expr.ty = Some(relation_ty.clone());
        }
        _ => {}
    }
}

fn literal_fits(value: &Expr, ty: &Ty) -> bool {
    let ExprNode::Lit { value } = &*value.node else { return false; };
    literal_fits_node(value, ty)
}

fn literal_fits_node(value: &Literal, ty: &Ty) -> bool {
    match (value, ty) {
        (Literal::Nil, Ty::Nil) | (Literal::Bool { .. }, Ty::Bool)
        | (Literal::Int { .. }, Ty::Int) | (Literal::Float { .. }, Ty::Float)
        | (Literal::Str { .. }, Ty::Str) => true,
        (_, Ty::Union { variants }) => variants.iter().any(|ty| literal_fits_node(value, ty)),
        _ => false,
    }
}


// Inlining must not turn a block-local assignment into a method local,
// which can capture a later receiverless method call of the same name.
fn has_other_local_assignment(expr: &Expr, block_param: &Symbol) -> bool {
    if matches!(&*expr.node, ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name != block_param) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| { found |= has_other_local_assignment(child, block_param); });
    found
}

// Pattern and rescue binding names are not Expr children, so the
// recursive expression walk cannot preserve their lexical bindings.
fn pattern_has_binding(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Bind { .. } => true,
        Pattern::Array { elems, rest } => rest.is_some() || elems.iter().any(pattern_has_binding),
        Pattern::Record { fields, .. } => fields.iter().any(|(_, value)| pattern_has_binding(value)),
        _ => false,
    }
}
