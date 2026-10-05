//! Diagnostics over graphql-ruby object types (`ingest::graphql_ruby`).
//!
//! The ingest pass gave each field a `__gql_value_<field>` method that
//! resolves the way graphql-ruby does, and inference typed it from the
//! schema's roots down. Two things come out of that:
//!
//! - The bodies of the GraphQL classes are walked like a controller's,
//!   so `check` reports inside them: a field whose `object` has no such
//!   method is a dispatch failure at the `field` call, which is where
//!   graphql-ruby's "Failed to implement" would point.
//! - A field declared `null: false` whose value can be nil is a
//!   `GraphqlNullableField` warning, unless the nil is one the database
//!   rules out (a required `belongs_to` on a NOT NULL column with a
//!   foreign key).

use std::collections::{BTreeMap, HashMap};

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::dialect::{Association, GraphqlResolution, LibraryClass, MethodDef};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::App;

pub(super) fn diagnose(app: &App, walk: fn(&Expr, &mut Vec<Diagnostic>)) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if app.graphql_types.is_empty() {
        return out;
    }
    // Indexed once: a large schema has thousands of types over a
    // hundred thousand library classes.
    let classes = class_index(app);
    let models: HashMap<&ClassId, &crate::dialect::Model> =
        app.models.iter().map(|m| (&m.name, m)).collect();
    for gql in &app.graphql_types {
        let Some(class) = classes.get(&gql.class).copied() else {
            continue;
        };
        let takes_arguments: Vec<&Symbol> = gql
            .fields
            .iter()
            .filter_map(|f| match &f.resolution {
                GraphqlResolution::Arguments { method } => Some(method),
                _ => None,
            })
            .collect();
        for method in &class.methods {
            let synthesized = gql.synthesized.contains(&method.name);
            // A modeled resolver's `resolve` is called with its typed
            // arguments; the rest of its methods (search_object's
            // `apply_*`) are called by code out of sight.
            let resolve = gql.resolver
                && method.name.as_str() == "resolve"
                && gql
                    .synthesized
                    .iter()
                    .any(|m| m.as_str() == "__gql_resolve");
            if (gql.resolver && !synthesized && !resolve) || takes_arguments.contains(&&method.name)
            {
                continue;
            }
            let mut found = Vec::new();
            walk(&method.body, &mut found);
            if synthesized {
                // From the generated plumbing only what graphql-ruby
                // would fail on: a method the object does not have.
                // A Hash object is read by key, not by method.
                found.retain(|d| match &d.kind {
                    DiagnosticKind::SendDispatchFailed { recv_ty, .. } => {
                        !matches!(recv_ty, Ty::Hash { .. })
                    }
                    _ => false,
                });
            }
            out.extend(found);
        }
        for field in &gql.fields {
            if field.nullable {
                continue;
            }
            let GraphqlResolution::Value { method } = &field.resolution else {
                continue;
            };
            let Some(def) = instance_method(class, method) else {
                continue;
            };
            let Some(Ty::Fn { ret, .. }) = &def.signature else {
                continue;
            };
            if !may_be_nil(ret) || proven_non_nil(app, &models, class, &def.body, 3) {
                continue;
            }
            let kind = DiagnosticKind::GraphqlNullableField {
                field: field.name.clone(),
                value_ty: (**ret).clone(),
            };
            out.push(Diagnostic {
                span: field.span,
                severity: Diagnostic::default_severity(&kind),
                message: format!(
                    "{} ({})",
                    Diagnostic::stub_text(&kind),
                    crate::ide::render_ty(ret)
                ),
                kind,
            });
        }
    }
    out
}

fn instance_method<'a>(class: &'a LibraryClass, name: &Symbol) -> Option<&'a MethodDef> {
    class
        .methods
        .iter()
        .find(|m| &m.name == name && matches!(m.receiver, crate::dialect::MethodReceiver::Instance))
}

fn may_be_nil(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(may_be_nil),
        _ => false,
    }
}

/// The value is a nil the database cannot hold: `object.<assoc>` where
/// every non-nil class `object` can be is a model whose `<assoc>` is a
/// required, non-polymorphic `belongs_to` on a NOT NULL column that a
/// foreign key constrains. A stored row's association then always
/// loads. Follows a body that only calls another method on the same
/// object (`self.posted_by`) a few steps, to its tail.
fn proven_non_nil(
    app: &App,
    models: &HashMap<&ClassId, &crate::dialect::Model>,
    class: &LibraryClass,
    body: &Expr,
    depth: u8,
) -> bool {
    let tail = tail_of(body);
    let ExprNode::Send {
        recv,
        method,
        args,
        block: None,
        ..
    } = &*tail.node
    else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    match recv.as_ref().map(|r| &*r.node) {
        None | Some(ExprNode::SelfRef) if depth > 0 => {
            return instance_method(class, method)
                .is_some_and(|m| proven_non_nil(app, models, class, &m.body, depth - 1));
        }
        None | Some(ExprNode::SelfRef) => return false,
        _ => {}
    }
    let Some(recv_ty) = recv.as_ref().and_then(|r| r.ty.as_ref()) else {
        return false;
    };
    let classes = non_nil_classes(recv_ty);
    !classes.is_empty()
        && classes
            .iter()
            .all(|id| required_belongs_to(app, models, id, method))
}

fn tail_of(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.last().map(tail_of).unwrap_or(expr),
        _ => expr,
    }
}

/// The class variants of `ty`, or empty if a non-nil variant is a
/// known non-class (a scalar). An unresolved variant makes no claim
/// either way and is passed over: a mutation's `resolve` that returns
/// the record or a `GraphQL::ExecutionError` (a gem class the analyzer
/// does not type) still wraps only the record, since graphql-ruby
/// turns the error into an error, not an object.
fn non_nil_classes(ty: &Ty) -> Vec<&crate::ident::ClassId> {
    let variants: Vec<&Ty> = match ty {
        Ty::Union { variants } => variants.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for v in variants {
        match v {
            Ty::Nil | Ty::Var { .. } | Ty::Untyped | Ty::Bottom => {}
            Ty::Class { id, .. } => out.push(id),
            _ => return Vec::new(),
        }
    }
    out
}

fn required_belongs_to(
    app: &App,
    models: &HashMap<&ClassId, &crate::dialect::Model>,
    model: &ClassId,
    assoc: &Symbol,
) -> bool {
    let Some(model) = models.get(model) else {
        return false;
    };
    let Some(Association::BelongsTo {
        foreign_key,
        optional: false,
        polymorphic: false,
        ..
    }) = model.associations().find(|a| a.name() == assoc)
    else {
        return false;
    };
    let Some(table) = app.schema.tables.get(&model.table.0) else {
        return false;
    };
    table
        .columns
        .iter()
        .any(|c| &c.name == foreign_key && !c.nullable)
        && table
            .foreign_keys
            .iter()
            .any(|fk| &fk.from_column == foreign_key)
}

fn class_index(app: &App) -> HashMap<&ClassId, &LibraryClass> {
    app.library_classes.iter().map(|c| (&c.name, c)).collect()
}

/// How much of a graphql-ruby schema `check` could follow: the
/// denominator for a clean result, and, on an app the analysis cannot
/// reach into yet, the list of what to model next.
#[derive(Debug, Default, PartialEq)]
pub struct GraphqlCoverage {
    pub types: usize,
    pub fields: usize,
    /// Fields whose value was typed on a type the roots reach.
    pub checked: usize,
    /// Fields with a value, on a type no modeled field constructs.
    pub unreached: usize,
    /// Fields resolving through a method that takes arguments.
    pub arguments: usize,
    /// Skipped fields by reason, most frequent first.
    pub skipped: Vec<(String, usize)>,
}

impl GraphqlCoverage {
    pub fn summary(&self) -> String {
        let mut line = format!(
            "graphql: {} object type(s), {} field(s): {} checked, {} on types nothing reaches, {} take arguments",
            self.types, self.fields, self.checked, self.unreached, self.arguments
        );
        let skipped: usize = self.skipped.iter().map(|(_, n)| n).sum();
        if skipped > 0 {
            const MAX_SHOWN: usize = 8;
            let shown: Vec<String> = self
                .skipped
                .iter()
                .take(MAX_SHOWN)
                .map(|(reason, n)| format!("{reason} {n}"))
                .collect();
            line.push_str(&format!(", {skipped} skipped ({}", shown.join(", ")));
            if self.skipped.len() > MAX_SHOWN {
                line.push_str(&format!(
                    ", … {} more reasons",
                    self.skipped.len() - MAX_SHOWN
                ));
            }
            line.push(')');
        }
        line
    }
}

/// `None` for an app without graphql-ruby object types.
pub fn coverage(app: &App) -> Option<GraphqlCoverage> {
    let types: Vec<_> = app.graphql_types.iter().filter(|t| !t.resolver).collect();
    if types.is_empty() {
        return None;
    }
    let classes = class_index(app);
    let mut out = GraphqlCoverage {
        types: types.len(),
        ..Default::default()
    };
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for gql in types {
        let reached = classes.get(&gql.class).is_some_and(|c| reached(gql, c));
        for field in &gql.fields {
            out.fields += 1;
            match &field.resolution {
                GraphqlResolution::Value { .. } if reached => out.checked += 1,
                GraphqlResolution::Value { .. } => out.unreached += 1,
                GraphqlResolution::Arguments { .. } => out.arguments += 1,
                GraphqlResolution::Skipped { reason } => {
                    *skipped.entry(reason.clone()).or_default() += 1
                }
            }
        }
    }
    out.skipped = skipped.into_iter().collect();
    out.skipped
        .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(out)
}

/// A root, or a type some modeled field constructs: its `object` has a
/// known non-nil type (a record, or a scalar such as a count).
fn reached(gql: &crate::dialect::GraphqlObjectType, class: &LibraryClass) -> bool {
    if gql.synthesized.iter().any(|m| m.as_str() == "__gql_root") {
        return true;
    }
    let Some(Ty::Fn { ret, .. }) =
        instance_method(class, &Symbol::from("object")).and_then(|m| m.signature.as_ref())
    else {
        return false;
    };
    let variants: Vec<&Ty> = match &**ret {
        Ty::Union { variants } => variants.iter().collect(),
        other => vec![other],
    };
    variants
        .iter()
        .any(|v| !matches!(v, Ty::Nil | Ty::Var { .. } | Ty::Untyped | Ty::Bottom))
}
