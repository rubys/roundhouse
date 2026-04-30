//! Action-body rewrite passes. Each `rewrite_*` / `expand_*` is an
//! independent IR-to-IR transform composed in declared order by
//! `lower_action_body` in `mod.rs`. Runs after `unwrap_respond_to` /
//! `synthesize_implicit_render` so the synthesized symbol-form render
//! shows up here as a plain `Send`.

use crate::dialect::Action;
use crate::expr::{ArrayStyle, BlockStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use super::util::map_expr;

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

pub(super) fn rewrite_render_to_views(expr: &Expr, module_name: Option<&str>, ivars: &[Symbol]) -> Expr {
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

pub(super) fn rewrite_assoc_through_parent(expr: &Expr) -> Expr {
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

pub(super) fn expand_build(
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

pub(super) fn expand_find(
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

pub(super) fn rewrite_drop_includes(expr: &Expr) -> Expr {
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

pub(super) fn rewrite_order_to_sort_by(expr: &Expr) -> Expr {
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

pub(super) fn rewrite_params_helpers_to_h(expr: &Expr, privs: &[Action]) -> Expr {
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

pub(super) fn rewrite_destroy_bang(expr: &Expr) -> Expr {
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

pub(super) fn rewrite_params(expr: &Expr) -> Expr {
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

pub(super) fn rewrite_redirect_to(expr: &Expr) -> Expr {
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

pub(super) fn rewrite_route_helpers(expr: &Expr) -> Expr {
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
    // Emit `permit([:f1, :f2, ...])` — single Array arg, not splat.
    // Monomorphic parameter slot for spinel + type-strict targets;
    // every per-target Parameters runtime takes Array[Symbol] here.
    let permit_array_elems: Vec<Expr> = fields
        .into_iter()
        .map(|f| Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: f } }))
        .collect();
    let permit_array = Expr::new(
        span,
        ExprNode::Array {
            elements: permit_array_elems,
            style: ArrayStyle::Brackets,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(require_call),
            method: Symbol::from("permit"),
            args: vec![permit_array],
            block: None,
            parenthesized: true,
        },
    )
}

fn ivar(name: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Ivar { name: Symbol::from(name) })
}
