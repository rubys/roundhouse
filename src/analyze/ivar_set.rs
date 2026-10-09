//! Statically resolvable `instance_variable_set` names.
//!
//! Rails controllers often write the ivar a template will read through
//! `instance_variable_set(:@article, record)` or
//! `instance_variable_set("@#{controller_name.singularize}", record)`
//! rather than a syntactic `@article =`. The view channel harvests
//! `@ivar =` writes; without folding those Kernel calls the template
//! reports `ivar_unresolved` for an assignment that already ran.
//!
//! Names that do not fold — `instance_variable_set(params[:name], x)`,
//! interpolation of a runtime local — stay unbound. That is fail-closed.

use std::collections::{BTreeSet, HashMap};

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::naming;
use crate::ty::Ty;

/// Context for folding an ivar name against the class whose `self` the
/// write runs as (the including controller, for a concern method).
pub(crate) struct IvarNameEnv<'a> {
    pub self_class: Option<&'a ClassId>,
    pub owned: Option<&'a HashMap<Symbol, Expr>>,
    pub lookup: Option<&'a dyn Fn(&Symbol) -> Option<&'a Expr>>,
    /// Model classes keyed by the ivar Rails would conventionally
    /// assign (`Page` → `page`). Used to narrow a polymorphic RHS
    /// once the set name folds to that convention.
    pub models_by_ivar: Option<&'a HashMap<Symbol, ClassId>>,
}

impl IvarNameEnv<'static> {
    pub const NONE: Self = Self {
        self_class: None,
        owned: None,
        lookup: None,
        models_by_ivar: None,
    };
}

impl<'a> IvarNameEnv<'a> {
    fn method(&self, name: &Symbol) -> Option<&'a Expr> {
        if let Some(map) = self.owned {
            return map.get(name);
        }
        if let Some(lookup) = self.lookup {
            return lookup(name);
        }
        None
    }
}

enum Folded {
    Str(String),
    Class(String),
}

impl Folded {
    fn as_str(&self) -> &str {
        match self {
            Folded::Str(s) | Folded::Class(s) => s,
        }
    }
}

/// `instance_variable_set` / `instance_variable_get` on implicit self
/// or an explicit `self`.
pub(crate) fn is_self_ivar_reflection(recv: &Option<Expr>, method: &str) -> bool {
    if method != "instance_variable_set" && method != "instance_variable_get" {
        return false;
    }
    match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    }
}

/// If `stmt` is a self `instance_variable_set` whose name folds without
/// looking up other methods, the ivar and the assigned type.
pub(crate) fn binding_from_send(stmt: &Expr, self_ty: Option<&Ty>) -> Option<(Symbol, Ty)> {
    let ExprNode::Send { recv, method, args, .. } = &*stmt.node else {
        return None;
    };
    if !is_self_ivar_reflection(recv, method.as_str()) || method.as_str() != "instance_variable_set"
    {
        return None;
    }
    // Ruby's Kernel#instance_variable_set takes exactly two arguments;
    // extra args raise before any assignment.
    if args.len() != 2 {
        return None;
    }
    let self_class = match self_ty {
        Some(Ty::Class { id, .. }) => Some(id),
        _ => None,
    };
    let env = IvarNameEnv {
        self_class,
        owned: None,
        lookup: None,
        models_by_ivar: None,
    };
    let name = fold_ivar_name(&args[0], &env)?;
    let ty = args[1].ty.clone().filter(|t| !t.is_open())?;
    Some((name, ty))
}

pub(crate) fn fold_ivar_name(expr: &Expr, env: &IvarNameEnv<'_>) -> Option<Symbol> {
    let folded = fold_value(expr, env, 0, &mut BTreeSet::new())?;
    let raw = folded.as_str().trim_start_matches('@');
    if !is_ivar_ident(raw) {
        return None;
    }
    Some(Symbol::from(raw))
}

fn is_ivar_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn fold_value(
    expr: &Expr,
    env: &IvarNameEnv<'_>,
    depth: u32,
    visiting: &mut BTreeSet<Symbol>,
) -> Option<Folded> {
    if depth > 12 {
        return None;
    }
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Some(Folded::Str(value.clone())),
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(Folded::Str(value.as_str().to_string())),
        ExprNode::SelfRef => env
            .self_class
            .map(|id| Folded::Class(id.0.as_str().to_string())),
        ExprNode::StringInterp { parts } => {
            let mut out = String::new();
            for part in parts {
                match part {
                    crate::expr::InterpPart::Text { value } => out.push_str(value),
                    crate::expr::InterpPart::Expr { expr } => {
                        out.push_str(fold_value(expr, env, depth + 1, visiting)?.as_str());
                    }
                }
            }
            Some(Folded::Str(out))
        }
        ExprNode::Send {
            recv, method, args, block: None, ..
        } => fold_send(recv, method, args, env, depth, visiting),
        ExprNode::Seq { exprs } => exprs.last().and_then(|e| fold_value(e, env, depth + 1, visiting)),
        ExprNode::Return { value } => fold_value(value, env, depth + 1, visiting),
        _ => None,
    }
}

fn fold_send(
    recv: &Option<Expr>,
    method: &Symbol,
    args: &[Expr],
    env: &IvarNameEnv<'_>,
    depth: u32,
    visiting: &mut BTreeSet<Symbol>,
) -> Option<Folded> {
    let name = method.as_str();
    if recv.is_none() && args.is_empty() {
        if let Some(class) = env.self_class {
            match name {
                "controller_name" => {
                    return Some(Folded::Str(controller_name_of(class)));
                }
                "controller_path" => {
                    return Some(Folded::Str(super::controller_view_prefix(class)));
                }
                _ => {}
            }
        }
        if visiting.insert(method.clone()) {
            let body = env.method(method);
            let folded = body.and_then(|b| fold_value(b, env, depth + 1, visiting));
            visiting.remove(method);
            if folded.is_some() {
                return folded;
            }
        }
    }

    let recv = recv.as_ref()?;
    if name == "class" && args.is_empty() {
        return match &*recv.node {
            ExprNode::SelfRef => env
                .self_class
                .map(|id| Folded::Class(id.0.as_str().to_string())),
            _ => match fold_value(recv, env, depth + 1, visiting)? {
                Folded::Class(c) => Some(Folded::Class(c)),
                Folded::Str(_) => None,
            },
        };
    }

    if matches!(name, "to_s" | "to_str" | "name") && args.is_empty() {
        return Some(Folded::Str(
            fold_value(recv, env, depth + 1, visiting)?.as_str().to_string(),
        ));
    }

    // `self.controller_name` is the same Kernel method as a receiverless
    // call — Rails style often writes the explicit form.
    if matches!(&*recv.node, ExprNode::SelfRef) && args.is_empty() {
        if let Some(class) = env.self_class {
            match name {
                "controller_name" => {
                    return Some(Folded::Str(controller_name_of(class)));
                }
                "controller_path" => {
                    return Some(Folded::Str(super::controller_view_prefix(class)));
                }
                _ => {}
            }
        }
    }

    let recv_s = fold_value(recv, env, depth + 1, visiting)?;
    let s = recv_s.as_str();
    let out = match name {
        "demodulize" if args.is_empty() => naming::demodulize(s).to_string(),
        "deconstantize" if args.is_empty() => match s.rsplit_once("::") {
            Some((head, _)) => head.to_string(),
            None => String::new(),
        },
        // Keep `/` from `naming::underscore` (matches runtime). A path
        // segment is not an ivar ident — `is_ivar_ident` fails closed
        // rather than flattening to `_` and seeding the wrong name.
        "underscore" if args.is_empty() => naming::underscore(s),
        // Fold only when `naming` agrees with the grounded runtime's
        // regular-suffix chop. Irregular / uncountable answers must
        // stay unresolved until the runtime table matches (inv. 6).
        "singularize" if args.is_empty() => {
            let named = naming::singularize(s);
            if named != runtime_singularize(s) {
                return None;
            }
            named
        }
        "camelize" | "camelcase" if args.is_empty() => naming::camelize(s),
        "downcase" if args.is_empty() => s.to_ascii_lowercase(),
        "upcase" if args.is_empty() => s.to_ascii_uppercase(),
        "strip" if args.is_empty() => s.trim().to_string(),
        "chomp" if args.is_empty() => s.trim_end_matches('\n').to_string(),
        // Match lowering: only the single-arg form grounds to
        // `ActiveSupport.remove`. Zero-arg raises at runtime; multi-arg
        // stays dynamic — both fail closed here.
        "remove" if args.len() == 1 => {
            let Folded::Str(pat) = fold_value(&args[0], env, depth + 1, visiting)? else {
                return None;
            };
            s.replace(&pat, "")
        }
        "delete_suffix" if args.len() == 1 => {
            let Folded::Str(suf) = fold_value(&args[0], env, depth + 1, visiting)? else {
                return None;
            };
            s.strip_suffix(&suf).unwrap_or(s).to_string()
        }
        "delete_prefix" if args.len() == 1 => {
            let Folded::Str(pre) = fold_value(&args[0], env, depth + 1, visiting)? else {
                return None;
            };
            s.strip_prefix(&pre).unwrap_or(s).to_string()
        }
        _ => return None,
    };
    Some(Folded::Str(out))
}

/// Regular-suffix singularize — must stay byte-identical to
/// `ActiveSupport.singularize` in `runtime/ruby/active_support_ext.rb`.
fn runtime_singularize(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    if s.len() > 3 && s.ends_with("ies") {
        return format!("{}y", &s[..s.len() - 3]);
    }
    if s.ends_with("ses")
        || s.ends_with("xes")
        || s.ends_with("zes")
        || s.ends_with("ches")
        || s.ends_with("shes")
    {
        return s[..s.len() - 2].to_string();
    }
    if s.ends_with('s') && !s.ends_with("ss") {
        return s[..s.len() - 1].to_string();
    }
    s.to_string()
}

/// `ArticlesController` → `"articles"`; `Admin::UsersController` →
/// `"users"`. Shared with the controller lowerer's AOT string-literal
/// overrides — one owner so acronym / strip rules cannot drift.
pub(crate) fn controller_name_of(class: &ClassId) -> String {
    let leaf = naming::demodulize(class.0.as_str());
    let stripped = leaf.strip_suffix("Controller").unwrap_or(leaf);
    naming::snake_case(stripped)
}

/// Record a folded `instance_variable_set` write into `out` when the
/// assigned value already carries a type.
pub(crate) fn harvest_ivar_set(
    recv: &Option<Expr>,
    method: &Symbol,
    args: &[Expr],
    env: &IvarNameEnv<'_>,
    out: &mut HashMap<Symbol, Ty>,
) {
    if !is_self_ivar_reflection(recv, method.as_str()) || method.as_str() != "instance_variable_set"
    {
        return;
    }
    // Exact arity — see `binding_from_send`.
    if args.len() != 2 {
        return;
    }
    let Some(name) = fold_ivar_name(&args[0], env) else {
        return;
    };
    let Some(ty) = args[1].ty.clone().filter(|t| !t.is_open()) else {
        return;
    };
    let ty = narrow_to_named_model(&name, ty, env.models_by_ivar);
    let merged = match out.remove(&name) {
        Some(prev) => super::body::join_ivar_slot(prev, ty),
        None => ty,
    };
    out.insert(name, merged);
}

/// When the folded ivar name is exactly one model's conventional
/// name and the RHS is a union that includes that model, keep only
/// that model — Nil and sibling leafable variants are dropped.
/// Polymorphic `leaf.leafable` written through
/// `instance_variable_set "@#{instance_name}", …` would otherwise
/// leave `@page` as `Page | Section | Picture | nil`. After the
/// filter ran under that name, the template reads a concrete model;
/// association nilability is not part of the ivar binding.
fn narrow_to_named_model(
    name: &Symbol,
    ty: Ty,
    models_by_ivar: Option<&HashMap<Symbol, ClassId>>,
) -> Ty {
    let Some(models) = models_by_ivar else {
        return ty;
    };
    let Some(model) = models.get(name) else {
        return ty;
    };
    let model_ty = Ty::Class {
        id: model.clone(),
        args: vec![],
    };
    match &ty {
        Ty::Class { id, .. } if id == model => ty,
        Ty::Union { variants } => {
            let has_model = variants.iter().any(|v| {
                matches!(v, Ty::Class { id, .. } if id == model)
            });
            if !has_model {
                return ty;
            }
            model_ty
        }
        _ => ty,
    }
}

/// Build `page` → `Page` for every model in the app.
pub(crate) fn models_by_conventional_ivar<'a>(
    models: impl Iterator<Item = &'a ClassId>,
) -> HashMap<Symbol, ClassId> {
    let mut out = HashMap::new();
    for id in models {
        let leaf = naming::demodulize(id.0.as_str());
        let key = Symbol::from(naming::snake_case(leaf).as_str());
        let namespaced = id.0.as_str().contains("::");
        match out.get(&key) {
            None => {
                out.insert(key, id.clone());
            }
            // Prefer the top-level model when both `Page` and
            // `Admin::Page` claim `page`, regardless of ingest order.
            Some(existing) if existing.0.as_str().contains("::") && !namespaced => {
                out.insert(key, id.clone());
            }
            _ => {}
        }
    }
    out
}
