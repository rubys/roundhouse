//! ActiveJob payloads on the Ruby-family lanes: `perform_later` puts an
//! ActiveJob-format JSON payload on the queue instead of a Proc, when
//! every argument of the job can be written in ActiveJob's wire format.
//!
//! Three halves, in three places:
//!
//! - [`plan_jobs`] works out a [`JobPlan`] per job from the job's
//!   `perform_later` / `perform_now` CALL SITES, unioned per position,
//!   and records it on `App::job_plans`. Call sites and not `perform`'s
//!   own signature, because nothing calls `perform` directly before
//!   `job_class_side` writes the wrappers, so its parameters are untyped
//!   in every corpus app; the sites are typed (Switchyard spikes/t1).
//!   Target-neutral data, run from `job_class_side` on every target.
//! - [`apply_ruby`] rewrites the enqueue arm of each planned job's
//!   `perform_later` from `ActiveJob.enqueue(-> { … })` to
//!   `ActiveJob.enqueue_payload(JobRegistry.payload_<job>(args))` and
//!   ledgers each job that keeps its Proc; [`push_model_to_gid_uri`]
//!   gives every model the GlobalID a payload writes. Called by the Ruby
//!   emitter only: the other targets have no drain, so their enqueue arm
//!   never runs.
//! - [`registry_source`] writes `JobRegistry`: per job, the payload
//!   writer and a `case` arm that reads the arguments back and calls
//!   `perform`; per model, the record locators. `project.rs` puts it
//!   between the markers in `runtime/job_registry.rb`.
//!
//! The hold arm (the `:test` adapter) and the inline arm are untouched,
//! so emitted test suites behave as before.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// How one argument is written to the payload and read back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Codec {
    Bool,
    Int,
    Float,
    Str,
    Sym,
    Time,
    /// A record, found on `model` (the STI base) and accepted as any
    /// name in `accepts` (the base and its STI subclasses).
    Record { model: String, accepts: Vec<String> },
    /// A record or nil.
    NullableRecord { model: String, accepts: Vec<String> },
    /// An Array of one of the codecs above (not nullable, not nested).
    Array(Box<Codec>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobParam {
    pub name: Symbol,
    /// `*name`: every argument from this position on, each written as
    /// its own element of the argument list, as Rails flattens a splat.
    pub rest: bool,
    pub codec: Codec,
}

/// Why a job keeps its Proc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobFallback {
    pub param: Option<Symbol>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobPlan {
    /// `Room::PushMessageJob`.
    pub job: String,
    /// `queue_as :name`, or "default".
    pub queue: String,
    /// `discard_on ActiveJob::DeserializationError`, declared on the job
    /// or inherited from a job ancestor.
    pub discards_deserialization_error: bool,
    /// `self.enqueue_after_transaction_commit = true`, on the job or a
    /// job ancestor. Rails 8.1 defaults it to false; Lobsters'
    /// `ApplicationJob` sets it. The payload is then held until the
    /// open transaction commits, and dropped if it rolls back.
    #[serde(default)]
    pub enqueue_after_commit: bool,
    pub params: Vec<JobParam>,
    /// Set when the job keeps its Proc; `params` is then empty.
    pub fallback: Option<JobFallback>,
}

impl JobPlan {
    pub fn is_payload(&self) -> bool {
        self.fallback.is_none()
    }

    /// `room__push_message_job`, the suffix of this job's generated
    /// `JobRegistry.payload_<suffix>`.
    pub fn suffix(&self) -> String {
        suffix(&self.job)
    }
}

fn suffix(class_name: &str) -> String {
    crate::naming::underscore(class_name).replace('/', "__")
}

// ---- Planning --------------------------------------------------------

/// A plan for every job in `wrapped` (the jobs `job_class_side` gave a
/// synthesized `perform_later`), in name order.
pub fn plan_jobs(app: &App, jobs: &BTreeSet<String>, wrapped: &BTreeSet<String>) -> Vec<JobPlan> {
    let sites = call_site_types(app, jobs);
    let unattributed = unattributed_jobs(app, jobs);
    let sti = crate::lower::sti_scope::sti_bases(app);
    let mut plans = Vec::new();
    for name in wrapped {
        let Some(lc) = app.library_classes.iter().find(|lc| lc.name.0.as_str() == name) else {
            continue;
        };
        let Some(perform) = lc.methods.iter().find(|m| {
            m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() == "perform"
        }) else {
            continue;
        };
        let queue = queue_name(app, lc);
        let discards = discards_deserialization_error(app, lc);
        let empty = Vec::new();
        let job_sites = sites.get(name).unwrap_or(&empty);
        let planned = if unattributed.contains(name) || unattributed.contains("*") {
            Err(JobFallback {
                param: None,
                reason: "a `perform_later` call doesn't name the job class, so its argument types are unknown"
                    .to_string(),
            })
        } else {
            plan_params(app, &sti, &perform.params, job_sites)
        };
        let (params, fallback) = match planned {
            Ok(params) => (params, None),
            Err(f) => (Vec::new(), Some(f)),
        };
        plans.push(JobPlan {
            job: name.clone(),
            queue,
            discards_deserialization_error: discards,
            enqueue_after_commit: enqueues_after_commit(app, lc),
            params,
            fallback,
        });
    }
    plans
}

/// Each site is the argument types, in order; `None` for an argument
/// whose type is unknown or which is a splat or a keyword hash.
type Site = Vec<Option<Ty>>;

fn call_site_types(app: &App, jobs: &BTreeSet<String>) -> BTreeMap<String, Vec<Site>> {
    let mut out: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        collect_sites(body, jobs, &mut out);
    });
    out
}

/// Jobs with a `perform_later` call that doesn't name its class: a bare
/// `perform_later` or `self.perform_later` in a job's own methods,
/// `self.class.perform_later`, or a class held in a variable. Such a
/// call can pass any type, so the job keeps its Proc rather than risk a
/// codec that doesn't fit. `"*"` means a call that could be any job.
fn unattributed_jobs(app: &App, jobs: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // In a job's own methods, an implicit or `self` receiver is the job
    // (or a subclass of it).
    for lc in &app.library_classes {
        if !jobs.contains(lc.name.0.as_str()) {
            continue;
        }
        for m in &lc.methods {
            let mut hit = false;
            find_self_enqueues(&m.body, &mut hit);
            if hit {
                out.insert(lc.name.0.as_str().to_string());
            }
        }
    }
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        find_dynamic_enqueues(body, app, jobs, &mut out);
    });
    // A subclass inherits whatever reaches its parent through `self`.
    let named: Vec<String> = out.iter().cloned().collect();
    for lc in &app.library_classes {
        if jobs.contains(lc.name.0.as_str())
            && ancestry(app, lc).iter().any(|a| named.iter().any(|n| n == a.name.0.as_str()))
        {
            out.insert(lc.name.0.as_str().to_string());
        }
    }
    out
}

fn is_enqueue(method: &Symbol) -> bool {
    method.as_str() == "perform_later"
}

fn find_self_enqueues(e: &Expr, hit: &mut bool) {
    if let ExprNode::Send { recv, method, .. } = &*e.node {
        if is_enqueue(method) && matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef)) {
            *hit = true;
        }
    }
    e.node.for_each_child(&mut |c| find_self_enqueues(c, hit));
}

fn find_dynamic_enqueues(e: &Expr, app: &App, jobs: &BTreeSet<String>, out: &mut BTreeSet<String>) {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
        if is_enqueue(method) {
            match &*r.node {
                ExprNode::Const { .. } | ExprNode::SelfRef => {}
                // `x.class.perform_later`: the job `x` is, when its type
                // names one; otherwise it could be any.
                ExprNode::Send { recv: Some(x), method: m, .. } if m.as_str() == "class" => {
                    match &x.ty {
                        Some(Ty::Class { id, .. }) if jobs.contains(id.0.as_str()) => {
                            out.insert(id.0.as_str().to_string());
                        }
                        _ if matches!(&*x.node, ExprNode::SelfRef) => {}
                        _ => {
                            out.insert("*".to_string());
                        }
                    }
                }
                // A class in a variable, or any other computed receiver.
                _ => {
                    out.insert("*".to_string());
                }
            }
        }
    }
    let _ = app;
    e.node.for_each_child(&mut |c| find_dynamic_enqueues(c, app, jobs, out));
}

fn collect_sites(e: &Expr, jobs: &BTreeSet<String>, out: &mut BTreeMap<String, Vec<Site>>) {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if matches!(method.as_str(), "perform_later" | "perform_now") {
            if let ExprNode::Const { path } = &*r.node {
                let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                if jobs.contains(&name) {
                    let site = args
                        .iter()
                        .map(|a| match &*a.node {
                            ExprNode::Splat { .. } | ExprNode::KeywordSplat { .. } => None,
                            ExprNode::Hash { .. } => None,
                            _ => a.ty.clone(),
                        })
                        .collect();
                    out.entry(name).or_default().push(site);
                }
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_sites(c, jobs, out));
}

fn plan_params(
    app: &App,
    sti: &std::collections::HashMap<ClassId, ClassId>,
    params: &[crate::dialect::Param],
    sites: &[Site],
) -> Result<Vec<JobParam>, JobFallback> {
    let gap = |param: Option<&Symbol>, reason: String| JobFallback { param: param.cloned(), reason };
    if params.is_empty() {
        return Ok(Vec::new());
    }
    if sites.is_empty() {
        return Err(gap(None, "no call site types its arguments".to_string()));
    }
    let rest_at = params.iter().position(|p| p.rest);
    if rest_at.is_some_and(|k| k != params.len() - 1) {
        return Err(gap(None, "a rest parameter that is not last".to_string()));
    }
    let fixed = rest_at.unwrap_or(params.len());
    let mut out = Vec::new();
    for (i, p) in params.iter().enumerate() {
        if p.keyword || p.forwarding {
            return Err(gap(Some(&p.name), "keyword or forwarding parameter".to_string()));
        }
        let mut codec: Option<Codec> = None;
        for site in sites {
            if rest_at.is_none() && site.len() != params.len() {
                return Err(gap(Some(&p.name), "a call site passes a different number of arguments".to_string()));
            }
            if site.len() < fixed {
                return Err(gap(Some(&p.name), "a call site passes too few arguments".to_string()));
            }
            let slots: Vec<&Option<Ty>> = if p.rest { site[i..].iter().collect() } else { vec![&site[i]] };
            for slot in slots {
                let Some(ty) = slot else {
                    return Err(gap(Some(&p.name), "an argument's type is unknown".to_string()));
                };
                let Some(c) = codec_of(app, sti, ty) else {
                    return Err(gap(Some(&p.name), format!("no codec for `{}`", describe(ty))));
                };
                codec = match codec.take() {
                    None => Some(c),
                    Some(prev) => match join(prev, c) {
                        Some(j) => Some(j),
                        None => {
                            return Err(gap(Some(&p.name), "call sites disagree on its type".to_string()));
                        }
                    },
                };
            }
        }
        let Some(codec) = codec else {
            return Err(gap(Some(&p.name), "no call site passes it".to_string()));
        };
        if p.rest && matches!(codec, Codec::NullableRecord { .. }) {
            return Err(gap(Some(&p.name), "a rest parameter of nullable records".to_string()));
        }
        out.push(JobParam { name: p.name.clone(), rest: p.rest, codec });
    }
    Ok(out)
}

fn describe(ty: &Ty) -> String {
    match ty {
        Ty::Class { id, .. } => id.0.as_str().to_string(),
        Ty::Union { variants } => variants.iter().map(describe).collect::<Vec<_>>().join(" | "),
        Ty::Array { elem } => format!("Array[{}]", describe(elem)),
        other => format!("{other:?}"),
    }
}

/// The STI base a record type is found on, with every name it accepts.
fn record_of(
    app: &App,
    sti: &std::collections::HashMap<ClassId, ClassId>,
    id: &ClassId,
) -> Option<(String, Vec<String>)> {
    let base = if app.models.iter().any(|m| &m.name == id) {
        id.clone()
    } else {
        sti.get(id)?.clone()
    };
    // Only a model with a table has `find_by` and a key to write: an
    // ActiveModel class under app/models or the abstract
    // `ApplicationRecord` keeps the Proc path. Same gate as
    // `project::apply_instantiate_named`.
    let has_table = app
        .models
        .iter()
        .find(|m| m.name == base)
        .is_some_and(|m| app.schema.tables.contains_key(&m.table.0));
    if !has_table {
        return None;
    }
    let mut accepts: Vec<String> = vec![base.0.as_str().to_string()];
    let mut subs: Vec<String> = sti
        .iter()
        .filter(|(_, b)| **b == base)
        .map(|(s, _)| s.0.as_str().to_string())
        .collect();
    subs.sort();
    accepts.extend(subs);
    Some((base.0.as_str().to_string(), accepts))
}

fn codec_of(app: &App, sti: &std::collections::HashMap<ClassId, ClassId>, ty: &Ty) -> Option<Codec> {
    Some(match ty {
        Ty::Bool => Codec::Bool,
        Ty::Int => Codec::Int,
        Ty::Float => Codec::Float,
        Ty::Str => Codec::Str,
        Ty::Sym => Codec::Sym,
        Ty::Time => Codec::Time,
        Ty::Class { id, .. } => {
            let (model, accepts) = record_of(app, sti, id)?;
            Codec::Record { model, accepts }
        }
        Ty::Union { variants } => {
            let mut codec: Option<Codec> = None;
            let mut nullable = false;
            for v in variants {
                if matches!(v, Ty::Nil) {
                    nullable = true;
                    continue;
                }
                let c = codec_of(app, sti, v)?;
                codec = Some(match codec {
                    None => c,
                    Some(prev) => join(prev, c)?,
                });
            }
            match (codec?, nullable) {
                (c, false) => c,
                (Codec::Record { model, accepts }, true) => Codec::NullableRecord { model, accepts },
                (c @ Codec::NullableRecord { .. }, true) => c,
                _ => return None,
            }
        }
        Ty::Array { elem } => {
            let inner = codec_of(app, sti, elem)?;
            match inner {
                Codec::NullableRecord { .. } | Codec::Array(_) => return None,
                c => Codec::Array(Box::new(c)),
            }
        }
        _ => return None,
    })
}

/// The codec two call sites' types share, or None when they disagree.
fn join(a: Codec, b: Codec) -> Option<Codec> {
    use Codec::*;
    match (a, b) {
        (x, y) if x == y => Some(x),
        (Record { model: m1, accepts }, NullableRecord { model: m2, .. })
        | (NullableRecord { model: m1, accepts }, Record { model: m2, .. })
            if m1 == m2 =>
        {
            Some(NullableRecord { model: m1, accepts })
        }
        (Array(x), Array(y)) => Some(Array(Box::new(join(*x, *y)?))),
        _ => None,
    }
}

/// `queue_as :name` in the job's class body or a job ancestor's,
/// nearest first; "default" when none says.
fn queue_name(app: &App, lc: &LibraryClass) -> String {
    for class in ancestry(app, lc) {
        for call in &class.unknown_calls {
            let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { continue };
            if method.as_str() != "queue_as" {
                continue;
            }
            if let Some(ExprNode::Lit { value: Literal::Sym { value } }) = args.first().map(|a| &*a.node) {
                return value.as_str().to_string();
            }
            if let Some(ExprNode::Lit { value: Literal::Str { value } }) = args.first().map(|a| &*a.node) {
                return value.clone();
            }
        }
    }
    "default".to_string()
}

fn discards_deserialization_error(app: &App, lc: &LibraryClass) -> bool {
    ancestry(app, lc).iter().any(|class| class.unknown_calls.iter().any(is_discard_on_deserialization))
}

fn is_discard_on_deserialization(call: &Expr) -> bool {
    let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return false };
    method.as_str() == "discard_on"
        && args.len() == 1
        && matches!(&*args[0].node, ExprNode::Const { path }
            if path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
                .trim_start_matches("::") == "ActiveJob::DeserializationError")
}

/// `self.enqueue_after_transaction_commit = <bool>` in the class body
/// or a job ancestor's, nearest class first and the last assignment in
/// a class winning; false (Rails 8.1's default) when none says.
fn enqueues_after_commit(app: &App, lc: &LibraryClass) -> bool {
    for class in ancestry(app, lc) {
        for call in class.unknown_calls.iter().rev() {
            if let Some(value) = enqueue_after_commit_literal(call) {
                return value;
            }
        }
    }
    false
}

fn enqueue_after_commit_literal(call: &Expr) -> Option<bool> {
    let ExprNode::Send { method, args, .. } = &*call.node else { return None };
    if method.as_str() != "enqueue_after_transaction_commit=" || args.len() != 1 {
        return None;
    }
    match &*args[0].node {
        ExprNode::Lit { value: Literal::Bool { value } } => Some(*value),
        _ => None,
    }
}

fn is_literal_queue_as(call: &Expr) -> bool {
    let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return false };
    method.as_str() == "queue_as"
        && args.len() == 1
        && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } })
}

/// Whether a class-body call of a job is carried by the payload plan
/// rather than dropped: a literal `queue_as`, or `discard_on
/// ActiveJob::DeserializationError`, on a job that has a payload — or on
/// an ancestor (`ApplicationJob`) every one of whose job descendants
/// does. The Ruby emitter skips its "dropped" ledger line for these.
pub fn modelled_class_body_call(app: &App, lc: &LibraryClass, call: &Expr) -> bool {
    if !is_literal_queue_as(call)
        && !is_discard_on_deserialization(call)
        && enqueue_after_commit_literal(call).is_none()
    {
        return false;
    }
    let name = lc.name.0.as_str();
    let mut users = app.job_plans.iter().filter(|p| {
        p.job == name
            || app
                .library_classes
                .iter()
                .find(|c| c.name.0.as_str() == p.job)
                .is_some_and(|c| ancestry(app, c).iter().any(|a| a.name == lc.name))
    });
    let mut any = false;
    users.all(|p| {
        any = true;
        p.is_payload()
    }) && any
}

/// The class, then its parents within the ingested set.
fn ancestry<'a>(app: &'a App, lc: &'a LibraryClass) -> Vec<&'a LibraryClass> {
    let mut out = vec![lc];
    let mut cursor = lc.parent.clone();
    while let Some(p) = cursor {
        if out.len() > 8 {
            break;
        }
        let Some(parent) = app.library_classes.iter().find(|c| c.name == p) else { break };
        out.push(parent);
        cursor = parent.parent.clone();
    }
    out
}

// ---- The Ruby-family rewrite ----------------------------------------

/// Rewrite each payload job's enqueue arm, and ledger the jobs that keep
/// their Proc.
pub fn apply_ruby(lcs: &mut [LibraryClass], app: &App) {
    let sidekiq = app.queue_adapter.as_deref() == Some("sidekiq");
    for lc in lcs.iter_mut() {
        let own_name = lc.name.0.as_str().to_string();
        let Some(plan) = app.job_plans.iter().find(|p| p.job == own_name) else { continue };
        let Some(wrapper) = lc.methods.iter_mut().find(|m| {
            m.receiver == crate::dialect::MethodReceiver::Class && m.name.as_str() == "perform_later"
        }) else {
            continue;
        };
        let span = wrapper.name_span;
        if let Some(f) = &plan.fallback {
            crate::emit::diagnostics::push(fallback_diagnostic(plan, f, span, sidekiq));
            continue;
        }
        let args: Vec<Expr> = wrapper
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut v = Expr::new(
                    wrapper.body.span,
                    ExprNode::Var { id: crate::ident::VarId(i as u32), name: p.name.clone() },
                );
                v.ty = match &wrapper.signature {
                    Some(Ty::Fn { params, .. }) => params.get(i).map(|tp| (*tp.ty).clone()),
                    _ => None,
                };
                v
            })
            .collect();
        if !rewrite_enqueue(&mut wrapper.body, plan, args, sidekiq) {
            crate::emit::diagnostics::push(fallback_diagnostic(
                plan,
                &JobFallback {
                    param: None,
                    reason: "the generated perform_later is not the shape this pass rewrites".to_string(),
                },
                span,
                sidekiq,
            ));
        }
    }
}

/// The ledger line for a job that keeps its Proc. Under `queue_adapter
/// :sidekiq` it is an error: a Proc can't be pushed to Redis, so the job
/// would silently run in process instead of where the app sends it.
fn fallback_diagnostic(
    plan: &JobPlan,
    f: &JobFallback,
    span: crate::span::Span,
    sidekiq: bool,
) -> crate::diagnostic::Diagnostic {
    let what = match &f.param {
        Some(p) => format!("`{}`'s parameter `{}`: {}", plan.job, p.as_str(), f.reason),
        None => format!("`{}`: {}", plan.job, f.reason),
    };
    if sidekiq {
        let mut d = crate::lower::residue_diagnostic(
            "job_payload",
            "job-closure-fallback",
            span,
            "argument not serializable",
            format!(
                "job_payload: {what}; under queue_adapter :sidekiq the job must be pushed to Redis \
                 as an ActiveJob payload, and a closure can't be"
            ),
        );
        d.severity = crate::diagnostic::Severity::Error;
        return d;
    }
    crate::lower::residue_diagnostic(
        "job_payload",
        "job-closure-fallback",
        span,
        "argument not serializable",
        format!("job_payload: {what}; the job is queued as a Proc, not an ActiveJob payload"),
    )
}

/// Find `ActiveJob.enqueue(<lambda>)` in the wrapper `job_class_side`
/// writes — `if !ActiveJob.enqueue_only; if ActiveJob.drain_registered;
/// <here> else <inline> end else <hold> end` — and replace it. The shape
/// is a contract with that pass; anything else is left alone and
/// answers false.
///
/// Under `queue_adapter :sidekiq` the arm is
/// `JobRegistry.push_<job>("<queue_as>", 0.0, args)` instead, which pushes
/// the payload to Redis in Sidekiq's format (`runtime/sidekiq_push.rb`).
fn rewrite_enqueue(body: &mut Expr, plan: &JobPlan, args: Vec<Expr>, sidekiq: bool) -> bool {
    let ExprNode::Seq { exprs } = &mut *body.node else { return false };
    for e in exprs.iter_mut() {
        let ExprNode::If { then_branch, .. } = &mut *e.node else { continue };
        let ExprNode::If { cond, then_branch: enqueue, .. } = &mut *then_branch.node else { continue };
        if !is_active_job_call(cond, "drain_registered") {
            continue;
        }
        if !is_active_job_call(enqueue, "enqueue") {
            continue;
        }
        let span = enqueue.span;
        if sidekiq {
            let mut queue = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: plan.queue.clone() } });
            queue.ty = Some(Ty::Str);
            let mut at = Expr::new(span, ExprNode::Lit { value: Literal::Float { value: 0.0 } });
            at.ty = Some(Ty::Float);
            let mut push_args = vec![queue, at];
            push_args.extend(args);
            let mut call = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JobRegistry")] })),
                    method: Symbol::from(format!("push_{}", plan.suffix()).as_str()),
                    args: push_args,
                    block: None,
                    parenthesized: true,
                },
            );
            call.ty = Some(Ty::Nil);
            *enqueue = call;
            return true;
        }
        let mut payload = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JobRegistry")] })),
                method: Symbol::from(format!("payload_{}", plan.suffix()).as_str()),
                args,
                block: None,
                parenthesized: true,
            },
        );
        payload.ty = Some(Ty::Str);
        let mut call = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveJob")] })),
                // `enqueue_payload_after_commit` holds it while a
                // transaction is open (`runtime/job_registry.rb`).
                method: Symbol::from(if plan.enqueue_after_commit {
                    "enqueue_payload_after_commit"
                } else {
                    "enqueue_payload"
                }),
                args: vec![payload],
                block: None,
                parenthesized: true,
            },
        );
        call.ty = Some(Ty::Nil);
        *enqueue = call;
        return true;
    }
    false
}

fn is_active_job_call(e: &Expr, name: &str) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: Some(r), method, .. }
        if method.as_str() == name
            && matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "ActiveJob"))
}

/// `def to_gid_uri; GlobalID.uri(<name>, self.id); end` on every
/// model, beside `to_gid_param` and for the same reasons
/// (`lower::broadcasts::push_to_gid_param`): the unencoded GlobalID a
/// payload carries for a record. The name is the one `to_gid_param`
/// mints, a literal or, on an STI base, a dispatch on the `type` column
/// (rows hydrate base-classed), so a `Rooms::Open` row writes
/// `gid://app/Rooms::Open/3`, as Rails does.
pub fn push_model_to_gid_uri(methods: &mut Vec<crate::dialect::MethodDef>, model: &crate::dialect::Model) {
    let name = Symbol::from("to_gid_uri");
    if methods
        .iter()
        .any(|m| m.name == name && m.receiver == crate::dialect::MethodReceiver::Instance)
    {
        return;
    }
    let sp = crate::span::Span::synthetic();
    let mut id_read = Expr::new(
        sp,
        ExprNode::Send {
            recv: Some(Expr::new(sp, ExprNode::SelfRef)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    id_read.ty = Some(model.attributes.fields.get(&Symbol::from("id")).cloned().unwrap_or(Ty::Int));
    let mut body = Expr::new(
        sp,
        ExprNode::Send {
            recv: Some(Expr::new(sp, ExprNode::Const { path: vec![Symbol::from("GlobalID")] })),
            method: Symbol::from("uri"),
            args: vec![crate::lower::broadcasts::gid_model_name(model), id_read],
            block: None,
            parenthesized: true,
        },
    );
    body.ty = Some(Ty::Str);
    methods.push(crate::dialect::MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: sp,
        name,
        receiver: crate::dialect::MethodReceiver::Instance,
        params: vec![],
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });
}

// ---- The generated registry -----------------------------------------

/// The Ruby that writes a job's arguments as an `Arguments.list` input:
/// one writer per fixed parameter, and a rest parameter's elements
/// appended, as Rails flattens a splat.
fn arguments_list(plan: &JobPlan) -> String {
    let fixed: Vec<String> = plan
        .params
        .iter()
        .filter(|p| !p.rest)
        .map(|p| write_expr(&p.codec, p.name.as_str()))
        .collect();
    let mut list = format!("[{}]", fixed.join(", "));
    if let Some(rest) = plan.params.iter().find(|p| p.rest) {
        let elems = format!("{}.map {{ |a| {} }}", rest.name.as_str(), write_expr(&rest.codec, "a"));
        list = if fixed.is_empty() { elems } else { format!("{list} + {elems}") };
    }
    list
}

/// The text between the `job-registry` markers in
/// `runtime/job_registry.rb`: the payload writers, `perform`,
/// `discards_deserialization_error`, and the record locators. Valid
/// Ruby for an app with no payload jobs too: `perform` answers false
/// and nothing else is written.
///
/// With `sidekiq` (the app is on `queue_adapter :sidekiq`), each payload
/// job also gets `push_<job>(queue, at, args)`, which the job's
/// `perform_later` calls instead of queueing in process: the payload,
/// with `scheduled_at` when `at` is set, handed to `SidekiqPush`.
pub fn registry_source(plans: &[JobPlan], sidekiq: bool) -> String {
    let payload: Vec<&JobPlan> = plans.iter().filter(|p| p.is_payload()).collect();
    let mut s = String::new();

    for plan in &payload {
        let list = arguments_list(plan);
        s.push_str(&format!("  def self.payload_{}{}\n", plan.suffix(), writer_params(plan)));
        s.push_str(&format!(
            "    ActiveJob::Payload.build({}, {}, ActiveJob::Arguments.list({list}))\n",
            ruby_str(&plan.job),
            ruby_str(&plan.queue)
        ));
        s.push_str("  end\n\n");
        if sidekiq {
            let params = writer_params(plan);
            let params = match params.strip_prefix('(') {
                Some(rest) => format!("(queue, at, {rest}"),
                None => "(queue, at)".to_string(),
            };
            let push = if plan.enqueue_after_commit { "enqueue_after_commit" } else { "enqueue" };
            s.push_str(&format!("  def self.push_{}{params}\n", plan.suffix()));
            s.push_str(&format!(
                "    SidekiqPush.{push}({job}, queue, at,\n      ActiveJob::Payload.build_at({job}, queue, at, ActiveJob::Arguments.list({list})))\n",
                job = ruby_str(&plan.job),
            ));
            s.push_str("  end\n\n");
        }
    }

    s.push_str("  def self.perform(job_class, args)\n");
    if payload.is_empty() {
        s.push_str("    false\n");
    } else {
        s.push_str("    case job_class\n");
        for plan in &payload {
            s.push_str(&format!("    when {}\n", ruby_str(&plan.job)));
            let mut call_args: Vec<String> = Vec::new();
            for (i, p) in plan.params.iter().enumerate() {
                if p.rest {
                    call_args.push(format!("*{}", read_rest(&p.codec, i)));
                } else {
                    call_args.push(read_expr(&p.codec, &format!("args, {i}")));
                }
            }
            if call_args.is_empty() {
                s.push_str(&format!("      {}.new.perform\n", plan.job));
            } else {
                s.push_str(&format!("      {}.new.perform({})\n", plan.job, call_args.join(", ")));
            }
            s.push_str("      true\n");
        }
        s.push_str("    else\n      false\n    end\n");
    }
    s.push_str("  end\n\n");

    s.push_str("  def self.discards_deserialization_error(job_class)\n");
    let discarding: Vec<&&JobPlan> =
        payload.iter().filter(|p| p.discards_deserialization_error).collect();
    if discarding.is_empty() {
        s.push_str("    false\n");
    } else {
        let names: Vec<String> = discarding.iter().map(|p| format!("job_class == {}", ruby_str(&p.job))).collect();
        s.push_str(&format!("    {}\n", names.join(" || ")));
    }
    s.push_str("  end\n");

    // One locator per record model a payload job reads, and a list
    // reader where a rest or Array parameter reads several.
    let mut models: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut lists: BTreeSet<String> = BTreeSet::new();
    for plan in &payload {
        for p in &plan.params {
            let mut c = &p.codec;
            let mut many = p.rest;
            if let Codec::Array(inner) = c {
                c = inner;
                many = true;
            }
            if let Codec::Record { model, accepts } | Codec::NullableRecord { model, accepts } = c {
                models.insert(model.clone(), accepts.clone());
                if many {
                    lists.insert(model.clone());
                }
            }
        }
    }
    for (model, accepts) in &models {
        let sfx = suffix(model);
        let check: Vec<String> = accepts.iter().map(|a| format!("name == {}", ruby_str(a))).collect();
        s.push_str(&format!(
            "\n  def self.locate_{sfx}_at(args, i)\n\
             \x20   parts = ActiveJob::Arguments.gid_parts(args[i])\n\
             \x20   name = parts[1]\n\
             \x20   unless {check}\n\
             \x20     raise ActiveJob::DeserializationError, ActiveJob::Arguments.deserialize_message(\"unexpected model \" + name)\n\
             \x20   end\n\
             \x20   record = {model}.find_by(id: GlobalID::Locator.cast_id(parts[2]))\n\
             \x20   raise ActiveJob::DeserializationError, ActiveJob::Arguments.missing_record_message(name, parts[2]) if record.nil?\n\
             \x20   record\n\
             \x20 end\n",
            check = check.join(" || "),
        ));
        if lists.contains(model) {
            s.push_str(&format!(
                "\n  def self.locate_{sfx}_from(args, from)\n\
                 \x20   (from...ActiveJob::Arguments.arg_count(args)).map {{ |j| locate_{sfx}_at(args, j) }}\n\
                 \x20 end\n",
            ));
        }
    }
    s
}

/// `(room, message)`, or nothing for a job without parameters. A rest
/// parameter is a plain one here: `perform_later(*comments)` hands the
/// writer `comments`, the Array it collected, not its elements.
fn writer_params(plan: &JobPlan) -> String {
    if plan.params.is_empty() {
        return String::new();
    }
    let list = plan
        .params
        .iter()
        .map(|p| p.name.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("({list})")
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write_expr(codec: &Codec, v: &str) -> String {
    let a = "ActiveJob::Arguments";
    match codec {
        Codec::Bool => format!("{a}.bool({v})"),
        Codec::Int => format!("{a}.int({v})"),
        Codec::Float => format!("{a}.float({v})"),
        Codec::Str => format!("{a}.str({v})"),
        Codec::Sym => format!("{a}.sym({v})"),
        Codec::Time => format!("{a}.time({v})"),
        Codec::Record { .. } => format!("{a}.record({v}.to_gid_uri)"),
        Codec::NullableRecord { .. } => {
            format!("({v}.nil? ? {a}.null : {a}.record({v}.to_gid_uri))")
        }
        Codec::Array(inner) => format!("{a}.list({v}.map {{ |e| {} }})", write_expr(inner, "e")),
    }
}

/// `at` is `args, i`: the parsed argument list and the index.
fn read_expr(codec: &Codec, at: &str) -> String {
    let a = "ActiveJob::Arguments";
    match codec {
        Codec::Bool => format!("{a}.bool_at({at})"),
        Codec::Int => format!("{a}.int_at({at})"),
        Codec::Float => format!("{a}.float_at({at})"),
        Codec::Str => format!("{a}.str_at({at})"),
        Codec::Sym => format!("{a}.sym_at({at})"),
        Codec::Time => format!("{a}.time_at({at})"),
        Codec::Record { model, .. } => format!("locate_{}_at({at})", suffix(model)),
        Codec::NullableRecord { model, .. } => {
            format!("({a}.null_at({at}) ? nil : locate_{}_at({at}))", suffix(model))
        }
        Codec::Array(inner) => match &**inner {
            Codec::Bool => format!("{a}.bool_array_at({at})"),
            Codec::Int => format!("{a}.int_array_at({at})"),
            Codec::Float => format!("{a}.float_array_at({at})"),
            Codec::Str => format!("{a}.str_array_at({at})"),
            Codec::Sym => format!("{a}.sym_array_at({at})"),
            Codec::Record { model, .. } => {
                let (args, i) = at.split_once(", ").unwrap_or((at, "0"));
                format!("locate_{}_from({args}[{i}], 0)", suffix(model))
            }
            // Excluded by `codec_of`.
            _ => "nil".to_string(),
        },
    }
}

/// The rest parameter's elements, from position `from` on.
fn read_rest(codec: &Codec, from: usize) -> String {
    let a = "ActiveJob::Arguments";
    match codec {
        Codec::Record { model, .. } => format!("locate_{}_from(args, {from})", suffix(model)),
        other => {
            let one = read_expr(other, "args, j");
            format!("(({from})...{a}.count(args)).map {{ |j| {one} }}")
        }
    }
}

/// The one warning a target that doesn't honor `queue_adapter :sidekiq`
/// files for an app on it: the strict targets have no Sidekiq push, so
/// the app's jobs run in process there. Only for an app with ActiveJob
/// jobs; one whose background work is all native Sidekiq workers has
/// nothing to say it about. Decided by the host, which knows the target
/// (`src/bin/roundhouse.rs`), as `elevate_nested_params_for_target` is.
pub fn sidekiq_adapter_target_warning(
    app: &App,
    target: crate::build_target::BuildTarget,
) -> Option<crate::diagnostic::Diagnostic> {
    use crate::build_target::BuildTarget;
    if app.queue_adapter.as_deref() != Some("sidekiq") || app.job_plans.is_empty() {
        return None;
    }
    if matches!(target, BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby) {
        return None;
    }
    Some(crate::lower::residue_diagnostic(
        "job_payload",
        "sidekiq-adapter-not-honored",
        crate::span::Span::synthetic(),
        "adapter not modeled on this target",
        format!(
            "job_payload: queue_adapter :sidekiq is honored on ruby, jruby and spinel; \
             on {} the app's jobs run in process",
            target.as_str()
        ),
    ))
}
