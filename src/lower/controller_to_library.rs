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
use crate::expr::{Arm, ArrayStyle, Expr, ExprNode, Literal, Pattern};
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
/// `unwrap_respond_to` to the body to drop format dispatch.
fn action_to_method(a: &Action) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    let params: Vec<Symbol> = a.params.fields.iter().map(|(n, _)| n.clone()).collect();
    MethodDef {
        name: Symbol::from(method_name),
        receiver: MethodReceiver::Instance,
        params,
        body: unwrap_respond_to(&a.body),
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
