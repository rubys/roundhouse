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
        // Untyped receiver: bind every block param to `Untyped` (the
        // gradual choice extends to the destructured params). Without
        // this, `untyped_hash.each { |k, v| ... }` would give k=Untyped
        // and v=Var since block_params_for returns a single-Untyped vec.
        if matches!(recv_ty, Some(Ty::Untyped)) {
            for name in params {
                new_ctx.local_bindings.insert(name.clone(), Ty::Untyped);
            }
            return new_ctx;
        }
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
                | "any?" | "all?" | "none?" | "one?"
                | "to_h" => Some(vec![(**elem).clone()]),
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
            // Generic class-registry lookup: when the method is
            // registered with a Ty::Fn whose `block` field is set, use
            // that as the block-param type. Lets framework stubs
            // declare what their block yields (form_with → FormBuilder,
            // ErrorCollection.each → Str) without hardcoding each one
            // in this match. Single-param yield only — multi-param
            // destructure isn't expressible in Ty::Fn::block today.
            Ty::Class { id, .. } => {
                let cls = self.classes().get(id)?;
                let sig = cls
                    .instance_methods
                    .get(method)
                    .or_else(|| cls.class_methods.get(method))?;
                if let Ty::Fn { block: Some(block_ty), .. } = sig {
                    Some(vec![(**block_ty).clone()])
                } else {
                    None
                }
            }
            // Union receivers (typically `T | Nil` from RBS optionals or
            // flow-sensitive ivar reads). Unwrap to the first concrete
            // container variant and recurse — `(Hash[K,V] | Nil).each
            // { |k, v| ... }` should yield `[K, V]` to the block, not
            // give up because the union confused dispatch. Skip Nil/Var
            // variants; they don't carry block-shape information.
            Ty::Union { variants } => {
                for v in variants {
                    if matches!(v, Ty::Nil | Ty::Var { .. }) {
                        continue;
                    }
                    if let Some(params) = self.block_params_for(Some(v), method) {
                        return Some(params);
                    }
                }
                None
            }
            // RBS-declared `untyped` receiver: a method call like
            // `untyped.each { |x| ... }` passes through with the block
            // param also typed `Untyped`, propagating the gradual choice
            // through the block body. Without this case the block param
            // would type as `Var` (inference gap), which is the wrong
            // signal — the gradual escape was authored, not inferred.
            // We don't know how many params the block takes (the
            // receiver type doesn't tell us); return a single-Untyped
            // shape, which covers the common `each { |x| }` case. Block
            // bodies that destructure with `|k, v|` will see `v` typed
            // as Untyped (right answer) but `k` will be missing
            // (analyzer fallback to Var); that residual is acceptable
            // — the caller has signed out of typing here.
            Ty::Untyped => Some(vec![Ty::Untyped]),
            _ => None,
        }
    }

    /// Walk the resolved method's signature and flip a trailing
    /// `kwargs: true` Hash to `kwargs: false` when the last param is
    /// positional (Required/Optional) with `Ty::Hash` type. Ruby's
    /// implicit kwargs-to-Hash collection doesn't survive into Crystal/
    /// strict targets — the IR has to commit one way or the other.
    /// Methods declared with `Keyword`/`KeywordRest` last params stay
    /// kwargs (the bare named-args call shape); methods declared with
    /// `opts = {}` (positional Hash) get the rewrite.
    pub(super) fn normalize_trailing_kwargs(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &mut [crate::expr::Expr],
    ) {
        use crate::expr::ExprNode;
        use crate::ty::ParamKind;
        let Some(last) = args.last_mut() else { return };
        let ExprNode::Hash { kwargs, .. } = &mut *last.node else { return };
        if !*kwargs {
            return;
        }
        let Some(Ty::Class { id, .. }) = recv_ty else { return };
        let mut current_id: Option<&ClassId> = Some(id);
        let mut seen = 0usize;
        // For `Class.new(…)` calls, the actual signature lives on
        // `initialize` (Ruby/Crystal auto-generate `new` to forward
        // to `initialize`). Look up under `initialize` instead so
        // the kwargs-flip fires for typed `def initialize(opts =
        // {})` model constructors.
        let lookup_name = if method.as_str() == "new" {
            Symbol::from("initialize")
        } else {
            method.clone()
        };
        let sig: Option<&Ty> = loop {
            let Some(cid) = current_id else { break None };
            seen += 1;
            if seen > 32 {
                break None;
            }
            let Some(cls) = self.classes().get(cid) else { break None };
            if let Some(s) = cls
                .instance_methods
                .get(&lookup_name)
                .or_else(|| cls.class_methods.get(&lookup_name))
            {
                break Some(s);
            }
            current_id = cls.parent.as_ref();
        };
        let Some(Ty::Fn { params, .. }) = sig else { return };
        let Some(last_param) = params.last() else { return };
        let last_kind_positional = matches!(
            last_param.kind,
            ParamKind::Required | ParamKind::Optional
        );
        let last_ty_is_hash = matches!(last_param.ty, Ty::Hash { .. });
        if last_kind_positional && last_ty_is_hash {
            *kwargs = false;
        }
    }

    pub(super) fn dispatch(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block_ret: Option<&Ty>,
        args: &[crate::expr::Expr],
    ) -> Ty {
        // `obj.class` is receiver-aware: our type system flattens the
        // class object and instances onto the same `Ty::Class { id }`,
        // so `instance_of_Base.class` returns `Ty::Class { Base }`
        // (not the generic `Ty::Class { Class }`). Keeps the type
        // available for chained dispatch like `self.class.table_name`.
        // For non-class receivers (`1.class`, `"x".class`) we still
        // hand back generic `Class` since the per-primitive metaclass
        // isn't represented in the registry.
        if method.as_str() == "class" {
            return match recv_ty {
                Some(Ty::Class { id, args }) => Ty::Class {
                    id: id.clone(),
                    args: args.clone(),
                },
                _ => Ty::Class {
                    id: ClassId(Symbol::from("Class")),
                    args: vec![],
                },
            };
        }
        // `freeze` / `itself` are receiver-identity: they return the
        // receiver unchanged. Receiver-aware (so they can't sit in the
        // receiver-agnostic `universal_method` table) and resolved before
        // the per-type tables so they work on every type — most
        // importantly the `CONST = {…}.freeze` idiom, where the trailing
        // `.freeze` must preserve the literal's Hash/Array/Range type for
        // the constant registry. With a known receiver, hand it back; with
        // none, fall through to `unknown()` like any other receiver-less
        // call.
        if matches!(method.as_str(), "freeze" | "itself") {
            if let Some(ty) = recv_ty {
                return ty.clone();
            }
        }
        // `recv.send(:m, …)` / `public_send` / `__send__` — Ruby's
        // reflective dispatch. With a LITERAL symbol/string argument
        // it's just a renamed call: dispatch the named method on the
        // receiver (tier 1 — `self.send(:title)` → the `title` method's
        // return). With a dynamic argument it can land on any of the
        // receiver's methods, so bound it by the union of the receiver
        // class's instance-method return types (tier 2), which absorbs
        // to `Untyped` when any is gradual. Either way it resolves —
        // never "no known method `send`". (The argument set is often a
        // literal array iterated by a block, e.g. `as_json`; tightening
        // the dynamic case to that enumerated set is a tier-3 follow-up.)
        if matches!(method.as_str(), "send" | "public_send" | "__send__") {
            if let Some(first) = args.first() {
                let literal_name = match &*first.node {
                    ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
                        Some(value.clone())
                    }
                    ExprNode::Lit { value: crate::expr::Literal::Str { value } } => {
                        Some(Symbol::from(value.as_str()))
                    }
                    _ => None,
                };
                if let Some(name) = literal_name {
                    return self.dispatch(recv_ty, &name, block_ret, &args[1..]);
                }
            }
            return match recv_ty {
                Some(t) => self.receiver_method_return_union(t),
                None => Ty::Untyped,
            };
        }
        // Universal Ruby methods — available on every object regardless
        // of receiver type. Resolved first so `nil?`, `is_a?`, etc.
        // don't fall through to per-type method tables that would miss.
        if let Some(ty) = universal_method(method) {
            return ty;
        }
        match recv_ty {
            None => unknown(),
            // RBS-declared gradual receiver. Method dispatch on
            // `Untyped` returns `Untyped` — the gradual choice
            // propagates unconditionally through the IR. Author-signed
            // opt-out, distinct from `Var` (inference gap, returns
            // `unknown()`).
            Some(Ty::Untyped) => Ty::Untyped,
            Some(Ty::Class { id, args }) => {
                // `Range` is modeled as `Ty::Class { id: "Range", args:
                // [elem] }` (see the body-typer's `ExprNode::Range` arm),
                // not a dedicated `Ty` variant, so its methods live in a
                // small table keyed off the element type rather than the
                // class registry. Covers the `CONST = (a..b).freeze` idiom
                // (`SCORE_RANGE_TO_HIDE.include?(score)`, `.first`) and
                // any other range value flowing through dispatch.
                if id.0.as_str() == "Range" {
                    if let Some(ty) = range_method(method, args.first()) {
                        return ty;
                    }
                }
                // Walk the parent chain so inherited methods resolve:
                // `Article.last` looks up `last` on Article → Application
                // Record → ActiveRecord::Base (where the RBS-declared
                // signature lives). Without the walk, lookups on the
                // immediate class miss inherited surface and return
                // `Ty::Var`, which downstream strict-target emit can't
                // reason about (no auto-`.not_nil!` on nilable-class
                // returns, etc.). Loop guard caps depth at 32 to match
                // `normalize_trailing_kwargs` (same shape, same cap).
                let mut current_id: Option<&ClassId> = Some(id);
                let mut depth = 0usize;
                while let Some(cid) = current_id {
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    let Some(cls) = self.classes().get(cid) else {
                        break;
                    };
                    if let Some(ty) = cls.class_methods.get(method) {
                        return unwrap_fn_ret(ty);
                    }
                    if let Some(ty) = cls.instance_methods.get(method) {
                        return unwrap_fn_ret(ty);
                    }
                    // Mixed-in modules (`include IntervalHelper`)
                    // contribute their instance methods to this class.
                    // Checked after the class's own methods, before the
                    // parent — Ruby's ancestor order puts an included
                    // module between the class and its superclass.
                    for module_id in &cls.includes {
                        if let Some(ty) = self.lookup_in_module(module_id, method) {
                            return ty;
                        }
                    }
                    current_id = cls.parent.as_ref();
                }
                let _ = args; // already shadowed below for new-call shortcut
                // Every class in Ruby responds to `.new`, returning an
                // instance of itself. Serve this universally — covers
                // unregistered classes (user-defined helpers) without
                // requiring the class to
                // appear in the catalog. Explicit catalog registrations
                // still win because they're checked above.
                //
                // Built-in containers map to their parameterized IR
                // type so subsequent `[]` / `[]=` / `each` etc. dispatch
                // through hash_method / array_method instead of falling
                // back to the (no-op) class-method table. Element types
                // start as Var; usage narrows them via flow-typing.
                if method.as_str() == "new" {
                    match id.0.as_str() {
                        "Hash" => {
                            return Ty::Hash {
                                key: Box::new(unknown()),
                                value: Box::new(unknown()),
                            };
                        }
                        "Array" => {
                            return Ty::Array { elem: Box::new(unknown()) };
                        }
                        _ => {}
                    }
                    return Ty::Class { id: id.clone(), args: args.clone() };
                }
                // Module/Class introspection built-ins — fall through
                // when no user-defined method shadows them. `name` on
                // a class returns the class's name as String;
                // `superclass`, `ancestors` are class-introspection
                // returning a Class / Array<Class> respectively.
                //
                // `clone` and `dup` are universally available on every
                // Ruby Object and return an instance of the same
                // class — `Article.new.clone` is still an `Article`.
                // Without this arm the lookup falls through to Var,
                // which masks downstream coercion (e.g. rust2's
                // setter-arg Borrow path keys on `recv: Ty::Class`
                // to wrap String→&str at `instance.clone().set_body
                // (row.body())` sites).
                match method.as_str() {
                    "name" => return Ty::Str,
                    "clone" | "dup" => return Ty::Class { id: id.clone(), args: args.clone() },
                    "superclass" => return Ty::Class {
                        id: ClassId(Symbol::from("Class")),
                        args: vec![],
                    },
                    "ancestors" => return Ty::Array {
                        elem: Box::new(Ty::Class {
                            id: ClassId(Symbol::from("Class")),
                            args: vec![],
                        }),
                    },
                    _ => {}
                }
                // Time stdlib subset — `Time.now.utc.iso8601` is the
                // canonical timestamp chain in `fill_timestamps`. We
                // only track the methods this corpus actually calls;
                // grow as new uses surface.
                if id.0.as_str() == "Time" {
                    if let Some(ty) = time_method(method) {
                        return ty;
                    }
                }
                // `Rails.env` returns an `ActiveSupport::StringInquirer`
                // — a String subclass whose `method_missing` answers any
                // `<word>?` (`development?`/`production?`/`staging?`) as
                // Bool. Model it exactly: a trailing-`?` method is a Bool
                // inquiry; everything else dispatches as a String
                // (`==`/interpolation/`upcase`/`to_sym` all work).
                if id.0.as_str() == "ActiveSupport::StringInquirer" {
                    if method.as_str().ends_with('?') {
                        return Ty::Bool;
                    }
                    return str_method(method);
                }
                // Base64 stdlib — `Base64.strict_encode64(JSON.generate(x))`
                // appears in turbo_stream_from. All Base64 module-level
                // encoders/decoders return String.
                if id.0.as_str() == "Base64" {
                    match method.as_str() {
                        "encode64" | "decode64"
                        | "strict_encode64" | "strict_decode64"
                        | "urlsafe_encode64" | "urlsafe_decode64" => return Ty::Str,
                        _ => {}
                    }
                }
                // JSON stdlib — `JSON.generate` and `JSON.dump` return
                // String; `JSON.parse` / `JSON.load` return parsed
                // structure (untyped — the body is genuinely
                // polymorphic). `pretty_generate` is also String.
                if id.0.as_str() == "JSON" {
                    match method.as_str() {
                        "generate" | "dump" | "pretty_generate" | "fast_generate" => {
                            return Ty::Str
                        }
                        "parse" | "load" => return Ty::Untyped,
                        _ => {}
                    }
                }
                // Regexp instance methods — `pattern.match?(s)`,
                // `pattern.match(s)`, `pattern =~ s` are the common
                // matchers. `match?` returns Bool; `match` returns
                // MatchData (or nil). `source` returns the pattern
                // String.
                if id.0.as_str() == "Regexp" {
                    match method.as_str() {
                        "match?" | "===" => return Ty::Bool,
                        "source" | "to_s" | "inspect" => return Ty::Str,
                        "options" | "casefold?" => return Ty::Int,
                        _ => {}
                    }
                }
                unknown()
            }
            Some(Ty::Array { elem }) => {
                let elem: &Ty = elem;
                // A relation delegates scope/builder calls to its element
                // model, so `user.comments.active` and `Story.where(..).hottest`
                // chain: any class method that returns a relation
                // (`Array[Self]`) — named scopes plus the query builders —
                // resolves on the relation and re-returns the relation.
                if let Ty::Class { id, .. } = elem {
                    if let Some(cls) = self.classes().get(id) {
                        if let Some(scope_ret @ Ty::Array { .. }) =
                            cls.class_methods.get(method)
                        {
                            return scope_ret.clone();
                        }
                    }
                }
                array_method(method, elem, block_ret)
            }
            Some(Ty::Hash { key, value }) => hash_method(method, key, value, block_ret),
            Some(Ty::Record { row }) => record_method(method, row, args),
            Some(Ty::Str) => str_method(method),
            Some(Ty::Sym) => sym_method(method),
            Some(Ty::Int) => int_method(method),
            Some(Ty::Float) => float_method(method),
            Some(Ty::Bool) => bool_method(method),
            // Union dispatch: try each concrete (non-Nil, non-Var) variant
            // and union the resolved results. Covers the common
            // `T | Nil` pattern (`find_by`, `params[:k]`, `.find` on
            // relation) where the method is valid on `T` and the Nil case
            // is handled elsewhere at run time.
            Some(Ty::Union { variants }) => {
                // Gradual absorption: any `Untyped` variant in the
                // union absorbs the dispatch — the result is `Untyped`.
                // Mirrors TypeScript's `any | T → any` semantics.
                if variants.iter().any(|v| matches!(v, Ty::Untyped)) {
                    return Ty::Untyped;
                }
                let mut resolved: Vec<Ty> = Vec::new();
                for v in variants {
                    if matches!(v, Ty::Nil | Ty::Var { .. }) {
                        continue;
                    }
                    let r = self.dispatch(Some(v), method, block_ret, args);
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
            // Receiver type is a `Var` (inference gap) or otherwise
            // unmodeled. Ruby's `to_*` conversions have a fixed return
            // type regardless of receiver, so even when we couldn't
            // type the receiver, `rows.to_h` is a Hash and `x.to_s` is
            // a String. Falling back to these (gradual element types)
            // resolves the read instead of leaving it `Var`.
            _ => conversion_fallback(method).unwrap_or_else(unknown),
        }
    }

    /// Resolve `method` against a mixed-in module's registered methods,
    /// chasing the module's own `include`s transitively. Returns the
    /// call-site result type (return type unwrapped). `Module`s carry
    /// their instance methods in the same registry slot classes use, so
    /// this is the class lookup minus the parent walk. A `seen` set
    /// guards the pathological `module A; include B; end; module B;
    /// include A; end` cycle.
    fn lookup_in_module(&self, module_id: &ClassId, method: &Symbol) -> Option<Ty> {
        let mut stack = vec![module_id.clone()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(m) = self.classes().get(&id) else { continue };
            if let Some(ty) = m.instance_methods.get(method) {
                return Some(unwrap_fn_ret(ty));
            }
            if let Some(ty) = m.class_methods.get(method) {
                return Some(unwrap_fn_ret(ty));
            }
            stack.extend(m.includes.iter().cloned());
        }
        None
    }

    /// The bound on a dynamic `recv.send(x)` (non-literal `x`): the
    /// union of every instance-method return type reachable on the
    /// receiver class — own methods plus parents plus mixed-in modules.
    /// A reflective dispatch can land on any of them, so this is the
    /// tightest sound bound from the receiver type alone. If any of
    /// those returns is `Untyped` (the gradual fallback most models
    /// carry on at least one method), the union absorbs to `Untyped` —
    /// the honest type for an opaque dynamic call. A non-class receiver
    /// (Var / primitive) carries no method table, so → `Untyped`.
    fn receiver_method_return_union(&self, recv_ty: &Ty) -> Ty {
        let Ty::Class { id, .. } = recv_ty else {
            return Ty::Untyped;
        };
        let mut rets: Vec<Ty> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = vec![id.clone()];
        while let Some(cid) = stack.pop() {
            if !seen.insert(cid.clone()) {
                continue;
            }
            let Some(cls) = self.classes().get(&cid) else { continue };
            for ty in cls.instance_methods.values() {
                let r = unwrap_fn_ret(ty);
                // A single gradual method makes the dynamic union
                // gradual — bail early with the absorbing type.
                if matches!(r, Ty::Untyped) {
                    return Ty::Untyped;
                }
                if !matches!(r, Ty::Var { .. } | Ty::Bottom) {
                    rets.push(r);
                }
            }
            if let Some(p) = &cls.parent {
                stack.push(p.clone());
            }
            stack.extend(cls.includes.iter().cloned());
        }
        if rets.is_empty() {
            Ty::Untyped
        } else {
            union_many(rets)
        }
    }
}

/// Canonical return type of a universal Ruby conversion method, used
/// as a last resort when the receiver type is unknown. NOT placed in
/// `universal_method` (which is consulted before per-type dispatch) so
/// the precise per-type versions — `array.to_h → Hash[K, V]` keyed off
/// the block, `array.to_a → Array[elem]` — still win when the receiver
/// IS typed. Element/value types are `Untyped` (gradual) here since
/// there's no receiver shape to derive them from.
fn conversion_fallback(method: &Symbol) -> Option<Ty> {
    Some(match method.as_str() {
        "to_h" => Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(Ty::Untyped),
        },
        "to_a" | "to_ary" => Ty::Array { elem: Box::new(Ty::Untyped) },
        "to_s" | "to_str" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "to_sym" => Ty::Sym,
        _ => return None,
    })
}

// Primitive method tables --------------------------------------------
//
// One function per receiver-type-kind. Each maps a method name to
// its return type. Entries grow as the type system gains coverage
// of Ruby's standard library; mining `functions_spec.rb` in the
// ruby2js codebase for additional translations is the ongoing work.

/// Methods on a `Range` value (`Ty::Class { id: "Range", args: [elem] }`).
/// `elem` is the bound type (`Int` for `(1..10)`); `None` when the range
/// is unparameterized (beginless+endless). Returns `None` for a method
/// this table doesn't model, so dispatch falls through to the generic
/// class handling. Range is enumerable, so collection-ish accessors
/// return the element type; bounds/predicates return their fixed types.
pub(super) fn range_method(method: &Symbol, elem: Option<&Ty>) -> Option<Ty> {
    let elem_ty = || elem.cloned().unwrap_or_else(unknown);
    let ty = match method.as_str() {
        // Endpoint / single-element accessors yield the bound type.
        "first" | "last" | "min" | "max" | "begin" | "end" => elem_ty(),
        // Membership / shape predicates.
        "include?" | "member?" | "cover?" | "===" | "exclude_end?" => Ty::Bool,
        "size" | "count" | "sum" => Ty::Int,
        "to_a" | "to_ary" | "entries" => Ty::Array { elem: Box::new(elem_ty()) },
        // `step` / `each` return the receiver range for chaining.
        "step" | "each" => Ty::Class {
            id: ClassId(Symbol::from("Range")),
            args: elem.cloned().into_iter().collect(),
        },
        _ => return None,
    };
    Some(ty)
}

/// Methods on a `Time` value — modeled as `Ty::Class { id: "Time" }`
/// (like `Range`), not a `Ty` variant. AR datetime columns now type as
/// `Time` (see `ingest::model::ty_of_column`), so this is the surface a
/// column read like `story.created_at.strftime(...)` dispatches
/// against. Class and instance flatten onto the same `Class { Time }`,
/// so the class-side constructors (`Time.now`) live here too. Date /
/// DateTime columns also map to `Time` (its method surface is a
/// superset for everything the corpus calls); a dedicated `date_method`
/// can split them out if a Date-only method ever surfaces. Returns
/// `None` for unmodeled methods so dispatch falls through to the
/// parent-chain walk.
pub(super) fn time_method(method: &Symbol) -> Option<Ty> {
    let time = || Ty::Class {
        id: ClassId(Symbol::from("Time")),
        args: vec![],
    };
    let ty = match method.as_str() {
        // Constructors, coercions, and Time-returning transforms.
        "now" | "current" | "utc" | "local" | "at" | "today"
        | "to_time" | "in_time_zone" | "localtime" | "getlocal" | "getutc"
        | "beginning_of_day" | "end_of_day" | "beginning_of_hour" | "end_of_hour"
        | "beginning_of_week" | "end_of_week" | "beginning_of_month" | "end_of_month"
        | "beginning_of_year" | "end_of_year" | "midnight" | "noon"
        | "change" | "advance" | "ago" | "since" | "from_now"
        | "round" | "floor" | "ceil" | "to_date" | "to_datetime" => time(),
        // `Time - x` is `Time` for a Duration arg but a Float for a
        // Time arg — the receiver-only dispatch can't disambiguate, so
        // gradual `Untyped` (the chains read `.before?`/`/ 60`/`> 1.minute`
        // off the result, all of which absorb Untyped).
        "+" | "-" => Ty::Untyped,
        // String renderings.
        "iso8601" | "rfc2822" | "rfc3339" | "to_s" | "to_fs" | "to_formatted_s"
        | "strftime" | "httpdate" | "ctime" | "asctime" | "inspect" | "zone" => Ty::Str,
        // Integer components / epoch seconds / spaceship.
        "to_i" | "tv_sec" | "tv_usec" | "tv_nsec" | "year" | "month" | "mon"
        | "day" | "mday" | "hour" | "min" | "sec" | "usec" | "nsec"
        | "wday" | "yday" | "<=>" => Ty::Int,
        "to_f" => Ty::Float,
        // Predicates / comparisons that read as method calls.
        // `==`/`!=` are handled by `universal_method` (checked before
        // this arm); the ordered comparisons aren't, so type them here:
        // `created_at >= cutoff` → Bool.
        "<" | ">" | "<=" | ">=" | "between?"
        | "after?" | "before?" | "past?" | "future?" | "today?"
        | "monday?" | "tuesday?" | "wednesday?" | "thursday?" | "friday?"
        | "saturday?" | "sunday?" | "on_weekend?" | "on_weekday?" => Ty::Bool,
        _ => return None,
    };
    Some(ty)
}

/// Is this array element type a model relation's element — a single
/// model class, or a union of model classes (a relation threaded
/// through a helper that several models share)? Used to gate the
/// ActiveRecord relation-builder surface in `array_method`.
fn is_model_relation_elem(elem: &Ty) -> bool {
    match elem {
        Ty::Class { .. } => true,
        Ty::Union { variants } => {
            !variants.is_empty()
                && variants.iter().all(|v| matches!(v, Ty::Class { .. }))
        }
        _ => false,
    }
}

pub(super) fn array_method(method: &Symbol, elem: &Ty, block_ret: Option<&Ty>) -> Ty {
    // AR-specific dispatches go FIRST so they win over the generic
    // array methods that share a name (`find` on a relation raises, so
    // it returns Class; on a plain Array it returns `Union<elem, Nil>`).
    // A relation's element is a model class — or, for a helper that
    // takes relations of several models (`period(query)` called with
    // both `Story…` and `Comment…`), a union of model classes. Both
    // admit the relation-builder surface.
    if is_model_relation_elem(elem) {
        match method.as_str() {
            // Relation chain methods preserve Array<Self>.
            "where" | "order" | "limit" | "offset" | "includes" | "preload"
            | "joins" | "left_outer_joins" | "distinct" | "group" | "having"
            | "references" | "eager_load" | "readonly" | "reorder"
            | "rewhere" | "merge" | "extending" | "unscope"
            | "not" | "or" | "and" | "none" | "load" | "reload" | "reselect" => {
                return Ty::Array { elem: Box::new(elem.clone()) };
            }
            // `relation.model` is the element model class. With a union
            // element it's ambiguous, so fall back to the gradual
            // escape (the common use is `query.model.table_name`).
            "model" => {
                return match elem {
                    Ty::Class { .. } => elem.clone(),
                    _ => Ty::Untyped,
                };
            }
            // CollectionProxy constructors / first-or-X return an element.
            "build" | "create" | "create!" | "find" | "find!" | "find_by!"
            | "first!" | "last!" | "take!" | "sole" | "sole!"
            | "first_or_initialize" | "first_or_create" | "first_or_create!"
            | "find_or_initialize_by" | "find_or_create_by" | "find_or_create_by!" => {
                return elem.clone();
            }
            // `ids` projects the primary keys; `arel` escapes to raw SQL.
            "ids" => return Ty::Array { elem: Box::new(Ty::Int) },
            "arel" => return Ty::Untyped,
            // `find_by` / `take` on a relation return Element | Nil
            // (same as a class call). Already covered by AR_CATALOG
            // for class receivers; cover the Array<Model> shape here.
            "find_by" | "take" => {
                return Ty::Union {
                    variants: vec![elem.clone(), Ty::Nil],
                };
            }
            // `find_each` / `find_in_batches` yield the elem but
            // return the receiver-relation for chaining.
            "find_each" | "find_in_batches" | "in_batches" => {
                return Ty::Array { elem: Box::new(elem.clone()) };
            }
            // `pluck(*cols)` returns Array of the column values; we
            // can't tell the column type from method/elem alone, so
            // produce `Array<Untyped>` (gradual escape — emitters
            // handle the cast at the call site if needed).
            "pluck" | "pick" => {
                return Ty::Array { elem: Box::new(Ty::Untyped) };
            }
            // `count` / `sum` / `average` on a relation return Int
            // (count) or Numeric (sum/avg). Approximate as Int; the
            // sum/avg float case is rare in controller code.
            "count" | "sum" | "average" | "minimum" | "maximum" => {
                return Ty::Int;
            }
            // `exists?` / `any?` / `none?` predicates.
            "exists?" => return Ty::Bool,
            // `update_all` / `delete_all` / `destroy_all` return
            // affected-row count (Int) or affected records.
            "update_all" | "delete_all" => return Ty::Int,
            "destroy_all" => return Ty::Array { elem: Box::new(elem.clone()) },
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
        // `delete(x)` returns the deleted element or nil.
        "delete" | "delete_at" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "pop" | "shift" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "dup" | "clone" => Ty::Array { elem: Box::new(elem.clone()) },
        // Array `+` (concat) and `-` (set difference) preserve Array[elem].
        // `<<` mutates in place and returns self (the array). `concat` /
        // `push` / `unshift` / `prepend` / `append` likewise return the
        // modified array.
        "+" | "-" | "<<" | "concat" | "push" | "unshift" | "prepend" | "append" => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        // Array `*` with an Int is array repetition (preserves Array[elem]);
        // with a Str it's `.join(sep)`, returning Str. The body-typer's
        // dispatch hands us the method name but not argument types, so
        // we can't distinguish here — the emitter's classifier handles
        // that branch using the operand `.ty` annotations. Returning
        // Array[elem] is the safe default (join→Str case is rare and the
        // result rarely chains into further array methods).
        "*" => Ty::Array { elem: Box::new(elem.clone()) },
        "any?" | "all?" | "none?" | "one?" | "empty?" | "include?" => Ty::Bool,
        "find" | "detect" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // Enumerable extrema return an element or nil (empty collection).
        "max" | "min" | "max_by" | "min_by" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // In-place / index-yielding transforms return the array itself.
        "each_with_index" | "keep_if" | "delete_if" | "select!" | "reject!" | "sort!"
        | "uniq!" | "compact!" | "reverse!" => Ty::Array { elem: Box::new(elem.clone()) },
        // `group_by`/`index_by` (ActiveSupport) force evaluation to a Hash.
        "group_by" => Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(Ty::Array { elem: Box::new(elem.clone()) }),
        },
        "index_by" => Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(elem.clone()),
        },
        "tally" => Ty::Hash { key: Box::new(elem.clone()), value: Box::new(Ty::Int) },
        // Fold/accumulate — result type depends on the block/seed (untracked).
        "inject" | "reduce" | "each_with_object" => Ty::Untyped,
        "to_sentence" => Ty::Str,
        // `Array#to_h { |elem| [k, v] }` — block returns a [k, v]
        // tuple; result is Hash<k, v>. We approximate as Hash<elem, elem>
        // when the block's tuple types aren't tracked at this layer;
        // refine when fixture demands richer tuple-element typing.
        "to_h" => match block_ret {
            Some(Ty::Tuple { elems }) if elems.len() == 2 => Ty::Hash {
                key: Box::new(elems[0].clone()),
                value: Box::new(elems[1].clone()),
            },
            Some(Ty::Array { elem: inner }) => Ty::Hash {
                key: Box::new((**inner).clone()),
                value: Box::new((**inner).clone()),
            },
            _ => Ty::Hash {
                key: Box::new(elem.clone()),
                value: Box::new(unknown()),
            },
        },
        "to_a" => Ty::Array { elem: Box::new(elem.clone()) },
        "join" => Ty::Str,
        _ => unknown(),
    }
}

/// Method dispatch for `Ty::Record` receivers — fixed-shape rows
/// (RBS record literals like `{action: Symbol, controller: Symbol,
/// path_params: Hash[String, String]}`). Bracket access with a
/// known Symbol/String literal key picks the matching field's type;
/// `length`/`size`/`empty?` work generically. Falls back through to
/// `hash_method` (treating the row as `Hash[Symbol|String, V_union]`)
/// for everything else, so dynamic-key access still types via the
/// value-union approximation.
pub(super) fn record_method(
    method: &Symbol,
    row: &crate::ty::Row,
    args: &[crate::expr::Expr],
) -> Ty {
    match method.as_str() {
        "[]" if args.len() == 1 => {
            // Literal-key bracket access → the field's exact type.
            // Non-literal keys fall through to the value-union form.
            if let crate::expr::ExprNode::Lit { value } = &*args[0].node {
                let key_str = match value {
                    crate::expr::Literal::Sym { value } => Some(value.clone()),
                    crate::expr::Literal::Str { value } => Some(Symbol::from(value.as_str())),
                    _ => None,
                };
                if let Some(k) = key_str {
                    if let Some(field_ty) = row.fields.get(&k) {
                        return field_ty.clone();
                    }
                }
            }
            // Unknown key → union of all field types + Nil.
            let mut variants: Vec<Ty> = row.fields.values().cloned().collect();
            variants.push(Ty::Nil);
            Ty::Union { variants }
        }
        "length" | "size" | "count" => Ty::Int,
        "empty?" | "any?" => Ty::Bool,
        "keys" => Ty::Array { elem: Box::new(Ty::Sym) },
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
        // `h[k] = v` returns the assigned value in Ruby, but here we
        // can't tell the argument's type from just the receiver's
        // generic Value — and the result is rarely chained. Return
        // Nil to keep the expression's type known (avoids a false
        // "unresolved" diagnostic when the hash's Value itself is a
        // type variable).
        "[]=" | "store" => Ty::Nil,
        // `delete(k)` returns the removed value, or nil if not found.
        "delete" => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        "clear" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        "to_a" => Ty::Array {
            elem: Box::new(Ty::Tuple { elems: vec![key.clone(), value.clone()] }),
        },
        "dup" | "clone" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        // Predicate-form indexing tested by `key?` / `value?`.
        "value?" | "has_value?" | "member?" => Ty::Bool,
        // `each` and similar return the receiver hash for chaining.
        "each" | "each_pair" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        "length" | "size" | "count" => Ty::Int,
        "values" => Ty::Array { elem: Box::new(value.clone()) },
        "empty?" | "any?" | "none?" | "key?" | "has_key?" | "include?" => Ty::Bool,
        "keys" => Ty::Array { elem: Box::new(key.clone()) },
        // `Hash#fetch(k, default)` returns the value at k or `default`
        // when missing. The default can be any value (including nil)
        // — Ruby's idiomatic `hash.fetch(k, nil)` produces
        // `Union<value, Nil>`. Conservatively widen to that shape so
        // emit can decide between `.get().cloned()` (Option<V>) and
        // `.get().cloned().unwrap_or(default)` (V) at the call site.
        // The per-target emit's fetch bridge already returns
        // Option-shaped Rust expressions for the nil-default case;
        // typing as Union<V, Nil> here matches that contract.
        "fetch" => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        "merge" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        // `Hash#to_h` is identity (returns self when called without a
        // block; with a block, transforms entries — same shape).
        // Common in controller bodies: `params.expect(...).to_h` to
        // strip the strong-params wrapper.
        "to_h" => Ty::Hash {
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
        // Rails strong-params: `params.expect(:id)` returns the
        // coerced value at that key. `params.require(:category)` and
        // `params.permit(...)` return a `Parameters`-shaped sub-Hash
        // that the caller typically chains further (`.permit(...)`,
        // `.except(...)`, `.to_h`). Return the receiver's Hash type
        // so chained calls resolve through hash_method instead of
        // bottoming out at the value's type. `expect` keeps its
        // value-type return since it's the terminal form in the
        // current Rails 8 idiom (`params.expect(article: [...])` →
        // the permitted hash).
        "expect" => value.clone(),
        "require" | "permit" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        _ => unknown(),
    }
}

pub(super) fn str_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "length" | "size" | "bytesize" => Ty::Int,
        "upcase" | "downcase" | "strip" | "chomp" | "chop" | "reverse" | "to_s"
        | "capitalize" | "swapcase" | "squeeze" | "dup" | "clone"
        | "tr" | "tr_s" | "delete" | "gsub" | "sub" | "lstrip" | "rstrip"
        | "succ" | "next" | "swapcase!" | "+@" | "-@" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "to_sym" | "intern" => Ty::Sym,
        // Case-insensitive comparison: `casecmp` returns -1/0/1 (Int),
        // `casecmp?` returns Bool.
        "casecmp" => Ty::Int,
        "casecmp?" => Ty::Bool,
        "chars" | "lines" | "split" | "bytes" | "scan" => Ty::Array { elem: Box::new(Ty::Str) },
        "empty?" | "blank?" | "present?" | "include?" | "start_with?"
        | "end_with?" | "match?" => Ty::Bool,
        // `String#match(regex)` returns MatchData or nil; we don't
        // model MatchData structurally so propagate Untyped (the
        // value is typically chained as `m[1]` which on Untyped
        // continues to flow gradually). `match` is also the regex
        // form of `=~` — same return shape.
        "match" => Ty::Untyped,
        // String slicing — `s[0, 4]`, `s[1..]`, `s[/regex/]` all
        // return String? (nil if out-of-range). Keep as Str for
        // simplicity; the nil-or-Str distinction can refine later.
        "[]" | "slice" => Ty::Str,
        // Operators. `+` concats; `<<` mutates in place but still returns self.
        // `*` is repetition ("a" * 3); `%` is sprintf (returns Str). Comparisons
        // uniformly return Bool.
        "+" | "<<" | "*" | "%" | "concat" => Ty::Str,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        // ActiveSupport String extensions. Pluralization/inflection
        // methods all return Str; comparison-style return Bool. Match
        // the surface of `ActiveSupport::Inflector` that real Rails
        // code reaches for in views and helpers.
        "pluralize" | "singularize" | "camelize" | "camelcase"
        | "underscore" | "dasherize" | "titleize" | "titlecase"
        | "humanize" | "demodulize" | "deconstantize" | "classify"
        | "tableize" | "foreign_key" | "parameterize" | "truncate"
        | "squish" | "remove" | "indent" | "strip_heredoc"
        | "html_safe" | "to_query" | "to_param" => Ty::Str,
        "constantize" | "safe_constantize" => unknown(),
        // ActiveSupport boolean predicates (Object#blank? is universal
        // and lives there; String#starts_with? / ends_with? are
        // ActiveSupport's underscore-style aliases of start_with? /
        // end_with?).
        "starts_with?" | "ends_with?" | "html_safe?"
        | "acts_like?" | "in?" => Ty::Bool,
        _ => unknown(),
    }
}

pub(super) fn sym_method(method: &Symbol) -> Ty {
    // Universal methods (`==`, `!=`, `to_s`, `inspect`, `class`, …)
    // resolve in `universal_method` before this is reached. Cover only
    // Sym-specific shapes here.
    match method.as_str() {
        "to_sym" => Ty::Sym,
        "length" | "size" => Ty::Int,
        "upcase" | "downcase" | "capitalize" | "swapcase" => Ty::Sym,
        "empty?" => Ty::Bool,
        "<=>" | "<" | ">" | "<=" | ">=" => Ty::Bool,
        _ => unknown(),
    }
}

pub(super) fn int_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" => Ty::Str,
        "to_i" | "abs" | "succ" | "pred" => Ty::Int,
        // Integer rounding is identity-typed: `n.ceil` / `n.floor` /
        // `n.round` / `n.truncate` with no digits arg return an Integer
        // (`(count / per_page).ceil` → page count). Float has these too
        // (line below) — Int needs its own entry or the chain bottoms
        // out at Var.
        "round" | "ceil" | "floor" | "truncate" => Ty::Int,
        // Unary minus/plus: Ruby desugars `-n` to `n.-@`. Int stays Int.
        "-@" | "+@" => Ty::Int,
        "to_f" => Ty::Float,
        "zero?" | "positive?" | "negative?" | "even?" | "odd?" => Ty::Bool,
        // Arithmetic: Int op Int → Int (we approximate Int/Float mixing here;
        // refine when a fixture demands it).
        "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" | "<<" | ">>" => Ty::Int,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        // Bit access (`flags[0]`) returns the bit as Int; `times` returns
        // the receiver (Int) — `n.times { }` evaluates to `n`.
        "[]" | "times" => Ty::Int,
        // ActiveSupport byte-size helpers — like the duration helpers,
        // they yield a Numeric-ish value we don't model structurally.
        "bytes" | "kilobytes" | "megabytes" | "gigabytes" | "terabytes"
        | "petabytes" | "exabytes" => Ty::Untyped,
        // ActiveSupport Numeric duration helpers — `1.day`, `2.hours`,
        // `30.minutes`, etc. Each returns an ActiveSupport::Duration
        // instance; we don't model that structurally so propagate
        // gradual escape via Untyped (Duration supports arithmetic
        // with Time/Date that flows through Untyped chains).
        "second" | "seconds" | "minute" | "minutes" | "hour" | "hours"
        | "day" | "days" | "week" | "weeks" | "fortnight" | "fortnights"
        | "month" | "months" | "year" | "years" => Ty::Untyped,
        // `ago` / `from_now` / `since` / `until` produce a Time-ish
        // value; same propagation rationale.
        "ago" | "from_now" | "since" | "until" => Ty::Untyped,
        // Common Int formatters from ActiveSupport.
        "ordinalize" | "ordinal" => Ty::Str,
        _ => unknown(),
    }
}

pub(super) fn float_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" | "inspect" => Ty::Str,
        // No-arg rounding returns Int (the common shape); with a digits
        // arg it returns Float, but we don't see args here — Int is the
        // safer default for the bare call.
        "to_i" | "to_int" | "round" | "ceil" | "floor" | "truncate" => Ty::Int,
        "to_f" | "abs" => Ty::Float,
        // Unary minus/plus: `-x` desugars to `x.-@`. Float stays Float.
        "-@" | "+@" => Ty::Float,
        "zero?" | "positive?" | "negative?" | "nan?" | "finite?" | "infinite?" => Ty::Bool,
        // Float arithmetic stays Float (Float op Int is also Float).
        "+" | "-" | "*" | "/" | "%" | "**" => Ty::Float,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        _ => unknown(),
    }
}

/// Unwrap a method's stored type to the call-site result type. Two
/// registration styles coexist: the catalog stores return types
/// directly (`Article.find: Ty::Class("Article")`), while
/// `parse_app_signatures` stores full function types
/// (`Ty::Fn { ret: ..., .. }`). Dispatch wants the return-type form
/// in both cases.
fn unwrap_fn_ret(ty: &Ty) -> Ty {
    match ty {
        Ty::Fn { ret, .. } => (**ret).clone(),
        other => other.clone(),
    }
}

/// Methods available on every Ruby object. Resolved before per-type
/// dispatch so receiver type doesn't matter — `nil?` on a String, an
/// Int, a user class, even Nil itself all return Bool.
pub(super) fn universal_method(method: &Symbol) -> Option<Ty> {
    match method.as_str() {
        // Type predicates.
        "nil?" | "is_a?" | "kind_of?" | "instance_of?" | "respond_to?"
        | "frozen?" | "tainted?" | "untrusted?" => Some(Ty::Bool),
        // Value equality / comparison operators.
        "==" | "!=" | "eql?" | "equal?" => Some(Ty::Bool),
        // Boolean negation — Ruby's `!x` desugars to `x.!()` and is
        // also written as bare `!cond` (Send recv=None, method="!").
        // Universally returns Bool regardless of receiver.
        "!" => Some(Ty::Bool),
        // `class` is receiver-aware and handled in `dispatch` itself
        // (preserves `Ty::Class { id }` so chained `obj.class.foo`
        // resolves against `id`'s registry entry).
        "hash" | "object_id" => Some(Ty::Int),
        "inspect" | "to_s" => Some(Ty::Str),
        // `raise` and `throw` are divergent — control transfers, the
        // call doesn't return a value. Surface them universally so a
        // bare `raise X, msg` Send (recv=None, method="raise") in any
        // method body resolves to a known type instead of falling
        // through dispatch to `Ty::Var`. Returning `Ty::Nil` here
        // matches `ExprNode::Raise`'s analyzer arm and is harmless
        // for callers (raise's "result" is never observed at run
        // time). Without this, methods that end with `raise ...`
        // harvest as `Ty::Var` and the dispatch registry never
        // learns their declared return type from the RBS contract.
        "raise" | "throw" => Some(Ty::Bottom),
        // ActiveSupport's universal `try` / `try!` — call a method if
        // the receiver responds, else nil. Return type is opaque
        // (depends on the dispatched method); `Ty::Untyped` propagates
        // the gradual choice rather than bottoming out at Var.
        // Recognized universally because it's a Kernel-style addition
        // that applies to every object regardless of receiver type.
        "try" | "try!" => Some(Ty::Untyped),
        // Object#tap returns the receiver itself; the block's return
        // is ignored. Receiver-aware in spirit but `dispatch` already
        // handles the receiver outside of this universal table — we
        // return Untyped here as a no-worse-than-Var fallback that
        // doesn't pretend to know more than it does.
        "tap" | "itself" => Some(Ty::Untyped),
        // `Hash#dig` / `Array#dig` / `Object#dig` walks a nested
        // structure by keys/indices. Receiver-aware dispatch would
        // need the full structural shape; in practice it's used at
        // the boundary with deeply-nested untyped data (params,
        // JSON), where Untyped is the honest answer.
        "dig" => Some(Ty::Untyped),
        // `presence` and `present?` are ActiveSupport's
        // blank-aware predicates. `presence` returns the receiver or
        // nil; we don't statically distinguish, so Untyped is the
        // gradual answer. `present?` / `blank?` are universally Bool.
        "present?" | "blank?" => Some(Ty::Bool),
        "presence" => Some(Ty::Untyped),
        _ => None,
    }
}

pub(super) fn bool_method(method: &Symbol) -> Ty {
    match method.as_str() {
        // Unary `!` and bitwise/logical operators all produce Bool.
        "!" | "&" | "|" | "^" => Ty::Bool,
        "==" | "!=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        "to_s" => Ty::Str,
        "inspect" => Ty::Str,
        _ => unknown(),
    }
}
