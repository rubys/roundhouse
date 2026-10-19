//! Receiver explosion for a traced instance `extend`.
//!
//! A statement list gains a local for each `extend` whose receiver is
//! one `.new` plus setter calls. The `extend` stays a statement, which
//! is where Spinel flips that object's class.

use std::collections::HashSet;

use super::{Factory, const_name, plain_new_class, statements, var_is, var_name};
use crate::dialect::Param;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol, VarId};

/// Statements of one scope — the test method, or a block body — gain a
/// local for each `extend` whose receiver explodes to a single `.new`.
pub(super) fn rewrite_scope(
    body: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
) {
    let span = body.span;
    let mut exprs = if let ExprNode::Seq { exprs } = &mut *body.node {
        std::mem::take(exprs)
    } else {
        vec![body.clone()]
    };
    let total = exprs.len();
    let mut seen = 0usize;
    let mut i = 0;
    while i < exprs.len() {
        // The last statement's value is the scope's value. `map { one.extend(mod) }`
        // has to answer the object, and an `extend` expression does not.
        let value_used = seen + 1 == total;
        seen += 1;
        rewrite_lambdas(&mut exprs[i], classes, modules, helpers, seq);
        let setups = pull_setups(&mut exprs[i], classes, modules, helpers, seq, value_used);
        let n = setups.len();
        if n > 0 {
            for (k, setup) in setups.into_iter().enumerate() {
                exprs.insert(i + k, setup);
            }
            i += n;
        }
        i += 1;
    }
    *body = if exprs.len() == 1 {
        exprs.pop().unwrap()
    } else {
        Expr::new(span, ExprNode::Seq { exprs })
    };
}

fn rewrite_lambdas(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
) {
    // Acceptance rejects a delayed site and a site under `if` / `&&` /
    // `case`. Those subtrees are not rewritten. An immediate block
    // (`map`, `tap`) that is not under a gate still is.
    if super::gates_control(expr) {
        return;
    }
    let skip_block = match &*expr.node {
        ExprNode::Lambda { .. } => return,
        ExprNode::Send {
            recv,
            method,
            block: Some(_),
            ..
        } => super::region::block_runs_later(recv.as_ref(), method.as_str()),
        _ => false,
    };
    if let ExprNode::Send {
        recv, args, block, ..
    } = &mut *expr.node
    {
        if let Some(recv) = recv {
            rewrite_lambdas(recv, classes, modules, helpers, seq);
        }
        for arg in args {
            rewrite_lambdas(arg, classes, modules, helpers, seq);
        }
        if !skip_block {
            if let Some(block) = block {
                rewrite_block(block, classes, modules, helpers, seq);
            }
        }
        return;
    }
    expr.node
        .for_each_child_mut(&mut |c| rewrite_lambdas(c, classes, modules, helpers, seq));
}

fn rewrite_block(
    block: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
) {
    if let ExprNode::Lambda { body, .. } = &mut *block.node {
        rewrite_scope(body, classes, modules, helpers, seq);
        return;
    }
    rewrite_lambdas(block, classes, modules, helpers, seq);
}

fn pull_setups(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
) -> Vec<Expr> {
    // Hoisting in front of this statement would run the extend before
    // an effect that Ruby evaluates first (`foo(bar(), obj.extend(mod))`).
    if super::region::effect_before_extend(expr) {
        return Vec::new();
    }
    let mut out = Vec::new();
    pull_one(expr, classes, modules, helpers, seq, value_used, &mut out);
    out
}

fn pull_one(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
    out: &mut Vec<Expr>,
) {
    if matches!(&*expr.node, ExprNode::Lambda { .. }) || super::gates_control(expr) {
        return;
    }
    if let Some(setups) = explode_extend(expr, classes, modules, helpers, seq, value_used) {
        out.extend(setups);
    }
    expr.node.for_each_child_mut(&mut |c| {
        pull_one(c, classes, modules, helpers, seq, true, out);
    });
}

struct Built {
    new_call: Expr,
    sets: Vec<Expr>,
}

/// Replace `extend`'s receiver with a fresh local and return the
/// statements that bind it, when the receiver is one `.new` plus
/// setter calls on that object.
///
/// Spinel flips the object's class at the `extend` *statement* and the
/// expression itself does not answer the object. A use of the value
/// (`request(obj.extend(mod))`, `obj.extend(mod).ping`) becomes the
/// statement followed by the local.
fn explode_extend(
    expr: &mut Expr,
    classes: &HashSet<String>,
    modules: &HashSet<String>,
    helpers: &[Factory],
    seq: &mut u32,
    value_used: bool,
) -> Option<Vec<Expr>> {
    let span = expr.span;
    let extend_of_known_module = matches!(&*expr.node, ExprNode::Send { recv: Some(_), method, args, .. }
        if method.as_str() == "extend" && args.len() == 1 && const_name(&args[0]).is_some_and(|n| modules.contains(&n)));
    if !extend_of_known_module {
        return None;
    }
    let receiver_is_local =
        matches!(&*expr.node, ExprNode::Send { recv: Some(recv), .. } if var_name(recv).is_some());
    if receiver_is_local {
        if !value_used {
            return None;
        }
        let stmt = expr.clone();
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &*stmt.node
        else {
            return None;
        };
        let local_expr = recv.clone();
        *expr = local_expr;
        return Some(vec![stmt]);
    }
    let built = {
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &*expr.node
        else {
            return None;
        };
        explode_receiver(recv, helpers, classes, modules)?
    };
    let class_name = plain_new_class(&built.new_call)?;
    if !classes.contains(&class_name) {
        return None;
    }
    *seq += 1;
    let local = Symbol::from(format!("__rh_extend_object_{seq}"));
    let mut setups = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: local.clone(),
            },
            value: built.new_call,
        },
    )];
    let local_expr = Expr::new(
        span,
        ExprNode::Var {
            id: VarId(0),
            name: local.clone(),
        },
    );
    for mut set in built.sets {
        subst_name(&mut set, &receiver_placeholder(), &local_expr);
        setups.push(set);
    }
    {
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &mut *expr.node
        else {
            return None;
        };
        *recv = local_expr.clone();
    }
    if value_used {
        let stmt = expr.clone();
        *expr = local_expr;
        setups.push(stmt);
    }
    Some(setups)
}

fn explode_receiver(
    recv: &Expr,
    helpers: &[Factory],
    classes: &HashSet<String>,
    modules: &HashSet<String>,
) -> Option<Built> {
    if let Some(built) = explode_expr(recv) {
        return Some(built);
    }
    let ExprNode::Send {
        recv: target,
        method,
        args,
        block: None,
        ..
    } = &*recv.node
    else {
        return None;
    };
    let implicit = match target {
        None => true,
        Some(t) => matches!(&*t.node, ExprNode::SelfRef),
    };
    if !implicit {
        return None;
    }
    let factory = helpers
        .iter()
        .find(|f| f.name.as_str() == method.as_str())?;
    // `yield`, `super`, and a block parameter belong to the helper.
    // The call above has no block; copying either would run them in
    // the test method. A call that does pass a block never matches.
    if factory.yields {
        return None;
    }
    let mut params = factory.params.clone();
    let mut body = value_of(&factory.body).clone();
    // The included module's `Note` and `Box::NOTE` are not the test
    // class's constants of those names. Anchor the body and the
    // defaults before the caller's arguments are spliced in.
    if factory.owner != factory.home {
        let mut absolute = classes.clone();
        absolute.extend(modules.iter().cloned());
        if !anchor_params(&mut params, &factory.owner, &factory.constants, &absolute)
            || !anchor_constants(&mut body, &factory.owner, &factory.constants, &absolute)
        {
            return None;
        }
    }
    let bound = bind_params(&params, args)?;
    // An argument with a call has to run before `Class.new`. Splicing
    // it into the body runs it at the use, which is later, and a second
    // use runs it twice. A literal, constant, or local can move.
    if bound.iter().any(|(_, value)| !pure_binding(value)) {
        return None;
    }
    let mut built = explode_expr(&body)?;
    // One walk, so a value that is a later parameter (`a` bound to the
    // local `b`) is not substituted again when `b` is bound.
    subst_names(&mut built.new_call, &bound);
    for set in &mut built.sets {
        subst_names(set, &bound);
    }
    Some(built)
}

fn explode_expr(expr: &Expr) -> Option<Built> {
    if plain_new_class(expr).is_some() {
        return Some(Built {
            new_call: expr.clone(),
            sets: Vec::new(),
        });
    }
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        block: Some(block),
        ..
    } = &*expr.node
    else {
        return None;
    };
    if method.as_str() != "tap" || !args.is_empty() || plain_new_class(inner).is_none() {
        return None;
    }
    let ExprNode::Lambda {
        params,
        rest_param,
        extra_params,
        body,
        ..
    } = &*block.node
    else {
        return None;
    };
    if params.len() != 1 || rest_param.is_some() || !extra_params.is_empty() {
        return None;
    }
    let param = params[0].clone();
    let sets = setter_sends(body, &param)?;
    let mut built = Built {
        new_call: (*inner).clone(),
        sets,
    };
    let placeholder = receiver_placeholder();
    for set in &mut built.sets {
        subst_name(
            set,
            &param,
            &Expr::new(
                set.span,
                ExprNode::Var {
                    id: VarId(0),
                    name: placeholder.clone(),
                },
            ),
        );
    }
    Some(built)
}

fn receiver_placeholder() -> Symbol {
    Symbol::from("__rh_extend_receiver")
}

fn setter_sends(body: &Expr, param: &Symbol) -> Option<Vec<Expr>> {
    let mut out = Vec::new();
    for stmt in statements(body) {
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*stmt.node
        else {
            return None;
        };
        if !var_is(recv, param) || args.len() != 1 || !is_setter(method.as_str()) {
            return None;
        }
        out.push(stmt.clone());
    }
    Some(out)
}

fn is_setter(name: &str) -> bool {
    name.ends_with('=')
        && !matches!(name, "==" | "!=" | "<=" | ">=" | "===" | "=~" | "[]=")
        && name.len() > 1
}

fn bind_params(params: &[Param], args: &[Expr]) -> Option<Vec<(Symbol, Expr)>> {
    if params
        .iter()
        .any(|p| p.keyword || p.rest || p.forwarding || p.from_keyword || p.from_kwrest)
    {
        return None;
    }
    let mut bound = Vec::new();
    let mut i = 0;
    for p in params {
        if i < args.len() {
            bound.push((p.name.clone(), args[i].clone()));
            i += 1;
        } else if let Some(default) = &p.default {
            let mut default = default.clone();
            // `note = tag` means the earlier parameter, not a local of
            // the method the body is copied into.
            for (name, value) in &bound {
                subst_name(&mut default, name, value);
            }
            bound.push((p.name.clone(), default));
        } else {
            return None;
        }
    }
    if i != args.len() { None } else { Some(bound) }
}

fn pure_binding(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Lit { .. } | ExprNode::Const { .. } | ExprNode::Var { .. } | ExprNode::SelfRef
    )
}

fn value_of(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => value_of(&exprs[0]),
        ExprNode::Return { value } => value_of(value),
        _ => expr,
    }
}

fn subst_names(expr: &mut Expr, bound: &[(Symbol, Expr)]) {
    if let Some((_, value)) = bound.iter().find(|(name, _)| var_is(expr, name)) {
        let span = expr.span;
        *expr = value.clone();
        if expr.span.is_synthetic() {
            expr.span = span;
        }
        return;
    }
    let shadowed = matches!(&*expr.node, ExprNode::Lambda { .. });
    if shadowed {
        let keep: Vec<(Symbol, Expr)> = bound
            .iter()
            .filter(|(name, _)| !lambda_binds(expr, name))
            .cloned()
            .collect();
        if let ExprNode::Lambda {
            extra_params, body, ..
        } = &mut *expr.node
        {
            for param in extra_params.iter_mut() {
                if let Some(default) = &mut param.default {
                    // `|tag, extra = tag|` reads the earlier block
                    // parameter, not the helper argument of the same name.
                    subst_names(default, &keep);
                }
            }
            subst_names(body, &keep);
        }
        return;
    }
    expr.node.for_each_child_mut(&mut |c| subst_names(c, bound));
}

fn subst_name(expr: &mut Expr, name: &Symbol, replacement: &Expr) {
    if var_is(expr, name) {
        let span = expr.span;
        *expr = replacement.clone();
        if expr.span.is_synthetic() {
            expr.span = span;
        }
        return;
    }
    // `|tag|` inside the helper body is a different variable. A default
    // is evaluated outside the block, so it still sees the outer name.
    if lambda_binds(expr, name) {
        if let ExprNode::Lambda { extra_params, .. } = &mut *expr.node {
            for param in extra_params {
                if let Some(default) = &mut param.default {
                    subst_name(default, name, replacement);
                }
            }
        }
        return;
    }
    expr.node
        .for_each_child_mut(&mut |c| subst_name(c, name, replacement));
}

fn lambda_binds(expr: &Expr, name: &Symbol) -> bool {
    let ExprNode::Lambda {
        params,
        rest_param,
        extra_params,
        block_param,
        ..
    } = &*expr.node
    else {
        return false;
    };
    params.iter().any(|p| p == name)
        || rest_param.as_ref() == Some(name)
        || block_param.as_ref() == Some(name)
        || extra_params.iter().any(|p| &p.name == name)
}

/// A constant copied out of `owner` is anchored to that class, or to a
/// known top-level class or module. Anything else refuses the copy:
/// emitting the bare name would bind a different constant.
pub(super) fn anchor_constants(
    expr: &mut Expr,
    owner: &ClassId,
    constants: &[Symbol],
    absolute: &HashSet<String>,
) -> bool {
    let mut ok = true;
    anchor(expr, owner, constants, absolute, &mut ok);
    ok
}

fn anchor_params(
    params: &mut [Param],
    owner: &ClassId,
    constants: &[Symbol],
    absolute: &HashSet<String>,
) -> bool {
    for param in params {
        if let Some(default) = &mut param.default {
            if !anchor_constants(default, owner, constants, absolute) {
                return false;
            }
        }
    }
    true
}

fn anchor(
    expr: &mut Expr,
    owner: &ClassId,
    constants: &[Symbol],
    absolute: &HashSet<String>,
    ok: &mut bool,
) {
    if !*ok {
        return;
    }
    if let ExprNode::Const { path } = &mut *expr.node {
        if path.first().is_some_and(|seg| !seg.as_str().is_empty()) {
            let head = path[0].clone();
            if constants.iter().any(|c| c == &head) {
                let mut full = const_prefix(owner);
                full.extend(path.iter().cloned());
                *path = full;
            } else if absolute.contains(head.as_str()) {
                let mut full = vec![Symbol::from("")];
                full.extend(path.iter().cloned());
                *path = full;
            } else {
                *ok = false;
                return;
            }
        }
    }
    expr.node
        .for_each_child_mut(&mut |c| anchor(c, owner, constants, absolute, ok));
}

fn const_prefix(owner: &ClassId) -> Vec<Symbol> {
    owner
        .0
        .as_str()
        .split("::")
        .filter(|seg| !seg.is_empty())
        .map(Symbol::from)
        .collect()
}
