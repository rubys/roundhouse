//! Action-body normalization — the pre-emit pipeline that reshapes
//! an action's body `Expr` so every target emitter can walk it
//! without per-target special cases:
//!
//!   1. Inline applicable `before_action` callback bodies
//!      (`actions::resolve_before_actions`).
//!   2. Flatten `respond_to { format.html {…} format.json {…} }` into
//!      an `if request_format == :json; …; else …; end` dispatch
//!      (when both branches use the `render :sym` shape); fall back
//!      to html-only when either branch has a more complex shape
//!      (`unwrap_respond_to`).
//!   3. Append a synthetic `render :<action>` when the body has no
//!      explicit response terminal (`synthesize_implicit_render`).
//!
//! Per-target ivar/params rewrites run AFTER this pipeline.

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::dialect::Controller;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use super::actions::resolve_before_actions;
use super::util::{is_format_binding, unwrap_lambda};

/// Flatten every `respond_to do |format| ... end` block in `expr`
/// into just its HTML branch — the legacy behavior used by the
/// per-target paths (`normalize_action_body`) that don't yet know
/// how to emit the format dispatch. Group 1 emitters (Ruby /
/// Crystal / TS, via `lower_controllers_with_arel_and_views`) call
/// `unwrap_respond_to_with_format_dispatch` instead, which
/// preserves the json branch as a `request_format == :json`
/// conditional.
pub fn unwrap_respond_to(expr: &Expr) -> Expr {
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ false, FormatBreadth::NARROW, ActionFormats::NONE)
}

/// What the lowering knows about the action a `respond_to` belongs to,
/// for the arms that name no body of their own: a bare
/// `format.turbo_stream` renders THE ACTION's `<action>.turbo_stream`
/// template, so the flattener has to know which action it is flattening
/// and whether that template exists. `NONE` is the answer for a body
/// that is not a public action (a filter, a helper, a rescue handler):
/// Rails resolves the implicit template of the action being PROCESSED,
/// which no static name reaches from there.
#[derive(Clone, Copy, Debug)]
pub struct ActionFormats<'a> {
    /// The public action's name.
    pub action: Option<&'a str>,
    /// The non-html formats the action has a template for, as
    /// `controller_to_library` scans them: `json`, `turbo_stream`, `js`.
    pub variants: &'a [&'a str],
    /// The action has an html template. A bare `format.html` then renders
    /// it explicitly: the implicit render behind the arm chain tests
    /// `request_format` over every template that exists, and would pick
    /// the turbo_stream one for an `Accept` that lists it, whichever arm
    /// negotiation ran.
    pub html_template: bool,
}

impl ActionFormats<'static> {
    pub const NONE: Self = Self { action: None, variants: &[], html_template: false };
}

/// Which non-html `respond_to` arms an emit path can carry. The two
/// widenings are independent because they cost different things:
///
/// `json_any` preserves `format.json` branches of ANY shape — inline
/// `render json: <expr>` normalizes downstream to an
/// `ActionController::JsonRender.encode` body render, and JsonRender
/// is a CRuby-overlay module (`respond_to?`-dispatching, heterogeneous
/// containers) that the AOT compile cannot type. CRuby/JRuby only.
/// With it off, the pre-existing narrow rule still admits the simple
/// `render :sym` json arms, which route to an emitted `<sym>_json`
/// view and need no encoder.
///
/// `rss` preserves `format.rss` branches under a `request_format ==
/// :rss` arm (lobsters' /rss feed). Everything such an arm reaches is
/// already on every ruby-family tree — the emitted `Views::<X>.rss`
/// template and the shared `Rails.cache.fetch_str` — so the spinel
/// tree carries this one too. Without it the route-pinned rss entries
/// fall through to the html arm and the feed serves the HTML page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatBreadth {
    pub json_any: bool,
    pub rss: bool,
    /// The tree raises `ActionController::ParameterMissing` from
    /// `params.expect` / `params.require` and its dispatcher answers an
    /// unrescued one with 400, as Rails does. Ruby family only: the
    /// strict targets have no exception control flow and hand-written
    /// `Params` primitives, so a missing resource reads as `{}` there
    /// (ledgered in docs/pipeline/runtime.md).
    pub raises_param_missing: bool,
    /// The tree wraps a JSON request body under the controller's model
    /// name, as Rails' ParamsWrapper does (`Params.wrap`, and the body
    /// params the ruby-family dispatchers hand the request). The strict
    /// targets read no JSON body (Rust, Python) or no body params apart
    /// (TypeScript), so they do not.
    pub wraps_json_params: bool,
    /// The tree carries `ActionController::Base#negotiate_format` and its
    /// dispatcher answers `ActionController::UnknownFormat` with 406, as
    /// Rails does. A `respond_to` then lowers to Rails' own selection
    /// rule (the registered arms, tested in source order against the
    /// request's Accept list) instead of a path-suffix sniff with an
    /// html default, `format.turbo_stream` joins the arms it can carry,
    /// and an arm the tree cannot run is an ERROR rather than a dropped
    /// warning. Ruby family only: the other trees have no negotiation in
    /// their runtimes and keep the legacy `request_format` dispatch.
    pub negotiates: bool,
}

impl FormatBreadth {
    /// html only (plus the simple-`render :sym` json arms) — the emit
    /// paths that don't recognize `request_format` at all.
    pub const NARROW: Self = Self {
        json_any: false,
        rss: false,
        raises_param_missing: false,
        wraps_json_params: false,
        negotiates: false,
    };
    /// The spinel/AOT tree: rss dispatch, no JsonRender.
    pub const RSS_ONLY: Self =
        Self { json_any: false, rss: true, raises_param_missing: true, wraps_json_params: true, negotiates: true };
    /// The CRuby/JRuby trees, whose overlay answers the full surface.
    pub const FULL: Self =
        Self { json_any: true, rss: true, raises_param_missing: true, wraps_json_params: true, negotiates: true };
}

/// Format-dispatching variant of `unwrap_respond_to`.
///
/// When both the html and json branches use the simple `render :sym`
/// shape, the respond_to becomes an `if request_format == :json` /
/// `else` dispatch with the json branch's render carrying a `format:
/// :json` kwarg (consumed downstream by `rewrite_render_to_views` to
/// route to the `<sym>_json` view and tag `content_type:
/// "application/json"`). For all other json-branch shapes — inline
/// `render json: <expr>`, `head :no_content`, redirects in error
/// branches — we fall back to html-only flattening so the HTTP-HTML
/// paths every emitter targets stay lossless.
///
/// `breadth` widens the dispatch, one format at a time, because the
/// two widenings have different runtime costs (see `FormatBreadth`).
/// A fully-narrow breadth keeps the legacy behavior so an emit that
/// asks for nothing stays byte-identical.
///
/// Handles both scaffold shapes:
///   - Simple:    `respond_to { format.html { a }; format.json { b } }` → `if c; b' else a end`
///   - Branched:  `respond_to { if c; format.html { a1 }; format.json { b1 }
///                              else;  format.html { a2 }; format.json { b2 } end }`
///                 → `if c; <a1+b1 dispatch> else <a2+b2 dispatch> end`
///
/// Walks recursively — nested `respond_to` calls (rare) flatten
/// bottom-up, and non-respond_to sub-expressions pass through their
/// structural variants so anything already at the top level is
/// preserved.
pub fn unwrap_respond_to_with_format_dispatch(expr: &Expr, breadth: FormatBreadth) -> Expr {
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ true, breadth, ActionFormats::NONE)
}

/// `unwrap_respond_to_with_format_dispatch` for a body that is a public
/// action, so its bare format arms can name the action's templates.
pub fn unwrap_respond_to_for_action(
    expr: &Expr,
    breadth: FormatBreadth,
    ctx: ActionFormats<'_>,
) -> Expr {
    if breadth.negotiates {
        report_request_format_assignments(expr);
    }
    unwrap_respond_to_inner(expr, /*with_format_dispatch=*/ true, breadth, ctx)
}

/// `request.format = :json` in an action. Rails re-pins the request's
/// format list there, which every later `respond_to` and implicit render
/// honors; the runtime's request has no such setter (negotiation reads
/// the Accept header and the URL's pinned format), so the call would be a
/// NoMethodError at runtime. An error at compile time instead.
fn report_request_format_assignments(expr: &Expr) {
    fn is_request(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "request" && args.is_empty())
    }
    fn walk(e: &Expr) {
        let hit = match &*e.node {
            ExprNode::Assign { target: LValue::Attr { recv, name }, .. } => {
                name.as_str() == "format" && is_request(recv)
            }
            ExprNode::Send { recv: Some(recv), method, .. } => {
                method.as_str() == "format=" && is_request(recv)
            }
            _ => false,
        };
        if hit {
            report_respond_to_error(
                "request.format=",
                e.span,
                "not modeled",
                "`request.format = …` cannot be lowered: the request's format is its Accept \
                 header and the URL's pinned format, with no setter, so the call would be a \
                 NoMethodError at runtime"
                    .to_string(),
            );
        }
        e.node.for_each_child(&mut |c| walk(c));
    }
    walk(expr);
}

/// `respond_to do |wants| wants.html … end`: the arms are recognised by
/// the binding being named `format`, so a block that names it otherwise
/// has the name rewritten to `format` before anything reads the arms.
/// Why the respond_to block parameter cannot be renamed to `format`, if
/// it cannot: the block writes to the parameter (`f = "x"`). Renamed, that
/// write would land on the action's own `format` local, if it has one,
/// where Rails rebinds only the block's parameter. Reads of a `format`
/// local are safe: the arms are flattened into the negotiated dispatch, so
/// no binding named `format` is left to shadow it (`shadow_local`).
fn rename_conflict(block: &Expr) -> Option<&'static str> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
    if params.first().is_some_and(|p| p.as_str() == "format") {
        return None;
    }
    let from = params.first().cloned();
    fn scan(e: &Expr, from: &Option<Symbol>, found: &mut Option<&'static str>) {
        if found.is_some() {
            return;
        }
        if let ExprNode::Assign { target: LValue::Var { name, .. }, .. }
        | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } = &*e.node
        {
            if from.as_ref() == Some(name) {
                *found = Some("assigns to its own parameter");
                return;
            }
        }
        e.node.for_each_child(&mut |c| scan(c, from, found));
    }
    let mut found = None;
    scan(body, &from, &mut found);
    found
}

fn rename_format_binding(block: &Expr) -> Expr {
    let ExprNode::Lambda { params, .. } = &*block.node else { return block.clone() };
    // `respond_to do _1.html … end` / `do it.html … end`: no declared
    // parameter, the implicit one is read as `_1` or `it`.
    let implicit = params.is_empty();
    let from = match params.first() {
        Some(first) if first.as_str() == "format" => return block.clone(),
        Some(first) => first.clone(),
        None => Symbol::from(""),
    };
    // The binding as the arm calls read it: `f` / `wants`, or the implicit
    // `_1` / `it` (a bare send, no receiver, no arguments).
    fn is_binding(e: &Expr, from: &Symbol, implicit: bool) -> bool {
        match &*e.node {
            ExprNode::Var { name, .. } => {
                if implicit { matches!(name.as_str(), "_1" | "it") } else { name == from }
            }
            ExprNode::Send { recv: None, method, args, block, .. } => {
                implicit && args.is_empty() && block.is_none() && matches!(method.as_str(), "_1" | "it")
            }
            _ => false,
        }
    }
    fn rename(e: &mut Expr) {
        *e.node = ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("format") };
    }
    // Only the arm calls (`f.html { … }`) and their bodies are rewritten.
    // A block nested inside an arm that declares the same name (`rows.map
    // { |f| f.id }`) or, in the implicit form, any nested block (its own
    // `it` / `_1` is not ours) keeps its binding.
    fn walk(e: &mut Expr, from: &Symbol, implicit: bool) {
        if let ExprNode::Lambda { params, rest_param, extra_params, block_param, .. } = &*e.node {
            // Any binding the nested block declares: positional, `*rest`,
            // optional / keyword / `**kwrest`, or `&blk`.
            let rebinds = params.contains(from)
                || rest_param.as_ref() == Some(from)
                || block_param.as_ref() == Some(from)
                || extra_params.iter().any(|p| &p.name == from);
            if implicit || rebinds {
                return;
            }
        }
        if is_binding(e, from, implicit) {
            rename(e);
            return;
        }
        let arm_call = matches!(&*e.node, ExprNode::Send { recv: Some(r), .. } if is_binding(r, from, implicit));
        if arm_call {
            if let ExprNode::Send { recv: Some(r), args, block, .. } = &mut *e.node {
                rename(r);
                for a in args.iter_mut() {
                    walk(a, from, implicit);
                }
                if let Some(blk) = block {
                    // The arm's own block: its body, not the Lambda node.
                    if let ExprNode::Lambda { body, .. } = &mut *blk.node {
                        walk(body, from, implicit);
                    } else {
                        walk(blk, from, implicit);
                    }
                }
            }
            return;
        }
        e.node.for_each_child_mut(&mut |c| walk(c, from, implicit));
    }
    let mut copy = block.clone();
    if let ExprNode::Lambda { params, body, .. } = &mut *copy.node {
        walk(body, &from, implicit);
        if !implicit {
            params[0] = Symbol::from("format");
        }
    }
    copy
}

fn unwrap_respond_to_inner(
    expr: &Expr,
    with_format_dispatch: bool,
    breadth: FormatBreadth,
    ctx: ActionFormats<'_>,
) -> Expr {
    // Top-level `respond_to` with a block — replace the whole Send
    // with its flattened body. This short-circuits the structural
    // recursion so we don't re-enter the respond_to's Send/Lambda
    // children via the generic path.
    if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*expr.node {
        if method.as_str() == "respond_to" {
            if let Some(reason) = rename_conflict(block) {
                report_respond_to_error(
                    "respond_to",
                    expr.span,
                    "block parameter cannot be renamed",
                    format!(
                        "a `respond_to` block whose parameter is not named `format` {reason}; \
                         the lowering reads the arms through `format`, and renamed, that \
                         write would land on the action's own `format` local"
                    ),
                );
                return expr.clone();
            }
            let renamed = rename_format_binding(block);
            let lambda_body = unwrap_lambda(&renamed);
            if with_format_dispatch && breadth.negotiates {
                return flatten_negotiated(lambda_body, breadth, ctx);
            }
            return flatten_respond_to_body(lambda_body, with_format_dispatch, breadth, ctx);
        }
    }
    let recurse = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch, breadth, ctx);
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(&recurse).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: recurse(cond),
            then_branch: recurse(then_branch),
            else_branch: recurse(else_branch),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(&recurse),
            method: method.clone(),
            args: args.iter().map(&recurse).collect(),
            block: block.as_ref().map(&recurse),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: recurse(left),
            right: recurse(right),
        },
        ExprNode::Lambda { rest_param, extra_params, params, block_param, body, block_style } => ExprNode::Lambda { rest_param: rest_param.clone(), extra_params: extra_params.clone(),
            params: params.clone(),
            block_param: block_param.clone(),
            body: recurse(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: recurse(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: recurse(recv),
                    index: recurse(index),
                },
                other => other.clone(),
            };
            ExprNode::Assign {
                target: new_target,
                value: recurse(value),
            }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(&recurse).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (recurse(k), recurse(v)))
                .collect(),
            kwargs: *kwargs,
        },
        // Literal, Const, Var, Ivar, Apply, Case, Yield, Raise,
        // RescueModifier, StringInterp, Let — no respond_to inside
        // today's fixtures; clone-verbatim. If future fixtures nest
        // respond_to inside these variants the recursion extends
        // here.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
        diagnostic: expr.diagnostic.clone(),
        hint: expr.hint,
        decisions: expr.decisions,
    }
}

/// Flatten the immediate body of a `respond_to` block. Recognized
/// shapes at this level are `Seq` (the `format.html/.json` pair) and
/// `If` (conditional branching to different format pairs); anything
/// else is handled via `flatten_format_pair_or_drop` directly.
///
/// `with_format_dispatch=false` keeps just the html branch (legacy
/// behavior, used by Group 2 emit paths). `true` emits the
/// `if request_format == :json; …; else; …; end` shape.
fn flatten_respond_to_body(
    body: &Expr,
    with_format_dispatch: bool,
    breadth: FormatBreadth,
    ctx: ActionFormats<'_>,
) -> Expr {
    let recurse_outer = |e: &Expr| unwrap_respond_to_inner(e, with_format_dispatch, breadth, ctx);
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut html: Option<Expr> = None;
            let mut json: Option<Expr> = None;
            let mut rss: Option<Expr> = None;
            let mut other: Vec<Expr> = Vec::new();
            for e in exprs {
                match classify_format_stmt(e) {
                    Some((fmt, branch_body)) if fmt.as_str() == "html" => {
                        html = Some(recurse_outer(&branch_body));
                    }
                    // `format.any` answers every format no earlier arm
                    // claimed — for the dispatch that is the html
                    // fallback, unless an explicit `format.html` has it.
                    Some((fmt, branch_body)) if fmt.as_str() == "any" => {
                        if html.is_none() {
                            html = Some(recurse_outer(&branch_body));
                        }
                    }
                    Some((fmt, branch_body)) if fmt.as_str() == "json" => {
                        // Narrow mode preserves only simple `render :sym
                        // [, kwargs]` shapes — others fall through and
                        // effectively drop (the html branch alone covers
                        // the response). Breadth mode (CRuby) keeps ANY
                        // body: inline `render json: <expr>` normalizes
                        // downstream. Group 2 emit doesn't carry the
                        // dispatch (its emitters don't recognize
                        // `request_format`), so we drop unconditionally
                        // when `with_format_dispatch=false`.
                        match json_arm_drop_reason(&branch_body, with_format_dispatch, breadth) {
                            None => json = Some(recurse_outer(&branch_body)),
                            Some(reason) => report_dropped_format_arm("json", e.span, reason),
                        }
                    }
                    Some((fmt, branch_body)) if fmt.as_str() == "rss" => {
                        if breadth.rss {
                            rss = Some(mark_render_format(&recurse_outer(&branch_body), "rss"));
                        } else {
                            report_dropped_format_arm(
                                "rss",
                                e.span,
                                "this tree does not carry the rss dispatch",
                            );
                        }
                    }
                    // Unknown format (e.g. format.xml) — no lowering
                    // models it, so it drops. Ledgered so the gap is
                    // visible rather than inferred from a parity diff.
                    Some((fmt, _)) => {
                        report_dropped_format_arm(fmt.as_str(), e.span, "format not modeled")
                    }
                    None => other.push(recurse_outer(e)),
                }
            }
            build_format_dispatch(html, json, rss, other, body.span)
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: recurse_outer(cond),
                then_branch: flatten_respond_to_body(then_branch, with_format_dispatch, breadth, ctx),
                else_branch: flatten_respond_to_body(else_branch, with_format_dispatch, breadth, ctx),
            },
        ),
        // A single expression at respond_to-body scope — either a
        // lone `format.html`/`format.json`, or some unrelated shape
        // the pass leaves to the generic walker.
        _ => match classify_format_stmt(body) {
            Some((fmt, branch_body)) if matches!(fmt.as_str(), "html" | "any") => {
                recurse_outer(&branch_body)
            }
            Some((fmt, branch_body))
                if fmt.as_str() == "json"
                    && json_arm_drop_reason(&branch_body, with_format_dispatch, breadth).is_none() =>
            {
                build_format_dispatch(
                    None,
                    Some(recurse_outer(&branch_body)),
                    None,
                    Vec::new(),
                    body.span,
                )
            }
            Some((fmt, branch_body)) if fmt.as_str() == "rss" && breadth.rss => {
                build_format_dispatch(
                    None,
                    None,
                    Some(mark_render_format(&recurse_outer(&branch_body), "rss")),
                    Vec::new(),
                    body.span,
                )
            }
            // Lone arm that survived none of the gates above. Same
            // ledger as the Seq path — a `respond_to` whose only arm is
            // json/rss/xml drops to an empty body, which is the most
            // silent shape of all.
            Some((fmt, branch_body)) => {
                let reason = if fmt.as_str() == "json" {
                    json_arm_drop_reason(&branch_body, with_format_dispatch, breadth)
                        .unwrap_or("dropped")
                } else if fmt.as_str() == "rss" {
                    "this tree does not carry the rss dispatch"
                } else {
                    "format not modeled"
                };
                report_dropped_format_arm(fmt.as_str(), body.span, reason);
                Expr::new(body.span, ExprNode::Seq { exprs: vec![] })
            }
            None => recurse_outer(body),
        },
    }
}

/// `respond_to` on a tree that negotiates (`FormatBreadth::negotiates`).
///
/// Rails does not branch on the path suffix with an html default. It
/// registers the arms in source order, then `negotiate_mime`s them
/// against the request's format list (a pinned `.json` / route format,
/// else the parsed `Accept` header, else html): the first requested
/// format that has an arm wins, a bare `*/*` takes the FIRST arm, and a
/// request no arm answers raises `ActionController::UnknownFormat`
/// (406). Measured against actionpack 8.1.3.1 + turbo-rails 2.0.23 in
/// `tests/turbo_stream_respond_to.rs`. So the flattened shape is
///
/// ```ruby
/// if negotiate_format(["html", "turbo_stream"]) == "turbo_stream"
///   <turbo_stream arm>
/// elsif negotiate_format(["html", "turbo_stream"]) == "html"
///   <html arm>
/// else
///   raise ActionController::UnknownFormat
/// end
/// ```
///
/// with `negotiate_format` the runtime's (runtime/ruby/action_controller
/// /base.rb). An arm this tree cannot run is a LOWERING ERROR — never a
/// warning that drops it. A dropped arm answers the html branch to a
/// request that asked for something else, with the strict gate green.
fn flatten_negotiated(body: &Expr, breadth: FormatBreadth, ctx: ActionFormats<'_>) -> Expr {
    let recurse_outer = |e: &Expr| unwrap_respond_to_inner(e, true, breadth, ctx);
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let mut arms: Vec<NegotiatedArm> = Vec::new();
            let mut other: Vec<Expr> = Vec::new();
            let mut branched = false;
            let direct_arms = exprs.iter().any(|e| matches!(classify_negotiated_stmt(e, true), NegotiatedStmt::Arm(..)));
            for e in exprs {
                match classify_negotiated_stmt(e, false) {
                    NegotiatedStmt::Arm(fmt, arm_body) => {
                        register_arm(&mut arms, fmt.as_str(), arm_body, e.span)
                    }
                    NegotiatedStmt::Refused => {}
                    NegotiatedStmt::Other => other.push(recurse_outer(e)),
                    // `if c; format.html …; else; format.html …; end` after
                    // other statements: Rails registers whichever branch
                    // ran, so the branch is its own negotiation, exactly as
                    // when the `if` is the whole block.
                    NegotiatedStmt::Branches(_) if !direct_arms => {
                        branched = true;
                        other.push(flatten_negotiated(e, breadth, ctx))
                    }
                    NegotiatedStmt::Branches(true) => report_respond_to_error(
                        "format.<arm>",
                        e.span,
                        "arms in a branch beside direct arms",
                        "a `respond_to` block that registers arms inside an if/else AND directly \
                         cannot be lowered: the lowering negotiates one arm set per branch"
                            .to_string(),
                    ),
                    NegotiatedStmt::Branches(false) => report_respond_to_error(
                        "format.<arm>",
                        e.span,
                        "not a direct arm",
                        "a `respond_to` arm registered conditionally (`format.json { … } if cond`) \
                         beside direct arms cannot be lowered; arms must be direct statements of \
                         the block"
                            .to_string(),
                    ),
                }
            }
            // A path that registers no arm at all (`if ok; format.html …;
            // end` with no else, an else that only `head`s) ends in
            // UnknownFormat: the collector is empty, so Rails raises after
            // whatever the path ran.
            if arms.is_empty() && !branched {
                other.push(raise_unknown_format(body.span));
            }
            build_negotiated_dispatch(arms, other, body.span, breadth, ctx)
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: recurse_outer(cond),
                then_branch: flatten_negotiated(then_branch, breadth, ctx),
                else_branch: flatten_negotiated(else_branch, breadth, ctx),
            },
        ),
        _ => match classify_negotiated_stmt(body, false) {
            NegotiatedStmt::Arm(fmt, arm_body) => {
                let mut arms = Vec::new();
                register_arm(&mut arms, fmt.as_str(), arm_body, body.span);
                build_negotiated_dispatch(arms, Vec::new(), body.span, breadth, ctx)
            }
            NegotiatedStmt::Refused | NegotiatedStmt::Branches(_) => {
                Expr::new(body.span, ExprNode::Seq { exprs: vec![] })
            }
            NegotiatedStmt::Other => Expr::new(
                body.span,
                ExprNode::Seq { exprs: vec![recurse_outer(body), raise_unknown_format(body.span)] },
            ),
        },
    }
}

/// What one statement of a negotiated `respond_to` block is.
enum NegotiatedStmt {
    /// A direct `format.<x> [{ … }]` registration.
    Arm(Symbol, Expr),
    /// A registration this lowering cannot honor; an error is reported.
    Refused,
    /// Anything else: ordinary code that runs before negotiation.
    Other,
    /// An `if` with arms in a branch (`true`: in both).
    Branches(bool),
}

/// `classify_format_stmt`, plus the registrations that look like arms
/// but are not: `format.any(:json, :turbo_stream) { … }` (the mime list
/// would be dropped and the arm answer formats it never named) and an
/// arm registered conditionally (`format.json { … } if cond`, or inside
/// any other construct), which would otherwise be emitted verbatim with
/// `format` unbound. Both are errors, never silent.
fn classify_negotiated_stmt(e: &Expr, quiet: bool) -> NegotiatedStmt {
    if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node {
        if is_format_binding(recv) {
            if method.as_str() == "any" && !args.is_empty() {
                if !quiet {
                    report_unrunnable_arm(
                    "any",
                    e.span,
                    "a mime list (`format.any(:json, :turbo_stream)`) would be dropped, and the \
                     arm would answer formats it never named",
                );
                }
                return NegotiatedStmt::Refused;
            }
            if let Some((fmt, body)) = classify_format_stmt(e) {
                return NegotiatedStmt::Arm(fmt, body);
            }
        }
    }
    if mentions_format_binding(e) {
        if let ExprNode::If { then_branch, else_branch, .. } = &*e.node {
            return NegotiatedStmt::Branches(
                mentions_format_binding(then_branch) && mentions_format_binding(else_branch),
            );
        }
        if quiet {
            return NegotiatedStmt::Refused;
        }
        report_respond_to_error(
            "format.<arm>",
            e.span,
            "not a direct arm",
            "a `respond_to` arm registered conditionally (`format.json { … } if cond`) or inside \
             another construct cannot be lowered; arms must be direct statements of the block"
                .to_string(),
        );
        return NegotiatedStmt::Refused;
    }
    NegotiatedStmt::Other
}

/// True when `e` calls a method on the `respond_to` block's `format`.
fn mentions_format_binding(e: &Expr) -> bool {
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(recv), .. } = &*e.node {
            if is_format_binding(recv) {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    let mut found = false;
    walk(e, &mut found);
    found
}

/// One `format.<x>` registration, in source order.
struct NegotiatedArm {
    fmt: String,
    body: Expr,
    span: Span,
}

/// Rails' `Collector#custom` keeps the FIRST registration of a mime
/// type (`@responses[mime_type] ||= …`), so a repeated arm is dead code.
fn register_arm(arms: &mut Vec<NegotiatedArm>, fmt: &str, body: Expr, span: Span) {
    if arms.iter().any(|a| a.fmt == fmt) {
        return;
    }
    arms.push(NegotiatedArm { fmt: fmt.to_string(), body, span });
}

fn build_negotiated_dispatch(
    arms: Vec<NegotiatedArm>,
    other: Vec<Expr>,
    span: Span,
    breadth: FormatBreadth,
    ctx: ActionFormats<'_>,
) -> Expr {
    if arms.is_empty() {
        return Expr::new(span, ExprNode::Seq { exprs: other });
    }
    let order: Vec<String> = arms.iter().map(|a| a.fmt.clone()).collect();
    let outcomes: Vec<(String, Span, ArmOutcome)> = arms
        .iter()
        .map(|a| (a.fmt.clone(), a.span, lower_negotiated_arm(a, breadth, ctx)))
        .collect();
    // The html arm (or `format.any`, which answers html when nothing
    // else claims it) is what a json arm the tree cannot encode falls
    // back to — the behavior the warning on that arm has always named.
    let html_body: Option<Expr> = outcomes.iter().find_map(|(fmt, _, o)| match o {
        ArmOutcome::Body(b) if fmt == "html" => Some(b.clone()),
        _ => None,
    });
    let html_body = html_body.or_else(|| {
        outcomes.iter().find_map(|(fmt, _, o)| match o {
            ArmOutcome::Body(b) if fmt == "any" => Some(b.clone()),
            _ => None,
        })
    });
    // Innermost first: whatever no arm answers is UnknownFormat.
    let mut chain = raise_unknown_format(span);
    for (fmt, arm_span, outcome) in outcomes.into_iter().rev() {
        let body = match outcome {
            ArmOutcome::Body(b) => b,
            ArmOutcome::Refused => continue,
            ArmOutcome::FallsBackToHtml(reason) => match &html_body {
                Some(h) => {
                    report_dropped_format_arm(&fmt, arm_span, reason);
                    h.clone()
                }
                None => {
                    report_unrunnable_arm(&fmt, arm_span, reason);
                    continue;
                }
            },
        };
        chain = Expr::new(
            span,
            ExprNode::If {
                cond: negotiated_format_is(span, &order, &fmt),
                then_branch: body,
                else_branch: chain,
            },
        );
    }
    if other.is_empty() {
        chain
    } else {
        let mut all = other;
        all.push(chain);
        Expr::new(span, ExprNode::Seq { exprs: all })
    }
}

/// What lowering one arm produced.
enum ArmOutcome {
    /// The arm's body.
    Body(Expr),
    /// The tree cannot run this arm and an error has been reported.
    Refused,
    /// A json arm whose body needs an encoder this tree lacks. Its
    /// requests are answered by the html arm and the drop is ledgered as
    /// a warning, exactly as before negotiation existed: `render json:
    /// @article.errors` is every Rails scaffold's failure arm, the shared
    /// runtime's `errors` is a flat Array rather than ActiveModel's
    /// Hash-by-attribute, and an error here would fail the strict gate
    /// for all of them. Only json takes this road.
    FallsBackToHtml(&'static str),
}

/// The body one arm runs, or why this tree cannot run it.
fn lower_negotiated_arm(arm: &NegotiatedArm, breadth: FormatBreadth, ctx: ActionFormats<'_>) -> ArmOutcome {
    let recurse_outer = |e: &Expr| unwrap_respond_to_inner(e, true, breadth, ctx);
    let bare = matches!(&*arm.body.node, ExprNode::Seq { exprs } if exprs.is_empty());
    let or_refused = |e: Option<Expr>| e.map_or(ArmOutcome::Refused, ArmOutcome::Body);
    match arm.fmt.as_str() {
        // `format.html` with no block renders the action's html template
        // through the implicit render that follows; `format.any` runs
        // whatever it was given for every format no arm above claimed.
        "html" => ArmOutcome::Body(html_arm_body(&recurse_outer(&arm.body), bare, arm.span, ctx)),
        "any" => ArmOutcome::Body(recurse_outer(&arm.body)),
        "json" => {
            if bare {
                return or_refused(implicit_template_arm("json", arm.span, ctx));
            }
            match negotiated_json_drop_reason(&arm.body, breadth) {
                None => ArmOutcome::Body(mark_render_format(&recurse_outer(&arm.body), "json")),
                Some(reason) => ArmOutcome::FallsBackToHtml(reason),
            }
        }
        "rss" if breadth.rss => {
            ArmOutcome::Body(mark_render_format(&recurse_outer(&arm.body), "rss"))
        }
        "turbo_stream" => {
            if bare {
                return or_refused(implicit_template_arm("turbo_stream", arm.span, ctx));
            }
            let marked = mark_render_format(&recurse_outer(&arm.body), "turbo_stream");
            if has_toplevel_terminal(&marked) {
                return ArmOutcome::Body(marked);
            }
            if contains_terminal(&marked) {
                report_unrunnable_arm(
                    "turbo_stream",
                    arm.span,
                    "a response that only some paths of the block perform; the arm needs a final \
                     render, or none at all",
                );
                return ArmOutcome::Refused;
            }
            // Side effects only (`flash.now[:notice] = …`): the block
            // performed nothing, so Rails' `default_render` renders the
            // action's turbo_stream template after it.
            match implicit_template_arm("turbo_stream", arm.span, ctx) {
                Some(render) => ArmOutcome::Body(guarded_default_render(&marked, render, arm.span)),
                None => ArmOutcome::Refused,
            }
        }
        other => {
            let reason = if other == "rss" {
                "this tree does not carry the rss dispatch"
            } else {
                "format not modeled"
            };
            report_unrunnable_arm(other, arm.span, reason);
            ArmOutcome::Refused
        }
    }
}

/// The html arm, finished the way Rails finishes it.
///
/// `respond_to` pins the template lookup to the arm that ran, so an html
/// arm that does not respond renders the action's HTML template, never
/// whichever template the implicit render behind the chain would have
/// negotiated for the same `Accept` (a Turbo list names turbo_stream
/// first). The arm therefore ends in its own default render:
///
/// - the action has an html template: `render :<action>`, guarded by
///   `performed?` when the block may already have responded;
/// - it has none but another format's template exists: Rails'
///   `default_render` raises `UnknownFormat` (406), and so does the arm,
///   rather than serve the other template;
/// - no template at all: unchanged (the implicit tail's `head
///   :no_content`).
///
/// An arm outside a public action (`ctx.action` is `None`) is left as
/// written; the unrunnable bare arms of that shape are refused elsewhere.
fn html_arm_body(lowered: &Expr, bare: bool, span: Span, ctx: ActionFormats<'_>) -> Expr {
    let Some(action) = ctx.action else { return lowered.clone() };
    if !bare && has_toplevel_terminal(lowered) {
        return lowered.clone();
    }
    let default = if ctx.html_template {
        render_symbol_send(action, span)
    } else if !ctx.variants.is_empty() {
        raise_unknown_format(span)
    } else {
        return lowered.clone();
    };
    if bare {
        return default;
    }
    guarded_default_render(lowered, default, span)
}

/// `<lowered>; render <default> unless performed?`: the arm's default
/// render, after whatever the block did. ALWAYS guarded: a block that
/// shows no render or redirect of its own may still call a helper that
/// responds (`format.html { fail_it }` with `fail_it` rendering), and a
/// syntactic search cannot see into the callee. Rails' own default
/// render is `performed?`, not syntax.
fn guarded_default_render(lowered: &Expr, default: Expr, span: Span) -> Expr {
    let guarded = Expr::new(
        span,
        ExprNode::If {
            cond: Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("performed?"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            ),
            then_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
            else_branch: default,
        },
    );
    append_statement(lowered, guarded)
}

/// `render :<action>, format: :<fmt>` for an arm with no body of its
/// own. Rails renders `<action>.<fmt>.erb` (or `.jbuilder`); with no
/// such template a POST answers `head :no_content` and a browser GET
/// raises `MissingExactTemplate`, a split this lowering does not model,
/// so a missing template is refused rather than guessed.
fn implicit_template_arm(fmt: &str, span: Span, ctx: ActionFormats<'_>) -> Option<Expr> {
    let Some(action) = ctx.action else {
        report_unrunnable_arm(
            fmt,
            span,
            "a bare arm renders the processed action's template, and this respond_to is not in a \
             public action body",
        );
        return None;
    };
    if !ctx.variants.contains(&fmt) {
        report_unrunnable_arm(
            fmt,
            span,
            "the action has no template in this format for the bare arm to render",
        );
        return None;
    }
    Some(mark_render_format(&render_symbol_send(action, span), fmt))
}

/// Why a json arm cannot run on a negotiating tree, or `None`.
///
/// The simple shapes keep their rule (`render :sym`, `head`, an already
/// encoded `render plain:`), and CRuby's `json_any` keeps everything.
/// What this adds is the inline `render json: <literal-or-primitive
/// collection>` the tree CAN encode: `rewrite_render_to_views` spells it
/// `JSON.generate`, which every target has. Anything else needs the
/// as_json-aware encoder the AOT trees lack.
fn negotiated_json_drop_reason(body: &Expr, breadth: FormatBreadth) -> Option<&'static str> {
    if json_arm_drop_reason(body, true, breadth).is_none() || is_inline_primitive_json_render(body) {
        return None;
    }
    Some("inline `render json: <expr>` needs an encoder this tree has none for")
}

/// `render json: V[, status: …]` where `V` is a closed collection of
/// primitives, the shape `json_render_encode` writes as `JSON.generate`.
fn is_inline_primitive_json_render(body: &Expr) -> bool {
    let body = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let ExprNode::Send { recv: None, method, args, .. } = &*body.node else { return false };
    if method.as_str() != "render" || args.len() != 1 {
        return false;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return false };
    let mut json_value = None;
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else { return false };
        match value.as_str() {
            "json" => json_value = Some(v),
            // Response options that ride along unchanged.
            "status" | "content_type" | "location" => {}
            _ => return false,
        }
    }
    json_value.is_some_and(crate::lower::controller_to_library::rewrites::json_primitive_value)
}

/// `negotiate_format(["a", "b"]) == "<fmt>"`.
fn negotiated_format_is(span: Span, order: &[String], fmt: &str) -> Expr {
    let str_lit = |s: &str| Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } });
    let negotiate = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("negotiate_format"),
            args: vec![Expr::new(
                span,
                ExprNode::Array {
                    elements: order.iter().map(|f| str_lit(f)).collect(),
                    style: crate::expr::ArrayStyle::Brackets,
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(negotiate),
            method: Symbol::from("=="),
            args: vec![str_lit(fmt)],
            block: None,
            parenthesized: false,
        },
    )
}

/// `raise ActionController::UnknownFormat` — what Rails raises when no
/// registered arm answers the request. The dispatchers map it to 406.
fn raise_unknown_format(span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Raise {
            value: Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(crate::lower::controller_to_library::rewrites::typed_exception_const(
                        &["ActionController", "UnknownFormat"],
                        span,
                    )),
                    method: Symbol::from("new"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            ),
        },
    )
}

/// A `respond_to` construct the negotiating tree cannot run, as an ERROR.
fn report_respond_to_error(construct: &str, span: Span, reason: &'static str, message: String) {
    crate::emit::diagnostics::push(Diagnostic {
        span,
        severity: crate::diagnostic::Severity::Error,
        kind: DiagnosticKind::LowerResidue {
            pass: Symbol::from("respond_to_negotiate"),
            construct: Symbol::from(construct),
            reason: Symbol::from(reason),
        },
        message,
    });
}

/// An arm the negotiating tree cannot run. An ERROR: the arm's absence
/// would not be modeling debt, it would answer the wrong body to a
/// request that asked for this format.
pub(crate) fn report_unrunnable_arm(format: &str, span: Span, reason: &'static str) {
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("respond_to_negotiate"),
        construct: Symbol::from(format!("format.{format}")),
        reason: Symbol::from(reason),
    };
    crate::emit::diagnostics::push(Diagnostic {
        span,
        severity: crate::diagnostic::Severity::Error,
        kind,
        message: format!(
            "`respond_to` arm `format.{format}` cannot be lowered ({reason}); dropping it would \
             answer the html branch to a request that negotiated `{format}`"
        ),
    });
}

/// Ledger a `respond_to` arm that the flattening discarded.
///
/// Dropping an arm is SILENT otherwise, and silence here is expensive:
/// the only downstream evidence is a parity report showing HTML where
/// Rails sent JSON, which reads like a routing bug and sends you to the
/// route ingester. `/hottest` cost exactly that detour — it is
/// `get "/hottest" => "home#index", :format => "json"`, so Rails serves
/// JSON and the emit served the HTML arm, with nothing in the emit log
/// naming the discarded branch.
///
/// Warning, not error: a dropped arm is modeling debt, not a broken
/// build — the html arm still answers the request. The span points at
/// the `respond_to` body, which is enough to name the action.
fn report_dropped_format_arm(format: &str, span: Span, reason: &'static str) {
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("respond_to_flatten"),
        construct: Symbol::from(format!("format.{format}")),
        reason: Symbol::from(reason),
    };
    let d = Diagnostic {
        span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`respond_to` arm `format.{format}` dropped ({reason}) — this action answers \
             every format with its html branch, so a request that negotiates `{format}` \
             (including a route pinned with `:format => \"{format}\"`) gets HTML back"
        ),
    };
    crate::emit::diagnostics::push(d);
}

/// Which `reason` string a json arm was dropped for, or `None` when it
/// survives. Keeping the gate in one place is what stops the predicate
/// and the diagnostic drifting apart — they must agree exactly, or the
/// ledger reports drops that did not happen (or misses ones that did).
fn json_arm_drop_reason(
    branch_body: &Expr,
    with_format_dispatch: bool,
    breadth: FormatBreadth,
) -> Option<&'static str> {
    if !with_format_dispatch {
        return Some("this emit path does not dispatch on request_format");
    }
    // A BARE `format.json` (empty body) renders the action's own JSON
    // template — the simplest render there is, and exactly what a
    // `render :sym` arm would bind.
    let bare = matches!(&*branch_body.node, ExprNode::Seq { exprs } if exprs.is_empty());
    if bare || breadth.json_any || is_simple_render_sym(branch_body) || is_encoded_render(branch_body) {
        return None;
    }
    Some("inline `render json: <expr>` needs an encoder this tree has none for")
}

/// Pull `(format_name, block_body)` out of a `format.<x> { body }`
/// Send. Returns `None` for any statement that isn't a format
/// binding. The bare-form `format.html` (no block) returns an empty
/// Seq body so callers can treat block-form and bare-form uniformly.
fn classify_format_stmt(e: &Expr) -> Option<(Symbol, Expr)> {
    if let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node {
        if is_format_binding(recv) {
            let body = match block.as_ref() {
                Some(b) => unwrap_lambda(b).clone(),
                None => Expr::new(e.span, ExprNode::Seq { exprs: vec![] }),
            };
            return Some((method.clone(), body));
        }
    }
    None
}

/// True when `body` is a shape the json dispatch supports today:
/// either `render :sym [, kwargs]` (simple view-template) or
/// `head :sym` (status-only terminal). Inline renders (`render
/// json: <expr>`) and redirects in error branches don't qualify
/// and the json branch gets dropped (html alone covers the
/// response).
fn is_simple_render_sym(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
        }
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "head" && !args.is_empty() =>
        {
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
                || matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { .. } })
        }
        _ => false,
    }
}

/// True when `body` renders text that is ALREADY encoded —
/// `render plain: <str>, …`. `lower::as_json_poro` respells a `render
/// json:` it could write down this way (`<v>.as_json_str`), so the arm
/// no longer reaches the runtime encoder the narrow breadths exclude and
/// travels like any other plain render.
fn is_encoded_render(body: &Expr) -> bool {
    let body = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let ExprNode::Send { recv: None, method, args, .. } = &*body.node else { return false };
    if method.as_str() != "render" || args.len() != 1 {
        return false;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return false };
    let has = |name: &str| {
        entries.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name)
        })
    };
    has("plain") && !has("json")
}

/// Build the `if request_format == :json; <json>; else; <html>; end`
/// dispatch. Drop branches that are `None`: a missing json branch
/// falls through to html on both paths; a missing html branch
/// (uncommon — the source action defined only `format.json`) uses
/// the same empty Seq on the else.
fn build_format_dispatch(
    html: Option<Expr>,
    json: Option<Expr>,
    rss: Option<Expr>,
    other: Vec<Expr>,
    span: Span,
) -> Expr {
    // Innermost-first: the html branch is the else-default, an rss arm
    // (breadth mode only) wraps it, and the json arm wraps outermost —
    // so the no-rss shape stays byte-identical to the legacy two-way
    // dispatch.
    let mut dispatch = match (&html, &json, &rss) {
        (Some(h), None, None) => h.clone(),
        (None, None, None) => Expr::new(span, ExprNode::Seq { exprs: vec![] }),
        _ => html.unwrap_or_else(|| Expr::new(span, ExprNode::Seq { exprs: vec![] })),
    };
    if let Some(r) = rss {
        dispatch = Expr::new(
            span,
            ExprNode::If {
                cond: request_format_eq(span, "rss"),
                then_branch: r,
                else_branch: dispatch,
            },
        );
    }
    if let Some(j) = json {
        let json_body = mark_render_format(&j, "json");
        dispatch = Expr::new(
            span,
            ExprNode::If {
                cond: request_format_eq(span, "json"),
                then_branch: json_body,
                else_branch: dispatch,
            },
        );
    }
    if other.is_empty() {
        dispatch
    } else {
        let mut all = other;
        all.push(dispatch);
        Expr::new(span, ExprNode::Seq { exprs: all })
    }
}

/// `request_format == :<fmt>` — the predicate every dispatched action
/// branches on. `request_format` is a Base accessor populated by the
/// CGI driver from a path-suffix sniff (`.json` → `:json`). Emitted
/// with an explicit `self` receiver so Group 2 emitters (Elixir,
/// Python, Go, Rust) that distinguish methods from locals at the
/// emit layer route it to the accessor rather than to a bare
/// variable lookup.
fn request_format_eq(span: Span, fmt: &str) -> Expr {
    let recv = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from("request_format"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fmt_sym = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from(fmt),
            },
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("=="),
            args: vec![fmt_sym],
            block: None,
            parenthesized: false,
        },
    )
}

/// Walk `body` and tag terminals with format-aware kwargs:
///   - `render(<sym>, …)` gets a `format: :<fmt>` marker; the kwarg
///     flows into `rewrite_render_to_views`, which strips it and
///     uses it to route to the `<sym>_<fmt>` view + tag the outer
///     render with `content_type: "<mime>"`.
///   - `head(<sym>, …)` gets a `content_type: "<mime>"` kwarg
///     directly. head doesn't go through view rewriting (its body
///     is empty regardless of format), so the lowerer plants the
///     MIME marker here rather than via the render-rewrite path.
fn mark_render_format(body: &Expr, fmt: &str) -> Expr {
    let new_node = match &*body.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if matches!(method.as_str(), "render" | "render_to_string")
            && !args.is_empty() =>
        {
            // `render action: "stories", layout: false` names its
            // template inside the Hash; the marker joins that Hash.
            let names_template = matches!(&*args[0].node, ExprNode::Hash { entries, .. }
                if entries.iter().any(|(k, _)| matches!(&*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } }
                        if matches!(value.as_str(), "action" | "template"))));
            if matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } }) || names_template {
                let new_args = add_format_kwarg(args, fmt, body.span);
                ExprNode::Send {
                    recv: None,
                    method: method.clone(),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: *parenthesized,
                }
            } else {
                return body.clone();
            }
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if method.as_str() == "head" && !args.is_empty() => {
            let new_args = add_content_type_kwarg(args, mime_for_format(fmt), body.span);
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args: new_args,
                block: block.clone(),
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| mark_render_format(e, fmt)).collect(),
        },
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        } => ExprNode::If {
            cond: cond.clone(),
            then_branch: mark_render_format(then_branch, fmt),
            else_branch: mark_render_format(else_branch, fmt),
        },
        // Into assignments and blocks, for a feed branch's
        // `content = Rails.cache.fetch("rss") { render_to_string action:
        // "stories", layout: false }` — the render sits in a block, under
        // an assignment. Only the rss/atom/xml marking walks this far;
        // the json/turbo_stream/svg callers keep their exact reach.
        _ if matches!(fmt, "rss" | "atom" | "xml") => {
            let mut out = body.clone();
            out.node.for_each_child_mut(&mut |c| *c = mark_render_format(c, fmt));
            return out;
        }
        _ => return body.clone(),
    };
    Expr::new(body.span, new_node)
}

/// `head(:no_content)` — Rails' answer when an action falls off the end
/// with no template to render.
fn head_no_content(span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("head"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("no_content") } },
            )],
            block: None,
            parenthesized: true,
        },
    )
}

/// Map a Rails format symbol to its canonical MIME string.
///
/// `pub(crate)` so the render-to-views rewrite tags a format-marked
/// render from the same table this one uses — two copies of a MIME
/// string is how the pair drifts.
pub(crate) fn mime_for_format(fmt: &str) -> &'static str {
    match fmt {
        "json" => "application/json",
        // What Turbo sends in `Accept` and expects back on a form
        // submission it drives.
        "turbo_stream" => "text/vnd.turbo-stream.html; charset=utf-8",
        // campfire renders a user's initials as an SVG avatar; the
        // response is an image and browsers treat it as one only with
        // this type.
        "svg" => "image/svg+xml",
        // A service worker (campfire's raw `pwa/service_worker.js`):
        // browsers refuse to register a script served as anything else.
        "js" => "text/javascript; charset=utf-8",
        // Feeds (Mime::Type's registrations): lobsters' /rss.
        "rss" => "application/rss+xml; charset=utf-8",
        "atom" => "application/atom+xml; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        _ => "text/html; charset=utf-8",
    }
}

/// Append (or merge into) a trailing kwarg-Hash carrying
/// `content_type: "<mime>"`. Used to tag head call sites in the
/// json branch — render call sites take the format-kwarg path so
/// the view-rewrite layer can decide the MIME.
fn add_content_type_kwarg(args: &[Expr], mime: &str, span: Span) -> Vec<Expr> {
    let pair = (
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("content_type"),
                },
            },
        ),
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Str {
                    value: mime.to_string(),
                },
            },
        ),
    );
    let mut out = args.to_vec();
    if let Some(last) = out.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let mut new_entries = entries.clone();
            new_entries.push(pair);
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return out;
        }
    }
    out.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![pair],
            kwargs: true,
        },
    ));
    out
}

/// Append (or merge into) a trailing kwarg-Hash carrying `format:
/// :<fmt>`. Render call args have the shape `[symbol, ...kwarg_hash?]`
/// — if a trailing Hash already exists we merge `format:` into it;
/// otherwise we append a new kwarg Hash.
fn add_format_kwarg(args: &[Expr], fmt: &str, span: Span) -> Vec<Expr> {
    let fmt_pair = (
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("format"),
                },
            },
        ),
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from(fmt),
                },
            },
        ),
    );
    let mut out = args.to_vec();
    if let Some(last) = out.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let mut new_entries = entries.clone();
            new_entries.push(fmt_pair);
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return out;
        }
    }
    out.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![fmt_pair],
            kwargs: true,
        },
    ));
    out
}

/// Append a synthesized `render :<action_name>` Send to `body` when
/// `body` has no top-level render / redirect_to / head terminal.
/// Encodes the Rails convention that an action falling off the end
/// renders its eponymous view.
///
/// When `has_json_variant` is true, the synthesized render expands
/// to a format dispatch — `if request_format == :json; render
/// :<action>, format: :json; else; render :<action>; end` — so
/// requests with a stripped `.json` suffix render the
/// `<action>.json.jbuilder` template. Without a json variant the
/// dispatch would reference an undefined `<action>_json` view at
/// emit time, so we only synthesize it when the variant exists.
///
/// Target-neutral — every emitter walking the result sees an explicit
/// terminal that `classify_controller_send` resolves to `Render`.
/// Before this pass, each scaffold template synthesized the terminal
/// ad-hoc at emit time; after, the walker path needs no special case.
/// `variants` are the NON-html formats this action has a template for,
/// in the order they should be tested. Each becomes a
/// `request_format == :<fmt>` arm ahead of the html fallback, so an
/// action with both a jbuilder and a `.turbo_stream.erb` template gets
/// both. Empty means html only.
pub fn synthesize_implicit_render(body: &Expr, action_name: &str, variants: &[&str]) -> Expr {
    synthesize_implicit_render_with_html(body, action_name, variants, true, true)
}

/// `html_template_exists = false` makes the HTML fallback `head
/// :no_content` instead of a render that would resolve to nothing.
///
/// Rails' `default_render` raises `MissingTemplate` only for an EXPLICIT
/// `render :foo`; falling off the end of an action with no template
/// logs "No template found … rendering head :no_content" and returns
/// 204. We raised in both cases, so campfire's `Messages::Boosts#destroy`
/// — a turbo_stream DELETE with no template, asserting `:success` —
/// died on a template Rails never looks for.
///
/// The explicit path keeps raising, which is what lobsters' about/privacy
/// actions rescue as their normal flow.
pub fn synthesize_implicit_render_with_html(
    body: &Expr,
    action_name: &str,
    variants: &[&str],
    html_template_exists: bool,
    html_exists: bool,
) -> Expr {
    if has_toplevel_terminal(body) {
        return body.clone();
    }
    // Build the chain inside-out so the FIRST variant ends up as the
    // outermost test.
    let mut terminal =
        html_fallback(action_name, body.span, variants, html_template_exists, html_exists);
    for fmt in variants.iter().rev() {
        let branch = mark_render_format(&render_symbol_send(action_name, body.span), fmt);
        terminal = Expr::new(
            body.span,
            ExprNode::If {
                cond: request_format_eq(body.span, fmt),
                then_branch: branch,
                else_branch: terminal,
            },
        );
    }
    // A body with SOME response terminal that isn't guaranteed at top
    // level (a render inside begin/rescue, or behind a condition the
    // detector can't prove) may already have responded by the time the
    // synthesized default runs — Rails' own default-render check is
    // `performed?`, not syntax. Guard the synthesized render the same
    // way. Bodies with no terminal at all (the common case — every
    // conventional index/show) keep the bare unguarded shape.
    let terminal = if contains_terminal(body) {
        Expr::new(
            body.span,
            ExprNode::If {
                cond: Expr::new(
                    body.span,
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("performed?"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                ),
                then_branch: Expr::new(body.span, ExprNode::Lit { value: Literal::Nil }),
                else_branch: terminal,
            },
        )
    } else {
        terminal
    };
    append_statement(body, terminal)
}

/// What the implicit render does when no variant arm matched — the
/// request is html as far as the arms can tell.
///
/// - An html template exists: render it.
/// - Some other format's template exists (`any_template_exists`):
///   Rails' `default_render` renders a template when one exists for a
///   format the request accepts. A bare `Accept: */*` — an
///   `XMLHttpRequest` or `fetch` that set none, campfire's attachment
///   uploader — accepts every format, so the action's other template
///   renders (`create.turbo_stream.erb` answering an upload, which
///   Rails serves 200). Chosen in Rails' Mime registration order, js
///   before json before turbo_stream. Asked of the controller as
///   `accepts_any_format`, which the dispatcher sets. Any other request
///   keeps the render that resolves to MissingTemplate — Rails'
///   UnknownFormat, approximated (see `controller_to_library`).
/// - No template at all: `head :no_content`, Rails' "No template found".
fn html_fallback(
    action_name: &str,
    span: Span,
    variants: &[&str],
    any_template_exists: bool,
    html_exists: bool,
) -> Expr {
    if html_exists {
        return render_symbol_send(action_name, span);
    }
    if !any_template_exists {
        return head_no_content(span);
    }
    let any_fmt = ["js", "json", "turbo_stream"]
        .into_iter()
        .find(|f| variants.contains(f));
    let Some(fmt) = any_fmt else {
        return render_symbol_send(action_name, span);
    };
    Expr::new(
        span,
        ExprNode::If {
            cond: Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("accepts_any_format"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            ),
            then_branch: mark_render_format(&render_symbol_send(action_name, span), fmt),
            else_branch: render_symbol_send(action_name, span),
        },
    )
}

/// The implicit render as a STANDALONE statement, for an action whose
/// default render has to run in the dispatcher instead of at the end of
/// its own body (see `actions_reached_by_super`).
///
/// ALWAYS guarded on `performed?`, where the in-body form guards only
/// when the body already contains some terminal. The whole reason this
/// variant exists is that something else may have responded first — a
/// subclass that called `super` and then `head :created` — so the guard
/// is the point rather than an optimization.
///
/// Returns `None` when the action's own body always responds; there is
/// no default render to run then, and emitting a dead guarded branch
/// into every dispatcher arm would be noise.
/// Append an ALWAYS-guarded default render, for an action whose tail is
/// about to be moved into the dispatcher.
///
/// `synthesize_implicit_render` guards on `performed?` only when the
/// body already contains some terminal — a sound optimization when the
/// tail stays put, and WRONG here. The reason this tail moves is that
/// something else may respond first: a subclass that called `super` and
/// then `head :created`. Without the guard the dispatcher renders over
/// the response the subclass just produced, and a body with no terminal
/// of its own (the common shape) is exactly the case that lost it.
pub fn synthesize_deferred_implicit_render(
    body: &Expr,
    action_name: &str,
    variants: &[&str],
    html_template_exists: bool,
    html_exists: bool,
) -> Expr {
    match implicit_render_statement(body, action_name, variants, html_template_exists, html_exists) {
        Some(tail) => append_statement(body, tail),
        None => body.clone(),
    }
}

pub fn implicit_render_statement(
    body: &Expr,
    action_name: &str,
    variants: &[&str],
    html_template_exists: bool,
    html_exists: bool,
) -> Option<Expr> {
    if has_toplevel_terminal(body) {
        return None;
    }
    let mut terminal =
        html_fallback(action_name, body.span, variants, html_template_exists, html_exists);
    for fmt in variants.iter().rev() {
        let branch = mark_render_format(&render_symbol_send(action_name, body.span), fmt);
        terminal = Expr::new(
            body.span,
            ExprNode::If {
                cond: request_format_eq(body.span, fmt),
                then_branch: branch,
                else_branch: terminal,
            },
        );
    }
    Some(Expr::new(
        body.span,
        ExprNode::If {
            cond: Expr::new(
                body.span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("performed?"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            ),
            then_branch: Expr::new(body.span, ExprNode::Lit { value: Literal::Nil }),
            else_branch: terminal,
        },
    ))
}

/// True when a response terminal (`render` / `redirect_to` / `head` /
/// `respond_to`-with-block) appears ANYWHERE in the body — the signal
/// that the synthesized default render needs a `performed?` guard (see
/// `synthesize_implicit_render`). Deliberately broader than
/// `has_toplevel_terminal`: that one proves a response always happens;
/// this one detects that a response MIGHT already have happened.
/// The sends that RESPOND — after one of these the action has produced
/// its body, and a synthesized default render would write over it.
///
/// `send_data` and `send_file` belong here for exactly the same reason
/// `render` does: both set the body and mark the response performed.
/// Leaving them out is not a missing feature, it is a WRONG one —
/// campfire's account logo picks a stock icon through a two-hop private
/// helper (`send_stock_icon` -> `send_png_file` -> `send_file`), and the
/// unguarded tail then ran `head :no_content` over the PNG that helper
/// had just written. The response was a 204 with an empty body, and the
/// test failed decoding it: "buffer is not in a known format", which
/// names the image library and nothing about the terminal.
const RESPONSE_TERMINALS: &[&str] = &[
    "render",
    "redirect_to",
    "redirect_back_or_to",
    "head",
    "send_data",
    "send_file",
    "render_inertia_json",
];

/// Rails' HTTP auth helpers that render the 401 challenge when the
/// credentials are missing or refused. They MIGHT respond, so a filter
/// calling one needs the preamble's halting check and an action calling
/// one a guarded default render; they are not in `RESPONSE_TERMINALS`
/// because on success they leave the response to the action, so
/// `has_toplevel_terminal` must not count them.
pub const HTTP_AUTH_CHALLENGES: &[&str] = &[
    "authenticate_or_request_with_http_basic",
    "authenticate_or_request_with_http_token",
    "request_http_basic_authentication",
    "request_http_token_authentication",
];

/// One statement that writes this controller's response: a send in
/// `RESPONSE_TERMINALS`, or an assignment to `self.response_body` (Rails
/// counts an assigned body as performed; campfire's MessagesController
/// and CachedResponses serve a prebuilt page that way). Shared by the
/// implicit-render guard here and the filter halting check in
/// `controller_to_library`, so the two cannot disagree about what a
/// response is.
pub fn is_response_terminal(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { recv: None, method, .. } => RESPONSE_TERMINALS.contains(&method.as_str()),
        // `self.render …` / `self.redirect_to …` are the same terminals.
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            matches!(&*r.node, ExprNode::SelfRef)
                && (RESPONSE_TERMINALS.contains(&method.as_str())
                    || (method.as_str() == "response_body="
                        && !args.first().is_some_and(is_nil_literal)))
        }
        // `self.response_body = nil` clears the body and performs
        // nothing, so the action's implicit render still runs.
        ExprNode::Assign { target: crate::expr::LValue::Attr { recv, name }, value } => {
            matches!(&*recv.node, ExprNode::SelfRef)
                && name.as_str() == "response_body"
                && !is_nil_literal(value)
        }
        _ => false,
    }
}

fn is_nil_literal(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

fn contains_terminal(body: &Expr) -> bool {
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if is_response_terminal(e) {
            *found = true;
            return;
        }
        if let ExprNode::Send { recv: None, method, block, .. } = &*e.node {
            if HTTP_AUTH_CHALLENGES.contains(&method.as_str())
                || (method.as_str() == "respond_to" && block.is_some())
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    let mut found = false;
    walk(body, &mut found);
    found
}

/// True when `body` is guaranteed to hit a response-terminal
/// (`render` / `redirect_to` / `head` / `respond_to`) at its top
/// level — including every branch of the final if/else, since both
/// branches must terminate for the action to have a response. A
/// `respond_to` block counts as terminal because the emitter's
/// SendKind render table expands it into per-format terminals.
pub fn has_toplevel_terminal(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().map_or(false, has_toplevel_terminal),
        ExprNode::Send { recv: None, method, block, .. }
            if method.as_str() == "respond_to" && block.is_some() => true,
        ExprNode::If { then_branch, else_branch, .. } => {
            has_toplevel_terminal(then_branch) && has_toplevel_terminal(else_branch)
        }
        // A `raise` leaves the action: nothing falls off the end after
        // it, so no default render belongs behind it. The lowered
        // `respond_to` ends its arms in `raise UnknownFormat`.
        ExprNode::Raise { .. } => true,
        _ => is_response_terminal(body),
    }
}

/// Build a synthetic `render :<name>` Send with the given span.
/// Used by `synthesize_implicit_render`; span is inherited from the
/// containing body so diagnostics / effect annotations point at a
/// meaningful location rather than a free-floating synthetic span.
fn render_symbol_send(action_name: &str, span: crate::span::Span) -> Expr {
    let sym = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym { value: Symbol::from(action_name) },
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("render"),
            args: vec![sym],
            block: None,
            parenthesized: false,
        },
    )
}

/// Append `tail` as the final statement of `body`. If `body` is
/// already a `Seq`, the result is a `Seq` with one more element;
/// otherwise the result wraps both in a new `Seq`.
fn append_statement(body: &Expr, tail: Expr) -> Expr {
    let mut exprs = match &*body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![body.clone()],
    };
    exprs.push(tail);
    Expr::new(body.span, ExprNode::Seq { exprs })
}

/// Apply the full pre-emit normalization pipeline to an action
/// body — the canonical three-pass sequence every target emitter
/// runs verbatim before walking. Returns a new `Expr`; the input
/// body is untouched.
///
///   1. `resolve_before_actions` — inline `before_action` callback
///      bodies into each action that uses them.
///   2. `unwrap_respond_to` — flatten `respond_to { format.html {…}
///      format.json {…} }` blocks to just their HTML branch.
///   3. `synthesize_implicit_render` — append `render :<action>`
///      when the body has no explicit response terminal.
///
/// Per-target ivar/params rewrites happen AFTER this pipeline
/// (e.g. TS's `rewrite_for_controller`), since the rewrite shape
/// differs between targets (JS-friendly `context.params.k` vs
/// Rust's axum-extractor locals).
pub fn normalize_action_body(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
) -> Expr {
    let with_callbacks = resolve_before_actions(controller, action_name, body);
    let flattened = unwrap_respond_to(&with_callbacks);
    synthesize_implicit_render(&flattened, action_name, /*variants=*/ &[])
}

/// True when `body` is an empty `Seq` or a `nil` literal — the two
/// shapes every walker needs to recognize so `if cond; A; end` with
/// no else-branch doesn't emit a spurious empty `else { }` block.
pub fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: Literal::Nil })
}
