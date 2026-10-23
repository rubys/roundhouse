//! Grouped-count grounding: `rel.group(:col).count` renames its
//! terminal to `group_count` — Rails' grouped count returns a Hash of
//! group-key => COUNT, a different type than the scalar `count`.
//! Splitting the name keeps both returns monomorphic (a count that
//! answers Integer-or-Hash is exactly the polymorphic-API shape the
//! runtime avoids); the runtime's `group_count` builds the
//! SELECT … GROUP BY and hydrates the Hash.
//!
//! Shape-directed: a zero-arg block-less `count` whose receiver chain
//! contains `group(...)` (directly, or through query refiners such as
//! `having` / `distinct` / `select`). Rails still answers a Hash for
//! those; only the immediate `group.count` pair is simple enough for
//! the Arel fold. Runs on the post-analyze hook with its siblings so
//! every target consumes the grounded form.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use std::collections::HashSet;

/// Query names alone do not prove Ruby dispatch: an ordinary object's
/// `group` may return an Array, and a model may override any query name.
/// Share the same model/scope/source-method facts as scope-chain lowering.
pub(crate) struct Receivers {
    models: HashSet<ClassId>,
    scopes: super::scope_chain::ScopeRegistry,
    user_returns: super::scope_chain::UserMethodReturns,
    instance_groups: HashSet<ClassId>,
}

impl Receivers {
    pub(crate) fn new(app: &App) -> Self {
        let source = crate::analyze::SourceMethods::new(app);
        Self {
            models: super::scope_chain::model_set(&app.models),
            scopes: super::scope_chain::build_scope_registry(&app.models),
            user_returns: super::scope_chain::build_user_method_returns(app),
            instance_groups: app.models.iter().filter(|model| source.lookup(
                &model.name, "group", crate::dialect::MethodReceiver::Instance,
            ).is_some()).map(|model| model.name.clone()).collect(),
        }
    }

    /// Whether this send reaches an application singleton method before
    /// framework query dispatch. Relation receivers keep their builtins.
    pub(crate) fn source_class_call(&self, expr: &Expr, owner: Option<&ClassId>) -> bool {
        let ExprNode::Send { recv, method, .. } = &*expr.node else { return false };
        let model = match recv {
            None => owner.cloned(),
            Some(recv) if matches!(&*recv.node, ExprNode::SelfRef) => owner.cloned(),
            Some(recv) => self.model_const(recv).or_else(|| match &recv.ty {
                Some(Ty::Class { id, .. }) if self.models.contains(id) => Some(id.clone()),
                _ => None,
            }),
        };
        model.is_some_and(|model| self.user_returns.defines_class_method(&model, method))
    }

    fn model_const(&self, expr: &Expr) -> Option<ClassId> {
        let ExprNode::Const { path } = &*expr.node else { return None };
        let id = ClassId(Symbol::from(path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::")));
        self.models.contains(&id).then_some(id)
    }

    fn class_query(&self, model: &ClassId, method: &Symbol) -> bool {
        if self.user_returns.defines_class_method(model, method) {
            return false;
        }
        if self.scopes.get(model).is_some_and(|scopes| scopes.contains_key(method)) {
            return !self.user_returns.returns_non_relation(model, method);
        }
        super::scope_chain::is_relation_chain_method(method.as_str()) || method.as_str() == "all"
    }

    fn query_call(&self, recv: Option<&Expr>, method: &Symbol, owner: Option<&ClassId>) -> bool {
        match recv {
            None => owner.is_some_and(|model| {
                self.models.contains(model) && !self.has_instance_group(model)
                    && self.class_query(model, method)
            }),
            Some(recv) if matches!(&*recv.node, ExprNode::SelfRef) => self.query_call(None, method, owner),
            Some(recv) => match self.model_const(recv) {
                Some(model) => self.class_query(&model, method),
                None => self.relation(recv, owner),
            },
        }
    }

    fn has_instance_group(&self, model: &ClassId) -> bool {
        let mut current = Some(model);
        for _ in 0..32 {
            let Some(model) = current else { return false };
            if self.instance_groups.contains(model) { return true; }
            current = self.user_returns.parents.get(model);
        }
        false
    }

    fn relation(&self, expr: &Expr, owner: Option<&ClassId>) -> bool {
        if let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node {
            if method.as_str() == "new" && args.len() == 1
                && recv.as_ref().is_some_and(|recv| matches!(&*recv.node,
                    ExprNode::Const { path } if path.iter().map(Symbol::as_str).eq(["ActiveRecord", "Relation"])))
            {
                return self.model_const(&args[0]).is_some();
            }
            // A source override may itself return a query. That proves a
            // *following* group is Relation#group, without claiming that
            // the override named `group` grouped anything itself.
            if recv.as_ref().and_then(|recv| self.model_const(recv))
                .is_some_and(|model| self.user_returns.class_return_model(&model, method).is_some())
            {
                return true;
            }
            let is_scope = recv.as_ref().and_then(|recv| self.model_const(recv))
                .is_some_and(|model| self.scopes.get(&model).is_some_and(|scopes| scopes.contains_key(method)));
            if super::scope_chain::is_relation_chain_method(method.as_str()) || method.as_str() == "all" || is_scope {
                if self.query_call(recv.as_ref(), method, owner) { return true; }
            }
        }
        // In particular, Array[Model] is not proof: actual record arrays
        // and queries can have that same analyzed representation.
        matches!(expr.ty, Some(Ty::Relation { .. }))
    }

    fn grouped_query(&self, expr: &Expr, owner: Option<&ClassId>) -> bool {
        let ExprNode::Send { recv, method, block: None, .. } = &*expr.node else { return false };
        if method.as_str() == "group" {
            return self.query_call(recv.as_ref(), method, owner);
        }
        COUNT_CHAIN_REFINERS.contains(&method.as_str())
            && recv.as_ref().is_some_and(|recv| self.grouped_query(recv, owner))
    }
}

pub fn apply_group_count_lowering(app: &mut App) {
    let receivers = Receivers::new(app);
    super::for_each_owned_hook_body(app, &mut |owner, body| rewrite(body, &receivers, owner));
}

/// The two parts of a grouped-count chain, for the callers that need
/// to look inside one.
pub struct GroupedCount<'a> {
    /// What the `group(...)` is called ON — the chain the group refines.
    /// `None` for an implicit-self body (`scope :by_x, -> {
    /// group(:x).count }`), which has no receiver to root a query at.
    pub base: Option<&'a Expr>,
    /// The `group(...)` arguments, as written.
    pub group_args: &'a [Expr],
}

/// Recognize a grouped count on `expr`, in EITHER spelling: the source
/// `rel.group(:col).count` and the `rel.group(:col).group_count` this
/// pass renames it to.
///
/// One predicate, two readers. The rename below asks it, and so does
/// `lower::arel::build` — which sees the chain at two different points
/// in the pipeline depending on the caller: the transpile driver runs
/// the post-analyze lowerings first (so the terminal arrives renamed),
/// while the emit-only harnesses call `Analyzer` + `emit` directly (so
/// it arrives as `count`). A recognizer keyed on one spelling folds the
/// query under one of those and emits a call to a `group` method no
/// target defines under the other. The QUERY is the same either way —
/// the rename exists to keep the runtime Relation's return
/// monomorphic, which is a fact about the runtime path, not about the
/// SQL.
pub fn grouped_count_parts(expr: &Expr) -> Option<GroupedCount<'_>> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if !matches!(method.as_str(), "count" | "group_count") || !args.is_empty() {
        return None;
    }
    group_in_relation_chain(recv)
}

/// Query methods that refine a grouped relation without changing that
/// it is grouped. `analyze::body::send::grouped_count_ty` walks this
/// same list via [`group_in_relation_chain`]. `group` itself is the
/// stop, not a skip.
pub const COUNT_CHAIN_REFINERS: &[&str] = &[
    "having",
    "distinct",
    "select",
    "where",
    "not",
    "joins",
    "left_outer_joins",
    "left_joins",
    "order",
    "reorder",
    "rewhere",
    "where!",
    "order!",
    "limit",
    "offset",
    "from",
    "includes",
    "preload",
    "eager_load",
    "merge",
    "references",
];

/// Arel may fold only `rel.group(:col).count` — intervening `having` /
/// `distinct` / `select` belong on Relation SQL so GROUP BY, HAVING,
/// and the DISTINCT projection stay in one place (#343).
pub fn group_immediately_precedes_count(expr: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return false;
    };
    if !matches!(method.as_str(), "count" | "group_count") || !args.is_empty() {
        return false;
    }
    matches!(
        &*recv.node,
        ExprNode::Send { method: gm, block: None, .. } if gm.as_str() == "group"
    )
}

pub fn group_in_relation_chain(expr: &Expr) -> Option<GroupedCount<'_>> {
    let mut cur = expr;
    loop {
        let ExprNode::Send { recv, method, args, block: None, .. } = &*cur.node else {
            return None;
        };
        if method.as_str() == "group" {
            return Some(GroupedCount { base: recv.as_ref(), group_args: args });
        }
        if !COUNT_CHAIN_REFINERS.contains(&method.as_str()) {
            return None;
        }
        cur = recv.as_ref()?;
    }
}

pub(crate) fn rewrite(expr: &mut Expr, receivers: &Receivers, owner: Option<&ClassId>) {
    expr.node.for_each_child_mut(&mut |child| rewrite(child, receivers, owner));
    rewrite_node(expr, receivers, owner);
}

pub(crate) fn rewrite_node(expr: &mut Expr, receivers: &Receivers, owner: Option<&ClassId>) {
    if matches!(&*expr.node, ExprNode::Send { recv: Some(recv), .. }
        if receivers.grouped_query(recv, owner))
    {
        rewrite_grouped_query(expr);
    }
}

/// The caller has proved the receiver is a query; retain the common
/// argument/block/chain checks and type update at both lowering seams.
pub(crate) fn rewrite_grouped_query(expr: &mut Expr) {
    // The SOURCE spelling only — `grouped_count_parts` accepts both, so
    // a second pass over an already-renamed tree is a no-op rather than
    // a rename of a rename.
    let is_source_count = matches!(
        &*expr.node,
        ExprNode::Send { method, .. } if method.as_str() == "count"
    );
    if is_source_count && grouped_count_parts(expr).is_some() {
        let ExprNode::Send { method, .. } = &mut *expr.node else { unreachable!() };
        *method = Symbol::from("group_count");
        // The rename is the only thing that knows this call is no longer
        // the `count` the source wrote, so it owns the renamed call's
        // type. Clearing it (what this line used to do) left a Send with
        // a known receiver and no type, which `analyze::diagnostics`
        // reads — it does not re-dispatch — as `send_dispatch_failed`:
        // "no known method `group_count` on Relation[Comment]", against
        // a name the app never wrote, a runtime method that exists
        // (`ActiveRecord::Relation#group_count`) and an RBS signature
        // that declares it. Every target failed the type gate on a
        // feature the pipeline ships (issue #77).
        //
        // A Hash already on the expression is the body-typer's answer
        // (`analyze::body::send::grouped_count_ty`, whose shape test
        // mirrors the match above), and it is the more precise one — it
        // carries the grouped COLUMN's key type. Keep it; fall back to
        // `relation.rbs`'s declared `Hash[untyped, Integer]` when the
        // typer had nothing to say (an untyped receiver, a shape its
        // schema read declined).
        if !matches!(expr.ty, Some(Ty::Hash { .. })) {
            expr.ty = Some(Ty::Hash {
                key: std::sync::Arc::new(Ty::Untyped),
                value: std::sync::Arc::new(Ty::Int),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Literal;
    use crate::ident::VarId;
    use crate::span::Span;

    fn model() -> ClassId { ClassId(Symbol::from("Widget")) }

    fn receivers() -> Receivers {
        Receivers {
            models: HashSet::from([model()]),
            scopes: Default::default(),
            user_returns: super::super::scope_chain::UserMethodReturns::new(),
            instance_groups: HashSet::new(),
        }
    }

    fn constant(name: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Const {
            path: name.split("::").map(Symbol::from).collect(),
        })
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Send {
            recv, method: Symbol::from(method), args, block: None, parenthesized: true,
        })
    }

    fn variable(ty: Option<Ty>) -> Expr {
        let mut expr = Expr::new(Span::synthetic(), ExprNode::Var {
            id: VarId(0), name: Symbol::from("query"),
        });
        expr.ty = ty;
        expr
    }

    fn grouped(recv: Option<Expr>) -> Expr {
        send(Some(send(recv, "group", vec![constant("COLUMN")])), "count", vec![])
    }

    fn method(expr: &Expr) -> &str {
        let ExprNode::Send { method, .. } = &*expr.node else { panic!("expected send") };
        method.as_str()
    }

    #[test]
    fn ordinary_receivers_keep_count_through_every_group_refiner() {
        let receivers = receivers();
        let ordinary = [
            send(Some(constant("Grouper")), "new", vec![]),
            variable(None), variable(Some(Ty::Untyped)), variable(Some(Ty::Int)),
            variable(Some(Ty::Array { elem: Ty::Int.into() })),
            variable(Some(Ty::Array { elem: Ty::Class { id: model(), args: vec![].into() }.into() })),
        ];
        for recv in ordinary {
            for refiner in std::iter::once("").chain(COUNT_CHAIN_REFINERS.iter().copied()) {
                let mut group = send(Some(recv.clone()), "group", vec![constant("COLUMN")]);
                if !refiner.is_empty() { group = send(Some(group), refiner, vec![]); }
                let mut count = send(Some(group), "count", vec![]);
                // Neither a stale result type nor the Array-of-model
                // representation is evidence about the group receiver.
                count.ty = Some(Ty::Hash { key: Ty::Int.into(), value: Ty::Int.into() });
                rewrite(&mut count, &receivers, None);
                assert_eq!(method(&count), "count", "ordinary receiver through {refiner}");
            }
        }
        for recv in [None, Some(Expr::new(Span::synthetic(), ExprNode::SelfRef))] {
            let mut count = grouped(recv);
            rewrite(&mut count, &receivers, Some(&ClassId(Symbol::from("Grouper"))));
            assert_eq!(method(&count), "count");
        }
    }

    #[test]
    fn query_receivers_lower_through_every_group_refiner() {
        let mut receivers = receivers();
        receivers.scopes.insert(model(), [(Symbol::from("active"), vec![])].into());
        let relation = Some(Ty::Relation { of: model() });
        let query_receivers = [
            constant("Widget"),
            variable(relation),
            send(Some(constant("Widget")), "where", vec![]),
            send(Some(constant("Widget")), "active", vec![]),
            send(Some(constant("ActiveRecord::Relation")), "new", vec![constant("Widget")]),
        ];
        for recv in query_receivers {
            for refiner in std::iter::once("").chain(COUNT_CHAIN_REFINERS.iter().copied()) {
                let mut group = send(Some(recv.clone()), "group", vec![constant("COLUMN")]);
                if !refiner.is_empty() { group = send(Some(group), refiner, vec![]); }
                let mut count = send(Some(group), "count", vec![]);
                rewrite(&mut count, &receivers, None);
                assert_eq!(method(&count), "group_count", "query receiver through {refiner}");
                let once = count.clone();
                rewrite(&mut count, &receivers, None);
                assert_eq!(count, once, "idempotent");
            }
        }
        for recv in [None, Some(Expr::new(Span::synthetic(), ExprNode::SelfRef))] {
            let mut count = grouped(recv);
            rewrite(&mut count, &receivers, Some(&model()));
            assert_eq!(method(&count), "group_count");
        }
    }

    #[test]
    fn source_overrides_do_not_prove_a_grouped_query() {
        for inherited in [false, true] {
            for name in ["group", "order", "where", "all", "left_joins", "order!"] {
                let mut receivers = receivers();
                let defining = if inherited { ClassId(Symbol::from("Parent")) } else { model() };
                receivers.user_returns.class_methods.insert((defining.clone(), Symbol::from(name)));
                if inherited { receivers.user_returns.parents.insert(model(), defining); }
                let query = if name == "group" { Some(constant("Widget")) }
                    else { Some(send(Some(constant("Widget")), name, vec![])) };
                let mut count = grouped(query);
                rewrite(&mut count, &receivers, None);
                assert_eq!(method(&count), "count", "{name}, inherited={inherited}");
            }
        }
        let mut receivers = receivers();
        let parent = ClassId(Symbol::from("Parent"));
        receivers.user_returns.parents.insert(model(), parent.clone());
        receivers.instance_groups.insert(parent);
        for recv in [None, Some(Expr::new(Span::synthetic(), ExprNode::SelfRef))] {
            let mut count = grouped(recv);
            rewrite(&mut count, &receivers, Some(&model()));
            assert_eq!(method(&count), "count", "inherited instance group");
        }
    }

    #[test]
    fn group_count_keeps_operand_and_block_boundaries() {
        let receivers = receivers();
        for args in [vec![constant("VALUE")], vec![constant("A"), constant("B")]] {
            let mut count = grouped(Some(constant("Widget")));
            let ExprNode::Send { args: target, .. } = &mut *count.node else { unreachable!() };
            *target = args;
            rewrite(&mut count, &receivers, None);
            assert_eq!(method(&count), "count");
        }
        for group_block in [false, true] {
            let mut count = grouped(Some(constant("Widget")));
            let ExprNode::Send { recv, block, .. } = &mut *count.node else { unreachable!() };
            if group_block {
                let ExprNode::Send { block, .. } = &mut *recv.as_mut().unwrap().node else { unreachable!() };
                *block = Some(constant("BLOCK"));
            } else { *block = Some(constant("BLOCK")); }
            rewrite(&mut count, &receivers, None);
            assert_eq!(method(&count), "count");
        }
        for refiner in COUNT_CHAIN_REFINERS {
            let mut middle = send(Some(send(Some(constant("Widget")), "group", vec![constant("A")])), refiner, vec![]);
            let ExprNode::Send { block, .. } = &mut *middle.node else { unreachable!() };
            *block = Some(constant("BLOCK"));
            let mut count = send(Some(middle), "count", vec![]);
            rewrite(&mut count, &receivers, None);
            assert_eq!(method(&count), "count", "block on {refiner}");
        }
        for args in [vec![], vec![constant("A")], vec![constant("A"), constant("B")]] {
            let mut count = send(Some(send(Some(constant("Widget")), "group", args)), "count", vec![]);
            rewrite(&mut count, &receivers, None);
            assert_eq!(method(&count), "group_count");
        }
        let mut scalar = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: 1 } });
        rewrite(&mut scalar, &receivers, None);
        assert!(matches!(*scalar.node, ExprNode::Lit { .. }));
    }
}
