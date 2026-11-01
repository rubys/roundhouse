//! `broadcasts_to` on `queue_adapter :sidekiq`: the create and update
//! broadcasts go to Redis as `Turbo::Streams::ActionBroadcastJob`, as
//! turbo-rails does, instead of rendering inside the save.
//!
//! turbo-rails' `broadcasts_to` installs `broadcast_action_later_to` on
//! create and `broadcast_replace_later_to` on update, and both enqueue
//! the job; only the destroy broadcast (`broadcast_remove_to`) runs at
//! once. `lower::model_to_library::broadcasts` expands the declaration
//! into the three inline `Broadcasts.<action>` calls every target runs.
//! On the Ruby-family lanes of an app on `:sidekiq`, [`apply_ruby`]
//! wraps the create and update calls so a process with a drain (the
//! server and the worker role) pushes the job instead:
//!
//! ```text
//! if ActiveJob.drain_registered
//!   JobRegistry.push_turbo_broadcast(stream, "prepend", target, "cars/car", "car", self.to_gid_uri)
//! else
//!   Broadcasts.prepend(stream: …, target: …, html: …)   # as before
//! end
//! ```
//!
//! and gives the model `turbo_broadcast_html`, the partial render the
//! inline call makes, which the worker calls on the record it finds
//! again. [`registry_source`] writes the push, the worker's arm for the
//! job, and its record lookups into `JobRegistry`. The emitted test
//! suites register no drain, so they keep the inline broadcasts their
//! assertions were written against, as the jobs' own wrappers do.
//!
//! The payload is turbo-rails 2.0's, key for key
//! (`ActiveJob::Arguments.turbo_broadcast_options`): the stream, then
//! `action:`, `target:`, `targets: nil`, `attributes: {}`, the record as
//! the one local under its `model_name.element`, and `partial:` from
//! `to_partial_path`.
//!
//! A model this can't write the payload for keeps its inline broadcast
//! with a ledger line: an STI model (`to_partial_path` and the local's
//! name follow the row's own class, which is known only at run time),
//! and a stream or `target:` that isn't a String expression.

use crate::app::App;
use crate::dialect::{LibraryClass, MethodReceiver, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// The job's class name, as turbo-rails names it.
pub const JOB_CLASS: &str = "Turbo::Streams::ActionBroadcastJob";

/// The model method holding the partial render the worker sends.
const HTML_METHOD: &str = "turbo_broadcast_html";

/// One model whose `broadcasts_to` enqueues the job.
#[derive(Clone, Debug, PartialEq)]
pub struct BroadcastModel {
    /// `Car`.
    pub model: String,
    /// `to_partial_path`: `cars/car`.
    pub partial: String,
    /// `model_name.element`, the local the partial gets: `car`.
    pub local: String,
}

/// Whether the app's broadcasts go through the job at all: the Ruby
/// family on `:sidekiq`.
fn enabled(app: &App) -> bool {
    app.queue_adapter.as_deref() == Some("sidekiq")
}

/// The `broadcasts_to` declarations of `model`, as the expansion in
/// `model_to_library::broadcasts` reads them: the first argument a
/// one-parameter lambda or a String literal.
fn declarations(model: &Model) -> Vec<&Expr> {
    model
        .body
        .iter()
        .filter_map(|item| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
            if method.as_str() != "broadcasts_to" || args.is_empty() {
                return None;
            }
            match &*args[0].node {
                ExprNode::Lambda { extra_params, .. } if extra_params.is_empty() => Some(expr),
                ExprNode::Lit { value: Literal::Str { .. } } => Some(expr),
                _ => None,
            }
        })
        .collect()
}

/// Every model whose `broadcasts_to` enqueues the job, in name order.
/// Empty off `:sidekiq`.
pub fn models(app: &App) -> Vec<BroadcastModel> {
    if !enabled(app) {
        return Vec::new();
    }
    let sti = crate::lower::sti_scope::sti_bases(app);
    let mut out: Vec<BroadcastModel> = app
        .models
        .iter()
        .filter(|m| !declarations(m).is_empty())
        .filter(|m| !sti.contains_key(&m.name) && !sti.values().any(|b| *b == m.name))
        .map(|m| {
            let name = m.name.0.as_str();
            let local = crate::naming::snake_case(crate::naming::demodulize(name));
            // ActiveModel::Name#collection: the namespaced path, the last
            // segment pluralized (`Admin::Car` -> `admin/cars`).
            let mut path: Vec<String> = name.split("::").map(crate::naming::snake_case).collect();
            if let Some(last) = path.last_mut() {
                *last = crate::naming::pluralize_snake(crate::naming::demodulize(name));
            }
            BroadcastModel { model: name.to_string(), partial: format!("{}/{local}", path.join("/")), local }
        })
        .collect();
    out.sort_by(|a, b| a.model.cmp(&b.model));
    out
}

/// Rewrite each broadcasting model's create and update broadcasts, and
/// ledger the ones that stay inline.
pub fn apply_ruby(lcs: &mut [LibraryClass], app: &App) {
    if !enabled(app) {
        return;
    }
    let planned = models(app);
    for model in &app.models {
        let decls = declarations(model);
        if decls.is_empty() {
            continue;
        }
        if !lcs.iter().any(|lc| lc.name == model.name) {
            continue;
        }
        let Some(plan) = planned.iter().find(|p| p.model == model.name.0.as_str()) else {
            for d in decls {
                crate::emit::diagnostics::push(inline_diagnostic(
                    d.span,
                    &format!("`{}` is an STI model, so its partial and local name follow each row's class", model.name.0.as_str()),
                ));
            }
            continue;
        };
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else { continue };
        let spans: Vec<Span> = decls.iter().map(|d| d.span).collect();
        let mut html: Option<Expr> = None;
        let mut kept: Vec<Span> = Vec::new();
        for m in lc.methods.iter_mut() {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            if !matches!(m.name.as_str(), "after_create_commit" | "after_update_commit") {
                continue;
            }
            rewrite(&mut m.body, &spans, plan, &mut html, &mut kept);
        }
        for span in kept {
            crate::emit::diagnostics::push(inline_diagnostic(
                span,
                "its stream or `target:` is not a String expression",
            ));
        }
        if let Some(html) = html {
            push_html_method(lc, model, html);
        }
    }
}

fn inline_diagnostic(span: Span, why: &str) -> crate::diagnostic::Diagnostic {
    crate::lower::residue_diagnostic(
        "broadcast_job",
        "broadcast-later-inline",
        span,
        "broadcast rendered inline",
        format!(
            "broadcast_job: this `broadcasts_to` renders its create and update broadcasts inline rather \
             than enqueuing Turbo::Streams::ActionBroadcastJob as Rails does: {why}"
        ),
    )
}

fn rewrite(e: &mut Expr, spans: &[Span], plan: &BroadcastModel, html: &mut Option<Expr>, kept: &mut Vec<Span>) {
    if spans.contains(&e.span) {
        if let Some(action) = broadcast_action(e) {
            let (stream, target, h) = kwargs(e);
            match (stream, target, h) {
                (Some(stream), Some(target), Some(h)) if stringish(&stream) && stringish(&target) => {
                    if html.is_none() {
                        *html = Some(h);
                    }
                    let push = push_call(e.span, stream, action, target, plan);
                    let orig = e.clone();
                    let mut cond = send(e.span, Some(konst(e.span, &["ActiveJob"])), "drain_registered", vec![]);
                    cond.ty = Some(Ty::Bool);
                    let mut wrapped = Expr::new(e.span, ExprNode::If { cond, then_branch: push, else_branch: orig });
                    wrapped.ty = Some(Ty::Nil);
                    *e = wrapped;
                }
                _ => {
                    if !kept.contains(&e.span) {
                        kept.push(e.span);
                    }
                }
            }
            return;
        }
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, spans, plan, html, kept));
}

/// `Broadcasts.<append|prepend|replace|update>(…)` -> the action.
fn broadcast_action(e: &Expr) -> Option<&'static str> {
    let ExprNode::Send { recv: Some(r), method, .. } = &*e.node else { return None };
    if !matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Broadcasts") {
        return None;
    }
    match method.as_str() {
        "append" => Some("append"),
        "prepend" => Some("prepend"),
        "replace" => Some("replace"),
        "update" => Some("update"),
        _ => None,
    }
}

fn kwargs(e: &Expr) -> (Option<Expr>, Option<Expr>, Option<Expr>) {
    let ExprNode::Send { args, .. } = &*e.node else { return (None, None, None) };
    let Some(first) = args.first() else { return (None, None, None) };
    let ExprNode::Hash { entries, .. } = &*first.node else { return (None, None, None) };
    let get = |name: &str| {
        entries.iter().find_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => Some(v.clone()),
            _ => None,
        })
    };
    (get("stream"), get("target"), get("html"))
}

/// A String literal or interpolation: what turbo-rails passes through
/// `stream_name_from` and `convert_to_turbo_stream_dom_id` unchanged.
fn stringish(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. })
}

fn konst(span: Span, path: &[&str]) -> Expr {
    Expr::new(span, ExprNode::Const { path: path.iter().map(|s| Symbol::from(*s)).collect() })
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
    )
}

fn str_lit(span: Span, s: &str) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } });
    e.ty = Some(Ty::Str);
    e
}

fn push_call(span: Span, stream: Expr, action: &str, target: Expr, plan: &BroadcastModel) -> Expr {
    let mut gid = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from("to_gid_uri"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    gid.ty = Some(Ty::Str);
    let mut call = send(
        span,
        Some(konst(span, &["JobRegistry"])),
        "push_turbo_broadcast",
        vec![stream, str_lit(span, action), target, str_lit(span, &plan.partial), str_lit(span, &plan.local), gid],
    );
    call.ty = Some(Ty::Nil);
    call
}

/// `def turbo_broadcast_html; <the partial render>; end` on the model.
fn push_html_method(lc: &mut LibraryClass, model: &Model, mut body: Expr) {
    let name = Symbol::from(HTML_METHOD);
    if lc.methods.iter().any(|m| m.name == name && m.receiver == MethodReceiver::Instance) {
        return;
    }
    body.ty = Some(Ty::Str);
    lc.methods.push(crate::dialect::MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name,
        receiver: MethodReceiver::Instance,
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

/// `JobRegistry.push_turbo_broadcast`, which the rewritten broadcasts
/// call, and `perform_turbo_broadcast`, the worker's arm for the job:
/// the record found again by its partial's local, rendered with the
/// model's `turbo_broadcast_html`, and sent with the action the payload
/// names. Empty when no model broadcasts through the job.
pub fn registry_source(models: &[BroadcastModel]) -> String {
    if models.is_empty() {
        return String::new();
    }
    let job = ruby_str(JOB_CLASS);
    let mut s = String::new();
    s.push_str("  def self.push_turbo_broadcast(stream, action, target, partial, local, gid_uri)\n");
    s.push_str(&format!(
        "    SidekiqPush.enqueue({job}, \"default\", 0.0,\n      ActiveJob::Payload.build_at({job}, \"default\", 0.0,\n        ActiveJob::Arguments.list([ActiveJob::Arguments.str(stream), ActiveJob::Arguments.turbo_broadcast_options(action, target, partial, local, gid_uri)])))\n"
    ));
    s.push_str("  end\n\n");

    s.push_str("  def self.perform_turbo_broadcast(args)\n");
    s.push_str("    stream = ActiveJob::Arguments.str_at(args, 0)\n");
    s.push_str("    action = ActiveJob::Arguments.option_sym_name_at(args, 1, \"action\")\n");
    s.push_str("    target = ActiveJob::Arguments.option_str_at(args, 1, \"target\")\n");
    s.push_str("    partial = ActiveJob::Arguments.option_str_at(args, 1, \"partial\")\n");
    s.push_str("    html = \"\"\n");
    s.push_str("    case partial\n");
    for m in models {
        s.push_str(&format!("    when {}\n", ruby_str(&m.partial)));
        s.push_str(&format!(
            "      html = {}(ActiveJob::Arguments.option_local_at(args, 1, {})).{HTML_METHOD}\n",
            locator_name(&m.model),
            ruby_str(&m.local)
        ));
    }
    s.push_str("    else\n");
    s.push_str("      raise ArgumentError, \"Turbo::Streams::ActionBroadcastJob: no partial \" + partial\n");
    s.push_str("    end\n");
    s.push_str("    case action\n");
    for a in ["append", "prepend", "replace", "update"] {
        s.push_str(&format!("    when \"{a}\"\n"));
        s.push_str(&format!("      Broadcasts.{a}(stream: stream, target: target, html: html)\n"));
    }
    s.push_str("    else\n");
    s.push_str("      raise ArgumentError, \"Turbo::Streams::ActionBroadcastJob: no action \" + action\n");
    s.push_str("    end\n");
    s.push_str("    nil\n");
    s.push_str("  end\n");

    for m in models {
        s.push_str(&format!(
            "\n  def self.{}(value)\n\
             \x20   parts = ActiveJob::Arguments.gid_parts(value)\n\
             \x20   name = parts[1]\n\
             \x20   unless name == {model}\n\
             \x20     raise ActiveJob::DeserializationError, ActiveJob::Arguments.deserialize_message(\"unexpected model \" + name)\n\
             \x20   end\n\
             \x20   record = {cls}.find_by(id: GlobalID::Locator.cast_id(parts[2]))\n\
             \x20   raise ActiveJob::DeserializationError, ActiveJob::Arguments.missing_record_message(name, parts[2]) if record.nil?\n\
             \x20   record\n\
             \x20 end\n",
            locator_name(&m.model),
            model = ruby_str(&m.model),
            cls = m.model,
        ));
    }
    s
}

fn locator_name(model: &str) -> String {
    format!("locate_broadcast_{}", crate::naming::underscore(model).replace('/', "__"))
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
