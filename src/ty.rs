//! The inference type lattice `Ty` that analyze stamps onto every
//! expression and emitters translate into each target's type system.
//! Rails-specific shapes are first-class variants — `Relation` so
//! query chains can be reasoned about across method boundaries (and
//! folded to SQL) instead of degrading to arrays, `Time` so each
//! target maps its native datetime — because adding a variant forces
//! every exhaustive match to handle it explicitly. Three "no type"
//! variants mean different things: `Var` is an unsolved inference
//! variable (a residual one surfaces as an `UnresolvedType` warning),
//! `Untyped` preserves the public unknown shape (with an internal reason
//! distinguishing a gradual declaration from a computed unknown or pending
//! harvest stub), and `Bottom` is raise/never,
//! absorbed by unions so a raising branch doesn't widen the type.

use indexmap::IndexMap;
use crate::shared::Shared;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TyVar};

/// Internal provenance. Only explicit gradual sources may survive return
/// stabilization. Computed unknowns retain main's behavior; Pending is reserved
/// for the harvest's not-yet-typed return stub. Public equality ignores reasons.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Why(u8);

impl Why {
    pub const PENDING: Why = Why(0);
    pub const INFERRED: Why = Why(1);
    pub const GRADUAL: Why = Why(2);
    pub fn union(self, other: Why) -> Why { Why(self.0.max(other.0)) }
    pub fn is_pending_only(self) -> bool { self.0 == Self::PENDING.0 }
    pub fn is_gradual(self) -> bool { self.0 == Self::GRADUAL.0 }
}

impl Default for Why {
    fn default() -> Why { Why::INFERRED }
}

impl PartialEq for Why {
    fn eq(&self, _: &Self) -> bool { true }
}

/// The types that inhabit Roundhouse values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Sym,

    /// A date-only calendar value, distinct from a timestamp. Storage
    /// is canonical YYYY-MM-DD text; application readers return a Date.
    /// Targets without a date-only runtime must report unsupported,
    /// never substitute a timestamp or a String value.
    Date,

    /// A temporal value — Ruby `Time` (DateTime / time-of-day columns
    /// fold in here too). Date-only columns use `Date`, not this type.
    ///
    /// First-class, deliberately NOT `Ty::Str` or `Ty::Class{"Time"}`:
    /// Time is language-specific like `Hash`, so each target maps it to
    /// its native datetime type (Ruby `Time`, Go `time.Time`, …). A
    /// target that hasn't wired a native representation yet must route
    /// this to an `Unsupported` emit diagnostic rather than silently
    /// degrading — adding the variant forces every exhaustive `match`
    /// to make that choice explicit. Storage stays ISO-8601 TEXT in
    /// every adapter; hydration/serialization happens at the target's
    /// column seam.
    Time,

    Nil,

    /// An unmaterialized ActiveRecord-style query over model `of` —
    /// the analysis-time type of a scope call (`Story.recent`), a
    /// relation-returning class method, or an association read
    /// (`tag.stories`) before a terminal executes it.
    ///
    /// First-class, deliberately NOT `Ty::Array { elem }` or
    /// `Ty::Class { "ActiveRecord::Relation" }`: the variant exists so
    /// the analyzer can reason about query chains across method
    /// boundaries (delegation of the class-side scope surface, chain
    /// builders preserving the receiver, terminals producing
    /// `Array[of]` / `of | Nil` / `Int`). It is an *interpreter-side*
    /// type in the Futamura sense: specialization folds
    /// statically-visible chains into direct SQL at their terminal
    /// (`lower/arel`), so the relation itself is erased from emitted
    /// code. A target whose emitter meets a reachable `Relation` must
    /// route it to an `Unsupported` emit diagnostic, never silently
    /// degrade — adding the variant forces every exhaustive `match`
    /// to make that choice explicit. Inline chains starting at
    /// `Model.where(...)` continue to type as `Array<Self>`; this
    /// variant is introduced only where that approximation fails
    /// (scope returns, relation-returning class-method bodies,
    /// association reads).
    Relation { of: ClassId },

    Array { elem: Arc<Ty> },
    Hash { key: Arc<Ty>, value: Arc<Ty> },
    Tuple { elems: Shared<Vec<Ty>> },
    Record { row: Row },
    Union { variants: Shared<Vec<Ty>> },

    /// An instance of the class that RECEIVED the call — RBS
    /// `instance` (and `self` on an instance-side member), sorbet
    /// `T.attached_class`.
    ///
    /// Carries no class id, because the whole point is that the
    /// DECLARING class does not know it: a factory written once on a
    /// base class answers with an instance of whichever subclass was
    /// called. Without this, every such signature had to be read as
    /// the base class or not at all, and the chain after it went
    /// untyped.
    ///
    /// Exists only between the signature readers and dispatch:
    /// `dispatch` substitutes it with the receiver's class (see
    /// [`Ty::subst_self`]) before the type is stored or joined, so no
    /// emitter should ever meet one. A target whose emitter DOES meet
    /// it must route it to an `Unsupported` emit diagnostic, never
    /// silently degrade — it is a defect that it got that far, and a
    /// silent fallback would hide it.
    SelfInstance,

    Class { id: ClassId, args: Shared<Vec<Ty>> },

    Fn {
        params: Shared<Vec<Param>>,
        block: Option<Arc<Ty>>,
        ret: Arc<Ty>,
        effects: EffectSet,
    },

    Var { var: TyVar },

    /// Public `untyped` shape, including the gradual-typing escape hatch. Distinct from
    /// `Ty::Var` (inference gap) in intent: `Untyped` is an
    /// author-signed declaration that this position opts out of
    /// checking, while `Var` means the analyzer couldn't determine a
    /// type.
    ///
    /// Provenance follows a fail-closed whitelist: source declarations,
    /// identity flows and reads from a gradual receiver retain Gradual.
    /// Other computed unknowns keep main's stabilization behavior.
    ///
    /// Targets that admit a gradual escape hatch (TypeScript `any`,
    /// Python no-annotation, Elixir dynamic dispatch) emit `Untyped`
    /// nodes cleanly. Strict targets (Rust, Go) are expected to
    /// elevate any reachable `Untyped` to an emit-time error via the
    /// diagnostic pipeline — the gradual escape only survives
    /// emission for targets that explicitly accept it.
    Untyped {
        /// Why this position is `untyped` (S2a, #964). Kept outside
        /// value comparison: equality, ordering, serialization and rendering
        /// ignore it, so carrying it cannot move a value.
        #[serde(skip)]
        why: Why,
    },

    /// The bottom type — values of this type don't exist at runtime
    /// because the expression diverges (`raise`, `return`, `next`,
    /// `exit`). Subtype of every other type, so `Bottom ≤ T` for
    /// any T; in `union_of` / `union_many` the variant is filtered
    /// out so `if cond then raise else x end` types as `typeof(x)`,
    /// not `typeof(x) | Nil`.
    ///
    /// Maps to Rust `!`, TypeScript `never`, Python `typing.Never`,
    /// Crystal `NoReturn`. Targets without a native bottom (Go,
    /// Elixir) fall back to a target-appropriate stand-in.
    ///
    /// Mirrors Crystal's `NoReturnType` (compiler/crystal/types.cr);
    /// the union filter is the analog of Crystal's `Type.merge`
    /// dropping NoReturn variants during type joining.
    Bottom,
}

impl Ty {
    /// An unknown computed result. Fail closed: it is never a gradual source.
    pub const fn untyped() -> Ty { Ty::Untyped { why: Why::INFERRED } }

    /// A declared/RBS untyped, catalog-any contract, or external untyped data.
    pub const fn gradual_untyped() -> Ty { Ty::Untyped { why: Why::GRADUAL } }

    /// Only the harvest's placeholder for a return not typed yet.
    pub const fn pending_untyped() -> Ty { Ty::Untyped { why: Why::PENDING } }

    /// Computation is not evidence of a gradual source. Preserve the exact
    /// value shape, clearing provenance at builtin computation boundaries.
    pub(crate) fn computed(&self) -> Ty {
        match self {
            Ty::Untyped { .. } => Ty::untyped(),
            Ty::Array { elem } => Ty::Array { elem: elem.computed().into() },
            Ty::Hash { key, value } => Ty::Hash { key: key.computed().into(), value: value.computed().into() },
            Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(Ty::computed).collect() },
            Ty::Union { variants } => Ty::Union { variants: variants.iter().map(Ty::computed).collect() },
            Ty::Class { id, args } => Ty::Class { id: id.clone(), args: args.iter().map(Ty::computed).collect() },
            Ty::Record { row } => Ty::Record { row: Row {
                fields: row.fields.iter().map(|(k, v)| (k.clone(), v.computed())).collect(),
                rest: row.rest.clone(),
            } },
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params.iter().map(|p| Param { ty: p.ty.computed().into(), ..p.clone() }).collect(),
                block: block.as_ref().map(|t| t.computed().into()),
                ret: ret.computed().into(), effects: effects.clone(),
            },
            other => other.clone(),
        }
    }

    /// Equality for inference state: public value equality plus every
    /// reason read by stabilization. Wire format and public `==` still
    /// ignore reasons; convergence and invalidation must observe them.
    pub(crate) fn inference_eq(&self, other: &Ty) -> bool {
        self == other && self.same_reasons(other)
    }

    /// Called only after value equality has established matching shapes.
    fn same_reasons(&self, other: &Ty) -> bool {
        fn arc_eq(a: &Arc<Ty>, b: &Arc<Ty>) -> bool {
            Arc::ptr_eq(a, b) || a.same_reasons(b)
        }
        match (self, other) {
            (Ty::Untyped { why: a }, Ty::Untyped { why: b }) => {
                a.0 == b.0
            }
            (Ty::Array { elem: a }, Ty::Array { elem: b }) => arc_eq(a, b),
            (Ty::Hash { key: ak, value: av }, Ty::Hash { key: bk, value: bv }) => {
                arc_eq(ak, bk) && arc_eq(av, bv)
            }
            (Ty::Tuple { elems: a }, Ty::Tuple { elems: b })
            | (Ty::Union { variants: a }, Ty::Union { variants: b })
            | (Ty::Class { args: a, .. }, Ty::Class { args: b, .. }) => {
                a.ptr_eq(b) || a.iter().zip(b.iter()).all(|(a, b)| a.same_reasons(b))
            }
            (Ty::Record { row: a }, Ty::Record { row: b }) => {
                a.fields.ptr_eq(&b.fields)
                    || a.fields
                        .iter()
                        .all(|(name, a)| a.same_reasons(&b.fields[name]))
            }
            (
                Ty::Fn {
                    params: a,
                    block: ab,
                    ret: ar,
                    ..
                },
                Ty::Fn {
                    params: b,
                    block: bb,
                    ret: br,
                    ..
                },
            ) => {
                (a.ptr_eq(b) || a.iter().zip(b.iter()).all(|(a, b)| arc_eq(&a.ty, &b.ty)))
                    && match (ab, bb) {
                        (Some(a), Some(b)) => arc_eq(a, b),
                        _ => true,
                    }
                    && arc_eq(ar, br)
            }
            _ => true,
        }
    }

    /// Join reasons at corresponding positions of `==` types. Keep
    /// the stored shape and record field order; allocate children only where
    /// a reason grows from Pending to Gradual.
    pub(crate) fn join_equal_reasons(&self, new: &Ty) -> Option<Ty> {
        debug_assert_eq!(self, new);
        if self.same_reasons(new) {
            return None;
        }
        fn join_arc(existing: &mut std::sync::Arc<Ty>, new: &Ty) -> bool {
            if let Some(joined) = existing.join_equal_reasons(new) {
                *existing = std::sync::Arc::new(joined);
                true
            } else {
                false
            }
        }

        let mut joined = self.clone();
        let changed = match (&mut joined, new) {
            (Ty::Untyped { why }, Ty::Untyped { why: other }) => {
                let next = why.union(*other);
                if why.0 == next.0 {
                    false
                } else {
                    *why = next;
                    true
                }
            }
            (Ty::Array { elem }, Ty::Array { elem: other }) => join_arc(elem, other),
            (
                Ty::Hash { key, value },
                Ty::Hash {
                    key: other_key,
                    value: other_value,
                },
            ) => join_arc(key, other_key) | join_arc(value, other_value),
            (Ty::Tuple { elems }, Ty::Tuple { elems: other })
            | (Ty::Union { variants: elems }, Ty::Union { variants: other })
            | (Ty::Class { args: elems, .. }, Ty::Class { args: other, .. }) => {
                let mut changed = false;
                for (i, next) in other.iter().enumerate() {
                    if let Some(ty) = elems[i].join_equal_reasons(next) {
                        elems[i] = ty;
                        changed = true;
                    }
                }
                changed
            }
            (Ty::Record { row }, Ty::Record { row: other }) => {
                let mut changed = false;
                for (name, next) in &other.fields {
                    if let Some(ty) = row.fields[name].join_equal_reasons(next) {
                        row.fields.insert(name.clone(), ty);
                        changed = true;
                    }
                }
                changed
            }
            (
                Ty::Fn {
                    params, block, ret, ..
                },
                Ty::Fn {
                    params: other,
                    block: other_block,
                    ret: other_ret,
                    ..
                },
            ) => {
                let mut changed = join_arc(ret, other_ret);
                if let (Some(block), Some(other)) = (block, other_block) {
                    changed |= join_arc(block, other);
                }
                for (i, next) in other.iter().enumerate() {
                    if let Some(ty) = params[i].ty.join_equal_reasons(&next.ty) {
                        params[i].ty = std::sync::Arc::new(ty);
                        changed = true;
                    }
                }
                changed
            }
            _ => false,
        };
        changed.then_some(joined)
    }

    /// Replace every [`Ty::SelfInstance`] in this type with `with`.
    ///
    /// The one place the substitution lives. `dispatch` applies it as
    /// soon as it has found a signature, against the class the
    /// ancestor walk STARTED from — the subclass that received the
    /// call, not the class the signature was found on. That is the
    /// whole point: a factory declared once on a base class answers
    /// with an instance of whoever called it.
    ///
    /// Recurses through every type that can CONTAIN one, so
    /// `Array[instance]`, `instance?` and a signature's params are all
    /// substituted, not just a bare return.
    pub fn subst_self(&self, with: &Ty) -> Ty {
        match self {
            Ty::SelfInstance => with.clone(),
            Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(elem.subst_self(with)) },
            Ty::Hash { key, value } => Ty::Hash {
                key: std::sync::Arc::new(key.subst_self(with)),
                value: std::sync::Arc::new(value.subst_self(with)),
            },
            Ty::Tuple { elems } => {
                Ty::Tuple { elems: elems.iter().map(|t| t.subst_self(with)).collect() }
            }
            Ty::Record { row } => Ty::Record {
                row: Row {
                    fields: row
                        .fields
                        .iter()
                        .map(|(name, ty)| (name.clone(), ty.subst_self(with)))
                        .collect(),
                    rest: row.rest.clone(),
                },
            },
            Ty::Union { variants } => {
                Ty::Union { variants: variants.iter().map(|t| t.subst_self(with)).collect() }
            }
            Ty::Class { id, args } => Ty::Class {
                id: id.clone(),
                args: args.iter().map(|t| t.subst_self(with)).collect(),
            },
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Param {
                        name: p.name.clone(),
                        ty: p.ty.subst_self(with).into(),
                        kind: p.kind.clone(),
                    })
                    .collect(),
                block: block.as_ref().map(|b| std::sync::Arc::new(b.subst_self(with))),
                ret: std::sync::Arc::new(ret.subst_self(with)),
                effects: effects.clone(),
            },
            // Leaves, and `Relation { of }` whose `of` is a ClassId
            // rather than a Ty.
            other => other.clone(),
        }
    }

    /// Every `Class { from }` / `Relation { of: from }` rewritten to `to`, recursing like [`Self::subst_self`].
    pub fn rebind_class(&self, from: &ClassId, to: &ClassId) -> Ty {
        let go = |t: &Ty| t.rebind_class(from, to);
        match self {
            Ty::Class { id, args } => Ty::Class {
                id: if id == from { to.clone() } else { id.clone() },
                args: args.iter().map(go).collect(),
            },
            Ty::Relation { of } if of == from => Ty::Relation { of: to.clone() },
            Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(go(elem)) },
            Ty::Hash { key, value } => Ty::Hash { key: std::sync::Arc::new(go(key)), value: std::sync::Arc::new(go(value)) },
            Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(go).collect() },
            Ty::Union { variants } => Ty::Union { variants: variants.iter().map(go).collect() },
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Param { name: p.name.clone(), ty: go(&p.ty).into(), kind: p.kind.clone() })
                    .collect(),
                block: block.as_ref().map(|b| std::sync::Arc::new(go(b))),
                ret: std::sync::Arc::new(go(ret)),
                effects: effects.clone(),
            },
            other => other.clone(),
        }
    }

    /// The same type with every class name it mentions rewritten by
    /// `f`. Structure (containers, unions, signatures) is preserved;
    /// only `Ty::Class` ids change.
    pub fn map_class_ids(&self, f: &dyn Fn(&ClassId) -> ClassId) -> Ty {
        match self {
            Ty::Class { id, args } => Ty::Class {
                id: f(id),
                args: args.iter().map(|t| t.map_class_ids(f)).collect(),
            },
            Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(elem.map_class_ids(f)) },
            Ty::Hash { key, value } => Ty::Hash {
                key: std::sync::Arc::new(key.map_class_ids(f)),
                value: std::sync::Arc::new(value.map_class_ids(f)),
            },
            Ty::Tuple { elems } => {
                Ty::Tuple { elems: elems.iter().map(|t| t.map_class_ids(f)).collect() }
            }
            Ty::Union { variants } => {
                Ty::Union { variants: variants.iter().map(|t| t.map_class_ids(f)).collect() }
            }
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Param { name: p.name.clone(), ty: p.ty.map_class_ids(f).into(), kind: p.kind.clone() })
                    .collect(),
                block: block.as_ref().map(|b| std::sync::Arc::new(b.map_class_ids(f))),
                ret: std::sync::Arc::new(ret.map_class_ids(f)),
                effects: effects.clone(),
            },
            other => other.clone(),
        }
    }

    /// True when an unknown ([`Ty::is_unknown`]) sits anywhere inside
    /// this type: `untyped`, `Array[untyped]`, `Hash[Symbol, untyped]`,
    /// `String | untyped`, a signature with an untyped parameter. A
    /// type that does NOT is fully spelled out, which is what makes a
    /// declaration the last word rather than a hint.
    pub fn mentions_unknown(&self) -> bool {
        match self {
            Ty::Var { .. } | Ty::Untyped { .. } => true,
            Ty::Class { args, .. } => args.iter().any(Ty::mentions_unknown),
            Ty::Array { elem } => elem.mentions_unknown(),
            Ty::Hash { key, value } => key.mentions_unknown() || value.mentions_unknown(),
            Ty::Tuple { elems } => elems.iter().any(Ty::mentions_unknown),
            Ty::Union { variants } => variants.iter().any(Ty::mentions_unknown),
            Ty::Record { row } => {
                row.fields.values().any(Ty::mentions_unknown) || row.rest.is_some()
            }
            Ty::Fn { params, block, ret, .. } => {
                params.iter().any(|p| p.ty.mentions_unknown())
                    || block.as_ref().is_some_and(|b| b.mentions_unknown())
                    || ret.mentions_unknown()
            }
            _ => false,
        }
    }

    /// True for the two "no known type" variants: [`Ty::Var`] (the
    /// analyzer couldn't infer a type) and [`Ty::Untyped`] (an
    /// author-signed gradual-typing opt-out). Both mean "don't reason
    /// about this value's shape."
    ///
    /// Note the deliberate exclusions: [`Ty::Nil`] and [`Ty::Bottom`]
    /// are *known* types, so sites that also treat those as noise use
    /// their own `matches!` and must not be folded into this predicate.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Var { .. } | Ty::Untyped { .. })
    }

    /// The element type of a collection-shaped type: `Array[T]` → `T`,
    /// `Relation[M]` → the record type `M`. `None` for everything else.
    ///
    /// The two spellings are one fact — "many records of this type" —
    /// held apart only by whether the query has materialized yet, and
    /// consumers that care about the element (collection renders, the
    /// N+1 detector, iteration typing) care about neither. This is the
    /// one place that equivalence is written down; a consumer matching
    /// `Ty::Array` directly to find an element is a site that will go
    /// quietly blind the next time a producer flips representation.
    ///
    /// A NILABLE collection — `Array[T] | nil`, the shape a `find_by`
    /// branch or an unassigned-path ivar leaves behind — still has that
    /// element: `render collection: nil` renders nothing, and iterating
    /// nil is a nil-safety question, not an element-type one. Any other
    /// union (two different collections, a record beside an array) is
    /// still `None`.
    pub fn collection_elem(&self) -> Option<Ty> {
        match self {
            Ty::Array { elem } => Some((**elem).clone()),
            Ty::Relation { of } => Some(Ty::Class { id: of.clone(), args: vec![].into() }),
            Ty::Union { variants } => {
                let mut non_nil = variants.iter().filter(|v| !matches!(v, Ty::Nil));
                let first = non_nil.next()?;
                if non_nil.next().is_some() {
                    return None;
                }
                first.collection_elem()
            }
            _ => None,
        }
    }

    /// True for the two variants that leave a value's type open to
    /// refinement: [`Ty::Var`] (not yet inferred) and [`Ty::Bottom`]
    /// (the expression diverges, so any type is admissible). Fixpoint
    /// passes use this to decide "safe to overwrite with a more precise
    /// type." Distinct from [`Ty::is_unknown`], which pairs `Var` with
    /// the *gradual* `Untyped`, not the *divergent* `Bottom`.
    pub fn is_open(&self) -> bool {
        matches!(self, Ty::Var { .. } | Ty::Bottom)
    }

    /// True for the primitive scalar leaf types: `Int`, `Float`,
    /// `Bool`, `Str`, `Sym`. Excludes `Time` (temporal, target-native)
    /// and every compound/reference type. Used by coercion-insertion and
    /// emit paths that treat "a plain scalar value" uniformly.
    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym)
    }

    /// True for the two string-like leaf types: `Str` and `Sym`. Many
    /// targets render a symbol identically to a string, so coercion and
    /// str-coloring paths treat the pair uniformly.
    pub fn is_stringish(&self) -> bool {
        match self {
            Ty::Str | Ty::Sym => true,
            Ty::Union { variants } => !variants.is_empty() && variants.iter().all(Ty::is_stringish),
            _ => false,
        }
    }

    /// True when this type is `Time` or a union containing it — the
    /// shape of a temporal-column reader's return (`Time | Nil`).
    /// Emitters without a native datetime seam key their stored-text
    /// fallback on this.
    pub fn contains_time(&self) -> bool {
        match self {
            Ty::Time => true,
            Ty::Union { variants } => variants.iter().any(Ty::contains_time),
            _ => false,
        }
    }

    /// A date-only value anywhere in a type, including unused signature
    /// parameters. Used to reject unsupported targets before rendering.
    pub fn contains_date(&self) -> bool {
        match self {
            Ty::Date => true,
            Ty::Array { elem } => elem.contains_date(),
            Ty::Hash { key, value } => key.contains_date() || value.contains_date(),
            Ty::Tuple { elems } => elems.iter().any(Ty::contains_date),
            Ty::Union { variants } => variants.iter().any(Ty::contains_date),
            Ty::Record { row } => row.fields.values().any(Ty::contains_date),
            Ty::Class { id, args } => id.0.as_str() == "Date" || args.iter().any(Ty::contains_date),
            Ty::Fn { params, block, ret, .. } => {
                params.iter().any(|p| p.ty.contains_date())
                    || block.as_ref().is_some_and(|b| b.contains_date())
                    || ret.contains_date()
            }
            _ => false,
        }
    }

    /// For a binary nilable union `T | Nil`, return the non-`Nil` arm
    /// `T`; otherwise return `self` unchanged. Only the exact two-variant
    /// `{T, Nil}` shape is peeled — wider unions stay intact.
    pub(crate) fn peel_nilable(&self) -> &Ty {
        if let Ty::Union { variants } = self {
            if variants.len() == 2 {
                let nil_idx = variants.iter().position(|v| matches!(v, Ty::Nil));
                if let Some(idx) = nil_idx {
                    return &variants[1 - idx];
                }
            }
        }
        self
    }

    /// Drop every `Nil` variant from a union, returning the reduced
    /// type: the sole survivor if one remains, a narrower `Union` if
    /// several do, and `Nil` if the union was all-`Nil`. Non-union
    /// types pass through unchanged. (This normalizes membership only;
    /// it does not re-canonicalize variant order.)
    pub(crate) fn strip_nil(self) -> Ty {
        let Ty::Union { variants } = self else { return self };
        let kept: Vec<Ty> = variants
            .into_iter()
            .filter(|v| !matches!(v, Ty::Nil))
            .collect();
        match kept.len() {
            0 => Ty::Nil,
            1 => kept.into_iter().next().unwrap(),
            _ => Ty::Union { variants: kept.into() },
        }
    }

    /// True when `self` is [`Ty::Untyped`] / [`Ty::Var`], or a union
    /// that carries either as a top-level arm.
    pub(crate) fn has_unknown_arm(&self) -> bool {
        match self {
            Ty::Union { variants } => variants.iter().any(Ty::is_unknown),
            other => other.is_unknown(),
        }
    }

    /// Drop every top-level [`Ty::Untyped`] / [`Ty::Var`] arm from a
    /// union, returning the concrete core: the sole survivor if one
    /// remains, a narrower `Union` if several do, and `Untyped` if
    /// nothing concrete remains. Unlike [`Self::strip_nil`] (which
    /// leaves non-unions unchanged, including bare `Nil`), a bare
    /// [`Ty::Var`] collapses to [`Ty::Untyped`] so harvest cores treat
    /// inference variables like unknown; other non-unions pass through.
    pub(crate) fn strip_unknown(self) -> Ty {
        match self {
            Ty::Union { variants } => {
                let kept: Vec<Ty> = variants.into_iter().filter(|v| !v.is_unknown()).collect();
                match kept.len() {
                    0 => Ty::untyped(),
                    1 => kept.into_iter().next().unwrap(),
                    _ => Ty::Union { variants: kept.into() },
                }
            }
            Ty::Var { .. } => Ty::untyped(),
            other => other,
        }
    }

    /// Sort a flattened variant list into the canonical order: `Nil`
    /// last (so nilable unions keep reading `T | Nil`), everything else
    /// by a structural total order. Two unions built from the same
    /// variants in any join order compare equal under derived `==` only
    /// because of this; `analyze`'s lattice join (`union_of`) relies on
    /// it for fixpoint convergence.
    ///
    /// Allocating a `Debug` string per variant on every join was the
    /// previous key; Campfire's analyzer fixpoint does this each round.
    pub(crate) fn canonicalize_variants(variants: &mut [Ty]) {
        variants.sort_by(cmp_ty_nil_last);
    }

    /// Of two `==` types, the one canonical order puts first, with their
    /// reasons joined recursively. Record field order can differ too;
    /// picking by order instead of by arrival keeps a join's stored
    /// form independent of the order its inputs came in.
    pub(crate) fn canonical_min(a: Ty, b: Ty) -> Ty {
        let (chosen, other) = if !a.holds_record() || cmp_ty(&b, &a) != std::cmp::Ordering::Less {
            (a, b)
        } else {
            (b, a)
        };
        chosen.join_equal_reasons(&other).unwrap_or(chosen)
    }

    fn holds_record(&self) -> bool {
        match self {
            Ty::Record { .. } => true,
            Ty::Array { elem } => elem.holds_record(),
            Ty::Hash { key, value } => key.holds_record() || value.holds_record(),
            Ty::Tuple { elems } | Ty::Union { variants: elems } | Ty::Class { args: elems, .. } => {
                elems.iter().any(Ty::holds_record)
            }
            Ty::Fn { params, block, ret, .. } => {
                params.iter().any(|p| p.ty.holds_record())
                    || block.as_ref().is_some_and(|b| b.holds_record())
                    || ret.holds_record()
            }
            _ => false,
        }
    }
}

fn cmp_ty_nil_last(a: &Ty, b: &Ty) -> std::cmp::Ordering {
    match (matches!(a, Ty::Nil), matches!(b, Ty::Nil)) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => cmp_ty(a, b),
    }
}

fn ty_tag(ty: &Ty) -> u8 {
    match ty {
        Ty::Int => 0,
        Ty::Float => 1,
        Ty::Bool => 2,
        Ty::Str => 3,
        Ty::Sym => 4,
        Ty::Date => 5,
        Ty::Time => 6,
        Ty::Relation { .. } => 7,
        Ty::Array { .. } => 8,
        Ty::Hash { .. } => 9,
        Ty::Tuple { .. } => 10,
        Ty::Record { .. } => 11,
        Ty::Union { .. } => 12,
        Ty::SelfInstance => 13,
        Ty::Class { .. } => 14,
        Ty::Fn { .. } => 15,
        Ty::Var { .. } => 16,
        Ty::Untyped { .. } => 17,
        Ty::Bottom => 18,
        Ty::Nil => 19,
    }
}

fn cmp_ty(a: &Ty, b: &Ty) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    ty_tag(a).cmp(&ty_tag(b)).then_with(|| match (a, b) {
        (Ty::Relation { of: x }, Ty::Relation { of: y }) => x.cmp(y),
        (Ty::Array { elem: x }, Ty::Array { elem: y }) => cmp_ty(x, y),
        (Ty::Hash { key: kx, value: vx }, Ty::Hash { key: ky, value: vy }) => {
            cmp_ty(kx, ky).then_with(|| cmp_ty(vx, vy))
        }
        (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => cmp_ty_slice(x, y),
        (Ty::Record { row: x }, Ty::Record { row: y }) => cmp_row(x, y),
        (Ty::Union { variants: x }, Ty::Union { variants: y }) => cmp_ty_slice(x, y),
        (Ty::Class { id: ix, args: ax }, Ty::Class { id: iy, args: ay }) => {
            ix.cmp(iy).then_with(|| cmp_ty_slice(ax, ay))
        }
        (
            Ty::Fn { params: px, block: bx, ret: rx, effects: ex },
            Ty::Fn { params: py, block: by, ret: ry, effects: ey },
        ) => cmp_params(px, py)
            .then_with(|| match (bx, by) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(x), Some(y)) => cmp_ty(x, y),
            })
            .then_with(|| cmp_ty(rx, ry))
            .then_with(|| ex.effects.cmp(&ey.effects)),
        (Ty::Var { var: x }, Ty::Var { var: y }) => x.cmp(y),
        _ => Ordering::Equal,
    })
}

fn cmp_ty_slice(a: &[Ty], b: &[Ty]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| {
        a.iter()
            .zip(b)
            .map(|(x, y)| cmp_ty(x, y))
            .find(|o| *o != std::cmp::Ordering::Equal)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Rows compare by their fields sorted by name, then `rest`, so the
/// order agrees with derived `==` (an `IndexMap` compares without
/// regard to insertion order) and a union of records sorts the same
/// whatever field order each arrived in (#617). Only rows that are
/// `==` are then told apart by field order, so a total order still
/// picks one stored form among them (see [`Ty::canonical_min`]);
/// stored rows keep a source order and are never re-sorted.
fn cmp_row(a: &Row, b: &Row) -> std::cmp::Ordering {
    fn sorted(r: &Row) -> Vec<(&Symbol, &Ty)> {
        let mut fields: Vec<(&Symbol, &Ty)> = r.fields.iter().collect();
        fields.sort_by(|x, y| x.0.cmp(y.0));
        fields
    }
    a.fields
        .len()
        .cmp(&b.fields.len())
        .then_with(|| {
            sorted(a)
                .into_iter()
                .zip(sorted(b))
                .map(|((ka, va), (kb, vb))| ka.cmp(kb).then_with(|| cmp_ty(va, vb)))
                .find(|o| *o != std::cmp::Ordering::Equal)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| a.rest.cmp(&b.rest))
        .then_with(|| a.fields.keys().cmp(b.fields.keys()))
}

fn cmp_params(a: &[Param], b: &[Param]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| {
        a.iter()
            .zip(b)
            .map(|(x, y)| {
                x.name
                    .cmp(&y.name)
                    .then_with(|| cmp_ty(&x.ty, &y.ty))
                    .then_with(|| param_kind_tag(&x.kind).cmp(&param_kind_tag(&y.kind)))
                    .then_with(|| match (&x.kind, &y.kind) {
                        (ParamKind::Keyword { required: ra }, ParamKind::Keyword { required: rb }) => {
                            ra.cmp(rb)
                        }
                        _ => std::cmp::Ordering::Equal,
                    })
            })
            .find(|o| *o != std::cmp::Ordering::Equal)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn param_kind_tag(kind: &ParamKind) -> u8 {
    match kind {
        ParamKind::Required => 0,
        ParamKind::Optional => 1,
        ParamKind::Rest => 2,
        ParamKind::Keyword { .. } => 3,
        ParamKind::KeywordRest => 4,
        ParamKind::Block => 5,
    }
}

/// A row-polymorphic record shape.
/// `fields` are known; `rest` is the open-extension variable if this is a partial view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub fields: Shared<IndexMap<Symbol, Ty>>,
    pub rest: Option<TyVar>,
}

impl Row {
    pub fn closed() -> Self {
        Row { fields: IndexMap::new().into(), rest: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Symbol,
    pub ty: std::sync::Arc<Ty>,
    pub kind: ParamKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Required,
    Optional,
    Rest,
    Keyword { required: bool },
    KeywordRest,
    Block,
}

impl From<std::sync::Arc<Ty>> for Ty {
    fn from(value: std::sync::Arc<Ty>) -> Self {
        std::sync::Arc::unwrap_or_clone(value)
    }
}

#[cfg(test)]
pub(crate) mod reason_tests {
    use super::*;

    /// One changed reason at every position which can hold a type,
    /// including a mixed nested shape and two independently changed leaves.
    pub(crate) fn shapes(leaf: Ty) -> Vec<(&'static str, Ty)> {
        let array = |t| Ty::Array { elem: Arc::new(t) };
        let hash = |key, value| Ty::Hash {
            key: Arc::new(key),
            value: Arc::new(value),
        };
        let record = |t| Ty::Record {
            row: Row {
                fields: [(Symbol::from("z"), t), (Symbol::from("a"), Ty::Int)]
                    .into_iter()
                    .collect(),
                rest: Some(TyVar(7)),
            },
        };
        let function = |param: Ty, block: Option<Ty>, ret: Ty| Ty::Fn {
            params: vec![Param {
                name: Symbol::from("value"),
                ty: Arc::new(param),
                kind: ParamKind::Required,
            }]
            .into(),
            block: block.map(Arc::new),
            ret: Arc::new(ret),
            effects: EffectSet::default(),
        };
        vec![
            ("bare", leaf.clone()),
            ("array", array(leaf.clone())),
            ("hash-key", hash(leaf.clone(), Ty::Str)),
            ("hash-value", hash(Ty::Str, leaf.clone())),
            ("hash-both", hash(leaf.clone(), leaf.clone())),
            (
                "tuple-first",
                Ty::Tuple {
                    elems: vec![leaf.clone(), Ty::Str].into(),
                },
            ),
            (
                "tuple-last",
                Ty::Tuple {
                    elems: vec![Ty::Str, leaf.clone()].into(),
                },
            ),
            ("record", record(leaf.clone())),
            (
                "union",
                Ty::Union {
                    variants: vec![Ty::Sym, leaf.clone()].into(),
                },
            ),
            (
                "class",
                Ty::Class {
                    id: ClassId(Symbol::from("Box")),
                    args: vec![Ty::Int, leaf.clone()].into(),
                },
            ),
            ("fn-param", function(leaf.clone(), None, Ty::Str)),
            ("fn-block", function(Ty::Int, Some(leaf.clone()), Ty::Str)),
            ("fn-return", function(Ty::Int, None, leaf.clone())),
            ("nested", array(hash(Ty::Str, record(array(leaf))))),
        ]
    }

    #[test]
    fn computed_results_reset_every_unknown_position_without_changing_value_shape() {
        for origin in [Ty::pending_untyped(), Ty::gradual_untyped(), Ty::untyped()] {
            for ((name, input), (_, expected)) in shapes(origin).into_iter().zip(shapes(Ty::untyped())) {
                let result = input.computed();
                assert_eq!(result, input, "public value shape: {name}");
                assert!(result.inference_eq(&expected), "non-source computation: {name}");
            }
        }
    }

    #[test]
    fn inference_equality_observes_reasons_at_every_type_position() {
        for ((name, pending), (_, gradual)) in shapes(Ty::pending_untyped())
            .into_iter()
            .zip(shapes(Ty::gradual_untyped()))
        {
            assert_eq!(pending, gradual, "public equality: {name}");
            assert!(
                !pending.inference_eq(&gradual),
                "inference equality: {name}"
            );
            assert!(
                !gradual.inference_eq(&pending),
                "reverse inference equality: {name}"
            );
            assert!(
                pending.inference_eq(&pending.clone()),
                "reflexivity: {name}"
            );
            assert!(
                gradual.inference_eq(&gradual.clone()),
                "reflexivity: {name}"
            );
            let joined = pending.join_equal_reasons(&gradual).expect(name);
            assert!(joined.inference_eq(&gradual), "joined: {name}");
            assert!(
                gradual.join_equal_reasons(&pending).is_none(),
                "monotonicity: {name}"
            );
            assert!(
                pending.join_equal_reasons(&pending).is_none(),
                "idempotence: {name}"
            );
        }
        assert!(!Ty::Str.inference_eq(&Ty::Sym));
        assert!(
            !Ty::Tuple {
                elems: vec![Ty::Str].into()
            }
            .inference_eq(&Ty::Tuple {
                elems: vec![].into()
            })
        );
    }

    #[test]
    fn equal_type_union_joins_nested_reasons_with_all_operand_shapes() {
        use crate::analyze::union_of;
        for ((name, pending), (_, gradual)) in shapes(Ty::pending_untyped())
            .into_iter()
            .zip(shapes(Ty::gradual_untyped()))
        {
            for (a, b) in [
                (pending.clone(), gradual.clone()),
                (gradual.clone(), pending.clone()),
            ] {
                let joined = union_of(a.clone(), b.clone());
                assert!(joined.inference_eq(&gradual), "direct: {name}: {joined:?}");
                // Distinct outer arms exercise flattened union dedup instead
                // of the value-equality shortcut at the root.
                let joined = union_of(union_of(Ty::Nil, a.clone()), union_of(b.clone(), Ty::Int));
                let expected = union_of(union_of(Ty::Nil, gradual.clone()), Ty::Int);
                assert!(
                    joined.inference_eq(&expected),
                    "union wrapped: {name}: {joined:?}"
                );
                for inputs in [
                    vec![a.clone(), b.clone(), pending.clone()],
                    vec![pending.clone(), a, b],
                ] {
                    assert!(
                        inputs
                            .into_iter()
                            .reduce(union_of)
                            .unwrap()
                            .inference_eq(&gradual),
                        "many: {name}"
                    );
                }
                let right_grouped =
                    union_of(pending.clone(), union_of(gradual.clone(), pending.clone()));
                assert!(
                    right_grouped.inference_eq(&gradual),
                    "associativity: {name}"
                );
                assert!(
                    pending.join_equal_reasons(&pending).is_none(),
                    "source mutated: {name}"
                );
            }
        }
    }

    #[test]
    fn canonical_record_order_and_reason_join_are_independent() {
        let record = |fields: Vec<(&str, Ty)>| Ty::Record {
            row: Row {
                fields: fields
                    .into_iter()
                    .map(|(k, v)| (Symbol::from(k), v))
                    .collect(),
                rest: None,
            },
        };
        let a = record(vec![("z", Ty::pending_untyped()), ("a", Ty::gradual_untyped())]);
        let b = record(vec![("a", Ty::pending_untyped()), ("z", Ty::gradual_untyped())]);
        let expected = record(vec![("a", Ty::gradual_untyped()), ("z", Ty::gradual_untyped())]);
        for ((name, a), ((_, b), (_, expected))) in shapes(a)
            .into_iter()
            .zip(shapes(b).into_iter().zip(shapes(expected)))
        {
            for (a, b) in [(a.clone(), b.clone()), (b, a)] {
                let joined = Ty::canonical_min(a, b);
                assert!(joined.inference_eq(&expected), "{name}");
                assert_eq!(
                    serde_json::to_string(&joined).unwrap(),
                    serde_json::to_string(&expected).unwrap(),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn untyped_reason_is_invisible_to_equality_order_wire_and_rendering() {
        let pending = Ty::pending_untyped();
        let gradual = Ty::gradual_untyped();
        assert_eq!(pending, gradual);
        assert_eq!(cmp_ty(&pending, &gradual), std::cmp::Ordering::Equal);
        for ty in [&pending, &gradual] {
            assert_eq!(serde_json::to_string(ty).unwrap(), r#"{"kind":"untyped"}"#);
            assert_eq!(crate::emit::ruby::ty_to_rbs(ty), "untyped");
        }
        let decoded: Ty = serde_json::from_str(r#"{"kind":"untyped"}"#).unwrap();
        assert!(matches!(decoded, Ty::Untyped { why } if !why.is_gradual()));

        let nested = |ty| Ty::Array { elem: Arc::new(ty) };
        let a = nested(pending);
        let b = nested(gradual);
        assert_eq!(a, b);
        assert_eq!(cmp_ty(&a, &b), std::cmp::Ordering::Equal);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert_eq!(
            crate::emit::ruby::ty_to_rbs(&a),
            crate::emit::ruby::ty_to_rbs(&b)
        );
    }

    #[test]
    fn reason_join_has_pending_identity_and_is_monotone() {
        for a in [Why::PENDING, Why::INFERRED, Why::GRADUAL] {
            assert_eq!(a.union(Why::PENDING).0, a.0);
            assert_eq!(Why::PENDING.union(a).0, a.0);
            assert_eq!(a.union(a).0, a.0);
            assert_eq!(a.union(Why::GRADUAL).0, Why::GRADUAL.0);
            for b in [Why::PENDING, Why::INFERRED, Why::GRADUAL] {
                assert_eq!(a.union(b).0, b.union(a).0);
                for c in [Why::PENDING, Why::INFERRED, Why::GRADUAL] {
                    assert_eq!(a.union(b).union(c).0, a.union(b.union(c)).0);
                }
            }
        }
    }
}
