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
    /// Some call site chains `.compact` off this permit. MEASURED
    /// against Rails 8.1: `Parameters#compact` drops keys whose value is
    /// explicitly nil and KEEPS `""` — so for a form POST it changes
    /// nothing, and what it really expresses is "assign only what the
    /// request actually provided."
    ///
    /// Our `from_raw` couldn't express that: it defaults an ABSENT key
    /// to `""`, which `update` then assigns, clobbering the column.
    /// campfire's profile page has an avatar-only form, so submitting it
    /// blanked the user's name, email and bio. Setting this flag makes
    /// the class presence-aware — absent (and JSON-null) keys read nil,
    /// which `update`'s existing skip-nil turns into Rails' semantics —
    /// at the cost of nilable field types, so it is demand-gated the way
    /// `wants_except` is.
    pub wants_compact: bool,
    /// Some call site writes `<Model>.create(<helper>)` / `.create!` on
    /// the model this list's resource names — synthesize the matching
    /// typed factory. Demand-gated like `wants_except`: the runtime's
    /// `create` takes an attribute Hash, so every params-permitted model
    /// in every app would otherwise carry two methods nobody calls.
    pub wants_create: bool,
    pub wants_create_bang: bool,
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
    pub fn find(&self, resource: &Symbol, fields: &[Symbol]) -> Option<&ParamsSpec> {
        self.specs
            .iter()
            .find(|s| &s.resource == resource && s.fields == fields)
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
}

/// Walk every controller's action bodies and collect one ParamsSpec per
/// distinct `(resource, fields)` pair.
pub fn collect_specs(controllers: &[Controller]) -> ParamsSpecs {
    let mut index: BTreeMap<(Symbol, Vec<Symbol>), usize> = BTreeMap::new();
    let mut specs: Vec<ParamsSpec> = Vec::new();
    for c in controllers {
        for action in c.actions() {
            collect_from_expr(&action.body, &c.name, &mut index, &mut specs);
        }
    }
    assign_class_ids(&mut specs);
    let mut specs = ParamsSpecs { specs };
    scan_create_demand(controllers, &mut specs);
    specs
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
    for c in controllers {
        let actions: Vec<crate::dialect::Action> = c.actions().cloned().collect();
        let helpers = helper_spec_map(&actions, specs);
        if helpers.is_empty() {
            continue;
        }
        for action in &actions {
            walk_all(&action.body, &mut |e| {
                let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else {
                    return;
                };
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
                wants_compact: false,
                wants_create: false,
                wants_create_bang: false,
                declaring: Vec::new(),
                is_canonical: true,
            })
            .collect(),
    }
}

/// Record one recognized permit list, folding it into an existing spec
/// when an identical `(resource, fields)` pair was already seen.
/// What a call site chained off the permit, and therefore what the
/// synthesized class has to grow.
#[derive(Clone, Copy, Default)]
struct Wants {
    except: bool,
    compact: bool,
}

fn record(
    resource: Symbol,
    fields: Vec<Symbol>,
    controller: &ClassId,
    span: Span,
    wants: Wants,
    index: &mut BTreeMap<(Symbol, Vec<Symbol>), usize>,
    specs: &mut Vec<ParamsSpec>,
) {
    let key = (resource.clone(), fields.clone());
    match index.get(&key) {
        Some(&i) => {
            specs[i].wants_except |= wants.except;
            specs[i].wants_compact |= wants.compact;
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
                fields,
                span,
                wants_except: wants.except,
                wants_compact: wants.compact,
                wants_create: false,
                wants_create_bang: false,
                declaring: vec![controller.clone()],
                is_canonical: false,
            });
        }
    }
}

fn collect_from_expr(
    expr: &Expr,
    controller: &ClassId,
    index: &mut BTreeMap<(Symbol, Vec<Symbol>), usize>,
    specs: &mut Vec<ParamsSpec>,
) {
    // `<permit-chain>.except(:key)` / `.compact` — mark the spec before
    // the walk reaches the inner chain, so the flag survives the fold in
    // `record`.
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node {
        let wants = match method.as_str() {
            "except" => Wants { except: true, ..Default::default() },
            "compact" => Wants { compact: true, ..Default::default() },
            _ => Wants::default(),
        };
        if (wants.except || wants.compact) && !matches!(&*recv.node, ExprNode::Lit { .. }) {
            if let Some((resource, fields)) = match_permit_call(recv) {
                record(resource, fields, controller, expr.span, wants, index, specs);
            }
        }
    }
    if let Some((resource, fields)) = match_permit_call(expr) {
        record(resource, fields, controller, expr.span, Wants::default(), index, specs);
        // Stop here. The merge form (`permit(...).merge(k: v)`) matched
        // this node with the WIDER field set; recursing would reach the
        // inner bare permit and — now that specs key on the field list —
        // register a second, narrower class for the same call site.
        return;
    }
    walk_children(expr, &mut |c| collect_from_expr(c, controller, index, specs));
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
    let mut taken: std::collections::HashSet<ClassId> = std::collections::HashSet::new();
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
    if spec.is_canonical {
        Symbol::from(format!("update{bang}"))
    } else {
        Symbol::from(format!(
            "update_from_{}{bang}",
            crate::naming::snake_case(spec.class_id.0.as_str())
        ))
    }
}

/// First permit list in `expr`, in the same pre-order the collector
/// uses — the way a `<resource>_params` helper body names its spec.
pub fn first_permit_in(expr: &Expr) -> Option<(Symbol, Vec<Symbol>)> {
    if let Some(found) = match_permit_call(expr) {
        return Some(found);
    }
    let mut found = None;
    walk_children(expr, &mut |c| {
        if found.is_none() {
            found = first_permit_in(c);
        }
    });
    found
}

/// Map each `<x>_params`-shaped helper to the spec its body declares.
/// Call-site rewrites (`Model.new(user_params)`, `@user.update
/// user_params`) see only the helper name, so this is how they reach
/// the right class when a resource carries several.
pub fn helper_spec_map<'a>(
    actions: &[crate::dialect::Action],
    specs: &'a ParamsSpecs,
) -> BTreeMap<Symbol, &'a ParamsSpec> {
    let mut out = BTreeMap::new();
    for a in actions {
        if !a.name.as_str().ends_with("_params") {
            continue;
        }
        if let Some((resource, fields)) = first_permit_in(&a.body) {
            if let Some(spec) = specs.find(&resource, &fields) {
                out.insert(a.name.clone(), spec);
            }
        }
    }
    out
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

    // Form 3: `<permit-chain>.merge(field: expr, …)` — server-side
    // fields folded into the permitted set (lobsters merges
    // `edit_user_id: @user.id` after permit). The merged keys join the
    // spec's fields, so the synthesized class carries their accessors;
    // `rewrite_to_from_raw` assigns the values after `from_raw`.
    // Pre-order collection sees this node before its inner permit, so
    // the wider spec wins the per-resource slot.
    if method.as_str() == "merge" && args.len() == 1 {
        let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
        let (resource, mut fields) = match_permit_call(recv)?;
        for (k, _) in entries {
            fields.push(sym_of(k)?);
        }
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

/// Companion slot naming: `bio` → `bio_provided`.
///
/// Presence is a DIFFERENT FACT from value, so it gets its own slot
/// rather than being encoded as a nil value. Nilable slots were the
/// first attempt and they cost more than they look: `update`'s
/// `if !p.name.nil? { self.name = p.name }` needs the emitter to
/// flow-narrow an Option through the guard, which rust2 doesn't do
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
    let presence = spec.wants_compact;
    let mut methods: Vec<MethodDef> = Vec::new();
    methods.push(synth_params_initialize(&spec.class_id, &spec.fields, presence));
    for field in &spec.fields {
        methods.push(synth_attr_reader(&spec.class_id, field, Ty::Str));
        methods.push(synth_attr_writer(&spec.class_id, field, Ty::Str));
        if presence {
            let flag = provided_field(field);
            methods.push(synth_attr_reader(&spec.class_id, &flag, Ty::Bool));
            methods.push(synth_attr_writer(&spec.class_id, &flag, Ty::Bool));
        }
    }
    methods.push(synth_from_raw(&spec.class_id, &spec.resource, &spec.fields, presence));
    methods.push(synth_to_h(&spec.class_id, &spec.fields));
    if spec.wants_except {
        methods.push(synth_except(&spec.class_id, &spec.fields));
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
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: Some(LibraryClassOrigin::ResourceParams {
            resource: spec.resource.clone(),
            fields: spec.fields.clone(),
        }),
        constants: Vec::new(),
        unknown_calls: Vec::new(),
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
fn synth_params_initialize(owner: &ClassId, fields: &[Symbol], presence: bool) -> MethodDef {
    let mut stmts: Vec<Expr> = Vec::new();
    if presence {
        for field in fields {
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: provided_field(field) },
                    value: Expr {
                        span: Span::synthetic(),
                        node: Box::new(ExprNode::Lit { value: Literal::Bool { value: false } }),
                        ty: Some(Ty::Bool),
                        effects: EffectSet::default(),
                        leading_blank_line: false,
                        diagnostic: None,
                        hint: None,
                        decisions: 0,
                    },
                },
            ));
        }
    }
    for field in fields {
        let rhs = Expr {
            span: Span::synthetic(),
            node: Box::new(ExprNode::Lit { value: Literal::Str { value: String::new() } }),
            ty: Some(Ty::Str),
            effects: EffectSet::default(),
            leading_blank_line: false,
            diagnostic: None,
            hint: None,
            decisions: 0,
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
            hint: None,
            decisions: 0,
        });
    }
    let body = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Seq { exprs: stmts }),
        ty: Some(Ty::Nil),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        hint: None,
        decisions: 0,
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
        diagnostic: None,
        hint: None,
        decisions: 0,
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
            block_param: None,
    }
}

/// `def except(key)` — nil the named field's slot and return self.
/// Rails' `permitted.except(:reason)` drops a key before `update`
/// consumes the params; the typed `update` skips nil fields, so a
/// nil'd slot is exactly "not provided". The receiver is always a
/// fresh `from_raw` product at the corpus sites, so mutate-and-return
/// stands in for Rails' copy semantics.
fn synth_except(owner: &ClassId, fields: &[Symbol]) -> MethodDef {
    let key = Symbol::from("key");
    let key_read = |()| Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: key.clone() },
    );
    let mut stmts: Vec<Expr> = Vec::new();
    for field in fields {
        let cond = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(key_read(())),
                method: Symbol::from("=="),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Sym { value: field.clone() } },
                )],
                block: None,
                parenthesized: false,
            },
        );
        let clear = Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: field.clone() },
                value: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
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
    stmts.push(Expr::new(Span::synthetic(), ExprNode::SelfRef));
    let body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts });
    let owner_ty = Ty::Class { id: owner.clone(), args: vec![] };
    MethodDef {
        name: Symbol::from("except"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(key.clone())],
        body,
        signature: Some(fn_sig(vec![(key, Ty::Sym)], owner_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
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
        diagnostic: None,
        hint: None,
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
        diagnostic: None,
        hint: None,
        decisions: 0,
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
fn synth_from_raw(
    owner: &ClassId,
    resource: &Symbol,
    fields: &[Symbol],
    presence: bool,
) -> MethodDef {
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
        // `<field>_provided` — present in the hash AND a String. Rails'
        // `permit` keeps a blank `""` and `compact` does NOT drop it
        // (measured against 8.1), so blank counts as provided; an
        // absent key and an explicit JSON null do not. A missing key
        // fetches as `""`, which `is_a?(String)` can't tell from a
        // blank one, so the hash is asked directly.
        if presence {
            let key_present = with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(var(&sub, inner_hash_ty.clone())),
                        method: Symbol::from("key?"),
                        args: vec![str_lit(field.as_str())],
                        block: None,
                        parenthesized: true,
                    },
                ),
                Ty::Bool,
            );
            let is_string = with_ty(
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
            stmts.push(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var(&instance, owner_ty.clone())),
                    method: Symbol::from(format!("{}=", provided_field(field).as_str())),
                    args: vec![with_ty(
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::BoolOp {
                                op: crate::expr::BoolOpKind::And,
                                surface: crate::expr::BoolOpSurface::default(),
                                left: key_present,
                                right: is_string,
                            },
                        ),
                        Ty::Bool,
                    )],
                    block: None,
                    parenthesized: false,
                },
            ));
        }
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
            block_param: None,
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
                hint: None,
                decisions: 0,
            };
            let value = Expr {
                span: Span::synthetic(),
                node: Box::new(ExprNode::Ivar { name: field.clone() }),
                ty: Some(Ty::Str),
                effects: EffectSet::default(),
                leading_blank_line: false,
                diagnostic: None,
                hint: None,
                decisions: 0,
            };
            (key, value)
        })
        .collect();
    let hash_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Str),
    };
    let hash = Expr {
        span: Span::synthetic(),
        node: Box::new(ExprNode::Hash { entries, kwargs: false }),
        ty: Some(hash_ty.clone()),
        effects: EffectSet::default(),
        leading_blank_line: false,
        diagnostic: None,
        hint: None,
        decisions: 0,
    };
    let ret_ty = hash_ty;
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
            block_param: None,
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
pub fn rewrite_to_from_raw(expr: &Expr, specs: &ParamsSpecs) -> Expr {
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
                    if let Some(spec) = specs.find(&resource, &fields) {
                        if spec.wants_compact {
                            return Some(build_from_raw_call(&spec.class_id, e.span));
                        }
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
                let spec = specs.find(&resource, &fields)?;
                let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
                return Some(build_from_raw_merge(&spec.class_id, entries, e.span));
            }
        }
        let (resource, fields) = match_permit_call(e)?;
        let spec = specs.find(&resource, &fields)?;
        Some(build_from_raw_call(&spec.class_id, e.span))
    })
}

/// `<chain>.merge(k: v)` → `_p = <Class>.from_raw(@params); _p.k = v;
/// _p` — a statement-shaped Seq; the corpus site is a params-helper
/// tail, where the Seq renders as plain statements. The setters run
/// after `from_raw`, so a client-supplied value under the same key is
/// overwritten (Rails' merge contract).
fn build_from_raw_merge(class_id: &ClassId, entries: &[(Expr, Expr)], span: Span) -> Expr {
    let p = |()| Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_p") });
    let mut stmts = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_p") },
            value: build_from_raw_call(class_id, span),
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
                value: v.clone(),
            },
        ));
    }
    stmts.push(p(()));
    Expr::new(span, ExprNode::Seq { exprs: stmts })
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
pub fn rewrite_typed_bracket_to_field(expr: &Expr, specs: &ParamsSpecs) -> Expr {
    use crate::ty::Ty;
    // Build a quick `class_id -> permitted-fields-set` lookup so the
    // walker can validate the literal key is one of the permitted
    // fields before rewriting.
    let mut permitted_fields: std::collections::HashMap<
        ClassId,
        (std::collections::HashSet<String>, Ty),
    > = std::collections::HashMap::new();
    for spec in specs.iter() {
        let mut set = std::collections::HashSet::new();
        for f in &spec.fields {
            set.insert(f.as_str().to_string());
        }
        permitted_fields.insert(spec.class_id.clone(), (set, Ty::Str));
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
        let (fields, slot_ty) = permitted_fields.get(recv_class_id)?;
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
            ty: Some(slot_ty.clone()),
            effects: e.effects.clone(),
            leading_blank_line: e.leading_blank_line,
            diagnostic: None,
            hint: None,
            decisions: 0,
        })
    })
}
