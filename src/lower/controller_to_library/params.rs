//! Per-resource `<Resource>Params` LibraryClass synthesis.
//!
//! Mirror of `model_to_library/row.rs`: where Row narrows the adapter's
//! `Hash[Symbol, untyped]` to typed model slots, Params narrows the
//! controller's `@params` (also `Hash[Symbol, untyped]`) to typed slots
//! per the `permit([:f1, :f2, …])` declaration.
//!
//! Concretely, for an `ArticlesController` whose `article_params` helper
//! permits `[:title, :body]`:
//!
//! ```ruby
//! class ArticleParams
//!   attr_accessor :title, :body
//!
//!   def self.from_raw(params)
//!     instance = new
//!     instance.title = params.fetch("title", "")
//!     instance.body  = params.fetch("body", "")
//!     instance
//!   end
//! end
//! ```
//!
//! And the controller's `article_params` helper body is rewritten:
//!
//! ```ruby
//! def article_params
//!   ArticleParams.from_raw(@params)        # was: @params.require(:article).permit([...])
//! end
//! ```
//!
//! Two source forms collapse to the same lowering target:
//!   - `params.expect(article: [:title, :body])`  (Rails 8 strong-params)
//!   - `params.require(:article).permit(:title, :body)` (older form)
//!
//! Recognition runs on the *source-shape* controller body (not after
//! `rewrite_params`) so we collect specs once before any rewrites fire.
//!
//! Tagged with `LibraryClassOrigin::ResourceParams { resource, fields }`
//! so per-target collapsers can group / fold (see
//! `project_specialization_strategy.md`).

use std::collections::BTreeMap;

use crate::dialect::{
    AccessorKind, Action, Controller, LibraryClass, LibraryClassOrigin, MethodDef,
    MethodReceiver, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::camelize;
use crate::span::Span;
use crate::ty::Ty;

use super::util::map_expr;

/// One (resource, fields) recognition: enough info to synthesize the
/// `<Resource>Params` class and to rewrite call sites that consume it.
#[derive(Clone, Debug)]
pub struct ParamsSpec {
    /// Resource symbol from the source (e.g. `:article`). Single-word,
    /// snake_case — used as the registry key.
    pub resource: Symbol,
    /// Permitted fields in source order. Values become `attr_accessor`
    /// declarations on the synthesized class.
    pub fields: Vec<Symbol>,
    /// Synthesized class name (`ArticleParams` for resource `:article`).
    pub class_id: ClassId,
}

/// Walk every controller's action bodies and collect one ParamsSpec per
/// unique resource. If two controllers permit the same resource with
/// different field sets, the first one wins (silently — collisions in
/// practice don't appear in real-blog; if they ever do we can promote
/// to per-controller naming or take field unions).
pub fn collect_specs(controllers: &[Controller]) -> BTreeMap<Symbol, ParamsSpec> {
    let mut out: BTreeMap<Symbol, ParamsSpec> = BTreeMap::new();
    for c in controllers {
        for action in c.actions() {
            collect_from_expr(&action.body, &mut out);
        }
    }
    out
}

fn collect_from_expr(expr: &Expr, out: &mut BTreeMap<Symbol, ParamsSpec>) {
    if let Some((resource, fields)) = match_permit_call(expr) {
        out.entry(resource.clone()).or_insert_with(|| ParamsSpec {
            class_id: params_class_id(&resource),
            resource,
            fields,
        });
    }
    walk_children(expr, &mut |c| collect_from_expr(c, out));
}

/// Match either of the two source forms:
///   - `params.expect(article: [:title, :body])`
///   - `params.require(:article).permit(:title, :body)`
///   - `params.require(:article).permit([:title, :body])`  (already-rewritten)
///
/// Returns the (resource, fields) tuple on success.
fn match_permit_call(expr: &Expr) -> Option<(Symbol, Vec<Symbol>)> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else {
        return None;
    };

    // Form 1: bare `params.expect(article: [...])`. The recv is the
    // `params` Send (no recv, no args).
    if method.as_str() == "expect" && is_bare_params(recv) && args.len() == 1 {
        let ExprNode::Hash { entries, .. } = &*args[0].node else {
            return None;
        };
        if entries.len() != 1 {
            return None;
        }
        let (k, v) = &entries[0];
        let resource = sym_of(k)?;
        let fields = sym_array(v)?;
        return Some((resource, fields));
    }

    // Form 2: `<x>.permit(...)` where `<x>` is `params.require(:resource)`.
    if method.as_str() == "permit" {
        let (resource, _) = match_require_chain(recv)?;
        let fields = collect_permit_args(args)?;
        return Some((resource, fields));
    }

    None
}

/// Match `params.require(:resource)` — returns the resource symbol on
/// success. The unit second tuple element is reserved for shapes that
/// might carry a third component later (e.g. nested permits).
fn match_require_chain(expr: &Expr) -> Option<(Symbol, ())> {
    let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "require" || args.len() != 1 {
        return None;
    }
    if !is_bare_params(inner) {
        return None;
    }
    let resource = sym_of(&args[0])?;
    Some((resource, ()))
}

/// `permit` accepts either a single Array arg (`permit([:f1, :f2])`) or
/// a splat of Sym args (`permit(:f1, :f2)`). Normalize to Vec<Symbol>.
fn collect_permit_args(args: &[Expr]) -> Option<Vec<Symbol>> {
    if args.len() == 1 {
        // Single Array arg form.
        if let ExprNode::Array { elements, .. } = &*args[0].node {
            let mut out = Vec::with_capacity(elements.len());
            for el in elements {
                out.push(sym_of(el)?);
            }
            return Some(out);
        }
        // Single Sym arg form (1-permit case).
        if let Some(s) = sym_of(&args[0]) {
            return Some(vec![s]);
        }
        return None;
    }
    // Splat-of-Syms form.
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(sym_of(a)?);
    }
    Some(out)
}

fn sym_of(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

fn sym_array(e: &Expr) -> Option<Vec<Symbol>> {
    let ExprNode::Array { elements, .. } = &*e.node else {
        return None;
    };
    let mut out = Vec::with_capacity(elements.len());
    for el in elements {
        out.push(sym_of(el)?);
    }
    Some(out)
}

fn is_bare_params(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty()
    ) || matches!(
        // Already-rewritten form: `@params`.
        &*e.node,
        ExprNode::Ivar { name } if name.as_str() == "params"
    )
}

fn walk_children<F: FnMut(&Expr)>(expr: &Expr, f: &mut F) {
    use crate::expr::InterpPart;
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().for_each(f),
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv.as_ref() {
                f(r);
            }
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            args.iter().for_each(&mut *f);
            if let Some(b) = block.as_ref() {
                f(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::Assign { value, .. } => f(value),
        ExprNode::Array { elements, .. } => elements.iter().for_each(&mut *f),
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    f(expr);
                }
            }
        }
        ExprNode::Return { value } => f(value),
        _ => {}
    }
}

/// `<Resource>Params` ClassId. e.g. `:article` → `ArticleParams`.
pub fn params_class_id(resource: &Symbol) -> ClassId {
    ClassId(Symbol::from(format!("{}Params", camelize(resource.as_str()))))
}

/// Synthesize one `<Resource>Params` LibraryClass per spec. Output is
/// emitted alongside the controller LCs into `app/models/` (the
/// universal-class location); routing it elsewhere is a per-target
/// emit-time choice.
pub fn synthesize_params_classes(specs: &BTreeMap<Symbol, ParamsSpec>) -> Vec<LibraryClass> {
    specs.values().map(build_params_class).collect()
}

fn build_params_class(spec: &ParamsSpec) -> LibraryClass {
    let mut methods: Vec<MethodDef> = Vec::new();
    methods.push(synth_params_initialize(&spec.class_id, &spec.fields));
    for field in &spec.fields {
        methods.push(synth_attr_reader(&spec.class_id, field));
        methods.push(synth_attr_writer(&spec.class_id, field));
    }
    methods.push(synth_from_raw(&spec.class_id, &spec.resource, &spec.fields));
    methods.push(synth_to_h(&spec.class_id, &spec.fields));

    LibraryClass {
        name: spec.class_id.clone(),
        is_module: false,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: Some(LibraryClassOrigin::ResourceParams {
            resource: spec.resource.clone(),
            fields: spec.fields.clone(),
        }),
    }
}

/// `def initialize` — zero-arg constructor that assigns each permitted
/// field to the empty string. Mirrors `synth_row_initialize` in
/// `model_to_library/row.rs`: the `from_raw` factory body calls
/// `instance = new`, then per-field setters; strict-typed targets
/// (Rust) need the explicit constructor since they don't have the
/// Ruby/Crystal/TS auto-init-from-attr_accessor convention. All
/// fields are `Ty::Str` (CGI string-typed) per `synth_attr_reader`'s
/// rule, so the literal default is consistently `""`.
fn synth_params_initialize(owner: &ClassId, fields: &[Symbol]) -> MethodDef {
    let mut stmts: Vec<Expr> = Vec::new();
    for field in fields {
        let rhs = Expr {
            span: Span::synthetic(),
            node: Box::new(ExprNode::Lit {
                value: Literal::Str { value: String::new() },
            }),
            ty: Some(Ty::Str),
            effects: EffectSet::default(),
            leading_blank_line: false,
            diagnostic: None,
            str_coercion: None,
        };
        stmts.push(Expr {
            span: Span::synthetic(),
            node: Box::new(ExprNode::Assign {
                target: LValue::Ivar { name: field.clone() },
                value: rhs,
            }),
            ty: Some(Ty::Nil),
            effects: EffectSet::default(),
            leading_blank_line: false,
            diagnostic: None,
            str_coercion: None,
        });
    }
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Seq { exprs: stmts }),
        ty: Some(Ty::Nil),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        str_coercion: None,
    };
    MethodDef {
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
    }
}

fn synth_attr_reader(owner: &ClassId, field: &Symbol) -> MethodDef {
    // Permitted fields are user-supplied strings from the request (CGI
    // string-typed before any model-side coercion). Type as Str so the
    // value flows uniformly into setter assignments.
    let field_ty = Ty::Str;
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Ivar { name: field.clone() }),
        ty: Some(field_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        str_coercion: None,
    };
    MethodDef {
        name: field.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], field_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeReader,
        is_async: false,
            mutates_self: false,
    }
}

fn synth_attr_writer(owner: &ClassId, field: &Symbol) -> MethodDef {
    let value = Symbol::from("value");
    let field_ty = Ty::Str;
    let rhs = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Var { id: VarId(0), name: value.clone() }),
        ty: Some(field_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        str_coercion: None,
    };
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Assign {
            target: LValue::Ivar { name: field.clone() },
            value: rhs,
        }),
        ty: Some(field_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        str_coercion: None,
    };
    MethodDef {
        name: Symbol::from(format!("{}=", field.as_str())),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value.clone())],
        body,
        signature: Some(fn_sig(vec![(value, field_ty.clone())], field_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::AttributeWriter,
        is_async: false,
            mutates_self: false,
    }
}

/// `def self.from_raw(params)`
/// `  sub = params.fetch("<resource>", {})`
/// `  instance = new`
/// `  instance.f = sub.fetch("f", "")`
/// `  ...`
/// `  instance`
/// `end`
///
/// The fetch-with-default-empty-string shape collapses missing keys to
/// "" rather than nil, keeping the field type concrete (Str). Same
/// convention as `app/views/articles/_form.html.erb` form-field
/// defaults. The leading `sub = params.fetch("<resource>", {})` dives
/// into the nested resource hash that controller params arrive under
/// (e.g. `{"article" => {"title" => …}}`); the empty-hash default keeps
/// the field fetches non-divergent if the resource key is absent.
fn synth_from_raw(owner: &ClassId, resource: &Symbol, fields: &[Symbol]) -> MethodDef {
    use crate::lower::typing::with_ty;
    let params = Symbol::from("params");
    let raw_sub = Symbol::from("raw_sub");
    let sub = Symbol::from("sub");
    let instance = Symbol::from("instance");

    // Type-shorthand helpers so the body's IR carries explicit annotations
    // — the body-typer in mod.rs runs over the synthesized class, but
    // attaching the types we know-by-construction keeps the emit
    // dispatch (TS `.fetch` → bracket access, Crystal Hash#fetch
    // narrowing) deterministic.
    let param_value_ty = Ty::Class {
        id: ClassId(Symbol::from("Roundhouse::ParamValue")),
        args: vec![],
    };
    let inner_hash_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(param_value_ty.clone()),
    };
    let outer_hash_ty = inner_hash_ty.clone();

    let str_lit = |s: &str| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: s.to_string() } },
        ),
        Ty::Str,
    );
    let empty_hash = |ty: Ty| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries: Vec::new(), kwargs: false },
        ),
        ty,
    );
    let var = |name: &Symbol, ty: Ty| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: name.clone() },
        ),
        ty,
    );

    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };

    // raw_sub = params.fetch("<resource>", {})
    //   — value type is `ParamValue` per the body-typer.
    let resource_fetch = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var(&params, Ty::Hash {
                    key: Box::new(Ty::Str),
                    value: Box::new(param_value_ty.clone()),
                })),
                method: Symbol::from("fetch"),
                args: vec![
                    str_lit(resource.as_str()),
                    empty_hash(inner_hash_ty.clone()),
                ],
                block: None,
                parenthesized: false,
            },
        ),
        param_value_ty.clone(),
    );

    // sub = raw_sub.is_a?(Hash) ? raw_sub : {}
    //   — narrows the ParamValue variant to Hash[String, ParamValue]
    //   on strict targets; degrades cleanly under duck typing.
    let is_a_hash = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var(&raw_sub, param_value_ty.clone())),
                method: Symbol::from("is_a?"),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Hash")] },
                )],
                block: None,
                parenthesized: true,
            },
        ),
        Ty::Bool,
    );
    // Then-branch wraps the `raw_sub` var read in a `Cast` to
    // `Hash[String, ParamValue]`. The lowerer types `raw_sub` as the
    // outer `ParamValue` (rust2 → `serde_json::Value`), so a bare Var
    // read in the then arm renders as `Value` while the else arm's
    // empty Hash literal renders as `HashMap<String, Value>` — the
    // branches mismatch under strict typing. The Cast surfaces the
    // narrowing intent so per-target emit can bridge: TS as-cast,
    // Crystal `as Hash(...)`, rust2 inserts `.as_object().cloned().
    // unwrap_or_default().into_iter().collect::<HashMap<_, _>>()`.
    let sub_narrowed = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: is_a_hash,
                then_branch: with_ty(
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Cast {
                            value: var(&raw_sub, param_value_ty.clone()),
                            target_ty: inner_hash_ty.clone(),
                        },
                    ),
                    inner_hash_ty.clone(),
                ),
                else_branch: empty_hash(inner_hash_ty.clone()),
            },
        ),
        inner_hash_ty.clone(),
    );

    let new_call = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![owner.0.clone()] },
                )),
                method: Symbol::from("new"),
                args: Vec::new(),
                block: None,
                parenthesized: true,
            },
        ),
        owner_ty.clone(),
    );

    let mut stmts: Vec<Expr> = Vec::new();
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: raw_sub.clone() },
            value: resource_fetch,
        },
    ));
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: sub.clone() },
            value: sub_narrowed,
        },
    ));
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: new_call,
        },
    ));

    for field in fields {
        // raw_<field> = sub.fetch("<field>", "")
        //   — value type at the body-typer level is `ParamValue`;
        //   `is_a?(String)` narrows it for the String-typed attr.
        let raw_field = Symbol::from(format!("raw_{}", field.as_str()));
        let fetch_call = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var(&sub, inner_hash_ty.clone())),
                    method: Symbol::from("fetch"),
                    args: vec![str_lit(field.as_str()), str_lit("")],
                    block: None,
                    parenthesized: false,
                },
            ),
            param_value_ty.clone(),
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: raw_field.clone() },
                value: fetch_call,
            },
        ));
        let is_a_string = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var(&raw_field, param_value_ty.clone())),
                    method: Symbol::from("is_a?"),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Const { path: vec![Symbol::from("String")] },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
            Ty::Bool,
        );
        let narrowed = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_a_string,
                    then_branch: var(&raw_field, Ty::Str),
                    else_branch: str_lit(""),
                },
            ),
            Ty::Str,
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var(&instance, owner_ty.clone())),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![narrowed],
                block: None,
                parenthesized: false,
            },
        ));
    }

    stmts.push(var(&instance, owner_ty.clone()));

    let _ = outer_hash_ty;

    // Declare `params` as `Hash[String, Roundhouse::ParamValue]` —
    // the same shape carried at the controller's `@params` slot
    // (see `runtime/ruby/action_controller/base.rbs`). ParamValue
    // is the recursive `String | Hash[String, PV] | Array[PV]`
    // union each target's runtime realizes natively (Crystal alias,
    // TS type, Ruby dynamic). Using it here keeps from_raw's
    // call-site type-check honest — passing `@params` directly
    // works without a cast on strict targets.
    let param_value_ty = Ty::Class {
        id: ClassId(Symbol::from("Roundhouse::ParamValue")),
        args: vec![],
    };
    let params_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(param_value_ty),
    };
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    MethodDef {
        name: Symbol::from("from_raw"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(params.clone())],
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts }),
        signature: Some(fn_sig(vec![(params, params_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
    }
}

/// `def to_h; { "field1" => @field1, "field2" => @field2, … }; end` —
/// returns a String-keyed Hash of the typed-struct's fields. Mirrors
/// the `Parameters#to_h` surface so `permitted.to_h` keeps working
/// after the lowerer rewrites `params.permit(...)` to typed-struct
/// construction. Value type is `Str` (matching the synthesized
/// attr_reader); strict targets see `Hash[String, String]`, no
/// `untyped` channel.
fn synth_to_h(owner: &ClassId, fields: &[Symbol]) -> MethodDef {
    let entries: Vec<(Expr, Expr)> = fields
        .iter()
        .map(|field| {
            let key = Expr {
                span: Span::synthetic(),
                node: Box::new(ExprNode::Lit {
                    value: Literal::Str { value: field.as_str().to_string() },
                }),
                ty: Some(Ty::Str),
                effects: EffectSet::default(),
                leading_blank_line: false,
                diagnostic: None,
                str_coercion: None,
            };
            let value = Expr {
                span: Span::synthetic(),
                node: Box::new(ExprNode::Ivar { name: field.clone() }),
                ty: Some(Ty::Str),
                effects: EffectSet::default(),
                leading_blank_line: false,
                diagnostic: None,
                str_coercion: None,
            };
            (key, value)
        })
        .collect();
    let hash = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Hash { entries, kwargs: false }),
        ty: Some(Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Str),
        }),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        str_coercion: None,
    };
    let ret_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Str),
    };
    MethodDef {
        name: Symbol::from("to_h"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: hash,
        signature: Some(fn_sig(vec![], ret_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
    }
}

fn fn_sig(params: Vec<(Symbol, Ty)>, ret: Ty) -> Ty {
    Ty::Fn {
        params: params
            .into_iter()
            .map(|(name, ty)| crate::ty::Param {
                name,
                ty,
                kind: crate::ty::ParamKind::Required,
            })
            .collect(),
        block: None,
        ret: Box::new(ret),
        effects: crate::effect::EffectSet::pure(),
    }
}

/// Build the `ClassInfo` registry entry for a synthesized Params class
/// — mirrors `model_to_library/row.rs::row_class_info`.
pub fn params_class_info(lc: &LibraryClass) -> crate::analyze::ClassInfo {
    let mut info = crate::analyze::ClassInfo::default();
    for m in &lc.methods {
        if let Some(sig) = &m.signature {
            match m.receiver {
                MethodReceiver::Instance => {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), m.kind);
                }
                MethodReceiver::Class => {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), m.kind);
                }
            }
        }
    }
    info
}

/// Rewrite controller-action expressions: replace each `params.expect(...)` /
/// `params.require(:r).permit(...)` with `<Resource>Params.from_raw(@params)`.
/// `specs` carries the (resource, class_id) mapping; expressions whose
/// resource isn't in `specs` (shouldn't happen — we collected from
/// these same bodies) fall through unchanged.
pub fn rewrite_to_from_raw(
    expr: &Expr,
    specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Expr {
    map_expr(expr, &|e| {
        let (resource, _fields) = match_permit_call(e)?;
        let spec = specs.get(&resource)?;
        Some(build_from_raw_call(&spec.class_id, e.span))
    })
}

fn build_from_raw_call(class_id: &ClassId, span: Span) -> Expr {
    let class_const = Expr::new(
        span,
        ExprNode::Const { path: vec![class_id.0.clone()] },
    );
    // `@params` directly — the synthesized `from_raw` dives into the
    // nested resource key itself (`sub = params.fetch("<resource>", {})`),
    // so the call site doesn't need a `.require(:r).to_h` chain.
    let params_ivar = Expr::new(span, ExprNode::Ivar { name: Symbol::from("params") });
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(class_const),
            method: Symbol::from("from_raw"),
            args: vec![params_ivar],
            block: None,
            parenthesized: true,
        },
    )
}

/// Rewrite `<typed-params>[:field]` to `<typed-params>.field` for any
/// receiver typed as a synthesized `<Resource>Params` class. The
/// synthesized class has typed `attr_reader` accessors per permitted
/// field; calling them via field access (instead of `[]` bracket
/// dispatch) gets strict-typed targets concrete typed dispatch
/// without going through the heterogeneous-Hash channel that
/// `[]` would imply.
///
/// Run AFTER body typing — the receiver's `.ty` annotation is what
/// drives the rewrite. Falls through silently when the receiver
/// isn't typed as a known `<Resource>Params` class, or when the
/// literal key isn't a permitted field.
///
/// Stage 3 of the Parameters specialization plan (see
/// `project_parameters_specialization_plan.md`). Stage 1 was the
/// `permit → typed-struct synthesis`; stage 2 enriched the
/// synthesized class API; this stage closes the loop so existing
/// `permitted[:title]`-shape call sites in test bodies / view
/// bodies dispatch through the typed accessor.
pub fn rewrite_typed_bracket_to_field(
    expr: &Expr,
    specs: &BTreeMap<Symbol, ParamsSpec>,
) -> Expr {
    use crate::ty::Ty;
    // Build a quick `class_id -> permitted-fields-set` lookup so the
    // walker can validate the literal key is one of the permitted
    // fields before rewriting.
    let mut permitted_fields: std::collections::HashMap<
        ClassId,
        std::collections::HashSet<String>,
    > = std::collections::HashMap::new();
    for spec in specs.values() {
        let mut set = std::collections::HashSet::new();
        for f in &spec.fields {
            set.insert(f.as_str().to_string());
        }
        permitted_fields.insert(spec.class_id.clone(), set);
    }

    map_expr(expr, &|e| {
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
            return None;
        };
        if method.as_str() != "[]" || args.len() != 1 {
            return None;
        }
        let recv_class_id = match recv.ty.as_ref() {
            Some(Ty::Class { id, .. }) => id,
            _ => return None,
        };
        let fields = permitted_fields.get(recv_class_id)?;
        let key = match &*args[0].node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => return None,
        };
        if !fields.contains(&key) {
            return None;
        }
        // Synthesize `recv.<field>` — a zero-arg Send to the typed
        // attr_reader. Carries the receiver's type forward and drops
        // the bracket-key arg.
        Some(Expr {
            span: e.span,
            node: Box::new(ExprNode::Send {
                recv: Some(recv.clone()),
                method: Symbol::from(key),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            }),
            ty: Some(Ty::Str),
            effects: e.effects.clone(),
            leading_blank_line: e.leading_blank_line,
            diagnostic: None,
            str_coercion: None,
        })
    })
}
