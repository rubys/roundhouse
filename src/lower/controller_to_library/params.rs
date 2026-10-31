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
//! One class per distinct `(resource, fields)` pair, NOT per resource:
//! an app may permit the same resource differently in different
//! controllers, and each list is its own mass-assignment boundary.
//!
//! Tagged with `LibraryClassOrigin::ResourceParams { resource, fields }`
//! so per-target collapsers can group / fold (see
//! `project_specialization_strategy.md`).

use std::collections::BTreeMap;

use crate::dialect::{
    AccessorKind, Controller, LibraryClass, LibraryClassOrigin, MethodDef,
    MethodReceiver, Param,
};
use crate::effect::EffectSet;
use crate::expr::{Arm, Expr, ExprNode, LValue, Literal, Pattern};
use crate::ident::{ClassId, Symbol, VarId};
use crate::lower::typing::with_ty;
use crate::naming::camelize;
use crate::span::Span;
use crate::ty::Ty;

use super::util::map_expr;

/// One (resource, fields) recognition: enough info to synthesize the
/// `<Resource>Params` class and to rewrite call sites that consume it.
#[derive(Clone, Debug)]
pub struct ParamsSpec {
    /// Resource symbol from the source (e.g. `:article`). Single-word,
    /// snake_case.
    pub resource: Symbol,
    /// Permitted fields in source order. Values become `attr_accessor`
    /// declarations on the synthesized class.
    pub fields: Vec<Symbol>,
    /// Synthesized class name (`ArticleParams` for resource `:article`;
    /// controller-qualified when one resource carries several lists —
    /// see `assign_class_ids`).
    pub class_id: ClassId,
    /// Span of the `permit(...)` / `expect(...)` call this spec was
    /// recognized from — the enclosing source span for everything the
    /// synthesized class contains.
    pub span: Span,
    /// Some call site chains `.except(:key)` off this permit —
    /// synthesize the `except` method. Demand-gated because its
    /// nil-writes widen the class's fields to nilable on inferring
    /// targets; classes nobody excepts keep tight types.
    pub wants_except: bool,
    /// Some call site writes `<Model>.create(<helper>)` / `.create!` on
    /// the model this list's resource names — synthesize the matching
    /// typed factory. Demand-gated like `wants_except`: the runtime's
    /// `create` takes an attribute Hash, so every params-permitted model
    /// in every app would otherwise carry two methods nobody calls.
    pub wants_create: bool,
    pub wants_create_bang: bool,
    /// Some call site needs this list as an ATTRIBUTE HASH — see
    /// [`synth_to_attrs`]. Demand is read off the rewritten controller
    /// body (`<helper>.to_attrs`), the same way `wants_create` is read
    /// off `<Model>.create(<helper>)`, so nothing has to be threaded
    /// from the pass that decided it.
    pub wants_to_attrs: bool,
    /// Some call site reads `<helper>.as_json` — synthesize `as_json`,
    /// Rails' `Parameters#as_json`: the permitted values, String-keyed
    /// (the class's `to_h`). Demand-gated like `wants_to_attrs`.
    pub wants_as_json: bool,
    /// Proven membership calls that require scalar presence and value lookup.
    pub lookup_methods: std::collections::BTreeSet<Symbol>,
    /// Writes and unsupported filters prevent use of the value map.
    pub lookup_blocked: bool,
    /// Set by [`ParamsSpecs::carry_nested`]: the target carries nested
    /// params (ruby family), so the value map replaces the scalar helpers.
    pub(crate) ruby_values: bool,
    /// Controllers whose bodies declared this exact permit list, in
    /// source order. Two controllers permitting the same fields share
    /// one class (campfire's `FirstRunsController` and `UsersController`
    /// both permit `:user` × name/avatar/email_address/password); the
    /// first entry names the class when it needs qualifying.
    pub declaring: Vec<ClassId>,
    /// This spec owns the unqualified `<Resource>Params` name — and with
    /// it the model's plain `from_params` / `update` / `update!`
    /// surface. Exactly one spec per resource can, and a resource whose
    /// lists all come from off-resource controllers has none.
    pub is_canonical: bool,
    /// Permitted fields the resource's model declares `has_one_attached`
    /// for (`:avatar` on `User`). These are FILE fields: a multipart
    /// part rather than a String, read back as an
    /// `ActionDispatch::Http::UploadedFile?` through the ruby family's
    /// `UploadedFile.from_params`, and handed to the model's `<attr>=`
    /// writer as-is. Filled by [`ParamsSpecs::mark_file_fields`] once
    /// the models are known; empty until then, which is every field a
    /// String — the shape every emitter already compiles.
    pub file_fields: std::collections::BTreeSet<Symbol>,
    /// The scalar permitted fields the list was recognized with, before
    /// [`ParamsSpecs::carry_nested`] adds carried nested keys to
    /// `fields`: what a call site's own `(resource, fields)` matches.
    pub key_fields: Vec<Symbol>,
    /// The nested keys' filters (`name:mode:allowed`), part of the spec's
    /// identity with `key_fields`: two lists that differ only there are
    /// two mass-assignment boundaries, so two specs.
    pub nested_signature: Vec<String>,
    /// Non-scalar keys whose filter this lowering can reproduce
    /// (`k: [[:a, :b]]`, `k: [:a, :b]`, `k: {}`), in source order.
    pub nested_candidates: Vec<NestedKey>,
    /// The candidates carried as fields (ruby family): each a
    /// `Roundhouse::ParamValue` member read with `Params.nested`.
    pub nested_fields: BTreeMap<Symbol, NestedKey>,
    pub written: BTreeMap<Symbol, Vec<Ty>>,
    /// Keys only a `.merge(k: v)` after the permit adds: the request
    /// never carries them, so their type is what the merge writes.
    pub merged_only: std::collections::BTreeSet<Symbol>,
    pub wants_assign: bool,
}

impl ParamsSpec {
    /// `fields` without the carried nested keys: the String-valued ones.
    pub fn scalar_fields(&self) -> Vec<Symbol> {
        self.fields.iter().filter(|f| !self.nested_fields.contains_key(*f)).cloned().collect()
    }
}

/// One non-scalar permitted key and how Rails' `permit` filters it:
/// `mode` as `Params.nested` takes it ("list", "any", "hash", "deep"),
/// or "scalars" for `k: []` (`Params.scalar_list`, an `Array[String]`),
/// `allowed` the key's flat list of scalar sub-keys.
#[derive(Clone, Debug, PartialEq)]
pub struct NestedKey {
    pub name: Symbol,
    pub allowed: Vec<String>,
    pub mode: &'static str,
    pub controller: ClassId,
    pub span: Span,
}

/// Every distinct `(resource, fields)` permit list in the app, deduped.
///
/// Keying by the PAIR (rather than by resource alone) is what keeps
/// per-controller permit lists apart. campfire permits `:user` four
/// times — three distinct lists — and folding them to one class silently
/// dropped `email_address` / `password` from the first-run signup.
/// Taking the union instead is not an option: it would let
/// `Accounts::BotsController` mass-assign `password`.
#[derive(Clone, Debug, Default)]
pub struct ParamsSpecs {
    specs: Vec<ParamsSpec>,
    /// Resources a model is named after (with a table), so the model
    /// lowering gives it the typed `update_from_<x>` variants. `None`
    /// until `typed_update::mark_model_resources` is told the models: a
    /// caller that lowers controllers without them keeps every rename.
    pub(super) model_resources: Option<crate::det::HashSet<Symbol>>,
}

impl ParamsSpecs {
    pub fn iter(&self) -> std::slice::Iter<'_, ParamsSpec> {
        self.specs.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// The spec a call site's own `(resource, fields)` names — the exact
    /// lookup every rewrite wants, since `match_permit_call` returns
    /// both halves of the key.
    pub fn find(&self, resource: &Symbol, fields: &[Symbol], nested: &[String]) -> Option<&ParamsSpec> {
        self.specs
            .iter()
            .find(|s| &s.resource == resource && s.key_fields == fields && s.nested_signature == nested)
    }

    pub fn by_class(&self, class_id: &ClassId) -> Option<&ParamsSpec> {
        self.specs.iter().find(|s| &s.class_id == class_id)
    }

    pub fn for_resource<'a>(
        &'a self,
        resource: &'a Symbol,
    ) -> impl Iterator<Item = &'a ParamsSpec> + 'a {
        self.specs.iter().filter(move |s| &s.resource == resource)
    }

    /// The spec holding the unqualified `<Resource>Params` name, if any.
    pub fn canonical<'a>(&'a self, resource: &Symbol) -> Option<&'a ParamsSpec> {
        self.specs
            .iter()
            .find(|s| &s.resource == resource && s.is_canonical)
    }

    /// Mark each spec's `has_one_attached` fields (see
    /// `ParamsSpec::file_fields`). The resource names its model the way
    /// `scan_create_demand` matches them — `snake_case(Model) ==
    /// resource` — so `:user` finds `User` and a list permitting
    /// `:avatar` on it types that field as a file.
    pub fn mark_file_fields(&mut self, models: &[crate::dialect::Model]) {
        for spec in &mut self.specs {
            let Some(model) = models
                .iter()
                .find(|m| crate::naming::snake_case(m.name.0.as_str()) == spec.resource.as_str())
            else {
                continue;
            };
            for (_span, attr) in crate::lower::attached::attached_attrs(model) {
                if spec.fields.contains(&attr) {
                    spec.file_fields.insert(attr);
                }
            }
        }
    }

    /// Carry each spec's nested candidates as fields (`on`, the ruby
    /// family, whose runtime reads them with `Params.nested`), or leave
    /// them dropped; `warn` ledgers each dropped one, which the
    /// controller-side lowering does once per target.
    pub fn carry_nested(&mut self, on: bool, warn: bool) {
        for spec in &mut self.specs {
            spec.ruby_values = on;
            for candidate in spec.nested_candidates.clone() {
                if on {
                    if !spec.fields.contains(&candidate.name) {
                        spec.fields.push(candidate.name.clone());
                    }
                    spec.nested_fields.insert(candidate.name.clone(), candidate);
                } else if warn {
                    warn_nested_keys(&spec.resource, std::slice::from_ref(&candidate.name), &candidate.controller, candidate.span);
                }
            }
        }
    }
}

/// A spec's nested identity: each candidate's name, mode and allowed keys.
fn signature_of(candidates: &[NestedKey]) -> Vec<String> {
    candidates.iter().map(|c| format!("{}:{}:{}", c.name.as_str(), c.mode, c.allowed.join(","))).collect()
}

/// The nested identity of the permit list at (or under) `expr`: what a
/// call site passes to [`ParamsSpecs::find`] with its scalar fields.
pub(crate) fn nested_signature(expr: &Expr) -> Vec<String> {
    signature_of(&nested_candidates(expr, &ClassId(Symbol::from(""))))
}

/// The non-scalar keys of the permit / expect list at (or under) `expr`
/// whose filter `Params.nested` reproduces: a flat list of Symbols
/// (`k: [:a, :b]`; "any" under `permit`, "hash" under `expect`), that
/// list wrapped once (`k: [[:a, :b]]`, "list"), or `k: {}` ("deep");
/// and `k: []` ("scalars"), which `Params.scalar_list` reads.
pub(crate) fn nested_candidates(expr: &Expr, controller: &ClassId) -> Vec<NestedKey> {
    fn find_call(e: &Expr) -> Option<(&str, &[Expr])> {
        let ExprNode::Send { recv, method, args, .. } = &*e.node else { return None };
        if matches!(method.as_str(), "permit" | "expect") {
            return Some((method.as_str(), args));
        }
        let mut found = None;
        if let Some(r) = recv {
            found = find_call(r);
        }
        if found.is_none() {
            for a in args {
                if let Some(f) = find_call(a) {
                    found = Some(f);
                    break;
                }
            }
        }
        found
    }
    let Some((method, args)) = find_call(expr) else { return Vec::new() };
    // The list the keys live in: `permit`'s own args, or the Array under
    // `expect(resource: [...])`.
    let list: Vec<&Expr> = if method == "expect" {
        let Some(ExprNode::Hash { entries, .. }) = args.first().map(|a| &*a.node) else { return Vec::new() };
        let Some((_, v)) = entries.first() else { return Vec::new() };
        let ExprNode::Array { elements, .. } = &*v.node else { return Vec::new() };
        elements.iter().collect()
    } else {
        match args {
            [one] => match &*one.node {
                ExprNode::Array { elements, .. } => elements.iter().collect(),
                _ => vec![one],
            },
            _ => args.iter().collect(),
        }
    };
    let Some(ExprNode::Hash { entries, .. }) = list.last().map(|e| &*e.node) else { return Vec::new() };
    let syms = |elements: &[Expr]| -> Option<Vec<String>> {
        elements.iter().map(|e| sym_of(e).map(|s| s.as_str().to_string())).collect()
    };
    let mut out = Vec::new();
    for (k, v) in entries {
        let Some(name) = sym_of(k) else { continue };
        let (allowed, mode) = match &*v.node {
            ExprNode::Hash { entries, .. } if entries.is_empty() => (Vec::new(), "deep"),
            ExprNode::Array { elements, .. } => match elements.as_slice() {
                [] => (Vec::new(), "scalars"),
                [inner] if matches!(&*inner.node, ExprNode::Array { .. }) => {
                    let ExprNode::Array { elements, .. } = &*inner.node else { continue };
                    match syms(elements) {
                        Some(a) if !a.is_empty() => (a, "list"),
                        _ => continue,
                    }
                }
                _ => match syms(elements) {
                    Some(a) => (a, if method == "expect" { "hash" } else { "any" }),
                    None => continue,
                },
            },
            _ => continue,
        };
        out.push(NestedKey { name, allowed, mode, controller: controller.clone(), span: expr.span });
    }
    out
}

impl ParamsSpec {
    fn value_demand(&self) -> bool {
        ["key?", "include?", "delete"].iter().any(|method|
            self.lookup_methods.contains(&Symbol::from(*method)))
    }

    pub(crate) fn scalar_lookup(&self) -> bool {
        self.value_demand() && !self.lookup_blocked && self.file_fields.is_empty()
    }

    pub(crate) fn value_map(&self) -> bool {
        self.ruby_values && self.scalar_lookup()
    }

    pub(super) fn has_nested_value(&self, key: &Expr) -> bool {
        if !self.value_map() { return false; }
        let name = match &*key.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
            ExprNode::Lit { value: Literal::Str { value } } => value.as_str(),
            _ => return false,
        };
        self.nested_fields.keys().any(|field| field.as_str() == name)
    }
}

/// The type a permitted field carries on the synthesized class: a
/// String — the request's own shape — or, for a `has_one_attached`
/// field, the uploaded file a multipart part became (nil when the
/// request carried none).
pub(crate) fn field_ty(spec: &ParamsSpec, field: &Symbol) -> Ty {
    if let Some(written) = merged_only_ty(spec, field) {
        return written;
    }
    let base = request_field_ty(spec, field);
    let written = match spec.written.get(field) {
        Some(written) if spec.ruby_values => written,
        _ => return base,
    };
    let mut variants: Vec<Ty> = match base {
        Ty::Union { variants } => variants.iter().cloned().collect(),
        other => vec![other],
    };
    for ty in written {
        if !variants.contains(ty) {
            variants.push(ty.clone());
        }
    }
    if variants.len() == 1 {
        variants.pop().unwrap()
    } else {
        Ty::Union { variants: variants.into() }
    }
}

/// A merged-only key's type on the Ruby family: what the merges write, or
/// nil before one runs.
fn merged_only_ty(spec: &ParamsSpec, field: &Symbol) -> Option<Ty> {
    if !spec.ruby_values || !spec.merged_only.contains(field) {
        return None;
    }
    let written = spec.written.get(field).filter(|w| !w.is_empty())?;
    let mut variants = written.clone();
    if !variants.contains(&Ty::Nil) {
        variants.push(Ty::Nil);
    }
    Some(Ty::Union { variants: variants.into() })
}

fn request_field_ty(spec: &ParamsSpec, field: &Symbol) -> Ty {
    if let Some(nested) = spec.nested_fields.get(field) {
        if nested.mode == "scalars" {
            Ty::Array { elem: Ty::Str.into() }
        } else if nested.mode == "list" {
            let record = Ty::Hash { key: Ty::Str.into(), value: param_value_ty().into() };
            Ty::Union { variants: vec![
                Ty::Array { elem: record.clone().into() },
                Ty::Hash { key: Ty::Str.into(), value: record.into() },
                Ty::Nil,
            ].into() }
        } else {
            param_value_ty()
        }
    } else if spec.file_fields.contains(field) {
        Ty::Union { variants: vec![uploaded_file_ty(), Ty::Nil].into() }
    } else {
        Ty::Str
    }
}

pub(crate) fn uploaded_file_class() -> ClassId {
    ClassId(Symbol::from("ActionDispatch::Http::UploadedFile"))
}

fn uploaded_file_ty() -> Ty {
    Ty::Class { id: uploaded_file_class(), args: vec![].into() }
}

/// The two reads a field takes off a params hash — was it provided,
/// and what is it — as `(provided, value)`. A String field goes through
/// `Params` (runtime/ruby/params.rb); a file field through the ruby
/// family's `ActionDispatch::Http::UploadedFile.provided` /
/// `.from_params`, the one narrowing of a params value to that class,
/// kept out of `Params` because the strict targets' `ParamValue` union
/// has no file arm (see the class's own header in
/// runtime/spinel/multipart.rb). `sub` is the hash to read — the
/// resource sub-hash in `from_raw`, the raw params in a helper that
/// permits at the top level.
fn field_reads(spec: &ParamsSpec, field: &Symbol, sub: Expr, span: Span) -> (Expr, Expr) {
    use crate::lower::typing::with_ty;
    let str_lit = |v: &str| {
        with_ty(
            Expr::new(span, ExprNode::Lit { value: Literal::Str { value: v.to_string() } }),
            Ty::Str,
        )
    };
    let call = |recv: Vec<Symbol>, method: &str, args: Vec<Expr>, ret: Ty| {
        with_ty(
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(span, ExprNode::Const { path: recv })),
                    method: Symbol::from(method),
                    args,
                    block: None,
                    parenthesized: true,
                },
            ),
            ret,
        )
    };
    if spec.nested_fields.get(field).is_some_and(|n| n.mode == "scalars") {
        let params = vec![Symbol::from("Params")];
        let args = vec![sub, str_lit(field.as_str())];
        return (
            call(params.clone(), "scalar_list_provided", args.clone(), Ty::Bool),
            call(params, "scalar_list", args, field_ty(spec, field)),
        );
    }
    if let Some(nested) = spec.nested_fields.get(field) {
        let allowed = with_ty(
            Expr::new(
                span,
                ExprNode::Array {
                    elements: nested.allowed.iter().map(|a| str_lit(a)).collect(),
                    style: crate::expr::ArrayStyle::default(),
                },
            ),
            Ty::Array { elem: Ty::Str.into() },
        );
        let args = vec![sub, str_lit(field.as_str()), allowed, str_lit(nested.mode)];
        let params = vec![Symbol::from("Params")];
        let value = if nested.mode == "list" {
            call(params.clone(), "record_list", args[..3].to_vec(), field_ty(spec, field))
        } else {
            call(params.clone(), "nested", args.clone(), param_value_ty())
        };
        return (
            call(params.clone(), "nested_provided", args.clone(), Ty::Bool),
            value,
        );
    }
    if spec.file_fields.contains(field) {
        let uploaded: Vec<Symbol> =
            uploaded_file_class().0.as_str().split("::").map(Symbol::from).collect();
        (
            call(uploaded.clone(), "provided", vec![sub.clone(), str_lit(field.as_str())], Ty::Bool),
            call(uploaded, "from_params", vec![sub, str_lit(field.as_str())], field_ty(spec, field)),
        )
    } else {
        // `scalar_provided` / `scalar`: the field's own key, or one of
        // its multiparameter keys (`password(1)`), which Rails' `permit`
        // keeps too — see runtime/ruby/params.rb.
        let params = vec![Symbol::from("Params")];
        (
            call(params.clone(), "scalar_provided", vec![sub.clone(), str_lit(field.as_str())], Ty::Bool),
            call(params, "scalar", vec![sub, str_lit(field.as_str())], Ty::Str),
        )
    }
}

/// The params object's record of the request's multiparameter keys
/// (`name(1)`) among its scalar fields: which fields they arrived for,
/// and whether any came in a shape whose Rails value is not reproduced
/// (`runtime/ruby/params.rb`). The model's `from_params` and
/// `update_from_*` read both, and raise rather than write a value Rails
/// would not.
pub fn multiparameter_fields_field() -> Symbol {
    Symbol::from("rh_multiparameter_fields")
}

pub fn multiparameter_unsupported_field() -> Symbol {
    Symbol::from("rh_multiparameter_unsupported")
}

fn multiparameter_stmts(spec: &ParamsSpec, recv: impl Fn() -> Expr, sub: impl Fn() -> Expr, span: Span) -> Vec<Expr> {
    use crate::lower::typing::with_ty;
    let names = || {
        let elements: Vec<Expr> = spec
            .fields
            .iter()
            .filter(|f| !spec.file_fields.contains(f))
            .map(|f| {
                with_ty(
                    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: f.as_str().to_string() } }),
                    Ty::Str,
                )
            })
            .collect();
        with_ty(
            Expr::new(span, ExprNode::Array { elements, style: Default::default() }),
            Ty::Array { elem: std::sync::Arc::new(Ty::Str) },
        )
    };
    let params_call = |method: &str, ret: Ty| {
        with_ty(
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] })),
                    method: Symbol::from(method),
                    args: vec![sub(), names()],
                    block: None,
                    parenthesized: true,
                },
            ),
            ret,
        )
    };
    let set = |field: Symbol, value: Expr| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(recv()),
                method: Symbol::from(format!("{}=", field.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        )
    };
    vec![
        set(
            multiparameter_fields_field(),
            params_call("multiparameter_fields", Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
        ),
        set(multiparameter_unsupported_field(), params_call("multiparameter_unsupported", Ty::Bool)),
    ]
}

/// Walk every controller's action bodies and collect one ParamsSpec per
/// distinct `(resource, fields)` pair.
pub fn collect_specs(controllers: &[Controller]) -> ParamsSpecs {
    collect_specs_impl(controllers, true)
}

fn collect_specs_impl(controllers: &[Controller], report: bool) -> ParamsSpecs {
    let mut index: BTreeMap<(Symbol, Vec<Symbol>, Vec<String>), usize> = BTreeMap::new();
    let mut specs: Vec<ParamsSpec> = Vec::new();
    for c in controllers {
        for action in c.actions() {
            collect_from_expr(&action.body, &c.name, &mut index, &mut specs, report);
        }
    }
    assign_class_ids(&mut specs);
    let mut specs = ParamsSpecs { specs, ..Default::default() };
    scan_create_demand(controllers, &mut specs);
    scan_lookup_demand(controllers, &mut specs);
    specs
}

pub(crate) fn lower_value_reads(app: &mut crate::app::App) {
    let mut specs = collect_specs_impl(&app.controllers, false);
    specs.mark_file_fields(&app.models);
    for controller in &mut app.controllers {
        let helpers: Vec<_> = controller.actions().cloned().collect();
        for action in controller.actions_mut() {
            action.body = rewrite_nested_lookups(&action.body, &helpers, &specs);
        }
    }
}

fn params_write(expr: &Expr) -> Option<(&Expr, &Expr, &Expr)> {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => Some((recv, index, value)),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "[]=" && args.len() == 2 => Some((recv, &args[0], &args[1])),
        _ => None,
    }
}

fn literal_key(key: &Expr) -> Option<Symbol> {
    match &*key.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

// An unknown type is not widened into the field: its RBS would no longer say what the record holds.
fn written_ty(value: &Expr) -> Option<Ty> {
    value.ty.clone().filter(|ty| !ty.mentions_unknown())
}

fn report_params_write(span: Span, controller: &ClassId, class: &ClassId, why: &str, strict: bool) {
    let detail = format!("`[]=` on the permitted params of `{}` (a synthesized {}) {why}; Rails' Parameters accepts it", controller.0, class.0);
    let mut d = crate::diagnostic::Diagnostic::unsupported(span, None, "params write", detail.clone());
    if strict {
        d.message = format!("params write not supported (strict-typed targets): {detail}");
    }
    crate::emit::diagnostics::push(d);
}

pub(super) fn rewrite_params_writes(
    body: &Expr,
    helpers: &[crate::dialect::Action],
    specs: &ParamsSpecs,
    controller: &ClassId,
) -> Expr {
    let helpers = helper_spec_map(helpers, specs).into_iter()
        .map(|(name, spec)| (name, spec.class_id.clone())).collect();
    let sources = super::params_sources::ParamsSources::new(body, helpers, specs);
    rewrite_params_writes_with(body, &sources, specs, controller)
}

fn rewrite_params_writes_with(
    body: &Expr,
    sources: &super::params_sources::ParamsSources,
    specs: &ParamsSpecs,
    controller: &ClassId,
) -> Expr {
    map_expr(body, &|expr| {
        let (recv, key, value) = params_write(expr)?;
        let class = sources.class(recv, specs)?;
        let spec = specs.by_class(&class)?;
        let field = literal_key(key).filter(|k| spec.fields.contains(k));
        let why = if !spec.ruby_values {
            Some("is not lowered on this target, whose record fields keep the type the request delivers (the Ruby, JRuby and Spinel trees assign it)")
        } else if field.is_none() {
            Some("names no permitted key, and the record has a slot only per permitted key")
        } else if written_ty(value).is_none() {
            Some("stores a value whose type is not known, and the record's field must carry it")
        } else if !matches!(&*recv.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
            Some("writes through a receiver that is not a local or an instance variable")
        } else {
            None
        };
        if let Some(why) = why {
            report_params_write(expr.span, controller, &class, why, !spec.ruby_values);
            return None;
        }
        let field = field?;
        let value = rewrite_params_writes_with(value, sources, specs, controller);
        let setter = |name: String, arg: Expr| Expr::new(expr.span, ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from(name),
            args: vec![arg],
            block: None,
            parenthesized: false,
        });
        let provided = with_ty(
            Expr::new(expr.span, ExprNode::Lit { value: Literal::Bool { value: true } }),
            Ty::Bool,
        );
        Some(Expr::new(expr.span, ExprNode::Seq { exprs: vec![
            setter(format!("{}=", provided_field(&field).as_str()), provided),
            setter(format!("{}=", field.as_str()), value),
        ] }))
    })
}

fn scan_lookup_demand(controllers: &[Controller], specs: &mut ParamsSpecs) {
    let mut calls = Vec::new();
    let mut blocked = Vec::new();
    let mut written: Vec<(ClassId, Symbol, Ty)> = Vec::new();
    let mut merged: Vec<(ClassId, Symbol)> = Vec::new();
    let mut direct: Vec<(ClassId, Symbol)> = Vec::new();
    for controller in controllers {
        let actions: Vec<_> = controller.actions().cloned().collect();
        let helpers: BTreeMap<Symbol, ClassId> = helper_spec_map(&actions, specs).into_iter()
            .map(|(name, spec)| (name, spec.class_id.clone())).collect();
        for action in actions {
            let sources = super::params_sources::ParamsSources::new(
                &action.body, helpers.clone(), specs,
            );
            walk_all(&action.body, &mut |expr| {
                if let Some((recv, key, value)) = params_write(expr) {
                    let class = sources.class(recv, specs);
                    if let (Some(class), Some(key), Some(ty)) = (class, literal_key(key), written_ty(value)) {
                        written.push((class, key, ty));
                    }
                }
                // A permit that names the key itself makes it a request key for the spec every call shares.
                if let ExprNode::Send { method, .. } = &*expr.node
                    && method.as_str() != "merge"
                    && let Some((resource, fields, _)) = match_permit_call_full(expr)
                    && let Some(spec) = specs.find(&resource, &fields, &nested_signature(expr))
                {
                    direct.extend(fields.into_iter().map(|f| (spec.class_id.clone(), f)));
                }
                if let ExprNode::Send { recv: Some(chain), method, args, block: None, .. } = &*expr.node
                    && method.as_str() == "merge"
                    && let [arg] = args.as_slice()
                    && let ExprNode::Hash { entries, .. } = &*arg.node
                    && let Some((_, permitted, _)) = match_permit_call_full(chain)
                    && let Some(class) = match_permit_call_full(expr).and_then(|(resource, fields, _)| {
                        specs.find(&resource, &fields, &nested_signature(expr)).map(|spec| spec.class_id.clone())
                    })
                {
                    for (k, v) in entries {
                        if let (Some(key), Some(ty)) = (literal_key(k), written_ty(v))
                            && !permitted.contains(&key)
                        {
                            merged.push((class.clone(), key.clone()));
                            written.push((class.clone(), key, ty));
                        }
                    }
                }
                if let ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } = &*expr.node {
                    let recv = match target {
                        LValue::Attr { recv, .. } | LValue::Index { recv, .. } => Some(recv),
                        _ => None,
                    };
                    if let Some(class) = recv.and_then(|recv| sources.class(recv, specs)) {
                        blocked.push(class);
                    }
                }
                let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
                    return;
                };
                let class = sources.class(recv, specs).or_else(|| {
                    match_reverse_merge(expr).or_else(|| match_permit_call_full(expr))
                        .and_then(|(resource, fields, _)| {
                        specs.find(&resource, &fields, &nested_signature(expr)).map(|spec| spec.class_id.clone())
                    })
                });
                let Some(class) = class else { return };
                if matches!(method.as_str(), "key?" | "include?" | "delete")
                    && args.len() == 1 && block.is_none()
                {
                    calls.push((class, method.clone()));
                } else if (method.as_str() == "compact" && args.is_empty() && block.is_none())
                    || (method.as_str() == "except" && block.is_none())
                {
                    calls.push((class, method.clone()));
                } else if matches!(method.as_str(), "merge" | "merge!"
                    | "reverse_merge" | "reverse_merge!" | "delete" | "[]=")
                    || (method.as_str().ends_with('=')
                        && !matches!(method.as_str(), "==" | "===" | "!=" | "<=" | ">="))
                {
                    if method.as_str() == "delete" { calls.push((class.clone(), method.clone())); }
                    blocked.push(class);
                }
            });
        }
    }
    for (class, method) in calls {
        if let Some(spec) = specs.specs.iter_mut().find(|spec| spec.class_id == class) {
            spec.lookup_methods.insert(method);
        }
    }
    for class in blocked {
        if let Some(spec) = specs.specs.iter_mut().find(|spec| spec.class_id == class) {
            spec.lookup_blocked = true;
        }
    }
    for (class, key) in merged.into_iter().filter(|m| !direct.contains(m)) {
        if let Some(spec) = specs.specs.iter_mut().find(|spec| spec.class_id == class) {
            spec.merged_only.insert(key);
        }
    }
    for (class, key, ty) in written {
        if let Some(spec) = specs.specs.iter_mut().find(|spec| spec.class_id == class) {
            let tys = spec.written.entry(key).or_default();
            if !tys.contains(&ty) {
                tys.push(ty);
            }
        }
    }
}

/// Second phase: which specs a `<Model>.create(<helper>)` call site
/// actually asks for. Can't ride the first walk — resolving `<helper>`
/// to a spec needs every spec named first.
///
/// Only a receiver naming THIS list's own model counts, which is the
/// same condition the model lowerer synthesizes under: `Boost.from_params`
/// exists on `Boost` because `:boost` is its resource, and
/// `Membership.create(boost_params)` has no typed factory to reach.
fn scan_create_demand(controllers: &[Controller], specs: &mut ParamsSpecs) {
    let mut demand: Vec<(ClassId, bool)> = Vec::new();
    let mut to_attrs: Vec<ClassId> = Vec::new();
    let mut as_json: Vec<ClassId> = Vec::new();
    let mut except: Vec<ClassId> = Vec::new();
    let mut assign: Vec<ClassId> = Vec::new();
    // Read-only for the whole walk; the demand lists are applied
    // after it, which is what keeps this a shared borrow.
    let specs_ro: &ParamsSpecs = specs;
    for c in controllers {
        let actions: Vec<crate::dialect::Action> = c.actions().cloned().collect();
        let helpers = helper_spec_map(&actions, specs_ro);
        // NO `helpers.is_empty()` skip: a controller that writes the
        // permit chain inline (lobsters' `HatsController#approve_request`)
        // has no `<x>_params` helper at all, and its `to_attrs` demand is
        // exactly the one that has to be counted.
        let helper_classes: BTreeMap<Symbol, ClassId> =
            helpers.iter().map(|(n, s)| (n.clone(), s.class_id.clone())).collect();
        for action in &actions {
            let sources = super::params_sources::ParamsSources::new(
                &action.body, helper_classes.clone(), specs_ro,
            );
            walk_all(&action.body, &mut |e| {
                if let ExprNode::Assign { .. } = &*e.node {
                    if let Some((_, value)) = attributes_assignment(e) {
                        if let Some(class) = assigned_params_class(value, &|v| sources.class(v, specs_ro)) {
                            assign.push(class);
                        }
                    }
                }
                let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
                    return;
                };
                if method.as_str() == "except" {
                    if let Some(class) = sources.class(recv, specs_ro) {
                        except.push(class);
                    }
                }
                if let Some((_, value)) = attributes_assignment(e) {
                    if let Some(class) = assigned_params_class(value, &|v| sources.class(v, specs_ro)) {
                        assign.push(class);
                    }
                }
                // `<params-source>.to_attrs` — the attribute-hash
                // conversion `params_merge` writes at a call site whose
                // callee takes a hash, not a params object.
                if method.as_str() == "to_attrs" && args.is_empty() {
                    if let Some(class) = sources.class(recv, specs_ro) {
                        to_attrs.push(class);
                    }
                    return;
                }
                // `<params-source>.as_json`: Rails' `Parameters#as_json`.
                if method.as_str() == "as_json" && args.is_empty() {
                    if let Some(class) = sources.class(recv, specs_ro) {
                        as_json.push(class);
                    }
                    return;
                }
                let bang = match method.as_str() {
                    "create" => false,
                    "create!" => true,
                    _ => return,
                };
                if args.len() != 1 {
                    return;
                }
                let ExprNode::Const { path } = &*recv.node else { return };
                let Some(model) = path.last() else { return };
                let ExprNode::Send { recv: None, method: h, args: hargs, block: None, .. } =
                    &*args[0].node
                else {
                    return;
                };
                if !hargs.is_empty() {
                    return;
                }
                let Some(spec) = helpers.get(h) else { return };
                if crate::naming::snake_case(model.as_str()) != spec.resource.as_str() {
                    return;
                }
                demand.push((spec.class_id.clone(), bang));
            });
        }
    }
    for (class_id, bang) in demand {
        if let Some(spec) = specs.specs.iter_mut().find(|s| s.class_id == class_id) {
            if bang {
                spec.wants_create_bang = true;
            } else {
                spec.wants_create = true;
            }
        }
    }
    for class_id in to_attrs {
        if let Some(spec) = specs.specs.iter_mut().find(|s| s.class_id == class_id) {
            spec.wants_to_attrs = true;
        }
    }
    for class_id in as_json {
        if let Some(spec) = specs.specs.iter_mut().find(|s| s.class_id == class_id) {
            spec.wants_as_json = true;
        }
    }
    for class_id in except {
        if let Some(spec) = specs.specs.iter_mut().find(|s| s.class_id == class_id) {
            spec.wants_except = true;
        }
    }
    for class_id in assign {
        if let Some(spec) = specs.specs.iter_mut().find(|s| s.class_id == class_id) {
            spec.wants_assign = true;
        }
    }
}

pub(super) fn attributes_assignment(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Attr { recv, name }, value } if name.as_str() == "attributes" => {
            Some((recv, value))
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if matches!(method.as_str(), "attributes=" | "assign_attributes") && args.len() == 1 =>
        {
            Some((recv, &args[0]))
        }
        _ => None,
    }
}

pub(super) fn assigned_params_class(
    value: &Expr,
    class_of: &dyn Fn(&Expr) -> Option<ClassId>,
) -> Option<ClassId> {
    if let ExprNode::If { then_branch, else_branch, .. } = &*value.node {
        let yes = assigned_params_class(then_branch, class_of)?;
        let no = assigned_params_class(else_branch, class_of)?;
        return (yes == no).then_some(yes);
    }
    class_of(value)
}

fn walk_all<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(expr);
    expr.node.for_each_child(&mut |c| walk_all(c, f));
}

/// Build specs straight from `(resource, fields)` pairs, for callers
/// with no controller bodies to scan (tests, synthetic apps). Each
/// resource gets one list, so every spec keeps the unqualified name.
pub fn specs_from_lists(lists: &[(Symbol, Vec<Symbol>)]) -> ParamsSpecs {
    ParamsSpecs {
        specs: lists
            .iter()
            .map(|(resource, fields)| ParamsSpec {
                class_id: params_class_id(resource),
                resource: resource.clone(),
                fields: fields.clone(),
                span: Span::synthetic(),
                wants_except: false,
                wants_create: false,
                wants_create_bang: false,
                wants_to_attrs: false,
                wants_as_json: false,
                lookup_methods: Default::default(),
                lookup_blocked: false,
                ruby_values: false,
                declaring: Vec::new(),
                is_canonical: true,
                file_fields: std::collections::BTreeSet::new(),
                key_fields: fields.clone(),
                nested_signature: Vec::new(),
                nested_candidates: Vec::new(),
                nested_fields: BTreeMap::new(),
                written: BTreeMap::new(),
                merged_only: Default::default(),
                wants_assign: false,
            })
            .collect(),
        model_resources: None,
    }
}

/// Record one recognized permit list, folding it into an existing spec
/// when an identical `(resource, fields)` pair was already seen.
/// What a call site chained off the permit, and therefore what the
/// synthesized class has to grow.
#[derive(Clone, Copy, Default)]
struct Wants {
    except: bool,
}

fn record(
    resource: Symbol,
    fields: Vec<Symbol>,
    controller: &ClassId,
    span: Span,
    wants: Wants,
    candidates: Vec<NestedKey>,
    index: &mut BTreeMap<(Symbol, Vec<Symbol>, Vec<String>), usize>,
    specs: &mut Vec<ParamsSpec>,
) {
    let key = (resource.clone(), fields.clone(), signature_of(&candidates));
    match index.get(&key) {
        Some(&i) => {
            specs[i].wants_except |= wants.except;
            if !specs[i].declaring.contains(controller) {
                specs[i].declaring.push(controller.clone());
            }
        }
        None => {
            index.insert(key, specs.len());
            specs.push(ParamsSpec {
                // Filled in by `assign_class_ids` once every list is
                // known — a name can't be chosen until we know whether
                // the resource carries one list or several.
                class_id: ClassId(Symbol::from("")),
                resource,
                fields: fields.clone(),
                span,
                wants_except: wants.except,
                wants_create: false,
                wants_create_bang: false,
                wants_to_attrs: false,
                wants_as_json: false,
                lookup_methods: Default::default(),
                lookup_blocked: false,
                ruby_values: false,
                declaring: vec![controller.clone()],
                is_canonical: false,
                file_fields: std::collections::BTreeSet::new(),
                key_fields: fields.clone(),
                nested_signature: signature_of(&candidates),
                nested_candidates: candidates,
                nested_fields: BTreeMap::new(),
                written: BTreeMap::new(),
                merged_only: Default::default(),
                wants_assign: false,
            });
        }
    }
}

fn collect_from_expr(
    expr: &Expr,
    controller: &ClassId,
    index: &mut BTreeMap<(Symbol, Vec<Symbol>, Vec<String>), usize>,
    specs: &mut Vec<ParamsSpec>,
    report: bool,
) {
    // `<permit-chain>.except(:key)` / `.compact` — mark the spec before
    // the walk reaches the inner chain, so the flag survives the fold in
    // `record`.
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node {
        let wants = match method.as_str() {
            "except" => Wants { except: true },
            _ => Wants::default(),
        };
        if wants.except {
            if let Some((resource, fields)) = match_permit_call(recv) {
                let candidates = nested_candidates(recv, controller);
                record(resource, fields, controller, expr.span, wants, candidates, index, specs);
            }
        }
    }
    let found = match_reverse_merge(expr).or_else(|| match_permit_call_full(expr));
    if let Some((resource, fields, nested)) = found {
        let candidates = nested_candidates(expr, controller);
        // A key no candidate reproduces stays dropped wherever it lowers;
        // a candidate's fate is the target's (`ParamsSpecs::carry_nested`).
        let unsupported: Vec<Symbol> =
            nested.into_iter().filter(|k| !candidates.iter().any(|c| &c.name == k)).collect();
        if report { warn_nested_keys(&resource, &unsupported, controller, expr.span); }
        let key = (resource.clone(), fields.clone(), signature_of(&candidates));
        record(resource, fields, controller, expr.span, Wants::default(), candidates, index, specs);
        if !unsupported.is_empty() {
            specs[index[&key]].lookup_blocked = true;
        }
        // Stop here. The merge form (`permit(...).merge(k: v)`) matched
        // this node with the WIDER field set; recursing would reach the
        // inner bare permit and — now that specs key on the field list —
        // register a second, narrower class for the same call site.
        return;
    }
    walk_children(expr, &mut |c| collect_from_expr(c, controller, index, specs, report));
}

/// Record the NON-SCALAR permit keys this pass drops.
///
/// One warning per (controller, key): the ledger entry for a field the
/// request may send and the emitted app will not assign. Deliberately
/// not an error — the rest of the list lowers, which is strictly more
/// than the whole helper staying dynamic (see
/// [`match_permit_call_full`]) — and deliberately not silent, because
/// Rails WOULD assign it.
fn warn_nested_keys(resource: &Symbol, nested: &[Symbol], controller: &ClassId, span: Span) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};

    for key in nested {
        let kind = DiagnosticKind::LowerResidue {
            pass: Symbol::from("params_nested_filter"),
            construct: Symbol::from("permit"),
            reason: Symbol::from("non-scalar key"),
        };
        let d = Diagnostic {
            span,
            severity: Diagnostic::default_severity(&kind),
            kind,
            message: format!(
                "permitted key `{key}` on `{resource}` in `{controller}` is \
                 non-scalar (`{key}: []` or `{key}: {{}}`) — a synthesized params \
                 record carries String fields only, so this key is dropped from the \
                 permit list and the emitted app will not assign it; the rest of the \
                 list still lowers",
                key = key.as_str(),
                resource = resource.as_str(),
                controller = controller.0.as_str(),
            ),
        };
        crate::emit::diagnostics::push(d);
    }
}

/// Name each spec, and decide which one owns the unqualified name.
///
/// A resource with a single permit list keeps `<Resource>Params`, so
/// nothing about a one-list-per-resource app changes. When a resource
/// carries several lists, the unqualified name goes to the list declared
/// by the controller the resource is named for (`UsersController` for
/// `:user`) — not to whichever controller sorted first, which in
/// campfire would have handed `UserParams` (and `User.from_params`) to
/// the bot-shaped list. The rest take their first declaring
/// controller's name as a prefix: `Accounts::BotsController` →
/// `AccountsBotsUserParams`. If no controller is named for the resource,
/// every list is qualified and the model keeps its untyped `update`.
fn assign_class_ids(specs: &mut [ParamsSpec]) {
    let mut by_resource: BTreeMap<Symbol, Vec<usize>> = BTreeMap::new();
    for (i, spec) in specs.iter().enumerate() {
        by_resource.entry(spec.resource.clone()).or_default().push(i);
    }
    for (resource, idxs) in &by_resource {
        let canonical = if idxs.len() == 1 {
            Some(idxs[0])
        } else {
            idxs.iter().copied().find(|&i| {
                specs[i]
                    .declaring
                    .iter()
                    .any(|c| controller_names_resource(c, resource))
            })
        };
        for &i in idxs {
            if Some(i) == canonical {
                specs[i].class_id = params_class_id(resource);
                specs[i].is_canonical = true;
            } else {
                let owner = specs[i].declaring[0].clone();
                specs[i].class_id = qualified_params_class_id(&owner, resource);
            }
        }
    }
    // Backstop: one controller declaring two different lists for the
    // same resource would qualify to the same name. Rare enough that a
    // positional suffix is a better answer than another naming rule.
    let mut taken: crate::det::HashSet<ClassId> = crate::det::HashSet::default();
    for spec in specs.iter_mut() {
        if taken.insert(spec.class_id.clone()) {
            continue;
        }
        for n in 2.. {
            let candidate = ClassId(Symbol::from(format!("{}{n}", spec.class_id.0.as_str())));
            if taken.insert(candidate.clone()) {
                spec.class_id = candidate;
                break;
            }
        }
    }
}

/// Is `controller` the one this resource is named for? `UsersController`
/// and `Accounts::UsersController` both answer yes for `:user`; the
/// namespace doesn't change what the controller is about.
fn controller_names_resource(controller: &ClassId, resource: &Symbol) -> bool {
    let last = crate::naming::last_segment(controller.0.as_str());
    let stem = crate::naming::snake_case(last.strip_suffix("Controller").unwrap_or(last));
    stem == resource.as_str() || crate::naming::singularize(&stem) == resource.as_str()
}

/// `Accounts::BotsController` + `:user` → `AccountsBotsUserParams`.
pub fn qualified_params_class_id(controller: &ClassId, resource: &Symbol) -> ClassId {
    let name = controller.0.as_str();
    let stem = name.strip_suffix("Controller").unwrap_or(name).replace("::", "");
    ClassId(Symbol::from(format!(
        "{stem}{}Params",
        camelize(resource.as_str())
    )))
}

/// The model factory a spec's class feeds. The canonical spec keeps
/// `from_params`; the rest name their class, since a strict target has
/// no overloading to lean on and the two classes are unrelated types.
pub fn model_from_params_name(spec: &ParamsSpec) -> Symbol {
    if spec.is_canonical {
        Symbol::from("from_params")
    } else {
        Symbol::from(format!(
            "from_{}",
            crate::naming::snake_case(spec.class_id.0.as_str())
        ))
    }
}

/// `create` / `create!` taking this spec's class — `from_params` plus a
/// save, named off the factory it wraps.
pub fn model_create_from_params_name(spec: &ParamsSpec, bang: bool) -> Symbol {
    Symbol::from(format!(
        "create_{}{}",
        model_from_params_name(spec).as_str(),
        if bang { "!" } else { "" }
    ))
}

/// Same rule for the typed `update` / `update!` pair.
pub fn model_update_name(spec: &ParamsSpec, bang: bool) -> Symbol {
    let bang = if bang { "!" } else { "" };
    // EVERY permit list gets a qualified name, canonical included. The
    // plain `update` / `update!` belong to Rails' attribute-Hash
    // contract, which is a different (wider, differently-typed) surface
    // than any mass-assignment boundary — see `synth_update_hash`.
    Symbol::from(format!(
        "update_from_{}{bang}",
        crate::naming::snake_case(spec.class_id.0.as_str())
    ))
}

pub fn model_assign_name(spec: &ParamsSpec) -> Symbol {
    Symbol::from(format!("assign_from_{}", crate::naming::snake_case(spec.class_id.0.as_str())))
}

/// First permit list in `expr`, in the same pre-order the collector
/// uses — the way a `<resource>_params` helper body names its spec.
pub fn first_permit_in(body: &Expr) -> Option<(Symbol, Vec<Symbol>, Vec<String>)> {
    first_permit_at(body)
}

fn first_permit_at(expr: &Expr) -> Option<(Symbol, Vec<Symbol>, Vec<String>)> {
    if let Some((resource, fields, _)) = match_reverse_merge(expr) {
        return Some((resource, fields, nested_signature(expr)));
    }
    if let Some((resource, fields)) = match_permit_call(expr) {
        return Some((resource, fields, nested_signature(expr)));
    }
    let mut found = None;
    walk_children(expr, &mut |c| {
        if found.is_none() {
            found = first_permit_at(c);
        }
    });
    found
}

/// Map each `<x>_params`-shaped helper to the spec its body declares.
/// Call-site rewrites (`Model.new(user_params)`, `@user.update
/// user_params`) see only the helper name, so this is how they reach
/// the right class when a resource carries several.
/// Rewrite an OVERRIDING `<x>_params` helper to yield the params class
/// its parent's helper yields.
///
/// A subclass may override the helper with a shape the permit
/// recognizer does not read — campfire's
/// `Messages::ByBotsController#message_params` is
///
/// ```text
/// if params[:attachment]
///   params.permit(:attachment)              # no `require`, so no resource
/// else
///   reading(request.body) { |body| { body: body } }   # a bare Hash
/// end
/// ```
///
/// Neither branch is a `require(:r).permit(...)` chain, so no spec was
/// recognized and the method emitted VERBATIM: one branch calling
/// `permit` on a Hash (undefined), the other returning a Hash the
/// parent's injected `.to_attrs` then died on. Both were broken; the
/// Hash one just failed first.
///
/// The class comes from the PARENT's helper of the same name, which is
/// also what Rails means: the callee consuming it
/// (`create_with_attachment!`) expects the same thing no matter which
/// subclass supplied it.
///
/// Construction is INLINED rather than routed through a new factory.
/// `from_raw` cannot serve: it opens with `Params.sub(params,
/// "<resource>")` because the recognized shape nests under
/// `require(:message)`, and these branches read the TOP level. Inlining
/// keeps each branch exact — `permit(:attachment)` sets attachment and
/// nothing else — where a shared flat factory would widen every such
/// call to the class's full field list.
///
/// Declines unless every key is a field of the class. A helper naming
/// something the parent never permitted is not the same list, and
/// silently dropping the extra would be mass-assignment by omission.
pub fn lower_overriding_params_helper(body: &Expr, spec: &ParamsSpec) -> Expr {
    rewrite_tail(body, spec)
}

/// Walk to every value the method can RETURN and rewrite it there.
/// Tail positions only: an intermediate `{ … }` is somebody's argument,
/// not the helper's product.
fn rewrite_tail(e: &Expr, spec: &ParamsSpec) -> Expr {
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut out = exprs.clone();
            let last = out.len() - 1;
            out[last] = rewrite_tail(&out[last], spec);
            Expr { node: Box::new(ExprNode::Seq { exprs: out }), ..e.clone() }
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr {
            node: Box::new(ExprNode::If {
                cond: cond.clone(),
                then_branch: rewrite_tail(then_branch, spec),
                else_branch: rewrite_tail(else_branch, spec),
            }),
            ..e.clone()
        },
        // `reading(request.body) { |body| … }` — the helper's value is
        // the BLOCK's, so the rewrite belongs inside it.
        ExprNode::Send { recv, method, args, block: Some(b), parenthesized } => {
            let ExprNode::Lambda { rest_param, extra_params, params, block_param, body, block_style } = &*b.node else {
                return e.clone();
            };
            let new_block = Expr {
                node: Box::new(ExprNode::Lambda { rest_param: rest_param.clone(), extra_params: extra_params.clone(),
                    params: params.clone(),
                    block_param: block_param.clone(),
                    body: rewrite_tail(body, spec),
                    block_style: *block_style,
                }),
                ..b.clone()
            };
            Expr {
                node: Box::new(ExprNode::Send {
                    recv: recv.clone(),
                    method: method.clone(),
                    args: args.clone(),
                    block: Some(new_block),
                    parenthesized: *parenthesized,
                }),
                ..e.clone()
            }
        }
        ExprNode::Hash { entries, kwargs: _ } => {
            let mut pairs: Vec<(Symbol, Expr)> = Vec::new();
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
                    return e.clone();
                };
                if !spec.fields.contains(value) {
                    return e.clone();
                }
                pairs.push((value.clone(), v.clone()));
            }
            if pairs.is_empty() {
                return e.clone();
            }
            build_params_object(e, spec, &pairs, /*from_literal=*/ true)
        }
        ExprNode::Send { method, args, block: None, .. } if method.as_str() == "permit" => {
            let mut pairs: Vec<(Symbol, Expr)> = Vec::new();
            for a in args {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else {
                    return e.clone();
                };
                if !spec.fields.contains(value) {
                    return e.clone();
                }
                pairs.push((value.clone(), a.clone()));
            }
            if pairs.is_empty() {
                return e.clone();
            }
            build_params_object(e, spec, &pairs, /*from_literal=*/ false)
        }
        _ => e.clone(),
    }
}

/// ```text
/// __params = <Class>.new
/// __params.<k>_provided = <true | Params.provided(@params, "k")>
/// __params.<k>         = <value | Params.str(@params, "k", "")>
/// __params
/// ```
///
/// Presence before value, matching `from_raw`'s ordering so the two read
/// the same way. A literal's key is provided BY BEING WRITTEN, which is
/// why that arm is a bare `true` rather than a lookup.
fn build_params_object(
    at: &Expr,
    spec: &ParamsSpec,
    pairs: &[(Symbol, Expr)],
    from_literal: bool,
) -> Expr {
    use crate::lower::typing::with_ty;
    let span = at.span;
    let owner_ty = Ty::Class { id: spec.class_id.clone(), args: vec![].into() };
    let local = Symbol::from("__params");
    let var = || {
        with_ty(
            Expr::new(span, ExprNode::Var { id: VarId(0), name: local.clone() }),
            owner_ty.clone(),
        )
    };
    let param_value_ty =
        Ty::Class { id: ClassId(Symbol::from("Roundhouse::ParamValue")), args: vec![].into() };
    let raw_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(param_value_ty) };
    let ivar_params = || {
        with_ty(
            Expr::new(span, ExprNode::Ivar { name: Symbol::from("params") }),
            raw_ty.clone(),
        )
    };
    let setter = |name: String, value: Expr| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(var()),
                method: Symbol::from(name),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        )
    };

    let mut stmts: Vec<Expr> = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: local.clone() },
            value: with_ty(
                Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const { path: vec![spec.class_id.0.clone()] },
                        )),
                        method: Symbol::from("new"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: true,
                    },
                ),
                owner_ty.clone(),
            ),
        },
    )];
    for (field, value) in pairs {
        let (provided, read) = if from_literal {
            (
                with_ty(
                    Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } }),
                    Ty::Bool,
                ),
                value.clone(),
            )
        } else {
            field_reads(spec, field, ivar_params(), span)
        };
        stmts.push(setter(format!("{}_provided=", field.as_str()), provided));
        stmts.push(setter(format!("{}=", field.as_str()), read));
    }
    if !from_literal {
        stmts.extend(multiparameter_stmts(spec, var, ivar_params, span));
    }
    stmts.push(var());
    with_ty(Expr::new(span, ExprNode::Seq { exprs: stmts }), owner_ty)
}

pub fn helper_spec_map<'a, 'b, I>(
    actions: I,
    specs: &'a ParamsSpecs,
) -> BTreeMap<Symbol, &'a ParamsSpec>
where
    I: IntoIterator<Item = &'b crate::dialect::Action>,
{
    let mut out = BTreeMap::new();
    for a in actions {
        if !a.name.as_str().ends_with("_params") {
            continue;
        }
        if let Some((resource, fields, nested)) = first_permit_in(&a.body) {
            if let Some(spec) = specs.find(&resource, &fields, &nested) {
                out.insert(a.name.clone(), spec);
            }
        }
    }
    out
}

pub(super) fn rewrite_nested_lookups(body: &Expr, helpers: &[crate::dialect::Action], specs: &ParamsSpecs) -> Expr {
    let helpers = helper_spec_map(helpers, specs).into_iter()
        .map(|(name, spec)| (name, spec.class_id.clone())).collect();
    let sources = super::params_sources::ParamsSources::new(body, helpers, specs);
    map_expr(body, &|expr| {
        let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
            return None;
        };
        if method.as_str() == "as_json" && args.is_empty() {
            let class = sources.class(recv, specs)?;
            if !specs.by_class(&class)?.scalar_lookup() { return None; }
            return Some(with_ty(Expr::new(expr.span, ExprNode::Send {
                recv: Some(recv.clone()), method: Symbol::from("to_h"),
                args: vec![], block: None, parenthesized: true,
            }), params_hash_ty()));
        }
        if method.as_str() != "[]" || args.len() != 1 || !sources.nested(recv, specs) {
            return None;
        }
        let string_key = matches!(&*args[0].node,
            ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })
            || args[0].ty.as_ref().is_some_and(Ty::is_stringish);
        let key = if string_key {
            with_ty(Expr::new(args[0].span, ExprNode::Send {
                recv: Some(args[0].clone()), method: Symbol::from("to_s"),
                args: vec![], block: None, parenthesized: false,
            }), Ty::Str)
        } else {
            args[0].clone()
        };
        Some(with_ty(Expr::new(expr.span, ExprNode::Send {
            recv: Some(Expr::new(expr.span, ExprNode::Const { path: vec![Symbol::from("Params")] })),
            method: Symbol::from("nested_lookup"), args: vec![recv.clone(), key],
            block: None, parenthesized: true,
        }), param_value_ty()))
    })
}

/// The params class an expression yields, in any of the spellings a
/// call site uses for one.
///
/// Three, and a rewrite that recognizes only the first sees a params
/// object where the callee wants an attribute hash:
///   - the helper: `hat_params`
///   - the permit chain written inline: `params.require(:hat_request)
///     .permit(:hat, :link, :reason)` — lobsters' `HatsController`
///     writes it straight into `update!`
///   - either of those under `.except(:reason)` or `.compact`, whose
///     result has the receiver's params class.
pub(crate) fn params_source_class(
    expr: &Expr,
    helpers: &BTreeMap<Symbol, ClassId>,
    specs: &ParamsSpecs,
) -> Option<ClassId> {
    if let ExprNode::Send { recv: Some(recv), method, block: None, .. } = &*expr.node {
        if matches!(method.as_str(), "except" | "compact") {
            return params_source_class(recv, helpers, specs);
        }
        if method.as_str() == "from_raw" {
            if let ExprNode::Const { path } = &*recv.node {
                return specs.iter().find(|s| Some(&s.class_id.0) == path.last())
                    .map(|s| s.class_id.clone());
            }
        }
    }
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
        if args.is_empty() {
            return helpers.get(method).cloned();
        }
    }
    let (resource, fields) = match_permit_call(expr)?;
    specs.find(&resource, &fields, &nested_signature(expr)).map(|s| s.class_id.clone())
}

/// Match either of the two source forms:
///   - `params.expect(article: [:title, :body])`
///   - `params.require(:article).permit(:title, :body)`
///   - `params.require(:article).permit([:title, :body])`  (already-rewritten)
///
/// Returns the (resource, fields) tuple on success.
fn match_permit_call(expr: &Expr) -> Option<(Symbol, Vec<Symbol>)> {
    match_permit_call_full(expr).map(|(resource, fields, _)| (resource, fields))
}

/// [`match_permit_call`] plus the permitted keys the synthesized record
/// cannot carry: the ARRAY-VALUED ones (`permit(:title, tags_a: [])`).
///
/// Every field of a `<Resource>Params` is `Ty::Str` — the shape a CGI
/// request actually delivers, narrowed by `Params.str` — so a key
/// declared `tags_a: []` has no slot to land in. Recognizing the form
/// anyway and dropping just that key is what keeps the rest of the list
/// lowered: before this, one array-valued key made `match_permit_call`
/// answer `None` for the WHOLE permit, so the helper kept its source
/// shape and the emitted tree called `require`/`permit` — methods no
/// target defines. lobsters' `StoriesController#story_params` is the
/// case: nine scalar keys and `tags_a: []`, and the emitted helper was
/// a `NoMethodError` waiting to be called (it also broke the Spinel
/// build outright, matz/spinel#4005).
///
/// The drop is warned about by the caller that collects specs, not
/// hidden — the array-valued key is a real modeling gap (`Ty` has no
/// `Array[String]` params field yet), and per AGENTS.md a gap gets
/// recorded rather than papered over.
fn match_permit_call_full(expr: &Expr) -> Option<(Symbol, Vec<Symbol>, Vec<Symbol>)> {
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
        let (fields, nested) = sym_array(v)?;
        return Some((resource, fields, nested));
    }

    // Form 2: `<x>.permit(...)` where `<x>` is `params.require(:resource)`.
    if method.as_str() == "permit" {
        let (resource, _) = match_require_chain(recv)?;
        let (fields, nested) = collect_permit_args(args)?;
        return Some((resource, fields, nested));
    }

    // `<permit-chain>.compact` — the same fields. Rails drops nil-valued
    // keys; the record's presence flags (`Params.provided` answers false
    // for a nil, or any non-String, value) already leave those unset, so
    // the chain under it is the whole answer. Seen through so a merge
    // written after it (campfire's `permit(…).compact.merge(password_
    // challenge: …)`) is still Form 3.
    if method.as_str() == "compact" && args.is_empty() {
        return match_permit_call_full(recv);
    }

    // Form 3: `<permit-chain>.merge(field: expr, …)` — server-side
    // fields folded into the permitted set (lobsters merges
    // `edit_user_id: @user.id` after permit). The merged keys join the
    // spec's fields, so the synthesized class carries their accessors;
    // `rewrite_to_from_raw` assigns the values after `from_raw`.
    // Pre-order collection sees this node before its inner permit, so
    // the wider spec wins the per-resource slot.
    if method.as_str() == "merge" && args.len() == 1 {
        let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
        let (resource, mut fields, nested) = match_permit_call_full(recv)?;
        for (k, _) in entries {
            fields.push(sym_of(k)?);
        }
        return Some((resource, fields, nested));
    }

    None
}

/// Form 4: `{ field: expr, … }.merge(<permit-chain>)`, ingest's spelling
/// of `<permit-chain>.reverse_merge(field: expr, …)`. The defaults' keys
/// join the fields as Form 3's do. The rewrite is a statement Seq, like
/// Form 3's, so it stands wherever Form 3's does: a params helper's tail,
/// a local's value (`attrs = <chain>.reverse_merge(...)`) or a call's
/// argument.
fn match_reverse_merge(expr: &Expr) -> Option<(Symbol, Vec<Symbol>, Vec<Symbol>)> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "merge" || args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*recv.node else { return None };
    let (resource, mut fields, nested) = match_permit_call_full(&args[0])?;
    for (k, _) in entries {
        let k = sym_of(k)?;
        if !fields.contains(&k) {
            fields.push(k);
        }
    }
    Some((resource, fields, nested))
}

/// The expression a body returns: the last statement, through nested Seqs
/// and an explicit `return`.
pub(super) fn body_tail(body: &Expr) -> &Expr {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => body_tail(&exprs[exprs.len() - 1]),
        ExprNode::Return { value } => body_tail(value),
        _ => body,
    }
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
/// a splat of Sym args (`permit(:f1, :f2)`), and either spelling may
/// carry a trailing keyword hash of ARRAY-valued keys
/// (`permit(:f1, tags_a: [])`). Returns (scalar fields, array-valued
/// keys) — see [`match_permit_call_full`] for why the second half is
/// separated rather than folded in or rejected.
fn collect_permit_args(args: &[Expr]) -> Option<(Vec<Symbol>, Vec<Symbol>)> {
    if args.len() == 1 {
        // Single Array arg form.
        if let ExprNode::Array { elements, .. } = &*args[0].node {
            return sym_list(elements);
        }
        // Single Sym arg form (1-permit case).
        if let Some(s) = sym_of(&args[0]) {
            return Some((vec![s], Vec::new()));
        }
        // Single keyword-hash form: `permit(tags_a: [])` permits
        // nothing this record can hold, but it is still a permit.
        if let Some(nested) = nested_keys(&args[0]) {
            return Some((Vec::new(), nested));
        }
        return None;
    }
    // Splat-of-Syms form.
    sym_list(args)
}

/// Split a permit list into its Sym elements and the array-valued keys
/// of a trailing keyword hash. Anything else in the list is
/// unrecognized and fails the whole match, as before.
fn sym_list(elements: &[Expr]) -> Option<(Vec<Symbol>, Vec<Symbol>)> {
    let mut fields = Vec::with_capacity(elements.len());
    for (i, el) in elements.iter().enumerate() {
        if let Some(s) = sym_of(el) {
            fields.push(s);
            continue;
        }
        // Only the LAST element may be the keyword hash — that is where
        // Ruby puts `tags_a: []`, and a hash anywhere else is a shape
        // this recognizer has never claimed.
        if i + 1 == elements.len() {
            if let Some(nested) = nested_keys(el) {
                return Some((fields, nested));
            }
        }
        return None;
    }
    Some((fields, Vec::new()))
}

/// The keys of a permit hash whose values are NON-SCALAR — Rails'
/// two spellings for "this key holds more than a String":
///
///   `tags_a: []`, `tag_ids: [:id]`   an array of scalars
///   `settings: {}`                   an arbitrary nested hash
///
/// The two mean different things to Rails and the same thing here: a
/// synthesized params record carries `Ty::Str` fields, so neither has a
/// slot, and both are dropped from the field list with the caller's
/// ledger warning. Recognizing the hash spelling matters because a
/// permit is all-or-nothing — one unrecognized entry left campfire's
/// `permit(:name, :logo, settings: {})` in its SOURCE shape, and the
/// emitted `@params.require(:account)` then reached Kernel's private
/// `require` ("private method 'require' called for an instance of
/// Hash"), which is a NoMethodError dressed as a permissions error.
///
/// A value of any other shape (an expression) is still unrecognized, so
/// the caller falls back to leaving the permit alone rather than
/// guessing at it.
fn nested_keys(e: &Expr) -> Option<Vec<Symbol>> {
    let ExprNode::Hash { entries, .. } = &*e.node else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        if !matches!(&*v.node, ExprNode::Array { .. } | ExprNode::Hash { .. }) {
            return None;
        }
        out.push(sym_of(k)?);
    }
    Some(out)
}

fn sym_of(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

/// The `expect(article: [...])` list — same split as
/// [`collect_permit_args`], since Rails 8 spells the nested keys the
/// same way inside the array (`expect(story: [:title, tags_a: []])`).
fn sym_array(e: &Expr) -> Option<(Vec<Symbol>, Vec<Symbol>)> {
    let ExprNode::Array { elements, .. } = &*e.node else {
        return None;
    };
    sym_list(elements)
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

/// Companion slot naming: `bio` → `bio_provided`.
///
/// Presence is a DIFFERENT FACT from value, so it gets its own slot
/// rather than being encoded as a nil value. Nilable slots were the
/// first attempt and they cost more than they look: `update`'s
/// `if !p.name.nil? { self.name = p.name }` needs the emitter to
/// flow-narrow an Option through the guard, which rust doesn't do
/// (`set_name(Option<String>)` — measured, 6 fresh errors) and which
/// every other strict target would need too. A `Bool` beside a `String`
/// needs nothing from any emitter.
pub fn provided_field(field: &Symbol) -> Symbol {
    Symbol::from(format!("{}_provided", field.as_str()))
}

/// `<Resource>Params` ClassId. e.g. `:article` → `ArticleParams`.
pub fn params_class_id(resource: &Symbol) -> ClassId {
    ClassId(Symbol::from(format!("{}Params", camelize(resource.as_str()))))
}

/// Synthesize one `<Resource>Params` LibraryClass per spec. Output is
/// emitted alongside the controller LCs into `app/models/` (the
/// universal-class location); routing it elsewhere is a per-target
/// emit-time choice.
pub fn synthesize_params_classes(specs: &ParamsSpecs) -> Vec<LibraryClass> {
    specs.iter().map(build_params_class).collect()
}

fn build_params_class(spec: &ParamsSpec) -> LibraryClass {
    let mut methods: Vec<MethodDef> = Vec::new();
    if spec.value_map() {
        methods.extend(super::params_values::methods(spec));
        methods.extend(synth_value_projections(spec));
        methods.push(synth_from_raw(spec));
    } else {
        methods.push(synth_params_initialize(spec));
        for field in &spec.fields {
            methods.push(synth_attr_reader(&spec.class_id, field, field_ty(spec, field)));
            methods.push(synth_attr_writer(&spec.class_id, field, field_ty(spec, field)));
            let flag = provided_field(field);
            methods.push(synth_attr_reader(&spec.class_id, &flag, Ty::Bool));
            methods.push(synth_attr_writer(&spec.class_id, &flag, Ty::Bool));
        }
        let fields_ty = Ty::Array { elem: std::sync::Arc::new(Ty::Str) };
        methods.push(synth_attr_reader(&spec.class_id, &multiparameter_fields_field(), fields_ty.clone()));
        methods.push(synth_attr_writer(&spec.class_id, &multiparameter_fields_field(), fields_ty));
        methods.push(synth_attr_reader(&spec.class_id, &multiparameter_unsupported_field(), Ty::Bool));
        methods.push(synth_attr_writer(&spec.class_id, &multiparameter_unsupported_field(), Ty::Bool));
        methods.push(synth_from_raw(spec));
        if spec.scalar_lookup() {
            methods.extend(synth_lookup_helpers(&spec.class_id));
            let values = Symbol::from("__rh_permitted_values");
            methods.push(synth_attr_reader(&spec.class_id, &values, params_hash_ty()));
            methods.push(synth_attr_writer(&spec.class_id, &values, params_hash_ty()));
            methods.push(synth_scalar_lookup(spec, "[]"));
            for method in ["key?", "include?"] {
                if spec.lookup_methods.contains(&Symbol::from(method)) {
                    methods.push(synth_scalar_lookup(spec, method));
                }
            }
        } else {
            methods.push(synth_index_read(spec));
        }
        if spec.value_demand() && !spec.scalar_lookup() {
            use crate::diagnostic::{Diagnostic, DiagnosticKind};
            let kind = DiagnosticKind::LowerResidue {
                pass: Symbol::from("params_scalar_lookup"),
                construct: Symbol::from("lookup"),
                reason: Symbol::from("mutable or file-bearing params"),
            };
            crate::emit::diagnostics::push(Diagnostic {
                span: spec.span,
                severity: Diagnostic::default_severity(&kind),
                kind,
                message: format!(
                    "value lookup on {} is unsupported with writes, uploaded-file fields, or unsupported nested filters",
                    spec.class_id.0,
                ),
            });
        }
        methods.push(synth_to_h(spec));
        if spec.wants_as_json {
            methods.push(synth_as_json(&spec.class_id));
        }
        if spec.wants_to_attrs {
            methods.push(synth_to_attrs(&spec.class_id, &spec.fields, &spec.nested_fields));
        }
        if spec.wants_except {
            methods.push(synth_except(spec));
        }
    }

    // Provenance: every synthesized body attributes to the
    // `permit(...)` / `expect(...)` call the spec was recognized from.
    for m in &mut methods {
        m.body.inherit_span(spec.span);
    }

    LibraryClass {
        name: spec.class_id.clone(),
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: Some(LibraryClassOrigin::ResourceParams {
            resource: spec.resource.clone(),
            fields: spec.fields.clone(),
        }),
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
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
fn synth_params_initialize(spec: &ParamsSpec) -> MethodDef {
    let owner = &spec.class_id;
    let fields = &spec.fields;
    let mut stmts: Vec<Expr> = Vec::new();
    if spec.scalar_lookup() {
        stmts.push(expr(ExprNode::Assign {
            target: LValue::Ivar { name: Symbol::from("__rh_permitted_values") },
            value: expr(ExprNode::Hash { entries: vec![], kwargs: false }, Some(params_hash_ty())),
        }, Some(params_hash_ty())));
    }
    {
        let unsupported = multiparameter_unsupported_field();
        for field in fields.iter().chain(std::iter::once(&unsupported)) {
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar {
                        name: if *field == unsupported { field.clone() } else { provided_field(field) },
                    },
                    value: Expr {
                        span: Span::synthetic(),
                        node: Box::new(ExprNode::Lit { value: Literal::Bool { value: false } }),
                        ty: Some(Ty::Bool),
                        effects: EffectSet::default(),
                        leading_blank_line: false,
                        cold: None,
                        decisions: 0,
                    },
                },
            ));
        }
    }
    // `Params.no_fields`, a typed empty `Array[String]`: an empty array
    // literal carries no element type for a strict target to declare.
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: multiparameter_fields_field() },
            value: expr(
                ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("Params")] })),
                    method: Symbol::from("no_fields"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: true,
                },
                Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
            ),
        },
    ));
    for field in fields {
        // A file field's "nothing" is nil, not "" — see `field_ty`.
        let rhs = if spec.nested_fields.get(field).is_some_and(|n| n.mode == "scalars") {
            expr(
                ExprNode::Array { elements: Vec::new(), style: crate::expr::ArrayStyle::default() },
                Some(Ty::Array { elem: Ty::Str.into() }),
            )
        } else if spec.file_fields.contains(field) || spec.nested_fields.contains_key(field) || merged_only_ty(spec, field).is_some() {
            expr(ExprNode::Lit { value: Literal::Nil }, Some(Ty::Nil))
        } else {
            expr(ExprNode::Lit { value: Literal::Str { value: String::new() } }, Some(Ty::Str))
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
            cold: None,
            decisions: 0,
        });
    }
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Seq { exprs: stmts }),
        ty: Some(Ty::Nil),
        effects: EffectSet::default(),
        leading_blank_line: false,
        cold: None,
        decisions: 0,
    };
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Private,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
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
        block_param: None,
    }
}

fn synth_attr_reader(owner: &ClassId, field: &Symbol, ty: Ty) -> MethodDef {
    // Permitted fields are user-supplied strings from the request (CGI
    // string-typed before any model-side coercion). Type as Str so the
    // value flows uniformly into setter assignments; a companion
    // `<field>_provided` slot is Bool.
    let field_ty = ty;
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Ivar { name: field.clone() }),
        ty: Some(field_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        cold: None,
        decisions: 0,
    };
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
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
            block_param: None,
    }
}

/// Copy the params record and clear only the copy's requested presence flags.
/// A local alias can retain the receiver, so mutating it would change
/// subsequent builds from the original permitted parameters.
fn synth_except(spec: &ParamsSpec) -> MethodDef {
    let owner = &spec.class_id;
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![].into() };
    let span = Span::synthetic();
    let result = Symbol::from("result");
    let result_read = || with_ty(Expr::new(span, ExprNode::Var {
        id: VarId(1), name: result.clone(),
    }), owner_ty.clone());
    let setter = |field: &Symbol, value: Expr| Expr::new(span, ExprNode::Send {
        recv: Some(result_read()), method: Symbol::from(format!("{field}=")),
        args: vec![value], block: None, parenthesized: false,
    });
    let keys = Symbol::from("keys");
    let keys_ty = Ty::Array { elem: Ty::Union { variants: vec![Ty::Sym, Ty::Str].into() }.into() };
    let keys_read = || with_ty(Expr::new(span,
        ExprNode::Var { id: VarId(0), name: keys.clone() },
    ), keys_ty.clone());
    let mut stmts = vec![Expr::new(span, ExprNode::Assign {
        target: LValue::Var { id: VarId(1), name: result.clone() },
        value: with_ty(Expr::new(span, ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const {
                path: owner.0.as_str().split("::").map(Symbol::from).collect(),
            })),
            method: Symbol::from("new"), args: vec![], block: None, parenthesized: false,
        }), owner_ty.clone()),
    })];
    for field in &spec.fields {
        let provided = provided_field(field);
        stmts.push(setter(field, with_ty(
            Expr::new(span, ExprNode::Ivar { name: field.clone() }), field_ty(spec, field),
        )));
        stmts.push(setter(&provided, with_ty(
            Expr::new(span, ExprNode::Ivar { name: provided.clone() }), Ty::Bool,
        )));
        let includes = |value| Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(keys_read()),
                method: Symbol::from("include?"),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value },
                )],
                block: None,
                parenthesized: false,
            },
        );
        let cond = Expr::new(span, ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: Default::default(),
            left: includes(Literal::Sym { value: field.clone() }),
            right: includes(Literal::Str { value: field.as_str().into() }),
        });
        let clear = setter(&provided,
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Bool { value: false } },
                ),
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond,
                then_branch: clear,
                else_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Nil },
                ),
            },
        ));
    }
    stmts.push(result_read());
    let body = with_ty(Expr::new(span, ExprNode::Seq { exprs: stmts }), owner_ty.clone());
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("except"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::rest(keys.clone())],
        body,
        signature: Some(Ty::Fn {
            params: vec![crate::ty::Param {
                name: keys, ty: keys_ty.into(), kind: crate::ty::ParamKind::Rest,
            }].into(),
            block: None,
            ret: owner_ty.into(),
            effects: EffectSet::pure(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn synth_attr_writer(owner: &ClassId, field: &Symbol, ty: Ty) -> MethodDef {
    let value = Symbol::from("value");
    let field_ty = ty;
    let rhs = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Var { id: VarId(0), name: value.clone() }),
        ty: Some(field_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        cold: None,
        decisions: 0,
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
        cold: None,
        decisions: 0,
    };
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
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
            block_param: None,
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
fn synth_from_raw(spec: &ParamsSpec) -> MethodDef {
    use crate::lower::typing::with_ty;
    let owner = &spec.class_id;
    let resource = &spec.resource;
    let fields = &spec.fields;
    let params = Symbol::from("params");
    let sub = Symbol::from("sub");
    let instance = Symbol::from("instance");

    let param_value_ty = Ty::Class {
        id: ClassId(Symbol::from("Roundhouse::ParamValue")),
        args: vec![].into(),
    };
    let hash_ty = Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(param_value_ty),
    };
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![].into() };

    let str_lit = |v: &str| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: v.to_string() } },
        ),
        Ty::Str,
    );
    let var = |name: &Symbol, ty: Ty| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: name.clone() },
        ),
        ty,
    );
    // `Params.<method>(...)` — the narrowing accessors in
    // runtime/ruby/params.rb. Everything this body used to open-code
    // (`fetch` with a default, `is_a?(Hash)` + `Cast`, `is_a?(String)`
    // narrowing, a `raw_<field>` temp per field) lives there now, in
    // ONE body, so no emitter has to recognize a narrowing idiom in
    // generated code to compile this.
    let call = |method: &str, args: Vec<Expr>, ret: Ty| with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Params")] },
                )),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        ),
        ret,
    );

    let mut stmts: Vec<Expr> = Vec::new();
    // sub = Params.sub(params, "<resource>")
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: sub.clone() },
            value: call(
                "sub",
                vec![var(&params, hash_ty.clone()), str_lit(resource.as_str())],
                hash_ty.clone(),
            ),
        },
    ));
    stmts.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: instance.clone() },
            value: with_ty(
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
            ),
        },
    ));

    if spec.scalar_lookup() {
        let array = |elements| with_ty(Expr::new(Span::synthetic(), ExprNode::Array {
            elements, style: Default::default(),
        }), Ty::Array { elem: Ty::Str.into() });
        let names = array(spec.key_fields.iter().map(|field| str_lit(field.as_str())).collect());
        let children = spec.nested_candidates.iter().filter_map(|candidate|
            spec.nested_fields.get(&candidate.name)).map(|nested| {
            (str_lit(nested.name.as_str()), array(nested.allowed.iter().map(|field| str_lit(field)).collect()))
        }).collect();
        let children = with_ty(Expr::new(Span::synthetic(), ExprNode::Hash {
            entries: children, kwargs: false,
        }), Ty::Hash { key: Ty::Str.into(), value: Ty::Array { elem: Ty::Str.into() }.into() });
        let modes = spec.nested_candidates.iter().filter_map(|candidate|
            spec.nested_fields.get(&candidate.name)).map(|nested|
            (str_lit(nested.name.as_str()), str_lit(nested.mode))).collect();
        let modes = with_ty(Expr::new(Span::synthetic(), ExprNode::Hash {
            entries: modes, kwargs: false,
        }), Ty::Hash { key: Ty::Str.into(), value: Ty::Str.into() });
        let values = if spec.value_map() {
            call("filtered_values", vec![var(&sub, hash_ty.clone()), names, children, modes],
                params_hash_ty())
        } else {
            with_ty(Expr::new(Span::synthetic(), ExprNode::Send {
                recv: Some(Expr::new(Span::synthetic(), ExprNode::Const { path: vec![owner.0.clone()] })),
                method: Symbol::from("__rh_scalar_values"),
                args: vec![var(&sub, hash_ty.clone()), names],
                block: None, parenthesized: true,
            }), params_hash_ty())
        };
        stmts.push(Expr::new(Span::synthetic(), ExprNode::Assign {
            target: LValue::Attr {
                recv: var(&instance, owner_ty.clone()),
                name: Symbol::from("__rh_permitted_values"),
            },
            value: values,
        }));
    }

    // Not a merged-only key: Rails' permit drops it from the request, and only the merge after `from_raw` sets it.
    for field in fields.iter().filter(|f| !spec.value_map() && merged_only_ty(spec, f).is_none()) {
        let setter = |name: Symbol, value: Expr| Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(var(&instance, owner_ty.clone())),
                method: name,
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        );
        // Presence BEFORE value, so reading the pair top-to-bottom says
        // "was it provided, and what is it".
        let (provided, read) =
            field_reads(spec, field, var(&sub, hash_ty.clone()), Span::synthetic());
        stmts.push(setter(Symbol::from(format!("{}=", provided_field(field).as_str())), provided));
        stmts.push(setter(Symbol::from(format!("{}=", field.as_str())), read));
    }
    if !spec.value_map() {
        stmts.extend(multiparameter_stmts(
            spec,
            || var(&instance, owner_ty.clone()),
            || var(&sub, hash_ty.clone()),
            Span::synthetic(),
        ));
    }

    stmts.push(var(&instance, owner_ty.clone()));

    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("from_raw"),
        receiver: MethodReceiver::Class,
        params: vec![Param::positional(params.clone())],
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts }),
        signature: Some(fn_sig(vec![(params, hash_ty)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `def to_attrs; attrs = {}; attrs[:field] = @field if @field_provided; …; attrs; end`
///
/// The ATTRIBUTE HASH this permitted list stands for — Symbol-keyed and
/// presence-guarded, which is exactly the shape `initialize(attrs)`
/// consumes, so `Model.create!(p.to_attrs)` writes the same row
/// `Model.from_params(p)` + save would.
///
/// It exists because a params object is not always what a callee wants.
/// campfire's `Message.create_with_attachment!(attributes)` is called
/// once from a controller with `message_params` and once from
/// `Webhook` with a plain `attachment:`/`creator:` hash — so the
/// parameter's real type is an attribute hash, and the params object is
/// what has to convert. Distinct from [`synth_to_h`], which mirrors
/// Rails' `Parameters#to_h`: String keys, every field, no guards.
///
/// A field the request did not send is OMITTED rather than written as
/// `""` — the same distinction `<field>_provided` was introduced for.
/// Writing it would overwrite a column default on create, which is the
/// data loss the presence slots were added to stop.
///
/// A nested or list field (`comments_attributes`, `labels: []`) is
/// written too, as the model's constructor takes it: leaving it out lost
/// the nested records of every create that went through this hash, while
/// the update path assigned them.
pub(super) fn synth_to_attrs(owner: &ClassId, fields: &[Symbol], nested: &BTreeMap<Symbol, NestedKey>) -> MethodDef {
    use crate::lower::typing::with_ty;
    let attrs = Symbol::from("attrs");
    // The declared type is `initialize`'s parameter type verbatim: this
    // hash is built to be handed straight to it, and a narrower element
    // type would need a widening conversion at every call site.
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
    let attrs_var = || {
        with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: attrs.clone() },
            ),
            hash_ty.clone(),
        )
    };

    let mut stmts: Vec<Expr> = vec![Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: attrs.clone() },
            value: with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries: Vec::new(), kwargs: false },
                ),
                hash_ty.clone(),
            ),
        },
    )];
    for field in fields {
        let key = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Sym { value: field.clone() } },
            ),
            Ty::Sym,
        );
        let value = with_ty(
            Expr::new(Span::synthetic(), ExprNode::Ivar { name: field.clone() }),
            if nested.contains_key(field) { Ty::Untyped } else { Ty::Str },
        );
        let write = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(attrs_var()),
                method: Symbol::from("[]="),
                args: vec![key, value],
                block: None,
                parenthesized: false,
            },
        );
        let guard = with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: provided_field(field) },
            ),
            Ty::Bool,
        );
        stmts.push(Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: guard,
                then_branch: write,
                else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
        ));
    }
    stmts.push(attrs_var());

    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("to_attrs"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts }),
        signature: Some(fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn param_value_ty() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("Roundhouse::ParamValue")), args: vec![].into() }
}

fn synth_lookup_helpers(owner: &ClassId) -> Vec<MethodDef> {
    let source = r#"class GeneratedParams
  def self.__rh_scalar_values(sub, fields)
    filtered = {}
    i = 0
    while i < fields.length
      field = fields[i]
      if sub.key?(field)
        value = sub.fetch(field, nil)
        unless value.is_a?(Hash) || value.is_a?(Array)
          filtered[field] = value
        end
      end
      i = i + 1
    end
    filtered
  end

  def self.__rh_param_value(values, key)
    values.fetch(key, nil)
  end
end
"#;
    let signatures = r#"class GeneratedParams
  def self.__rh_scalar_values: (Hash[String, Roundhouse::ParamValue], Array[String]) -> Hash[String, Roundhouse::ParamValue]
  def self.__rh_param_value: (Hash[String, Roundhouse::ParamValue], String) -> Roundhouse::ParamValue
end
"#;
    crate::runtime_src::parse_library_with_rbs(source.as_bytes(), signatures, "<params_lookup>")
        .expect("typed params lookup template")
        .into_iter().flat_map(|class| class.methods)
        .map(|mut method| {
            method.enclosing_class = Some(owner.0.clone());
            method
        }).collect()
}

fn synth_scalar_lookup(spec: &ParamsSpec, method: &str) -> MethodDef {
    let key = Symbol::from("key");
    let key_ty = Ty::Union { variants: vec![Ty::Str, Ty::Sym].into() };
    let key_string = expr(ExprNode::Send {
        recv: Some(expr(ExprNode::Var { id: VarId(0), name: key.clone() }, Some(key_ty.clone()))),
        method: Symbol::from("to_s"), args: vec![], block: None, parenthesized: false,
    }, Some(Ty::Str));
    let values = expr(ExprNode::Ivar { name: Symbol::from("__rh_permitted_values") }, Some(params_hash_ty()));
    let ret = if method == "[]" { param_value_ty() } else { Ty::Bool };
    let body = if method == "[]" {
        expr(ExprNode::Send {
            recv: Some(expr(ExprNode::Const { path: vec![spec.class_id.0.clone()] }, None)),
            method: Symbol::from("__rh_param_value"), args: vec![values, key_string], block: None, parenthesized: true,
        }, Some(ret.clone()))
    } else {
        expr(ExprNode::Send {
            recv: Some(values), method: Symbol::from("key?"), args: vec![key_string],
            block: None, parenthesized: true,
        }, Some(ret.clone()))
    };
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None, has_anonymous_block: false,
        name_span: Span::synthetic(), name: Symbol::from(method),
        receiver: MethodReceiver::Instance, params: vec![Param::positional(key.clone())],
        body, signature: Some(fn_sig(vec![(key, key_ty)], ret)),
        effects: EffectSet::default(), enclosing_class: Some(spec.class_id.0.clone()),
        kind: AccessorKind::Method, is_async: false, mutates_self: false, block_param: None,
    }
}

pub(crate) fn params_hash_ty() -> Ty {
    Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(param_value_ty()) }
}

/// `def to_h; { "field1" => @field1, "field2" => @field2, … }; end` —
/// returns a String-keyed Hash of the typed-struct's fields. Mirrors
/// the `Parameters#to_h` surface so `permitted.to_h` keeps working
/// after the lowerer rewrites `params.permit(...)` to typed-struct
/// construction. Value type is `Str` (matching the synthesized
/// attr_reader); strict targets see `Hash[String, String]`, no
/// `untyped` channel.
/// `params[:name]` — the Hash face of a params object.
///
/// The monomorphized paths never need it: `@room.update! room_params`
/// lowers to `update_from_room_params!`, which reads the typed fields
/// directly. This is for the params object that reaches a GENERIC
/// Hash-consuming API instead — campfire's
/// `Rooms::Open.create_for(room_params, users:)` hands it to a
/// user-written method whose parameter is untyped (the same method is
/// called with a `{}` literal elsewhere, so it cannot be specialized),
/// and that method calls `create!(attributes)`, whose synthesized
/// `initialize` reads `attrs[:name]` per column. Without `[]` the
/// object arrives and the create dies with `undefined method '[]' for
/// an instance of RoomParams`.
///
/// PRESENCE IS NOT HONORED HERE, deliberately, and it is a known gap:
/// an absent key reads as the `""` the slot was initialized to rather
/// than as nil, so `create!` on a params object missing a key writes the
/// empty string where Rails would leave null. `to_h` — the other Hash
/// face of this object, synthesized right below — has always had exactly
/// that behavior, so the two agree, and the divergence is one line in
/// two places rather than a new one here.
///
/// Gating on the `_provided` companion slot is what it SHOULD do, and
/// the flags are already there for it (`update_from_<resource>_params!`
/// reads them). The blocker is downstream: an
/// `if @x_provided then @x else nil end` in an arm body emits as
/// `serde_json::Value::from(if self.x_provided { self.x.clone() })` —
/// the Rust emitter drops a `Nil` else branch, which is fine in
/// statement position and E0317 in expression position, and the
/// Untyped coercion needs to move INSIDE the branches for the typed
/// form to work at all. Measured on the real-blog rust toolchain gate.
/// That emitter fix is worth doing on its own; it should not ride in
/// on a params change.
///
/// Modeled on `model_to_library::schema::synth_index_read` — same Case
/// over Symbol-literal patterns, same `Ty::Sym -> Untyped` signature.
fn synth_index_read(spec: &ParamsSpec) -> MethodDef {
    let owner = &spec.class_id;
    let key = Symbol::from("key");
    let arms: Vec<Arm> = spec
        .fields
        .iter()
        .map(|field| Arm {
            pattern: Pattern::Lit { value: Literal::Sym { value: field.clone() } },
            guard: None,
            body: expr(ExprNode::Ivar { name: field.clone() }, Some(field_ty(spec, field))),
        })
        .collect();

    // A `case` with zero arms has no `when` clause at all, which is a
    // Ruby syntax error — reachable here whenever every permitted key is
    // non-scalar (`permit(widget_ids: [])`, nothing else) and so `fields`
    // drops to empty (see `tests/params_all_non_scalar_keys.rs`). `key`
    // is then unused; the body is just `nil`, same signature, same
    // `Ty::Untyped` result.
    let body = if arms.is_empty() {
        expr(ExprNode::Lit { value: Literal::Nil }, Some(Ty::Untyped))
    } else {
        expr(
            ExprNode::Case {
                scrutinee: expr(
                    ExprNode::Var { id: VarId(0), name: key.clone() },
                    Some(Ty::Sym),
                ),
                arms,
            },
            Some(Ty::Untyped),
        )
    };

    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("[]"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(key.clone())],
        body,
        // Heterogeneous: the value when provided, nil when not.
        signature: Some(crate::lower::typing::fn_sig(
            vec![(key, Ty::Sym)],
            Ty::Untyped,
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// Node + type, with the boilerplate every synthesized `Expr` here
/// repeats.
pub(super) fn expr(node: ExprNode, ty: Option<Ty>) -> Expr {
    Expr {
        span: Span::synthetic(),
        node: Box::new(node),
        ty,
        effects: EffectSet::default(),
        leading_blank_line: false,
        cold: None,
        decisions: 0,
    }
}

fn synth_to_h(spec: &ParamsSpec) -> MethodDef {
    let owner = &spec.class_id;
    // A carried nested key, or a merged-only one typed by its merge, is no
    // String: the String-valued hash leaves it out, and the model reads it
    // through its own member.
    let entries: Vec<(Expr, Expr)> = spec
        .fields
        .iter()
        .filter(|field| !spec.nested_fields.contains_key(*field) && merged_only_ty(spec, field).is_none())
        .map(|field| {
            let key = expr(
                ExprNode::Lit { value: Literal::Str { value: field.as_str().to_string() } },
                Some(Ty::Str),
            );
            let ivar = expr(ExprNode::Ivar { name: field.clone() }, Some(field_ty(spec, field)));
            // A String hash cannot hold a file: the uploaded NAME stands
            // in, which is what Rails' own `to_s` on one gives.
            let value = if spec.file_fields.contains(field) {
                expr(
                    ExprNode::Send {
                        recv: Some(expr(
                            ExprNode::Const {
                                path: uploaded_file_class()
                                    .0
                                    .as_str()
                                    .split("::")
                                    .map(Symbol::from)
                                    .collect(),
                            },
                            None,
                        )),
                        method: Symbol::from("name_of"),
                        args: vec![ivar],
                        block: None,
                        parenthesized: true,
                    },
                    Some(Ty::Str),
                )
            } else {
                ivar
            };
            (key, value)
        })
        .collect();
    let hash_ty = Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(Ty::Str),
    };
    let hash = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Hash { entries, kwargs: false }),
        ty: Some(hash_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        cold: None,
        decisions: 0,
    };
    let ret_ty = hash_ty;
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
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
            block_param: None,
    }
}

/// `def as_json; to_h; end` — Rails' `Parameters#as_json` is the
/// permitted values as a String-keyed Hash, which `to_h` already is.
fn synth_as_json(owner: &ClassId) -> MethodDef {
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Str) };
    let body = expr(
        ExprNode::Send {
            recv: Some(expr(ExprNode::SelfRef, Some(Ty::Class { id: owner.clone(), args: vec![].into() }))),
            method: Symbol::from("to_h"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
        Some(hash_ty.clone()),
    );
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("as_json"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn fn_sig(params: Vec<(Symbol, Ty)>, ret: Ty) -> Ty {
    Ty::Fn {
        params: params
            .into_iter()
            .map(|(name, ty)| crate::ty::Param {
                name,
                ty: ty.into(),
                kind: crate::ty::ParamKind::Required,
            })
            .collect(),
        block: None,
        ret: std::sync::Arc::new(ret),
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

pub(super) fn synth_to_attrs_for_spec(spec: &ParamsSpec) -> MethodDef {
    if !spec.value_map() {
        return synth_to_attrs(&spec.class_id, &spec.fields, &spec.nested_fields);
    }
    let mut spec = spec.clone();
    spec.wants_to_attrs = true;
    super::params_values::methods(&spec).into_iter()
        .find(|method| method.name.as_str() == "to_attrs").expect("value map to_attrs")
}

pub(crate) fn spec_class_info(spec: &ParamsSpec) -> crate::analyze::ClassInfo {
    params_class_info(&build_params_class(spec))
}

fn synth_value_projections(spec: &ParamsSpec) -> Vec<MethodDef> {
    let owner_ty = Ty::Class { id: spec.class_id.clone(), args: vec![].into() };
    let values = || with_ty(Expr::new(spec.span, ExprNode::Send {
        recv: Some(with_ty(Expr::new(spec.span, ExprNode::SelfRef), owner_ty.clone())),
        method: Symbol::from("__rh_values"), args: vec![], block: None, parenthesized: false,
    }), params_hash_ty());
    let mut reads = Vec::new();
    for field in &spec.fields {
        let (provided, value) = field_reads(spec, field, values(), spec.span);
        reads.push((provided_field(field), provided, Ty::Bool));
        reads.push((field.clone(), value, field_ty(spec, field)));
    }
    let names = with_ty(Expr::new(spec.span, ExprNode::Array {
        elements: spec.scalar_fields().iter().map(|field| with_ty(
            Expr::new(spec.span, ExprNode::Lit { value: Literal::Str { value: field.as_str().to_string() } }),
            Ty::Str,
        )).collect(),
        style: Default::default(),
    }), Ty::Array { elem: Ty::Str.into() });
    for (name, method, ty) in [
        (multiparameter_fields_field(), "multiparameter_fields", Ty::Array { elem: Ty::Str.into() }),
        (multiparameter_unsupported_field(), "multiparameter_unsupported", Ty::Bool),
    ] {
        let body = with_ty(Expr::new(spec.span, ExprNode::Send {
            recv: Some(Expr::new(spec.span, ExprNode::Const { path: vec![Symbol::from("Params")] })),
            method: Symbol::from(method), args: vec![values(), names.clone()], block: None, parenthesized: true,
        }), ty.clone());
        reads.push((name, body, ty));
    }
    let mut methods = Vec::new();
    for (name, mut body, ty) in reads {
        super::util::map_expr_mut(&mut body, &|expr| {
            if let ExprNode::Const { path } = &*expr.node {
                Some(with_ty(expr.clone(), Ty::Class {
                    id: ClassId(Symbol::from(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"))),
                    args: vec![].into(),
                }))
            } else { None }
        });
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None, has_anonymous_block: false,
            name_span: spec.span, name, receiver: MethodReceiver::Instance, params: vec![],
            body, signature: Some(fn_sig(vec![], ty)), effects: EffectSet::default(),
            enclosing_class: Some(spec.class_id.0.clone()), kind: AccessorKind::Method,
            is_async: false, mutates_self: false, block_param: None,
        });
    }
    methods
}

/// Rewrite controller-action expressions: replace each `params.expect(...)` /
/// `params.require(:r).permit(...)` with `<Resource>Params.from_raw(@params)`.
/// `specs` carries the (resource, class_id) mapping; expressions whose
/// resource isn't in `specs` (shouldn't happen — we collected from
/// these same bodies) fall through unchanged.
///
/// `guard` (the tree raises `ParameterMissing`, see
/// `FormatBreadth::raises_param_missing`) hands the factory
/// `Params.expect_present(@params, …)` / `Params.require_present(@params,
/// …)` instead of `@params`: Rails refuses a missing or malformed
/// resource there, and the two forms refuse differently, which the shared
/// `<Resource>Params` class cannot know - so the check sits at the call.
pub fn rewrite_to_from_raw(expr: &Expr, specs: &ParamsSpecs, guard: bool) -> Expr {
    let tail = body_tail(expr);
    map_expr(expr, &|e| {
        // `<permit-chain>.compact` — DROP the `.compact`. Rails' version
        // removes nil-valued keys, and a presence-aware `from_raw`
        // (which the same `.compact` demanded, see
        // `ParamsSpec::wants_compact`) already reads "not provided" as
        // nil, so there is nothing left for it to remove. Emitting an
        // identity method instead would be the same no-op with a call.
        if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node {
            if method.as_str() == "compact" && args.is_empty() {
                if let Some((resource, fields)) = match_permit_call(recv) {
                    if let Some(spec) = specs.find(&resource, &fields, &nested_signature(recv)) {
                        let arg = factory_arg(recv, guard, e.span);
                        if spec.value_map() {
                            return Some(Expr::new(e.span, ExprNode::Send {
                                recv: Some(build_from_raw_call(&spec.class_id, e.span, arg)),
                                method: method.clone(), args: vec![], block: None, parenthesized: true,
                            }));
                        }
                        return Some(build_from_raw_call(&spec.class_id, e.span, arg));
                    }
                }
            }
        }
        // Merge form first — the bare-permit arm below would match the
        // same node (Form 3 delegates) and drop the merged values.
        if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node {
            if method.as_str() == "merge"
                && args.len() == 1
                && match_permit_call(recv).is_some()
            {
                let (resource, fields) = match_permit_call(e)?;
                let spec = specs.find(&resource, &fields, &nested_signature(e))?;
                let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
                // The factory reads the permit chain's own shape, under a
                // `.compact` (see `match_permit_call_full`).
                let chain = match &*recv.node {
                    ExprNode::Send { recv: Some(inner), method, args, block: None, .. }
                        if method.as_str() == "compact" && args.is_empty() => inner,
                    _ => recv,
                };
                let arg = factory_arg(chain, guard, e.span);
                return Some(build_from_raw_merge(&spec.class_id, entries, e.span, arg, &|k| merged_only_ty(spec, k).is_some()));
            }
        }
        // Reverse-merge form (Form 4). Under an explicit `return` at the
        // body's tail the statements replace the `return`, which moves
        // onto the final `_p`.
        let defaults = |e: &Expr| -> Option<Expr> {
            let (resource, fields, _) = match_reverse_merge(e)?;
            let ExprNode::Send { recv: Some(recv), args, .. } = &*e.node else { unreachable!() };
            let ExprNode::Hash { entries, .. } = &*recv.node else { unreachable!() };
            let (_, permitted) = match_permit_call(&args[0])?;
            let spec = specs.find(&resource, &fields, &nested_signature(e))?;
            let arg = factory_arg(&args[0], guard, e.span);
            Some(build_from_raw_defaults(&spec.class_id, entries, &permitted, e.span, arg))
        };
        if let ExprNode::Return { value } = &*e.node {
            if std::ptr::eq(value, tail) {
                if let Some(seq) = defaults(value) {
                    let ExprNode::Seq { mut exprs } = *seq.node else { unreachable!() };
                    let last = exprs.pop().expect("from_raw Seq ends in `_p`");
                    exprs.push(Expr::new(e.span, ExprNode::Return { value: last }));
                    return Some(Expr::new(e.span, ExprNode::Seq { exprs }));
                }
            }
        }
        if let Some(seq) = defaults(e) {
            return Some(seq);
        }
        let (resource, fields) = match_permit_call(e)?;
        let spec = specs.find(&resource, &fields, &nested_signature(e))?;
        Some(build_from_raw_call(&spec.class_id, e.span, factory_arg(e, guard, e.span)))
    })
}

/// `<chain>.reverse_merge(k: v)`: [`build_from_raw_merge`], except that a
/// key the chain permits keeps the request's value when one was provided
/// (`_p.k = v unless _p.k_provided`). A key it does not permit never
/// comes from the request in Rails, so it is always assigned.
fn build_from_raw_defaults(
    class_id: &ClassId,
    entries: &[(Expr, Expr)],
    permitted: &[Symbol],
    span: Span,
    arg: Expr,
) -> Expr {
    let ExprNode::Seq { exprs } = *build_from_raw_merge(class_id, entries, span, arg, &|_| false).node else {
        unreachable!("build_from_raw_merge returns a Seq")
    };
    // `_p = from_raw(...)`, then a (value, flag) setter pair per Symbol
    // key in entry order, then `_p`.
    let mut setters = exprs[1..exprs.len() - 1].chunks(2);
    let mut out = vec![exprs[0].clone()];
    for (k, _) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else { continue };
        let pair = Expr::new(span, ExprNode::Seq { exprs: setters.next().unwrap().to_vec() });
        if !permitted.contains(name) {
            out.push(pair);
            continue;
        }
        let send = |recv: Expr, method: Symbol| {
            Expr::new(span, ExprNode::Send { recv: Some(recv), method, args: vec![], block: None, parenthesized: false })
        };
        let p = Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_p") });
        out.push(Expr::new(
            span,
            ExprNode::If {
                cond: send(send(p, provided_field(name)), Symbol::from("!")),
                then_branch: pair,
                else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
            },
        ));
    }
    out.push(exprs[exprs.len() - 1].clone());
    Expr::new(span, ExprNode::Seq { exprs: out })
}

/// `<chain>.merge(k: v)` → `_p = <Class>.from_raw(@params); _p.k = v;
/// _p.k_provided = true; _p` — a statement-shaped Seq; the corpus site
/// is a params-helper tail, where the Seq renders as plain statements.
/// The setters run after `from_raw`, so a client-supplied value under
/// the same key is overwritten (Rails' merge contract).
///
/// The presence flag has to be set alongside: a merged key is a
/// SERVER-side value, so `update` must assign it even when the request
/// never mentioned it. (The `<field>=` writer can't do this itself —
/// emitters collapse an `AttributeWriter` into a plain field and drop
/// its body, so every producer sets the flag explicitly.)
fn build_from_raw_merge(class_id: &ClassId, entries: &[(Expr, Expr)], span: Span, arg: Expr, typed: &dyn Fn(&Symbol) -> bool) -> Expr {
    let p = |()| Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_p") });
    let mut stmts = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_p") },
            value: build_from_raw_call(class_id, span, arg),
        },
    )];
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            continue;
        };
        stmts.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: p(()), name: name.clone() },
                value: if typed(name) { v.clone() } else { param_string_literal(v).unwrap_or_else(|| v.clone()) },
            },
        ));
        stmts.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: p(()), name: provided_field(name) },
                value: Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } }),
            },
        ));
    }
    stmts.push(p(()));
    Expr::new(span, ExprNode::Seq { exprs: stmts })
}

/// A literal merged value as the String a request would carry for it:
/// every field of a `<Resource>Params` is a String (the model's typed
/// factory casts it as it casts the request's), so `status: 0` is
/// `"0"`, `role: :owner` is `"owner"`. A literal of another type in a
/// String field is a type error on a strict target, and Spinel
/// miscompiles it. Any other value is left as written.
fn param_string_literal(v: &Expr) -> Option<Expr> {
    let text = match &*v.node {
        ExprNode::Lit { value: Literal::Int { value } } => value.to_string(),
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        ExprNode::Lit { value: Literal::Bool { value } } => value.to_string(),
        _ => return None,
    };
    Some(crate::lower::typing::with_ty(
        Expr::new(v.span, ExprNode::Lit { value: Literal::Str { value: text } }),
        Ty::Str,
    ))
}

/// The factory's argument for the permit chain `chain`: `@params`, or,
/// under `guard`, `@params` behind the refusal its source form makes in
/// Rails - `Params.expect_present(@params, "<r>", ...)` with the filter's
/// keys by kind ([`ExpectKeys`]) for `params.expect(r: [...])`,
/// `Params.require_present(@params, "<r>")` for
/// `params.require(:r).permit(...)`. A `.merge(...)` on top refuses as
/// the chain it extends.
fn factory_arg(chain: &Expr, guard: bool, span: Span) -> Expr {
    let params_ivar = Expr::new(span, ExprNode::Ivar { name: Symbol::from("params") });
    if !guard {
        return params_ivar;
    }
    let str_lit = |v: &str| Expr::new(span, ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let params_call = |method: &str, args: Vec<Expr>| Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] })),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    );
    let mut node = chain;
    loop {
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*node.node else {
            return params_ivar;
        };
        match method.as_str() {
            "merge" => node = recv,
            "expect" if is_bare_params(recv) && args.len() == 1 => {
                let ExprNode::Hash { entries, .. } = &*args[0].node else { return params_ivar };
                let [(k, v)] = entries.as_slice() else { return params_ivar };
                let ExprNode::Array { elements, .. } = &*v.node else { return params_ivar };
                let (Some(resource), Some(keys)) = (sym_of(k), expect_keys(elements)) else {
                    return params_ivar;
                };
                return params_call("expect_present", expect_present_args(params_ivar, str_lit(resource.as_str()), &keys));
            }
            "permit" => {
                let Some((resource, ())) = match_require_chain(recv) else { return params_ivar };
                // `params.expect(r: [...])`, respelled by `rewrite_params`.
                if node.decisions & crate::expr::FROM_PARAMS_EXPECT != 0 {
                    let elements = match args.as_slice() {
                        [single] => match &*single.node {
                            ExprNode::Array { elements, .. } => elements.as_slice(),
                            _ => args.as_slice(),
                        },
                        _ => args.as_slice(),
                    };
                    let Some(keys) = expect_keys(elements) else { return params_ivar };
                    return params_call("expect_present", expect_present_args(params_ivar, str_lit(resource.as_str()), &keys));
                }
                return params_call("require_present", vec![params_ivar, str_lit(resource.as_str())]);
            }
            _ => return params_ivar,
        }
    }
}

/// An `expect` filter's keys by the value each accepts, as Rails'
/// `expect` decides whether the resource holds anything permitted:
/// `title` a scalar, `tags: []` an array of scalars, `settings: [:theme]`
/// or `settings: {}` a hash, `items: [[:name]]` an array (of hashes).
/// Separate from the record-field split ([`collect_permit_args`]), which
/// only needs to know which keys are not scalars.
#[derive(Default)]
struct ExpectKeys {
    scalars: Vec<Symbol>,
    scalar_arrays: Vec<Symbol>,
    hashes: Vec<Symbol>,
    arrays: Vec<Symbol>,
}

fn expect_keys(elements: &[Expr]) -> Option<ExpectKeys> {
    let mut keys = ExpectKeys::default();
    for (i, el) in elements.iter().enumerate() {
        if let Some(s) = sym_of(el) {
            keys.scalars.push(s);
            continue;
        }
        // Only the last element may be the keyword hash, as in
        // [`sym_list`].
        let ExprNode::Hash { entries, .. } = &*el.node else { return None };
        if i + 1 != elements.len() || entries.is_empty() {
            return None;
        }
        for (k, v) in entries {
            let key = sym_of(k)?;
            match &*v.node {
                ExprNode::Hash { .. } => keys.hashes.push(key),
                ExprNode::Array { elements, .. } => match elements.first().map(|e| &*e.node) {
                    None => keys.scalar_arrays.push(key),
                    Some(ExprNode::Array { .. }) => keys.arrays.push(key),
                    Some(_) => keys.hashes.push(key),
                },
                _ => return None,
            }
        }
    }
    Some(keys)
}

/// `Params.expect_present(@params, "<r>", scalars, scalar_arrays, hashes,
/// arrays)`'s arguments.
fn expect_present_args(params_ivar: Expr, resource: Expr, keys: &ExpectKeys) -> Vec<Expr> {
    let span = params_ivar.span;
    let str_array = |items: &[Symbol]| Expr::new(
        span,
        ExprNode::Array {
            elements: items
                .iter()
                .map(|s| Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.as_str().to_string() } }))
                .collect(),
            style: crate::expr::ArrayStyle::default(),
        },
    );
    vec![
        params_ivar,
        resource,
        str_array(&keys.scalars),
        str_array(&keys.scalar_arrays),
        str_array(&keys.hashes),
        str_array(&keys.arrays),
    ]
}

fn build_from_raw_call(class_id: &ClassId, span: Span, arg: Expr) -> Expr {
    let class_const = Expr::new(
        span,
        ExprNode::Const { path: vec![class_id.0.clone()] },
    );
    // `@params` directly — the synthesized `from_raw` dives into the
    // nested resource key itself (`sub = params.fetch("<resource>", {})`),
    // so the call site doesn't need a `.require(:r).to_h` chain.
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(class_const),
            method: Symbol::from("from_raw"),
            args: vec![arg],
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
pub fn rewrite_typed_bracket_to_field(expr: &Expr, specs: &ParamsSpecs) -> Expr {
    let permitted_fields = permitted_field_tys(specs);
    crate::lower::controller_to_library::util::map_expr(expr, &|e| {
        try_rewrite_typed_bracket(e, &permitted_fields)
    })
}

pub(crate) fn permitted_field_tys(
    specs: &ParamsSpecs,
) -> crate::det::HashMap<ClassId, crate::det::HashMap<String, Ty>> {
    use crate::ty::Ty;
    let mut permitted_fields: crate::det::HashMap<
        ClassId,
        crate::det::HashMap<String, Ty>,
    > = crate::det::HashMap::default();
    for spec in specs.iter() {
        if spec.scalar_lookup() {
            continue;
        }
        let mut set = crate::det::HashMap::default();
        for f in &spec.fields {
            set.insert(f.as_str().to_string(), field_ty(spec, f));
        }
        permitted_fields.insert(spec.class_id.clone(), set);
    }
    permitted_fields
}

pub(crate) fn rewrite_typed_bracket_to_field_in_place(
    expr: &mut Expr,
    permitted_fields: &crate::det::HashMap<ClassId, crate::det::HashMap<String, crate::ty::Ty>>,
) -> bool {
    crate::lower::controller_to_library::util::map_expr_mut(expr, &|e| {
        try_rewrite_typed_bracket(e, permitted_fields)
    })
}

fn try_rewrite_typed_bracket(
    e: &Expr,
    permitted_fields: &crate::det::HashMap<ClassId, crate::det::HashMap<String, crate::ty::Ty>>,
) -> Option<Expr> {
    use crate::ty::Ty;
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
    let slot_ty = fields.get(&key)?;
    Some(Expr {
        span: e.span,
        node: Box::new(ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from(key),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        }),
        ty: Some(slot_ty.clone()),
        effects: e.effects.clone(),
        leading_blank_line: e.leading_blank_line,
        cold: None,
        decisions: 0,
    })
}
