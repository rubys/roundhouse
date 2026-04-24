//! Send handling for the body-typer.
//!
//! Everything the `ExprNode::Send` arm of `compute` needs beyond
//! `analyze_expr`'ing the receiver/args/block: dispatch against a
//! receiver's `ClassInfo` or the primitive method tables, seed block
//! parameters from the receiver-aware signature, and propagate block
//! return types back to the call's result type.
//!
//! The primitive method tables (`array_method`, `hash_method`,
//! `str_method`, `int_method`) are the main growth surface here —
//! every method from Ruby's core we want to type for a target lives
//! in one of them. This file owns that catalog.

use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::{BodyTyper, Ctx, union_many, unknown};

impl<'a> BodyTyper<'a> {
    /// Build the Ctx used to analyze a block passed to `recv.method(...) { |p1, p2| ... }`.
    /// Seeds the block's local_bindings with parameter types derived from the receiver
    /// and method (e.g. `array.each { |x| }` binds `x` to the array's element type).
    pub(super) fn block_ctx_for(
        &self,
        outer: &Ctx,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block: &Expr,
    ) -> Ctx {
        let mut new_ctx = outer.clone();
        let ExprNode::Lambda { params, .. } = &*block.node else {
            return new_ctx;
        };
        let Some(param_tys) = self.block_params_for(recv_ty, method) else {
            return new_ctx;
        };
        for (name, ty) in params.iter().zip(param_tys.iter()) {
            new_ctx.local_bindings.insert(name.clone(), ty.clone());
        }
        new_ctx
    }

    /// Per-param types a block yields, given the receiver type and method.
    /// `None` means "no binding info available" — params stay unknown.
    pub(super) fn block_params_for(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
    ) -> Option<Vec<Ty>> {
        let recv_ty = recv_ty?;
        match recv_ty {
            Ty::Array { elem } => match method.as_str() {
                "each" | "map" | "collect" | "flat_map" | "collect_concat"
                | "select" | "filter" | "reject"
                | "find" | "detect" | "sort_by" | "group_by" | "min_by" | "max_by"
                | "any?" | "all?" | "none?" | "one?" => Some(vec![(**elem).clone()]),
                "each_with_index" => Some(vec![(**elem).clone(), Ty::Int]),
                _ => None,
            },
            Ty::Hash { key, value } => match method.as_str() {
                "each" | "each_pair" | "map" | "collect"
                | "flat_map" | "collect_concat"
                | "select" | "filter" | "reject"
                | "any?" | "all?" | "none?" => {
                    Some(vec![(**key).clone(), (**value).clone()])
                }
                // `transform_values { |v| ... }` — block receives just the value.
                "transform_values" => Some(vec![(**value).clone()]),
                // `transform_keys { |k| ... }` — block receives just the key.
                "transform_keys" => Some(vec![(**key).clone()]),
                _ => None,
            },
            // ActiveModel::Errors iteration yields an Error to the block.
            Ty::Class { id, .. } if id.0.as_str() == "ActiveModel::Errors" => {
                match method.as_str() {
                    "each" | "map" | "collect" | "select" | "filter" | "reject"
                    | "any?" | "all?" | "none?" => Some(vec![Ty::Class {
                        id: ClassId(Symbol::from("ActiveModel::Error")),
                        args: vec![],
                    }]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn dispatch(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block_ret: Option<&Ty>,
    ) -> Ty {
        match recv_ty {
            None => unknown(),
            Some(Ty::Class { id, .. }) => {
                if let Some(cls) = self.classes().get(id) {
                    if let Some(ty) = cls.class_methods.get(method) {
                        return ty.clone();
                    }
                    if let Some(ty) = cls.instance_methods.get(method) {
                        return ty.clone();
                    }
                }
                unknown()
            }
            Some(Ty::Array { elem }) => array_method(method, elem, block_ret),
            Some(Ty::Hash { key, value }) => hash_method(method, key, value, block_ret),
            Some(Ty::Str) => str_method(method),
            Some(Ty::Int) => int_method(method),
            // Union dispatch: try each concrete (non-Nil, non-Var) variant
            // and union the resolved results. Covers the common
            // `T | Nil` pattern (`find_by`, `params[:k]`, `.find` on
            // relation) where the method is valid on `T` and the Nil case
            // is handled elsewhere at run time.
            Some(Ty::Union { variants }) => {
                let mut resolved: Vec<Ty> = Vec::new();
                for v in variants {
                    if matches!(v, Ty::Nil | Ty::Var { .. }) {
                        continue;
                    }
                    let r = self.dispatch(Some(v), method, block_ret);
                    if !matches!(r, Ty::Var { .. }) {
                        resolved.push(r);
                    }
                }
                match resolved.len() {
                    0 => unknown(),
                    1 => resolved.into_iter().next().unwrap(),
                    _ => union_many(resolved),
                }
            }
            _ => unknown(),
        }
    }
}

// Primitive method tables --------------------------------------------
//
// One function per receiver-type-kind. Each maps a method name to
// its return type. Entries grow as the type system gains coverage
// of Ruby's standard library; mining `functions_spec.rb` in the
// ruby2js codebase for additional translations is the ongoing work.

pub(super) fn array_method(method: &Symbol, elem: &Ty, block_ret: Option<&Ty>) -> Ty {
    // AR-specific dispatches go FIRST so they win over the generic
    // array methods that share a name (`find` on a relation raises, so
    // it returns Class; on a plain Array it returns `Union<elem, Nil>`).
    if matches!(elem, Ty::Class { .. }) {
        match method.as_str() {
            // Relation chain methods preserve Array<Self>.
            "where" | "order" | "limit" | "offset" | "includes" | "preload"
            | "joins" | "distinct" | "group" | "having" => {
                return Ty::Array { elem: Box::new(elem.clone()) };
            }
            // CollectionProxy constructors return an element instance.
            "build" | "create" | "create!" | "find" | "find!" => {
                return elem.clone();
            }
            _ => {}
        }
    }
    // Block-returning transformations: output element type comes from
    // the block body when available (populated by the body-typer),
    // otherwise falls back to the input element type.
    let transformed_elem = || block_ret.cloned().unwrap_or_else(|| elem.clone());
    match method.as_str() {
        "length" | "size" | "count" => Ty::Int,
        "first" | "last" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "[]" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // `map` / `collect` produce Array of the block's return type.
        "map" | "collect" => Ty::Array { elem: Box::new(transformed_elem()) },
        // `flat_map` expects the block to return an Array, flattens by one.
        "flat_map" | "collect_concat" => match block_ret {
            Some(Ty::Array { elem: inner }) => Ty::Array { elem: inner.clone() },
            _ => Ty::Array { elem: Box::new(elem.clone()) },
        },
        // `each`, predicates, and shape-preserving transforms keep elem.
        "each" | "select" | "filter" | "reject"
        | "sort" | "sort_by" | "reverse" | "compact" | "flatten" | "uniq" => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        // Array `+` (concat) and `-` (set difference) preserve Array[elem].
        "+" | "-" => Ty::Array { elem: Box::new(elem.clone()) },
        "any?" | "all?" | "none?" | "one?" | "empty?" | "include?" => Ty::Bool,
        "find" | "detect" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        _ => unknown(),
    }
}

pub(super) fn hash_method(
    method: &Symbol,
    key: &Ty,
    value: &Ty,
    block_ret: Option<&Ty>,
) -> Ty {
    match method.as_str() {
        "[]" => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        "length" | "size" | "count" => Ty::Int,
        "values" => Ty::Array { elem: Box::new(value.clone()) },
        "empty?" | "any?" | "none?" | "key?" | "has_key?" | "include?" => Ty::Bool,
        "keys" => Ty::Array { elem: Box::new(key.clone()) },
        "fetch" => value.clone(),
        "merge" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        // `Hash#map` / `Hash#collect` returns an Array — block yields
        // (k, v) and returns some U; result is Array[U].
        "map" | "collect" => Ty::Array {
            elem: Box::new(block_ret.cloned().unwrap_or_else(unknown)),
        },
        // `transform_values { |v| ... }` → Hash[K, U].
        "transform_values" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(block_ret.cloned().unwrap_or_else(|| value.clone())),
        },
        // `transform_keys { |k| ... }` → Hash[U, V].
        "transform_keys" => Ty::Hash {
            key: Box::new(block_ret.cloned().unwrap_or_else(|| key.clone())),
            value: Box::new(value.clone()),
        },
        // Rails strong-params: `params.expect(:id)` and
        // `params.expect(k: [...])` both return the coerced value (a
        // scalar or a permitted-params-hash). Approximate both as the
        // value type for now; refine when a fixture forces a richer
        // return shape.
        "expect" | "require" | "permit" => value.clone(),
        _ => unknown(),
    }
}

pub(super) fn str_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "length" | "size" | "bytesize" => Ty::Int,
        "upcase" | "downcase" | "strip" | "chomp" | "chop" | "reverse" | "to_s"
        | "capitalize" | "swapcase" | "squeeze" | "dup" | "clone" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "empty?" | "blank?" | "present?" | "include?" | "start_with?"
        | "end_with?" | "match?" => Ty::Bool,
        // Operators. `+` concats; `<<` mutates in place but still returns self.
        // `*` is repetition ("a" * 3). Comparisons uniformly return Bool.
        "+" | "<<" | "*" | "concat" => Ty::Str,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        _ => unknown(),
    }
}

pub(super) fn int_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" => Ty::Str,
        "to_i" | "abs" | "succ" | "pred" => Ty::Int,
        "to_f" => Ty::Float,
        "zero?" | "positive?" | "negative?" | "even?" | "odd?" => Ty::Bool,
        // Arithmetic: Int op Int → Int (we approximate Int/Float mixing here;
        // refine when a fixture demands it).
        "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" | "<<" | ">>" => Ty::Int,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        _ => unknown(),
    }
}
