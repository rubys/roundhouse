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
//! - A collection RENDER is an iteration: `render @microposts` (or
//!   `render partial: …, collection: …`) runs the partial once per
//!   member with the member bound to the partial's local, so the
//!   partial's body is walked as the block body. The Rails tutorial
//!   iterates every collection this way and nothing else.
//! - A model METHOD whose body ends in a recognizable chain
//!   (`User#feed` → `Micropost.where(…).includes(:user, …)`) is
//!   harvested like a scope, so `current_user.feed` carries its
//!   preloads; a method whose tail is anything else stays opaque.
//! - Active Storage: `micropost.image` needs `image_attachment`
//!   preloaded (`includes(image_attachment: :blob)` or
//!   `with_attached_image`) — the attachment is an association under
//!   another name, and the read is `attached?`/`variant` per row.

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
    /// preloaded (explicitly, or implicitly — see
    /// [`inverse_of_owner`]), and the query originates at `origin`.
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
    /// The declarations behind `assocs`, for the inverse rule.
    decls: HashMap<ClassId, Vec<&'a crate::dialect::Association>>,
    scopes: HashMap<ClassId, HashMap<Symbol, &'a Scope>>,
    /// `has_one_attached :image` / `has_many_attached :files` per
    /// model: the reader name → the association Rails preloads it
    /// through (`image_attachment`, `files_attachments`).
    attachments: HashMap<ClassId, HashMap<Symbol, Symbol>>,
    /// Model methods (instance or class side) whose body ends in a
    /// recognizable chain, harvested once: `User#feed`'s preloads ride
    /// every `current_user.feed` read.
    chain_methods: HashMap<ClassId, HashMap<Symbol, ChainInfo>>,
}

impl<'a> ModelIndex<'a> {
    fn build(app: &'a App) -> Self {
        let mut assocs: HashMap<ClassId, BTreeSet<Symbol>> = HashMap::new();
        let mut decls: HashMap<ClassId, Vec<&'a crate::dialect::Association>> = HashMap::new();
        let mut scopes: HashMap<ClassId, HashMap<Symbol, &'a Scope>> = HashMap::new();
        let mut attachments: HashMap<ClassId, HashMap<Symbol, Symbol>> = HashMap::new();
        let concern_items = &app.concern_model_items;
        for model in &app.models {
            let a = assocs.entry(model.name.clone()).or_default();
            let d = decls.entry(model.name.clone()).or_default();
            let s = scopes.entry(model.name.clone()).or_default();
            let at = attachments.entry(model.name.clone()).or_default();
            let mut fold = |items: &'a [ModelBodyItem]| {
                for item in items {
                    match item {
                        ModelBodyItem::Association { assoc, .. } => {
                            a.insert(assoc.name().clone());
                            d.push(assoc);
                        }
                        ModelBodyItem::Scope { scope, .. } => {
                            s.insert(scope.name.clone(), scope);
                        }
                        ModelBodyItem::Unknown { expr, .. } => {
                            if let Some((attr, key)) = attached_decl(expr) {
                                at.insert(attr, key);
                            }
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
        let mut index =
            ModelIndex { assocs, decls, scopes, attachments, chain_methods: HashMap::new() };
        // Second pass: methods whose tail is a chain, harvested against
        // the associations/scopes just indexed. A method body binds no
        // env of its own here (locals feeding the tail — the tutorial's
        // `following_ids = "…"; Micropost.where(…)` — are SQL strings,
        // not chains).
        let mut chain_methods: HashMap<ClassId, HashMap<Symbol, ChainInfo>> = HashMap::new();
        for model in &app.models {
            for method in model.methods() {
                let tail = body_tail(&method.body);
                if let info @ ChainInfo::Known { .. } =
                    harvest_chain(tail, &index, &HashMap::new(), 0)
                {
                    chain_methods
                        .entry(model.name.clone())
                        .or_default()
                        .insert(method.name.clone(), info);
                }
            }
        }
        index.chain_methods = chain_methods;
        index
    }

    fn is_model(&self, id: &ClassId) -> bool {
        self.assocs.contains_key(id)
    }

    /// Rails' automatic `inverse_of`: records loaded through
    /// `owner.<has_many>` answer the inverse `belongs_to` with the
    /// already-loaded owner, no query — so `micropost.user` inside
    /// `render @user.microposts` is not an N+1, and reporting it would
    /// be the first thing a Rails reader disproves. Detected the way
    /// Rails does (`automatic_inverse_of`): a plain `has_many` (no
    /// `through:`, no scope) whose target declares a `belongs_to` back
    /// to the owner's class on the same foreign key. Returns that
    /// association's name as an implicit preload; empty otherwise.
    fn inverse_of_owner(&self, owner: &ClassId, assoc: &Symbol) -> Option<Symbol> {
        use crate::dialect::Association;
        let decl = self.decls.get(owner)?.iter().find(|a| a.name() == assoc)?;
        let Association::HasMany { target, foreign_key, through: None, scope: None, .. } = decl
        else {
            return None;
        };
        self.decls.get(target)?.iter().find_map(|a| match a {
            Association::BelongsTo { name, target: t, foreign_key: fk, polymorphic: false, .. }
                if t == owner && fk == foreign_key =>
            {
                Some(name.clone())
            }
            _ => None,
        })
    }

    /// Is `method`, sent to `member.<assoc>`, a query that preloading
    /// cannot serve? A [`SQL_TAILS`] entry, or a scope of the
    /// association's target model (`column.cards.active`).
    fn is_sql_tail(&self, owner: &ClassId, assoc: &Symbol, method: &Symbol) -> bool {
        use crate::dialect::Association;
        if SQL_TAILS.contains(&method.as_str()) {
            return true;
        }
        let Some(decl) = self.decls.get(owner).and_then(|d| d.iter().find(|a| a.name() == assoc))
        else {
            return false;
        };
        let target = match decl {
            Association::HasMany { target, .. }
            | Association::HasAndBelongsToMany { target, .. } => target,
            _ => return false,
        };
        self.scopes.get(target).is_some_and(|s| s.contains_key(method))
    }

    /// Every per-member read this pass checks on `model`, mapped to the
    /// preload key that satisfies it: an association to itself, an
    /// attachment reader to its `<attr>_attachment(s)` association.
    fn read_keys(&self, model: &ClassId) -> Option<HashMap<Symbol, Symbol>> {
        let assocs = self.assocs.get(model)?;
        let mut out: HashMap<Symbol, Symbol> =
            assocs.iter().map(|a| (a.clone(), a.clone())).collect();
        if let Some(at) = self.attachments.get(model) {
            for (attr, key) in at {
                out.insert(attr.clone(), key.clone());
            }
        }
        Some(out)
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

/// Methods on a loaded collection that go back to SQL anyway:
/// `product.subscribers.count` runs `SELECT COUNT(*)` per row whether
/// or not `:subscribers` was preloaded (`size`/`length`/`any?` read
/// the loaded target; `count` never does). A read under one of these
/// is still an N+1, but `.includes` is not its fix, and a message
/// that says so is worse than none — the reader who knows Rails stops
/// trusting the rest. Not here: `first`/`last`/`take`, `any?`/`empty?`,
/// `size`/`length`, which read the loaded target when there is one.
const SQL_TAILS: &[&str] = &[
    "count", "sum", "minimum", "maximum", "average", "calculate", "pluck", "pick", "ids",
    "exists?", "where", "not", "order", "reorder", "limit", "offset", "group", "distinct",
    "find", "find_by", "find_by!", "select", "joins", "left_joins",
];

/// `has_one_attached :image` → (`image`, `image_attachment`);
/// `has_many_attached :files` → (`files`, `files_attachments`) — the
/// reader and the association Rails' `with_attached_<attr>` preloads.
fn attached_decl(expr: &Expr) -> Option<(Symbol, Symbol)> {
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
    let [arg] = args.as_slice() else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: attr } } = &*arg.node else { return None };
    let key = match method.as_str() {
        "has_one_attached" => crate::lower::attached::attachment_assoc_name(attr),
        "has_many_attached" => Symbol::from(format!("{}_attachments", attr.as_str())),
        _ => return None,
    };
    Some((attr.clone(), key))
}

/// A body's value: the last expression of a `Seq`, through `Return`.
fn body_tail(body: &Expr) -> &Expr {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().map(body_tail).unwrap_or(body),
        ExprNode::Return { value } => body_tail(value),
        _ => body,
    }
}

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
            walk_body(&action.body, &index, &mut env, app, None, &mut out, &mut cov);
        }
    }
    for model in &app.models {
        for method in model.methods() {
            let mut env = HashMap::new();
            walk_body(&method.body, &index, &mut env, app, None, &mut out, &mut cov);
        }
    }

    // Cross-procedure: each view's ivar env is the intersection of
    // what its feeding actions bound — a finding requires the preload
    // missing from every feeder (see module docs). Partials inherit
    // their renderers' envs the same way (a partial renders in its
    // parent's view context and reads its ivars).
    let view_envs = build_view_envs(app, &index);
    for view in &app.views {
        let mut env = view_envs.get(&view.name).cloned().unwrap_or_default();
        walk_body(&view.body, &index, &mut env, app, Some(&view.name), &mut out, &mut cov);
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

/// The relation-typed element class of `Relation[M]` / `Array[M]`
/// (and their nilable forms).
///
/// Both spellings reach here: a lazy chain is `Relation { of }`, and a
/// materialized collection — `to_a`, an association read, a scope the
/// seed classifier could not recognize — is `Array[M]`. An N+1 is the
/// same defect either way (iterate a collection of `M`, touch an
/// association per element), so both answer with the element class.
fn relation_elem(ty: Option<&Ty>) -> Option<&ClassId> {
    let ty = match ty? {
        Ty::Union { variants } => variants.iter().find(|v| !matches!(v, Ty::Nil))?,
        other => other,
    };
    match ty {
        Ty::Relation { of } => Some(of),
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
                            preloads: index.inverse_of_owner(owner, method).into_iter().collect(),
                            origin: expr.span,
                        };
                    }
                    // A model method whose body is a chain
                    // (`current_user.feed`) — its preloads and origin
                    // are the method's.
                    if let Some(info) =
                        index.chain_methods.get(owner).and_then(|m| m.get(method))
                    {
                        return info.clone();
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
                // `with_attached_<attr>` — the scope Active Storage's
                // macro declares: `includes(<attr>_attachment: :blob)`.
                if let Some(attr) = m.strip_prefix("with_attached_") {
                    if let Some(key) = index
                        .attachments
                        .get(&model)
                        .and_then(|at| at.get(&Symbol::from(attr)))
                    {
                        preloads.insert(key.clone());
                        return ChainInfo::Known { preloads, origin };
                    }
                }
                // A class-side chain method (`Model.visible_to(user)`)
                // contributes its own preloads like a scope.
                if let Some(ChainInfo::Known { preloads: theirs, .. }) =
                    index.chain_methods.get(&model).and_then(|c| c.get(&Symbol::from(m)))
                {
                    preloads.extend(theirs.iter().cloned());
                    return ChainInfo::Known { preloads, origin };
                }
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
            if index.scopes.get(&inner).is_some_and(|s| s.contains_key(&Symbol::from(m)))
                || m.starts_with("with_attached_")
                || index.chain_methods.get(&inner).is_some_and(|c| c.contains_key(&Symbol::from(m)))
            {
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
    view: Option<&Symbol>,
    out: &mut Vec<Diagnostic>,
    cov: &mut PreloadCoverage,
) {
    match &*expr.node {
        ExprNode::Assign { target, value } => {
            walk_body(value, index, env, app, view, out, cov);
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
                walk_body(r, index, env, app, view, out, cov);
            }
            for a in args {
                walk_body(a, index, env, app, view, out, cov);
            }
            if let (Some(r), Some(b)) = (recv, block) {
                if ITERATORS.contains(&method.as_str()) {
                    if let ExprNode::Lambda { params, body, .. } = &*b.node {
                        if let Some(member) = params.first() {
                            check_members(r, body, member, "iterating", index, env, app, out, cov);
                        }
                    }
                }
                walk_body(b, index, env, app, view, out, cov);
            } else if let Some(b) = block {
                walk_body(b, index, env, app, view, out, cov);
            }
            // A collection render is an iteration whose block is the
            // partial's body and whose block parameter is the partial's
            // local: `render @microposts` runs `_micropost` once per
            // member.
            if recv.is_none() && method.as_str() == "render" {
                if let Some((coll, partial, local)) = collection_render(args, view) {
                    if let Some(pv) = app.views.iter().find(|v| v.name == partial) {
                        check_members(coll, &pv.body, &local, "rendering", index, env, app, out, cov);
                    }
                }
            }
        }
        _ => {
            expr.node.for_each_child(&mut |c| walk_body(c, index, env, app, view, out, cov));
        }
    }
}

/// The collection, partial view name and member local of a collection
/// render: `render @items` (partial and local from the element class,
/// as Rails derives them) or `render partial: "name", collection:
/// items[, as: :member]` (partial relative to the rendering view).
fn collection_render<'e>(
    args: &'e [Expr],
    view: Option<&Symbol>,
) -> Option<(&'e Expr, Symbol, Symbol)> {
    let first = args.first()?;
    if let ExprNode::Hash { entries, .. } = &*first.node {
        let mut partial: Option<String> = None;
        let mut coll: Option<&Expr> = None;
        let mut as_name: Option<Symbol> = None;
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { continue };
            match key.as_str() {
                "partial" => {
                    if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                        partial = Some(value.clone());
                    }
                }
                "collection" => coll = Some(v),
                "as" => {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                        as_name = Some(value.clone());
                    }
                }
                _ => {}
            }
        }
        let (partial, coll) = (partial?, coll?);
        let resolved = super::render::resolve_partial_path(&partial, view?);
        let local = as_name.unwrap_or_else(|| {
            Symbol::from(resolved.rsplit("/_").next().unwrap_or(&resolved))
        });
        return Some((coll, Symbol::from(resolved.as_str()), local));
    }
    // `render @items`: only the collection form is an iteration; a
    // single-record render (`render @user`) runs the partial once.
    let ty = first.ty.as_ref()?;
    ty.collection_elem()?;
    let (partial, local, _) = super::render::partial_from_receiver_type(ty)?;
    Some((first, Symbol::from(partial.as_str()), Symbol::from(local.as_str())))
}

/// One iteration site — a block iteration or a collection render:
/// the receiver must be a typed model relation with a recognized
/// chain; every single-hop association (or attachment) read on
/// `member` inside `body` that isn't preloaded is a finding. `how`
/// names the site in the message ("iterating" / "rendering").
fn check_members(
    recv: &Expr,
    body: &Expr,
    member: &Symbol,
    how: &str,
    index: &ModelIndex,
    env: &HashMap<Symbol, ChainInfo>,
    app: &App,
    out: &mut Vec<Diagnostic>,
    cov: &mut PreloadCoverage,
) {
    let Some(model) = relation_elem(recv.ty.as_ref()) else { return };
    let Some(keys) = index.read_keys(model) else { return };
    cov.iteration_sites += 1;
    let ChainInfo::Known { preloads, origin } = harvest_chain(recv, index, env, 0) else {
        cov.opaque_chains += 1;
        return; // opaque: no claim either way
    };
    cov.known_chains += 1;

    let mut reads: Vec<(Symbol, Span, Option<Symbol>)> = Vec::new();
    collect_member_assoc_reads(body, member, &keys, None, &mut reads);
    for r in &mut reads {
        if r.2.as_ref().is_some_and(|q| !index.is_sql_tail(model, &r.0, q)) {
            r.2 = None;
        }
    }
    // One finding per association per shape: a `.count` and an
    // `.each` on the same association are two different fixes.
    reads.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1.start.cmp(&b.1.start)));
    reads.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);
    for (read, span, sql_tail) in reads {
        let key = &keys[&read];
        if preloads.contains(key) && sql_tail.is_none() {
            continue;
        }
        let query_site =
            render_site(app, origin).map(|s| format!(" at {s}")).unwrap_or_default();
        let message = if let Some(q) = &sql_tail {
            // The read is a query in its own right; the preload is
            // beside the point and the honest fix is structural.
            let fix = match q.as_str() {
                "count" => format!(
                    "read `.size` over `.includes(:{})`, or add a `counter_cache`",
                    key.as_str()
                ),
                "exists?" => format!("read `.any?` over `.includes(:{})`", key.as_str()),
                _ => "move it into a scoped association (`has_many :…, -> { … }`) and \
                      preload that, or into the query itself"
                    .to_string(),
            };
            format!(
                "{how} this relation runs `{}.{}.{}` per row — a query that preloading \
                 :{}{query_site} would not avoid; {fix}",
                member.as_str(),
                read.as_str(),
                q.as_str(),
                key.as_str(),
            )
        } else {
            let fix = if key == &read {
                format!("`.includes(:{})`", key.as_str())
            } else {
                // An attachment: Rails' own scope, or its expansion.
                format!(
                    "`.with_attached_{}` (or `.includes({}: :blob)`)",
                    read.as_str(),
                    key.as_str()
                )
            };
            format!(
                "{how} this relation reads `{}.{}`, but the query{query_site} does not \
                 preload :{} — add {fix}",
                member.as_str(),
                read.as_str(),
                key.as_str(),
            )
        };
        out.push(Diagnostic {
            span,
            kind: DiagnosticKind::MissingPreload { association: key.clone(), query_span: origin },
            severity: Severity::Warning,
            message,
        });
    }
}

/// `member.<read>` sends inside the body (single hop, direct receiver
/// only), for every read in `keys`. Descends everything including
/// nested blocks — a read inside a nested `map` still runs per member.
///
/// `tail` is the method the enclosing send applies to this expression
/// (`member.assoc` as the receiver of `.count`), recorded with the
/// read so the caller can tell a query from a load — a bare `sum`
/// with a block is Ruby, `sum(:column)` is SQL, and only the caller
/// has the model to say which names are scopes.
fn collect_member_assoc_reads(
    expr: &Expr,
    member: &Symbol,
    keys: &HashMap<Symbol, Symbol>,
    tail: Option<&Symbol>,
    out: &mut Vec<(Symbol, Span, Option<Symbol>)>,
) {
    if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*expr.node {
        // A block parameter reads as a `Var`; a partial's local reads
        // as a bare no-arg `Send` (Prism sees an unbound bareword and
        // ingest lifts it that way — see `ingest::expr`'s `defined?`
        // note). Same member, two spellings.
        let is_member = match &*r.node {
            ExprNode::Var { name, .. } => name == member,
            ExprNode::Send { recv: None, method: m, args, block: None, .. } => {
                m == member && args.is_empty()
            }
            _ => false,
        };
        if is_member && keys.contains_key(method) {
            out.push((method.clone(), expr.span, tail.cloned()));
        }
        // `assoc.sum { … }` / `assoc.count { … }` enumerate the loaded
        // target; only the block-less form is a query.
        let applied = if block.is_none() { Some(method) } else { None };
        collect_member_assoc_reads(r, member, keys, applied, out);
        for a in args {
            collect_member_assoc_reads(a, member, keys, None, out);
        }
        if let Some(b) = block {
            collect_member_assoc_reads(b, member, keys, None, out);
        }
        return;
    }
    expr.node.for_each_child(&mut |c| collect_member_assoc_reads(c, member, keys, None, out));
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
                intersect_env(entry, &local);
            }
        }
    }
    // Partials: a partial's env is the intersection of its renderers'
    // envs, closed over partial-renders-partial edges (bounded — the
    // render graph is shallow and a cycle would only re-intersect).
    for _ in 0..4 {
        let mut next = envs.clone();
        let mut seen: HashMap<Symbol, u32> = HashMap::new();
        for (renderer, partials) in &app.render_edges {
            let Some(renv) = envs.get(renderer) else { continue };
            for partial in partials {
                let n = seen.entry(partial.clone()).or_insert(0);
                *n += 1;
                let entry = next.entry(partial.clone()).or_default();
                if *n == 1 {
                    *entry = renv.clone();
                } else {
                    intersect_env(entry, renv);
                }
            }
        }
        if next == envs {
            break;
        }
        envs = next;
    }
    envs
}

/// Keep the ivars both envs bind to a known chain, with the
/// intersection of their preloads (the missing-from-ALL rule).
fn intersect_env(entry: &mut HashMap<Symbol, ChainInfo>, other: &HashMap<Symbol, ChainInfo>) {
    entry.retain(|k, v| {
        let (Some(ChainInfo::Known { preloads: theirs, .. }), ChainInfo::Known { preloads, .. }) =
            (other.get(k), v)
        else {
            return false;
        };
        preloads.retain(|p| theirs.contains(p));
        true
    });
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
