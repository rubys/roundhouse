//! Harvested method-return stabilization across fixpoint rounds.
//!
//! Circular accessors (`config` ↔ `Configuration.load!`) can thrash the
//! harvest table between a concrete type and `Concrete|Untyped` every
//! round and exhaust `FIXPOINT_CAP`. When an existing entry and a new
//! body type share the same concrete core (after dropping top-level
//! `Untyped`/`Var` arms), store that core. Nil-only cores keep sticky
//! `Nil|Untyped` — a full lattice join was tried and rejected because
//! `Untyped` then `Nil` collapsed Campfire URI helpers to bare `Nil`.
//!
//! First writes are stored as-is (including gradual unions). Any later
//! same-core pair collapses per stabilize (Nil sticky). A bare
//! `Untyped`/`Var` never replaces an already-informative return.
//! Distinct concrete cores still last-write (residual thrash for a
//! later call-graph pass).

use crate::ident::Symbol;
use crate::ty::Ty;

use super::body;

fn gradual_nil() -> Ty {
    body::union_of(Ty::Nil, Ty::Untyped)
}

/// True when stripping top-level unknown arms leaves a usable core
/// (anything other than bare [`Ty::Untyped`]).
fn has_informative_core(ty: &Ty) -> bool {
    match ty {
        Ty::Untyped | Ty::Var { .. } => false,
        // Nested unions (a sorbet signature keeps their shape) count only
        // if some arm somewhere is known.
        Ty::Union { variants } => variants.iter().any(has_informative_core),
        _ => true,
    }
}

/// If two harvested returns differ only by top-level `Untyped`/`Var`
/// arms, return the stable form to store. Non-Nil concrete cores keep
/// the stripped type (`Configuration|Untyped` → `Configuration`).
/// Nil-only cores keep `Nil|Untyped` when either side was gradual.
fn stabilize_untyped_return_oscillation(existing: &Ty, new: &Ty) -> Option<Ty> {
    let core_e = existing.clone().strip_unknown();
    let core_n = new.clone().strip_unknown();
    if core_e != core_n {
        return None;
    }
    if matches!(core_e, Ty::Nil) {
        if existing.has_unknown_arm() || new.has_unknown_arm() {
            return Some(gradual_nil());
        }
        return Some(core_e);
    }
    Some(core_e)
}

/// `ty` with every strict subterm that is `prior` replaced by `untyped`. A
/// union holding all of `prior`'s variants counts: `prior` flattens into a
/// union it joins, so it is never there as one term.
fn untie(ty: &Ty, prior: &Ty) -> Ty {
    let go = |t: &Ty| {
        if t == prior {
            return Ty::Untyped;
        }
        if let (Ty::Union { variants: pv }, Ty::Union { variants: tv }) = (prior, t)
            && pv.iter().all(|v| tv.contains(v))
        {
            let mut rest: Vec<Ty> = tv.iter().filter(|v| !pv.contains(v)).map(|v| untie(v, prior)).collect();
            if !rest.contains(&Ty::Untyped) {
                rest.push(Ty::Untyped);
            }
            return Ty::Union { variants: rest };
        }
        untie(t, prior)
    };
    match ty {
        Ty::Array { elem } => Ty::Array { elem: Box::new(go(elem)) },
        Ty::Hash { key, value } => Ty::Hash { key: Box::new(go(key)), value: Box::new(go(value)) },
        Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(go).collect() },
        Ty::Union { variants } => Ty::Union { variants: variants.iter().map(go).collect() },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row.fields.iter().map(|(name, field)| (name.clone(), go(field))).collect(),
                rest: row.rest.clone(),
            },
        },
        Ty::Class { id, args } => Ty::Class { id: id.clone(), args: args.iter().map(go).collect() },
        other => other.clone(),
    }
}

/// A return that nests the previous round's return is a recursive method
/// (`value.map { |v| sanitize(v) }`) unrolled one level more: left alone it
/// doubles every round and never converges. The nested copy is cut to
/// `untyped`. A scalar previous return is too weak a witness to cut on.
fn untie_recursive_return(existing: &Ty, new: &Ty) -> Option<Ty> {
    if !matches!(
        existing,
        Ty::Union { .. } | Ty::Array { .. } | Ty::Hash { .. } | Ty::Tuple { .. } | Ty::Record { .. }
    )
        && !matches!(existing, Ty::Class { args, .. } if !args.is_empty())
    {
        return None;
    }
    let untied = untie(new, existing);
    (untied != *new).then_some(untied)
}

enum HarvestWrite {
    Keep,
    Set(Ty),
}

/// Single merge decision for an existing harvested return vs a new body type.
fn decide_harvested_return(existing: &Ty, new: Ty) -> HarvestWrite {
    if matches!(existing, Ty::Fn { .. }) {
        return HarvestWrite::Keep;
    }
    if existing == &new {
        return HarvestWrite::Keep;
    }
    if let Some(untied) = untie_recursive_return(existing, &new) {
        if existing == &untied {
            return HarvestWrite::Keep;
        }
        return HarvestWrite::Set(untied);
    }
    if let Some(stable) = stabilize_untyped_return_oscillation(existing, &new) {
        if existing == &stable {
            return HarvestWrite::Keep;
        }
        return HarvestWrite::Set(stable);
    }
    // Bare `Untyped`/`Var`, and unions of only those, must not wipe a
    // concrete return. `Union[Untyped, Untyped]` is not `is_unknown()`
    // (that matches only the bare forms) but it has no informative core.
    if has_informative_core(existing) && (new.is_unknown() || !has_informative_core(&new)) {
        return HarvestWrite::Keep;
    }
    // Distinct cores: last-write wins (residual thrash; not this PR's fix).
    HarvestWrite::Set(new)
}

/// Conservative insertion into the harvested-return table.
///
/// RBS-sourced `Ty::Fn` stays authoritative. Same-core returns that
/// differ only by unknown arms stabilize to the concrete core (Nil
/// cores stay gradual). An informative existing return is never
/// replaced by bare `Untyped`/`Var`. First writes are stored as-is,
/// including gradual unions.
pub(super) fn insert_inferred_return(
    table: &mut std::collections::HashMap<Symbol, Ty>,
    method: &Symbol,
    ty: Ty,
) {
    let ty = super::fixpoint_bound::bound(ty);
    match table.get(method) {
        None => {
            table.insert(method.clone(), ty);
        }
        Some(existing) => match decide_harvested_return(existing, ty) {
            HarvestWrite::Keep => {}
            HarvestWrite::Set(next) => {
                table.insert(method.clone(), next);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{ClassId, TyVar};
    use std::collections::HashMap;

    fn cfg() -> Ty {
        Ty::Class {
            id: ClassId(Symbol::from("Probe::Configuration")),
            args: vec![],
        }
    }

    #[test]
    fn configuration_versus_configuration_or_untyped_stabilizes_to_configuration() {
        let concrete = cfg();
        let noisy = body::union_of(cfg(), Ty::Untyped);
        let stable = stabilize_untyped_return_oscillation(&concrete, &noisy)
            .expect("same concrete core");
        assert_eq!(stable, concrete);
        let stable_rev = stabilize_untyped_return_oscillation(&noisy, &concrete)
            .expect("order-independent");
        assert_eq!(stable_rev, concrete);
    }

    #[test]
    fn nil_versus_nil_or_untyped_keeps_gradual_nil() {
        let nil = Ty::Nil;
        let gradual = body::union_of(Ty::Nil, Ty::Untyped);
        let stable =
            stabilize_untyped_return_oscillation(&nil, &gradual).expect("nil-only cores match");
        assert_eq!(stable, gradual);
    }

    #[test]
    fn a_nested_union_of_unknown_arms_is_not_informative() {
        let unknown = Ty::Union {
            variants: vec![Ty::Union { variants: vec![Ty::Untyped, Ty::Var { var: TyVar(0) }] }, Ty::Untyped],
        };
        assert!(!has_informative_core(&unknown));
        let known = Ty::Union { variants: vec![Ty::Union { variants: vec![Ty::Str, Ty::Untyped] }, Ty::Untyped] };
        assert!(has_informative_core(&known));
    }

    #[test]
    fn distinct_concrete_cores_do_not_stabilize() {
        assert!(stabilize_untyped_return_oscillation(&Ty::Str, &Ty::Int).is_none());
    }

    #[test]
    fn var_nil_versus_untyped_nil_keeps_gradual_nil() {
        let a = body::union_of(Ty::Var { var: TyVar(0) }, Ty::Nil);
        let b = body::union_of(Ty::Untyped, Ty::Nil);
        let stable = stabilize_untyped_return_oscillation(&a, &b).expect("same nil core");
        assert_eq!(stable, gradual_nil());
    }

    #[test]
    fn insert_preserves_rbs_fn() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        let fn_ty = Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::Str),
            block: None,
            effects: crate::effect::EffectSet::default(),
        };
        insert_inferred_return(&mut table, &method, fn_ty.clone());
        insert_inferred_return(&mut table, &method, cfg());
        assert_eq!(table.get(&method), Some(&fn_ty));
    }

    #[test]
    fn insert_stabilizes_configuration_versus_noisy() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, cfg());
        insert_inferred_return(&mut table, &method, body::union_of(cfg(), Ty::Untyped));
        assert_eq!(table.get(&method), Some(&cfg()));
    }

    #[test]
    fn insert_preserves_first_write_of_gradual_union() {
        let method = Symbol::from("build");
        let mut table = HashMap::new();
        let gradual = body::union_of(Ty::Str, Ty::Untyped);
        insert_inferred_return(&mut table, &method, gradual.clone());
        assert_eq!(table.get(&method), Some(&gradual));
    }

    #[test]
    fn insert_gradual_first_write_then_same_core_concrete_narrows() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, body::union_of(cfg(), Ty::Untyped));
        insert_inferred_return(&mut table, &method, cfg());
        assert_eq!(table.get(&method), Some(&cfg()));
    }

    #[test]
    fn insert_distinct_cores_last_write_wins() {
        let method = Symbol::from("value");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, Ty::Str);
        insert_inferred_return(&mut table, &method, Ty::Int);
        assert_eq!(table.get(&method), Some(&Ty::Int));
    }

    #[test]
    fn insert_keeps_known_return_against_bare_untyped() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, cfg());
        insert_inferred_return(&mut table, &method, Ty::Untyped);
        assert_eq!(table.get(&method), Some(&cfg()));
    }

    #[test]
    fn insert_keeps_known_return_against_untyped_only_union() {
        // `Union[Untyped, Untyped]` is not bare `is_unknown()`, but it has
        // no informative core and must not wipe a concrete harvest —
        // class_attribute readers thrash on that shape across rounds.
        let method = Symbol::from("_preload_definitions");
        let concrete = arr(Ty::Str);
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, concrete.clone());
        insert_inferred_return(
            &mut table,
            &method,
            Ty::Union { variants: vec![Ty::Untyped, Ty::Untyped] },
        );
        assert_eq!(table.get(&method), Some(&concrete));
    }

    #[test]
    fn insert_gradual_nil_is_sticky_against_bare_nil() {
        let method = Symbol::from("uri");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, gradual_nil());
        insert_inferred_return(&mut table, &method, Ty::Nil);
        assert_eq!(table.get(&method), Some(&gradual_nil()));
    }

    fn arr(elem: Ty) -> Ty {
        Ty::Array { elem: Box::new(elem) }
    }

    fn sym_hash(value: Ty) -> Ty {
        Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(value) }
    }

    fn union(variants: Vec<Ty>) -> Ty {
        Ty::Union { variants }
    }

    // `def sanitize(v) = v.is_a?(Array) ? v.map { sanitize(_1) } : v.to_s`
    #[test]
    fn insert_recursive_return_that_nests_its_previous_round_keeps_the_previous() {
        let method = Symbol::from("sanitize");
        let mut table = HashMap::new();
        let first = union(vec![Ty::Str, arr(Ty::Untyped)]);
        insert_inferred_return(&mut table, &method, first.clone());
        insert_inferred_return(&mut table, &method, union(vec![Ty::Str, arr(first.clone())]));
        assert_eq!(table.get(&method), Some(&first));
    }

    #[test]
    fn insert_recursive_return_flattened_into_a_union_is_cut() {
        let method = Symbol::from("sanitize");
        let mut table = HashMap::new();
        let first = union(vec![Ty::Str, sym_hash(Ty::Untyped)]);
        insert_inferred_return(&mut table, &method, first.clone());
        let nested = union(vec![Ty::Str, sym_hash(union(vec![Ty::Str, sym_hash(Ty::Untyped), Ty::Int]))]);
        insert_inferred_return(&mut table, &method, nested);
        let cut = union(vec![Ty::Str, sym_hash(union(vec![Ty::Int, Ty::Untyped]))]);
        assert_eq!(table.get(&method), Some(&cut));
        insert_inferred_return(&mut table, &method, union(vec![Ty::Str, sym_hash(union(vec![Ty::Str, sym_hash(union(vec![Ty::Int, Ty::Untyped])), Ty::Int]))]));
        assert_eq!(table.get(&method), Some(&cut));
    }

    // `def wrap(v) = { nested: wrap(v.inner) }`
    #[test]
    fn insert_recursive_return_nested_in_a_record_keeps_the_previous() {
        let method = Symbol::from("wrap");
        let record = |field: Ty| Ty::Record {
            row: crate::ty::Row { fields: [(Symbol::from("nested"), field)].into_iter().collect(), rest: None },
        };
        let mut table = HashMap::new();
        let first = record(Ty::Untyped);
        insert_inferred_return(&mut table, &method, first.clone());
        insert_inferred_return(&mut table, &method, record(first.clone()));
        assert_eq!(table.get(&method), Some(&first));
    }

    #[test]
    fn insert_scalar_previous_return_is_not_a_recursion_witness() {
        let method = Symbol::from("names");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, Ty::Str);
        let widened = union(vec![Ty::Str, arr(Ty::Str)]);
        insert_inferred_return(&mut table, &method, widened.clone());
        assert_eq!(table.get(&method), Some(&widened));
    }
}
