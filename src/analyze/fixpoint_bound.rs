//! A size and depth bound on the types the whole-program fixpoint carries
//! from one round to the next.
//!
//! Harvested returns, unified parameter types and harvested ivar types
//! are the next round's input, so a method whose result reaches its own
//! input rebuilds its type from the previous one every round. `harvest_return` cuts the direct
//! case, a return that nests its own previous copy. Two shapes escape any
//! comparison with earlier types:
//!
//! - a cycle of methods (`walk_0` maps through `walk_1`, which maps back
//!   through `walk_0`): a return nests the other method's previous return,
//!   which nests this one's from two rounds back;
//! - a result that comes back rewritten (`canonical(inner.merge(…))`):
//!   `merge` widens the Hash spine and call-site unification joins it with
//!   the literals, so the earlier type never reappears intact.
//!
//! With two recursive positions (an Array element and a Hash value) either
//! one doubles every round, through the production rounds and on into the
//! absorb rounds, until memory runs out.
//!
//! So every carried type is bounded instead. A type over [`MAX_NODES`] is
//! cut to the deepest container nesting that fits, and no type nests more
//! than [`MAX_DEPTH`] containers; a cut position reads `untyped`. A
//! recursive value has no finite type in this lattice, so the cut gives
//! up only precision that was never reachable, and it surfaces as gradual
//! `untyped` instead of a deeper copy each round. Both bounds sit above
//! every type the public corpora carry (depth 10 and about 320 nodes at
//! most), so a program without such a cycle is unchanged.
//!
//! Gradual is a warning, not an error, and every emitter accepts it. These
//! walks are an open gap on Rust whether or not the bound cuts them: a
//! value that may be a Hash, an Array or a scalar renders as
//! `serde_json::Value`, which has none of the Hash and Array methods
//! called on it (`transform_values`, `sort`, `merge`, iteration), and a
//! `case … when Hash` renders every arm as `_`. The crate fails
//! `cargo check` while `check` reports no error. The emitted Ruby runs
//! (`tests/recursive_type_bound.rs`).

use crate::ty::{Param, Row, Ty};

use super::body;

/// Nested containers a carried type may hold.
const MAX_DEPTH: usize = 16;

/// Type nodes a carried type may hold.
const MAX_NODES: usize = 512;

/// `ty` within [`MAX_DEPTH`] and [`MAX_NODES`]; unchanged when it already is.
pub(super) fn bound(ty: Ty) -> Ty {
    let mut budget = MAX_NODES;
    if fits(&ty, MAX_DEPTH, &mut budget) {
        super::fixpoint_check::note_bound(false);
        return ty;
    }
    super::fixpoint_check::note_bound(true);
    let mut limit = measure(&ty).0.min(MAX_DEPTH);
    loop {
        let cut = cut(&ty, limit);
        if measure(&cut).1 <= MAX_NODES {
            return cut;
        }
        // A union with more leaf arms than the node bound has no nesting
        // left to cut.
        if limit == 0 {
            return Ty::Untyped;
        }
        limit -= 1;
    }
}

/// A type that adds a level of nesting: one with children, other than a
/// union, whose arms sit at its own level.
fn is_container(ty: &Ty) -> bool {
    match ty {
        Ty::Array { .. } | Ty::Hash { .. } | Ty::Fn { .. } => true,
        Ty::Tuple { elems } => !elems.is_empty(),
        Ty::Record { row } => !row.fields.is_empty(),
        Ty::Class { args, .. } => !args.is_empty(),
        _ => false,
    }
}

fn each_child<'a>(ty: &'a Ty, f: &mut dyn FnMut(&'a Ty)) {
    match ty {
        Ty::Array { elem } => f(elem),
        Ty::Hash { key, value } => {
            f(key);
            f(value);
        }
        Ty::Tuple { elems } => elems.iter().for_each(f),
        Ty::Record { row } => row.fields.values().for_each(f),
        Ty::Union { variants } => variants.iter().for_each(f),
        Ty::Class { args, .. } => args.iter().for_each(f),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().for_each(|p| f(&p.ty));
            block.as_deref().into_iter().for_each(&mut *f);
            f(ret);
        }
        _ => {}
    }
}

/// Whether `ty` nests at most `levels` containers and holds at most
/// `nodes` nodes, stopping at the first that does not.
fn fits(ty: &Ty, levels: usize, nodes: &mut usize) -> bool {
    if *nodes == 0 || (levels == 0 && is_container(ty)) {
        return false;
    }
    *nodes -= 1;
    let child_levels = levels - usize::from(is_container(ty));
    let mut ok = true;
    each_child(ty, &mut |child| ok = ok && fits(child, child_levels, nodes));
    ok
}

/// Container depth and node count.
fn measure(ty: &Ty) -> (usize, usize) {
    let (mut depth, mut nodes) = (0, 1);
    each_child(ty, &mut |child| {
        let (d, n) = measure(child);
        depth = depth.max(d);
        nodes += n;
    });
    (depth + usize::from(is_container(ty)), nodes)
}

/// `ty` with at most `levels` nested containers: one that would sit
/// deeper is `untyped`. Unions are re-joined, since cut arms can coincide.
fn cut(ty: &Ty, levels: usize) -> Ty {
    if let Ty::Union { variants } = ty {
        return body::union_many(variants.iter().map(|v| cut(v, levels)).collect());
    }
    if !is_container(ty) {
        return ty.clone();
    }
    if levels == 0 {
        return Ty::Untyped;
    }
    let inner = |t: &Ty| cut(t, levels - 1);
    match ty {
        Ty::Array { elem } => Ty::Array { elem: Box::new(inner(elem)) },
        Ty::Hash { key, value } => Ty::Hash { key: Box::new(inner(key)), value: Box::new(inner(value)) },
        Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(inner).collect() },
        Ty::Record { row } => Ty::Record {
            row: Row {
                fields: row.fields.iter().map(|(name, field)| (name.clone(), inner(field))).collect(),
                rest: row.rest,
            },
        },
        Ty::Class { id, args } => Ty::Class { id: id.clone(), args: args.iter().map(inner).collect() },
        Ty::Fn { params, block, ret, effects } => Ty::Fn {
            params: params
                .iter()
                .map(|p| Param { name: p.name.clone(), ty: inner(&p.ty), kind: p.kind.clone() })
                .collect(),
            block: block.as_ref().map(|b| Box::new(inner(b))),
            ret: Box::new(inner(ret)),
            effects: effects.clone(),
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{ClassId, Symbol};

    fn arr(elem: Ty) -> Ty {
        Ty::Array { elem: Box::new(elem) }
    }

    fn str_hash(value: Ty) -> Ty {
        Ty::Hash { key: Box::new(Ty::Str), value: Box::new(value) }
    }

    fn nested_arrays(depth: usize) -> Ty {
        (0..depth).fold(Ty::Int, |t, _| arr(t))
    }

    /// `J = Int | Str | nil | Hash[String, J] | Array[J]` unrolled `depth`
    /// times: two recursive positions per level, so it doubles per level.
    fn json_like(depth: usize) -> Ty {
        (0..depth).fold(body::union_many(vec![Ty::Int, Ty::Str, Ty::Nil]), |j, _| {
            body::union_many(vec![Ty::Int, Ty::Str, Ty::Nil, str_hash(j.clone()), arr(j)])
        })
    }

    #[test]
    fn the_early_exit_check_agrees_with_the_full_measure() {
        let samples = [Ty::Int, nested_arrays(5), json_like(2), json_like(6), str_hash(Ty::Tuple { elems: vec![] })];
        for ty in &samples {
            let (depth, nodes) = measure(ty);
            for levels in 0..8 {
                for limit in [1, 8, 40, 200, 1000] {
                    let mut budget = limit;
                    assert_eq!(fits(ty, levels, &mut budget), depth <= levels && nodes <= limit, "{ty:?}");
                }
            }
        }
    }

    #[test]
    fn a_type_within_both_bounds_is_unchanged() {
        let deep_literal = nested_arrays(10);
        assert_eq!(bound(deep_literal.clone()), deep_literal);
        let small_union = json_like(3);
        assert!(measure(&small_union).1 <= MAX_NODES);
        assert_eq!(bound(small_union.clone()), small_union);
    }

    #[test]
    fn nesting_past_the_depth_bound_is_cut_to_untyped() {
        let cut = bound(nested_arrays(MAX_DEPTH + 4));
        assert_eq!(measure(&cut).0, MAX_DEPTH);
        let mut innermost = &cut;
        while let Ty::Array { elem } = innermost {
            innermost = elem;
        }
        assert_eq!(innermost, &Ty::Untyped);
    }

    #[test]
    fn a_type_over_the_node_bound_is_cut_shallower_until_it_fits() {
        let big = json_like(12);
        let (depth, nodes) = measure(&big);
        assert!(depth <= MAX_DEPTH && nodes > MAX_NODES);
        let cut = bound(big);
        let (cut_depth, cut_nodes) = measure(&cut);
        assert!(cut_nodes <= MAX_NODES, "{cut_nodes} nodes");
        assert!(cut_depth < depth);
        assert!(cut.mentions_unknown());
    }

    #[test]
    fn a_leaf_union_wider_than_the_node_bound_is_untyped() {
        let classes = (0..MAX_NODES).map(|i| Ty::Class { id: ClassId(Symbol::from(format!("C{i}").as_str())), args: vec![] });
        let wide = body::union_many(classes.chain([arr(Ty::Int)]).collect());
        assert!(measure(&wide).1 > MAX_NODES);
        assert_eq!(bound(wide), Ty::Untyped);
    }

    /// An ivar's type is carried too: the next round seeds it into every
    /// method that reads it, and a write rebuilt from it (`@h = @h.…`)
    /// grows it the same way.
    #[test]
    fn an_ivar_harvested_from_a_write_is_bounded() {
        let mut value = crate::expr::Expr::new(
            crate::span::Span::synthetic(),
            crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil },
        );
        value.ty = Some(nested_arrays(MAX_DEPTH + 4));
        let write = crate::expr::Expr::new(
            crate::span::Span::synthetic(),
            crate::expr::ExprNode::Assign { target: crate::expr::LValue::Ivar { name: Symbol::from("h") }, value },
        );
        let mut ivars = std::collections::HashMap::new();
        super::super::extract_ivar_assignments(&write, &mut ivars);
        assert_eq!(measure(&ivars[&Symbol::from("h")]).0, MAX_DEPTH);

        // `@h["k"] = v` widens the Hash's value type from the written value.
        let mut deep = crate::expr::Expr::new(
            crate::span::Span::synthetic(),
            crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil },
        );
        deep.ty = Some(nested_arrays(MAX_DEPTH + 4));
        let ivar = crate::expr::Expr::new(crate::span::Span::synthetic(), crate::expr::ExprNode::Ivar { name: Symbol::from("g") });
        let key = crate::expr::Expr::new(crate::span::Span::synthetic(), crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil });
        let index_write = crate::expr::Expr::new(
            crate::span::Span::synthetic(),
            crate::expr::ExprNode::Assign { target: crate::expr::LValue::Index { recv: ivar, index: key }, value: deep },
        );
        super::super::extract_ivar_assignments(&index_write, &mut ivars);
        assert_eq!(measure(&ivars[&Symbol::from("g")]).0, MAX_DEPTH);
    }

    /// `bound` doesn't distribute over the join, so applying it after
    /// every write made an ivar slot depend on the order of its
    /// writes: with a 10-deep Array and two Hashes of 300 classes each,
    /// one order cut the slot to `untyped` and another kept the deep
    /// Array (#617). The bound applies once per slot, after the walk.
    fn wide_hash(prefix: &str) -> Ty {
        let classes = (0..300).map(|i| Ty::Class { id: ClassId(Symbol::from(format!("{prefix}{i}").as_str())), args: vec![] });
        str_hash(body::union_many(classes.collect()))
    }

    fn bound_order_writers() -> [Ty; 3] {
        [nested_arrays(10), wide_hash("A"), wide_hash("B")]
    }

    #[test]
    fn an_ivar_slot_is_bounded_once_after_its_writes() {
        let write = |ty: Ty| {
            let mut value = crate::expr::Expr::new(
                crate::span::Span::synthetic(),
                crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil },
            );
            value.ty = Some(ty);
            crate::expr::Expr::new(
                crate::span::Span::synthetic(),
                crate::expr::ExprNode::Assign { target: crate::expr::LValue::Ivar { name: Symbol::from("h") }, value },
            )
        };
        let harvest = |order: [usize; 3]| {
            let writers = bound_order_writers();
            let body = crate::expr::Expr::new(
                crate::span::Span::synthetic(),
                crate::expr::ExprNode::Seq { exprs: order.iter().map(|&i| write(writers[i].clone())).collect() },
            );
            let mut ivars = std::collections::HashMap::new();
            super::super::extract_ivar_assignments(&body, &mut ivars);
            ivars.remove(&Symbol::from("h")).unwrap()
        };
        let first = harvest([0, 1, 2]);
        for order in [[1, 2, 0], [2, 0, 1], [0, 2, 1]] {
            assert_eq!(harvest(order), first, "writes in order {order:?}");
        }
    }

    /// The same for a parameter row: its call sites' observations are
    /// joined first and the row is bounded once.
    #[test]
    fn a_param_slot_is_bounded_once_after_its_observations() {
        let key = (ClassId(Symbol::from("C")), Symbol::from("m"), crate::dialect::MethodReceiver::Instance);
        let fold = |order: [usize; 3]| {
            let writers = bound_order_writers();
            let mut rows = std::collections::HashMap::new();
            let observations = order.iter().map(|&i| (key.clone(), vec![writers[i].clone()])).collect();
            super::super::fold_param_observations(&mut rows, observations);
            rows.remove(&key).unwrap()
        };
        let first = fold([0, 1, 2]);
        for order in [[1, 2, 0], [2, 0, 1], [0, 2, 1]] {
            assert_eq!(fold(order), first, "observations in order {order:?}");
        }
    }

    #[test]
    fn bounding_is_idempotent_so_a_cut_type_is_a_fixed_point() {
        for ty in [json_like(12), nested_arrays(MAX_DEPTH + 4)] {
            let once = bound(ty);
            assert_eq!(bound(once.clone()), once);
        }
    }

    /// What the fixpoint does with a recursive method: each round wraps the
    /// stored type in one more level and stores the bounded result. The
    /// sequence must reach a type that the next round reproduces.
    #[test]
    fn rounds_that_unroll_a_recursive_type_reach_a_fixed_point() {
        let leaves = || vec![Ty::Int, Ty::Str, Ty::Nil];
        let mut stored = bound(body::union_many(leaves()));
        for _ in 0..2 * MAX_DEPTH {
            let mut next = leaves();
            next.extend([str_hash(stored.clone()), arr(stored.clone())]);
            let next = bound(body::union_many(next));
            if next == stored {
                assert!(measure(&stored).1 <= MAX_NODES);
                return;
            }
            stored = next;
        }
        panic!("the bounded unrolling never settled");
    }
}
