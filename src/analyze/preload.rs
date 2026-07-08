//! Static N+1 detection (#64): `missing_preload` findings over the
//! typed query chain.
//!
//! When code iterates a relation and reads a member association the
//! originating query didn't `includes`/`preload`/`eager_load`, warn,
//! naming both sites and the one-line fix. Bullet/Prosopite observe
//! this at runtime on the traffic you happened to generate; this
//! proves it from source, whole-app, pre-deploy — and it exercises
//! exactly the inference that distinguishes roundhouse: the relation
//! is typed (`Array[Status]`), the association is known from the
//! model registry (concern-declared included), and the controller→
//! view ivar channel carries the query across the procedure boundary
//! where most real N+1s live (query in the controller, iteration in
//! the template).
//!
//! Precision posture (severity is Warning, never Error):
//! - The preload set is harvested *syntactically* from the chain
//!   (`includes`/`preload`/`eager_load` collect; `where`/`order`/…
//!   preserve; named scopes recurse into their bodies, depth-bound).
//!   A chain that passes through anything unrecognized — `merge`,
//!   custom class methods, `Arel` — is **opaque**: silently skipped,
//!   never reported as "no preloads". Not-modeled ≠ absent (the
//!   diagnostics-as-ledger rule applied to this analysis's own gaps).
//! - Single-hop only: `s.account` needing `includes(:account)`.
//!   Nested access (`s.account.avatar`) is a later phase.
//! - Cross-procedure findings require the association missing from
//!   **every** feeding action's preload set — a shared template whose
//!   `index` preloads but `search` doesn't stays silent rather than
//!   accusing the preloading path. Under-reports by design.
//! - `find_each`/`in_batches` preserve the chain (batching doesn't
//!   preload). `strict_loading`, `default_scope` preloads, and manual
//!   `Preloader` calls are not modeled; chains through them go opaque.

use std::collections::{BTreeSet, HashMap};

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::dialect::{ModelBodyItem, Scope};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// What we know about the relation an expression evaluates to.
#[derive(Clone, Debug, PartialEq)]
enum ChainInfo {
    /// The chain was fully recognized: these associations are
    /// preloaded, and the query originates at `origin`.
    Known { preloads: BTreeSet<Symbol>, origin: Span },
    /// The chain passed through something this pass doesn't model —
    /// no claim can be made either way.
    Opaque,
}

/// Per-model surface the detector consults: association names (own +
/// concern-folded) and scope bodies (own + concern-folded) for the
/// chain harvest.
struct ModelIndex<'a> {
    assocs: HashMap<ClassId, BTreeSet<Symbol>>,
    scopes: HashMap<ClassId, HashMap<Symbol, &'a Scope>>,
}

impl<'a> ModelIndex<'a> {
    fn build(app: &'a App) -> Self {
        let mut assocs: HashMap<ClassId, BTreeSet<Symbol>> = HashMap::new();
        let mut scopes: HashMap<ClassId, HashMap<Symbol, &'a Scope>> = HashMap::new();
        let concern_items = &app.concern_model_items;
        for model in &app.models {
            let a = assocs.entry(model.name.clone()).or_default();
            let s = scopes.entry(model.name.clone()).or_default();
            let mut fold = |items: &'a [ModelBodyItem]| {
                for item in items {
                    match item {
                        ModelBodyItem::Association { assoc, .. } => {
                            a.insert(assoc.name().clone());
                        }
                        ModelBodyItem::Scope { scope, .. } => {
                            s.insert(scope.name.clone(), scope);
                        }
                        _ => {}
                    }
                }
            };
            fold(&model.body);
            for module in super::model_includes(model) {
                if let Some(items) = concern_items.get(&module) {
                    fold(items);
                }
            }
        }
        ModelIndex { assocs, scopes }
    }

    fn is_model(&self, id: &ClassId) -> bool {
        self.assocs.contains_key(id)
    }
}

/// Block-taking enumerators whose block parameter binds one member of
/// the receiver collection. Deliberately the common-iteration core;
/// a miss here only under-reports.
const ITERATORS: &[&str] = &[
    "each", "map", "flat_map", "collect", "select", "filter", "reject", "find", "detect",
    "any?", "all?", "none?", "sum", "min_by", "max_by", "sort_by", "group_by", "index_by",
    "each_with_index", "each_with_object", "partition", "find_each", "count", "take_while",
    "drop_while",
];

/// Chain links that preserve the relation (and its preload set)
/// unchanged. Anything not listed here, not a preloader, and not a
/// recognized scope makes the chain opaque.
const PRESERVERS: &[&str] = &[
    "where", "not", "order", "reorder", "limit", "offset", "joins", "left_joins",
    "left_outer_joins", "references", "distinct", "group", "having", "all", "unscope",
    "rewhere", "readonly", "strict_loading", "in_batches", "with_discarded", "kept",
    "page", "per", "paginate",
];

const PRELOADERS: &[&str] = &["includes", "preload", "eager_load"];

/// The coverage triple's raw counts (#64: "checked N chains, M
/// findings, K unverifiable" — a clean report is only actionable with
/// its denominator). `iteration_sites` = block-iterations seen over a
/// model-typed collection; each either resolved to a `known` chain
/// (checked) or an `opaque` one (unverifiable — no claim made).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreloadCoverage {
    pub iteration_sites: usize,
    pub known_chains: usize,
    pub opaque_chains: usize,
    pub findings: usize,
}

/// Entry point — called from [`super::diagnose_with_coverage`] (the
/// findings ride the diagnostics ledger, the coverage triple rides the
/// report skins) and from `ide::traceroute` (findings annotate the hop
/// containing the access site). Emits one Warning per (iteration site,
/// association) with both spans on the finding: anchored at the access
/// site, naming the query site in the message.
pub fn missing_preload_report(app: &App) -> (Vec<Diagnostic>, PreloadCoverage) {
    let index = ModelIndex::build(app);
    let mut out = Vec::new();
    let mut cov = PreloadCoverage::default();

    // Same-procedure: controller actions, model methods/scopes — the
    // local env threads ivar/local assignments walked in order, so
    // `@statuses = Status.recent; @statuses.each {…}` resolves within
    // one body.
    for controller in &app.controllers {
        // Ivars are request-wide state: `before_action :set_requests`
        // binds the @requests an action then iterates. Seed every
        // action's env with the controller-wide union (an ivar bound
        // to different chains in different methods drops — ambiguous).
        let base_env = controller_ivar_env(controller, &index);
        for action in controller.actions() {
            let mut env = base_env.clone();
            walk_body(&action.body, &index, &mut env, app, &mut out, &mut cov);
        }
    }
    for model in &app.models {
        for method in model.methods() {
            let mut env = HashMap::new();
            walk_body(&method.body, &index, &mut env, app, &mut out, &mut cov);
        }
    }

    // Cross-procedure: each view's ivar env is the intersection of
    // what its feeding actions bound — a finding requires the preload
    // missing from every feeder (see module docs).
    let view_envs = build_view_envs(app, &index);
    for view in &app.views {
        let mut env = view_envs.get(&view.name).cloned().unwrap_or_default();
        walk_body(&view.body, &index, &mut env, app, &mut out, &mut cov);
    }

    // A chain expression re-walked through nested Seq/If wrappers can
    // fire twice at one site; collapse exact duplicates.
    out.sort_by_key(|d| (d.span.file.0, d.span.start, d.message.clone()));
    out.dedup_by(|a, b| a.span == b.span && a.message == b.message);
    cov.findings = out.len();
    (out, cov)
}

/// The instance-typed model class of `M` / `M?`.
fn instance_model(ty: Option<&Ty>) -> Option<&ClassId> {
    let ty = match ty? {
        Ty::Union { variants } => variants.iter().find(|v| !matches!(v, Ty::Nil))?,
        other => other,
    };
    match ty {
        Ty::Class { id, .. } => Some(id),
        _ => None,
    }
}

/// The relation-typed element class of `Array[M]` / `Array[M]?`.
fn relation_elem(ty: Option<&Ty>) -> Option<&ClassId> {
    let ty = match ty? {
        Ty::Union { variants } => variants.iter().find(|v| !matches!(v, Ty::Nil))?,
        other => other,
    };
    match ty {
        Ty::Array { elem } => match elem.as_ref() {
            Ty::Class { id, .. } => Some(id),
            _ => None,
        },
        _ => None,
    }
}

/// Harvest the preload set of a query-chain expression. `Some(Known)`
/// only when every link is recognized down to a model-class (or
/// typed-association) base.
fn harvest_chain(
    expr: &Expr,
    index: &ModelIndex,
    env: &HashMap<Symbol, ChainInfo>,
    depth: u32,
) -> ChainInfo {
    if depth > 6 {
        return ChainInfo::Opaque;
    }
    match &*expr.node {
        // Chain base: the model class itself (`Status`).
        ExprNode::Const { path } => {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            if index.is_model(&ClassId(Symbol::from(joined))) {
                ChainInfo::Known { preloads: BTreeSet::new(), origin: expr.span }
            } else {
                ChainInfo::Opaque
            }
        }
        // A local/ivar the walked body previously bound to a known chain.
        ExprNode::Var { name, .. } => env.get(name).cloned().unwrap_or(ChainInfo::Opaque),
        ExprNode::Ivar { name, .. } => env.get(name).cloned().unwrap_or(ChainInfo::Opaque),
        ExprNode::Send { recv, method, args, .. } => {
            let m = method.as_str();
            if PRELOADERS.contains(&m) {
                let Some(recv) = recv else { return ChainInfo::Opaque };
                match harvest_chain(recv, index, env, depth + 1) {
                    ChainInfo::Known { mut preloads, origin } => {
                        collect_preload_args(args, &mut preloads);
                        ChainInfo::Known { preloads, origin }
                    }
                    ChainInfo::Opaque => ChainInfo::Opaque,
                }
            } else if PRESERVERS.contains(&m) {
                let Some(recv) = recv else { return ChainInfo::Opaque };
                harvest_chain(recv, index, env, depth + 1)
            } else if let Some(recv) = recv {
                // Association read on a typed model instance starts a
                // fresh chain (`@event.severed_relationships.…`,
                // `current_user.webauthn_credentials.each`).
                if let Some(owner) = instance_model(recv.ty.as_ref()) {
                    if index.assocs.get(owner).is_some_and(|a| a.contains(method)) {
                        return ChainInfo::Known {
                            preloads: BTreeSet::new(),
                            origin: expr.span,
                        };
                    }
                }
                // Named scope on a chain that bottoms at model M: fold
                // the scope body's own preloads in (its implicit-self
                // base contributes nothing) and keep walking.
                let base = harvest_chain(recv, index, env, depth + 1);
                let ChainInfo::Known { mut preloads, origin } = base else {
                    return ChainInfo::Opaque;
                };
                let Some(model) = chain_model(recv, index) else { return ChainInfo::Opaque };
                if let Some(scope) = index.scopes.get(&model).and_then(|s| s.get(&Symbol::from(m)))
                {
                    match harvest_scope_body(&scope.body, index, depth + 1) {
                        Some(scope_preloads) => {
                            preloads.extend(scope_preloads);
                            ChainInfo::Known { preloads, origin }
                        }
                        None => ChainInfo::Opaque,
                    }
                } else if index
                    .assocs
                    .get(&model)
                    .is_some_and(|a| a.contains(&Symbol::from(m)))
                {
                    // Association read as a new chain base
                    // (`@account.statuses.includes(…)` walks through
                    // here when `statuses` is the recv of `includes`).
                    ChainInfo::Known { preloads: BTreeSet::new(), origin: expr.span }
                } else {
                    ChainInfo::Opaque
                }
            } else {
                ChainInfo::Opaque
            }
        }
        _ => ChainInfo::Opaque,
    }
}

/// The model class a recognized chain bottoms out at, for scope-name
/// resolution. Mirrors `harvest_chain`'s link set; `None` when the
/// base isn't a model class or typed association.
fn chain_model(expr: &Expr, index: &ModelIndex) -> Option<ClassId> {
    match &*expr.node {
        ExprNode::Const { path } => {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            let id = ClassId(Symbol::from(joined));
            index.is_model(&id).then_some(id)
        }
        ExprNode::Send { recv, method, .. } => {
            let m = method.as_str();
            if PRELOADERS.contains(&m) || PRESERVERS.contains(&m) {
                return chain_model(recv.as_ref()?, index);
            }
            let inner = chain_model(recv.as_ref()?, index)?;
            if index.scopes.get(&inner).is_some_and(|s| s.contains_key(&Symbol::from(m))) {
                return Some(inner); // scopes return the same relation class
            }
            None
        }
        ExprNode::Var { .. } | ExprNode::Ivar { .. } => {
            // Typed binding: the element class rides the type.
            relation_elem(expr.ty.as_ref()).cloned()
        }
        _ => None,
    }
}

/// Preloads contributed by a scope body (`scope :with_account, -> {
/// includes(:account) }`). The body is a chain over implicit self;
/// `None` when it passes through anything unrecognized.
fn harvest_scope_body(body: &Expr, _index: &ModelIndex, depth: u32) -> Option<BTreeSet<Symbol>> {
    if depth > 6 {
        return None;
    }
    let body = match &*body.node {
        ExprNode::Lambda { body, .. } => body,
        _ => body,
    };
    let mut preloads = BTreeSet::new();
    let mut cur = body;
    loop {
        match &*cur.node {
            ExprNode::Send { recv, method, args, .. } => {
                let m = method.as_str();
                if PRELOADERS.contains(&m) {
                    collect_preload_args(args, &mut preloads);
                } else if !PRESERVERS.contains(&m) {
                    return None; // scope chains through something unmodeled
                }
                match recv {
                    Some(r) => cur = r,
                    None => return Some(preloads), // implicit-self base
                }
            }
            _ => return None,
        }
    }
}

/// `includes(:account)`, `includes(:account, :media)`,
/// `includes(account: :avatar)` (top-level keys — single-hop),
/// `includes([:a, :b])`.
fn collect_preload_args(args: &[Expr], out: &mut BTreeSet<Symbol>) {
    for arg in args {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                out.insert(value.clone());
            }
            ExprNode::Hash { entries, .. } => {
                for (k, _) in entries {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        out.insert(value.clone());
                    }
                }
            }
            ExprNode::Array { elements, .. } => collect_preload_args(elements, out),
            _ => {}
        }
    }
}

/// Walk one body in evaluation order, threading `env` (ivar/local →
/// chain info) through assignments and reporting missing preloads at
/// iteration sites.
fn walk_body(
    expr: &Expr,
    index: &ModelIndex,
    env: &mut HashMap<Symbol, ChainInfo>,
    app: &App,
    out: &mut Vec<Diagnostic>,
    cov: &mut PreloadCoverage,
) {
    match &*expr.node {
        ExprNode::Assign { target, value } => {
            walk_body(value, index, env, app, out, cov);
            let name = match target {
                LValue::Ivar { name } => Some(name),
                LValue::Var { name, .. } => Some(name),
                _ => None,
            };
            if let Some(name) = name {
                match harvest_chain(value, index, env, 0) {
                    info @ ChainInfo::Known { .. } => {
                        env.insert(name.clone(), info);
                    }
                    ChainInfo::Opaque => {
                        env.remove(name);
                    }
                }
            }
        }
        ExprNode::Send { recv, method, block, args, .. } => {
            if let Some(r) = recv {
                walk_body(r, index, env, app, out, cov);
            }
            for a in args {
                walk_body(a, index, env, app, out, cov);
            }
            if let (Some(r), Some(b)) = (recv, block) {
                if ITERATORS.contains(&method.as_str()) {
                    check_iteration(r, b, index, env, app, out, cov);
                }
                walk_body(b, index, env, app, out, cov);
            } else if let Some(b) = block {
                walk_body(b, index, env, app, out, cov);
            }
        }
        _ => {
            expr.node.for_each_child(&mut |c| walk_body(c, index, env, app, out, cov));
        }
    }
}

/// One iteration site: receiver must be a typed model relation with a
/// recognized chain; every single-hop association read on the block
/// param that isn't preloaded is a finding.
fn check_iteration(
    recv: &Expr,
    block: &Expr,
    index: &ModelIndex,
    env: &HashMap<Symbol, ChainInfo>,
    app: &App,
    out: &mut Vec<Diagnostic>,
    cov: &mut PreloadCoverage,
) {
    let Some(model) = relation_elem(recv.ty.as_ref()) else { return };
    let Some(assocs) = index.assocs.get(model) else { return };
    cov.iteration_sites += 1;
    let ChainInfo::Known { preloads, origin } = harvest_chain(recv, index, env, 0) else {
        cov.opaque_chains += 1;
        return; // opaque: no claim either way
    };
    cov.known_chains += 1;
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return };
    let Some(member) = params.first() else { return };

    let mut reads: Vec<(Symbol, Span)> = Vec::new();
    collect_member_assoc_reads(body, member, assocs, &mut reads);
    reads.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.start.cmp(&b.1.start)));
    reads.dedup_by(|a, b| a.0 == b.0);
    for (assoc, span) in reads {
        if preloads.contains(&assoc) {
            continue;
        }
        let query_site = render_site(app, origin);
        let message = format!(
            "iterating this relation reads `{}.{}`, but the query{} does not \
             preload :{} — add `.includes(:{})`",
            member.as_str(),
            assoc.as_str(),
            query_site.map(|s| format!(" at {s}")).unwrap_or_default(),
            assoc.as_str(),
            assoc.as_str(),
        );
        out.push(Diagnostic {
            span,
            kind: DiagnosticKind::MissingPreload { association: assoc, query_span: origin },
            severity: Severity::Warning,
            message,
        });
    }
}

/// `member.assoc` reads inside the block body (single hop, direct
/// receiver only). Descends everything including nested blocks — a
/// read inside a nested `map` still runs per member.
fn collect_member_assoc_reads(
    expr: &Expr,
    member: &Symbol,
    assocs: &BTreeSet<Symbol>,
    out: &mut Vec<(Symbol, Span)>,
) {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if let ExprNode::Var { name, .. } = &*r.node {
            if name == member && assocs.contains(method) {
                out.push((method.clone(), expr.span));
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_member_assoc_reads(c, member, assocs, out));
}

fn render_site(app: &App, span: Span) -> Option<String> {
    if span.is_synthetic() {
        return None;
    }
    let src = app.sources.get((span.file.0 as usize).checked_sub(1)?)?;
    let (line, _) = src.line_col(span.start);
    Some(format!("{}:{line}", src.path))
}

/// View ivar envs from the controller→view channel: for each action,
/// harvest every ivar whose assigned expression is a recognized chain
/// and key it by the action's view. Multiple feeders intersect —
/// preload set = intersection, and an ivar any feeder binds opaquely
/// drops out entirely (missing-from-ALL rule).
fn build_view_envs(
    app: &App,
    index: &ModelIndex,
) -> HashMap<Symbol, HashMap<Symbol, ChainInfo>> {
    let mut envs: HashMap<Symbol, HashMap<Symbol, ChainInfo>> = HashMap::new();
    let mut seen_feeders: HashMap<Symbol, u32> = HashMap::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            let Some(view) = super::view_name_for_action(&controller.name, action) else {
                continue;
            };
            // Harvest this action's chain-bound ivars (walk in order so
            // locals feeding ivars resolve), layered over the
            // controller-wide env (filters bind ivars views iterate).
            let mut local = controller_ivar_env(controller, index);
            harvest_assignments(&action.body, index, &mut local);
            let n = seen_feeders.entry(view.clone()).or_insert(0);
            *n += 1;
            let entry = envs.entry(view.clone()).or_default();
            if *n == 1 {
                *entry = local;
            } else {
                // Intersect: keep ivars every feeder bound to a known
                // chain, with the intersection of their preloads.
                entry.retain(|k, v| {
                    let (Some(ChainInfo::Known { preloads: theirs, .. }),
                         ChainInfo::Known { preloads, .. }) = (local.get(k), v)
                    else {
                        return false;
                    };
                    preloads.retain(|p| theirs.contains(p));
                    true
                });
            }
        }
    }
    envs
}

/// Controller-wide ivar→chain env: the union over every method's
/// chain-bound ivars, dropping names bound inconsistently. Mirrors the
/// controller-wide ivar *type* seeding in `run_typing_passes` — same
/// Ruby semantics (ivars are shared mutable request state), applied to
/// the preload fact.
fn controller_ivar_env(
    controller: &crate::dialect::Controller,
    index: &ModelIndex,
) -> HashMap<Symbol, ChainInfo> {
    let mut merged: HashMap<Symbol, ChainInfo> = HashMap::new();
    let mut dropped: BTreeSet<Symbol> = BTreeSet::new();
    for action in controller.actions() {
        let mut local: HashMap<Symbol, ChainInfo> = HashMap::new();
        harvest_assignments(&action.body, index, &mut local);
        for (k, v) in local {
            if dropped.contains(&k) {
                continue;
            }
            match merged.get(&k) {
                None => {
                    merged.insert(k, v);
                }
                Some(prev) if *prev == v => {}
                Some(_) => {
                    merged.remove(&k);
                    dropped.insert(k);
                }
            }
        }
    }
    merged
}

/// Ivar/local chain bindings from one body, in evaluation order.
fn harvest_assignments(expr: &Expr, index: &ModelIndex, env: &mut HashMap<Symbol, ChainInfo>) {
    if let ExprNode::Assign { target, value } = &*expr.node {
        harvest_assignments(value, index, env);
        let name = match target {
            LValue::Ivar { name } => Some(name),
            LValue::Var { name, .. } => Some(name),
            _ => None,
        };
        if let Some(name) = name {
            match harvest_chain(value, index, env, 0) {
                info @ ChainInfo::Known { .. } => {
                    env.insert(name.clone(), info);
                }
                ChainInfo::Opaque => {
                    env.remove(name);
                }
            }
        }
        return;
    }
    expr.node.for_each_child(&mut |c| harvest_assignments(c, index, env));
}
