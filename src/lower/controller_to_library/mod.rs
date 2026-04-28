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

use std::collections::BTreeSet;

use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass, MethodDef,
    MethodReceiver, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Arm, ArrayStyle, BlockStyle, Expr, ExprNode, InterpPart, LValue, Literal, Pattern, RescueClause};
use crate::ident::{Symbol, VarId};
use crate::lower::controller::body::{synthesize_implicit_render, unwrap_respond_to};
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
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ true));
    }
    for a in &privs {
        methods.push(action_to_method(a, controller, &privs, /*is_public=*/ false));
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
        params: vec![Param::positional(Symbol::from("action_name"))],
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
/// the full action-body rewrite pipeline (see `lower_action_body`).
/// `is_public` gates the implicit-render synthesis: private filter
/// targets (`set_article`) and param helpers (`article_params`)
/// don't render — their callers do.
fn action_to_method(
    a: &Action,
    controller: &Controller,
    privs: &[Action],
    is_public: bool,
) -> MethodDef {
    let method_name = method_name_for_action(a.name.as_str());
    let params: Vec<Param> = a
        .params
        .fields
        .iter()
        .map(|(n, _)| Param::positional(n.clone()))
        .collect();
    let body = lower_action_body(&a.body, controller, a.name.as_str(), privs, is_public);
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

/// Apply the controller-body rewrite pipeline in declared order:
///
/// 1. `unwrap_respond_to` — drop `respond_to do |format| format.html
///    {…}; format.json {…} end` wrappers, keeping the HTML branch.
/// 2. `synthesize_implicit_render` — append `render :<action>` when
///    the body has no top-level terminal (Rails' implicit-render).
/// 3. `rewrite_render_to_views` — `render :sym, **kw` →
///    `render(Views::<Module>.<sym>(<ivars>), **kw)`. Uses the action's
///    ivar scope (body + every `before_action` filter target that fires)
///    to determine the positional args of the Views call.
/// 4. `rewrite_params` — `params` → `@params`, `params.expect(...)` →
///    indexed/require-permit forms.
/// 5. `rewrite_redirect_to` — polymorphic `redirect_to @x` →
///    `redirect_to(RouteHelpers.<x>_path(@x.id), ...)`.
/// 6. `rewrite_assoc_through_parent` — `@parent.assoc.build(args)` →
///    3-statement `attrs = …; attrs[:fk] = @parent.id; @x = Class.new(attrs)`.
///    `@parent.assoc.find(args)` → `@x = Class.find(args); if @x.fk !=
///    @parent.id; head(:not_found); return; end`.
/// 7. `rewrite_drop_includes` — drop `.includes(…)` from method chains.
///    Spinel has no relation-level eager-load; access is lazy by default.
/// 8. `rewrite_order_to_sort_by` — `<recv>.order(field: dir)` →
///    `<recv'>.sort_by { |a| a.field.to_s }<.reverse>` (`<recv'>` =
///    recv with `.all` prepended if recv is a bare Const).
/// 9. `rewrite_params_helpers_to_h` — wrap bare `<x>_params` calls with
///    `.to_h`. Spinel's strong-params chain returns a Parameters-like
///    object; model constructors expect a plain Hash.
/// 10. `rewrite_destroy_bang` — `<recv>.destroy!` → `<recv>.destroy`.
///    Spinel's runtime model has only one destroy variant.
/// 11. `rewrite_route_helpers` — bare `<x>_path` → `RouteHelpers.<x>_path`
///    (covers `articles_path` and the like that appear outside
///    redirect_to's first arg).
///
/// Run in this order because each pass leaves the IR in a shape the
/// next pass expects: render-views needs the synthesized symbol-form
/// call to rewrite; redirect_to rewrite needs the bare ivar before
/// route_helpers prefixes it; route_helpers needs to skip already-
/// rewritten `RouteHelpers.x_path(...)` calls (they have a recv now).
fn lower_action_body(
    body: &Expr,
    controller: &Controller,
    action_name: &str,
    privs: &[Action],
    is_public: bool,
) -> Expr {
    let unwrapped = unwrap_respond_to(body);
    let with_render = if is_public {
        let synth = synthesize_implicit_render(&unwrapped, action_name);
        let ivars = ivars_in_scope(controller, action_name, &synth, privs);
        let module_name = views_module_name(controller);
        rewrite_render_to_views(&synth, module_name.as_deref(), &ivars)
    } else {
        unwrapped
    };
    let with_params = rewrite_params(&with_render);
    let with_redirects = rewrite_redirect_to(&with_params);
    let with_assoc = rewrite_assoc_through_parent(&with_redirects);
    let with_no_includes = rewrite_drop_includes(&with_assoc);
    let with_order = rewrite_order_to_sort_by(&with_no_includes);
    let with_params_to_h = rewrite_params_helpers_to_h(&with_order, privs);
    let with_destroy = rewrite_destroy_bang(&with_params_to_h);
    rewrite_route_helpers(&with_destroy)
}

// ---------------------------------------------------------------------------
// Render-template-as-Views-call rewrite. Spinel doesn't have Rails'
// implicit-render-of-eponymous-template; every render goes through an
// explicit `render(Views::<Module>.<method>(<args>))` call. This pass
// handles two source shapes uniformly:
//
//   - `render :show` (synthesized by the upstream pass for actions with
//     no terminal) → `render(Views::Articles.show(@article))`.
//   - `render :new, status: :unprocessable_entity` (explicit, in
//     create's else branch after unwrap_respond_to) →
//     `render(Views::Articles.new(@article), status: :unprocessable_entity)`.
//
// `ivars` is the precomputed scope: every `@x = ...` assignment in the
// action body PLUS every filter target that fires for this action. The
// view-method call gets all of them as positional args, in the order
// they appear in scope. View-side parameter names that don't match
// here are a follow-on lowerer's problem.
// ---------------------------------------------------------------------------

fn rewrite_render_to_views(expr: &Expr, module_name: Option<&str>, ivars: &[Symbol]) -> Expr {
    let Some(module) = module_name else {
        return expr.clone();
    };
    let module_name_owned = module.to_string();
    let ivars = ivars.to_vec();
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            let view_method = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
                _ => return None,
            };
            let view_args: Vec<Expr> = ivars
                .iter()
                .map(|n| ivar(n.as_str(), e.span))
                .collect();
            let view_call = Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(const_path(&["Views", &module_name_owned], e.span)),
                    method: view_method,
                    args: view_args,
                    block: None,
                    parenthesized: true,
                },
            );
            let mut new_args = vec![view_call];
            new_args.extend(args.iter().skip(1).cloned());
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("render"),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Has-many-through-parent rewrite. Rails' `@article.comments.build(args)`
// and `@article.comments.find(args)` both go through the association
// proxy: build pre-fills the FK from the parent, find scopes the lookup
// to children of the parent. Spinel doesn't have association proxies;
// the parent linkage has to be made explicit at the call site.
//
//   @x = @parent.<assoc>.build(<args>)
//   ─────────────────────────────────────────────────────────────────
//   attrs = <args>.to_h
//   attrs[:<parent>_id] = @parent.id
//   @x = <Singular>.new(attrs)
//
//   @x = @parent.<assoc>.find(<args>)
//   ─────────────────────────────────────────────────────────────────
//   @x = <Singular>.find(<args>)
//   if @x.<parent>_id != @parent.id
//     head(:not_found)
//     return
//   end
//
// One Assign expands to a Seq of multiple Exprs — the outer Seq the
// emitter walks for line-per-statement output flattens implicitly.
// ---------------------------------------------------------------------------

fn rewrite_assoc_through_parent(expr: &Expr) -> Expr {
    map_expr(expr, &|e| {
        let ExprNode::Assign { target: LValue::Ivar { name: lhs }, value } = &*e.node else {
            return None;
        };
        let ExprNode::Send {
            recv: Some(outer_recv),
            method: outer_method,
            args: outer_args,
            block: None,
            ..
        } = &*value.node
        else {
            return None;
        };
        let kind = match outer_method.as_str() {
            "build" => AssocKind::Build,
            "find" => AssocKind::Find,
            _ => return None,
        };
        if outer_args.len() != 1 {
            return None;
        }
        let ExprNode::Send {
            recv: Some(inner_recv),
            method: assoc_method,
            args: inner_args,
            block: None,
            ..
        } = &*outer_recv.node
        else {
            return None;
        };
        if !inner_args.is_empty() {
            return None;
        }
        let ExprNode::Ivar { name: parent_name } = &*inner_recv.node else {
            return None;
        };
        let model_class = crate::naming::singularize_camelize(assoc_method.as_str());
        let fk = format!("{}_id", parent_name.as_str());
        Some(match kind {
            AssocKind::Build => expand_build(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span),
            AssocKind::Find => expand_find(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span),
        })
    })
}

enum AssocKind {
    Build,
    Find,
}

fn expand_build(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    span: Span,
) -> Expr {
    // attrs = <arg>
    // The `.to_h` wrap is added by the params-helpers pass when <arg>
    // is a `<x>_params` call — keeping the two concerns separate avoids
    // double-wrapping if <arg> already has `.to_h` for some reason.
    let attrs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("attrs") },
            value: arg.clone(),
        },
    );

    // attrs[:<fk>] = @<parent>.id
    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fk_sym = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(fk) } },
    );
    let attrs_var = Expr::new(
        span,
        ExprNode::Var { id: VarId(0), name: Symbol::from("attrs") },
    );
    let index_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Index { recv: attrs_var.clone(), index: fk_sym },
            value: parent_id,
        },
    );

    // @<lhs> = <Class>.new(attrs)
    let new_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("new"),
            args: vec![attrs_var],
            block: None,
            parenthesized: true,
        },
    );
    let final_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: new_call,
        },
    );

    Expr::new(
        span,
        ExprNode::Seq { exprs: vec![attrs_assign, index_assign, final_assign] },
    )
}

fn expand_find(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    span: Span,
) -> Expr {
    // @<lhs> = <Class>.find(<arg>)
    let find_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("find"),
            args: vec![arg.clone()],
            block: None,
            parenthesized: true,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: find_call,
        },
    );

    // if @<lhs>.<fk> != @<parent>.id; head(:not_found); return; end
    let lhs_fk = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(fk),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let cond = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(lhs_fk),
            method: Symbol::from("!="),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );
    let head_call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("head"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("not_found") } },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let return_stmt = Expr::new(
        span,
        ExprNode::Return {
            value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    let if_body = Expr::new(
        span,
        ExprNode::Seq { exprs: vec![head_call, return_stmt] },
    );
    let if_stmt = Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch: if_body,
            else_branch: Expr::new(span, ExprNode::Seq { exprs: vec![] }),
        },
    );

    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, if_stmt] })
}

// ---------------------------------------------------------------------------
// `.includes(…)` drop. Rails' `.includes(:assoc)` is an eager-load
// optimization on a relation; spinel models access associations lazily
// at the call site (e.g. `belongs_to` lowered to `Class.find_by(...)`),
// so the eager-load has no runtime equivalent and is dropped from any
// chain it appears in. Correctness-equivalent under N+1 rather than
// performance-equivalent.
// ---------------------------------------------------------------------------

fn rewrite_drop_includes(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: Some(inner), method, .. }
            if method.as_str() == "includes" =>
        {
            // Replace the entire `.includes(...)` call with its receiver
            // — the rest of the chain. Recurse so `.includes(...)` calls
            // nested inside the recv get dropped too.
            Some(rewrite_drop_includes(inner))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// `.order(field: :dir)` → `.sort_by { |a| a.field.to_s }[.reverse]`.
// Spinel doesn't have ActiveRecord::Relation; sort happens in memory
// over the loaded array. The `.to_s` cast normalizes Time-like fields
// (which spinel stores as strings) and is idempotent for strings.
//
// When the order call's receiver is a bare model Const (e.g. `Article`),
// `.all` is prepended so the chain returns an Array to call `.sort_by`
// on (`Article.sort_by { ... }` would fail; `Article.all.sort_by` is
// the spinel idiom).
// ---------------------------------------------------------------------------

fn rewrite_order_to_sort_by(expr: &Expr) -> Expr {
    map_expr(expr, &|e| {
        let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
            return None;
        };
        if method.as_str() != "order" || args.len() != 1 {
            return None;
        }
        let ExprNode::Hash { entries, .. } = &*args[0].node else {
            return None;
        };
        if entries.len() != 1 {
            return None;
        }
        let (k, v) = &entries[0];
        let ExprNode::Lit { value: Literal::Sym { value: field } } = &*k.node else {
            return None;
        };
        let direction = match &*v.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            _ => return None,
        };
        // Lower the recv first so nested order/includes are handled.
        let recv_lowered = rewrite_order_to_sort_by(recv);
        let sort_recv = match &*recv_lowered.node {
            ExprNode::Const { .. } => Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(recv_lowered),
                    method: Symbol::from("all"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            _ => recv_lowered,
        };
        let a_var = Expr::new(
            e.span,
            ExprNode::Var { id: VarId(0), name: Symbol::from("a") },
        );
        let a_field = Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(a_var),
                method: field.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let block_body = Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(a_field),
                method: Symbol::from("to_s"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let block = Expr::new(
            e.span,
            ExprNode::Lambda {
                params: vec![Symbol::from("a")],
                block_param: None,
                body: block_body,
                block_style: BlockStyle::Brace,
            },
        );
        let sort_by_call = Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(sort_recv),
                method: Symbol::from("sort_by"),
                args: vec![],
                block: Some(block),
                parenthesized: false,
            },
        );
        let result = if direction == "desc" {
            Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(sort_by_call),
                    method: Symbol::from("reverse"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )
        } else {
            sort_by_call
        };
        Some(result)
    })
}

// ---------------------------------------------------------------------------
// `<x>_params` to-h wrap. Spinel's strong-params chain
// (`@params.require(:resource).permit(:f, …)`) returns a Parameters-like
// object; model constructors and `update` expect a plain Hash. Every
// no-recv call to a controller-defined `<x>_params` helper gets `.to_h`
// appended at the use site.
//
// Scoped to private actions whose name ends in `_params` — narrow
// enough to avoid catching unrelated method names, broad enough to
// cover the `article_params` / `comment_params` convention without
// inspecting body shape.
// ---------------------------------------------------------------------------

fn rewrite_params_helpers_to_h(expr: &Expr, privs: &[Action]) -> Expr {
    let helper_names: Vec<Symbol> = privs
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| n.as_str().ends_with("_params"))
        .collect();
    if helper_names.is_empty() {
        return expr.clone();
    }
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && helper_names.iter().any(|h| h == method) =>
        {
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(e.clone()),
                    method: Symbol::from("to_h"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// `destroy!` → `destroy`. Spinel's runtime model exposes one destroy
// method (raise-on-failure semantics); the bang form has no separate
// behavior to preserve, so the surface gets normalized here. Applies to
// any Send (any recv shape) whose method name is exactly `destroy!`.
// ---------------------------------------------------------------------------

fn rewrite_destroy_bang(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv, method, args, block, parenthesized }
            if method.as_str() == "destroy!" =>
        {
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: recv.as_ref().map(rewrite_destroy_bang),
                    method: Symbol::from("destroy"),
                    args: args.iter().map(rewrite_destroy_bang).collect(),
                    block: block.as_ref().map(rewrite_destroy_bang),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

/// Derive the `Views::*` submodule name from a controller's class name.
/// `ArticlesController` → `Articles`. Returns None when the name doesn't
/// follow the `*Controller` convention or strips down to "Application"
/// (which has no view module).
fn views_module_name(controller: &Controller) -> Option<String> {
    let name = controller.name.0.as_str();
    let stem = name.strip_suffix("Controller")?;
    if stem.is_empty() || stem == "Application" {
        return None;
    }
    Some(stem.to_string())
}

/// Collect every ivar that this action sees in scope at render time:
/// each `@x = ...` assignment in the body itself, plus the same in
/// every filter target whose `only:`/`except:` filter applies to this
/// action. Source order is preserved (body first, then each fired
/// filter in declaration order); duplicates dropped.
fn ivars_in_scope(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
    privs: &[Action],
) -> Vec<Symbol> {
    let mut seen: BTreeSet<Symbol> = BTreeSet::new();
    let mut out: Vec<Symbol> = Vec::new();

    let push = |sym: Symbol, seen: &mut BTreeSet<Symbol>, out: &mut Vec<Symbol>| {
        if seen.insert(sym.clone()) {
            out.push(sym);
        }
    };

    let mut from_body: Vec<Symbol> = Vec::new();
    collect_assigned_ivars(body, &mut from_body);
    for s in from_body {
        push(s, &mut seen, &mut out);
    }

    let action_sym = Symbol::from(action_name);
    for filter in controller.filters() {
        if !matches!(filter.kind, FilterKind::Before) {
            continue;
        }
        if !filter_applies_to(filter, &action_sym) {
            continue;
        }
        let Some(target_action) = privs.iter().find(|p| p.name == filter.target) else {
            continue;
        };
        let mut from_filter: Vec<Symbol> = Vec::new();
        collect_assigned_ivars(&target_action.body, &mut from_filter);
        for s in from_filter {
            push(s, &mut seen, &mut out);
        }
    }

    out
}

/// True when `filter` applies to `action`, per its `only:` / `except:`
/// list. Empty `only` + empty `except` = applies to everything.
fn filter_applies_to(filter: &Filter, action: &Symbol) -> bool {
    if !filter.only.is_empty() {
        return filter.only.iter().any(|a| a == action);
    }
    if !filter.except.is_empty() {
        return !filter.except.iter().any(|a| a == action);
    }
    true
}

/// Walk `expr` collecting every ivar that appears on the LHS of an
/// `Assign`. Source-order, deduplication is done by the caller.
fn collect_assigned_ivars(expr: &Expr, out: &mut Vec<Symbol>) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*expr.node {
        out.push(name.clone());
    }
    walk_children(expr, &mut |c| collect_assigned_ivars(c, out));
}

/// Visit every direct child Expr of `expr`. Mirrors `map_expr`'s
/// traversal but in read-only form — used by passes that need to scan
/// the tree without rewriting it.
fn walk_children<F: FnMut(&Expr)>(expr: &Expr, f: &mut F) {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().for_each(f),
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = a.guard.as_ref() {
                    f(g);
                }
                f(&a.body);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_ref() {
                f(r);
            }
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::Assign { target, value } => {
            match target {
                LValue::Attr { recv, .. } => f(recv),
                LValue::Index { recv, index } => {
                    f(recv);
                    f(index);
                }
                _ => {}
            }
            f(value);
        }
        ExprNode::Array { elements, .. } => elements.iter().for_each(&mut *f),
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    f(expr);
                }
            }
        }
        ExprNode::Yield { args } => args.iter().for_each(&mut *f),
        ExprNode::Raise { value } => f(value),
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr);
            f(fallback);
        }
        ExprNode::Return { value } => f(value),
        ExprNode::Super { args: Some(args) } => args.iter().for_each(&mut *f),
        ExprNode::Next { value: Some(v) } => f(v),
        ExprNode::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        ExprNode::MultiAssign { value, .. } => f(value),
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_ref() {
                f(b);
            }
            if let Some(e) = end.as_ref() {
                f(e);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body);
            for r in rescues {
                r.classes.iter().for_each(&mut *f);
                f(&r.body);
            }
            if let Some(e) = else_branch.as_ref() {
                f(e);
            }
            if let Some(e) = ensure.as_ref() {
                f(e);
            }
        }
        _ => {}
    }
}

/// Action name → Ruby method name. `new` is the only rename (it
/// shadows `Object#new` if defined as an instance method; spinel's
/// router maps `:new` action to `new_action`).
fn method_name_for_action(action: &str) -> &str {
    if action == "new" { "new_action" } else { action }
}

// ---------------------------------------------------------------------------
// Generic Expr rewrite helper. `f` runs on each node pre-order: when
// it returns `Some(replacement)`, the result is used verbatim (no
// further recursion into that subtree — `f` is responsible for
// recursing into children if needed). When it returns `None`, the
// default structural map runs, applying `map_expr` to every child.
//
// This is the small kernel that lets each rewriter (params,
// redirect_to, …) be a 10-line pattern match instead of a 130-line
// case-per-variant walker.
// ---------------------------------------------------------------------------

fn map_expr<F>(expr: &Expr, f: &F) -> Expr
where
    F: Fn(&Expr) -> Option<Expr>,
{
    if let Some(replacement) = f(expr) {
        return replacement;
    }
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| map_expr(e, f)).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: map_expr(cond, f),
            then_branch: map_expr(then_branch, f),
            else_branch: map_expr(else_branch, f),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: map_expr(scrutinee, f),
            arms: arms
                .iter()
                .map(|a| Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(|g| map_expr(g, f)),
                    body: map_expr(&a.body, f),
                })
                .collect(),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| map_expr(r, f)),
            method: method.clone(),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
            parenthesized: *parenthesized,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: map_expr(fun, f),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: map_expr(left, f),
            right: map_expr(right, f),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: map_expr(body, f),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: map_expr(recv, f),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: map_expr(recv, f),
                    index: map_expr(index, f),
                },
                other => other.clone(),
            };
            ExprNode::Assign { target: new_target, value: map_expr(value, f) }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(|e| map_expr(e, f)).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (map_expr(k, f), map_expr(v, f)))
                .collect(),
            braced: *braced,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: map_expr(expr, f) },
                    other => other.clone(),
                })
                .collect(),
        },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(|a| map_expr(a, f)).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: map_expr(value, f) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: map_expr(expr, f),
            fallback: map_expr(fallback, f),
        },
        ExprNode::Return { value } => ExprNode::Return { value: map_expr(value, f) },
        ExprNode::Super { args: Some(args) } => ExprNode::Super {
            args: Some(args.iter().map(|a| map_expr(a, f)).collect()),
        },
        ExprNode::Next { value: Some(v) } => ExprNode::Next { value: Some(map_expr(v, f)) },
        ExprNode::Let { name, id, value, body } => ExprNode::Let {
            name: name.clone(),
            id: *id,
            value: map_expr(value, f),
            body: map_expr(body, f),
        },
        ExprNode::MultiAssign { targets, value } => ExprNode::MultiAssign {
            targets: targets.clone(),
            value: map_expr(value, f),
        },
        ExprNode::While { cond, body, until_form } => ExprNode::While {
            cond: map_expr(cond, f),
            body: map_expr(body, f),
            until_form: *until_form,
        },
        ExprNode::Range { begin, end, exclusive } => ExprNode::Range {
            begin: begin.as_ref().map(|b| map_expr(b, f)),
            end: end.as_ref().map(|e| map_expr(e, f)),
            exclusive: *exclusive,
        },
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: map_expr(body, f),
                rescues: rescues
                    .iter()
                    .map(|r| RescueClause {
                        classes: r.classes.iter().map(|c| map_expr(c, f)).collect(),
                        binding: r.binding.clone(),
                        body: map_expr(&r.body, f),
                    })
                    .collect(),
                else_branch: else_branch.as_ref().map(|e| map_expr(e, f)),
                ensure: ensure.as_ref().map(|e| map_expr(e, f)),
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
    map_expr(expr, &|e| match &*e.node {
        // `params.expect(...)` — recognized first so the recv is still
        // the bare `params` Send, not the @params ivar (which would lose
        // the recognition pattern).
        ExprNode::Send { recv: Some(recv), method, args, block, parenthesized }
            if method.as_str() == "expect" && is_bare_params(recv) =>
        {
            Some(rewrite_expect(args, block.as_ref(), *parenthesized, e.span))
        }
        // Bare `params` (no recv, no args, no block) → `@params`.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty() =>
        {
            Some(Expr::new(e.span, ExprNode::Ivar { name: Symbol::from("params") }))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// `redirect_to` polymorphic rewrite. Rails' `redirect_to @article` does
// implicit polymorphic resolution to `article_path(@article)`; spinel
// requires the explicit form. The IR-level shape:
//
//   Send { method: "redirect_to", args: [Ivar{name}, ...kwargs] }
//
// becomes:
//
//   Send { method: "redirect_to", args: [
//     Send { recv: Const(RouteHelpers), method: "<name>_path",
//            args: [Send { recv: Ivar{name}, method: "id" }] },
//     ...kwargs
//   ], parenthesized: true }
//
// Only the first positional arg is rewritten; trailing keyword-hash
// args (notice:, status:) pass through unchanged.
// ---------------------------------------------------------------------------

fn rewrite_redirect_to(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if method.as_str() == "redirect_to" && !args.is_empty() =>
        {
            // Two recognized first-arg shapes:
            //   - `@x` (Ivar) → wrap as `RouteHelpers.<x>_path(@x.id)`.
            //   - `<x>_path` (no-recv Send ending in _path) → prefix
            //     with RouteHelpers so all redirect_to call sites
            //     render uniformly with the parenthesized form.
            //
            // Other shapes (string URL, hash, …) leave the call alone
            // so we don't accidentally mangle an idiom we don't handle.
            let first = &args[0];
            let new_first = match &*first.node {
                ExprNode::Ivar { name } => polymorphic_path(name, e.span),
                ExprNode::Send { recv: None, method: m, args: m_args, block: m_block, parenthesized }
                    if m.as_str().ends_with("_path") =>
                {
                    Expr::new(
                        first.span,
                        ExprNode::Send {
                            recv: Some(const_path(&["RouteHelpers"], first.span)),
                            method: m.clone(),
                            args: m_args.clone(),
                            block: m_block.clone(),
                            parenthesized: *parenthesized,
                        },
                    )
                }
                _ => return None,
            };
            let mut new_args = vec![new_first];
            new_args.extend(args.iter().skip(1).cloned());
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("redirect_to"),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    })
}

/// `RouteHelpers.<ivar_name>_path(@<ivar_name>.id)` — the explicit form
/// that replaces Rails' polymorphic `redirect_to @x`.
fn polymorphic_path(ivar_name: &Symbol, span: Span) -> Expr {
    let ivar_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(ivar_name.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let helper_name = format!("{}_path", ivar_name.as_str());
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&["RouteHelpers"], span)),
            method: Symbol::from(helper_name),
            args: vec![ivar_id],
            block: None,
            parenthesized: true,
        },
    )
}

// ---------------------------------------------------------------------------
// `<x>_path` route-helper prefix. Bare calls to route helpers (`Send`
// with no recv whose method ends in `_path`) get the `RouteHelpers.`
// receiver added. Spinel's runtime defines all path helpers as module
// functions on `RouteHelpers`; controllers must call them through that
// namespace, since the `xxx_path` magic Rails injects via include
// doesn't exist here.
//
// This pass runs AFTER `rewrite_redirect_to` so the polymorphic
// rewrite's freshly-synthesized `RouteHelpers.x_path(...)` calls (which
// have a recv) are skipped — only original bare calls get the prefix.
// ---------------------------------------------------------------------------

fn rewrite_route_helpers(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if method.as_str().ends_with("_path") =>
        {
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(const_path(&["RouteHelpers"], e.span)),
                    method: method.clone(),
                    args: args.iter().map(rewrite_route_helpers).collect(),
                    block: block.as_ref().map(rewrite_route_helpers),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

fn const_path(segments: &[&str], span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Const {
            path: segments.iter().map(|s| Symbol::from(*s)).collect(),
        },
    )
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
