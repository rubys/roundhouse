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

use super::{BodyTyper, Ctx, union_many, union_of, unknown};

impl<'a> BodyTyper<'a> {
    /// `record[:column]` / `read_attribute(:column)` (and their writer
    /// twins) are schema-indexed APIs.  The generic ActiveRecord catalog
    /// can only describe their fallback shape, but a literal key and a
    /// concrete model receiver make the exact column type available.
    pub(super) fn column_attribute_access_ty(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
    ) -> Option<Ty> {
        let model = match recv_ty? {
            Ty::Class { id, .. } => id,
            _ => return None,
        };
        let key_arg = match method.as_str() {
            "[]" | "read_attribute" if args.len() == 1 => &args[0],
            "[]=" | "write_attribute" if args.len() == 2 => &args[0],
            _ => return None,
        };
        let methods = &self.classes().get(model)?.instance_methods;
        let entry = match &*key_arg.node {
            ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => methods.get(value),
            ExprNode::Lit { value: crate::expr::Literal::Str { value } } => {
                methods.get(&Symbol::from(value.as_str()))
            }
            _ => None,
        }?;
        // A reader declared as a method (an RBS or Sorbet signature, an
        // input object's argument) is its return type, not the method.
        Some(match entry {
            Ty::Fn { ret, .. } => (**ret).clone(),
            other => other.clone(),
        })
    }

    /// `pluck(:col)` / `pick(:col)` on a relation over a known model.
    ///
    /// The class-side registration in `analyze/mod.rs` types these
    /// `Array<Untyped>` because "column type unknowable from the name
    /// alone" — true of the METHOD, false of the CALL. Here the column
    /// is a Symbol literal and the model is known, so the schema
    /// answers: `sessions.pluck(:ip_address)` is `Array[Str]`, not an
    /// open var.
    ///
    /// That distinction is what `compact_blank` needs. `lower::blank`
    /// grounds ActiveSupport's blank family through the ELEMENT type
    /// and files residue when it cannot; campfire's
    /// `sessions.pluck(:ip_address).compact_blank.uniq` was the residue
    /// entry, and an untyped element is exactly the receiver every
    /// strict target cannot compile ([[feedback_types_are_performance_
    /// avoid_bags]]).
    ///
    /// MULTI-COLUMN `pluck(:a, :b)` is not claimed: Rails answers an
    /// Array of tuples, which is `Ty::Array<Ty::Array<...>>` only if
    /// the columns share a type. The registered `Array<Untyped>`
    /// stands for that form.
    fn column_projection(
        &self,
        model: &ClassId,
        method: &Symbol,
        args: &[Expr],
    ) -> Option<Ty> {
        if !matches!(method.as_str(), "pluck" | "pick") {
            return None;
        }
        let [arg] = args else { return None };
        let ExprNode::Lit { value: crate::expr::Literal::Sym { value: col } } = &*arg.node else {
            return None;
        };
        let cls = self.classes().get(model)?;
        let col_ty = cls.instance_methods.get(col)?.clone();
        Some(match method.as_str() {
            // `pick` is `pluck(...).first` — the column value, or nil
            // when the relation is empty.
            "pick" => Ty::Union { variants: vec![col_ty, Ty::Nil] },
            _ => Ty::Array { elem: Box::new(col_ty) },
        })
    }

    /// Rails' per-column dynamic finders (`find_by_<attr>`,
    /// `find_by_<attr>!`) are generated at runtime for any real column —
    /// the same shape `find_by`/`find_by!` already type, just spelled
    /// with the attribute baked into the method name instead of passed
    /// as a keyword. Column existence is checked against the model's
    /// actual schema table (`ClassInfo::has_schema_column` — the same
    /// check the arel lowerer's `normalize_dynamic_finder_send` makes,
    /// #558): an attribute no known column backs isn't a dynamic
    /// finder (and the pipeline has no runtime for inventing one), so
    /// it falls through to "no known method" rather than guessing. This
    /// runs only once the caller has already checked `class_methods`
    /// for the name (see the call site), so a real class method of this
    /// shape — a `has_secure_password` token finder, say — still wins.
    fn dynamic_finder_ty(&self, model: &ClassId, method: &Symbol) -> Option<Ty> {
        let (base, bang) = match method.as_str().strip_suffix('!') {
            Some(base) => (base, true),
            None => (method.as_str(), false),
        };
        let attr = base.strip_prefix("find_by_")?;
        if attr.is_empty() {
            return None;
        }
        let cls = self.classes().get(model)?;
        if !cls.has_schema_column(&Symbol::from(attr)) {
            return None;
        }
        let kind = if bang {
            crate::catalog::ReturnKind::SelfType
        } else {
            crate::catalog::ReturnKind::SelfOrNil
        };
        Some(crate::analyze::instantiate_return_kind(kind, model))
    }

    /// `rel.group(:col).count` — Rails' GROUPED count, a Hash of
    /// group-key => COUNT rather than the scalar Integer.
    ///
    /// The pipeline already handles this shape end to end:
    /// `lower::group_count` renames the terminal to `group_count`
    /// (splitting the name is what keeps both returns monomorphic — a
    /// `count` answering Integer-or-Hash is the polymorphic-API shape
    /// the runtime avoids), and `ActiveRecord::Relation#group_count`
    /// builds the `SELECT … GROUP BY` and hydrates the Hash. But that
    /// lowering runs on the POST-ANALYZE hook, so the typer still sees
    /// the source spelling `count` and the catalog answers `Int` —
    /// `.keys` on the result then read as a dispatch failure against a
    /// feature the pipeline fully supports (issue #75).
    ///
    /// The shape test mirrors `lower::group_count::rewrite` — zero-arg,
    /// block-less `count` whose receiver chain contains `group(...)`,
    /// walking the same refiners (`having`, `distinct`, `select`, …) —
    /// so the type this reports and the method the lowering actually
    /// emits cannot disagree. Widening one without the other is the
    /// failure mode to avoid: `sum`/`average`/`minimum`/`maximum`
    /// switch to a grouped Hash in Rails too, but no `group_sum`
    /// lowering or runtime method exists, so typing them here would
    /// trade a false error for a false PASS.
    ///
    /// The key type is the grouped COLUMN's, read off the schema the
    /// same way `column_projection` does — `group(:feed_id).count.keys`
    /// wants `Array[Int]`, not `Array[Untyped]`. Anything the schema
    /// cannot answer (a multi-column group, a String or expression
    /// argument, an unknown column) falls back to `Untyped`, matching
    /// `relation.rbs`'s `Hash[untyped, Integer]`.
    pub(super) fn grouped_count_ty(
        &self,
        recv: Option<&Expr>,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
        block: Option<&Expr>,
    ) -> Option<Ty> {
        if method.as_str() != "count" || !args.is_empty() || block.is_some() {
            return None;
        }
        let group_args = Self::group_args_in_count_chain(recv?)?;
        // The receiver of `count` is the `group(...)` result, so its
        // type names the model whose schema owns the grouped column.
        // Both relation representations reach here: an inline
        // `Model.where(...).group(...)` chain carries the Array shape,
        // a scope or association read carries `Ty::Relation`.
        let model = match recv_ty? {
            Ty::Relation { of } => of,
            Ty::Array { elem } => match &**elem {
                Ty::Class { id, .. } => id,
                _ => return None,
            },
            _ => return None,
        };
        let key = self.grouped_key_ty(model, group_args).unwrap_or(Ty::Untyped);
        Some(Ty::Hash { key: Box::new(key), value: Box::new(Ty::Int) })
    }

    fn group_args_in_count_chain(expr: &Expr) -> Option<&[Expr]> {
        crate::lower::group_count::group_in_relation_chain(expr).map(|g| g.group_args)
    }

    /// The column type a single-symbol `group(:col)` groups by, when the
    /// model's schema answers it. `None` for every other argument shape.
    fn grouped_key_ty(&self, model: &ClassId, args: &[Expr]) -> Option<Ty> {
        let [arg] = args else { return None };
        let ExprNode::Lit { value: crate::expr::Literal::Sym { value: col } } = &*arg.node else {
            return None;
        };
        self.classes().get(model)?.instance_methods.get(col).cloned()
    }

    /// Build the Ctx used to analyze a block passed to `recv.method(...) { |p1, p2| ... }`.
    /// Seeds the block's local_bindings with parameter types derived from the receiver
    /// and method (e.g. `array.each { |x| }` binds `x` to the array's element type).
    pub(super) fn block_ctx_for(
        &self,
        outer: &Ctx,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
        class_object_receiver: bool,
        block: &Expr,
    ) -> Ctx {
        let mut new_ctx = outer.clone();
        let ExprNode::Lambda { params, .. } = &*block.node else {
            return new_ctx;
        };
        for name in params { new_ctx.class_objects.remove(name); }
        super::forget_class_object_writes(block, &mut new_ctx);
        // `form_with model: product do |form|` / `form_for @product do
        // |f|`: the builder is parameterized by the record the form is
        // for, so `form.object` (and `form.object.errors`) answer it.
        if matches!(method.as_str(), "form_with" | "form_for" | "simple_form_for") {
            // `form_with ..., builder: CustomFormBuilder` yields THAT
            // builder, not the stock one. An app that adds field helpers
            // on a FormBuilder subclass (a very common Rails shape)
            // otherwise has every `f.my_field` go unresolved. The builder
            // is honoured even when the model's type is unknown — an
            // untyped `@record` (assigned through a gem roundhouse does
            // not model) must not cost the app its own builder methods.
            if let Some(name) = params.first() {
                let model = Self::form_model_ty(method, args);
                let builder = Self::form_builder_id(args);
                if model.is_some() || builder.is_some() {
                    let id = builder.unwrap_or_else(|| {
                        ClassId(Symbol::from("ActionView::Helpers::FormBuilder"))
                    });
                    new_ctx.local_bindings.insert(
                        name.clone(),
                        Ty::Class { id, args: model.into_iter().collect() },
                    );
                    return new_ctx;
                }
            }
        }
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
        let Some(param_tys) = self.block_params_for(recv_ty, method, class_object_receiver) else {
            return new_ctx;
        };
        for (name, ty) in params.iter().zip(param_tys.iter()) {
            new_ctx.local_bindings.insert(name.clone(), ty.clone());
        }
        new_ctx
    }

    /// Form options can still carry source keyword provenance during inference.
    /// Borrow their value without projecting or consuming the call argument.
    fn form_option<'e>(args: &'e [Expr], name: &str) -> Option<&'e Expr> {
        args.iter().find_map(|a| {
            let a = match &*a.node {
                ExprNode::KeywordSplat { value } => value,
                _ => a,
            };
            let ExprNode::Hash { entries, .. } = &*a.node else { return None };
            entries.iter().find_map(|(k, v)| match &*k.node {
                ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                    if value.as_str() == name =>
                {
                    Some(v)
                }
                _ => None,
            })
        })
    }

    /// The class named by a `builder:` keyword, when the call carries one
    /// and it is a plain constant. `::Foo` and `Foo` name the same class.
    fn form_builder_id(args: &[Expr]) -> Option<ClassId> {
        let expr = Self::form_option(args, "builder")?;
        let ExprNode::Const { path } = &*expr.node else { return None };
        let joined = path
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("::");
        (!joined.is_empty()).then(|| ClassId(Symbol::from(joined.as_str())))
    }

    /// The record a form helper is building for: `form_with`'s `model:`
/// kwarg (a record, or `[parent, child]` — the last element), or
/// `form_for`'s first positional argument. Nil is peeled: a
/// `Product?` still builds a form for a Product. `None` for URL forms.
    fn form_model_ty(method: &Symbol, args: &[Expr]) -> Option<Ty> {
    let model_expr = if method.as_str() == "form_with" {
        Self::form_option(args, "model")?
    } else {
        args.first().filter(|a| !matches!(&*a.node, ExprNode::Hash { .. }))?
    };
    let ty = match &*model_expr.node {
        // `[product, Subscriber.new]` — the form is for the last one.
        ExprNode::Array { elements, .. } => elements.last()?.ty.clone()?,
        _ => model_expr.ty.clone()?,
    };
    let peeled = super::narrowing::remove_nil(&ty);
    matches!(peeled, Ty::Class { .. }).then_some(peeled)
}

/// Per-param types a block yields, given the receiver type and method.
    /// `None` means "no binding info available" — params stay unknown.
    /// `class_object_receiver` picks class-method block contracts before
    /// instance ones, matching ordinary dispatch on a class/module object.
    pub(super) fn block_params_for(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        class_object_receiver: bool,
    ) -> Option<Vec<Ty>> {
        let recv_ty = recv_ty?;
        if matches!(recv_ty, Ty::Class { id, .. } if id.0.as_str() == PARAM_VALUE) {
            return match method.as_str() {
                "each_with_index" | "each_with_object" => Some(vec![param_value_ty(), Ty::Int]),
                "each" | "each_pair" | "map" | "collect" | "flat_map" | "filter_map" | "select"
                | "filter" | "reject" | "any?" | "all?" | "none?" | "count" | "find" | "detect"
                | "each_value" | "each_key" | "sort_by" | "group_by" | "partition" | "sum" => {
                    Some(vec![param_value_ty(), param_value_ty()])
                }
                _ => None,
            };
        }
        // `then` / `yield_self` / `tap` yield the RECEIVER itself, on
        // every type — Kernel methods, not container ones, so they are
        // answered before the shape match rather than repeated inside
        // each arm. campfire's `Opengraph::Location.new(url).then { |l|
        // l.read_html }` is the shape: without this the block parameter
        // is unbound and every read through it goes unresolved.
        if matches!(method.as_str(), "then" | "yield_self" | "tap") {
            return Some(vec![recv_ty.clone()]);
        }
        if let Ty::Tuple { elems } = recv_ty {
            let as_array = Ty::Array {
                elem: Box::new(elems.iter().cloned().reduce(union_of).unwrap_or(Ty::Untyped)),
            };
            return self.block_params_for(Some(&as_array), method, class_object_receiver);
        }
        match recv_ty {
            Ty::Str if method.as_str() == "bytes" => Some(vec![Ty::Int]),
            Ty::Array { elem } => match method.as_str() {
                "each" | "map" | "collect" | "flat_map" | "collect_concat"
                | "select" | "filter" | "reject"
                | "find" | "detect" | "sort_by" | "group_by" | "min_by" | "max_by"
                | "partition" | "sum" | "filter_map" | "map!" | "collect!"
                | "index" | "find_index"
                | "any?" | "all?" | "none?" | "one?"
                | "to_h" => Some(vec![(**elem).clone()]),
                "each_with_index" | "with_index" => Some(vec![(**elem).clone(), Ty::Int]),
                "sort_by!" | "select!" | "reject!" | "keep_if" | "delete_if" => Some(vec![(**elem).clone()]),
                _ => None,
            },
            Ty::Int => match method.as_str() {
                "times" | "upto" | "downto" | "step" => Some(vec![Ty::Int]),
                _ => None,
            },
            // A relation iterates its element model — same block
            // surface as `Array<of>`, so delegate to the Array arm
            // with the materialized element type.
            Ty::Relation { of } => {
                let as_array = Ty::Array {
                    elem: Box::new(Ty::Class { id: of.clone(), args: vec![] }),
                };
                self.block_params_for(Some(&as_array), method, class_object_receiver)
            }
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
            Ty::Class { id, .. } if id.0.as_str() == "CSV" && method.as_str() == "generate" => {
                Some(vec![recv_ty.clone()])
            }
            // `PTY.spawn(...) { |r, w, pid| ... }` yields the same three
            // values the non-block form answers as a Tuple.
            Ty::Class { id, .. }
                if class_object_receiver
                    && id.0.as_str() == "PTY"
                    && method.as_str() == "spawn" =>
            {
                let file = Ty::Class { id: ClassId(Symbol::from("File")), args: vec![] };
                Some(vec![file.clone(), file, Ty::Int])
            }
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
                // Walk the class + parent chain (and includes) for a
                // registered method whose `Ty::Fn` declares a block param,
                // so a block-yielding helper registered on a base class
                // binds the param for subclasses too (`respond_to` on
                // ApplicationController, `form_with` on ActionView::Base).
                // Mirrors the parent walk in result dispatch. A method
                // found without block info stops the walk (it shadows).
                let mut current = Some(id.clone());
                let mut depth = 0usize;
                while let Some(cid) = current {
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    let cls = self.classes().get(&cid)?;
                    // Own methods, then included modules — both ahead of
                    // the parent in Ruby's ancestor order.
                    for c in std::iter::once(cls)
                        .chain(cls.includes.iter().filter_map(|m| self.classes().get(m)))
                    {
                        // Prefer the receiver-side table. The fixpoint
                        // often seeds that side with a bare return
                        // (`Nil` / `Str` / …) while the RBS block
                        // contract still lives on the other side —
                        // take the other side's block-bearing Fn only
                        // then. A real receiver-side `Fn` without a
                        // block must not steal the opposite method's
                        // block (dispatch still picks the receiver
                        // side). Dual-name both-sides-with-block keeps
                        // the receiver side.
                        let (preferred, other) = if class_object_receiver {
                            (&c.class_methods, &c.instance_methods)
                        } else {
                            (&c.instance_methods, &c.class_methods)
                        };
                        let return_seed = |ty: &Ty| !matches!(ty, Ty::Fn { .. });
                        let sig = match (preferred.get(method), other.get(method)) {
                            (Some(s @ Ty::Fn { block: Some(_), .. }), _) => Some(s),
                            (pref, Some(s @ Ty::Fn { block: Some(_), .. }))
                                if pref.map_or(true, return_seed) =>
                            {
                                Some(s)
                            }
                            (Some(s), _) => Some(s),
                            (None, o) => o,
                        };
                        if let Some(sig) = sig {
                            // The block's yield may name the receiver
                            // (`{ (instance) -> void }`); substitute
                            // against the class the walk started from,
                            // as dispatch does.
                            let self_ty = Ty::Class { id: id.clone(), args: Vec::new() };
                            return match sig {
                                // A block that yields SEVERAL values
                                // names them in its own `Ty::Fn`
                                // params — `{ (String, String) -> bool }`
                                // for an authenticator yielding a
                                // username and a password. Spread them,
                                // or the second parameter binds nothing
                                // and everything read from it is
                                // untyped. A block of ONE parameter
                                // yields that parameter's type and a
                                // block of none yields nothing: the
                                // block's `Ty::Fn` is its signature,
                                // not the value it yields. (A stub
                                // that registers the yielded type
                                // itself has a non-`Fn` block and
                                // takes the last arm.)
                                Ty::Fn { block: Some(block_ty), .. } => match &**block_ty {
                                    Ty::Fn { params, .. } => Some(
                                        params
                                            .iter()
                                            .map(|p| p.ty.subst_self(&self_ty))
                                            .collect(),
                                    ),
                                    _ => Some(vec![block_ty.subst_self(&self_ty)]),
                                },
                                _ => None,
                            };
                        }
                    }
                    current = cls.parent.clone();
                }
                None
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
                    if let Some(params) =
                        self.block_params_for(Some(v), method, class_object_receiver)
                    {
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
    /// `room.memberships.grant_to(users)` — an association EXTENSION
    /// method, resolved from the receiver's SHAPE because its type
    /// cannot carry the answer: the association read is
    /// `Array<Membership>`, and `Membership` has no `grant_to` (nor
    /// does `Room`). Reading the two hops here is the same thing the
    /// emit-time flattening reads when it rewrites the call to
    /// `room.memberships_grant_to(users)`.
    ///
    /// The owner has to be a KNOWN class and the pair has to be
    /// registered, so this resolves declared extension methods and
    /// nothing else — `room.memberships.no_such_thing` still lands in
    /// the ledger.
    pub(super) fn assoc_extension_ty(
        &self,
        recv: Option<&crate::expr::Expr>,
        method: &Symbol,
    ) -> Option<Ty> {
        let ExprNode::Send { recv: Some(owner), method: assoc, block: None, .. } =
            &*recv?.node
        else {
            return None;
        };
        // Peel a nilable owner: `Room.first.memberships.grant_to(u)`
        // has a `Room | Nil` receiver, and the extension is the same
        // one either way — the nil case is a NoMethodError in Rails
        // too, reported (if at all) about `memberships`, not about the
        // extension.
        let id = match owner.ty.as_ref()? {
            Ty::Class { id, .. } => id,
            Ty::Union { variants } => variants.iter().find_map(|v| match v {
                Ty::Class { id, .. } => Some(id),
                _ => None,
            })?,
            _ => return None,
        };
        // Walk the parent chain: an STI subclass declares no
        // associations of its own, so campfire's `@room` — a
        // `Rooms::Closed` after `becomes!` — reaches `memberships`
        // and its extension block through `Room`.
        let key = (assoc.clone(), method.clone());
        let mut current = Some(id);
        for _ in 0..32 {
            let cls = self.classes().get(current?)?;
            if let Some(ty) = cls.assoc_extensions.get(&key) {
                return Some(ty.clone());
            }
            current = cls.parent.as_ref();
        }
        None
    }

    /// `message.boosts.loaded?` — Rails AssociationProxy spelling.
    ///
    /// The has_many reader types as `Array[Boost]`, which has no
    /// `loaded?`. Emit lowers the two-hop onto the synthesized
    /// `boosts_loaded?` Bool (`lower::assoc_loaded`), but `check` /
    /// LSP analyze without that pass and would otherwise residual the
    /// tip Campfire view. Mirror the rewrite's admission here: only
    /// when the owner actually has `<assoc>_loaded?` (has_many was
    /// synthesized) answer Bool — never catalog Relation `#loaded?`
    /// (invariant 6: no silent runtime gap).
    ///
    /// View locals are often `Untyped`; then fall back to the same
    /// unique-name rule `assoc_loaded` uses for non-model owners.
    pub(super) fn assoc_loaded_ty(
        &self,
        recv: Option<&crate::expr::Expr>,
        method: &Symbol,
    ) -> Option<Ty> {
        if method.as_str() != "loaded?" {
            return None;
        }
        let ExprNode::Send {
            recv: Some(owner),
            method: assoc,
            args,
            block: None,
            ..
        } = &*recv?.node
        else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let flat = Symbol::from(format!("{}_loaded?", assoc.as_str()));
        let has_flat = |id: &ClassId| -> bool {
            let mut current = Some(id);
            for _ in 0..32 {
                let Some(cid) = current else { return false };
                let Some(cls) = self.classes().get(cid) else { return false };
                if cls.instance_methods.contains_key(&flat) {
                    return true;
                }
                current = cls.parent.as_ref();
            }
            false
        };
        // Mirror `lower::assoc_loaded::owner_from_typed_recv`: a single
        // Class admits when it has `<assoc>_loaded?`; a Union admits
        // only when *every* class alternative does (no first-member
        // find_map — that would quiet check while emit still refuses).
        // Typed owners that miss the flat predicate return None — do
        // not fall through to the unique-name path (that is for
        // untyped view locals only).
        match owner.ty.as_ref() {
            Some(Ty::Class { id, .. }) => {
                return has_flat(id).then_some(Ty::Bool);
            }
            Some(Ty::Union { variants }) => {
                let ids: Vec<&ClassId> = variants
                    .iter()
                    .filter_map(|v| match v {
                        Ty::Class { id, .. } => Some(id),
                        _ => None,
                    })
                    .collect();
                return (!ids.is_empty() && ids.iter().all(|id| has_flat(id)))
                    .then_some(Ty::Bool);
            }
            _ => {}
        }
        // Untyped / missing owner type (view local): unique `<assoc>_loaded?`
        // across modeled classes, matching `lower::assoc_loaded`'s
        // unique-name path for views.
        let mut found = false;
        for cls in self.classes().values() {
            if cls.instance_methods.contains_key(&flat) {
                if found {
                    return None;
                }
                found = true;
            }
        }
        found.then_some(Ty::Bool)
    }

    pub(super) fn normalize_trailing_kwargs(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &mut [crate::expr::Expr],
    ) {
        use crate::expr::ExprNode;
        use crate::ty::ParamKind;
        // Producer provenance must survive until the source declaration
        // selects native forwarding or the ordinary lowering projection.
        if args.iter().any(|a| matches!(&*a.node, ExprNode::KeywordSplat { .. })) {
            return;
        }
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
        // The same substitution dispatch makes, for the same reason:
        // a signature that declares `(instance) -> bool` (`==`,
        // `<=>`) must present a concrete param type to the check
        // below, not a self type it cannot classify.
        let sig = sig.map(|s| s.subst_self(&Ty::Class { id: id.clone(), args: Vec::new() }));
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

    fn ancestor_defining_class_method(&self, of: &ClassId, method: &Symbol) -> Option<ClassId> {
        let own = self.classes().get(of)?;
        if own.class_methods.contains_key(method) {
            return None;
        }
        let mut current = own.parent.clone();
        for _ in 0..32 {
            let id = current?;
            let cls = self.classes().get(&id)?;
            if cls.table.is_some() && cls.class_methods.contains_key(method) {
                return Some(id);
            }
            current = cls.parent.clone();
        }
        None
    }

    fn defines_instance_method(&self, of: &ClassId, method: &Symbol) -> bool {
        let mut current = Some(of.clone());
        for _ in 0..32 {
            let Some(id) = current else { return false };
            let Some(cls) = self.classes().get(&id) else { return false };
            if cls.instance_methods.contains_key(method)
                || cls.includes.iter().any(|m| self.lookup_in_module(m, method).is_some())
            {
                return true;
            }
            current = cls.parent.clone();
        }
        false
    }

    pub(super) fn dispatch(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block_ret: Option<&Ty>,
        args: &[crate::expr::Expr],
    ) -> Ty {
        self.dispatch_on(recv_ty, method, block_ret, args, false)
    }

    // The class object and its instances are the same `Ty::Class`, so only the caller, which sees the receiver expression, can tell the side.
    pub(super) fn dispatch_on(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block_ret: Option<&Ty>,
        args: &[crate::expr::Expr],
        instance_receiver: bool,
    ) -> Ty {
        // `Parameters` is a Hash-shaped bag: what its own class does not
        // answer (`fetch`, `each`, `map`, `count`, ...) is the Hash
        // reading over Symbol -> param value. Done here,
        // not only where a `params` receiver is typed, so the Parameters
        // arm of an element read's union answers the same surface.
        if let Some(Ty::Class { id, .. }) = recv_ty {
            if id.0.as_str() == "ActionController::Parameters"
                && method.as_str() != "new"
                && !self.classes().get(id).is_some_and(|c| {
                    c.instance_methods.contains_key(method) || c.class_methods.contains_key(method)
                })
            {
                let as_hash = params_as_hash();
                return self.dispatch(Some(&as_hash), method, block_ret, args);
            }
        }
        // `self[:generated_at]` / `read_attribute(:generated_at)` read
        // the attribute TYPECAST, so `Time?` for a datetime column and
        // `BigDecimal?` for a decimal one -- the same value the
        // column's own reader answers. The catalog entry for `[]`
        // and `read_attribute` says `String`, the shape of the
        // storage text the lowered model deals in, which is not what
        // the source program reads: `self[:rate] || BigDecimal("1")`
        // was `String`, and `1 / rate` an "Integer / String" error.
        // Only a literal name that IS an attribute is answered; any
        // other key keeps the catalog's answer.
        if let (true, Some(Ty::Class { id, .. })) = (
            matches!(method.as_str(), "[]" | "read_attribute" | "_read_attribute"),
            recv_ty,
        ) {
            if let Some(ty) = self.attribute_by_name(id, args) {
                return ty;
            }
        }
        // A tuple (a method returning `[a, b]` of mixed types — see
        // `tuple_return_ty`) is still an Array at runtime: anything but
        // destructuring reads it as one, over the union of its slots.
        if let Some(Ty::Tuple { elems }) = recv_ty {
            let as_array = Ty::Array {
                elem: Box::new(elems.iter().cloned().reduce(union_of).unwrap_or(Ty::Untyped)),
            };
            return self.dispatch(Some(&as_array), method, block_ret, args);
        }
        // `Class[C]` — a method that returns the class object itself
        // (see `class_object_return_ty`). Dispatch reads it back as
        // `C`, the flattened type a bare `C` constant already has, so
        // a call on the returned class resolves exactly as one written
        // on the constant does.
        if let Some(Ty::Class { id, args: of }) = recv_ty {
            if id.0.as_str() == "Class" && of.len() == 1 {
                return self.dispatch(Some(&of[0]), method, block_ret, args);
            }
            if id.0.as_str() == PARAM_VALUE {
                return param_value_method(method, block_ret).unwrap_or_else(|| str_method(method));
            }
        }
        // `obj.class` is receiver-aware: our type system flattens the
        // class object and instances onto the same `Ty::Class { id }`,
        // so `instance_of_Base.class` returns `Ty::Class { Base }`
        // (not the generic `Ty::Class { Class }`). Keeps the type
        // available for chained dispatch like `self.class.table_name`.
        // For non-class receivers (`1.class`, `"x".class`) we still
        // hand back generic `Class` since the per-primitive metaclass
        // isn't represented in the registry.
        // `record.becomes!(Rooms::Closed)` — Rails' STI recast. The
        // answer is named by the ARGUMENT, not the receiver, which no
        // class table can express, so it is resolved here rather than
        // through the registry walk. `lower::sti_scope` rewrites the
        // call to `Rooms::Closed.becomes_from(record)` after analyze;
        // both spellings answer the same class, and without this one
        // campfire's `@room = @room.becomes!(Rooms::Closed)` made
        // `@room` shapeless for every read downstream of the filter
        // that ran it — four in the Closeds controller, two in Opens.
        if matches!(method.as_str(), "becomes" | "becomes!") && args.len() == 1 {
            if let ExprNode::Const { path } = &*args[0].node {
                return Ty::Class {
                    id: ClassId(Symbol::from(
                        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str(),
                    )),
                    args: vec![],
                };
            }
        }
        // `recv.try(:m, …)` — the method's answer, or nil when the
        // receiver does not respond. Precise when the registry knows
        // `m` on the receiver's class (walking its parents and mixins);
        // otherwise the gradual answer stands — silence about a method
        // is not a claim that the class lacks it.
        if matches!(method.as_str(), "try" | "try!") {
            if let (Some(Ty::Class { id, .. }), Some(first)) = (recv_ty, args.first()) {
                if let ExprNode::Lit { value: crate::expr::Literal::Sym { value: m } } = &*first.node {
                    let mut cur = Some(id.clone());
                    let mut depth = 0;
                    while let Some(cid) = cur {
                        let Some(cls) = self.classes().get(&cid) else { break };
                        if let Some(ty) = cls.instance_methods.get(m).or_else(|| cls.class_methods.get(m)) {
                            // Same substitution as dispatch, against the
                            // receiver's class: `try(:instance)` on a
                            // subclass answers the subclass.
                            let ty = ty.subst_self(&Ty::Class { id: id.clone(), args: Vec::new() });
                            return union_of(unwrap_fn_ret(&ty), Ty::Nil);
                        }
                        depth += 1;
                        if depth > 32 {
                            break;
                        }
                        cur = cls.parent.clone();
                    }
                }
            }
        }
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
        // `tap` is receiver-identity too — the block's value is
        // discarded and the receiver handed back — so it belongs with
        // `freeze`/`itself`, not in the receiver-agnostic
        // `universal_method` table (whose Untyped answer swallowed
        // `create!(…).tap { |room| … }` — the campfire idiom — into
        // the gradual escape). No-receiver falls through like the
        // pair above.
        if method.as_str() == "tap" {
            if let Some(ty) = recv_ty {
                return ty.clone();
            }
        }
        // `presence` hands the receiver back or nil, so on a typed
        // receiver it is `T?` — what `lower::blank` stamps on the
        // `blank? ? nil : r` it rewrites the site to. Resolved ahead of
        // the receiver-agnostic table, which answers `Untyped`.
        //
        // Not an Array: a `has_many` reader types as one while the
        // runtime answers a Relation, and `lower::enumerable_ext` reads
        // the untyped half of `rel.presence || [x]` as its sign that
        // the value may be either.
        if method.as_str() == "presence" {
            if let Some(ty) = recv_ty.filter(|ty| {
                !matches!(ty, Ty::Var { .. } | Ty::Array { .. } | Ty::Untyped)
            }) {
                return super::union_of(ty.clone(), Ty::Nil);
            }
        }
        // `.call` on a value TYPED as a function (an RBS `^() -> T`
        // parameter — `broadcast_render(blk)`'s `blk.call` is the
        // runtime site) answers the function's declared return. Only
        // a `Ty::Fn` receiver claims this; anything else keeps its
        // own `call` (a callable app class, an untyped proc).
        if method.as_str() == "call" {
            if let Some(Ty::Fn { ret, .. }) = recv_ty {
                return (**ret).clone();
            }
        }
        // `then` / `yield_self` are the receiver-identity pair's mirror:
        // where `tap` discards the block's value and hands the receiver
        // back, these hand the BLOCK's value back. Kernel methods, so
        // they apply to every receiver and have to be resolved here for
        // the same reason `freeze` is — ahead of the per-type tables,
        // which have no arm for a class the analyzer models only as a
        // name. Blockless (`then` returning an Enumerator) is not a
        // shape any corpus app writes; `Untyped` is the honest answer.
        if matches!(method.as_str(), "then" | "yield_self") && recv_ty.is_some() {
            return block_ret.cloned().unwrap_or(Ty::Untyped);
        }
        // `Model.transaction { … }` / `ActiveRecord::Base.transaction
        // { … }` returns its block's value (commit) — the registered
        // class-side entry says Untyped ("we don't statically track"),
        // but right here the block's return IS tracked, so hand it
        // back. Class receivers only: that is the AR idiom's shape,
        // and it keeps an app's own instance method named
        // `transaction` out of this arm. Campfire's
        // `Room.create_for` is the beneficiary — `transaction do
        // create!(…).tap { … } end` now answers the model instead of
        // dissolving the whole `find_or_create_for` chain to poly.
        // Only an INFORMATIVE block type is adopted: a `Var` block
        // (e.g. `transaction { @story.save }` over an unmodeled ivar)
        // falls through to the registered `Untyped`, which is the
        // gradual answer and not a dispatch failure.
        if let Some(t) = self.block_value_return(recv_ty, method, block_ret) {
            return t;
        }
        if method.as_str() == "transaction"
            && matches!(recv_ty, Some(Ty::Class { .. }))
        {
            if let Some(ret) = block_ret {
                if !matches!(ret, Ty::Var { .. }) {
                    return ret.clone();
                }
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
        // The call's arguments, under a name the `Ty::Class { id, args }`
        // pattern below does not shadow.
        let call_args = args;
        // Universal Ruby methods — available on every object regardless
        // of receiver type. Resolved first so `nil?`, `is_a?`, etc.
        // don't fall through to per-type method tables that would miss.
        if let Some(ty) = universal_method(method) {
            return ty;
        }
        // Captured before the match: the `Ty::Class { id, args }` arm
        // below shadows the call-args slice with the class's generic
        // type args, so the `format_db_time` intrinsic's arg-
        // optionality probe must read the real first argument here.
        let first_arg_is_plain_time =
            matches!(args.first().and_then(|a| a.ty.as_ref()), Some(Ty::Time));
        match recv_ty {
            None => unknown(),
            // RBS-declared gradual receiver. Method dispatch on
            // `Untyped` returns `Untyped` — the gradual choice
            // propagates through the IR. Author-signed opt-out,
            // distinct from `Var` (inference gap, returns `unknown()`).
            //
            // EXCEPT THE CONVERSIONS, which have a fixed return type
            // whatever the receiver is: `x.to_s` is a String even when
            // nothing is known about `x`. The `Var` arm below already
            // says so; saying it here too is what lets a chain RECOVER
            // from a gradual receiver instead of staying gradual to its
            // end. campfire's `stream_name.to_s.split(":", 2).second`
            // is the case that earned it — `stream_name` is an untyped
            // class-method param, so the whole chain absorbed, `second`
            // reached the emit as a dynamic dispatch, and on a target
            // with no `Array#second` it was a NoMethodError that killed
            // the process on the first cable subscribe.
            //
            // Signing out of typing for a receiver is not signing out
            // of Ruby's own guarantees about the answer.
            Some(Ty::Untyped) => conversion_fallback(method).unwrap_or(Ty::Untyped),
            Some(Ty::Class { id, args }) => {
                // `GlobalID::Locator.locate(…, only: Model)` /
                // `locate_signed(…, only: Model, for:)` — the literal
                // `only:` IS the finder, so the answer is that model
                // (nilable: a bad/mismatched gid is nil). Refined here
                // rather than in the registry: the registry cannot see
                // the call-site kwarg.
                if id.0.as_str() == "GlobalID::Locator"
                    && matches!(method.as_str(), "locate" | "locate_signed")
                {
                    if let Some(model) = locator_only_class(call_args) {
                        return Ty::Union {
                            variants: vec![
                                Ty::Class { id: model, args: vec![] },
                                Ty::Nil,
                            ],
                        };
                    }
                }
                if id.0.as_str() == "Date" {
                    if let Some(ty) = date_constructor(method, call_args) {
                        return ty;
                    }
                }
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
                // `ActiveSupport.parse_db_time(<stored text>)` — the
                // synthesized temporal-column reader intrinsic (see
                // `src/lower/model_to_library/schema.rs`). It parses
                // ISO-8601 storage text into a native `Time` (nilable: a
                // stored value can be absent). Not a real ActiveSupport
                // method — an internal lowering intrinsic each backend
                // renders natively (`parse_db_time` → `RhDateTime.parse`
                // etc.) — so it's resolved here rather than via the class
                // registry, which would leave it an unresolved `Ty::Var`.
                if id.0.as_str() == "ActiveSupport" && method.as_str() == "parse_db_time" {
                    return Ty::Union { variants: vec![Ty::Time, Ty::Nil] };
                }
                if id.0.as_str() == "ActiveSupport" && method.as_str() == "parse_db_date" {
                    return Ty::Union { variants: vec![Ty::Date, Ty::Nil] };
                }
                // The Date calendar intrinsics `time_calendar` lowers to.
                if id.0.as_str() == "ActiveSupport" {
                    match method.as_str() {
                        "date_current"
                        | "date_from_civil"
                        | "date_days_since"
                        | "date_days_ago"
                        | "date_yesterday"
                        | "date_tomorrow"
                        | "date_weeks_since"
                        | "date_weeks_ago"
                        | "date_months_since"
                        | "date_months_ago"
                        | "date_years_since"
                        | "date_years_ago"
                        | "date_beginning_of_week"
                        | "date_end_of_week"
                        | "date_next_week"
                        | "date_prev_week"
                        | "date_beginning_of_month"
                        | "date_end_of_month"
                        | "date_beginning_of_year"
                        | "date_end_of_year" => return Ty::Date,
                        "date_at_midnight"
                        | "date_beginning_of_day"
                        | "date_end_of_day" => return Ty::Time,
                        "date_today?"
                        | "date_yesterday?"
                        | "date_tomorrow?"
                        | "date_past?"
                        | "date_future?" => return Ty::Bool,
                        _ => {}
                    }
                }
                if id.0.as_str() == "ActiveSupport" && method.as_str() == "format_db_date" {
                    return if matches!(call_args.first().and_then(|a| a.ty.as_ref()), Some(Ty::Date)) {
                        Ty::Str
                    } else {
                        Ty::Union { variants: vec![Ty::Str, Ty::Nil] }
                    };
                }
                // `ActiveSupport.db_now` — the write-side sibling:
                // current UTC time in Rails' exact storage form
                // ("YYYY-MM-DD HH:MM:SS.ffffff"). Same internal-intrinsic
                // treatment; `fill_timestamps` stamps with it.
                if id.0.as_str() == "ActiveSupport" && method.as_str() == "db_now" {
                    return Ty::Str;
                }
                // `ActiveSupport.format_db_time(value)` — the write-side
                // normalize sibling: `Time` → the same storage text
                // `db_now` produces, propagating the argument's
                // optionality (a `Time|Nil` arg — nullable column —
                // yields `Str|Nil`; a plain `Time` yields `Str`, which
                // is what lets strict targets assign the result into a
                // NOT NULL column's non-optional storage field). The
                // synthesized public `<col>=` temporal writer
                // normalizes through it (`schema::synth_temporal_
                // writer`).
                if id.0.as_str() == "ActiveSupport" && method.as_str() == "format_db_time" {
                    return if first_arg_is_plain_time {
                        Ty::Str
                    } else {
                        Ty::Union { variants: vec![Ty::Str, Ty::Nil] }
                    };
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
                // `Model.first(n)` — the class-side spelling of the
                // counted terminal. Only where the class carries the AR
                // catalog's `first`/`last` (a model or a library class
                // registered with the same surface); any other class's
                // `first(x)` is its own method.
                if counted_first_last(method, call_args)
                    && self
                        .classes()
                        .get(id)
                        .is_some_and(|c| c.class_methods.contains_key(method))
                {
                    return Ty::Array {
                        elem: Box::new(Ty::Class { id: id.clone(), args: vec![] }),
                    };
                }
                // Class-side defs stay a fallback: a value typed as an instance may still be a class object (`@klass = Post`).
                let instance_side = instance_receiver && self.defines_instance_method(id, method);
                let mut current_id: Option<&ClassId> = Some(id);
                let mut depth = 0usize;
                // Set when the chain reaches a *named* superclass we don't
                // model (a gem/external class like `SVG::Graph::TimeSeries`).
                // Only meaningful once we've resolved the class itself
                // (`steps > 0`): a modeled class inheriting from an unmodeled
                // gem parent has a genuinely-unknown inherited surface, so an
                // unresolved method is a gradual escape (`Untyped`), not an
                // error. A wholly-unregistered receiver keeps erroring.
                // Jbuilder: `json.title "x"` writes a key and answers the
                // value; `json.author do … end` builds a nested object;
                // `json.array!` the array; the builder mutations
                // (`extract!`, `partial!`, `cache!`, `merge!`, `call`) nil.
                // The compiler interprets every one of these in
                // `lower::jbuilder_to_library`; the analyzer's answer is
                // the value Jbuilder itself returns.
                if id.0.as_str() == "Jbuilder" {
                    return match method.as_str() {
                        "array!" => Ty::Array { elem: Box::new(Ty::Untyped) },
                        "target!" => Ty::Str,
                        "attributes!" => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) },
                        "cache!" | "cache_if!" | "cache_root!" => block_ret.cloned().unwrap_or(Ty::Nil),
                        "extract!" | "partial!" | "merge!" | "ignore_nil!" | "key_format!"
                        | "deep_format_keys!" | "nil!" | "null!" | "call" | "child!" => Ty::Nil,
                        // `json.set! :key, value` / `json.key value [, partial:, as:]`
                        // — the value written. An argument the typer could
                        // not resolve is reported at the argument; the
                        // write's own value is then simply unknown.
                        "set!" => jbuilder_value(call_args.get(1)),
                        _ if block_ret.is_some() => {
                            Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) }
                        }
                        _ => jbuilder_value(call_args.first()),
                    };
                }
                // `form.object` is the record the form was built for —
                // `block_ctx_for` parameterizes the builder from
                // `form_with model: product` (`FormBuilder[Product]`), and
                // the type argument is the answer. Before the registry
                // walk, whose entry is the gradual answer for a builder
                // without one (`form_with url:`).
                if id.0.as_str() == "ActionView::Helpers::FormBuilder"
                    && method.as_str() == "object"
                {
                    if let [model] = args.as_slice() {
                        return model.clone();
                    }
                }
                let mut steps = 0usize;
                let mut unknown_named_ancestor = false;
                // The first `method_missing` on the ancestor walk (the
                // class's own, then its mixins', then its parents').
                // Ruby consults it only once the WHOLE chain has failed
                // to find the method, so it is held here and applied
                // after the walk, never in place of a real method.
                let mut method_missing_ty: Option<Ty> = None;
                // Whether the walk has been through a class the RBI did not
                // declare: an app class inheriting a gem class gets what the
                // gem's macros generate on top of the declared surface.
                let mut via_undeclared = false;
                while let Some(cid) = current_id {
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    let Some(cls) = self.classes().get(cid) else {
                        if steps > 0 {
                            unknown_named_ancestor = true;
                        }
                        break;
                    };
                    steps += 1;
                    if cls.open || (cls.gem_boundary && via_undeclared) {
                        unknown_named_ancestor = true;
                    }
                    if !cls.gem_boundary {
                        via_undeclared = true;
                    }
                    // A signature found ANYWHERE on this walk may name
                    // the receiving class as `instance` /
                    // `T.attached_class` — a fact the declaring class
                    // cannot spell, because it is about its callers.
                    // Substituted here, against the class the walk
                    // STARTED from rather than the one the signature
                    // was found on: an inherited factory answers with
                    // an instance of the subclass that called it. Done
                    // before `unwrap_fn_ret` so a signature's params
                    // are substituted too, and the arity and
                    // kwargs-flip checks see a concrete type.
                    // Not the ancestor's own class either: an inherited scope or finder answers the receiver's (`User.active` is a `Relation[User]`).
                    let subst = |ty: &Ty| {
                        let ty = ty.subst_self(&Ty::Class { id: id.clone(), args: Vec::new() });
                        let receiver_is_model = self.classes().get(id).is_some_and(|c| c.table.is_some());
                        if cid != id && cls.table.is_some() && receiver_is_model { ty.rebind_class(cid, id) } else { ty }
                    };
                    // The class object and its instances share this one type, so a
                    // name both sides define is ambiguous. The catalog gives every model
                    // the relation builders (`order`, `group`, `limit`, `page`, …)
                    // class-side; an instance may own the same name as a reader
                    // (`belongs_to :order`, `delegated_type` singular `page`, …).
                    // A relation builder called with no arguments is not a useful
                    // query shape here (`Refund.order` is an error; bare
                    // `leaf.page` is the delegated_type reader, not
                    // `Relation[Leaf]`), so the zero-argument call is the
                    // instance reader. Callers that want the builder pass an
                    // argument (`Model.page(2)`, `Model.order(:name)`).
                    if call_args.is_empty()
                        && matches!(
                            method.as_str(),
                            "order"
                                | "group"
                                | "limit"
                                | "offset"
                                | "having"
                                | "joins"
                                | "includes"
                                | "select"
                                | "distinct"
                                | "page"
                                | "per"
                                | "paginate"
                                | "padding"
                                | "without_count"
                        )
                    {
                        if let (Some(cm), Some(im)) =
                            (cls.class_methods.get(method), cls.instance_methods.get(method))
                        {
                            if matches!(unwrap_fn_ret(cm), Ty::Relation { .. }) {
                                return unwrap_fn_ret(&subst(im));
                            }
                        }
                    }
                    if !instance_side && let Some(ty) = cls.class_methods.get(method) {
                        return unwrap_fn_ret(&subst(ty));
                    }
                    if let Some(ty) = cls.instance_methods.get(method) {
                        return unwrap_fn_ret(&subst(ty));
                    }
                    // Mixed-in modules (`include IntervalHelper`)
                    // contribute their instance methods to this class.
                    // Checked after the class's own methods, before the
                    // parent — Ruby's ancestor order puts an included
                    // module between the class and its superclass.
                    for module_id in &cls.includes {
                        if let Some(ty) = self.lookup_in_module(module_id, method) {
                            return subst(&ty);
                        }
                    }
                    // Class-object sends only consult class-side
                    // `method_missing`. Instance / included handlers answer
                    // instance sends; applying them here would type an
                    // undefined class-side call from an RBS instance sig.
                    if method_missing_ty.is_none() {
                        method_missing_ty = cls
                            .class_methods
                            .get(&Symbol::from("method_missing"))
                            .cloned();
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
                        // `String.new(x)` is the third builtin
                        // constructor, and the same argument applies:
                        // the value is a Str, so `String.new(body)
                        // .force_encoding("UTF-8")` must dispatch
                        // through `str_method` rather than looking for
                        // `force_encoding` in a class table that has
                        // none. campfire's `Webhook#extract_text_from`
                        // is the shape.
                        "String" => return Ty::Str,
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
                // which masks downstream coercion (e.g. rust's
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
                if id.0.as_str() == "Time" && method.as_str() == "use_zone" {
                    // Not the block's unresolved Var: that would report `use_zone` itself as the failure.
                    return block_ret.filter(|t| !matches!(t, Ty::Var { .. })).cloned().unwrap_or(Ty::Untyped);
                }
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
                // `tag.div(…)` / `tag.section { … }` — the TagBuilder
                // builds an element from the METHOD NAME, so every
                // method is a rendered String. (`tag.attributes(h)` is
                // one too.)
                if id.0.as_str() == "ActionView::Helpers::TagHelper::TagBuilder" {
                    return Ty::Str;
                }

                // Base64 stdlib — `Base64.strict_encode64(JSON.generate(x))`
                // appears in turbo_stream_from. All Base64 module-level
                // encoders/decoders return String.
                if id.0.as_str() == "Base64" {
                    match method.as_str() {
                        "encode64" | "decode64"
                        | "strict_encode64" | "strict_decode64"
                        | "urlsafe_encode64" | "urlsafe_decode64"
                        // The `padding: false` form GlobalID#to_param uses.
                        | "urlsafe_encode64_nopad" => return Ty::Str,
                        _ => {}
                    }
                }
                // `Process.pid` — Ruby's `Logger::Formatter` and
                // Campfire's web-push pool (`forget_after_fork`). An
                // Integer on every target that has a process at all.
                if id.0.as_str() == "Process" && method.as_str() == "pid" {
                    return Ty::Int;
                }
                // `Process.kill(signal, pid)` — TimeLimitedVideoPreviewer
                // kills a stuck ffmpeg. Answers the signal as Integer.
                if id.0.as_str() == "Process" && method.as_str() == "kill" {
                    return Ty::Int;
                }
                // `Process.clock_gettime(clock, unit = :float_second)`:
                // the `:float_*` units answer Float; every other unit
                // (`:millisecond`, `:microsecond`, `:nanosecond`, …)
                // answers Integer. No unit means Float.
                if id.0.as_str() == "Process" && method.as_str() == "clock_gettime" {
                    return match call_args.get(1).map(|a| &*a.node) {
                        None => Ty::Float,
                        Some(ExprNode::Lit { value: crate::expr::Literal::Sym { value } }) => {
                            if value.as_str().starts_with("float_") { Ty::Float } else { Ty::Int }
                        }
                        Some(_) => Ty::Untyped,
                    };
                }
                // `Timeout.timeout(sec) { ... }` — block result, or raises
                // Timeout::Error. Campfire unfurl + video previewer.
                if id.0.as_str() == "Timeout" && method.as_str() == "timeout" {
                    return Ty::Untyped;
                }
                // `r, w, pid = PTY.spawn(env, *cmd)`: the pty's reader
                // and writer and the child's pid. With a block — brace,
                // `do`, or forwarded `&callback` — CRuby yields those
                // and answers nil. `block_ret.is_some()` is presence
                // (forwarded procs carry `Untyped`); not only a typed
                // lambda body.
                if id.0.as_str() == "PTY" && method.as_str() == "spawn" {
                    if block_ret.is_some() {
                        return Ty::Nil;
                    }
                    let file = || Ty::Class { id: ClassId(Symbol::from("File")), args: vec![] };
                    return Ty::Tuple { elems: vec![file(), file(), Ty::Int] };
                }
                // `IO.popen` / `IO.copy_stream` — capture path; popen is
                // polymorphic (block vs handle), copy_stream answers bytes.
                if id.0.as_str() == "IO" {
                    match method.as_str() {
                        "popen" => return Ty::Untyped,
                        "copy_stream" => return Ty::Int,
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
                        "escape" | "quote" => return Ty::Str,
                        _ => {}
                    }
                }
                // The receiver is a modeled class that inherits from an
                // unmodeled gem/external superclass — the method is most
                // likely inherited from that gem, so treat it as a gradual
                // escape rather than an "unknown method" error. Reached only
                // after the precise builtins above have had their say.
                if unknown_named_ancestor {
                    return Ty::Untyped;
                }
                // The chain never defined the method but a class on it
                // defines `method_missing`: the message is answered at
                // runtime, by code the analyzer cannot follow name by
                // name (core's `CustomerAccountUrlHelpers` turns
                // `customer_account_*_url` into a `UrlHelpers` send). The
                // answer is what `method_missing` is declared to return,
                // gradual when it says nothing more.
                if let Some(mm) = method_missing_ty {
                    return match unwrap_fn_ret(&mm) {
                        Ty::Var { .. } => Ty::Untyped,
                        t => t,
                    };
                }
                unknown()
            }
            Some(Ty::Array { elem }) => {
                let elem: &Ty = elem;
                if is_model_relation_elem(elem) && array_find(method, args, block_ret) {
                    return Ty::Array { elem: Box::new(elem.clone()) };
                }
                // A relation delegates scope/builder calls to its element
                // model, so `user.comments.active` and `Story.where(..).hottest`
                // chain: any class method that returns a relation
                // (`Array[Self]`) — named scopes plus the query builders —
                // resolves on the relation and re-returns the relation.
                if let Ty::Class { id, .. } = elem {
                    if let Some(t) = self.column_projection(id, method, args) {
                        return t;
                    }
                    // Not only the element model's own scopes: one inherited from an abstract base answers on an association of the subclass too.
                    if let Some(anc) = self.ancestor_defining_class_method(id, method) {
                        let base = Ty::Array { elem: Box::new(Ty::Class { id: anc.clone(), args: vec![] }) };
                        return self.dispatch(Some(&base), method, block_ret, args).rebind_class(&anc, id);
                    }
                    if let Some(cls) = self.classes().get(id) {
                        match cls.class_methods.get(method) {
                            Some(scope_ret @ Ty::Array { .. }) => {
                                return scope_ret.clone();
                            }
                            // A scope/class method whose registered
                            // return carries the Relation
                            // representation still delegates on an
                            // Array-representation receiver — chain
                            // methods preserve the RECEIVER's
                            // representation (settled staging
                            // decision), so re-wrap as Array over the
                            // relation's element. Without this arm, a
                            // chain that starts on an unflipped scope
                            // (`Account.with_username(u)` — ternary
                            // body, conservative Array seed) and hops
                            // through a flipped one (`.with_domain(d)`)
                            // fell to Var and poisoned the method's
                            // harvested return to Untyped.
                            Some(Ty::Relation { of }) => {
                                return Ty::Array {
                                    elem: Box::new(Ty::Class {
                                        id: of.clone(),
                                        args: vec![],
                                    }),
                                };
                            }
                            // A TERMINAL scope — one whose body ends in
                            // `first`/`last`/`count`/… rather than a
                            // builder. It materializes, so it does NOT
                            // preserve the receiver's representation:
                            // its own return type is the answer.
                            // campfire's `user.rooms.original` is the
                            // shape. Gated on `scopes` so this stays
                            // scope delegation and doesn't become
                            // method_missing over the whole class side.
                            Some(other) if cls.relation_derived.contains(method) => {
                                return other.clone();
                            }
                            // Any other class method the element model
                            // defines — the Array-representation twin of
                            // the Relation arm's widened delegation
                            // below, and for the same reason: an
                            // association read is a relation in Rails,
                            // the scope-chain survey re-roots the call at
                            // the constant for ANY class method
                            // (`user.searches.record(q)`,
                            // `@room.messages.create_with_attachment!`),
                            // and the analyzer declining to type it left
                            // it stricter than the pipeline. Gated on the
                            // model DEFINING the name, so it is not
                            // method_missing.
                            //
                            // NOT for a name the AR catalog owns. Those
                            // are already registered on every model's
                            // class side, and their CLASS-context answer
                            // is deliberately not their relation-context
                            // one — `find` raises on a relation and
                            // returns `Self`, while on this receiver it
                            // is `Enumerable#detect`. Delegating them
                            // here jumped over `array_method`, and
                            // `@room.messages.where(…).first` came back
                            // shaped wrongly enough that the route
                            // helper stopped coercing it to an id:
                            // `room_message_path(@room.id, message)`.
                            Some(other)
                                if crate::catalog::lookup(
                                    method.as_str(),
                                    crate::catalog::ReceiverContext::Class,
                                )
                                .is_none() =>
                            {
                                return other.clone()
                            }
                            _ => {}
                        }
                    }
                    // `relation.arel` exposes the relation's underlying
                    // Arel select manager (`Tagging.where(..).select(..).arel`),
                    // so `.arel.exists` and further Arel calls type instead
                    // of collapsing to untyped at `.arel`. Gated on a
                    // model-class element so it's a relation, not a plain
                    // array.
                    if method.as_str() == "arel" {
                        return Ty::Class {
                            id: ClassId(Symbol::from("Arel::SelectManager")),
                            args: vec![],
                        };
                    }
                }
                if counted_first_last(method, args) {
                    return Ty::Array { elem: Box::new(elem.clone()) };
                }
                if let Some(t) = sub_array_slice(method, args, elem) {
                    return t;
                }
                // An initial value decides the result type (`[1, 2].sum(0.0)` is a Float).
                if method.as_str() == "sum" && !args.is_empty() {
                    return Ty::Untyped;
                }
                // `[] + [h]` is an Array of `h`: when the receiver's
                // element is empty or not yet known, `+`, `|` and
                // `concat` take the argument's. A known receiver element
                // keeps answering for the result, as before.
                if let ("+" | "|" | "concat", [other]) = (method.as_str(), args) {
                    if let (Ty::Var { .. } | Ty::Bottom, Some(Ty::Array { elem: other })) =
                        (elem, &other.ty)
                    {
                        return Ty::Array { elem: other.clone() };
                    }
                }
                array_method(method, elem, block_ret)
            }
            // Relation-typed receiver — a chain started from a scope
            // call, a relation-returning class method, or (later) an
            // association read. Resolution order:
            //   1. Relation-context catalog builtins: builders
            //      preserve the relation, terminals produce exactly
            //      the types the Array-representation arms produce
            //      (settled: terminal result types must not change).
            //   2. Class-side delegation: scopes + relation-returning
            //      class methods resolve against the element model
            //      (`Story.recent.for_user(u)`). ONLY the relation-
            //      returning surface delegates — forwarding arbitrary
            //      class methods would be Rails' method_missing
            //      semantics; unresolved sends should surface, not
            //      silently forward. No parent walk, mirroring the
            //      Array-representation delegation above.
            //   3. Enumerable/array fallback on the materialized
            //      element (`each`/`map`/`size`/…) — same types the
            //      Array representation gives.
            Some(Ty::Relation { of }) => {
                if let Some(t) = self.column_projection(of, method, args) {
                    return t;
                }
                if counted_first_last(method, args) || array_find(method, args, block_ret) {
                    return Ty::Array {
                        elem: Box::new(Ty::Class { id: of.clone(), args: vec![] }),
                    };
                }
                // `users[2..50]` on a relation loads it and slices the
                // Array — the tutorial's seed builds follower sets this
                // way. Same rule as the Array representation above.
                if let Some(t) =
                    sub_array_slice(method, args, &Ty::Class { id: of.clone(), args: vec![] })
                {
                    return t;
                }
                if let Some(entry) = crate::catalog::lookup(
                    method.as_str(),
                    crate::catalog::ReceiverContext::Relation,
                ) {
                    if let Some(kind) = entry.return_kind {
                        return crate::analyze::instantiate_return_kind(kind, of);
                    }
                }
                // Not only the element model's own scopes: one inherited from an abstract base answers on the subclass's relation.
                if let Some(anc) = self.ancestor_defining_class_method(of, method) {
                    let t = self.dispatch(Some(&Ty::Relation { of: anc.clone() }), method, block_ret, args);
                    return t.rebind_class(&anc, of);
                }
                if let Some(cls) = self.classes().get(of) {
                    match cls.class_methods.get(method) {
                        Some(ret @ Ty::Relation { .. }) => return ret.clone(),
                        // A class-side entry that answers with a
                        // COLLECTION. Chain methods preserve the
                        // receiver's representation, so a single-model
                        // element (a builder seed, or a scope seed that
                        // didn't qualify for the Relation flip)
                        // re-wraps as a relation over that model. Every
                        // other element answers with its own type: a
                        // union-of-models element isn't expressible as
                        // `Relation { of }`, and a hand-written class
                        // method that returns a plain Array is not a
                        // relation at all.
                        //
                        // The width here is deliberate and must match
                        // the Array-representation arm above, which
                        // delegates every Array-returning class method.
                        // Narrower — model elements only — is how
                        // lobsters' `merged_comments.arrange_for_user(
                        // nil)` came to resolve on an Array receiver
                        // and fail to dispatch on a Relation one.
                        //
                        // Unless the classifier READ the body as a
                        // materializing terminal: campfire's
                        // `scope :last_page, -> { ordered.last(PAGE_SIZE) }`
                        // carries the same `Array[Message]` seed as an
                        // unreadable scope would, and re-wrapping it made
                        // `@room.messages.last_page` a relation — then
                        // `Message | nil` one hop later. Its Array is the
                        // answer, same as the terminal arm below.
                        Some(Ty::Array { elem }) if !cls.materializing_scopes.contains(method) => {
                            return match &**elem {
                                Ty::Class { id, .. } => Ty::Relation { of: id.clone() },
                                _ => Ty::Array { elem: elem.clone() },
                            };
                        }
                        // Terminal scope on a relation receiver — same
                        // reasoning as the Array arm above: a scope that
                        // materializes answers with its own return type,
                        // and `scopes` keeps the delegation scoped to
                        // declared scopes.
                        Some(other) if cls.relation_derived.contains(method) => {
                            return other.clone();
                        }
                        // Any OTHER class method the model actually
                        // defines. Rails runs it inside the relation's
                        // `scoping` block and answers whatever it
                        // answers, and so does this pipeline: the
                        // scope-chain survey re-roots the call at the
                        // constant and threads the relation in as
                        // `__rel` (`User.active.find_by_transfer_id(id)`
                        // → `User.find_by_transfer_id(id, User.active)`),
                        // for any class method, not only the
                        // relation-returning ones. Declining to type it
                        // here left the analyzer STRICTER than the
                        // pipeline it describes: campfire's session
                        // transfer read out as `no known method
                        // find_by_transfer_id on Relation { User }` while
                        // the emitted call site was correct.
                        //
                        // Not method_missing: `class_methods.get` already
                        // said the model defines this name. A name no
                        // model defines still falls through to the
                        // Enumerable surface and then to the ledger.
                        // Same widening, same carve-out as the
                        // Array arm above: a name the AR catalog owns
                        // was already answered in relation context by
                        // the lookup at the top of this arm, and its
                        // class-context entry means something else.
                        Some(other)
                            if crate::catalog::lookup(
                                method.as_str(),
                                crate::catalog::ReceiverContext::Class,
                            )
                            .is_none() =>
                        {
                            return other.clone()
                        }
                        _ => {}
                    }
                }
                // No class-side entry claimed this name (including a
                // hand-written `self.find_by_<attr>`, caught above) — a
                // per-column dynamic finder is the last static reading
                // before falling to the Enumerable surface (#558).
                if let Some(t) = self.dynamic_finder_ty(of, method) {
                    return t;
                }
                let elem = Ty::Class { id: of.clone(), args: vec![] };
                array_method(method, &elem, block_ret)
            }
            Some(Ty::Hash { key, value }) => hash_method(method, key, value, block_ret, args),
            Some(Ty::Record { row }) => record_method(method, row, args),
            // A method the app adds by reopening `String` (campfire's
            // `all_emoji?`) answers where the builtin table has nothing.
            Some(Ty::Str) if method.as_str() == "bytes" && block_ret.is_some() => Ty::Str,
            Some(Ty::Str)
                if method.as_str() == "[]"
                    && args.len() == 2
                    && args[0].ty.as_ref().is_some_and(|ty| {
                        matches!(ty.peel_nilable(), Ty::Class { id, .. } if id.0.as_str() == "Regexp")
                    }) =>
            {
                Ty::Union {
                    variants: vec![Ty::Str, Ty::Nil],
                }
            }
            Some(Ty::Str) => match str_method(method) {
                Ty::Var { .. } => self
                    .lookup_string_instance(method)
                    .unwrap_or_else(unknown),
                ty => ty,
            },
            Some(Ty::Sym) => sym_method(method),
            // A `Ty::Time` value (datetime-column read, `Time.now`, etc.)
            // dispatches through the same table the `Time` class constant
            // uses. Unmodeled methods fall back to `unknown()` (an
            // inference gap) rather than the parent-chain walk — `Time`
            // has no user-defined ancestors in this corpus.
            Some(Ty::Time) => time_method(method).unwrap_or_else(unknown),
            Some(Ty::Date) => date_method(method, args).unwrap_or_else(unknown),
            // `Integer#in_time_zone` lowers only for ≤1 arg (invariant 6).
            Some(Ty::Int)
                if method.as_str() == "in_time_zone" && args.len() > 1 =>
            {
                unknown()
            }
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
                    let r = self.dispatch_on(Some(v), method, block_ret, args, instance_receiver);
                    if !matches!(r, Ty::Var { .. }) {
                        resolved.push(r);
                    }
                }
                // An un-inferred `Var` arm makes the union gradual the
                // same way an `Untyped` arm (handled above) does: when it
                // is the *only* non-Nil arm, no concrete dispatch can run,
                // but the receiver could still be anything (an Array with
                // `join`, a comment with `id`, …). Absorbing to `Untyped`
                // when nothing concrete resolved is the honest answer — a
                // gradual escape (warning), not a "no known method" error.
                // A union of concrete arms that all lack the method
                // (`Str | Nil` with `join`) still errors, as it should.
                let has_var = variants.iter().any(|v| matches!(v, Ty::Var { .. }));
                match resolved.len() {
                    0 if has_var => Ty::Untyped,
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

    /// The type of the attribute a literal `:name` / `"name"` argument
    /// names on `class`, its own or an ancestor's (`self[:name]`).
    fn attribute_by_name(&self, class: &ClassId, args: &[Expr]) -> Option<Ty> {
        let [arg] = args else { return None };
        let ExprNode::Lit { value } = &*arg.node else { return None };
        let key = match value {
            crate::expr::Literal::Sym { value } => value.clone(),
            crate::expr::Literal::Str { value } => Symbol::from(value.as_str()),
            _ => return None,
        };
        let mut cursor = Some(class.clone());
        // Bounded: a parent link that cycles must not hang the typer.
        for _ in 0..16 {
            let cur = cursor?;
            let info = self.classes().get(&cur)?;
            if let Some(ty) = info.attributes.fields.get(&key) {
                return Some(ty.clone());
            }
            cursor = info.parent.clone();
        }
        None
    }

    /// Resolve `method` against a mixed-in module's registered methods,
    /// chasing the module's own `include`s transitively. Returns the
    /// call-site result type (return type unwrapped). `Module`s carry
    /// their instance methods in the same registry slot classes use, so
    /// this is the class lookup minus the parent walk. A `seen` set
    /// guards the pathological `module A; include B; end; module B;
    /// include A; end` cycle.
    /// The call site's block type, when the method called is one that
    /// returns its block's value (`ClassInfo::block_value_methods`) —
    /// found on the receiver's class, its mixins, or a parent. The
    /// runtime's `Rails::Cache#fetch` is one by construction: its body
    /// is `yield`, and a hit answers what an earlier miss's block
    /// stored. Only an INFORMATIVE block type is adopted, as for
    /// `transaction`; otherwise the registered answer stands.
    pub(crate) fn block_value_return(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block_ret: Option<&Ty>,
    ) -> Option<Ty> {
        let ret = block_ret.filter(|t| !matches!(t, Ty::Var { .. } | Ty::Untyped))?;
        let Some(Ty::Class { id, .. }) = recv_ty else { return None };
        if id.0.as_str() == "Rails::Cache" && method.as_str() == "fetch" {
            return Some(ret.clone());
        }
        let mut current = Some(id.clone());
        for _ in 0..32 {
            let cls = self.classes().get(current.as_ref()?)?;
            let own = std::iter::once(cls).chain(cls.includes.iter().filter_map(|m| self.classes().get(m)));
            for c in own {
                if c.block_value_methods.contains(method) {
                    return Some(ret.clone());
                }
                // Defined here without being one: it shadows anything
                // further up.
                if c.instance_methods.contains_key(method) || c.class_methods.contains_key(method) {
                    return None;
                }
            }
            current = cls.parent.clone();
        }
        None
    }

    /// A method the app adds by reopening `String`, or by including a
    /// module into it. Class methods do not answer `"text".foo`.
    fn lookup_string_instance(&self, method: &Symbol) -> Option<Ty> {
        let mut stack = vec![ClassId(Symbol::from("String"))];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(m) = self.classes().get(&id) else { continue };
            if let Some(ty) = m.instance_methods.get(method) {
                return Some(unwrap_fn_ret(ty));
            }
            stack.extend(m.includes.iter().cloned());
        }
        None
    }

    pub(super) fn lookup_in_module(&self, module_id: &ClassId, method: &Symbol) -> Option<Ty> {
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
                // `-> self` on an ancestor answers the receiver, not
                // the ancestor — substitute before the return is read.
                let r = unwrap_fn_ret(&ty.subst_self(recv_ty));
                // A single gradual method makes the dynamic union
                // gradual — bail early with the absorbing type.
                if matches!(r, Ty::Untyped) {
                    return Ty::Untyped;
                }
                if !r.is_open() {
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

/// Methods on a `Time` value — modeled as the first-class `Ty::Time`
/// variant. AR datetime columns type as `Ty::Time` (see
/// `ingest::model::ty_of_column`), so this is the surface a column read
/// like `story.created_at.strftime(...)` dispatches against. The `Time`
/// class constant (`Ty::Class{"Time"}`, the receiver of `Time.now`)
/// flattens onto the same table, so the class-side constructors live
/// here too. DateTime/time-of-day columns also use `Ty::Time`;
/// date-only columns have their own table. Returns `None` for unmodeled methods so dispatch falls
/// through to the parent-chain walk.
pub(super) fn time_method(method: &Symbol) -> Option<Ty> {
    let time = || Ty::Time;
    let ty = match method.as_str() {
        // Constructors, coercions, and Time-returning transforms.
        "now" | "current" | "utc" | "local" | "at" | "today"
        | "to_time" | "in_time_zone" | "localtime" | "getlocal" | "getutc"
        | "beginning_of_day" | "end_of_day" | "beginning_of_hour" | "end_of_hour"
        | "beginning_of_week" | "end_of_week" | "beginning_of_month" | "end_of_month"
        | "beginning_of_year" | "end_of_year" | "midnight" | "noon"
        | "beginning_of_minute" | "end_of_minute" | "middle_of_day" | "at_midnight"
        | "at_beginning_of_day" | "at_end_of_day" | "at_noon" | "at_middle_of_day"
        | "at_beginning_of_hour" | "at_end_of_hour" | "at_beginning_of_minute" | "at_end_of_minute"
        | "at_beginning_of_week" | "at_end_of_week" | "at_beginning_of_month" | "at_end_of_month"
        | "at_beginning_of_year" | "at_end_of_year"
        | "yesterday" | "tomorrow" | "prev_day" | "next_day" | "days_ago" | "days_since"
        | "weeks_ago" | "weeks_since" | "next_week" | "prev_week" | "last_week"
        | "prev_month" | "next_month" | "last_month" | "months_ago" | "months_since"
        | "prev_year" | "next_year" | "last_year" | "years_ago" | "years_since"
        | "change" | "advance" | "ago" | "since" | "from_now"
        | "round" | "floor" | "ceil" | "to_datetime" => time(),
        "to_date" => Ty::Date,
        // `Time - x` is `Time` for a Duration arg but a Float for a
        // Time arg — the receiver-only dispatch can't disambiguate, so
        // gradual `Untyped` (the chains read `.before?`/`/ 60`/`> 1.minute`
        // off the result, all of which absorb Untyped).
        "+" | "-" => Ty::Untyped,
        "all_day" | "all_week" | "all_month" | "all_year" => Ty::Class {
            id: ClassId(Symbol::from("Range")),
            args: vec![time()],
        },
        // String renderings.
        "iso8601" | "rfc2822" | "rfc3339" | "to_s" | "to_fs" | "to_formatted_s"
        | "strftime" | "httpdate" | "rfc822" | "ctime" | "asctime" | "inspect"
        | "zone" => Ty::Str,
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
        | "after?" | "before?" | "past?" | "future?" | "today?" | "yesterday?" | "tomorrow?"
        | "monday?" | "tuesday?" | "wednesday?" | "thursday?" | "friday?"
        | "saturday?" | "sunday?" | "on_weekend?" | "on_weekday?" => Ty::Bool,
        _ => return None,
    };
    Some(ty)
}

/// Ruby's native date-only surface. Do not inherit the Time table:
/// Date has neither epoch seconds nor a zone, and only Date supports >>.
/// Literal `only: Model` class from a Locator kwargs hash, if present.
fn locator_only_class(args: &[crate::expr::Expr]) -> Option<ClassId> {
    for arg in args.iter().rev() {
        let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
        for (key, value) in entries {
            let ExprNode::Lit {
                value: crate::expr::Literal::Sym { value: k },
            } = &*key.node
            else {
                continue;
            };
            if k.as_str() != "only" {
                continue;
            }
            let ExprNode::Const { path } = &*value.node else { return None };
            return Some(ClassId(Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
            )));
        }
    }
    None
}

fn date_constructor(method: &Symbol, args: &[crate::expr::Expr]) -> Option<Ty> {
    // Every core Date argument is optional. Reject known wrong types
    // and excess arguments rather than declaring a crashing call clean.
    let numeric = Ty::Union { variants: vec![Ty::Int, Ty::Float] };
    let expected: Vec<Ty> = match method.as_str() {
        "new" | "civil" => vec![numeric.clone(), numeric.clone(), numeric.clone(), numeric],
        "parse" => vec![Ty::Str, Ty::Bool, numeric],
        "strptime" => vec![Ty::Str, Ty::Str, numeric],
        "iso8601" => vec![Ty::Str, numeric],
        // `Date.today` is Ruby stdlib (optional start-day). Rails'
        // `Date.current` / `yesterday` / `tomorrow` take no arguments.
        "today" => vec![numeric],
        // ActiveSupport's zone-aware today / neighbors; lowered by `time_calendar`.
        "current" | "yesterday" | "tomorrow" => vec![],
        _ => return None,
    };
    let accepts = |actual: Option<&Ty>, expected: &Ty| match actual {
        None | Some(Ty::Var { .. } | Ty::Untyped) => true,
        Some(actual) => actual == expected || matches!(expected, Ty::Union { variants } if variants.contains(actual)),
    };
    Some(if args.len() <= expected.len()
        && args.iter().zip(&expected).all(|(a, t)| accepts(a.ty.as_ref(), t)) {
        Ty::Date
    } else {
        unknown()
    })
}

fn date_method(method: &Symbol, args: &[crate::expr::Expr]) -> Option<Ty> {
    let date = || Ty::Date;
    let time = || Ty::Time;
    // Known Integer (or not-yet-typed). Untyped is excluded for day-shift
    // results: Spinel Date has no `+`/`-`, and lowering only grounds Int/Var.
    let int_shift = |a: Option<&Ty>| {
        a.is_none_or(|t| matches!(t, Ty::Int | Ty::Var { .. }))
    };
    // `ActiveSupport.in_time_zone` takes a String?/Symbol zone (runtime
    // `to_s`); reject known non-zone types such as `TimeZoneData`.
    let zone_arg = |a: Option<&crate::expr::Expr>| {
        a.is_none_or(|e| {
            matches!(
                e.ty.as_ref(),
                None | Some(Ty::Str | Ty::Sym | Ty::Nil | Ty::Var { .. } | Ty::Untyped)
            )
        })
    };
    // Arity must match `lower::time_calendar::rewrite_date_value` — typing
    // a send the lowerer leaves alone is invariant 6 (silent NoMethodError).
    let zero = args.is_empty();
    let at_most_one_int = args.len() <= 1 && args.first().map(|a| int_shift(a.ty.as_ref())).unwrap_or(true);
    Some(match method.as_str() {
        // Stdlib month shift stays on Date (Spinel implements >> / <<).
        // `Date >> Untyped` stays gradual — same bar as `+` (operand may
        // not be an Integer; claiming Date would green-light Date-only
        // follow-ups after a send that can TypeError).
        ">>" | "<<" if args.len() == 1 => match args[0].ty.as_ref() {
            Some(Ty::Untyped) => Ty::Untyped,
            Some(Ty::Int) | Some(Ty::Var { .. }) | None => date(),
            _ => return None,
        },
        // `Date + Integer` → Date. `Date + Untyped` stays gradual: the
        // operand might not be an Integer day shift, and Spinel has no `+`.
        "+" if args.len() == 1 => match args[0].ty.as_ref() {
            Some(Ty::Untyped) => Ty::Untyped,
            Some(Ty::Int) | Some(Ty::Var { .. }) | None => date(),
            _ => return None,
        },
        // `Date - Integer` → Date; `Date - Date` → a Rational day count
        // we do not model structurally (gradual, like Time − Time).
        // `Date - Untyped` stays gradual for the same reason as `+`.
        "-" if args.len() == 1 => match args[0].ty.as_ref() {
            Some(Ty::Date) | Some(Ty::Untyped) => Ty::Untyped,
            Some(Ty::Int) | Some(Ty::Var { .. }) | None => date(),
            _ => return None,
        },
        "to_date" if zero => date(),
        // ActiveSupport calendar that preserves a date-only value.
        "yesterday" | "tomorrow" | "next_week" | "prev_week" | "last_week"
        | "last_month" | "last_year"
        | "beginning_of_week" | "end_of_week" | "at_beginning_of_week" | "at_end_of_week"
        | "beginning_of_month" | "end_of_month" | "at_beginning_of_month" | "at_end_of_month"
        | "beginning_of_year" | "end_of_year" | "at_beginning_of_year" | "at_end_of_year"
            if zero =>
        {
            date()
        }
        "prev_day" | "next_day" | "days_ago" | "days_since" | "weeks_ago" | "weeks_since"
        | "next_month" | "prev_month" | "months_ago" | "months_since"
        | "next_year" | "prev_year" | "years_ago" | "years_since"
            if at_most_one_int =>
        {
            date()
        }
        // Date → time-of-day / zone conversions (Rails returns TimeWithZone).
        // `to_fs` / `to_formatted_s` / `after?` / `before?` stay unmodeled
        // until runtime + lowering exist (invariant 6).
        "to_time" | "beginning_of_day" | "end_of_day" | "midnight" | "at_midnight"
        | "at_beginning_of_day" | "at_end_of_day" | "noon" | "at_noon" | "middle_of_day"
        | "at_middle_of_day" if zero => time(),
        "in_time_zone" if args.len() <= 1 && zone_arg(args.first()) => time(),
        // `all_day` is a Time range (day edges); month/week/year stay Date.
        "all_day" if zero => Ty::Class {
            id: ClassId(Symbol::from("Range")),
            args: vec![time()],
        },
        "all_week" | "all_month" | "all_year" if zero => Ty::Class {
            id: ClassId(Symbol::from("Range")),
            args: vec![date()],
        },
        "year" | "month" | "mon" | "day" | "mday" | "wday" | "yday" if zero => Ty::Int,
        "<=>" if args.len() == 1 => Ty::Int,
        "iso8601" | "xmlschema" | "to_s" | "strftime" | "inspect" => Ty::Str,
        // `change` / `advance` / `between?` / `after?` / `before?` stay
        // unmodeled until runtime + lowering exist (invariant 6).
        "<" | ">" | "<=" | ">=" if args.len() == 1 => Ty::Bool,
        "leap?"
        | "past?" | "future?" | "today?" | "yesterday?" | "tomorrow?"
        | "monday?" | "tuesday?" | "wednesday?" | "thursday?" | "friday?"
        | "saturday?" | "sunday?" | "on_weekend?" | "on_weekday?"
            if zero =>
        {
            Ty::Bool
        }
        _ => return None,
    })
}

/// `first(n)` / `last(n)` — the COUNTED form. Ruby's `Array#first(n)`,
/// Rails' `Relation#last(n)` and `Model.first(n)` all answer an Array of
/// up to n elements, where the bare form answers one element or nil.
/// Every method table here is keyed by name alone and so typed the
/// counted form as `elem | nil`; campfire's searches controller then
/// carried `Message?` for `reachable_messages.search(q).last(100)`, the
/// controller ivar joined that with the other branch's `Message.none`,
/// and the emitted view signature — which the LOWERER got right
/// (`last_n`, declared `Array[untyped]`) — disagreed with the analyzer
/// about the same value. Gated on one non-block argument that is an
/// Integer (or not yet typed): `first { … }` and `first` stay as they
/// were. Mirrors `lower::scope_chain::counted_terminal`, which renames
/// the call for the runtime for the same reason.
/// `Array#[]` / `slice` return a *sub-array* (`Array<elem> | Nil`) when
/// indexed by a `Range` or a `(start, length)` pair, but a single
/// *element* (`elem | Nil`) for a lone integer. `array_method` sees only
/// the method name, so the argument shape is read here — otherwise
/// `comment.split[0..10].join(' ')` mistypes the slice as `Str | Nil`
/// and `.join` fails to dispatch. `None` for every other call.
fn sub_array_slice(method: &Symbol, args: &[crate::expr::Expr], elem: &Ty) -> Option<Ty> {
    if !matches!(method.as_str(), "[]" | "slice") {
        return None;
    }
    let range_index = args.len() == 1
        && matches!(
            args[0].ty.as_ref(),
            Some(Ty::Class { id, .. }) if id.0.as_str() == "Range"
        );
    (range_index || args.len() == 2).then(|| Ty::Union {
        variants: vec![Ty::Array { elem: Box::new(elem.clone()) }, Ty::Nil],
    })
}

fn counted_first_last(method: &Symbol, args: &[crate::expr::Expr]) -> bool {
    // `take(n)` is the same counted terminal as `first(n)`: the Rails
    // tutorial seeds with `User.order(:created_at).take(6)` and then
    // `.each`es the result.
    matches!(method.as_str(), "first" | "last" | "take" | "sample")
        && args.len() == 1
        && matches!(
            args[0].ty.as_ref(),
            None | Some(Ty::Int) | Some(Ty::Untyped) | Some(Ty::Var { .. })
        )
}

fn array_find(method: &Symbol, args: &[crate::expr::Expr], block_ret: Option<&Ty>) -> bool {
    method.as_str() == "find" && block_ret.is_none() && args.len() == 1
        && matches!(args[0].ty, Some(Ty::Array { .. }))
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

/// Instantiate a Relation-context catalog [`ReturnKind`] against the
/// ARRAY representation of a relation (`Array<elem>`, the inline-chain
/// stand-in), where `elem` is the element model — or a union of models
/// for a helper-shared relation, which is why this takes the element
/// `Ty` rather than a `ClassId`. The receiver's representation is
/// preserved: builders (`RelationOfSelf`) stay `Array<elem>` here;
/// Relation-typed chains instantiate through
/// `crate::analyze::instantiate_return_kind` instead. `SelfOrNil` uses
/// `union_of`, not a literal `Union`, so a union element flattens
/// instead of nesting.
fn relation_return_on_array_repr(kind: crate::catalog::ReturnKind, elem: &Ty) -> Ty {
    use crate::catalog::ReturnKind;
    match kind {
        ReturnKind::SelfType => elem.clone(),
        ReturnKind::RelationOfSelf | ReturnKind::ArrayOfSelf => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        ReturnKind::SelfOrNil => union_of(elem.clone(), Ty::Nil),
        ReturnKind::Int => Ty::Int,
        ReturnKind::IntOrNil => union_of(Ty::Int, Ty::Nil),
        ReturnKind::Bool => Ty::Bool,
        ReturnKind::ArrayOfInt => Ty::Array { elem: Box::new(Ty::Int) },
        ReturnKind::ArrayOfUntyped => Ty::Array { elem: Box::new(Ty::Untyped) },
        ReturnKind::Untyped => Ty::Untyped,
        ReturnKind::ClassRef(path) => Ty::Class {
            id: crate::ident::ClassId(Symbol::from(path)),
            args: vec![],
        },
        // Not declared by any Relation-context entry today; kept
        // total so a future entry can't panic this instantiation.
        ReturnKind::HashSymStr => Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        },
        ReturnKind::ArrayOfSym => Ty::Array { elem: Box::new(Ty::Sym) },
        ReturnKind::Str => Ty::Str,
    }
}

/// The non-nil half of an element type — what survives a `compact` /
/// `compact_blank`. Mirrors `lower::blank::non_nil`, which decides the
/// same question on the emit side; the two must not disagree.
fn non_nil_elem(elem: &Ty) -> Ty {
    match elem {
        Ty::Union { variants } => {
            let kept: Vec<Ty> =
                variants.iter().filter(|v| !matches!(v, Ty::Nil)).cloned().collect();
            match kept.len() {
                0 => Ty::Nil,
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        other => other.clone(),
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
            // Genuinely elem-dependent arms stay code — the catalog's
            // ReturnKind can't express their union-element cases
            // (refactor-plan 4.2 guidance).
            //
            // `relation.model` is the element model class. With a union
            // element it's ambiguous, so fall back to the gradual
            // escape (the common use is `query.model.table_name`).
            "model" => {
                return match elem {
                    Ty::Class { .. } => elem.clone(),
                    _ => Ty::Untyped,
                };
            }
            // `arel` on a single-model relation is intercepted by the
            // dispatch arm above (→ `Arel::SelectManager`) before
            // array_method runs; reaching here means a union element,
            // where the manager's model is ambiguous — gradual.
            "arel" => return Ty::Untyped,
            _ => {}
        }
        // Everything else resolves through the Relation-context
        // catalog (refactor-plan 4.2: the former string arms live in
        // AR_CATALOG; `relation_context_mirrors_send_rs_relation_branch`
        // pins the surface). The receiver here carries the ARRAY
        // representation — an inline `Model.where(...)` chain — so
        // builders (`RelationOfSelf`) preserve `Array<elem>`, and
        // terminals instantiate against the element exactly as the
        // arms did.
        if let Some(entry) = crate::catalog::lookup(
            method.as_str(),
            crate::catalog::ReceiverContext::Relation,
        ) {
            if let Some(kind) = entry.return_kind {
                return relation_return_on_array_repr(kind, elem);
            }
        }
    }
    // Block-returning transformations: output element type comes from
    // the block body when available (populated by the body-typer),
    // otherwise falls back to the input element type.
    let transformed_elem = || block_ret.cloned().unwrap_or_else(|| elem.clone());
    match method.as_str() {
        "length" | "size" | "count" => Ty::Int,
        // ActiveSupport's ordinal readers are `first`/`last`'s
        // neighbours in every way that matters here: an index read that
        // can miss. See `Array#third` in the CRuby overlay's
        // active_support_core_ext.rb.
        "first" | "last" | "second" | "third" | "fourth" | "fifth" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "[]" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // `map` / `collect` produce Array of the block's return type.
        "map" | "collect" | "map!" | "collect!" => {
            Ty::Array { elem: Box::new(transformed_elem()) }
        }
        "filter_map" => Ty::Array { elem: Box::new(non_nil_elem(&transformed_elem())) },
        "index_with" => Ty::Hash { key: Box::new(elem.clone()), value: Box::new(transformed_elem()) },
        // `flat_map` expects the block to return an Array, flattens by one.
        "flat_map" | "collect_concat" => match block_ret {
            Some(Ty::Array { elem: inner }) => Ty::Array { elem: inner.clone() },
            _ => Ty::Array { elem: Box::new(elem.clone()) },
        },
        // `partition { … }` → `[matching, rest]`: two same-element
        // Arrays, so an Array of Array-of-elem. Rails' own
        // `users.partition(&:administrator?)` destructures it.
        "partition" => Ty::Array {
            elem: Box::new(Ty::Array { elem: Box::new(elem.clone()) }),
        },
        // `each`, predicates, and shape-preserving transforms keep elem.
        // `flatten` (no depth): nested Arrays unwrap, and a Relation
        // element contributes its RECORDS — Ruby splices anything with
        // `to_ary`, which a Relation answers. lobsters' story page builds
        // `[@story, @story.merged_stories.….includes(:votes)].flatten`
        // and renders every element as a Story; left at the union, each
        // read off it (`ms.comments.build`) was gradual.
        "flatten" => Ty::Array { elem: Box::new(flatten_elem(elem)) },
        "each" | "reverse_each" | "select" | "filter" | "reject"
        | "sort" | "sort_by" | "reverse" | "compact" | "uniq"
        // `drop`/`take` (and their block forms) return a same-element
        // sub-array — the tail of a splat destructuring (`a, *rest =`
        // desugars `rest` to `arr.drop(n)`) among other uses.
        | "drop" | "take" | "drop_while" | "take_while"
        // ActiveSupport's `without` / `excluding` — the receiver minus
        // the named elements — and `including`, the receiver plus
        // them; same-element either way.
        | "without" | "excluding" | "including" => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        // ActiveSupport's `compact_blank` — `reject(&:blank?)`. Same
        // element type minus its nil half, which is precisely the
        // rewrite `lower::blank::try_rewrite_compact_blank` performs:
        // it re-stamps the receiver as `Array[non_nil(elem)]` and every
        // send READING that receiver keeps whatever the analyzer said
        // earlier. Without this arm the analyzer said nothing, so
        // campfire's `[ name, bio ].compact_blank.join(" - ")` reported
        // `no known method join on Array[Str]` — against a receiver the
        // lowering had just typed correctly.
        "compact_blank" | "compact_blank!" => {
            Ty::Array { elem: Box::new(non_nil_elem(elem)) }
        }
        // `delete(x)` returns the deleted element or nil.
        "delete" | "delete_at" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "pop" | "shift" | "sample" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "index" | "find_index" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        "dup" | "clone" => Ty::Array { elem: Box::new(elem.clone()) },
        // `clear` empties in place and returns SELF, so it keeps the
        // element type — the array is empty, not differently-typed.
        // Reached by `Resolv.clear_getaddresses_stubs` resetting the
        // mocha stub table (`lower::mocha`).
        "clear" => Ty::Array { elem: Box::new(elem.clone()) },
        // Array `+` (concat), `-` (set difference), `&` (set
        // intersection), and `|` (set union) preserve Array[elem].
        // `<<` mutates in place and returns self (the array). `concat` /
        // `push` / `unshift` / `prepend` / `append` likewise return the
        // modified array.
        "+" | "-" | "&" | "|" | "<<" | "concat" | "push" | "unshift" | "prepend"
        | "append" => {
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
        // ActiveSupport `Enumerable#many?` — more than one element.
        "many?" => Ty::Bool,
        // Not the element type as-is: `[nil].sum` raises, and a non-numeric
        // `sum` needs an initial value this arm doesn't see.
        "sum" => match block_ret.unwrap_or(elem) {
            Ty::Int => Ty::Int,
            Ty::Float => Ty::Float,
            _ => Ty::Untyped,
        },
        "exclude?" | "intersect?" => Ty::Bool,
        // `Set` isn't parameterized, so the element type can't be carried.
        "to_set" => Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] },
        // JSON serialization of a collection is a String whatever the
        // elements are.
        "to_json" => Ty::Str,
        // ActiveSupport `Array#as_json`: the JSON-primitive structure,
        // an Array of whatever each element serializes to.
        "as_json" => Ty::Array { elem: Box::new(Ty::Untyped) },
        "find" | "detect" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // Enumerable extrema return an element or nil (empty collection).
        "max" | "min" | "max_by" | "min_by" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        // In-place / index-yielding transforms return the array itself.
        "each_with_index" | "keep_if" | "delete_if" | "select!" | "reject!" | "sort!"
        | "uniq!" | "compact!" | "reverse!" | "sort_by!" | "insert" => Ty::Array { elem: Box::new(elem.clone()) },
        // Not the receiver's elements: `map.with_index { }` builds from the block, the only enumerator its callers chain.
        "with_index" => Ty::Array { elem: Box::new(block_ret.cloned().unwrap_or_else(|| elem.clone())) },
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
        // Not keyed by `elem` without a block: each element is the [key, value]
        // pair, and `h = h.sort_by { … }.to_h` nested the pair one level per round.
        "to_h" => match block_ret.or(Some(elem)) {
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
        // `[0, 0, 0].pack("CCC")` — binary packing (lobsters'
        // confidence_order byte strings).
        "pack" => Ty::Str,
        // `arr[i] = v` returns the assigned value in Ruby, but the value
        // type isn't available from the receiver alone and the result is
        // rarely chained. Return Nil to keep the expression's type known
        // (avoids a false "unresolved" when elem is a type variable),
        // mirroring the Hash `[]=` handling.
        "[]=" | "store" => Ty::Nil,
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
            // Unknown key → union of all field types + Nil. Folded
            // through union_many so duplicate field types collapse
            // (two Str columns must not yield `Str | Str | Nil`) and
            // union-typed fields flatten instead of nesting.
            let variants: Vec<Ty> = row
                .fields
                .values()
                .cloned()
                .chain(std::iter::once(Ty::Nil))
                .collect();
            union_many(variants)
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
    args: &[Expr],
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
        "empty?" | "any?" | "none?" | "all?" | "one?" | "key?" | "has_key?" | "include?" => Ty::Bool,
        "keys" => Ty::Array { elem: Box::new(key.clone()) },
        "key" => Ty::Union { variants: vec![key.clone(), Ty::Nil] },
        // `Hash#fetch(k, default)` answers `default` when the key is
        // missing, so the result is `value | typeof(default)` — a Nil
        // arm appears only when the default IS nil. Reading the
        // default is often the ONLY evidence about a key's shape:
        // campfire's `params.fetch(:user_ids, [])` is an Array, and
        // the params model types every value `Str` because
        // `Roundhouse::ParamValue` is not a type the analyzer carries
        // — the `[]` is the app's own statement of what that key
        // holds. Union dispatch then resolves
        // `…including(Current.user.id)`'s lowered `to_a` off the Array
        // arm and lets the Str arm decline, which is exactly right.
        //
        // This is the analyzer catching up to the emit, not diverging
        // from it: the rust fetch bridge already renders the two-arg
        // non-nil form as `.get(k).cloned().unwrap_or(default)` — a
        // `V`, not an `Option` (`emit/rust/expr/send/index.rs:414`).
        //
        // The one-arg form raises `KeyError` rather than answering
        // nil, so `value` alone would be right; it stays `value | Nil`
        // because the extra arm only costs a nil-check and every
        // target's one-arg path is written against it.
        "fetch" => match args.get(1).and_then(|a| a.ty.clone()) {
            Some(default) if !default.is_open() => union_of(value.clone(), default),
            _ => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        },
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
        // `transform_values { |v| ... }` → Hash[K, U] (the bang form
        // mutates in place but returns self — same resulting shape).
        "transform_values" | "transform_values!" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(block_ret.cloned().unwrap_or_else(|| value.clone())),
        },
        // ActiveSupport key conversions: the same values under Symbol /
        // String keys (`params.permit(:x).to_h.symbolize_keys`).
        "symbolize_keys" | "deep_symbolize_keys" | "symbolize_keys!"
        | "deep_symbolize_keys!" => Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(value.clone()),
        },
        "stringify_keys" | "deep_stringify_keys" | "stringify_keys!"
        | "deep_stringify_keys!" | "with_indifferent_access" => Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(value.clone()),
        },
        // `transform_keys { |k| ... }` → Hash[U, V].
        "transform_keys" | "transform_keys!" => Ty::Hash {
            key: Box::new(block_ret.cloned().unwrap_or_else(|| key.clone())),
            value: Box::new(value.clone()),
        },
        // Subset selections keep the Hash shape. `except`/`slice`/
        // `without` drop or keep named keys; `select`/`filter`/`reject`/
        // `compact` filter by a block/value. ActiveSupport adds
        // `except`/`without`; Ruby 3.0+ has `except` natively.
        // ActiveSupport's default-filling merges (`with_defaults` is an
        // alias of `reverse_merge`) and ActionController::Parameters'
        // `to_unsafe_h` (the unfiltered hash) also keep the shape.
        "except" | "except!" | "slice" | "slice!" | "without"
        | "select" | "filter" | "reject" | "compact" | "compact!"
        | "select!" | "filter!" | "reject!" | "keep_if" | "delete_if"
        | "merge!" | "update" | "with_defaults" | "with_defaults!"
        | "reverse_merge" | "reverse_merge!" | "deep_merge" | "deep_merge!"
        // ActiveSupport's `compact_blank` — `reject(&:blank?)`, so the
        // same shape as the `compact` beside it.
        | "compact_blank" | "compact_blank!"
        | "to_unsafe_h" | "permit!" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        // `values_at`/`fetch_values(*keys)` → Array of the value type.
        "values_at" | "fetch_values" => Ty::Array { elem: Box::new(value.clone()) },
        // ActiveSupport `Hash#to_query` / `to_param` answers a String.
        // The shared runtime hosts the scalar form; nesting stays in
        // the ruby-family reopen.
        "to_query" | "to_param" => Ty::Str,
        // `sort`/`sort_by` evaluate the hash to a sorted Array of
        // `[key, value]` pairs (same element shape as `to_a`).
        "sort" | "sort_by" => Ty::Array {
            elem: Box::new(Ty::Tuple { elems: vec![key.clone(), value.clone()] }),
        },
        // `min_by`/`max_by`/`find`/`detect` yield (k, v) and return a
        // single `[key, value]` pair, or nil on an empty hash.
        "min_by" | "max_by" | "find" | "detect" => Ty::Union {
            variants: vec![
                Ty::Tuple { elems: vec![key.clone(), value.clone()] },
                Ty::Nil,
            ],
        },
        // `invert` swaps keys and values.
        "invert" => Ty::Hash {
            key: Box::new(value.clone()),
            value: Box::new(key.clone()),
        },
        // `flat_map` returns an Array (block return flattened by one);
        // we don't track the block's element type here.
        "flat_map" => Ty::Array { elem: Box::new(unknown()) },
        // Folds / aggregates whose result depends on the block or seed,
        // and nested `dig` access — gradual.
        "reduce" | "inject" | "each_with_object" | "sum" | "dig" => Ty::Untyped,
        // Shape-neutral iteration helpers that return self (the hash)
        // for chaining (`length`/`size`/`count` are Int, handled above).
        "each_value" | "each_key" | "each_with_index" => Ty::Hash {
            key: Box::new(key.clone()),
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
        // ActiveSupport's Hash core_ext. The key-normalizing copies
        // (`symbolize_keys`, `with_indifferent_access`, ...) and
        // `to_query`/`to_param` are typed above; `to_xml` renders a
        // document.
        "to_xml" => Ty::Str,
        // `first` without a count is the first `[key, value]` pair (nil
        // when empty); with a count, an Array of pairs.
        "first" => {
            let pair = Ty::Tuple { elems: vec![key.clone(), value.clone()] };
            if args.is_empty() {
                Ty::Union { variants: vec![pair, Ty::Nil] }
            } else {
                Ty::Array { elem: Box::new(pair) }
            }
        }
        // The first key that maps to a value, or nil.
        "filter_map" => Ty::Array { elem: Box::new(block_ret.cloned().unwrap_or_else(unknown)) },
        // Strong-parameters `permit!` marks everything permitted.
        "to_hash" => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        // JSON/string renderings of a Hash are Strings whatever the
        // value type — campfire's `Webhook#payload(message).to_json`
        // nests hashes three deep.
        "to_json" | "to_s" | "inspect" => Ty::Str,
        // ActiveSupport `Hash#as_json`: string keys, JSON-primitive values.
        "as_json" => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) },
        _ => unknown(),
    }
}

/// The value a Jbuilder field write answers: its argument's type, or
/// `nil` for a bare `json.key` with nothing to write. A Var argument
/// (unresolved, reported at the argument itself) makes the value
/// gradual rather than a second "no known method" about the builder.
fn jbuilder_value(arg: Option<&crate::expr::Expr>) -> Ty {
    match arg.and_then(|a| a.ty.clone()) {
        None => Ty::Nil,
        Some(Ty::Var { .. }) => Ty::Untyped,
        Some(t) => t,
    }
}

/// Does String answer `method`? The table below is the authority — it
/// carries the ActiveSupport core_ext predicates (`present?`, `blank?`,
/// `in?`) as well as the core ones, and no class-registry entry does.
/// `lower::inquiry` asks before folding an unknown `foo?` into an
/// equality against the label; consulting the class registry instead
/// answered "String has no methods at all", and the pass rewrote
/// `notice.present?` to `notice == "present"`. A String also answers
/// every `universal_method`: without that, a String-typed `value.nil?`
/// became `value == "nil"`.
pub(crate) fn string_answers(method: &Symbol) -> bool {
    universal_method(method).is_some() || !matches!(str_method(method), Ty::Var { .. })
}

pub(super) fn str_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "length" | "size" | "bytesize" => Ty::Int,
        "upcase" | "downcase" | "strip" | "chomp" | "chop" | "reverse" | "to_s"
        | "capitalize" | "swapcase" | "squeeze" | "dup" | "clone"
        | "tr" | "tr_s" | "delete" | "gsub" | "sub" | "lstrip" | "rstrip"
        | "delete_prefix" | "delete_suffix"
        | "succ" | "next" | "swapcase!" | "+@" | "-@"
        // Encoding re-tags: same bytes, same Str. `force_encoding`
        // reaches every response body a Rails app reads back off the
        // wire — campfire's `Webhook#extract_text_from` is
        // `String.new(response.body).force_encoding("UTF-8")`, and
        // `Opengraph::Metadata::Fetching` writes the same line for
        // fxtwitter's encoding-less HTML.
        | "force_encoding" | "b" | "scrub" | "unicode_normalize"
        // Padding to a width: `severity.rjust(5)` is how Ruby's
        // `Logger::Formatter` right-aligns a level in its `%5s` field,
        // and the runtime's port spells it the same way (a `%` format
        // on a String is a shape the transpiled targets do not all
        // lower). Both answer a String whatever the width.
        | "rjust" | "ljust" | "center" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "to_sym" | "intern" => Ty::Sym,
        // Case-insensitive comparison: `casecmp` returns -1/0/1 (Int),
        // `casecmp?` returns Bool.
        "casecmp" => Ty::Int,
        // Bang forms answer nil when nothing changed, so the value is `String?`.
        "gsub!" | "sub!" | "strip!" | "lstrip!" | "rstrip!" | "chomp!" | "chop!" | "squeeze!"
        | "downcase!" | "upcase!" | "capitalize!" | "slice!" | "tr!" | "delete!" | "squish!" => Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
        "casecmp?" => Ty::Bool,
        // `ord` → the codepoint of the first character.
        "ord" => Ty::Int,
        // `getbyte(i)` → the byte at `i`, nil past the end. Kept Int, as
        // `[]` below keeps Str: the readers index inside `bytesize` (the
        // verifier's constant-time `secure_compare`).
        "getbyte" => Ty::Int,
        // `=~` (regex-match operator, desugars to `str.=~(re)`) → the
        // match position or nil. `match` (below) is the MatchData form.
        "=~" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        // `index`/`rindex` → the substring position or nil.
        "index" | "rindex" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        "bytes" => Ty::Array { elem: Box::new(Ty::Int) },
        "chars" | "lines" | "split" | "scan" | "each_char" => Ty::Array { elem: Box::new(Ty::Str) },
        // `String#count(chars)` — how many of the given characters occur.
        "count" => Ty::Int,
        // `String#ascii_only?` and ActiveSupport's `exclude?` (the
        // negation of `include?`).
        "ascii_only?" | "exclude?" => Ty::Bool,
        // `String#insert(index, str)` mutates and answers the receiver;
        // `to_str` is the implicit-conversion spelling of itself;
        // `encode` re-encodes; ActiveSupport's `first(n)` / `last(n)`
        // are the leading / trailing slice.
        "insert" | "to_str" | "encode" | "first" | "last" => Ty::Str,
        // The in-place forms answer the receiver, or nil when nothing
        // changed.
        // `String#unpack` decodes into an Array of whatever the
        // template names; `unpack1` its first element.
        "unpack" => Ty::Array { elem: Box::new(Ty::Untyped) },
        "unpack1" => Ty::Untyped,
        "empty?" | "blank?" | "present?" | "include?" | "start_with?"
        | "end_with?" | "match?" => Ty::Bool,
        // ActiveSupport `Object#presence_in(collection)` — the receiver
        // when the collection includes it, else nil. campfire's
        // `params.require(:user)[:role].presence_in(%w[ member
        // administrator ]) || "member"` whitelists a role.
        "presence_in" => Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
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
        // ActiveSupport's `inquiry` answers a StringInquirer — a String
        // subclass that exists only to host `method_missing` predicates.
        // Typed as the String it is, which is also what
        // `lower::inquiry` rewrites it to; without the entry the value
        // reads Untyped and that pass can't see a String receiver to
        // fold the predicate against.
        "inquiry" => Ty::Str,
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
        // JSON encoding is provided by the Ruby runtime; the other
        // ActiveSupport conversions need their own shared implementations.
        "to_json" => Ty::Str,
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
        // Symbol delegates string-ish matching to `to_s`: `match` →
        // MatchData|nil (Untyped, mirroring `String#match` — typically
        // chained as `m[1]`, gradual from there); `match?` the predicate
        // form; `=~` the match-position operator.
        "match" => Ty::Untyped,
        "match?" => Ty::Bool,
        "=~" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        "<=>" | "<" | ">" | "<=" | ">=" => Ty::Bool,
        _ => unknown(),
    }
}

pub(super) fn int_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" => Ty::Str,
        // `chr` → the single-character String for the codepoint (the
        // inverse of String#ord).
        "chr" => Ty::Str,
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
        "to_d" => Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from("BigDecimal")), args: vec![] },
        "zero?" | "positive?" | "negative?" | "even?" | "odd?" => Ty::Bool,
        // Arithmetic: Int op Int → Int (we approximate Int/Float mixing here;
        // refine when a fixture demands it).
        "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" | "<<" | ">>" => Ty::Int,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        // Bit access (`flags[0]`) returns the bit as Int; `times` returns
        // the receiver (Int) — `n.times { }` evaluates to `n`.
        "[]" | "times" | "clamp" | "div" | "modulo" | "gcd" | "lcm" | "pow" | "bit_length" => Ty::Int,
        "fdiv" => Ty::Float,
        "divmod" => Ty::Array { elem: Box::new(Ty::Int) },
        // Not the block form's receiver: the corpus chains these (`1.upto(5).map`), so the enumerator's values are what flows.
        "upto" | "downto" | "step" => Ty::Array { elem: Box::new(Ty::Int) },
        // ActiveSupport byte-size helpers — like the duration helpers,
        // they yield a Numeric-ish value we don't model structurally.
        "bytes" | "kilobytes" | "megabytes" | "gigabytes" | "terabytes"
        | "petabytes" | "exabytes" | "byte" | "kilobyte" | "megabyte"
        | "gigabyte" | "terabyte" | "petabyte" | "exabyte" => Ty::Untyped,
        // Integer's own protocol, which `Comparable`, `Numeric` and
        // `Integer` give it and which the table above left out.
        // `clamp` answers one of its bounds or the receiver: Integer for
        // Integer bounds, the common shape (`page.clamp(1, 100)`).
        "to_int" | "size" | "remainder" | "ceildiv" | "ord" | "magnitude" => Ty::Int,
        "between?" | "integer?" | "finite?" | "infinite?" | "nan?" | "allbits?"
        | "anybits?" | "nobits?" => Ty::Bool,
        "digits" => Ty::Array { elem: Box::new(Ty::Int) },
        "nonzero?" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        // `upto` / `downto` / `step` return the receiver with a block and
        // an Enumerator without one; the two are not told apart here.
        // `to_d` / `to_r` / `to_c` build BigDecimal / Rational / Complex,
        // which the registry does not model.
        "to_r" | "to_c" | "rationalize" | "coerce" => Ty::Untyped,
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
        // ActiveSupport `Numeric#in_time_zone` — epoch seconds → Time.
        "in_time_zone" => Ty::Time,
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
        "to_f" | "abs" | "fdiv" | "clamp" | "modulo" => Ty::Float,
        "div" => Ty::Int,
        "divmod" => Ty::Array { elem: Box::new(Ty::Float) },
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
        // Kernel#block_given? — bare call inside any method body
        // (`yield x if block_given?` in the runtime's create/create!).
        "block_given?" => Some(Ty::Bool),
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
        // `defined?(x)` — Ruby keyword ingested as a marker Send (recv:None,
        // method "defined?"). Returns a description String when the operand
        // is defined, else nil. Universal because the operand is arbitrary;
        // resolving it here keeps the common `defined?(local) && local`
        // partial-local guard off the unresolved-type ledger. (The view
        // lowerer later rewrites the marker to `!name.nil?` — emit unaffected.)
        "defined?" => Some(Ty::Union { variants: vec![Ty::Str, Ty::Nil] }),
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
        // nil; a typed receiver is answered `T?` before this table, so
        // Untyped is the answer only for an unknown one. `present?` /
        // `blank?` are universally Bool.
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

/// The element type after a full `flatten`.
fn flatten_elem(t: &Ty) -> Ty {
    match t {
        Ty::Array { elem } => flatten_elem(elem),
        Ty::Relation { of } => Ty::Class { id: of.clone(), args: vec![] },
        Ty::Union { variants } => variants
            .iter()
            .map(flatten_elem)
            .fold(Ty::Bottom, union_of),
        other => other.clone(),
    }
}

/// Ruby's own object protocol, answered for any receiver whose type
/// table has no entry for `method`.
///
/// Every object is a `Kernel`/`Object` and every class is a `Module`,
/// so the reflective surface (`instance_variable_get`,
/// `define_singleton_method`, `singleton_class`, `extend`) and
/// ActiveSupport's `Object` extensions (`in?`, `as_json`, `to_param`,
/// `instance_values`) exist on an `Integer`, a `Symbol`, a rescued
/// `StandardError` and an app model alike. The per-type tables model
/// what a type adds; none of them repeats what all of them inherit, so
/// a send that reaches the end of a table without a hit tries this one
/// before it is called unknown. Consulted AFTER the receiver's own
/// dispatch, so an app class that defines `send` or `in?` itself, or a
/// table that types `Symbol#in?` more precisely, wins.
pub(super) fn is_module_protocol(method: &Symbol) -> bool {
    matches!(method.as_str(),
        "define_method" | "alias_method" | "class_eval" | "class_exec" | "module_eval" | "module_exec"
        | "instance_method" | "public_instance_method" | "const_get" | "constants"
        | "class_variable_get" | "class_variable_set" | "class_variable_defined?"
        | "method_defined?" | "public_method_defined?" | "private_method_defined?"
        | "const_defined?" | "include?" | "instance_methods" | "public_instance_methods"
        | "private_instance_methods" | "remove_method" | "undef_method")
}

pub(super) fn object_protocol_method(
    recv_ty: Option<&Ty>,
    method: &Symbol,
    block_ret: Option<&Ty>,
    class_object: bool,
) -> Option<Ty> {
    // `include?` doubles as Enumerable/String membership — answer Bool even
    // without a proven Module receiver. Other Module-protocol names still
    // require a class/module object.
    if is_module_protocol(method) && !class_object && method.as_str() != "include?" {
        return None;
    }
    let sym_list = || Ty::Array { elem: Box::new(Ty::Sym) };
    let recv = || recv_ty.cloned().unwrap_or(Ty::Untyped);
    let block = || block_ret.filter(|t| !matches!(t, Ty::Var { .. })).cloned().unwrap_or(Ty::Untyped);
    Some(match method.as_str() {
        // Kernel / Object reflection.
        "instance_variable_get" | "instance_variable_set" | "method" | "instance_method"
        | "public_instance_method" | "const_get" | "class_variable_get" | "class_variable_set"
        | "enum_for" | "to_enum" => Ty::Untyped,
        "instance_variable_defined?" | "in?" | "method_defined?"
        | "public_method_defined?" | "private_method_defined?" | "const_defined?"
        | "class_variable_defined?" | "include?" => Ty::Bool,
        "instance_variables" | "methods" | "public_methods" | "private_methods"
        | "protected_methods" | "singleton_methods" | "instance_methods"
        | "public_instance_methods" | "private_instance_methods" | "constants" => sym_list(),
        "define_singleton_method" | "define_method" | "alias_method" => Ty::Sym,
        "singleton_class" => Ty::Class { id: ClassId(Symbol::from("Class")), args: vec![] },
        "extend" | "remove_method" | "undef_method" | "dup" | "clone" => recv(),
        "instance_eval" | "instance_exec" | "class_eval" | "class_exec" | "module_eval"
        | "module_exec" => block(),
        "display" => Ty::Nil,
        // ActiveSupport's Object extensions. `to_param` on Untyped (and
        // other non-nominal receivers) lowers to `ActiveSupport.to_param`
        // (runtime exists). A nominal class without a synthesized/
        // registered reader must hit the Object-extension refusal instead
        // of being answered here (see critic_admission Value.new.to_param).
        "to_json" | "to_yaml" => Ty::Str,
        "to_param" if !matches!(recv_ty, Some(Ty::Class { .. })) => Ty::Str,
        "===" | "!~" => Ty::Bool,
        "<=>" => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        _ => return None,
    })
}

/// `ActionController::Parameters` read as the Hash it stands in for:
/// Symbol keys over param values (`String | Array | Parameters`), so
/// `params.fetch(:ids, [])`, `params.each { |k, v| }` and
/// `params.values` hand out the same element union `params[:k]` does.
pub(super) fn params_as_hash() -> Ty {
    Ty::Hash {
        key: Box::new(Ty::Sym),
        value: Box::new(crate::analyze::registry::controllers::param_value_ty(false)),
    }
}

/// The request-params value a nested read answers: a scalar, a hash or an array, whichever the request carried.
pub(crate) const PARAM_VALUE: &str = "Roundhouse::ParamValue";

pub(crate) fn param_value_ty() -> Ty {
    Ty::Class { id: crate::ident::ClassId(Symbol::from(PARAM_VALUE)), args: vec![] }
}

// Not `permit`/`to_unsafe_h`/`require`: ActionController::Parameters methods no ruby-family runtime Hash answers.
fn param_value_method(method: &Symbol, block_ret: Option<&Ty>) -> Option<Ty> {
    let pv = param_value_ty;
    let maybe_pv = || Ty::Union { variants: vec![pv(), Ty::Nil] };
    Some(match method.as_str() {
        "[]" | "dig" | "first" | "last" | "presence" => maybe_pv(),
        "fetch" | "[]=" => pv(),
        "key?" | "has_key?" | "include?" | "member?" | "present?" | "blank?" | "empty?" | "any?"
        | "all?" | "none?" | "nil?" | "is_a?" | "kind_of?" | "instance_of?" | "respond_to?" | "=="
        | "!=" | "===" | "equal?" | "eql?" => Ty::Bool,
        "each" | "each_pair" | "each_value" | "each_key" | "each_with_index" | "reverse_each"
        | "select" | "filter" | "reject" | "compact" | "uniq" | "sort" | "sort_by" | "reverse"
        | "merge" | "except" | "slice" | "permit" | "permit!" | "to_unsafe_h" | "to_h" | "require"
        | "with_defaults" | "with_defaults!" | "reverse_merge" | "reverse_merge!" => pv(),
        "map" | "collect" | "flat_map" | "filter_map" => {
            Ty::Array { elem: Box::new(block_ret.cloned().unwrap_or(Ty::Untyped)) }
        }
        "keys" => Ty::Array { elem: Box::new(Ty::Str) },
        "values" | "to_a" => Ty::Array { elem: Box::new(pv()) },
        "size" | "length" | "count" | "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "to_s" | "join" => Ty::Str,
        "to_sym" => Ty::Sym,
        "tap" | "dup" | "freeze" => pv(),
        "!" => Ty::Bool,
        "inspect" | "to_json" => Ty::Str,
        _ => return None,
    })
}
