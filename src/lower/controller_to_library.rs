//! Lower a Rails-shape `Controller` into a post-lowering `LibraryClass`
//! whose body is a flat sequence of `MethodDef`s — the universal IR
//! shape every emitter consumes (see
//! `project_universal_post_lowering_ir.md`).
//!
//! The output target is `fixtures/spinel-blog/app/controllers/<name>.rb`:
//! a synthesized `process_action(action_name)` dispatcher that
//! conditionally invokes before-action filters and case-dispatches to
//! per-action methods, plus the public actions and the private filter
//! targets as ordinary methods.
//!
//! What this pass does NOT do (each is a separate follow-on lowerer):
//!
//! - Action-body rewrites: `params` → `@params`, `flash` → `@flash`,
//!   polymorphic `redirect_to @x` → `redirect_to(RouteHelpers.x_path(...))`,
//!   `Article.includes(:foo).order(...)` → `.all` + in-memory sort.
//! - Implicit-render synthesis: spinel actions all carry explicit
//!   `render(Views::...)` calls; this lowering just unwraps any
//!   `respond_to` wrappers and trusts the body otherwise.
//!
//! The skeleton landed first because it surfaces the dispatcher shape
//! (the structural piece tests can pin down) without requiring every
//! body-level rewrite to be wired up at once. Body rewrites layer on
//! top by transforming each action's `body` Expr before it's hung off
//! the synthesized `MethodDef`.

use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass, MethodDef,
    MethodReceiver,
};
use crate::effect::EffectSet;
use crate::expr::{Arm, ArrayStyle, Expr, ExprNode, InterpPart, LValue, Literal, Pattern, RescueClause};
use crate::ident::{Symbol, VarId};
use crate::lower::controller::body::unwrap_respond_to;
use crate::span::Span;

/// Entry point: take a `Controller` (Rails-shape, with filters +
/// actions in `body`) and produce the post-lowering `LibraryClass`.
pub fn lower_controller_to_library_class(controller: &Controller) -> LibraryClass {
    let mut methods: Vec<MethodDef> = Vec::new();

    let (publics, privs) = split_public_private_actions(controller);
    let before_filters: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .collect();

    if !publics.is_empty() || !before_filters.is_empty() {
        methods.push(synthesize_process_action(&before_filters, &publics));
    }

    for a in &publics {
        methods.push(action_to_method(a));
    }
    for a in &privs {
        methods.push(action_to_method(a));
    }

    LibraryClass {
        name: controller.name.clone(),
        is_module: false,
        parent: controller.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

/// Walk the controller body in source order, partitioning actions at
/// the `private` marker. Filters and unknown class-body statements are
/// dropped here — filters get re-synthesized into `process_action`,
/// unknowns (e.g. `allow_browser`) carry no semantics in spinel.
fn split_public_private_actions(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

/// Build the `process_action(action_name)` dispatcher:
///
/// ```ruby
/// def process_action(action_name)
///   set_article if [:show, :edit, ...].include?(action_name)
///   case action_name
///   when :index then index
///   when :new then new_action
///   ...
///   end
/// end
/// ```
fn synthesize_process_action(filters: &[&Filter], publics: &[Action]) -> MethodDef {
    let mut stmts: Vec<Expr> = Vec::new();

    for f in filters {
        stmts.push(filter_dispatch_stmt(f));
    }

    if !publics.is_empty() {
        stmts.push(case_dispatch(publics));
    }

    let body = match stmts.len() {
        0 => syn(ExprNode::Seq { exprs: vec![] }),
        1 => stmts.into_iter().next().unwrap(),
        _ => syn(ExprNode::Seq { exprs: stmts }),
    };

    MethodDef {
        name: Symbol::from("process_action"),
        receiver: MethodReceiver::Instance,
        params: vec![Symbol::from("action_name")],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: None,
    }
}

/// `set_X if [:a, :b, ...].include?(action_name)` — or unconditionally
/// (no filter `only:` / `except:`) just `set_X`.
fn filter_dispatch_stmt(f: &Filter) -> Expr {
    let target_call = syn(ExprNode::Send {
        recv: None,
        method: f.target.clone(),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    if f.only.is_empty() && f.except.is_empty() {
        return target_call;
    }
    let cond = include_check(&f.only, &f.except);
    syn(ExprNode::If {
        cond,
        then_branch: target_call,
        else_branch: empty_seq(),
    })
}

/// `[:a, :b].include?(action_name)` — or for `except:`,
/// `![:a, :b].include?(action_name)` (we pass the list through `not`
/// upstream; this helper just builds the include? form).
fn include_check(only: &[Symbol], except: &[Symbol]) -> Expr {
    let (syms, negate) = if !only.is_empty() {
        (only, false)
    } else {
        (except, true)
    };
    let array = syn(ExprNode::Array {
        elements: syms.iter().map(|s| sym_lit(s.as_str())).collect(),
        style: ArrayStyle::Brackets,
    });
    let include = syn(ExprNode::Send {
        recv: Some(array),
        method: Symbol::from("include?"),
        args: vec![var_ref("action_name")],
        block: None,
        parenthesized: true,
    });
    if negate {
        syn(ExprNode::Send {
            recv: Some(include),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    } else {
        include
    }
}

/// `case action_name; when :foo then foo; ...; end` — one arm per
/// public action. The `:new` action dispatches to `new_action` (Ruby
/// `def new` would shadow `Object#new`).
fn case_dispatch(publics: &[Action]) -> Expr {
    let arms: Vec<Arm> = publics
        .iter()
        .map(|a| {
            let action_name = a.name.as_str();
            let method_name = method_name_for_action(action_name);
            Arm {
                pattern: Pattern::Lit {
                    value: Literal::Sym { value: Symbol::from(action_name) },
                },
                guard: None,
                body: syn(ExprNode::Send {
                    recv: None,
                    method: Symbol::from(method_name),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                }),
            }
        })
        .collect();
    syn(ExprNode::Case {
        scrutinee: var_ref("action_name"),
        arms,
    })
}

/// Convert one `Action` into a `MethodDef`. Renames `new` →
/// `new_action` (Ruby `def new` would shadow `Object#new`); applies
/// `unwrap_respond_to` to drop format dispatch and `rewrite_params`
/// to lower `params` references to the `@params` ivar shape spinel
/// expects.
fn action_to_method(a: &Action) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    let params: Vec<Symbol> = a.params.fields.iter().map(|(n, _)| n.clone()).collect();
    let unwrapped = unwrap_respond_to(&a.body);
    let body = rewrite_params(&unwrapped);
    MethodDef {
        name: Symbol::from(method_name),
        receiver: MethodReceiver::Instance,
        params,
        body,
        signature: None,
        effects: a.effects.clone(),
        enclosing_class: None,
    }
}

/// Action name → Ruby method name. `new` is the only rename (it
/// shadows `Object#new` if defined as an instance method; spinel's
/// router maps `:new` action to `new_action`).
fn method_name_for_action(action: &str) -> &str {
    if action == "new" { "new_action" } else { action }
}

// ---------------------------------------------------------------------------
// `params` rewrites. Spinel controllers don't have the magic `params`
// method — request params arrive as a plain Hash on `@params`. The two
// Rails 8 idioms encountered here:
//
//   - `params.expect(:id)` → `@params[:id].to_i` (single-symbol form;
//     coerces because @params holds string values from the URL).
//   - `params.expect(post: [ :title, :body ])` → `@params.require(:post)
//     .permit(:title, :body)` (the older strong-params form, which
//     spinel's runtime implements).
//
// And bare `params` references (with no method call) lower to `@params`.
// ---------------------------------------------------------------------------

fn rewrite_params(expr: &Expr) -> Expr {
    let new_node = match &*expr.node {
        // `params.expect(...)` — recognized first so the recv is the
        // bare params Send, NOT the rewritten ivar (which would lose
        // the recognition pattern).
        ExprNode::Send { recv: Some(recv), method, args, block, parenthesized }
            if method.as_str() == "expect" && is_bare_params(recv) =>
        {
            return rewrite_expect(args, block.as_ref(), *parenthesized, expr.span);
        }
        // Bare `params` (no recv, no args, no block) → `@params`.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty() =>
        {
            ExprNode::Ivar { name: Symbol::from("params") }
        }
        // Generic structural recursion.
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_params).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_params(cond),
            then_branch: rewrite_params(then_branch),
            else_branch: rewrite_params(else_branch),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: rewrite_params(scrutinee),
            arms: arms
                .iter()
                .map(|a| Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(rewrite_params),
                    body: rewrite_params(&a.body),
                })
                .collect(),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_params),
            method: method.clone(),
            args: args.iter().map(rewrite_params).collect(),
            block: block.as_ref().map(rewrite_params),
            parenthesized: *parenthesized,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite_params(fun),
            args: args.iter().map(rewrite_params).collect(),
            block: block.as_ref().map(rewrite_params),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_params(left),
            right: rewrite_params(right),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_params(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: rewrite_params(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: rewrite_params(recv),
                    index: rewrite_params(index),
                },
                other => other.clone(),
            };
            ExprNode::Assign { target: new_target, value: rewrite_params(value) }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_params).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_params(k), rewrite_params(v)))
                .collect(),
            braced: *braced,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite_params(expr) },
                    other => other.clone(),
                })
                .collect(),
        },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(rewrite_params).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: rewrite_params(value) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: rewrite_params(expr),
            fallback: rewrite_params(fallback),
        },
        ExprNode::Return { value } => ExprNode::Return { value: rewrite_params(value) },
        ExprNode::Super { args: Some(args) } => ExprNode::Super {
            args: Some(args.iter().map(rewrite_params).collect()),
        },
        ExprNode::Next { value: Some(v) } => ExprNode::Next { value: Some(rewrite_params(v)) },
        ExprNode::Let { name, id, value, body } => ExprNode::Let {
            name: name.clone(),
            id: *id,
            value: rewrite_params(value),
            body: rewrite_params(body),
        },
        ExprNode::MultiAssign { targets, value } => ExprNode::MultiAssign {
            targets: targets.clone(),
            value: rewrite_params(value),
        },
        ExprNode::While { cond, body, until_form } => ExprNode::While {
            cond: rewrite_params(cond),
            body: rewrite_params(body),
            until_form: *until_form,
        },
        ExprNode::Range { begin, end, exclusive } => ExprNode::Range {
            begin: begin.as_ref().map(rewrite_params),
            end: end.as_ref().map(rewrite_params),
            exclusive: *exclusive,
        },
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: rewrite_params(body),
                rescues: rescues
                    .iter()
                    .map(|r| RescueClause {
                        classes: r.classes.iter().map(rewrite_params).collect(),
                        binding: r.binding.clone(),
                        body: rewrite_params(&r.body),
                    })
                    .collect(),
                else_branch: else_branch.as_ref().map(rewrite_params),
                ensure: ensure.as_ref().map(rewrite_params),
                implicit: *implicit,
            }
        }
        // Leaves (Lit / Var / Ivar / Const / SelfRef / Super{None} /
        // Next{None}) carry no children to rewrite.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
        diagnostic: expr.diagnostic.clone(),
    }
}

/// True when `e` is a bare `params` send: no receiver, no args, no
/// block. This is the recv shape `params.expect(...)` parses to.
fn is_bare_params(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// Lower `params.expect(...)` based on its argument shape:
/// - `params.expect(:id)` → `@params[:id].to_i`
/// - `params.expect(post: [:title, :body])` → `@params.require(:post).permit(:title, :body)`
///
/// Anything else (no args, multi-arg, unrecognized arg shape) is left
/// as `@params.expect(args...)` so we don't silently drop an idiom we
/// don't yet understand. The lowerer's job is rewrite, not erasure.
fn rewrite_expect(
    args: &[Expr],
    block: Option<&Expr>,
    parenthesized: bool,
    span: Span,
) -> Expr {
    if args.len() == 1 {
        let arg = &args[0];
        // Single-symbol form → @params[:sym].to_i
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
            return params_index_to_i(value, span);
        }
        // Single-keyword-hash form → @params.require(:k).permit(:f1, :f2, ...)
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            if let Some(pair) = single_resource_hash(entries) {
                return params_require_permit(pair.0, pair.1, span);
            }
        }
    }
    // Fallback: keep .expect with @params recv. Rewrite the args
    // recursively so any nested `params` references inside them get
    // lowered too.
    let recv = ivar("params", span);
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("expect"),
            args: args.iter().map(rewrite_params).collect(),
            block: block.map(rewrite_params),
            parenthesized,
        },
    )
}

/// `@params[:sym].to_i` — used for the single-symbol expect shape.
fn params_index_to_i(sym: &Symbol, span: Span) -> Expr {
    let index = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("params", span)),
            method: Symbol::from("[]"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Sym { value: sym.clone() } },
            )],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(index),
            method: Symbol::from("to_i"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `@params.require(:resource).permit(:f1, :f2, ...)` — the strong-
/// params chain spinel's runtime implements. Returns None at the call
/// site if the entries don't match the single-resource shape.
fn single_resource_hash(entries: &[(Expr, Expr)]) -> Option<(Symbol, Vec<Symbol>)> {
    if entries.len() != 1 {
        return None;
    }
    let (k, v) = &entries[0];
    let resource = match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
        _ => return None,
    };
    let fields = match &*v.node {
        ExprNode::Array { elements, .. } => {
            let mut out = Vec::with_capacity(elements.len());
            for el in elements {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => out.push(value.clone()),
                    _ => return None,
                }
            }
            out
        }
        _ => return None,
    };
    Some((resource, fields))
}

fn params_require_permit(resource: Symbol, fields: Vec<Symbol>, span: Span) -> Expr {
    let require_sym = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: resource } },
    );
    let require_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("params", span)),
            method: Symbol::from("require"),
            args: vec![require_sym],
            block: None,
            parenthesized: true,
        },
    );
    let permit_args: Vec<Expr> = fields
        .into_iter()
        .map(|f| Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: f } }))
        .collect();
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(require_call),
            method: Symbol::from("permit"),
            args: permit_args,
            block: None,
            parenthesized: true,
        },
    )
}

fn ivar(name: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Ivar { name: Symbol::from(name) })
}

// ---------------------------------------------------------------------------
// Synthetic-Expr helpers — every node a synthesized span and default
// effects/ty so the rest of the pipeline doesn't choke on them.
// ---------------------------------------------------------------------------

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn sym_lit(s: &str) -> Expr {
    syn(ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
}

fn var_ref(name: &str) -> Expr {
    syn(ExprNode::Var {
        id: VarId(0),
        name: Symbol::from(name),
    })
}

fn empty_seq() -> Expr {
    syn(ExprNode::Seq { exprs: vec![] })
}
