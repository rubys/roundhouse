//! View/partial render-site resolution: which partials a view renders,
//! the locals they receive, dynamic-render ivar collection, and controller
//! action → view-name mapping helpers. Extracted verbatim from
//! `src/analyze/mod.rs` (pure code motion).

use std::collections::HashMap;

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// A view name identifies a partial when any path segment starts with `_`
/// (Rails convention: `app/views/articles/_article.html.erb` → view name
/// `articles/_article`).
pub(super) fn is_partial_view_name(name: &Symbol) -> bool {
    name.as_str().split('/').any(|seg| seg.starts_with('_'))
}

/// Layout templates live under `layouts/` (`layouts/application`).
pub(super) fn is_layout_view_name(name: &Symbol) -> bool {
    name.as_str().starts_with("layouts/")
}

/// Walk a view body collecting `render ...` call sites. For each recognized
/// shape, determine the target partial's view name and the locals the render
/// passes into it, merging into `out`.
///
/// Shapes recognized (matching real-blog + the common idioms):
/// - `render @collection` where `@collection` types as `Array<Class>` →
///   partial `pluralize(snake(Class))/_snake(Class)`, local `snake(Class)`.
/// - `render some_single_record` typing as `Class` → same partial path, local
///   bound to the record's type.
/// - `render "name", k1: v1, k2: v2` → partial name resolved relative to the
///   current view's directory (`articles/index` + `"form"` → `articles/_form`),
///   locals from the trailing kwarg hash.
/// - `render partial: "name", locals: { k: v }` → same resolution, locals
///   sourced from the `locals:` hash.
///
/// Call-site argument shapes outside these cases are skipped silently;
/// an unrecognized render just leaves the target partial seeded by other
/// sites (or unseeded).
pub(super) fn extract_partial_render_sites(
    expr: &Expr,
    current_view: &Symbol,
    out: &mut HashMap<Symbol, HashMap<Symbol, Ty>>,
    targets: &mut Vec<Symbol>,
) {
    match &*expr.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            // Detect the `render` call shape (no explicit receiver, or the
            // receiver is an implicit context — Rails makes both work).
            let render_args = if is_implicit_render_receiver(recv.as_ref()) && method.as_str() == "render" {
                Some(args.as_slice())
            } else if is_turbo_stream_render(recv.as_ref(), method.as_str()) {
                // Turbo Stream actions accept the same partial/collection
                // options as `render`, but the render is nested in the
                // action call (`turbo_stream.replace target, partial: …`).
                // Those options are the actual call-site evidence for the
                // partial's local type and must seed it just like render.
                args.iter()
                    .find(|arg| matches!(&*arg.node, ExprNode::Hash { entries, .. } if entries.iter().any(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "partial"))))
                    .map(std::slice::from_ref)
            } else {
                None
            };
            if let Some(render_args) = render_args {
                if let Some((partial_name, locals)) = interpret_render_call(render_args, current_view) {
                    // Record the renderer→partial edge so the caller can
                    // propagate the renderer's ivar context to the partial
                    // (partials render in their parent's view context and
                    // read its `@ivars`).
                    targets.push(partial_name.clone());
                    let entry = out.entry(partial_name).or_default();
                    for (k, v) in locals {
                        // One partial may be rendered from both well-typed
                        // and gradual sites. Keep useful evidence when a
                        // later site only knows `Untyped`/an inference var;
                        // otherwise traversal order silently erases the
                        // concrete record type used to type the partial.
                        match entry.get(&k) {
                            Some(existing) if !existing.is_unknown() && v.is_unknown() => {}
                            Some(existing) if existing.is_unknown() && !v.is_unknown() => {
                                entry.insert(k, v);
                            }
                            _ => {
                                entry.insert(k, v);
                            }
                        }
                    }
                }
            }
            if let Some(r) = recv {
                extract_partial_render_sites(r, current_view, out, targets);
            }
            for a in args {
                extract_partial_render_sites(a, current_view, out, targets);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out, targets);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                extract_partial_render_sites(e, current_view, out, targets);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                extract_partial_render_sites(k, current_view, out, targets);
                extract_partial_render_sites(v, current_view, out, targets);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_partial_render_sites(cond, current_view, out, targets);
            extract_partial_render_sites(then_branch, current_view, out, targets);
            extract_partial_render_sites(else_branch, current_view, out, targets);
        }
        ExprNode::Case { scrutinee, arms } => {
            extract_partial_render_sites(scrutinee, current_view, out, targets);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    extract_partial_render_sites(g, current_view, out, targets);
                }
                extract_partial_render_sites(&arm.body, current_view, out, targets);
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            extract_partial_render_sites(left, current_view, out, targets);
            extract_partial_render_sites(right, current_view, out, targets);
        }
        ExprNode::Let { value, body, .. } => {
            extract_partial_render_sites(value, current_view, out, targets);
            extract_partial_render_sites(body, current_view, out, targets);
        }
        ExprNode::Lambda { body, .. } => {
            extract_partial_render_sites(body, current_view, out, targets);
        }
        ExprNode::Apply { fun, args, block } => {
            extract_partial_render_sites(fun, current_view, out, targets);
            for a in args {
                extract_partial_render_sites(a, current_view, out, targets);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out, targets);
            }
        }
        ExprNode::Assign { value, .. } => {
            extract_partial_render_sites(value, current_view, out, targets);
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    extract_partial_render_sites(expr, current_view, out, targets);
                }
            }
        }
        _ => {}
    }
}

/// Rails templates use both `render(...)` and `self.render(...)`. The
/// latter is still the view-context render API; treating every receiver as
/// an unrelated object silently drops its partial-local type evidence.
fn is_implicit_render_receiver(recv: Option<&Expr>) -> bool {
    match recv.map(|r| &*r.node) {
        None => true,
        Some(ExprNode::Var { name, .. }) => name.as_str() == "self",
        _ => false,
    }
}

fn is_turbo_stream_render(recv: Option<&Expr>, method: &str) -> bool {
    matches!(method, "append" | "prepend" | "replace" | "update" | "before" | "after")
        && matches!(recv.map(|r| &*r.node), Some(ExprNode::Var { name, .. }) if name.as_str() == "turbo_stream")
}

/// Collect the ivar names that views use as *dynamic* partial-render
/// targets — `render @above` or `render partial: @above`. These name
/// a content partial whose identity is only known at runtime (the
/// ivar holds a string literal like `'for_domain'` assigned by the
/// action). Pairing this set with the per-action string-literal
/// assignments (`@above = 'for_domain'`) lets the analyzer seed the
/// `_for_domain` partial with that action's ivars — the edge
/// `extract_partial_render_sites` can't resolve statically.
pub(super) fn collect_dynamic_render_ivars(expr: &Expr, out: &mut std::collections::HashSet<Symbol>) {
    if let ExprNode::Send { recv, method, args, .. } = &*expr.node {
        if recv.is_none() && method.as_str() == "render" {
            for arg in args {
                match &*arg.node {
                    // `render @above`
                    ExprNode::Ivar { name } => {
                        out.insert(name.clone());
                    }
                    // `render partial: @above` (the kwarg-hash form)
                    ExprNode::Hash { entries, .. } => {
                        for (k, v) in entries {
                            let is_partial_key = matches!(
                                &*k.node,
                                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "partial"
                            );
                            if is_partial_key {
                                if let ExprNode::Ivar { name } = &*v.node {
                                    out.insert(name.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_dynamic_render_ivars(c, out));
}

/// Collect `@ivar = "string literal"` assignments whose ivar name is
/// in `targets` (the dynamic-render ivar set). Returns each literal
/// value — the content-partial basename the action wants rendered.
pub(super) fn collect_content_partial_literals(
    expr: &Expr,
    targets: &std::collections::HashSet<Symbol>,
    out: &mut Vec<String>,
) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*expr.node {
        if targets.contains(name) {
            if let ExprNode::Lit { value: Literal::Str { value } } = &*value.node {
                out.push(value.clone());
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_content_partial_literals(c, targets, out));
}

/// Resolve a content-partial literal (`"for_domain"`, `"saved/subnav"`)
/// to a partial view name. A value with a `/` carries its own
/// directory (`saved/subnav` → `saved/_subnav`); a bare value is
/// relative to the rendering controller's view prefix (`for_domain` in
/// HomeController → `home/_for_domain`).
pub(super) fn content_partial_view_name(literal: &str, prefix: &str) -> Symbol {
    match literal.rfind('/') {
        Some(idx) => {
            let (dir, base) = literal.split_at(idx);
            Symbol::from(format!("{}/_{}", dir, &base[1..]))
        }
        None => Symbol::from(format!("{}/_{}", prefix, literal)),
    }
}

/// Collect every full-template view an action renders via an explicit
/// `render :action => "x"` / `render :template => "x"` /
/// `render_to_string :action => "x"` call anywhere in its body —
/// including calls buried in a `respond_to`/`Rails.cache.fetch` block
/// that never surface as the action's primary `RenderTarget`. The
/// `tree` action's `render_to_string :action => "tree"` inside a cache
/// block is the motivating case: without this the `tree` view gets no
/// ivar seed at all. `:action` names resolve relative to the
/// controller's view prefix; `:template` names are taken verbatim
/// (they already carry their directory).
pub(super) fn collect_action_render_views(expr: &Expr, prefix: &str, out: &mut Vec<Symbol>) {
    if let ExprNode::Send { recv, method, args, .. } = &*expr.node {
        if recv.is_none() && matches!(method.as_str(), "render" | "render_to_string") {
            for arg in args {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    for (k, v) in entries {
                        let key = match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                            _ => continue,
                        };
                        let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else {
                            continue;
                        };
                        match key {
                            "action" => {
                                let name = if value.contains('/') {
                                    value.clone()
                                } else {
                                    format!("{}/{}", prefix, value)
                                };
                                out.push(Symbol::from(name));
                            }
                            "template" => out.push(Symbol::from(value.clone())),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_action_render_views(c, prefix, out));
}

/// Figure out the target partial name and the locals a `render(...)` call
/// passes to it. Returns `None` for shapes not yet handled.
fn interpret_render_call(
    args: &[Expr],
    current_view: &Symbol,
) -> Option<(Symbol, HashMap<Symbol, Ty>)> {
    if args.is_empty() {
        return None;
    }
    let first = &args[0];

    // Collection / single-record render: `render @articles`, `render @article.comments`,
    // `render @article` — first arg types as Array<Class> or Class.
    if let Some(ty) = first.ty.as_ref() {
        if let Some((partial, local_name, elem_ty)) = partial_from_receiver_type(ty) {
            let mut locals = HashMap::new();
            locals.insert(Symbol::from(local_name.as_str()), elem_ty);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    // Named partial: `render "name", k: v, k: v` or `render "name"`.
    if let ExprNode::Lit { value: Literal::Str { value: name } } = &*first.node {
        let partial = resolve_partial_path(name, current_view);
        let mut locals = HashMap::new();
        for a in &args[1..] {
            if let ExprNode::Hash { entries, .. } = &*a.node {
                for (k, v) in entries {
                    if let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node {
                        if let Some(ty) = v.ty.clone() {
                            locals.insert(key.clone(), ty);
                        }
                    }
                }
            }
        }
        return Some((Symbol::from(partial.as_str()), locals));
    }

    // Hash form: `render partial: "name", locals: { k: v }` — first arg is a Hash.
    // The collection form rides the same hash: `render partial: "status",
    // collection: @statuses[, as: :status]` binds an implicit local named
    // after the partial's basename (or the `as:` override), typed as the
    // collection's element, plus Rails' `<name>_counter` index local.
    if let ExprNode::Hash { entries, .. } = &*first.node {
        let mut partial_name: Option<String> = None;
        // `render layout: "rooms/layouts/edit", locals: { … } do … end`
        // — a PARTIAL used as a layout. Rails renders `_edit` with the
        // block as its content, in the caller's view context, so it
        // reads the caller's ivars and the given locals exactly like
        // any other partial. Keyed separately so an explicit
        // `partial:` in the same hash still wins.
        let mut layout_name: Option<String> = None;
        let mut locals: HashMap<Symbol, Ty> = HashMap::new();
        let mut collection_ty: Option<Ty> = None;
        let mut as_name: Option<Symbol> = None;
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            match key.as_str() {
                "partial" => {
                    if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                        partial_name = Some(value.clone());
                    }
                }
                "locals" => {
                    if let ExprNode::Hash { entries: loc_entries, .. } = &*v.node {
                        for (lk, lv) in loc_entries {
                            if let ExprNode::Lit { value: Literal::Sym { value: loc_key } } =
                                &*lk.node
                            {
                                if let Some(ty) = lv.ty.clone() {
                                    locals.insert(loc_key.clone(), ty);
                                }
                            }
                        }
                    }
                }
                "collection" => {
                    collection_ty = v.ty.clone();
                }
                "as" => {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                        as_name = Some(value.clone());
                    }
                }
                "layout" => {
                    if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                        layout_name = Some(value.clone());
                    }
                }
                _ => {}
            }
        }
        if let Some(name) = partial_name.or(layout_name) {
            if let Some(coll) = collection_ty {
                // Unknown/gradual collection still binds the local —
                // gradual element beats an unresolved bare name.
                let elem_ty = coll.collection_elem().unwrap_or(Ty::Untyped);
                let local = as_name.unwrap_or_else(|| {
                    let base = name.rsplit('/').next().unwrap_or(&name);
                    Symbol::from(base.trim_start_matches('_'))
                });
                locals
                    .entry(Symbol::from(format!("{}_counter", local.as_str()).as_str()))
                    .or_insert(Ty::Int);
                locals.entry(local).or_insert(elem_ty);
            }
            let partial = resolve_partial_path(&name, current_view);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    None
}

/// If the receiver type implies a collection/single-record render target,
/// return (partial_view_name, local_name, element_ty). For
/// `Array<Article>` / `Relation[Article]`: partial `articles/_article`,
/// local `article`, element `Article`.
pub(super) fn partial_from_receiver_type(ty: &Ty) -> Option<(String, String, Ty)> {
    if let Some(elem) = ty.collection_elem() {
        if let Ty::Class { id, .. } = &elem {
            let class_name = id.0.as_str();
            let local = crate::naming::snake_case(class_name);
            let folder = crate::naming::pluralize_snake(class_name);
            return Some((format!("{folder}/_{local}"), local, elem.clone()));
        }
        return None;
    }
    match ty {
        Ty::Class { id, .. } => {
            let class_name = id.0.as_str();
            let local = crate::naming::snake_case(class_name);
            let folder = crate::naming::pluralize_snake(class_name);
            Some((format!("{folder}/_{local}"), local, ty.clone()))
        }
        _ => None,
    }
}

/// Resolve a partial name relative to the current view's directory.
/// `"form"` in `articles/index` → `articles/_form`; `"shared/nav"` (absolute,
/// contains `/`) → `shared/_nav`.
pub(super) fn resolve_partial_path(name: &str, current_view: &Symbol) -> String {
    if let Some(idx) = name.rfind('/') {
        let (dir, file) = name.split_at(idx + 1);
        format!("{dir}_{file}")
    } else {
        let current = current_view.as_str();
        match current.rfind('/') {
            Some(idx) => format!("{}_{}", &current[..=idx], name),
            None => format!("_{name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_self_render_contributes_partial_local_type_evidence() {
        let user_ty = Ty::Class {
            id: crate::ident::ClassId(Symbol::from("Accounts::User")),
            args: vec![],
        };
        let sym = |s: &str| Expr::new(
            Default::default(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } },
        );
        let str_lit = |s: &str| Expr::new(
            Default::default(),
            ExprNode::Lit { value: Literal::Str { value: s.to_string() } },
        );
        let mut local_value = Expr::new(
            Default::default(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("account_user") },
        );
        local_value.ty = Some(user_ty.clone());
        let locals = Expr::new(
            Default::default(),
            ExprNode::Hash {
                entries: vec![(sym("user"), local_value)],
                kwargs: false,
            },
        );
        let kwargs = Expr::new(
            Default::default(),
            ExprNode::Hash {
                entries: vec![(sym("partial"), str_lit("users/user")), (sym("locals"), locals)],
                kwargs: true,
            },
        );
        let receiver = Expr::new(
            Default::default(),
            ExprNode::Var { id: crate::ident::VarId(1), name: Symbol::from("self") },
        );
        let render = Expr::new(
            Default::default(),
            ExprNode::Send {
                recv: Some(receiver),
                method: Symbol::from("render"),
                args: vec![kwargs],
                block: None,
                parenthesized: true,
            },
        );

        let mut sites = HashMap::new();
        let mut targets = Vec::new();
        extract_partial_render_sites(
            &render,
            &Symbol::from("accounts/show"),
            &mut sites,
            &mut targets,
        );
        assert_eq!(targets, [Symbol::from("users/_user")]);
        assert_eq!(sites[&Symbol::from("users/_user")][&Symbol::from("user")], user_ty);

        // Carry the extracted fact through the actual view lowerer: this
        // catches regressions where render collection works but the
        // partial's emitted signature still falls back to an unknown type.
        let view_name = Symbol::from("users/_user");
        let view = crate::dialect::View {
            name: view_name.clone(),
            format: Symbol::from("html"),
            locals: Default::default(),
            body: Expr::new(Default::default(), ExprNode::Lit { value: Literal::Nil }),
            strict_locals: Some(vec![crate::dialect::Param::positional(Symbol::from("user"))]),
            analysis_only: false,
            jbuilder: false,
        };
        let mut app = crate::App::default();
        app.partial_local_types = sites;
        let lowered = crate::lower::view_to_library::lower_view_to_library_class(&view, &app);
        let signature = lowered.methods[0].signature.as_ref().expect("partial signature");
        let Ty::Fn { params, .. } = signature else { panic!("expected function signature") };
        assert_eq!(params[0].ty, user_ty);
    }

    #[test]
    fn turbo_stream_collection_as_local_seeds_the_actual_partial_contract() {
        let user_ty = Ty::Class {
            id: crate::ident::ClassId(Symbol::from("User")),
            args: vec![],
        };
        let target = Expr::new(
            Default::default(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from("next_page_container") } },
        );
        let key = |name: &str| Expr::new(
            Default::default(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
        );
        let str_value = |value: &str| Expr::new(
            Default::default(),
            ExprNode::Lit { value: Literal::Str { value: value.to_string() } },
        );
        let mut collection = Expr::new(
            Default::default(),
            ExprNode::Ivar { name: Symbol::from("page") },
        );
        collection.ty = Some(Ty::Array { elem: Box::new(user_ty.clone()) });
        let options = Expr::new(
            Default::default(),
            ExprNode::Hash {
                entries: vec![
                    (key("partial"), str_value("accounts/users/user")),
                    (key("collection"), collection),
                    (key("as"), key("user")),
                ],
                kwargs: true,
            },
        );
        let receiver = Expr::new(
            Default::default(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("turbo_stream") },
        );
        let replace = Expr::new(
            Default::default(),
            ExprNode::Send {
                recv: Some(receiver),
                method: Symbol::from("replace"),
                args: vec![target, options],
                block: None,
                parenthesized: true,
            },
        );

        let mut sites = HashMap::new();
        let mut targets = Vec::new();
        extract_partial_render_sites(
            &replace,
            &Symbol::from("accounts/users/index"),
            &mut sites,
            &mut targets,
        );
        let partial = Symbol::from("accounts/users/_user");
        assert_eq!(targets, [partial.clone()]);
        assert_eq!(sites[&partial][&Symbol::from("user")], user_ty);

        // This call's local cannot be inferred from the plural directory;
        // only the collection/as: site identifies the element model.
        let view = crate::dialect::View {
            name: partial.clone(),
            format: Symbol::from("html"),
            locals: Default::default(),
            body: Expr::new(Default::default(), ExprNode::Lit { value: Literal::Nil }),
            strict_locals: None,
            analysis_only: false,
            jbuilder: false,
        };
        let mut app = crate::App::default();
        app.partial_local_types = sites;
        let lowered = crate::lower::view_to_library::lower_view_to_library_class(&view, &app);
        let signature = lowered.methods[0].signature.as_ref().expect("partial signature");
        let Ty::Fn { params, .. } = signature else { panic!("expected function signature") };
        assert_eq!(params[0].ty, user_ty);
    }
}
