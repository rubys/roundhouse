// Not a runtime class: no target can subclass Hash, so the indifferent hash runs as a String-keyed Hash and `lower::indifferent_access` converts each call's keys and values at the site.

use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub const CLASS: &str = "ActiveSupport::HashWithIndifferentAccess";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    FirstKey,
    AllKeys,
    Write,
    Merge,
    ReverseMerge,
    Copy,
    ToHash,
    SymbolizeKeys,
    Plain,
}

pub fn shape(method: &str, args: &[Expr], has_block: bool) -> Option<Shape> {
    let n = args.len();
    let splat = |a: &Expr| matches!(&*a.node, ExprNode::Splat { .. });
    if args.iter().any(|a| matches!(&*a.node, ExprNode::KeywordSplat { .. }))
        || args.iter().any(splat) && !matches!(method, "values_at" | "slice" | "except" | "without")
    {
        return None;
    }
    let shape = match method {
        "[]" if n == 1 && !has_block => Shape::FirstKey,
        "fetch" if (1..=2).contains(&n) => Shape::FirstKey,
        "key?" | "has_key?" | "include?" | "member?" | "delete" if n == 1 && !has_block => Shape::FirstKey,
        "dig" if n >= 1 && !has_block => Shape::AllKeys,
        "values_at" | "slice" | "except" | "without" if !has_block => Shape::AllKeys,
        "[]=" | "store" if n == 2 && !has_block => Shape::Write,
        "merge" | "update" | "merge!" if n == 1 && !has_block => Shape::Merge,
        "reverse_merge" | "with_defaults" if n == 1 && !has_block => Shape::ReverseMerge,
        "dup" | "with_indifferent_access" | "stringify_keys" | "to_h" if n == 0 && !has_block => Shape::Copy,
        "to_hash" if n == 0 && !has_block => Shape::ToHash,
        "symbolize_keys" | "to_options" if n == 0 && !has_block => Shape::SymbolizeKeys,
        "freeze" | "deep_dup" | "compact" | "keys" | "values" | "size" | "length" | "empty?" | "to_a"
        | "to_json" | "as_json" | "to_query" | "to_param" | "invert" | "clear"
            if n == 0 && !has_block =>
        {
            Shape::Plain
        }
        "first" | "key" | "value?" | "has_value?" if n <= 1 && !has_block => Shape::Plain,
        "select" | "filter" | "reject" | "transform_values" | "select!" | "filter!" | "reject!"
        | "keep_if" | "delete_if" | "each" | "each_pair" | "each_key" | "each_value"
        | "map" | "collect" | "flat_map" | "filter_map" | "sum" | "min_by" | "max_by"
        | "sort_by" | "find" | "detect" | "each_with_index" | "any?" | "none?" | "all?" | "one?"
            if n == 0 && has_block =>
        {
            Shape::Plain
        }
        "each_with_object" | "reduce" | "inject" if n == 1 && has_block => Shape::Plain,
        "any?" | "count" if n == 0 && !has_block => Shape::Plain,
        "count" if n == 0 && has_block => Shape::Plain,
        // Not the String-keyed Hash table: its key-taking methods would miss a Symbol key.
        _ => return None,
    };
    Some(shape)
}

pub fn of(value: Ty) -> Ty {
    Ty::Class { id: ClassId(Symbol::from(CLASS)), args: vec![value] }
}

pub fn value_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Class { id, args } if id.0.as_str() == CLASS => Some(args.first().unwrap_or(&Ty::Untyped)),
        _ => None,
    }
}

fn indifferent_keys(key: &Ty) -> bool {
    match key {
        Ty::Str | Ty::Sym | Ty::Untyped | Ty::Var { .. } => true,
        Ty::Union { variants } => variants.iter().all(indifferent_keys),
        _ => false,
    }
}

pub fn converted(ty: &Ty) -> Ty {
    match ty {
        Ty::Hash { key, value } if indifferent_keys(key) => of(converted(value)),
        Ty::Hash { key, value } => Ty::Hash { key: key.clone(), value: Box::new(converted(value)) },
        Ty::Array { elem } => Ty::Array { elem: Box::new(converted(elem)) },
        Ty::Union { variants } => Ty::Union { variants: variants.iter().map(converted).collect() },
        other => other.clone(),
    }
}

pub fn plain(ty: &Ty) -> Ty {
    match ty {
        Ty::Class { id, args } if id.0.as_str() == CLASS => Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(plain(args.first().unwrap_or(&Ty::Untyped))),
        },
        Ty::Hash { key, value } => Ty::Hash { key: Box::new(plain(key)), value: Box::new(plain(value)) },
        Ty::Array { elem } => Ty::Array { elem: Box::new(plain(elem)) },
        Ty::Union { variants } => Ty::Union { variants: variants.iter().map(plain).collect() },
        Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(plain).collect() },
        Ty::Class { id, args } => Ty::Class { id: id.clone(), args: args.iter().map(plain).collect() },
        other => other.clone(),
    }
}

pub fn mentions(ty: &Ty) -> bool {
    match ty {
        Ty::Class { id, args } => id.0.as_str() == CLASS || args.iter().any(mentions),
        Ty::Hash { key, value } => mentions(key) || mentions(value),
        Ty::Array { elem } => mentions(elem),
        Ty::Union { variants } => variants.iter().any(mentions),
        Ty::Tuple { elems } => elems.iter().any(mentions),
        _ => false,
    }
}

pub fn from(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Untyped | Ty::Nil => Some(of(Ty::Untyped)),
        Ty::Hash { key, value } if indifferent_keys(key) => Some(of(converted(value))),
        Ty::Class { .. } => value_of(ty).map(|v| of(v.clone())),
        Ty::Union { variants } => {
            let mut values: Vec<Ty> = Vec::new();
            for v in variants {
                if *v == Ty::Nil {
                    continue;
                }
                let t = from(v)?;
                values.push(value_of(&t)?.clone());
            }
            match values.len() {
                0 => Some(of(Ty::Untyped)),
                1 => Some(of(values.pop().unwrap())),
                _ => Some(of(Ty::Union { variants: values })),
            }
        }
        _ => None,
    }
}

fn merged_value(other: &Ty) -> Option<Ty> {
    from(other).and_then(|t| value_of(&t).cloned())
}

fn without_nil(ty: &Ty) -> Ty {
    match ty {
        Ty::Union { variants } => {
            let mut kept: Vec<Ty> = variants.iter().filter(|v| **v != Ty::Nil).cloned().collect();
            if kept.len() == 1 { kept.pop().unwrap() } else { Ty::Union { variants: kept } }
        }
        other => other.clone(),
    }
}

pub fn method_ty(
    value: &Ty,
    method: &str,
    args: &[Expr],
    block_ret: Option<&Ty>,
    has_block: bool,
    hash_answer: impl Fn(&str) -> Ty,
) -> Option<Ty> {
    let same = || of(value.clone());
    let ty = match shape(method, args, has_block)? {
        Shape::FirstKey | Shape::Write => hash_answer(method),
        Shape::AllKeys => match method {
            "slice" | "except" | "without" => same(),
            _ => hash_answer(method),
        },
        Shape::Merge | Shape::ReverseMerge => {
            let other = merged_value(args[0].ty.as_ref()?)?;
            of(super::body::union_of(value.clone(), other))
        }
        Shape::Copy if method == "to_h" => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(value.clone()) },
        Shape::Copy => same(),
        Shape::ToHash => plain(&Ty::Hash { key: Box::new(Ty::Str), value: Box::new(value.clone()) }),
        Shape::SymbolizeKeys => Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(plain(value)) },
        Shape::Plain => match method {
            "freeze" | "deep_dup" | "select" | "filter" | "reject" | "select!" | "filter!" | "reject!" | "keep_if"
            | "delete_if" | "clear" | "each" | "each_pair" | "each_key" | "each_value" => same(),
            "compact" => of(without_nil(value)),
            "transform_values" => of(block_ret.cloned().unwrap_or(Ty::Untyped)),
            _ => hash_answer(method),
        },
    };
    Some(ty)
}

pub fn reverse_merged(defaults: &Ty, hash: &Ty) -> Option<Ty> {
    let value = value_of(hash)?;
    if value_of(defaults).is_some() {
        return None;
    }
    let other = merged_value(defaults)?;
    Some(of(super::body::union_of(other, value.clone())))
}
