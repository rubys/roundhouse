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
    let params = Symbol::from("params");
    let sub = Symbol::from("sub");
    let instance = Symbol::from("instance");

    let resource_fetch = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: params.clone() },
            )),
            method: Symbol::from("fetch"),
            args: vec![
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: Literal::Str { value: resource.as_str().to_string() },
                    },
                ),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries: Vec::new(), kwargs: false },
                ),
            ],
            block: None,
            parenthesized: false,
        },
    );

    let new_call = Expr::new(
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
    );

    let mut stmts: Vec<Expr> = Vec::new();
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: sub.clone() },
            value: resource_fetch,
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
        // sub.fetch("field", "") — string key matches the request-body
        // parser's String-keyed Hash output. Symbol keys would fall
        // through to the default and silently produce empty fields.
        let fetch_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: VarId(0), name: sub.clone() },
                )),
                method: Symbol::from("fetch"),
                args: vec![
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit {
                            value: Literal::Str { value: field.as_str().to_string() },
                        },
                    ),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Str { value: String::new() } },
                    ),
                ],
                block: None,
                parenthesized: false,
            },
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: VarId(0), name: instance.clone() },
                )),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![fetch_call],
                block: None,
                parenthesized: false,
            },
        ));
    }

    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: instance },
    ));

    // Declare `params` as a nested Hash[String, Hash[String, untyped]]
    // rather than Hash[String, untyped]: from_raw's body does
    // `sub = params.fetch("<resource>", {})` then `sub.fetch("field", "")`,
    // so the body typer needs `sub` to be Hash-typed for per-target
    // emit (e.g. TS's `.fetch` → bracket-access rewrite) to fire on
    // the inner accesses. The actual `@params` argument is
    // Hash[String, untyped] at the call site (mixed path-captures +
    // nested resource body); the wider declared type is a no-op
    // under untyped's slidability and accurate for the only access
    // pattern from_raw uses (resource-keyed read).
    let inner_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) };
    let params_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(inner_ty) };
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
            };
            let value = Expr {
                span: Span::synthetic(),
                node: Box::new(ExprNode::Ivar { name: field.clone() }),
                ty: Some(Ty::Str),
                effects: EffectSet::default(),
                leading_blank_line: false,
                diagnostic: None,
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
        })
    })
}
