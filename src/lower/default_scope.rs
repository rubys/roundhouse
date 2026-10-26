//! `default_scope { where(…) }` — the shapes lowered.
//!
//! Rails applies a model's default scope to every relation built on it,
//! when the relation is built: class finders and terminals (`count`,
//! `find`, `find_by`, `exists?`, `first`/`last`), association readers
//! (`shelf.gadgets`, `note.gadget`), `joins`/`includes` of the model, and
//! bulk writes (`update_all`, `delete_all`). `unscoped`, `unscoped { }`
//! and `unscope(where: :col)` lift it; `reload` and the UPDATE or DELETE
//! of a loaded record go by primary key and never see it. A new record
//! takes the scope's attributes before the ones it is given.
//!
//! The scope is applied in ONE place, the runtime Relation's
//! constructor (`runtime/ruby/active_record/relation.rb`), which asks
//! the model for `_default_scope_where`. So a scoped model is only ever
//! queried through the Relation: every lowering that writes SQL at
//! transpile time declines for it ([`ClassInfo::default_scoped`] in the
//! arel builder), the join tails carry the scope's predicate, and the
//! targets without a runtime Relation report the chain as residue
//! rather than read the whole table.
//!
//! Two shapes are lowered ([`DefaultScope`]):
//!
//! - `where(col: literal, …)`: `_default_scope_where` answers the
//!   literals, and `new` starts from them.
//! - `where(col: <expr>, …)`, optionally under a trailing `if <cond>` or
//!   `unless <cond>` (`where(account_id: Current.account.id) if
//!   Current.account`): `_default_scope_where` evaluates the block's
//!   expression each time it is called, so each relation keeps the
//!   values of the moment it was built, and a guard that turns the scope
//!   off (the block's `nil`) applies nothing. `new` reads the same
//!   method. A join into such a model is not composed at transpile time
//!   (no SQL written there can read the values).
//!
//! Anything else in a `default_scope` (an `order`, a bare method call or
//! `self` in a value, `all_queries:`, two declarations, a literal whose
//! kind the column does not hold) stays the reported model-DSL gap.
//!
//! [`ClassInfo::default_scoped`]: crate::analyze::ClassInfo

use crate::dialect::{Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// The `default_scope` call sites in the model's body.
fn declarations(model: &Model) -> impl Iterator<Item = &Expr> {
    model.body.iter().filter_map(|item| {
        let ModelBodyItem::Unknown { expr, .. } = item else { return None };
        let ExprNode::Send { recv, method, .. } = &*expr.node else { return None };
        let own = recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
        (own && method.as_str() == "default_scope").then_some(expr)
    })
}

/// A model's one `default_scope` in a shape the runtime Relation applies.
#[derive(Debug, Clone)]
pub enum DefaultScope {
    /// `where(col: literal, …)`: the same conditions on every relation.
    Static(Vec<(Symbol, Literal)>),
    /// `where(col: <expr>, …)`, optionally under a trailing `if <cond>` or
    /// `unless <cond>`: evaluated each time a relation is built.
    Dynamic(DynamicScope),
}

/// `default_scope { where(account_id: Current.account.id) if Current.account }`.
/// `body` is the block's own expression with each `where(…)` call in it
/// replaced by its condition hash and each `nil` branch left as it is:
/// either the bare hash, or an `If` whose one branch is the hash and the
/// other `nil` (Ruby's modifier `if`/`unless`). `columns` are the hash's
/// keys, in source order.
#[derive(Debug, Clone)]
pub struct DynamicScope {
    pub body: Expr,
    pub columns: Vec<Symbol>,
}

impl DefaultScope {
    /// The columns the scope's `where` names.
    pub fn columns(&self) -> Vec<Symbol> {
        match self {
            DefaultScope::Static(pairs) => pairs.iter().map(|(c, _)| c.clone()).collect(),
            DefaultScope::Dynamic(d) => d.columns.clone(),
        }
    }
}

/// The model's one `default_scope`, when the runtime Relation can apply
/// it: [`static_default_scope`], or a `where(…)` whose values are read
/// when each relation is built ([`DynamicScope`]). `None` for no scope
/// and for every shape that stays a reported gap.
pub fn default_scope(model: &Model) -> Option<DefaultScope> {
    if let Some(pairs) = static_default_scope(model) {
        return Some(DefaultScope::Static(pairs));
    }
    dynamic_default_scope(model).map(DefaultScope::Dynamic)
}

/// The block's body expression of the model's one `default_scope`, when
/// it is a block (or lambda argument) with no parameters.
fn scope_body(model: &Model) -> Option<&Expr> {
    let mut decls = declarations(model);
    let decl = decls.next()?;
    if decls.next().is_some() {
        return None;
    }
    let ExprNode::Send { args, block, .. } = &*decl.node else { return None };
    let lambda = match (block, args.as_slice()) {
        (Some(b), []) => b,
        (None, [l]) => l,
        _ => return None,
    };
    let ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. } =
        &*lambda.node
    else {
        return None;
    };
    if !params.is_empty() || !extra_params.is_empty() {
        return None;
    }
    let mut body = body;
    while let ExprNode::Seq { exprs } = &*body.node {
        let [only] = exprs.as_slice() else { return None };
        body = only;
    }
    Some(body)
}

/// `where(col: <expr>, …)` [`if`|`unless` <cond>] whose values or guard
/// read state at the time the relation is built. Each value and the
/// guard must mean the same thing in a class method and in `initialize`
/// (both evaluate the block here): constants, literals, and calls on
/// them (`Current.account.id`, `Current.account&.id`). A bare method
/// call, `self`, an instance variable or a block keeps the shape a gap,
/// and so does a scope whose values are all literals with no guard
/// (that is [`static_default_scope`]'s, or a literal it refused).
fn dynamic_default_scope(model: &Model) -> Option<DynamicScope> {
    let body = scope_body(model)?;
    let (guarded, where_call) = match &*body.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            if !portable(cond, &[]) {
                return None;
            }
            match (is_nil(then_branch), is_nil(else_branch)) {
                (false, true) => (true, then_branch),
                (true, false) => (true, else_branch),
                _ => return None,
            }
        }
        _ => (false, body),
    };
    let (hash, columns, all_literal) = where_hash(model, where_call)?;
    if !guarded && all_literal {
        return None;
    }
    let rebuilt = match &*body.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let swap = |b: &Expr| if is_nil(b) { b.clone() } else { hash.clone() };
            let mut e = Expr::new(
                body.span,
                ExprNode::If { cond: cond.clone(), then_branch: swap(then_branch), else_branch: swap(else_branch) },
            );
            e.ty = None;
            e
        }
        _ => hash,
    };
    Some(DynamicScope { body: rebuilt, columns })
}

fn is_nil(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// `where(col: v, …)` over the model's own columns, each column once,
/// every value [`portable`] and every literal one its column holds. The
/// condition hash, its columns, and whether every value is a literal.
fn where_hash(model: &Model, call: &Expr) -> Option<(Expr, Vec<Symbol>, bool)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    if method.as_str() != "where" {
        return None;
    }
    let [hash] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
    if entries.is_empty() {
        return None;
    }
    let mut columns: Vec<Symbol> = Vec::new();
    let mut all_literal = true;
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: col } } = &*k.node else { return None };
        let column = model.attributes.fields.get(col)?;
        if columns.contains(col) {
            return None;
        }
        match &*v.node {
            ExprNode::Lit { value } => {
                if !literal_fits(value, column) {
                    return None;
                }
            }
            _ => {
                if !portable(v, &[]) {
                    return None;
                }
                all_literal = false;
            }
        }
        columns.push(col.clone());
    }
    Some((hash.clone(), columns, all_literal))
}

/// An expression that reads the same in any method of the model: built
/// from literals, constants and calls on them, with `&&`/`||`/`!` and
/// the locals a `&.` desugars to. No bare (implicit-self) call, `self`,
/// instance variable, block, assignment or control flow.
fn portable(e: &Expr, bound: &[Symbol]) -> bool {
    match &*e.node {
        ExprNode::Lit { value } => !matches!(value, Literal::Regex { .. }),
        ExprNode::Const { .. } => true,
        // A local is portable only when a `Let` inside the expression
        // binds it: one captured from outside the block does not exist
        // in the method the expression is lowered into.
        ExprNode::Var { name, .. } => bound.contains(name),
        ExprNode::Send { recv: Some(recv), args, block: None, .. } => {
            portable(recv, bound) && args.iter().all(|a| portable(a, bound))
        }
        ExprNode::BoolOp { left, right, .. } => portable(left, bound) && portable(right, bound),
        ExprNode::Let { name, value, body, .. } => {
            let mut inner = bound.to_vec();
            inner.push(name.clone());
            portable(value, bound) && portable(body, &inner)
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            portable(cond, bound) && portable(then_branch, bound) && portable(else_branch, bound)
        }
        ExprNode::Array { elements, .. } => elements.iter().all(|a| portable(a, bound)),
        ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| portable(k, bound) && portable(v, bound)),
        _ => false,
    }
}

/// Is `target`'s default scope a condition on `foreign_key` alone? Then
/// Rails' association scope REPLACES it with the owner's key rather than
/// ANDing both: `b.projects` under `default_scope { where(account_id:
/// Current.account.id) }` reads b's projects, where
/// `Project.where(account_id: b.id)` reads none (Rails 8.1.4). A scope
/// over more columns keeps the AND, which answers fewer rows, never
/// more: `unscope(where:)` cannot split the scope's one condition.
///
/// Rails' preloader does NOT replace it: `includes(:projects)` ANDs the
/// scope with the owners' keys. So a model with such a reader always
/// gets its batch loaders (`emit::ruby::library::apply_preload_lowering`):
/// a preload hint never falls back to the replacing lazy reader.
pub fn scope_is_foreign_key(models: &[Model], target: &ClassId, foreign_key: &Symbol) -> bool {
    models
        .iter()
        .find(|m| &m.name == target)
        .and_then(default_scope)
        .is_some_and(|s| s.columns() == [foreign_key.clone()])
}

/// Does this association's reader replace its target's default scope on
/// the key ([`scope_is_foreign_key`])? Only a plain has_many: no
/// `through:`, `as:` or association scope. Those are exactly the readers
/// with a batch loader (`emit::ruby::library::preload_targets`), so an
/// eager load of one never falls back to the replacing reader; the
/// others keep the AND, which answers fewer rows than Rails, never more.
pub fn reader_replaces_scope(models: &[Model], assoc: &crate::dialect::Association) -> bool {
    matches!(assoc, crate::dialect::Association::HasMany {
        target, foreign_key, through: None, as_interface: None, scope: None, ..
    } if scope_is_foreign_key(models, target, foreign_key))
}

/// The `(column, literal)` pairs of the model's one `default_scope`
/// when it is `where(col: literal, …)` over its own columns, in source
/// order. `None` for no default scope and for every other shape.
pub fn static_default_scope(model: &Model) -> Option<Vec<(Symbol, Literal)>> {
    let body = scope_body(model)?;
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*body.node else {
        return None;
    };
    if method.as_str() != "where" {
        return None;
    }
    let [hash] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: col } } = &*k.node else { return None };
        let ExprNode::Lit { value } = &*v.node else { return None };
        let column = model.attributes.fields.get(col)?;
        if !literal_fits(value, column) || out.iter().any(|(c, _)| c == col) {
            return None;
        }
        out.push((col.clone(), value.clone()));
    }
    Some(out)
}

/// A literal a column of type `ty` stores as written. A Symbol is left
/// out: on an enum column it names a stored value, elsewhere a String.
fn literal_fits(value: &Literal, ty: &Ty) -> bool {
    let wanted = match value {
        Literal::Nil => return true,
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        Literal::Sym { .. } | Literal::Regex { .. } => return false,
    };
    match ty {
        Ty::Union { variants } => variants.iter().any(|v| *v == wanted),
        other => *other == wanted,
    }
}

/// `col = literal` as SQL, for an association join's ON clause. `None`
/// for a literal this does not render.
pub fn sql_predicate(table: &str, col: &Symbol, value: &Literal) -> Option<String> {
    let rhs = match value {
        Literal::Nil => return Some(format!("{table}.{col} IS NULL")),
        Literal::Bool { value: true } => "TRUE".to_string(),
        Literal::Bool { value: false } => "FALSE".to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } if value.is_finite() => format!("{value:?}"),
        Literal::Str { value } => format!("'{}'", value.replace('\'', "''")),
        _ => return None,
    };
    Some(format!("{table}.{col} = {rhs}"))
}

/// `def self._default_scope_where; { archived: false }; end` — the
/// conditions the runtime Relation's constructor adds (`where!`), and
/// so also the attributes a record built through it starts with.
/// `ActiveRecord::Base` answers `{}`.
pub fn synth_default_scope_where(model: &Model, scope: &[(Symbol, Literal)]) -> crate::dialect::MethodDef {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility};
    use crate::effect::EffectSet;
    use crate::span::Span;
    let lit_ty = |l: &Literal| match l {
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        _ => Ty::Nil,
    };
    let mut value_ty = None::<Ty>;
    for (_, l) in scope {
        let t = lit_ty(l);
        value_ty = match value_ty {
            None => Some(t),
            Some(prev) if prev == t => Some(prev),
            Some(_) => Some(Ty::Untyped),
        };
    }
    let hash_ty = Ty::Hash {
        key: std::sync::Arc::new(Ty::Sym),
        value: std::sync::Arc::new(value_ty.unwrap_or(Ty::Untyped)),
    };
    let entries = scope
        .iter()
        .map(|(col, l)| {
            let mut k = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: col.clone() } });
            k.ty = Some(Ty::Sym);
            let mut v = Expr::new(Span::synthetic(), ExprNode::Lit { value: l.clone() });
            v.ty = Some(lit_ty(l));
            (k, v)
        })
        .collect();
    let mut body = Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: false });
    body.ty = Some(hash_ty.clone());
    MethodDef {
        visibility: MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name: Symbol::from("_default_scope_where"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body,
        signature: Some(crate::lower::model_to_library::fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `_default_scope_where` for a [`DynamicScope`]: the block's expression,
/// read when the runtime Relation's constructor calls it, so each
/// relation keeps the values of the moment it was built. A `nil` branch
/// (the guard turned the scope off) answers `{}`, which applies nothing.
///
/// ```ruby
/// def self._default_scope_where
///   if Current.account
///     { account_id: Current.account.id }
///   else
///     {}
///   end
/// end
/// ```
pub fn synth_dynamic_default_scope_where(model: &Model, scope: &DynamicScope) -> crate::dialect::MethodDef {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility};
    use crate::effect::EffectSet;
    use crate::span::Span;
    let hash_of = |e: &Expr| -> Option<Expr> {
        matches!(&*e.node, ExprNode::Hash { .. }).then(|| e.clone())
    };
    let hash = match &*scope.body.node {
        ExprNode::If { then_branch, else_branch, .. } => hash_of(then_branch).or_else(|| hash_of(else_branch)),
        _ => hash_of(&scope.body),
    }
    .expect("a dynamic default scope carries its condition hash");
    let mut value_ty = None::<Ty>;
    if let ExprNode::Hash { entries, .. } = &*hash.node {
        for (_, v) in entries {
            let t = v.ty.clone().unwrap_or(Ty::Untyped);
            value_ty = match value_ty {
                None => Some(t),
                Some(prev) if prev == t => Some(prev),
                Some(_) => Some(Ty::Untyped),
            };
        }
    }
    let hash_ty = Ty::Hash {
        key: std::sync::Arc::new(Ty::Sym),
        value: std::sync::Arc::new(value_ty.unwrap_or(Ty::Untyped)),
    };
    let typed_hash = |mut e: Expr| {
        if let ExprNode::Hash { kwargs, .. } = &mut *e.node {
            *kwargs = false;
        }
        e.ty = Some(hash_ty.clone());
        e
    };
    let empty = || {
        let mut e = Expr::new(Span::synthetic(), ExprNode::Hash { entries: Vec::new(), kwargs: false });
        e.ty = Some(hash_ty.clone());
        e
    };
    let branch = |b: &Expr| if is_nil(b) { empty() } else { typed_hash(b.clone()) };
    let body = match &*scope.body.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let mut e = Expr::new(
                Span::synthetic(),
                ExprNode::If { cond: cond.clone(), then_branch: branch(then_branch), else_branch: branch(else_branch) },
            );
            e.ty = Some(hash_ty.clone());
            e
        }
        _ => typed_hash(scope.body.clone()),
    };
    MethodDef {
        visibility: MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name: Symbol::from("_default_scope_where"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body,
        signature: Some(crate::lower::model_to_library::fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}
