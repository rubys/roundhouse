//! ActiveJob class-side call idiom: `NotifyCommentJob.perform_later(c)`
//! enqueues; under the transpiled runtime the adapter is `:inline` (the
//! solid_queue-era starting point — no queue daemon in-process), so the
//! class-side entry runs the job synchronously:
//!
//!   def self.perform_later(comment)
//!     new.perform(comment)
//!   end
//!
//! `perform_now` gets the identical wrapper (its Rails semantics is
//! already synchronous). `SendWebmentionJob.set(wait: 5.minutes)`
//! returns a scheduling proxy in Rails; inline semantics has nothing
//! to defer, so `Job.set(…).perform_later(args)` folds to
//! `Job.perform_later(args)` at the call site, the dropped options
//! ledgered as residue (see `fold_set_chains`).
//!
//! Job classes are those whose parent chain (within the ingested set)
//! reaches ActiveJob::Base. Same guards as the mailer twin
//! (`mailer_class_side`): positional forwarding only — kwarg or block
//! `perform`s stay unwrapped on the residue ledger.
//!
//! Structural pass on `app.library_classes` (where `app/jobs/*.rb`
//! ingest); runs on the post-analyze hook so every target's tree
//! carries the wrappers.

use std::collections::BTreeSet;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_job_class_side(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Transitive parent-chain closure (ApplicationJob names
    // ActiveJob::Base; concrete jobs name ApplicationJob).
    let mut jobs: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut changed = false;
        for lc in app.library_classes.iter() {
            let name = lc.name.0.as_str();
            if jobs.contains(name) {
                continue;
            }
            if let Some(p) = &lc.parent {
                let ps = p.0.as_str();
                if ps == "ActiveJob::Base" || jobs.contains(ps) {
                    jobs.insert(name.to_string());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    if jobs.is_empty() {
        return diags;
    }

    // `Job.set(wait:/queue:/priority:).perform_later(args)` folds to
    // `Job.perform_later(args)` at the call site. Inline semantics has
    // nothing to schedule, so the options were always dropped; folding
    // drops them HERE (ledgered per site) instead of routing through a
    // class-side `set` answering the class object — which no target's
    // types can name (the RBS said `-> Job`, an instance, and spinel's
    // C returned an `sp_Class` through it: 13 cc errors on lobsters).
    let mut unfolded: BTreeSet<String> = BTreeSet::new();
    let mut folded: Vec<crate::span::Span> = Vec::new();
    super::for_each_hook_body(app, &mut |body| {
        fold_set_chains(body, &jobs, &mut unfolded, &mut folded)
    });
    for span in folded {
        diags.push(crate::lower::residue_diagnostic(
            "job_class_side",
            "job-set-options",
            span,
            "inline job semantics",
            "`set(wait:/queue:/priority:)` options are dropped under inline job semantics"
                .to_string(),
        ));
    }

    for lc in app.library_classes.iter_mut() {
        if !jobs.contains(lc.name.0.as_str()) {
            continue;
        }
        let class_side: BTreeSet<&str> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == crate::dialect::MethodReceiver::Class)
            .map(|m| m.name.as_str())
            .collect();
        let Some(perform) = lc
            .methods
            .iter()
            .find(|m| {
                m.receiver == crate::dialect::MethodReceiver::Instance
                    && m.name.as_str() == "perform"
            })
            .cloned()
        else {
            continue;
        };
        if perform.params.iter().any(|p| p.keyword) {
            diags.push(residue(&perform, "keyword parameters do not forward positionally"));
            continue;
        }
        if perform.block_param.is_some() {
            diags.push(residue(&perform, "the wrapper cannot forward a block"));
            continue;
        }

        let (param_tys, ret_ty): (Vec<Option<Ty>>, Option<Ty>) = match &perform.signature {
            Some(Ty::Fn { params, ret, .. }) if params.len() == perform.params.len() => (
                params.iter().map(|p| Some(p.ty.clone())).collect(),
                Some((**ret).clone()),
            ),
            _ => (vec![None; perform.params.len()], None),
        };

        let span = perform.body.span;
        let mut wrappers: Vec<crate::dialect::MethodDef> = Vec::new();
        for entry in ["perform_later", "perform_now"] {
            if class_side.contains(entry) {
                continue;
            }
            let args: Vec<Expr> = perform
                .params
                .iter()
                .zip(param_tys.iter())
                .enumerate()
                .map(|(i, (p, ty))| {
                    let mut v = Expr::new(
                        span,
                        ExprNode::Var {
                            id: crate::ident::VarId(i as u32),
                            name: p.name.clone(),
                        },
                    );
                    v.ty = ty.clone();
                    v
                })
                .collect();
            let mut new_call = Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("new"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            new_call.ty = Some(Ty::Class { id: lc.name.clone(), args: vec![] });
            let mut body = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(new_call),
                    method: Symbol::from("perform"),
                    args,
                    block: None,
                    parenthesized: true,
                },
            );
            body.ty = ret_ty.clone();
            // `perform_later` LOGS before dispatching. Rails' test
            // helpers count enqueues, and the inline adapter has no
            // queue to count — so the log is the seam that answers
            // them, and `perform_later` is the only entry that
            // enqueues (`perform_now` runs without one, in Rails as
            // here).
            let body = if entry == "perform_later" {
                let mut record = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const { path: vec![Symbol::from("ActiveJob")] },
                        )),
                        method: Symbol::from("record_performed"),
                        args: vec![{
                            let mut lit = Expr::new(
                                span,
                                ExprNode::Lit {
                                    value: crate::expr::Literal::Str {
                                        value: lc.name.0.as_str().to_string(),
                                    },
                                },
                            );
                            lit.ty = Some(Ty::Str);
                            lit
                        }],
                        block: None,
                        parenthesized: true,
                    },
                );
                record.ty = Some(Ty::Nil);
                // …and THEN ONE OF THREE THINGS, in this order.
                //
                // `ActiveJob.enqueue_only` — the `:test` adapter: record
                // and return. Rails' test environment runs `:test`, not
                // `:inline`, and the difference is load-bearing rather
                // than cosmetic: campfire's `Message` has
                // `after_create_commit -> { room.receive(self) }` whose
                // tail is a `perform_later`, so dispatching would run
                // `Room::MessagePusher` for every message a FIXTURE
                // loads and take the whole suite down in its
                // unresolvable nested join.
                //
                // `ActiveJob.drain_registered` — a real queue: hand the
                // work over and return, which is what Rails' production
                // adapters do and what `perform_later` has always
                // promised. A ZERO-ARGUMENT PROC closing over the
                // arguments is what makes this expressible on a strict
                // target: a job queue is heterogeneous by nature, and
                // `() -> nil` is the one type every entry shares
                // whatever it closed over.
                //
                // Otherwise INLINE, unchanged. A job put on a queue
                // nobody drains is a job silently dropped, so the
                // enqueue arm is taken only when a drain has said it
                // exists — `main.rb` registers one, the emitted test
                // harness does not.
                let active_job = |sp| {
                    Expr::new(sp, ExprNode::Const { path: vec![Symbol::from("ActiveJob")] })
                };
                let mut nil_arm = Expr::new(
                    span,
                    ExprNode::Lit { value: crate::expr::Literal::Nil },
                );
                nil_arm.ty = Some(Ty::Nil);

                // The Proc: `{ new.perform(...); nil }`. The trailing
                // nil is what makes every entry's type the same one —
                // `perform` answers whatever the app wrote.
                let mut block_body = Expr::new(
                    span,
                    ExprNode::Seq { exprs: vec![body.clone(), nil_arm.clone()] },
                );
                block_body.ty = Some(Ty::Nil);
                let block = Expr::new(
                    span,
                    ExprNode::Lambda {
                        params: Vec::new(),
                        rest_param: None,
                        block_param: None,
                        body: block_body,
                        block_style: crate::expr::BlockStyle::Do,
                    },
                );
                // AN ARGUMENT, NOT A BLOCK: `ActiveJob.enqueue` names its
                // parameter `^() -> nil` so the runtime file types
                // fully, and RBS has no syntax for naming a block.
                let mut enqueue = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(active_job(span)),
                        method: Symbol::from("enqueue"),
                        args: vec![block],
                        block: None,
                        parenthesized: true,
                    },
                );
                enqueue.ty = Some(Ty::Nil);

                let mut drained = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(active_job(span)),
                        method: Symbol::from("drain_registered"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    },
                );
                drained.ty = Some(Ty::Bool);
                let mut queued_or_inline = Expr::new(
                    span,
                    ExprNode::If {
                        cond: drained,
                        then_branch: enqueue,
                        else_branch: body,
                    },
                );
                queued_or_inline.ty = Some(Ty::Nil);

                let mut gate = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(active_job(span)),
                        method: Symbol::from("enqueue_only"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    },
                );
                gate.ty = Some(Ty::Bool);
                let mut cond = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(gate),
                        method: Symbol::from("!"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                );
                cond.ty = Some(Ty::Bool);
                let mut guarded = Expr::new(
                    span,
                    ExprNode::If {
                        cond,
                        then_branch: queued_or_inline,
                        else_branch: nil_arm.clone(),
                    },
                );
                guarded.ty = Some(Ty::Nil);
                // The wrapper answers NIL, not the perform's value.
                // Rails' `perform_later` answers the job (or `false`),
                // never the result — nothing in any corpus reads it —
                // and a Nil return is what lets the guarded call sit in
                // statement position on the strict targets instead of
                // forcing a `<perform-return> | nil` union.
                let mut seq =
                    Expr::new(span, ExprNode::Seq { exprs: vec![record, guarded, nil_arm] });
                seq.ty = Some(Ty::Nil);
                seq
            } else {
                body
            };
            let mut w = perform.clone();
            w.name = Symbol::from(entry);
            w.receiver = crate::dialect::MethodReceiver::Class;
            w.body = body;
            wrappers.push(w);
        }

        // `set(options) → self`, only for a job something still calls
        // `set` on OTHER than in the chain folded above (a scheduling
        // proxy kept in a variable, say). Its value is the class object,
        // which `Ty` cannot name, so the signature says `untyped` rather
        // than claim an instance it does not return.
        if !class_side.contains("set") && unfolded.contains(lc.name.0.as_str()) {
            let mut body = Expr::new(span, ExprNode::SelfRef);
            body.ty = Some(Ty::Untyped);
            let mut w = perform.clone();
            w.name = Symbol::from("set");
            w.receiver = crate::dialect::MethodReceiver::Class;
            w.params = vec![crate::dialect::Param::positional(Symbol::from("options"))];
            w.signature = Some(Ty::Fn {
                params: vec![crate::ty::Param {
                    name: Symbol::from("options"),
                    ty: Ty::Untyped,
                    kind: crate::ty::ParamKind::Required,
                }],
                block: None,
                ret: Box::new(Ty::Untyped),
                effects: crate::effect::EffectSet::pure(),
            });
            w.body = body;
            wrappers.push(w);
            diags.push(residue(
                &perform,
                "set(wait:/queue:/priority:) options are dropped under inline job semantics",
            ));
        }

        lc.methods.extend(wrappers);
    }
    diags
}

/// Fold `Job.set(…).perform_later(args)` / `.perform_now(args)` to the
/// class-side call; record every `Job.set` left standing.
fn fold_set_chains(
    e: &mut Expr,
    jobs: &BTreeSet<String>,
    unfolded: &mut BTreeSet<String>,
    folded: &mut Vec<crate::span::Span>,
) {
    let job_of = |r: &Expr| -> Option<String> {
        let ExprNode::Const { path } = &*r.node else { return None };
        let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
        jobs.contains(&name).then_some(name)
    };
    if let ExprNode::Send { recv: Some(inner), method, .. } = &mut *e.node {
        if matches!(method.as_str(), "perform_later" | "perform_now") {
            let job_recv = match &*inner.node {
                ExprNode::Send { recv: Some(j), method: set, block: None, .. }
                    if set.as_str() == "set" && job_of(j).is_some() =>
                {
                    Some(j.clone())
                }
                _ => None,
            };
            if let Some(j) = job_recv {
                folded.push(inner.span);
                *inner = j;
            }
        }
    }
    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
        if method.as_str() == "set" {
            if let Some(name) = job_of(r) {
                unfolded.insert(name);
            }
        }
    }
    e.node.for_each_child_mut(&mut |c| fold_set_chains(c, jobs, unfolded, folded));
}

fn residue(m: &crate::dialect::MethodDef, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "job_class_side",
        "job-class-entry",
        m.body.span,
        reason,
        format!("job_class_side: `{}` — {}", m.name.as_str(), reason),
    )
}
