//! Action-body rewrite passes. Each `rewrite_*` / `expand_*` is an
//! independent IR-to-IR transform composed in declared order by
//! `lower_action_body` in `mod.rs`. Runs after `unwrap_respond_to` /
//! `synthesize_implicit_render` so the synthesized symbol-form render
//! shows up here as a plain `Send`.

use std::collections::BTreeMap;

use crate::dialect::Action;
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use super::params::ParamsSpec;
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

pub(super) fn rewrite_render_to_views(
    expr: &Expr,
    module_name: Option<&str>,
    ivars: &[Symbol],
    view_ivars: &super::ViewIvarMap,
    current_action: &str,
) -> Expr {
    let Some(module) = module_name else {
        return expr.clone();
    };
    let module_name_owned = module.to_string();
    let ivars = ivars.to_vec();
    // Controller-context literals every view can read (see
    // extra_params.rs): the current action name and the controller's
    // resource name (`HomeController` → module "Home" → "home").
    let current_action = current_action.to_string();
    let controller_name = crate::naming::snake_case(&module_name_owned);
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            // View name comes from `render :index` (a Symbol first arg) or
            // `render action: "index"` / `render action: :index` (a kwarg
            // hash). The kwarg form may also carry other options (status:,
            // …); those leftover entries ride through as `action_hash_extra`.
            let (view_method, action_hash_extra): (Symbol, Vec<Expr>) = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => (value.clone(), Vec::new()),
                ExprNode::Hash { entries, kwargs: true } => {
                    let mut name: Option<Symbol> = None;
                    let mut body: Option<(Expr, bool)> = None; // (expr, is_plain)
                    let mut rest: Vec<(Expr, Expr)> = Vec::new();
                    for (k, v) in entries {
                        let key = match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                            _ => "",
                        };
                        match key {
                            "action" => name = render_target_symbol(v),
                            // `render html:` / `render plain:` — a body
                            // expression, not a template name. Normalized
                            // below to the positional-body form the runtime
                            // render takes.
                            "html" => body = Some((v.clone(), false)),
                            "plain" => body = Some((v.clone(), true)),
                            _ => rest.push((k.clone(), v.clone())),
                        }
                    }
                    if name.is_none() {
                        // Non-template render: `render html: X, layout:
                        // "application"` → `render(X, layout: …)` (the
                        // Ruby emit layout pass honors + strips `layout:`);
                        // `render plain: X, status: N` → `render(X,
                        // status: N, content_type: "text/plain")`. A
                        // `layout: false` entry is dropped (no layout is
                        // already the runtime default for a body render).
                        let Some((body_expr, is_plain)) = body else { return None };
                        rest.retain(|(k, v)| {
                            let is_layout_false = matches!(
                                &*k.node,
                                ExprNode::Lit { value: Literal::Sym { value } }
                                    if value.as_str() == "layout"
                            ) && matches!(
                                &*v.node,
                                ExprNode::Lit { value: Literal::Bool { value: false } }
                            );
                            !is_layout_false
                        });
                        let mut new_args = vec![body_expr];
                        if !rest.is_empty() {
                            new_args.push(Expr::new(
                                args[0].span,
                                ExprNode::Hash { entries: rest, kwargs: true },
                            ));
                        }
                        if is_plain {
                            merge_or_append_kwarg(
                                &mut new_args,
                                "content_type",
                                "text/plain",
                                e.span,
                            );
                        }
                        new_args.extend(args.iter().skip(1).cloned());
                        return Some(Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: None,
                                method: Symbol::from("render"),
                                args: new_args,
                                block: block.clone(),
                                parenthesized: true,
                            },
                        ));
                    }
                    let Some(n) = name else { return None };
                    let extra = if rest.is_empty() {
                        Vec::new()
                    } else {
                        vec![Expr::new(
                            args[0].span,
                            ExprNode::Hash { entries: rest, kwargs: true },
                        )]
                    };
                    (n, extra)
                }
                _ => return None,
            };
            // View-driven arg list: an action view's params are exactly the
            // @ivars it reads, so pass `@<name>` for each (matching the
            // generated view signature). Look up by the html action stem
            // (before the `_json` rename below). Falls back to the
            // controller's in-scope ivars when the view isn't in the map
            // (json/jbuilder views, or a render with no matching template).
            let contract =
                view_ivars.get(&(module_name_owned.clone(), view_method.as_str().to_string()));
            // Peek ahead for the jbuilder marker (also computed below):
            // json renders resolve to `<stem>_json` view methods that are
            // deliberately absent from the html-view contract map, so a
            // lookup miss is normal for them.
            let is_json = render_kwargs_have_format(args, "json");
            // A non-json render whose target isn't among the emitted
            // action views means the template doesn't exist in the source
            // tree. Rails raises ActionView::MissingTemplate there — and
            // lobsters' about/privacy actions rescue it as their NORMAL
            // path (hardcoded-fallback pages). Emitting the Views call
            // anyway would be a NoMethodError no rescue catches.
            if contract.is_none() && !is_json {
                return Some(Expr::new(
                    e.span,
                    ExprNode::Raise {
                        value: Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: Some(const_path(
                                    &["ActionView", "MissingTemplate"],
                                    e.span,
                                )),
                                method: Symbol::from("new"),
                                args: vec![str_lit(e.span, view_method.as_str())],
                                block: None,
                                parenthesized: true,
                            },
                        ),
                    },
                ));
            }
            let resolved_ivars: Vec<Symbol> = contract
                .map(|c| c.ivars.clone())
                .unwrap_or_else(|| ivars.clone());
            // Pass action_name/controller_name literals only to views whose
            // contract records that they reference them (so views that don't
            // get no extra args — no arity mismatch).
            let pass_action_name = contract.map(|c| c.uses_action_name).unwrap_or(false);
            let pass_controller_name = contract.map(|c| c.uses_controller_name).unwrap_or(false);
            // Peek at the trailing kwarg-Hash for a `format: :json`
            // marker that the respond_to flattener planted. If
            // present, route to `<sym>_json` view and tag the outer
            // render with `content_type: "application/json"`. The
            // marker drops out of the rewritten kwargs so it doesn't
            // leak past the lowerer.
            let json_format = render_kwargs_have_format(args, "json");
            let (view_method, content_type) = if json_format {
                (
                    Symbol::from(format!("{}_json", view_method.as_str())),
                    Some("application/json"),
                )
            } else {
                (view_method, None)
            };
            let mut view_args: Vec<Expr> = resolved_ivars
                .iter()
                .map(|n| ivar(n.as_str(), e.span))
                .collect();
            // Every view's signature carries `notice = nil, alert = nil`
            // as trailing extra params (uniform shape; see
            // `view_to_library/extra_params.rs`). Pass `@flash[:notice]`
            // and `@flash[:alert]` from the controller so views that
            // render flash messages receive them. Views that don't
            // reference flash get unused-local args — harmless under
            // any target's emit.
            //
            // The jbuilder lowerer does NOT plumb flash extras (json
            // templates never reference notice/alert), so for the
            // `_json` view variant we pass just the ivars.
            if !json_format {
                view_args.push(flash_lookup(e.span, "notice"));
                view_args.push(flash_lookup(e.span, "alert"));
                if pass_action_name {
                    view_args.push(str_lit(e.span, &current_action));
                }
                if pass_controller_name {
                    view_args.push(str_lit(e.span, &controller_name));
                }
            }
            // Digit-leading stems (`about/404`) carry a `_` prefix on the
            // method — must match the def-site naming in view_to_library.
            let view_method = crate::lower::view::view_method_name(view_method.as_str());
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
            new_args.extend(action_hash_extra);
            let rest: Vec<Expr> = args
                .iter()
                .skip(1)
                .cloned()
                .filter_map(|a| strip_format_kwarg(&a))
                .collect();
            new_args.extend(rest);
            if let Some(ct) = content_type {
                merge_or_append_kwarg(&mut new_args, "content_type", ct, e.span);
            }
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

/// A String-literal expression (for the action_name/controller_name
/// view args passed from the controller).
fn str_lit(span: Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

/// The view name from a `render action:` value — accepts a Symbol
/// (`action: :index`) or a String (`action: "index"`) literal.
fn render_target_symbol(v: &Expr) -> Option<Symbol> {
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

/// True when render's args have a trailing kwarg-Hash whose `format:`
/// entry is `:<fmt>`. The marker is planted by the respond_to
/// flattener for the json branch only — html renders never have it.
fn render_kwargs_have_format(args: &[Expr], fmt: &str) -> bool {
    let Some(last) = args.last() else { return false };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else {
        return false;
    };
    entries.iter().any(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "format")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == fmt)
    })
}

/// Drop the `format:` entry from a trailing kwarg-Hash on a render
/// call. `format:` is the dispatch marker the respond_to flattener
/// planted; it's consumed at lower time and shouldn't leak to the
/// runtime. `location:` passes through (the runtime's render takes
/// `location:` and main.rb ships it as the Location header).
/// Returns `Some(stripped)` if the Hash still has entries, `None` if
/// the strip left it empty (caller drops the now-empty Hash).
fn strip_format_kwarg(arg: &Expr) -> Option<Expr> {
    if let ExprNode::Hash { entries, kwargs: true } = &*arg.node {
        let kept: Vec<(Expr, Expr)> = entries
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    &*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } }
                        if value.as_str() == "format"
                )
            })
            .cloned()
            .collect();
        if kept.is_empty() {
            return None;
        }
        return Some(Expr::new(
            arg.span,
            ExprNode::Hash {
                entries: kept,
                kwargs: true,
            },
        ));
    }
    Some(arg.clone())
}

/// Merge a single `key: <str-value>` entry into the trailing
/// kwarg-Hash of `args`. If args already ends with a kwarg Hash,
/// append the entry to it; otherwise push a fresh kwarg Hash with
/// just this entry. The runtime's `render(body, status:, content_type:)`
/// expects ONE kwargs hash, not multiple.
fn merge_or_append_kwarg(args: &mut Vec<Expr>, key: &str, value: &str, span: Span) {
    let key_node = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from(key),
            },
        },
    );
    let val_node = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Str {
                value: value.to_string(),
            },
        },
    );
    if let Some(last) = args.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let mut new_entries = entries.clone();
            new_entries.push((key_node, val_node));
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return;
        }
    }
    args.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![(key_node, val_node)],
            kwargs: true,
        },
    ));
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

pub(super) fn rewrite_assoc_through_parent_typed(
    expr: &Expr,
    privs: &[Action],
    params_specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Expr {
    let helper_names: Vec<Symbol> = privs
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| n.as_str().ends_with("_params"))
        .collect();
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
        // `find` needs its id arg; `build` may be bare (`@story.comments.
        // build` — new child for a form, FK pre-filled, no attributes).
        let max_args = 1;
        if outer_args.len() > max_args
            || (outer_args.is_empty() && !matches!(kind, AssocKind::Build))
        {
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
            AssocKind::Build => {
                if outer_args.is_empty() {
                    return Some(expand_build_bare(&model_class, &fk, parent_name, lhs, e.span));
                }
                if let Some(resource) = match_params_helper(&outer_args[0], &helper_names) {
                    if params_specs.contains_key(&resource) {
                        return Some(expand_build_typed(
                            &model_class,
                            &fk,
                            parent_name,
                            lhs,
                            &outer_args[0],
                            e.span,
                        ));
                    }
                }
                expand_build(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span)
            }
            AssocKind::Find => {
                expand_find(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span)
            }
        })
    })
}

/// True-when-Some: `arg` is a bare call to a `<x>_params` helper. Returns
/// the resource symbol (`<x>` minus the `_params` suffix) so callers can
/// look up the corresponding `ParamsSpec`.
fn match_params_helper(arg: &Expr, helper_names: &[Symbol]) -> Option<Symbol> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*arg.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    if !helper_names.iter().any(|h| h == method) {
        return None;
    }
    let stem = method.as_str().trim_end_matches("_params");
    Some(Symbol::from(stem))
}

enum AssocKind {
    Build,
    Find,
}

/// Typed-factory build expansion:
///
/// ```ruby
/// @<lhs> = <Class>.from_params(<arg>)
/// @<lhs>.<fk> = @<parent>.id
/// ```
///
/// `<arg>` is the typed `<resource>_params` helper call (returning
/// `<Resource>Params`); `<Class>.from_params` is the per-model factory
/// added by `model_to_library/schema.rs`. The FK setter follows the
/// model's `attr_writer` for the foreign key column.
pub(super) fn expand_build_typed(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    span: Span,
) -> Expr {
    let from_params_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("from_params"),
            args: vec![arg.clone()],
            block: None,
            parenthesized: true,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: from_params_call,
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
    let fk_setter = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(format!("{fk}=")),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );

    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, fk_setter] })
}

/// Zero-arg build expansion (`@comment = @story.comments.build`):
///
/// ```ruby
/// @<lhs> = <Class>.new
/// @<lhs>.<fk> = @<parent>.id
/// ```
///
/// No attributes to absorb — just the bare constructor and the parent
/// linkage through the model's foreign-key writer.
fn expand_build_bare(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    span: Span,
) -> Expr {
    let new_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign { target: LValue::Ivar { name: lhs.clone() }, value: new_call },
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
    let fk_setter = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(format!("{fk}=")),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, fk_setter] })
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
// `<Model>.new(<resource>_params)` → `<Model>.from_params(<resource>_params)`.
//
// The typed-factory rewrite that replaces the legacy `.to_h`-wrap pass.
// Now that the `<resource>_params` helper returns a typed `<Resource>Params`
// (synthesized from the controller's `permit` declaration via
// `controller_to_library::params`), the model's `new(attrs: Hash)`
// constructor isn't the right entry point — `from_params` takes the
// typed instance and assigns each permitted field through the named
// accessor.
//
// Match shape: `<Const>.new(<bare _params helper call>)` where the
// helper's name is in `privs` and ends with `_params`. Any other
// argument shape (Hash literal, Array, …) flows through unchanged.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_model_new_to_from_params(
    expr: &Expr,
    privs: &[Action],
    params_specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Expr {
    let helper_names: Vec<Symbol> = privs
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| n.as_str().ends_with("_params"))
        .collect();
    if helper_names.is_empty() {
        return expr.clone();
    }
    map_expr(expr, &|e| {
        let ExprNode::Send {
            recv: Some(model_recv),
            method,
            args,
            block: None,
            parenthesized,
        } = &*e.node
        else {
            return None;
        };
        if method.as_str() != "new" || args.len() != 1 {
            return None;
        }
        // Only rewrite `<Const>.new(...)` shapes — leaves `instance.new(...)`
        // (rare but legal) untouched.
        let ExprNode::Const { .. } = &*model_recv.node else {
            return None;
        };
        let resource = match &*args[0].node {
            ExprNode::Send { recv: None, method: helper_method, args: helper_args, block: None, .. }
                if helper_args.is_empty()
                    && helper_names.iter().any(|h| h == helper_method) =>
            {
                let stem = helper_method.as_str().trim_end_matches("_params");
                Symbol::from(stem)
            }
            _ => return None,
        };
        if !params_specs.contains_key(&resource) {
            return None;
        }
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(model_recv.clone()),
                method: Symbol::from("from_params"),
                args: vec![args[0].clone()],
                block: None,
                parenthesized: *parenthesized,
            },
        ))
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

/// `render(:show, …, location: @article)` — Rails' POST-201 idiom.
/// The kwarg value is a polymorphic record reference; rewrite to
/// `RouteHelpers.<singular>_path(@x.id)` so the runtime's
/// `render(body, location: <string>)` sees a path string. Mirrors
/// the `redirect_to @x` polymorphic rewrite below; runs over render
/// Sends specifically (not redirect_to, which has its own pass).
pub(super) fn rewrite_render_location_kwarg(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            let new_args: Vec<Expr> = args
                .iter()
                .map(|a| rewrite_location_in_kwargs(a))
                .collect();
            if new_args
                .iter()
                .zip(args.iter())
                .all(|(a, b)| std::ptr::eq(a.node.as_ref(), b.node.as_ref()))
            {
                // No change — let map_expr recurse normally.
                return None;
            }
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: method.clone(),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

/// If `arg` is a kwarg-Hash with a `location:` entry whose value is
/// a polymorphic record ref (Ivar), rewrite that entry's value to a
/// path-helper call. Other shapes pass through untouched.
fn rewrite_location_in_kwargs(arg: &Expr) -> Expr {
    let ExprNode::Hash { entries, kwargs: true } = &*arg.node else {
        return arg.clone();
    };
    let mut changed = false;
    let new_entries: Vec<(Expr, Expr)> = entries
        .iter()
        .map(|(k, v)| {
            let is_location = matches!(
                &*k.node,
                ExprNode::Lit { value: Literal::Sym { value } }
                    if value.as_str() == "location"
            );
            if !is_location {
                return (k.clone(), v.clone());
            }
            match &*v.node {
                ExprNode::Ivar { name } => {
                    changed = true;
                    (k.clone(), polymorphic_path(name, v.span))
                }
                _ => (k.clone(), v.clone()),
            }
        })
        .collect();
    if !changed {
        return arg.clone();
    }
    Expr::new(
        arg.span,
        ExprNode::Hash {
            entries: new_entries,
            kwargs: true,
        },
    )
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
// `<x>_path` / `<x>_url` route-helper prefix. Bare calls to route
// helpers (`Send` with no recv whose method ends in `_path` or `_url`)
// get the `RouteHelpers.` receiver added. Spinel's runtime defines
// every helper as a module function on `RouteHelpers`; controllers
// and tests must reach them through that namespace, since the
// `xxx_path` / `xxx_url` magic Rails injects via include doesn't
// exist here.
//
// This pass runs AFTER `rewrite_redirect_to` so the polymorphic
// rewrite's freshly-synthesized `RouteHelpers.x_path(...)` calls (which
// have a recv) are skipped — only original bare calls get the prefix.
// ---------------------------------------------------------------------------

pub fn rewrite_route_helpers(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if method.as_str().ends_with("_path")
                || method.as_str().ends_with("_url") =>
        {
            // `RouteHelpers` only emits `_path` helpers — Rails'
            // `_url` form differs by host prefix, which we don't
            // model. Fold `_url` onto its `_path` twin so test/
            // controller bodies that use the URL form resolve.
            let raw = method.as_str();
            let dispatch_method = if let Some(stem) = raw.strip_suffix("_url") {
                Symbol::from(format!("{stem}_path"))
            } else {
                method.clone()
            };
            // Polymorphic AR-instance → `.id` extraction. Rails
            // accepts `article_url(@article)` and dispatches via
            // implicit `.id`; the route_helpers in this codebase take
            // an `id: number` directly. Extract `.id` for:
            //   - Ivar args (`@article` → `@article.id`)
            //   - Class-method calls on capital-named Const recvs
            //     (`Article.last`, `Article.find(1)` → `<x>.id`).
            //     Heuristic at lower time: a Send whose recv is a
            //     Const with capitalized first segment is almost
            //     always a model class method returning an instance.
            // Already-projected args (`@article.id`) pass through
            // since they're Sends with method `id` — adding another
            // `.id` would double-wrap, so detect that shape.
            let projected_args: Vec<Expr> = args
                .iter()
                .map(|arg| {
                    let needs_id = match &*arg.node {
                        ExprNode::Ivar { .. } => true,
                        ExprNode::Send { recv: Some(r), method, .. }
                            if method.as_str() != "id" =>
                        {
                            matches!(
                                &*r.node,
                                ExprNode::Const { path }
                                    if path.first().map(|s| {
                                        s.as_str()
                                            .chars()
                                            .next()
                                            .is_some_and(|c| c.is_ascii_uppercase())
                                    }).unwrap_or(false)
                            )
                        }
                        _ => false,
                    };
                    if needs_id {
                        Expr::new(
                            arg.span,
                            ExprNode::Send {
                                recv: Some(arg.clone()),
                                method: Symbol::from("id"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        )
                    } else {
                        rewrite_route_helpers(arg)
                    }
                })
                .collect();
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(const_path(&["RouteHelpers"], e.span)),
                    method: dispatch_method,
                    args: projected_args,
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

/// `@params.fetch("sym", "0").to_i` — used for the single-symbol
/// expect shape. `fetch` with a default returns non-nil so the
/// `.to_i` chain compiles under strict targets (Crystal). Default
/// `"0"` parses to integer 0 — matches the spinel-blog convention
/// for missing-id-as-unsaved-sentinel. String key matches the
/// request-body parser's String-keyed Hash; a Symbol key would miss.
fn params_index_to_i(sym: &Symbol, span: Span) -> Expr {
    // `@params.fetch("<sym>", "0").to_s.to_i` — the leading `.to_s`
    // bridges the recursive `Roundhouse::ParamValue` union (String |
    // Hash | Array) into a single String, so the subsequent `.to_i`
    // type-checks on strict targets. For the only access pattern
    // this rewrite covers (`params.expect(:id)` scalar lookup), the
    // value is always a String leaf at runtime — the `.to_s` is a
    // no-op on String (Ruby/Crystal) / `String(x)` coercion (TS),
    // matching Rails' string-default param semantics.
    let fetched = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("params", span)),
            method: Symbol::from("fetch"),
            args: vec![
                Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str { value: sym.as_str().to_string() },
                    },
                ),
                Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Str { value: "0".to_string() } },
                ),
            ],
            block: None,
            parenthesized: true,
        },
    );
    let to_s = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(fetched),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(to_s),
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

/// `@flash[:<key>]` — used by render-rewrite to pass the controller's
/// flash slots through to view extra_params.
fn flash_lookup(span: Span, key: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("flash", span)),
            method: Symbol::from("[]"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Sym { value: Symbol::from(key) },
                },
            )],
            block: None,
            parenthesized: false,
        },
    )
}
