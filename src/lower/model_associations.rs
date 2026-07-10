//! Cross-model association graph analysis.
//!
//! Walks `app.models`, collects every association edge
//! (`has_many` / `belongs_to` / `has_one` / `has_and_belongs_to_many`),
//! and marks each edge as `Direct` or `Registry` based on cycle
//! membership. Cycle edges need an indirection (interface + registry
//! lookup) to avoid import cycles in strict-typed targets (Go, Rust,
//! future Kotlin/Swift); tree edges stay direct.
//!
//! Each emitter consumes the same graph and picks its own resolution
//! shape:
//!
//! | Target | Direct edge | Registry edge |
//! |---|---|---|
//! | Go | `*models.Comment` field/method | interface + `registry.Lookup(...)` |
//! | Rust | `Vec<Comment>` typed | trait object + registry |
//! | TS | direct `import` | direct `import` (ignores Registry hint) |
//! | Crystal | direct class ref | direct class ref (lazy) |
//! | Spinel/Ruby | direct constant | direct constant (Zeitwerk) |
//!
//! See GitHub issue #20 for the full design rationale.

use crate::dialect::Association;
use crate::ident::{ClassId, Symbol};
use crate::App;

/// How a target should resolve an association reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Edge does not participate in any cycle. Emitters can use a
    /// direct type reference (typed field, real `import`, etc.).
    Direct,
    /// Edge participates in a cycle. Strict-typed emitters resolve
    /// the target via the framework registry (interface + lookup);
    /// lazy emitters can still use direct refs.
    Registry,
}

/// Coarse-grained association kind. Mirrors `dialect::Association`
/// variants without their per-kind fields — those that matter for
/// graph analysis (`foreign_key`, `through`) are lifted to
/// `AssociationEdge`'s top level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssocKind {
    HasMany,
    HasManyThrough,
    BelongsTo,
    HasOne,
    HasAndBelongsToMany,
}

/// One association edge in the model graph. The lowerer emits one
/// of these per declared association across the app.
#[derive(Clone, Debug)]
pub struct AssociationEdge {
    /// Class the association is declared on (e.g. `Article` for
    /// `Article.has_many :comments`).
    pub from: ClassId,
    /// Target class the association points at (e.g. `Comment`).
    pub to: ClassId,
    /// Association name as declared (`:comments`).
    pub name: Symbol,
    /// Coarse kind of association.
    pub kind: AssocKind,
    /// Foreign-key column. For `has_many`/`has_one` this is on the
    /// target's table; for `belongs_to` it's on `from`'s table. For
    /// HABTM it's the conventional name (`<other>_id`) and the join
    /// table lives in `join_table`.
    pub foreign_key: Symbol,
    /// `has_many :through` join association name, when applicable.
    pub through: Option<Symbol>,
    /// HABTM join-table name, when applicable.
    pub join_table: Option<Symbol>,
    /// `Direct` for tree edges; `Registry` for edges that participate
    /// in a cycle.
    pub resolution: Resolution,
}

/// Compute the full association graph for `app`. Every declared
/// association in every model becomes one `AssociationEdge`;
/// resolution is computed by walking the graph and marking each
/// edge that participates in a strongly-connected component of
/// size > 1 (or a self-loop) as `Registry`.
///
/// Edges to models that aren't in `app.models` (polymorphic
/// associations, references to gem-provided types) are emitted with
/// `Direct` resolution — they can't participate in a cycle inside
/// the app, by definition.
pub fn compute_association_graph(app: &App) -> Vec<AssociationEdge> {
    let mut edges: Vec<AssociationEdge> = Vec::new();
    for model in &app.models {
        for assoc in model.associations() {
            edges.push(lift_edge(model.name.clone(), assoc));
        }
    }
    let known_models: std::collections::HashSet<&ClassId> =
        app.models.iter().map(|m| &m.name).collect();
    annotate_resolutions(&mut edges, &known_models);
    edges
}

fn lift_edge(from: ClassId, assoc: &Association) -> AssociationEdge {
    match assoc {
        Association::HasMany {
            name,
            target,
            foreign_key,
            through,
            ..
        } => AssociationEdge {
            from,
            to: target.clone(),
            name: name.clone(),
            kind: if through.is_some() {
                AssocKind::HasManyThrough
            } else {
                AssocKind::HasMany
            },
            foreign_key: foreign_key.clone(),
            through: through.clone(),
            join_table: None,
            resolution: Resolution::Direct,
        },
        Association::BelongsTo {
            name,
            target,
            foreign_key,
            ..
        } => AssociationEdge {
            from,
            to: target.clone(),
            name: name.clone(),
            kind: AssocKind::BelongsTo,
            foreign_key: foreign_key.clone(),
            through: None,
            join_table: None,
            resolution: Resolution::Direct,
        },
        Association::HasOne {
            name,
            target,
            foreign_key,
            ..
        } => AssociationEdge {
            from,
            to: target.clone(),
            name: name.clone(),
            kind: AssocKind::HasOne,
            foreign_key: foreign_key.clone(),
            through: None,
            join_table: None,
            resolution: Resolution::Direct,
        },
        Association::HasAndBelongsToMany {
            name,
            target,
            join_table,
        } => AssociationEdge {
            from,
            to: target.clone(),
            name: name.clone(),
            kind: AssocKind::HasAndBelongsToMany,
            // HABTM has no direct foreign_key on either side — it lives
            // on the join table. Use the conventional `<other>_id` so
            // downstream consumers have something usable.
            foreign_key: Symbol::from(format!("{}_id", target.0.as_str().to_lowercase())),
            through: None,
            join_table: Some(join_table.clone()),
            resolution: Resolution::Direct,
        },
    }
}

/// Mark each edge as `Registry` iff it participates in a cycle.
/// "Participates in a cycle" means: there exists a directed path from
/// `to` back to `from` along other edges (or `from == to`, a
/// self-loop).
///
/// Edges to unknown models (not in `app.models`) stay `Direct` — they
/// can't form an in-app cycle.
///
/// Algorithm: per-edge reachability BFS. O(E * (V+E)) overall; fine
/// for app-scale graphs (typical Rails app: <100 models). Simpler to
/// verify than full SCC and the input size makes it equivalent in
/// practice.
fn annotate_resolutions(
    edges: &mut [AssociationEdge],
    known_models: &std::collections::HashSet<&ClassId>,
) {
    let adjacency = build_adjacency(edges);
    for edge in edges.iter_mut() {
        if !known_models.contains(&edge.to) {
            continue;
        }
        if edge.from == edge.to || reachable(&edge.to, &edge.from, &adjacency) {
            edge.resolution = Resolution::Registry;
        }
    }
}

fn build_adjacency(
    edges: &[AssociationEdge],
) -> std::collections::HashMap<ClassId, Vec<ClassId>> {
    let mut adj: std::collections::HashMap<ClassId, Vec<ClassId>> =
        std::collections::HashMap::new();
    for edge in edges {
        adj.entry(edge.from.clone()).or_default().push(edge.to.clone());
    }
    adj
}

/// BFS: is `target` reachable from `start` along directed edges?
fn reachable(
    start: &ClassId,
    target: &ClassId,
    adjacency: &std::collections::HashMap<ClassId, Vec<ClassId>>,
) -> bool {
    let mut seen: std::collections::HashSet<&ClassId> = std::collections::HashSet::new();
    let mut frontier: Vec<&ClassId> = vec![start];
    while let Some(node) = frontier.pop() {
        if !seen.insert(node) {
            continue;
        }
        if let Some(neighbors) = adjacency.get(node) {
            for n in neighbors {
                if n == target {
                    return true;
                }
                frontier.push(n);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{Dependent, Model, ModelBodyItem};
    use crate::ident::TableRef;
    use crate::ty::Row;

    fn cid(name: &str) -> ClassId {
        ClassId(Symbol::from(name))
    }

    fn sym(name: &str) -> Symbol {
        Symbol::from(name)
    }

    fn model(name: &str, assocs: Vec<Association>) -> Model {
        let body = assocs
            .into_iter()
            .map(|assoc| ModelBodyItem::Association {
                assoc,
                leading_comments: Vec::new(),
                leading_blank_line: false,
                span: crate::span::Span::synthetic(),
            })
            .collect();
        Model {
            name: cid(name),
            parent: None,
            table: TableRef(sym(&name.to_lowercase())),
            attributes: Row::default(),
            body,
            span: crate::span::Span::synthetic(),
        }
    }

    fn has_many(name: &str, target: &str, fk: &str) -> Association {
        Association::HasMany {
            name: sym(name),
            target: cid(target),
            foreign_key: sym(fk),
            through: None,
            dependent: Dependent::None,
            scope: None,
        }
    }

    fn belongs_to(name: &str, target: &str, fk: &str) -> Association {
        Association::BelongsTo {
            name: sym(name),
            target: cid(target),
            foreign_key: sym(fk),
            optional: false,
        }
    }

    fn make_app(models: Vec<Model>) -> App {
        let mut app = App::default();
        app.models = models;
        app
    }

    fn find_edge<'a>(
        edges: &'a [AssociationEdge],
        from: &str,
        name: &str,
    ) -> &'a AssociationEdge {
        edges
            .iter()
            .find(|e| e.from.0.as_str() == from && e.name.as_str() == name)
            .unwrap_or_else(|| panic!("no edge {from}.{name}"))
    }

    #[test]
    fn no_associations() {
        let app = make_app(vec![model("Article", vec![]), model("Comment", vec![])]);
        let edges = compute_association_graph(&app);
        assert!(edges.is_empty());
    }

    #[test]
    fn tree_associations_only_are_direct() {
        // Article has_many :comments. Comment has no inverse — tree.
        let app = make_app(vec![
            model("Article", vec![has_many("comments", "Comment", "article_id")]),
            model("Comment", vec![]),
        ]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 1);
        let e = find_edge(&edges, "Article", "comments");
        assert_eq!(e.resolution, Resolution::Direct);
        assert_eq!(e.kind, AssocKind::HasMany);
        assert_eq!(e.to.0.as_str(), "Comment");
    }

    #[test]
    fn simple_article_comment_cycle_is_registry() {
        // Article has_many :comments; Comment belongs_to :article.
        // Both edges form the Article ↔ Comment cycle.
        let app = make_app(vec![
            model("Article", vec![has_many("comments", "Comment", "article_id")]),
            model(
                "Comment",
                vec![belongs_to("article", "Article", "article_id")],
            ),
        ]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 2);
        assert_eq!(
            find_edge(&edges, "Article", "comments").resolution,
            Resolution::Registry
        );
        assert_eq!(
            find_edge(&edges, "Comment", "article").resolution,
            Resolution::Registry
        );
    }

    #[test]
    fn three_model_cycle_marks_all_edges_registry() {
        // A -> B -> C -> A. Every edge participates in the cycle.
        let app = make_app(vec![
            model("A", vec![has_many("bs", "B", "a_id")]),
            model("B", vec![has_many("cs", "C", "b_id")]),
            model("C", vec![belongs_to("a", "A", "c_id")]),
        ]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 3);
        for e in &edges {
            assert_eq!(
                e.resolution,
                Resolution::Registry,
                "edge {}.{} should be Registry",
                e.from.0.as_str(),
                e.name.as_str()
            );
        }
    }

    #[test]
    fn cycle_and_tree_edges_split_correctly() {
        // A ↔ B cycle (registry edges), plus A -> C tree edge (direct).
        let app = make_app(vec![
            model(
                "A",
                vec![
                    has_many("bs", "B", "a_id"),
                    has_many("cs", "C", "a_id"),
                ],
            ),
            model("B", vec![belongs_to("a", "A", "a_id")]),
            model("C", vec![]),
        ]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 3);
        assert_eq!(
            find_edge(&edges, "A", "bs").resolution,
            Resolution::Registry
        );
        assert_eq!(
            find_edge(&edges, "B", "a").resolution,
            Resolution::Registry
        );
        assert_eq!(
            find_edge(&edges, "A", "cs").resolution,
            Resolution::Direct
        );
    }

    #[test]
    fn unknown_target_stays_direct_polymorphic_like() {
        // Comment belongs_to :commentable — target class not defined
        // in the app (Rails polymorphic associations resolve at
        // runtime through commentable_type). Treat as Direct: the
        // edge can't participate in an in-app cycle by definition.
        let app = make_app(vec![model(
            "Comment",
            vec![belongs_to("commentable", "Polymorphic", "commentable_id")],
        )]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].resolution, Resolution::Direct);
        assert_eq!(edges[0].to.0.as_str(), "Polymorphic");
    }

    #[test]
    fn self_referential_association_is_registry() {
        // Employee belongs_to :manager — points back at Employee.
        // Self-loop is a cycle by definition.
        let app = make_app(vec![model(
            "Employee",
            vec![belongs_to("manager", "Employee", "manager_id")],
        )]);
        let edges = compute_association_graph(&app);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].resolution, Resolution::Registry);
    }
}
