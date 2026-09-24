//! `ActiveJob.perform_all_later(jobs)` over jobs built by a `map`:
//!
//!   jobs = xs.map { |x| SomeJob.new(a, …).set(queue: :q) }
//!   ActiveJob.perform_all_later(jobs)
//!
//! becomes
//!
//!   xs.each { |x| SomeJob.set(queue: :q).perform_later(a, …) }
//!
//! Rails' bulk enqueue is one `perform_later` per job with the
//! callbacks skipped; the lowered job classes keep no argument list on
//! the instance (`new` takes none, `perform_later(args)` is the
//! class-side entry), so the enqueue has to happen where the arguments
//! are still in hand. The class-side `set` is the same no-op the
//! instance's would be. Enqueue ORDER is kept; what is lost is the
//! batching, which the in-process queue does not have to lose.
//!
//! Only the adjacent-statement shape (and the same with the `map`
//! inline as the argument), with the jobs local read nowhere else:
//! anything that keeps the array around needs the instances.
//! lobsters' PrefillPageCacheJob is the case.

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;

pub fn apply_perform_all_later_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // Inline form: `ActiveJob.perform_all_later(xs.map { … })`.
    if let Some(arg) = perform_all_later_arg(expr) {
        if let Some(each) = map_to_each(arg) {
            *expr = each;
            return;
        }
    }
    let ExprNode::Seq { exprs } = &mut *expr.node else { return };
    let mut i = 0;
    while i + 1 < exprs.len() {
        let replaced = (|| {
            let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*exprs[i].node else {
                return None;
            };
            let arg = perform_all_later_arg(&exprs[i + 1])?;
            if !matches!(&*arg.node, ExprNode::Var { name: n, .. } if n == name) {
                return None;
            }
            let rest_reads = exprs[i + 2..].iter().any(|e| reads_var(e, name));
            if rest_reads {
                return None;
            }
            map_to_each(value)
        })();
        if let Some(each) = replaced {
            exprs[i] = each;
            exprs.remove(i + 1);
        }
        i += 1;
    }
}

fn perform_all_later_arg(e: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    let is_active_job = matches!(&*r.node, ExprNode::Const { path }
        if path.len() == 1 && path[0].as_str() == "ActiveJob");
    (is_active_job && method.as_str() == "perform_all_later" && args.len() == 1).then(|| &args[0])
}

/// `xs.map { |x| J.new(a…)[.set(o)] }` → `xs.each { |x| J[.set(o)].perform_later(a…) }`.
fn map_to_each(value: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(xs), method, args, block: Some(block), parenthesized } =
        &*value.node
    else {
        return None;
    };
    if method.as_str() != "map" || !args.is_empty() {
        return None;
    }
    let ExprNode::Lambda { rest_param, params, block_param, body, block_style } = &*block.node
    else {
        return None;
    };
    // Peel an optional `.set(opts)` off the `J.new(a…)` construction.
    let (ctor, set_args) = match &*body.node {
        ExprNode::Send { recv: Some(inner), method, args, block: None, .. }
            if method.as_str() == "set" =>
        {
            (inner, Some(args.clone()))
        }
        _ => (body, None),
    };
    let ExprNode::Send { recv: Some(job), method: new, args: job_args, block: None, .. } =
        &*ctor.node
    else {
        return None;
    };
    if new.as_str() != "new" || !matches!(&*job.node, ExprNode::Const { .. }) {
        return None;
    }
    let span = body.span;
    let target = match set_args {
        Some(a) => send(job.clone(), "set", a, span),
        None => job.clone(),
    };
    let enqueue = send(target, "perform_later", job_args.clone(), span);
    let lambda = Expr::new(
        block.span,
        ExprNode::Lambda {
            rest_param: rest_param.clone(),
            params: params.clone(),
            block_param: block_param.clone(),
            body: enqueue,
            block_style: *block_style,
        },
    );
    Some(Expr::new(
        value.span,
        ExprNode::Send {
            recv: Some(xs.clone()),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(lambda),
            parenthesized: *parenthesized,
        },
    ))
}

fn send(recv: Expr, method: &str, args: Vec<Expr>, span: crate::span::Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn reads_var(e: &Expr, name: &Symbol) -> bool {
    if matches!(&*e.node, ExprNode::Var { name: n, .. } if n == name) {
        return true;
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found |= reads_var(c, name));
    found
}
