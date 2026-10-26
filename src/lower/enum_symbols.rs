//! `where(role: :bot)` → `where(role: 2)`.
//!
//! Rails' `enum :role, %i[member administrator bot]` lets every query
//! and assignment name the LABEL where the column stores an integer;
//! ActiveRecord translates at runtime. The declaration itself expands at
//! ingest into scopes, predicates and bang writers that already carry
//! the stored value — but a HAND-WRITTEN label (`User.new(role:
//! :administrator)`, `where.not(role: :bot)`, `update! status:
//! :deactivated`) had nothing to translate it, so the symbol was
//! assigned or compared verbatim. `User.new(role: :administrator)
//! .administrator?` answered false, because the predicate the same
//! declaration generated compares against `1`.
//!
//! `Model::enums` exists for exactly this and had no consumer: its own
//! doc comment says it "is the only thing that can tell a hand-written
//! `where(role: :bot)` which integer `:bot` means". This is that
//! consumer.
//!
//! **Keyed by MODEL and column.** The call's model comes from its
//! receiver: a model constant, a receiver typed as a relation, record or
//! record list of a model, `where.not`'s `where`, or (no receiver) the
//! model whose body the call sits in. Two models may declare an enum on
//! columns of the same name with different labels (`role` is common),
//! and Rails casts through the relation's own model. When the receiver
//! names no model (`user_params.merge(role: :administrator)`), the
//! column's mapping is used if every model that declares it agrees, and
//! a column two models map DIFFERENTLY is left alone rather than guessed
//! at. The narrowing that makes this safe is threefold: the call has to
//! be one of ActiveRecord's attribute-taking methods (so an ARIA `role:
//! :button` in a view helper is untouched), the key has to name an enum
//! column, and the VALUE has to be one of that enum's labels.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};

/// ActiveRecord methods that take column=>value attribute hashes. A
/// symbol under any other call is somebody else's keyword.
const ATTR_METHODS: &[&str] = &[
    "new", "create", "create!", "build", "first_or_create", "first_or_create!",
    "first_or_initialize", "where", "not", "rewhere", "find_by", "find_by!",
    "find_or_create_by", "find_or_create_by!", "find_or_initialize_by",
    "create_or_find_by", "create_or_find_by!",
    "exists?", "update", "update!", "update_all", "update_attribute",
    "update_columns", "assign_attributes", "where_scope",
    // `user_params.merge(role: :administrator)` — campfire builds the
    // attribute hash and hands it straight to `User.new`.
    "merge",
];

type Mapping = std::collections::HashMap<String, Literal>;

/// model → column → (label → stored value), the model-agnostic column
/// map for receivers that name no model, plus the class-body constants a
/// `where(role: ROLES)` may name in place of a literal label list.
#[derive(Default)]
pub(crate) struct EnumMap {
    /// Every model's own enum columns.
    by_model: std::collections::HashMap<crate::ident::ClassId, std::collections::HashMap<String, Mapping>>,
    /// Every model of the app, with or without enums: a receiver that
    /// names one of these is mapped through that model's enums only.
    models: std::collections::HashSet<crate::ident::ClassId>,
    /// column → mapping, for columns every declaring model maps alike.
    columns: std::collections::HashMap<String, Mapping>,
    /// `NAME = %i[a b]` (optionally `.freeze`) in a model or controller
    /// body: its Symbols, keyed by `NAME` and by `Owner::NAME`. A bare
    /// name two classes bind to different lists is left out.
    symbol_lists: std::collections::HashMap<String, Vec<crate::ident::Symbol>>,
}

impl EnumMap {
    pub(crate) fn is_empty(&self) -> bool {
        self.by_model.is_empty()
    }
}

pub fn apply_enum_symbol_lowering(app: &mut App) {
    let map = enum_columns(app);
    if map.is_empty() {
        return;
    }
    super::for_each_owned_hook_body(app, &mut |owner, e| rewrite(e, &map, owner));
    for view in &mut app.views {
        rewrite(&mut view.body, &map, None);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &map, None);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &map, None);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &map, None);
        }
    }
}

/// Every model's enum columns, plus the column → (label → stored value)
/// map of the columns the app declares CONSISTENTLY. A column two models
/// map differently is not in the model-agnostic map: a receiver that
/// names no model has no single right answer for it, and guessing one
/// would write the wrong integer into a real query.
pub(crate) fn enum_columns(app: &App) -> EnumMap {
    let by_model: std::collections::HashMap<_, _> = app
        .models
        .iter()
        .filter(|m| !m.enums.is_empty())
        .map(|m| {
            let columns = m
                .enums
                .iter()
                .map(|(column, labels)| (column.as_str().to_string(), labels.iter().cloned().collect()))
                .collect();
            (m.name.clone(), columns)
        })
        .collect();
    let models = app.models.iter().map(|m| m.name.clone()).collect();
    let columns = enum_column_mappings(app);
    let symbol_lists = if by_model.is_empty() { Default::default() } else { symbol_list_constants(app) };
    EnumMap { by_model, models, columns, symbol_lists }
}

fn enum_column_mappings(
    app: &App,
) -> std::collections::HashMap<String, std::collections::HashMap<String, Literal>> {
    let mut out: std::collections::HashMap<String, std::collections::HashMap<String, Literal>> =
        Default::default();
    let mut conflicted: std::collections::HashSet<String> = Default::default();
    for model in &app.models {
        for (column, labels) in &model.enums {
            let col = column.as_str().to_string();
            if conflicted.contains(&col) {
                continue;
            }
            let mapping: std::collections::HashMap<String, Literal> =
                labels.iter().cloned().collect();
            match out.get(&col) {
                Some(existing) if !same_mapping(existing, &mapping) => {
                    out.remove(&col);
                    conflicted.insert(col);
                }
                Some(_) => {}
                None => {
                    out.insert(col, mapping);
                }
            }
        }
    }
    out
}

/// Every model's and controller's `NAME = %i[...]` / `NAME = [:a,
/// :b].freeze` class-body constant whose elements are all Symbols.
fn symbol_list_constants(app: &App) -> std::collections::HashMap<String, Vec<crate::ident::Symbol>> {
    use crate::ident::Symbol;
    let mut out: std::collections::HashMap<String, Vec<Symbol>> = Default::default();
    let mut conflicted: std::collections::HashSet<String> = Default::default();
    let mut add = |owner: &str, expr: &Expr| {
        let ExprNode::Assign { target: crate::expr::LValue::Const { path }, value } = &*expr.node else { return };
        let [name] = path.as_slice() else { return };
        let Some(symbols) = symbol_list(value) else { return };
        out.insert(format!("{owner}::{}", name.as_str()), symbols.clone());
        let bare = name.as_str().to_string();
        if conflicted.contains(&bare) {
            return;
        }
        match out.get(&bare) {
            Some(existing) if *existing != symbols => {
                out.remove(&bare);
                conflicted.insert(bare);
            }
            Some(_) => {}
            None => {
                out.insert(bare, symbols);
            }
        }
    };
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                add(model.name.0.as_str(), expr);
            }
        }
    }
    for controller in &app.controllers {
        for item in &controller.body {
            if let crate::dialect::ControllerBodyItem::Unknown { expr, .. } = item {
                add(controller.name.0.as_str(), expr);
            }
        }
    }
    out
}

/// The Symbols of `%i[a b]`, `[:a, :b]`, or either with `.freeze`.
fn symbol_list(value: &Expr) -> Option<Vec<crate::ident::Symbol>> {
    let value = match &*value.node {
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "freeze" && args.is_empty() => recv,
        _ => value,
    };
    let ExprNode::Array { elements, .. } = &*value.node else { return None };
    if elements.is_empty() {
        return None;
    }
    elements
        .iter()
        .map(|el| match &*el.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn same_mapping(
    a: &std::collections::HashMap<String, Literal>,
    b: &std::collections::HashMap<String, Literal>,
) -> bool {
    a.len() == b.len()
        && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| literal_eq(v, w)))
}

fn literal_eq(a: &Literal, b: &Literal) -> bool {
    match (a, b) {
        (Literal::Int { value: x }, Literal::Int { value: y }) => x == y,
        (Literal::Str { value: x }, Literal::Str { value: y }) => x == y,
        (Literal::Sym { value: x }, Literal::Sym { value: y }) => x == y,
        _ => false,
    }
}

fn rewrite(expr: &mut Expr, map: &EnumMap, owner: Option<&crate::ident::ClassId>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, map, owner));
    rewrite_node(expr, map, owner);
}

/// `owner` is the class whose body the expression sits in, if any: a
/// receiverless `where(role: …)` in a model's body is on that model.
pub(crate) fn rewrite_node(expr: &mut Expr, map: &EnumMap, owner: Option<&crate::ident::ClassId>) {
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else {
        return;
    };
    if !ATTR_METHODS.contains(&method.as_str()) {
        return;
    }
    let model = match recv {
        Some(recv) => receiver_model(recv, map),
        None => owner.filter(|id| map.models.contains(*id)).cloned(),
    };
    let model_columns = model.as_ref().map(|id| map.by_model.get(id));
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (key, value) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value: col } } = &*key.node else {
                continue;
            };
            let mapping = match model_columns {
                // The receiver names a model: only its own enums apply.
                Some(columns) => columns.and_then(|c| c.get(col.as_str())),
                None => map.columns.get(col.as_str()),
            };
            let Some(mapping) = mapping else { continue };
            expand_symbol_list_constant(value, mapping, &map.symbol_lists);
            rewrite_value(value, mapping);
        }
    }
}

/// The model an attribute-taking call's receiver is on: a model constant
/// (`Widget.where`), a receiver typed as one of its relations, records
/// or record lists, or `where.not`'s `where` on any of those.
fn receiver_model(recv: &Expr, map: &EnumMap) -> Option<crate::ident::ClassId> {
    use crate::ty::Ty;
    let known = |id: &crate::ident::ClassId| map.models.contains(id).then(|| id.clone());
    if let ExprNode::Const { path } = &*recv.node {
        let name = path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
        return known(&crate::ident::ClassId(crate::ident::Symbol::from(name.trim_start_matches("::"))));
    }
    let from_ty = match recv.ty.as_ref() {
        Some(Ty::Relation { of }) => known(of),
        Some(Ty::Class { id, args }) if args.is_empty() => known(id),
        Some(Ty::Array { elem }) => match &**elem {
            Ty::Class { id, args } if args.is_empty() => known(id),
            _ => None,
        },
        _ => None,
    };
    if from_ty.is_some() {
        return from_ty;
    }
    // `Widget.where(active: true).where.not(role: :admin)`: `not`'s
    // receiver is the argument-less `where`, whose receiver is the
    // relation.
    match &*recv.node {
        ExprNode::Send { recv: Some(inner), method, args, block: None, .. }
            if method.as_str() == "where" && args.is_empty() =>
        {
            receiver_model(inner, map)
        }
        _ => None,
    }
}

/// `where(role: ROLES)` with `ROLES = %i[admin owner].freeze`: Rails
/// casts each element of the constant's list through the enum type, as
/// it does a literal list. The constant reaches the SQL as Symbols
/// otherwise, and the condition matches no row. Replaced by the literal
/// list (then mapped element-wise by `rewrite_value`) only when every
/// element is one of this column's labels; a constant whose definition
/// is not a literal Symbol list, or is ambiguous, is left as written.
fn expand_symbol_list_constant(
    value: &mut Expr,
    mapping: &Mapping,
    lists: &std::collections::HashMap<String, Vec<crate::ident::Symbol>>,
) {
    let ExprNode::Const { path } = &*value.node else { return };
    let key = path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
    let key = key.trim_start_matches("::");
    let Some(symbols) = lists.get(key) else { return };
    if !symbols.iter().all(|sym| mapping.contains_key(sym.as_str())) {
        return;
    }
    let elements = symbols
        .iter()
        .map(|sym| {
            crate::lower::typing::with_ty(
                Expr::new(value.span, ExprNode::Lit { value: Literal::Sym { value: sym.clone() } }),
                crate::ty::Ty::Sym,
            )
        })
        .collect();
    *value = Expr::new(value.span, ExprNode::Array { elements, style: crate::expr::ArrayStyle::Brackets });
}

/// One attribute VALUE: a bare label, or an ARRAY of them.
///
/// `where(status: [ :active, :banned ])` is the same translation as the
/// scalar — Rails casts each element through the enum's type and renders
/// `IN (0, 2)`. Without the array arm the symbols reached the SQL
/// verbatim and the condition matched nothing, silently: campfire's
/// account page asks for exactly this set and rendered an EMPTY user
/// list, which reads as a missing feature rather than a wrong query.
///
/// Element-wise, and only for elements that ARE labels — the same
/// narrowing the scalar arm uses. A heterogeneous array is left with its
/// non-label elements untouched rather than refused, which is also what
/// Rails does with one.
fn rewrite_value(
    value: &mut Expr,
    mapping: &std::collections::HashMap<String, Literal>,
) {
    if let ExprNode::Array { elements, .. } = &mut *value.node {
        let mut all_int = !elements.is_empty();
        for el in elements.iter_mut() {
            let rewrote = rewrite_label(el, mapping);
            all_int &= rewrote.is_some_and(|l| matches!(l, Literal::Int { .. }));
        }
        // The container's own type follows its elements: an `Array[Sym]`
        // stamped at ingest, left standing over Int elements, is the kind
        // of disagreement a strict target reads as the element type.
        if all_int {
            value.ty = Some(crate::ty::Ty::Array {
                elem: std::sync::Arc::new(crate::ty::Ty::Int),
            });
        }
        return;
    }
    rewrite_label(value, mapping);
}

/// Replace a label Symbol with its stored value in place; answers the
/// stored literal when it did.
fn rewrite_label(
    value: &mut Expr,
    mapping: &std::collections::HashMap<String, Literal>,
) -> Option<Literal> {
    let ExprNode::Lit { value: Literal::Sym { value: label } } = &*value.node else {
        return None;
    };
    let stored = mapping.get(label.as_str())?.clone();
    *value = crate::lower::typing::with_ty(
        Expr::new(value.span, ExprNode::Lit { value: stored.clone() }),
        match stored {
            Literal::Int { .. } => crate::ty::Ty::Int,
            Literal::Nil => crate::ty::Ty::Nil,
            _ => crate::ty::Ty::Str,
        },
    );
    Some(stored)
}
