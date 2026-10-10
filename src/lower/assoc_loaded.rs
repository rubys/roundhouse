//! Flatten Rails' AssociationProxy spelling onto Roundhouse's flat
//! association surface.
//!
//! Roundhouse's has_many readers return a plain Array (cache hit) or run
//! a query (cache miss) — never Rails' AssociationProxy — so the Rails
//! spelling `message.boosts.loaded?` has nothing to dispatch on. The
//! synthesizer already exposes the flag as a flat predicate
//! (`message.boosts_loaded?`); rewrite the two-hop form onto that name
//! so analyze, emit, and the Spinel AOT all see an ordinary Bool method.
//!
//! Same for `association(:name).target` → `name`: Rails' reflection
//! API reaches the cached association object; Roundhouse's readers ARE
//! that object (or its Array), so the hop collapses onto the reader.
//! Campfire's FTS index update uses
//! `association(:rich_text_body).target&.saved_change_to_body?`.
//!
//! Same shape as `has_json`'s two-hop flatten (`account.settings.foo?` →
//! `account.settings_foo?`): the intermediate object Rails invents is
//! erased, and every target keeps a typed one-hop call.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::Association;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Per-model has_many names that have a synthesized `<name>_loaded?`
/// reader. Keyed by owner ClassId so `room.boosts.loaded?` is not
/// rewritten when only `Message` declares `has_many :boosts`.
pub(crate) fn has_many_by_model(app: &App) -> HashMap<ClassId, HashSet<Symbol>> {
    let mut out: HashMap<ClassId, HashSet<Symbol>> = HashMap::new();
    for model in &app.models {
        // Every model gets a key so `in_model` owner checks don't treat
        // a has_many-less model (Room) as a view/library class.
        let entry = out.entry(model.name.clone()).or_default();
        for (_, assoc) in model.spanned_associations() {
            // Only has_many synthesizes `<name>_loaded?` /
            // `<name>_target` (see `model_to_library::associations`).
            // has_one keeps a singular reader with no flat loaded flag.
            if let Association::HasMany { name, .. } = assoc {
                entry.insert(name.clone());
            }
        }
    }
    out
}

/// Per-model association / rich-text reader names `association(:x).target`
/// may collapse onto. Keyed by owner so `room.association(:boosts).target`
/// is not rewritten when only `Message` declares `:boosts`.
pub(crate) fn association_readers_by_model(app: &App) -> HashMap<ClassId, HashSet<Symbol>> {
    let mut out: HashMap<ClassId, HashSet<Symbol>> = HashMap::new();
    for model in &app.models {
        let entry = out.entry(model.name.clone()).or_default();
        for (_, assoc) in model.spanned_associations() {
            entry.insert(assoc.name().clone());
        }
        for (_, attr) in crate::lower::rich_text::rich_text_attrs(model) {
            entry.insert(Symbol::from(format!("rich_text_{}", attr.as_str())));
        }
        if crate::lower::plain_text_attr::record_table_present(&app.schema) {
            for (_, attr) in crate::lower::plain_text_attr::plain_text_attrs(model) {
                entry.insert(Symbol::from(format!("markdown_{}", attr.as_str())));
            }
        }
    }
    out
}

/// Per-model association names with a synthesized flat `<name>_loaded?`:
/// has_many, has_one, non-polymorphic belongs_to, and the
/// `rich_text_<attr>` has_one. `association(:name).loaded?` rewrites onto
/// it; any other name (a polymorphic belongs_to, habtm) stays put.
pub(crate) fn flat_loaded_by_model(app: &App) -> HashMap<ClassId, HashSet<Symbol>> {
    let mut out: HashMap<ClassId, HashSet<Symbol>> = HashMap::new();
    for model in &app.models {
        let entry = out.entry(model.name.clone()).or_default();
        for (_, assoc) in model.spanned_associations() {
            match assoc {
                Association::HasMany { name, .. }
                | Association::HasOne { name, .. }
                | Association::BelongsTo { name, polymorphic: false, .. } => {
                    entry.insert(name.clone());
                }
                _ => {}
            }
        }
        for (_, attr) in crate::lower::rich_text::rich_text_attrs(model) {
            entry.insert(Symbol::from(format!("rich_text_{}", attr.as_str())));
        }
    }
    out
}

/// Resolve which model owns the association hop.
///
/// Typed explicit receiver wins — including a union, but only when every
/// class alternative declares `name`. Untyped explicit receivers in a
/// **model** method are left alone (unrewritten `.loaded?` / `.target`
/// must stay visible). Views, tests, and other non-model owners still
/// unique-name: view assigns are untyped, and Campfire ERB is
/// `message.boosts.loaded?` (view bodies are library classes, so
/// `enclosing` is Some). Implicit self uses the enclosing model, or a
/// concern module's sole model includer.
fn resolve_owner_model(
    recv: Option<&Expr>,
    enclosing: Option<&ClassId>,
    sole_includer: &HashMap<ClassId, ClassId>,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> Option<ClassId> {
    if let Some(base) = recv {
        if let Some(id) = owner_from_typed_recv(base, by_model, name) {
            return Some(id);
        }
        // Unique-name for view/test/non-model owners only. Model methods
        // with an untyped or mixed-union receiver stay unre-written.
        // View locals are `Ty::Untyped`, not `ty: None`.
        let in_model = enclosing.is_some_and(|id| by_model.contains_key(id));
        if !in_model {
            return unique_model_for_name(by_model, name);
        }
        return None;
    }
    let enclosing = enclosing?;
    if by_model.contains_key(enclosing) {
        return Some(enclosing.clone());
    }
    let includer = sole_includer.get(enclosing)?;
    by_model.contains_key(includer).then(|| includer.clone())
}

/// ClassId of a typed explicit receiver. Mixed-owner unions rewrite only
/// when every class alternative declares `name`; a missing type or a
/// non-class type returns `None` (no unique-name fallback).
fn owner_from_typed_recv(
    expr: &Expr,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> Option<ClassId> {
    match expr.ty.as_ref()? {
        Ty::Class { id, .. } => Some(id.clone()),
        Ty::Union { variants } => {
            let ids: Vec<ClassId> = variants
                .iter()
                .filter_map(|v| match v {
                    Ty::Class { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            if ids.is_empty() {
                return None;
            }
            ids.iter()
                .all(|id| by_model.get(id).is_some_and(|names| names.contains(name)))
                .then(|| ids[0].clone())
        }
        _ => None,
    }
}

fn unique_model_for_name(
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> Option<ClassId> {
    let mut matches = by_model
        .iter()
        .filter(|(_, names)| names.contains(name))
        .map(|(id, _)| id);
    let first = matches.next().cloned();
    if matches.next().is_some() {
        None
    } else {
        first
    }
}

/// Rewrite every `recv.assoc.loaded?` whose `assoc` is a known has_many
/// **on the receiver's model** into `recv.assoc_loaded?`, and every
/// `association(:name).target` into a bare `name` reader when `name`
/// is a known association/rich-text reader **on the receiver's model**.
/// Implicit-self forms get a `self.` hop (same collapse `has_json` uses).
pub fn apply_assoc_loaded_lowering(app: &mut App) -> Vec<Diagnostic> {
    let by_model = has_many_by_model(app);
    let readers = association_readers_by_model(app);
    let flat_loaded = flat_loaded_by_model(app);
    let sole_includer = app.sole_includer_of_modules();
    super::for_each_owned_hook_body(app, &mut |owner, e| {
        rewrite(e, owner, &sole_includer, &by_model, &readers, &flat_loaded);
    });
    for view in &mut app.views {
        rewrite(&mut view.body, None, &sole_includer, &by_model, &readers, &flat_loaded);
    }
    super::for_each_test_body(app, &mut |e| {
        rewrite(e, None, &sole_includer, &by_model, &readers, &flat_loaded);
    });
    Vec::new()
}

fn rewrite(
    expr: &mut Expr,
    enclosing: Option<&ClassId>,
    sole_includer: &HashMap<ClassId, ClassId>,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    readers: &HashMap<ClassId, HashSet<Symbol>>,
    flat_loaded: &HashMap<ClassId, HashSet<Symbol>>,
) {
    expr.node.for_each_child_mut(&mut |c| {
        rewrite(c, enclosing, sole_includer, by_model, readers, flat_loaded)
    });
    rewrite_node(expr, enclosing, sole_includer, by_model, readers, flat_loaded);
}

pub(crate) fn rewrite_node(
    expr: &mut Expr,
    enclosing: Option<&ClassId>,
    sole_includer: &HashMap<ClassId, ClassId>,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    readers: &HashMap<ClassId, HashSet<Symbol>>,
    flat_loaded: &HashMap<ClassId, HashSet<Symbol>>,
) {
    if rewrite_association_target(expr, enclosing, sole_includer, readers) {
        return;
    }
    if rewrite_association_loaded(expr, enclosing, sole_includer, flat_loaded) {
        return;
    }
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        ..
    } = &*expr.node
    else {
        return;
    };
    // `.loaded?` → `<assoc>_loaded?`; `.reload` → `reload_<assoc>`
    // (Rails' CollectionProxy#reload: the rows again, from the database).
    let flat_name = |assoc: &Symbol| match method.as_str() {
        "loaded?" => format!("{}_loaded?", assoc.as_str()),
        _ => format!("reload_{}", assoc.as_str()),
    };
    let reload = method.as_str() == "reload";
    if !(method.as_str() == "loaded?" || reload) || !args.is_empty() {
        return;
    }
    let ExprNode::Send {
        recv: owner,
        method: assoc,
        args: assoc_args,
        ..
    } = &*inner.node
    else {
        return;
    };
    if !assoc_args.is_empty() {
        return;
    }
    // Scope by the association receiver's model (`message` in
    // `message.boosts.loaded?`), not a global name set — a has_one or
    // plain method of the same name on another class must stay.
    let Some(owner_model) =
        resolve_owner_model(owner.as_ref(), enclosing, sole_includer, by_model, assoc)
    else {
        return;
    };
    let Some(names) = by_model.get(&owner_model) else {
        return;
    };
    if !names.contains(assoc) {
        return;
    }
    let flat = Symbol::from(flat_name(assoc));
    let reload_ty = reload.then(|| Ty::Array {
        elem: std::sync::Arc::new(inner.ty.as_ref().and_then(|t| match t {
            Ty::Array { elem } => Some((**elem).clone()),
            _ => None,
        }).unwrap_or(Ty::Untyped)),
    });
    // Explicit `message.boosts.loaded?` keeps `message` as receiver.
    // Implicit-self `boosts.loaded?` collapses to `self.boosts_loaded?`
    // — the same SelfRef hop `has_json` uses for `settings.foo?`.
    let new_recv = match owner {
        None => {
            let mut s = Expr::new(inner.span, ExprNode::SelfRef);
            s.ty = Some(Ty::Class {
                id: owner_model,
                args: vec![].into(),
            });
            Some(s)
        }
        Some(base) => Some(base.clone()),
    };
    let mut rewritten = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: flat,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    rewritten.ty = Some(reload_ty.unwrap_or(Ty::Bool));
    *expr = rewritten;
}

/// `association(:creator).loaded?` → `creator_loaded?` (or
/// `self.creator_loaded?` when implicit-self), for every association
/// with a synthesized flat predicate on the receiver's model.
fn rewrite_association_loaded(
    expr: &mut Expr,
    enclosing: Option<&ClassId>,
    sole_includer: &HashMap<ClassId, ClassId>,
    flat_loaded: &HashMap<ClassId, HashSet<Symbol>>,
) -> bool {
    let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "loaded?" || !args.is_empty() {
        return false;
    }
    let ExprNode::Send { recv: owner, method: assoc_method, args: assoc_args, .. } = &*inner.node
    else {
        return false;
    };
    if assoc_method.as_str() != "association" || assoc_args.len() != 1 {
        return false;
    }
    let Some(name) = sym_lit(&assoc_args[0]) else {
        return false;
    };
    let Some(owner_model) =
        resolve_owner_model(owner.as_ref(), enclosing, sole_includer, flat_loaded, &name)
    else {
        return false;
    };
    if !flat_loaded.get(&owner_model).is_some_and(|names| names.contains(&name)) {
        return false;
    }
    let new_recv = match owner {
        None => {
            let mut s = Expr::new(inner.span, ExprNode::SelfRef);
            s.ty = Some(Ty::Class { id: owner_model, args: vec![].into() });
            Some(s)
        }
        Some(base) => Some(base.clone()),
    };
    let mut rewritten = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: Symbol::from(format!("{}_loaded?", name.as_str())),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    rewritten.ty = Some(Ty::Bool);
    *expr = rewritten;
    true
}

/// `association(:rich_text_body).target` → `rich_text_body` (or
/// `self.rich_text_body` when the association call was implicit-self).
/// Scoped to the association call's receiver model so a name declared
/// only on an unrelated model does not rewrite onto this receiver.
fn rewrite_association_target(
    expr: &mut Expr,
    enclosing: Option<&ClassId>,
    sole_includer: &HashMap<ClassId, ClassId>,
    readers: &HashMap<ClassId, HashSet<Symbol>>,
) -> bool {
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        ..
    } = &*expr.node
    else {
        return false;
    };
    if method.as_str() != "target" || !args.is_empty() {
        return false;
    }
    let ExprNode::Send {
        recv: owner,
        method: assoc_method,
        args: assoc_args,
        ..
    } = &*inner.node
    else {
        return false;
    };
    if assoc_method.as_str() != "association" || assoc_args.len() != 1 {
        return false;
    }
    let Some(name) = sym_lit(&assoc_args[0]) else {
        return false;
    };
    let Some(owner_model) =
        resolve_owner_model(owner.as_ref(), enclosing, sole_includer, readers, &name)
    else {
        return false;
    };
    let Some(names) = readers.get(&owner_model) else {
        return false;
    };
    if !names.contains(&name) {
        return false;
    }
    let new_recv = match owner {
        None => {
            let mut s = Expr::new(inner.span, ExprNode::SelfRef);
            s.ty = Some(Ty::Class {
                id: owner_model,
                args: vec![].into(),
            });
            Some(s)
        }
        Some(base) => Some(base.clone()),
    };
    // Preserve the analyzed type so a safe-nav chain on `.target`
    // (`….target&.saved_change_to_body?`) still has a typed receiver
    // for emit — `Expr::new` alone leaves `ty: None`.
    let prior_ty = expr.ty.clone();
    let mut rewritten = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: name,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    rewritten.ty = prior_ty;
    *expr = rewritten;
    true
}

fn sym_lit(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.clone()),
        _ => None,
    }
}
