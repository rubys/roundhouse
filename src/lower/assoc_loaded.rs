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
    let resolved = resolve_owner_model(owner.as_ref(), enclosing, sole_includer, by_model, assoc)
        .filter(|id| by_model.get(id).is_some_and(|names| names.contains(assoc)));
    // A view/test receiver whose model the name does not single out
    // (`widget.parts.loaded?` in a partial, when Widget and Gadget both
    // `has_many :parts`): the same widening `association(:name).loaded?`
    // takes below. Every declaring model synthesizes the same flat
    // predicate, so the spelling is right whichever one the receiver is.
    let stamp = match (&resolved, owner) {
        (None, Some(base)) => match widened_owner(base, enclosing, by_model, assoc) {
            Some(id) => Some(id),
            None => return,
        },
        (None, None) => return,
        (Some(_), _) => None,
    };
    let Some(owner_model) = resolved.or_else(|| stamp.clone()) else {
        return;
    };
    let flat = Symbol::from(flat_name(assoc));
    let reload_ty = reload.then(|| Ty::Array {
        elem: Box::new(inner.ty.as_ref().and_then(|t| match t {
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
                args: vec![],
            });
            Some(s)
        }
        Some(base) => {
            let mut recv = base.clone();
            if let Some(id) = stamp {
                recv.ty = Some(Ty::Class { id, args: vec![] });
            }
            Some(recv)
        }
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

/// The model to stamp on a view/test receiver of `<recv>.<name>.loaded?`
/// (or `.reload`) that `resolve_owner_model` could not single out: some
/// model declares `has_many :<name>`, and the receiver is not typed as
/// (or as a union with) a model that lacks it. `None` in a model method,
/// where an unresolved receiver stays as written. The first declaring
/// model by name, so the stamp does not depend on map order.
fn widened_owner(
    base: &Expr,
    enclosing: Option<&ClassId>,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> Option<ClassId> {
    if enclosing.is_some_and(|id| by_model.contains_key(id)) {
        return None;
    }
    let typed_as_model = |ty: &Ty| matches!(ty, Ty::Class { id, .. } if by_model.contains_key(id));
    let refused = match base.ty.as_ref() {
        Some(ty @ Ty::Class { .. }) => typed_as_model(ty),
        // A union with a model alternative that lacks the name (the
        // others' would have resolved it already).
        Some(Ty::Union { variants }) => variants.iter().any(typed_as_model),
        _ => false,
    };
    if refused {
        return None;
    }
    by_model
        .iter()
        .filter(|(_, names)| names.contains(name))
        .map(|(id, _)| id)
        .min_by(|a, b| a.0.as_str().cmp(b.0.as_str()))
        .cloned()
}

/// True when at least one model synthesizes `<name>_loaded?`.
fn any_model_declares_name(
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> bool {
    by_model.values().any(|names| names.contains(name))
}

/// A ClassId that synthesizes `<name>_loaded?`, for stamping the
/// rewritten send's receiver type on strict targets. When several
/// models share the name (Message and Membership both `:room`), any
/// declaring class is enough — the spelling is identical.
fn a_model_declaring(
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    name: &Symbol,
) -> Option<ClassId> {
    by_model
        .iter()
        .find(|(_, names)| names.contains(name))
        .map(|(id, _)| id.clone())
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
    let flat = Symbol::from(format!("{}_loaded?", name.as_str()));
    let in_model = enclosing.is_some_and(|id| flat_loaded.contains_key(id));
    // Test/view explicit receivers: unique-name fails when Message and
    // Membership both `belongs_to :room`, and `message.reload` often
    // types as `ActiveRecord::Base` so typed resolve has no flat
    // `room_loaded?`. Name existence on *some* model gates the widen,
    // but a known receiver model that itself lacks the flat predicate
    // (polymorphic `belongs_to`) keeps the dynamic path. Stamp a
    // declaring ClassId on the kept receiver for strict targets.
    // Model methods and implicit-self keep the stricter resolve below.
    if let Some(base) = owner {
        if !in_model {
            let typed = owner_from_typed_recv(base, flat_loaded, &name);
            let allow = match &typed {
                // `owner_from_typed_recv` only returns Some when every
                // class alt declares `name` — or a single Class id. A
                // concrete model key that omits the name (poly assoc)
                // must not rewrite.
                Some(id) if flat_loaded.contains_key(id) => {
                    flat_loaded.get(id).is_some_and(|n| n.contains(&name))
                }
                // Base / untyped / unknown: any declaring model is enough.
                _ => any_model_declares_name(flat_loaded, &name),
            };
            if allow {
                let mut recv = base.clone();
                let stamp = typed
                    .filter(|id| {
                        flat_loaded.get(id).is_some_and(|n| n.contains(&name))
                    })
                    .or_else(|| a_model_declaring(flat_loaded, &name));
                if let Some(id) = stamp {
                    recv.ty = Some(Ty::Class { id, args: vec![] });
                }
                let mut rewritten = Expr::new(
                    expr.span,
                    ExprNode::Send {
                        recv: Some(recv),
                        method: flat,
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                rewritten.ty = Some(Ty::Bool);
                *expr = rewritten;
                return true;
            }
        }
    }
    let Some(owner_model) = resolve_owner_model(
        owner.as_ref(),
        enclosing,
        sole_includer,
        flat_loaded,
        &name,
    ) else {
        return false;
    };
    if !flat_loaded
        .get(&owner_model)
        .is_some_and(|names| names.contains(&name))
    {
        return false;
    }
    let new_recv = match owner {
        None => {
            let mut s = Expr::new(inner.span, ExprNode::SelfRef);
            s.ty = Some(Ty::Class {
                id: owner_model,
                args: vec![],
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
                args: vec![],
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
