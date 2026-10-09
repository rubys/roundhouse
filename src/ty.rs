//! The inference type lattice `Ty` that analyze stamps onto every
//! expression and emitters translate into each target's type system.
//! Rails-specific shapes are first-class variants — `Relation` so
//! query chains can be reasoned about across method boundaries (and
//! folded to SQL) instead of degrading to arrays, `Time` so each
//! target maps its native datetime — because adding a variant forces
//! every exhaustive match to handle it explicitly. Three "no type"
//! variants mean different things: `Var` is an unsolved inference
//! variable (a residual one surfaces as an `UnresolvedType` warning),
//! `Untyped` is RBS `untyped` — a deliberate gradual opt-out that
//! strict targets reject at emit — and `Bottom` is raise/never,
//! absorbed by unions so a raising branch doesn't widen the type.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TyVar};

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

    Array { elem: Box<Ty> },
    Hash { key: Box<Ty>, value: Box<Ty> },
    Tuple { elems: Vec<Ty> },
    Record { row: Row },
    Union { variants: Vec<Ty> },

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

    Class { id: ClassId, args: Vec<Ty> },

    Fn {
        params: Vec<Param>,
        block: Option<Box<Ty>>,
        ret: Box<Ty>,
        effects: EffectSet,
    },

    Var { var: TyVar },

    /// RBS `untyped` — gradual-typing escape hatch. Distinct from
    /// `Ty::Var` (inference gap) in intent: `Untyped` is an
    /// author-signed declaration that this position opts out of
    /// checking, while `Var` means the analyzer couldn't determine a
    /// type.
    ///
    /// Propagation: dispatching a method on `Untyped` returns
    /// `Untyped`, so the gradual choice flows through the IR
    /// unconditionally.
    ///
    /// Targets that admit a gradual escape hatch (TypeScript `any`,
    /// Python no-annotation, Elixir dynamic dispatch) emit `Untyped`
    /// nodes cleanly. Strict targets (Rust, Go) are expected to
    /// elevate any reachable `Untyped` to an emit-time error via the
    /// diagnostic pipeline — the gradual escape only survives
    /// emission for targets that explicitly accept it.
    Untyped,

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
            Ty::Array { elem } => Ty::Array { elem: Box::new(elem.subst_self(with)) },
            Ty::Hash { key, value } => Ty::Hash {
                key: Box::new(key.subst_self(with)),
                value: Box::new(value.subst_self(with)),
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
                        ty: p.ty.subst_self(with),
                        kind: p.kind.clone(),
                    })
                    .collect(),
                block: block.as_ref().map(|b| Box::new(b.subst_self(with))),
                ret: Box::new(ret.subst_self(with)),
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
            Ty::Array { elem } => Ty::Array { elem: Box::new(go(elem)) },
            Ty::Hash { key, value } => Ty::Hash { key: Box::new(go(key)), value: Box::new(go(value)) },
            Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(go).collect() },
            Ty::Union { variants } => Ty::Union { variants: variants.iter().map(go).collect() },
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Param { name: p.name.clone(), ty: go(&p.ty), kind: p.kind.clone() })
                    .collect(),
                block: block.as_ref().map(|b| Box::new(go(b))),
                ret: Box::new(go(ret)),
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
            Ty::Array { elem } => Ty::Array { elem: Box::new(elem.map_class_ids(f)) },
            Ty::Hash { key, value } => Ty::Hash {
                key: Box::new(key.map_class_ids(f)),
                value: Box::new(value.map_class_ids(f)),
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
                    .map(|p| Param { name: p.name.clone(), ty: p.ty.map_class_ids(f), kind: p.kind.clone() })
                    .collect(),
                block: block.as_ref().map(|b| Box::new(b.map_class_ids(f))),
                ret: Box::new(ret.map_class_ids(f)),
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
            Ty::Var { .. } | Ty::Untyped => true,
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
        matches!(self, Ty::Var { .. } | Ty::Untyped)
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
            Ty::Relation { of } => Some(Ty::Class { id: of.clone(), args: vec![] }),
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
            _ => Ty::Union { variants: kept },
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
                    0 => Ty::Untyped,
                    1 => kept.into_iter().next().unwrap(),
                    _ => Ty::Union { variants: kept },
                }
            }
            Ty::Var { .. } => Ty::Untyped,
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

    /// Of two `==` types, the one canonical order puts first. They can
    /// differ only in the field order of a record somewhere inside;
    /// picking by order instead of by arrival keeps a join's stored
    /// form independent of the order its inputs came in.
    pub(crate) fn canonical_min(a: Ty, b: Ty) -> Ty {
        if !a.holds_record() || cmp_ty(&b, &a) != std::cmp::Ordering::Less {
            a
        } else {
            b
        }
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
        Ty::Untyped => 17,
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
    pub fields: IndexMap<Symbol, Ty>,
    pub rest: Option<TyVar>,
}

impl Row {
    pub fn closed() -> Self {
        Row { fields: IndexMap::new(), rest: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Symbol,
    pub ty: Ty,
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
