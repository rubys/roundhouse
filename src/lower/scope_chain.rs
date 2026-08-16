//! Scope-call normalization — the lowering that lets ActiveRecord scope
//! chains run against the metaprogramming-free `ActiveRecord::Relation`
//! runtime (see project_lobsters_benchmark_parity_plan).
//!
//! A model `scope :name, ->(args){ body }` lowers (in `push_scope_methods`)
//! to a class method `def self.name(args, _rel = ActiveRecord::Relation.new(self))`.
//! For `Story.base(u).positive_ranked` to work without `method_missing`,
//! every scope INVOCATION is rewritten so the scope is an ordinary class
//! method taking the current relation as a trailing argument:
//!
//!   recv.scope(args)        (recv is a relation)  ->  Model.scope(args, recv)
//!   Model.scope(args)       (call on the class)   ->  Model.scope(args)   [default rel]
//!   <implicit>.scope(args)  (inside a scope body) ->  Model.scope(args, _rel)
//!
//! Relation built-ins (`where`/`order`/`limit`/…) stay `recv.method(args)`;
//! a `Model.where(...)` / `Model.all` chain-start (no scope) is seeded with
//! `ActiveRecord::Relation.new(Model)` so it, too, is chainable.

use std::collections::{HashMap, HashSet};

use crate::dialect::{Association, Model, ModelBodyItem, Param};
use crate::expr::{BlockStyle, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::pluralize_snake;

/// model class id -> (scope name -> the scope's user params, in order).
/// The params are the lambda's own parameters (NOT the synthesized trailing
/// `__rel`); the rewriter reads them to pad omitted leading args so a
/// threaded relation lands in the `__rel` slot (see `thread_rel`).
pub type ScopeRegistry = HashMap<ClassId, HashMap<Symbol, Vec<Param>>>;

/// Build `model -> {scope name -> params}` from the app's models.
///
/// Alongside declared scopes, a user-written CLASS method whose body
/// starts a bare relation chain (`def self.arrange_for_user(user)` with
/// bare `order(...)`) registers too: in Rails such a method called on a
/// relation runs with that relation as its implicit scope, so it takes
/// the same call-site threading a scope does. The Ruby emit seam appends
/// its `__rel` parameter and threads its body (`apply_scope_lowering`);
/// entry here is what makes call sites `recv.arrange_for_user(u)` become
/// `Comment.arrange_for_user(u, recv)`.
pub fn build_scope_registry(models: &[Model]) -> ScopeRegistry {
    let mut reg: ScopeRegistry = HashMap::new();
    for m in models {
        let map = reg.entry(m.name.clone()).or_default();
        for item in &m.body {
            match item {
                ModelBodyItem::Scope { scope, .. } => {
                    map.insert(scope.name.clone(), scope.params.clone());
                }
                ModelBodyItem::Method { method, .. }
                    if method.receiver == crate::dialect::MethodReceiver::Class
                        && mentions_bare_chain_start(&method.body) =>
                {
                    // Declared scopes win on a name collision.
                    map.entry(method.name.clone())
                        .or_insert_with(|| method.params.clone());
                }
                _ => {}
            }
        }
    }
    reg
}

/// The set of model class ids (so a `Const([M])` receiver can be recognized
/// as a class-level scope call vs. an arbitrary constant).
pub fn model_set(models: &[Model]) -> HashSet<ClassId> {
    models.iter().map(|m| m.name.clone()).collect()
}

/// Keyword-count cap for the fixed-arity relation entry points: each
/// keyword doubles the subset variants a scope generates, so a scope
/// declaring more keywords than this gets no mid-chain delegate.
pub const MAX_DELEGATE_KEYWORDS: usize = 2;

/// A registered scope's user params, split the way the fixed-arity
/// relation entry points (`push_scope_variants`) and the Relation
/// delegate emitter (`emit_relation_scope_delegates`) both need them —
/// one admissibility judgment so the two can't drift. `None` when the
/// shape can't take fixed-arity entries: a rest param (no fixed arity
/// covers it), more than [`MAX_DELEGATE_KEYWORDS`] keywords, or
/// optional positionals that don't form a contiguous tail (a
/// supplied-prefix arity can't express `def f(a = 1, b)` binding).
pub struct DelegableShape<'a> {
    /// Positional params in declaration order.
    pub positionals: Vec<&'a Param>,
    /// Count of leading required positionals — the minimum arity a
    /// call can supply; everything past it carries a default.
    pub min_required: usize,
    /// Keyword params in declaration order.
    pub keywords: Vec<&'a Param>,
}

impl<'a> DelegableShape<'a> {
    pub fn of(params: &'a [Param]) -> Option<DelegableShape<'a>> {
        if params.iter().any(|p| p.rest) {
            return None;
        }
        let positionals: Vec<&Param> = params.iter().filter(|p| !p.keyword).collect();
        let keywords: Vec<&Param> = params.iter().filter(|p| p.keyword).collect();
        if keywords.len() > MAX_DELEGATE_KEYWORDS {
            return None;
        }
        let min_required = positionals
            .iter()
            .position(|p| p.default.is_some())
            .unwrap_or(positionals.len());
        if positionals[min_required..].iter().any(|p| p.default.is_none()) {
            return None;
        }
        Some(DelegableShape { positionals, min_required, keywords })
    }

    /// Keywords a call site MUST supply (declared without a default).
    pub fn required_keywords(&self) -> Vec<&Symbol> {
        self.keywords
            .iter()
            .filter(|p| p.default.is_none())
            .map(|p| &p.name)
            .collect()
    }

    /// The keyword subsets a call site can supply — every subset of the
    /// declared keywords that includes all required ones, empty subset
    /// first, each sorted by name. These are exactly the `__kw_` entry
    /// variants a model generates.
    pub fn keyword_subsets(&self) -> Vec<Vec<&Param>> {
        let mut subsets: Vec<Vec<&Param>> = vec![vec![]];
        for kw in &self.keywords {
            for i in 0..subsets.len() {
                let mut with = subsets[i].clone();
                with.push(kw);
                subsets.push(with);
            }
        }
        let required = self.required_keywords();
        subsets.retain(|s| required.iter().all(|r| s.iter().any(|p| &p.name == *r)));
        for s in &mut subsets {
            s.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        }
        subsets.sort_by_key(|s| (s.len(), s.iter().map(|p| p.name.as_str().to_string()).collect::<Vec<_>>()));
        subsets
    }
}

/// Whether a registered name can carry the `__scope_<name>__<k>` entry
/// mangling: `?`/`!` are only legal at the END of a Ruby method name,
/// so a predicate-named scope gets no mid-chain delegate.
pub fn delegable_name(name: &Symbol) -> bool {
    name.as_str().chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The fixed-arity relation entry point name for `k` supplied
/// positionals and the given keyword subset: `__scope_<name>__<k>`,
/// plus `__kw_<names>` (sorted) when keywords are supplied.
pub fn scope_variant_name(name: &Symbol, k: usize, subset: &[&Param]) -> Symbol {
    let mut s = format!("__scope_{}__{k}", name.as_str());
    if !subset.is_empty() {
        s.push_str("__kw");
        for p in subset {
            s.push('_');
            s.push_str(p.name.as_str());
        }
    }
    Symbol::from(s)
}

/// method name -> the model its relation chain returns, for user-written
/// INSTANCE methods whose body tail is a query chain rooted at a model
/// constant (`Story#merged_comments` ends in `Comment.where(...)` →
/// `Comment`). Keyed by NAME because the receiver's class is usually
/// unknowable at the call site (`@story.merged_comments` — an untyped
/// ivar): a name that resolves to different targets on different models
/// maps to `None` and is never tracked. Consulted only when a registered
/// scope follows, so a collision with a non-model method of the same
/// name is inert unless that call ALSO chains into a known scope.
pub type UserMethodReturns = HashMap<Symbol, Option<ClassId>>;

pub fn build_user_method_returns(models: &[Model]) -> UserMethodReturns {
    let model_ids: HashSet<ClassId> = models.iter().map(|m| m.name.clone()).collect();
    let mut reg: UserMethodReturns = HashMap::new();
    for m in models {
        for item in &m.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            if method.receiver != crate::dialect::MethodReceiver::Instance {
                continue;
            }
            let Some(target) = relation_return_model(&method.body, &model_ids) else {
                continue;
            };
            reg.entry(method.name.clone())
                .and_modify(|t| {
                    if t.as_ref() != Some(&target) {
                        *t = None;
                    }
                })
                .or_insert(Some(target));
        }
    }
    reg
}

// ---- class methods reached THROUGH an association ------------------

/// Model class methods called through an association read —
/// `user.sessions.start!(…)`, `@room.messages.paged?`,
/// `messages.page_around(m)`.
///
/// Rails runs such a call with the association's relation as the
/// current scope. Two things follow from that, and a body can want
/// either or both:
///
///   * a CONSTRUCTOR in the body picks the foreign key up from the
///     scope (`scope_for_create`), so the row lands owned by the right
///     record without anybody naming the column;
///   * a QUERY in the body runs against the scope rather than the whole
///     table — `Message.paged?`'s bare `count` counts THIS ROOM's
///     messages.
///
/// Our association read is arel-folded to an Array, so neither call
/// even resolves — and threading the seeded Relation the way a scope
/// takes it is the same mechanism one layer over: the method gains a
/// trailing `__rel`, the call site passes the seed, and the body reads
/// it. The two halves read it differently — a constructor merges
/// `__rel.scope_attributes` under its own attributes (Rails lets
/// explicit attributes win), a query roots on `__rel` exactly as a
/// scope body does — which is why the registry records which applies.
///
/// Same shape as [`ScopeRegistry`] — model -> name -> the method's own
/// params — so `thread_rel` pads call sites identically.
pub type AssocClassMethods = HashMap<ClassId, HashMap<Symbol, AssocScopedMethod>>;

/// One registered method: its own params (NOT the synthesized trailing
/// `__rel`), plus which halves of the association scope its body reads.
pub struct AssocScopedMethod {
    pub params: Vec<Param>,
    /// The body constructs at implicit self — [`merge_scope_attributes`].
    pub creates: bool,
    /// The body queries at implicit self — [`rewrite_assoc_scope_body`].
    pub queries: bool,
}

/// Constructors a class-method body roots at implicit self. `build` is
/// absent deliberately: it is a CollectionProxy method, never a class
/// method, and the proxy form is seeded at the call site instead.
const SELF_CONSTRUCTORS: &[&str] = &["create", "create!", "new"];

/// What a class-method body does with the association's scope.
enum AssocScopeShape {
    /// Neither a constructor nor a query at implicit self — nothing an
    /// association scope would change; the method is not registered.
    None,
    /// The body reads the scope, in one or both of the two ways.
    Takes { creates: bool, queries: bool },
    /// A constructor whose argument is neither absent nor a hash —
    /// most often a bare parameter (`create!(attributes)`), where the
    /// value's own shape is unknown here. Registering the method would
    /// emit `attributes.merge(...)` against something that may not be a
    /// Hash at all, so the whole method declines and says why.
    Blocked(String),
}

/// Is this send a constructor rooted at the class the body belongs to?
/// Implicit self and `self.` are the spellings a hand-written body uses;
/// the explicit `<Model>.new` is what `params_merge` leaves behind when
/// it composes a params-object create out of `new` + the typed
/// `update!`, and it is a constructor on this class exactly as much.
fn self_rooted_ctor(recv: Option<&Expr>, method: &Symbol, owner: &ClassId) -> bool {
    if !SELF_CONSTRUCTORS.contains(&method.as_str()) {
        return false;
    }
    match recv {
        None => true,
        Some(r) => match &*r.node {
            ExprNode::SelfRef => true,
            ExprNode::Const { path } => path
                .last()
                .is_some_and(|last| owner.0.as_str().rsplit("::").next() == Some(last.as_str())),
            _ => false,
        },
    }
}

/// The AR class methods `ingest::app::qualify_model_class_method_ar_calls`
/// rewrites from the bare form to `<Model>.…`. Past that pass the two
/// spellings are the same expression, so reaching one of these THROUGH
/// the model's own constant has to be read as the implicit self it
/// stands for. Any other name on the constant is the author's own
/// writing and keeps its class-level, deliberately unscoped meaning.
const INGEST_QUALIFIED_AR_CALLS: &[&str] = &["all", "count", "where", "find_by", "exists?"];

/// Is this send a QUERY rooted at the class the body belongs to — the
/// implicit-self relation surface that Rails runs against the caller's
/// scope? Four kinds, all unambiguous ON A MODEL'S OWN CLASS METHOD,
/// because there `self` IS the class: a declared scope of this model,
/// `all`, a chain method, or a terminal.
///
/// Receivers: absent, `self.`, or the model's own constant limited to
/// [`INGEST_QUALIFIED_AR_CALLS`] — the same allowance
/// [`self_rooted_ctor`] makes, and for the same reason (a pass upstream
/// wrote the receiver, not the author).
fn self_rooted_query(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    block: Option<&Expr>,
    owner: &ClassId,
    scopes: &ScopeRegistry,
) -> bool {
    match recv {
        None => {}
        Some(r) => match &*r.node {
            ExprNode::SelfRef => {}
            ExprNode::Const { path }
                if INGEST_QUALIFIED_AR_CALLS.contains(&method.as_str())
                    && path.last().map(|l| l.as_str())
                        == owner.0.as_str().rsplit("::").next() => {}
            _ => return false,
        },
    }
    is_relation_chain_method(method.as_str())
        || method.as_str() == "all"
        || is_relation_terminal(method.as_str(), args, block)
        || scopes.get(owner).is_some_and(|s| s.contains_key(method))
}

/// Classify what a class method's body wants from the association's
/// scope: constructors to preset the foreign key on, queries to run
/// against, or both.
///
/// All-or-nothing on the CREATE half: one unmergeable constructor
/// blocks the whole method, because a half-scoped body writes a row
/// with the foreign key missing — silently the wrong owner rather than
/// a loud failure. The query half has no such trade (an unrecognized
/// send simply stays where it is), so it never blocks.
fn assoc_scope_shape(
    method_def: &crate::dialect::MethodDef,
    owner: &ClassId,
    scopes: &ScopeRegistry,
) -> AssocScopeShape {
    let mut found = false;
    let mut queries = false;
    let mut blocked: Option<String> = None;
    // Parameters DECLARED to be an attribute hash. `params_merge` stamps
    // them when every call site agreed the argument is one, which is
    // what makes `create!(attributes)` mergeable without guessing.
    let hash_params: HashSet<Symbol> = match &method_def.signature {
        Some(crate::ty::Ty::Fn { params, .. }) => params
            .iter()
            .filter(|p| matches!(p.ty, crate::ty::Ty::Hash { .. }))
            .map(|p| p.name.clone())
            .collect(),
        _ => HashSet::new(),
    };
    #[allow(clippy::too_many_arguments)]
    fn walk(
        e: &Expr,
        owner: &ClassId,
        scopes: &ScopeRegistry,
        hash_params: &HashSet<Symbol>,
        found: &mut bool,
        queries: &mut bool,
        blocked: &mut Option<String>,
    ) {
        if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
            if self_rooted_ctor(recv.as_ref(), method, owner) {
                *found = true;
                let mergeable = args.is_empty()
                    || (args.len() == 1
                        && match &*args[0].node {
                            ExprNode::Hash { .. } => true,
                            ExprNode::Var { name, .. } => hash_params.contains(name),
                            _ => false,
                        });
                if !mergeable && blocked.is_none() {
                    *blocked = Some(format!(
                        "`{}` takes an argument that is not an attribute hash",
                        method.as_str()
                    ));
                }
            } else if self_rooted_query(
                recv.as_ref(),
                method,
                args,
                block.as_ref(),
                owner,
                scopes,
            ) {
                *queries = true;
            }
        }
        e.node.for_each_child(&mut |c| {
            walk(c, owner, scopes, hash_params, found, queries, blocked)
        });
    }
    walk(
        &method_def.body,
        owner,
        scopes,
        &hash_params,
        &mut found,
        &mut queries,
        &mut blocked,
    );
    match (found, queries, blocked) {
        (false, false, _) => AssocScopeShape::None,
        (true, _, Some(why)) => AssocScopeShape::Blocked(why),
        (creates, queries, _) => AssocScopeShape::Takes { creates, queries },
    }
}

/// `(association name, method)` for every call in `expr` whose receiver
/// is an association-derived relation. The demand side of the registry:
/// a class method is only given the `__rel` parameter when some call
/// site actually reaches it through an association, so an app that never
/// writes that shape emits exactly what it emitted before.
///
/// Two receiver forms, because campfire writes both within four lines of
/// each other:
///
///   @room.messages.paged?                    the read, called on
///   messages = @room.messages.with_creator   … or parked in a local
///   messages.page_around(m)                  and called off that
///
/// The local form is tracked here rather than left to the rewriter's
/// `locals` map because demand is surveyed over the whole App BEFORE any
/// body is rewritten — that ordering is what makes the model pass and
/// the view/controller passes agree about which methods grew a
/// parameter.
pub fn collect_assoc_method_demand(
    expr: &Expr,
    assocs: &AssocRegistry,
    scope_names: &HashSet<Symbol>,
    out: &mut HashSet<(Symbol, Symbol)>,
) {
    let mut assoc_locals: HashMap<Symbol, Symbol> = HashMap::new();
    walk_demand(expr, assocs, scope_names, &mut assoc_locals, out);
}

/// The association a relation expression descends from, peeling
/// relation-preserving hops off the tail: `@room.messages.with_creator
/// .with_boosts` → `messages`. A hop is a Relation built-in or a name
/// some model declares as a scope — loose on purpose (this only decides
/// whether to ASK about a method, and the ask is resolved precisely in
/// [`build_assoc_class_methods`]).
fn assoc_read_name<'a>(
    expr: &'a Expr,
    assocs: &AssocRegistry,
    scope_names: &HashSet<Symbol>,
) -> Option<&'a Symbol> {
    let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { return None };
    // The read itself. A receiver-less spelling counts: `messages
    // .paged?` inside Room's own body is the same association.
    if args.is_empty() && block.is_none() && assocs.is_has_many_name(method) {
        return Some(method);
    }
    if is_relation_chain_method(method.as_str()) || scope_names.contains(method) {
        return assoc_read_name(recv.as_ref()?, assocs, scope_names);
    }
    None
}

fn walk_demand(
    expr: &Expr,
    assocs: &AssocRegistry,
    scope_names: &HashSet<Symbol>,
    assoc_locals: &mut HashMap<Symbol, Symbol>,
    out: &mut HashSet<(Symbol, Symbol)>,
) {
    match &*expr.node {
        ExprNode::Assign { target, value } => {
            if let crate::expr::LValue::Var { name, .. } = target {
                match assoc_read_name(value, assocs, scope_names) {
                    Some(aname) => {
                        assoc_locals.insert(name.clone(), aname.clone());
                    }
                    // Reassigned to something else: the name stops
                    // standing for the association.
                    None => {
                        assoc_locals.remove(name);
                    }
                }
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            let aname = match &*r.node {
                ExprNode::Var { name, .. } => assoc_locals.get(name),
                _ => assoc_read_name(r, assocs, scope_names),
            };
            if let Some(aname) = aname {
                out.insert((aname.clone(), method.clone()));
            }
        }
        _ => {}
    }
    // Children in source order — a `Seq`'s statements included, which is
    // what lets an assignment above be visible to the lines below it.
    expr.node
        .for_each_child(&mut |c| walk_demand(c, assocs, scope_names, assoc_locals, out));
}

/// Survey the whole App for class methods reached through an
/// association, and resolve them against the models.
///
/// THE one entry point, because three passes need this answer and they
/// must not disagree: the Ruby model lowering (which holds back the
/// arel fold on the query-shaped ones), the emit seam that inserts the
/// parameter, and the emit seam that rewrites the call sites. The first
/// two run over different slices, so surveying a slice rather than the
/// App would thread a relation into a method that never grew the
/// parameter.
///
/// Views are surveyed alongside the hook bodies and are NOT in that
/// walk — a view is lowered later, once, by its own pass — but campfire
/// reaches `Message.paged?` only from a partial.
pub fn survey_assoc_class_methods(
    app: &crate::app::App,
    assocs: &AssocRegistry,
    scopes: &ScopeRegistry,
) -> (AssocClassMethods, Vec<DeclinedAssocScope>) {
    let scope_names = all_scope_names(scopes);
    let mut demand: HashSet<(Symbol, Symbol)> = HashSet::new();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        collect_assoc_method_demand(body, assocs, &scope_names, &mut demand);
    });
    for view in &app.views {
        collect_assoc_method_demand(&view.body, assocs, &scope_names, &mut demand);
    }
    build_assoc_class_methods(&app.models, assocs, scopes, &demand)
}

/// `(model, method)` for the QUERY-shaped half of that survey — the
/// methods whose body must run against the caller's scope.
///
/// `lower::model_to_library` reads it to hold the arel fold back on
/// exactly those bodies. `qualify_model_class_method_ar_calls` has
/// already spelled their implicit-self query as `<Model>.count`, and
/// folding that to an inline whole-table `SELECT COUNT(*)` would bake
/// in the one reading the method must not have. Ruby-family only: a
/// strict target has no relation to thread, so it keeps the fold and
/// the class-level reading that goes with it.
pub fn assoc_query_method_names(acm: &AssocClassMethods) -> HashSet<(ClassId, Symbol)> {
    acm.iter()
        .flat_map(|(model, per_model)| {
            per_model
                .iter()
                .filter(|(_, e)| e.queries)
                .map(move |(name, _)| (model.clone(), name.clone()))
        })
        .collect()
}

/// One class method that could not take an association scope, with the
/// reason — reported as a modeling-debt line rather than dropped.
pub struct DeclinedAssocScope {
    pub model: ClassId,
    pub method: Symbol,
    pub reason: String,
}

/// Resolve the demand set against the models: which class methods must
/// take the association's relation, and which ones wanted to and could
/// not.
///
/// A name already registered as a scope is skipped — that path threads
/// the relation as a FILTER and is what the seed arm already does.
pub fn build_assoc_class_methods(
    models: &[Model],
    assocs: &AssocRegistry,
    scopes: &ScopeRegistry,
    demand: &HashSet<(Symbol, Symbol)>,
) -> (AssocClassMethods, Vec<DeclinedAssocScope>) {
    let mut reg: AssocClassMethods = HashMap::new();
    let mut declined: Vec<DeclinedAssocScope> = Vec::new();
    let mut seen: HashSet<(ClassId, Symbol)> = HashSet::new();
    // Sorted so the declined ledger (and any emit that keys off the
    // registry) is byte-stable across runs.
    let mut wanted: Vec<&(Symbol, Symbol)> = demand.iter().collect();
    wanted.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
    for (aname, mname) in wanted {
        for owner in models {
            let Some((target, _)) = assocs.has_many_fk(&owner.name, aname) else { continue };
            // An association the seed cannot reproduce (`as:`, a
            // row-changing scope) never reaches the call-site rewrite,
            // so the method must not grow a parameter nobody passes.
            if assocs.is_unseedable(Some(&owner.name), aname) {
                continue;
            }
            if scopes.get(target).is_some_and(|s| s.contains_key(mname)) {
                continue;
            }
            if !seen.insert((target.clone(), mname.clone())) {
                continue;
            }
            let Some(target_model) = models.iter().find(|m| m.name == *target) else { continue };
            let Some(method) = target_model.body.iter().find_map(|item| match item {
                ModelBodyItem::Method { method, .. }
                    if method.receiver == crate::dialect::MethodReceiver::Class
                        && method.name == *mname =>
                {
                    Some(method)
                }
                _ => None,
            }) else {
                continue;
            };
            match assoc_scope_shape(method, target, scopes) {
                AssocScopeShape::None => {}
                AssocScopeShape::Takes { creates, queries } => {
                    reg.entry(target.clone()).or_default().insert(
                        mname.clone(),
                        AssocScopedMethod { params: method.params.clone(), creates, queries },
                    );
                }
                AssocScopeShape::Blocked(reason) => declined.push(DeclinedAssocScope {
                    model: target.clone(),
                    method: mname.clone(),
                    reason,
                }),
            }
        }
    }
    (reg, declined)
}

/// Merge the threaded relation's scope attributes under every
/// implicit-self constructor in a class-method body:
///
///   create!(user_agent: ua)  ->  create!(__rel.scope_attributes.merge(user_agent: ua))
///   new                      ->  new(__rel.scope_attributes)
///
/// The caller's own attributes stay on the OUTSIDE of the merge because
/// Rails assigns them after the scope's, so an explicit value wins over
/// the association's. Only shapes [`ctor_shape`] admitted reach here.
pub fn merge_scope_attributes(body: &mut Expr, owner: &ClassId, rel: &Symbol) {
    fn walk(e: &mut Expr, owner: &ClassId, rel: &Symbol) {
        let span = e.span;
        if let ExprNode::Send { recv, method, args, parenthesized, .. } = &mut *e.node {
            if self_rooted_ctor(recv.as_ref(), method, owner) {
                let scope_attrs = syn(
                    span,
                    ExprNode::Send {
                        recv: Some(var_expr(span, rel)),
                        method: Symbol::from("scope_attributes"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                let arg_is_attrs = args.len() == 1
                    && matches!(
                        &*args[0].node,
                        ExprNode::Hash { .. } | ExprNode::Var { .. }
                    );
                if args.is_empty() {
                    *args = vec![scope_attrs];
                    *parenthesized = true;
                } else if arg_is_attrs {
                    let own = args[0].clone();
                    *args = vec![syn(
                        span,
                        ExprNode::Send {
                            recv: Some(scope_attrs),
                            method: Symbol::from("merge"),
                            args: vec![own],
                            block: None,
                            parenthesized: true,
                        },
                    )];
                }
            }
        }
        e.node.for_each_child_mut(&mut |c| walk(c, owner, rel));
    }
    walk(body, owner, rel);
}

/// The model whose relation `expr` evaluates to, when that is statically
/// evident: the tail expression is a chain of relation-preserving hops
/// (`where`/`order`/`includes`/… or further sends we can't classify are
/// REJECTED) rooted at `Const(Model)` or `ActiveRecord::Relation.new(
/// Model)`. Conservative: anything else — branches, terminals like
/// `count`/`pluck`, unknown hops — returns None.
fn relation_return_model(expr: &Expr, models: &HashSet<ClassId>) -> Option<ClassId> {
    let tail = match &*expr.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => expr,
    };
    let mut e = tail;
    loop {
        match &*e.node {
            ExprNode::Send { recv: Some(r), method, .. } => {
                // `Relation.new(Model)` root.
                if method.as_str() == "new" {
                    if let ExprNode::Const { path } = &*r.node {
                        if path.len() == 2
                            && path[0].as_str() == "ActiveRecord"
                            && path[1].as_str() == "Relation"
                        {
                            if let ExprNode::Send { args, .. } = &*e.node {
                                if let Some(a) = args.first() {
                                    return const_model(a, models);
                                }
                            }
                        }
                    }
                    return None;
                }
                // A hop must preserve the relation. `where`-family and
                // the chain methods qualify; on the ROOT constant, any
                // method could be a scope — accept it there (checked
                // when we reach the Const).
                if let Some(m) = const_model(r, models) {
                    return Some(m);
                }
                if !is_relation_chain_method(method.as_str()) {
                    return None;
                }
                e = r;
            }
            _ => return None,
        }
    }
}

/// Per-model association facts the chain rewriter consumes once a chain's
/// model is known: `joins(:assoc)` expands to its JOIN SQL, and a
/// `belongs_to`-named hash key in `where`/`not` rewrites to the foreign-key
/// column (the runtime Relation sees only columns and SQL — the compiler is
/// where association knowledge lives).
#[derive(Default)]
pub struct AssocRegistry {
    /// (model, association name) -> `"<target_table> ON <cond>"`; the
    /// rewrite prefixes `INNER JOIN` / `LEFT OUTER JOIN` by call. Direct
    /// `belongs_to`/`has_many`/`has_one`, plus resolvable `has_many
    /// :through` — a through tail carries its own second `INNER JOIN`, so
    /// a `left_outer_joins(:through_assoc)` would outer-join only the
    /// first hop (no such call exists in the exercised apps; revisit if
    /// one appears). Habtm and unresolvable through shapes stay absent,
    /// so their `joins(:sym)` is left untouched (visible at runtime
    /// rather than silently mis-joined).
    join_tails: HashMap<(ClassId, Symbol), String>,
    /// (model, belongs_to name) -> foreign-key column, for
    /// `where(user: user)` -> `where(user_id: user && user.id)`.
    belongs_to_fk: HashMap<(ClassId, Symbol), Symbol>,
    /// (model, association name) -> the association target's TABLE, for
    /// the nested-hash `where` form: `where(story: {merged_story_id:
    /// id})` names conditions on the JOINED table, not on this model's
    /// foreign key. Every association shape that has a join tail has
    /// one, `:through` included (its conditions land on the far table).
    assoc_table: HashMap<(ClassId, Symbol), Symbol>,
    /// (model, association name) -> the join tail with the target table
    /// ALIASED to the association name (`stories story ON story.id =
    /// comments.story_id`). Rails switches a join to this form as soon
    /// as a `where` hash keys off the association name rather than the
    /// table name, and app SQL fragments are written against that alias
    /// — lobsters' `merged_comments` ORs in `'"story"."id" = ?'`. Only
    /// present where the names actually differ (a `has_many :comments`
    /// on the `comments` table needs no alias) and only for the
    /// single-hop shapes; a `:through` chain keeps its unaliased tail
    /// and falls back to table-name qualification.
    aliased_join_tails: HashMap<(ClassId, Symbol), String>,
    /// (model, has_many name) -> (target model, foreign-key column), for
    /// scope-on-self-association chains (`self.comments.accessible_to_user`
    /// seeds `Relation.new(Comment).where(user_id: @id)`). Direct
    /// has_many only — a `:through` receiver would need the join seed,
    /// left untouched until an app exercises it.
    has_many_fk: HashMap<(ClassId, Symbol), (ClassId, Symbol)>,
    /// has_many name -> (target model, foreign-key column) when the name
    /// is UNIQUE across all models (`merged_stories` → Story), `None`
    /// when ambiguous (`comments` lives on both Story and User). Lets
    /// `@story.merged_stories.<scope|chain|terminal>` seed a Relation
    /// even though the receiver's class is statically unknown; ambiguous
    /// names are never tracked.
    has_many_by_name: HashMap<Symbol, Option<(ClassId, Symbol)>>,
    /// `(model, has_many name)` pairs the FK seed CANNOT reproduce, so
    /// the rewriter declines them and the chain keeps its source shape.
    ///
    /// The seed is exactly `Relation.new(Target).where(fk => owner.id)`.
    /// Two declarations make that an under-constrained query rather than
    /// the association Rails would hand back, and both fail SILENTLY —
    /// extra rows, not an exception:
    ///
    ///   * `as: :notifiable` — the rows are keyed by `<as>_id` AND
    ///     `<as>_type`; seeding only the id half reaches every other
    ///     implementor's rows too.
    ///   * a scope lambda that can change the ROW SET
    ///     (`-> { where(...) }`, `-> { joins(...) }`) —
    ///     `synth_has_many_reader` grafts it onto the reader; nothing
    ///     grafts it here.
    ///
    /// A scope built only from preload directives is NOT listed:
    /// `has_many :stories, -> { includes :user }` selects exactly the
    /// same rows either way, and `includes` is an eager-load hint whose
    /// loss costs a query, not an answer — the reader's own eager-load
    /// path already doesn't apply scopes (see `synth_has_many_reader`).
    /// Declining it would have turned lobsters'
    /// `author.stories.not_deleted(nil)` from slow into broken.
    ///
    /// Declining leaves the pre-existing NoMethodError-on-Array, which is
    /// loud and locatable. Wrong rows are neither.
    has_many_unseedable: std::collections::HashSet<(ClassId, Symbol)>,
}

impl AssocRegistry {
    fn join_tail(&self, model: &ClassId, assoc: &Symbol) -> Option<&String> {
        self.join_tails.get(&(model.clone(), assoc.clone()))
    }
    fn belongs_to_fk(&self, model: &ClassId, assoc: &Symbol) -> Option<&Symbol> {
        self.belongs_to_fk.get(&(model.clone(), assoc.clone()))
    }
    fn assoc_table(&self, model: &ClassId, assoc: &Symbol) -> Option<&Symbol> {
        self.assoc_table.get(&(model.clone(), assoc.clone()))
    }
    fn aliased_join_tail(&self, model: &ClassId, assoc: &Symbol) -> Option<&String> {
        self.aliased_join_tails.get(&(model.clone(), assoc.clone()))
    }
    fn has_many_fk(&self, model: &ClassId, assoc: &Symbol) -> Option<&(ClassId, Symbol)> {
        self.has_many_fk.get(&(model.clone(), assoc.clone()))
    }
    /// Is `name` a has_many association on ANY model? Presence check
    /// only — used by the `mentions_assoc_constructor` gate, where an
    /// ambiguous name still qualifies (the rewriter may resolve it
    /// from the owner's type).
    fn is_has_many_name(&self, name: &Symbol) -> bool {
        self.has_many_fk.keys().any(|(_, a)| a == name)
    }
    fn has_many_by_name(&self, assoc: &Symbol) -> Option<&(ClassId, Symbol)> {
        self.has_many_by_name.get(assoc).and_then(|o| o.as_ref())
    }
    /// See `has_many_unseedable`. Checked against every model declaring
    /// the name, not just the resolved owner: the by-NAME rung answers
    /// without an owner, so a name that is unseedable ANYWHERE has to be
    /// declined there too.
    fn is_unseedable(&self, owner: Option<&ClassId>, assoc: &Symbol) -> bool {
        match owner {
            Some(m) => self.has_many_unseedable.contains(&(m.clone(), assoc.clone())),
            None => self.has_many_unseedable.iter().any(|(_, a)| a == assoc),
        }
    }
}

/// Does this association-scope lambda select the same rows the bare
/// foreign-key query would?
///
/// True only for a chain built entirely from eager-load directives —
/// those name what to LOAD ALONGSIDE, never which rows to return, so a
/// seed that drops them answers identically and just costs the extra
/// queries. Anything else (`where`, `joins`, `merge`, `limit`, and also
/// `order`, which decides what `.first`/`.last` mean) is treated as
/// row-changing: the list is an allowlist so an unrecognized method
/// declines rather than being assumed harmless.
fn scope_is_row_preserving(scope: &Expr) -> bool {
    fn walk(e: &Expr) -> bool {
        match &*e.node {
            ExprNode::Send { recv, method, .. } => {
                matches!(method.as_str(), "includes" | "preload" | "eager_load")
                    && recv.as_ref().is_none_or(|r| walk(r))
            }
            // The chain root a receiver-less lambda body lowers to.
            ExprNode::SelfRef => true,
            _ => false,
        }
    }
    walk(scope)
}

/// Build the association registry. Table names use the same
/// `pluralize_snake` the synthesized `table_name` methods use, so the
/// generated SQL and the runtime agree by construction.
pub fn build_assoc_registry(models: &[Model]) -> AssocRegistry {
    let mut reg = AssocRegistry::default();
    for m in models {
        let own = pluralize_snake(m.name.0.as_str());
        for a in m.associations() {
            match a {
                Association::BelongsTo { name, target, foreign_key, .. } => {
                    let t = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.id = {own}.{foreign_key}"),
                    );
                    reg.belongs_to_fk
                        .insert((m.name.clone(), name.clone()), foreign_key.clone());
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    if name.as_str() != t {
                        reg.aliased_join_tails.insert(
                            (m.name.clone(), name.clone()),
                            format!("{t} {name} ON {name}.id = {own}.{foreign_key}"),
                        );
                    }
                }
                Association::HasMany {
                    name,
                    target,
                    foreign_key,
                    through: None,
                    as_interface,
                    scope,
                    ..
                } => {
                    if as_interface.is_some()
                        || scope.as_ref().is_some_and(|s| !scope_is_row_preserving(s))
                    {
                        reg.has_many_unseedable.insert((m.name.clone(), name.clone()));
                    }
                    let t = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.id"),
                    );
                    reg.has_many_fk.insert(
                        (m.name.clone(), name.clone()),
                        (target.clone(), foreign_key.clone()),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    if name.as_str() != t {
                        reg.aliased_join_tails.insert(
                            (m.name.clone(), name.clone()),
                            format!("{t} {name} ON {name}.{foreign_key} = {own}.id"),
                        );
                    }
                    let entry = (target.clone(), foreign_key.clone());
                    reg.has_many_by_name
                        .entry(name.clone())
                        .and_modify(|t| {
                            if t.as_ref() != Some(&entry) {
                                *t = None;
                            }
                        })
                        .or_insert(Some(entry));
                }
                Association::HasOne { name, target, foreign_key, .. } => {
                    let t = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.id"),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    if name.as_str() != t {
                        reg.aliased_join_tails.insert(
                            (m.name.clone(), name.clone()),
                            format!("{t} {name} ON {name}.{foreign_key} = {own}.id"),
                        );
                    }
                }
                // `has_many :through`: two hops, owner-side direction
                // (`Tag.joins(:stories)` → JOIN taggings ON tag_id, JOIN
                // stories ON story_id). Same fk resolution as the
                // through-reader lowering: the through association names
                // the join model; its `belongs_to` matching the assoc's
                // target class supplies the source fk (survives `source:`
                // renames, which ingest folds into `target`).
                Association::HasMany { name, target, through: Some(thr_name), .. } => {
                    let Some(Association::HasMany { target: thr_target, foreign_key: thr_fk, .. }) =
                        m.associations().find(|a| {
                            matches!(a, Association::HasMany { name, .. } if name == thr_name)
                        })
                    else {
                        continue;
                    };
                    let Some(thr_model) = models.iter().find(|tm| &tm.name == thr_target) else {
                        continue;
                    };
                    let Some(Association::BelongsTo { foreign_key: src_fk, .. }) =
                        thr_model.associations().find(|a| {
                            matches!(a, Association::BelongsTo { target: t, .. } if t == target)
                        })
                    else {
                        continue;
                    };
                    let thr_table = pluralize_snake(thr_target.0.as_str());
                    let target_table = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!(
                            "{thr_table} ON {thr_table}.{thr_fk} = {own}.id \
                             INNER JOIN {target_table} ON {target_table}.id = {thr_table}.{src_fk}"
                        ),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(target_table.as_str()));
                }
                _ => {}
            }
        }
    }
    reg
}

/// True when any model declares a scope (the whole pass is a no-op
/// otherwise — e.g. the scope-free blog).
pub fn any_scopes(scopes: &ScopeRegistry) -> bool {
    scopes.values().any(|s| !s.is_empty())
}

/// Union of every scope name across all models — a cheap pre-filter so a
/// body that names no scope is left completely untouched.
pub fn all_scope_names(scopes: &ScopeRegistry) -> HashSet<Symbol> {
    scopes.values().flat_map(|m| m.keys().cloned()).collect()
}

/// True if `expr` (or a descendant) calls a method whose name is a scope.
pub fn mentions_scope(expr: &Expr, names: &HashSet<Symbol>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, names: &HashSet<Symbol>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { method, .. } = &*e.node {
            if names.contains(method) {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, names, found));
    }
    walk(expr, names, &mut found);
    found
}

/// True if `expr` (or a descendant) starts a query chain directly on a
/// known model constant (`Vote.where(...)`, `Story.all`). Scope-free
/// bodies can still hold such chains — the arel inline pass refuses a
/// where-hash whose value isn't statically scalar (an Array means `IN`,
/// nil means `IS NULL`, only runtime knows), so those chains reach emit
/// as plain sends and must be seeded with a Relation to run.
/// True when `expr` contains a CollectionProxy constructor on an
/// association read — `<owner>.<has_many>.build/create(…)`. Gate for
/// `rewrite_call_site` so a body whose only relation surface is the
/// constructor (`comment = story.comments.build`) still enters the
/// rewrite; the resolution itself (owner typing / by-name uniqueness)
/// happens inside the rewriter. Name presence alone qualifies here —
/// an ambiguous name may still resolve there via the owner's type.
pub fn mentions_assoc_constructor(expr: &Expr, assocs: &AssocRegistry) -> bool {
    let mut found = false;
    fn walk(e: &Expr, assocs: &AssocRegistry, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
            // NOTE (measured, deliberately NOT widened): the seed arm
            // also handles chain methods and terminals on a has_many
            // read, so a body whose only relation surface is
            // `@room.messages.first(3)` — naming no scope, tripping no
            // other gate — never reaches the rewriter and calls a
            // relation method on the reader's folded Array.
            //
            // Widening this to match costs more than it fixes TODAY:
            // it moves 9 lobsters files, and the ones that move are
            // `domain.origins.count` in a per-row VIEW, where the folded
            // Array's `count` is correct AND rides the reader's preload
            // cache. Turning those into a fresh `SELECT COUNT(*)` is an
            // N+1 on the benchmark app. Doing it right means teaching the
            // seed to reuse a loaded cache, which is its own commit.
            if matches!(method.as_str(), "build" | "create" | "create!") {
                if let ExprNode::Send { method: aname, args, .. } = &*r.node {
                    if args.is_empty() && assocs.is_has_many_name(aname) {
                        *found = true;
                        return;
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, assocs, found));
    }
    walk(expr, assocs, &mut found);
    found
}

/// True when `expr` LOOKS A ROW UP through an association read —
/// `Current.user.memberships.find_by!(room_id: …)`, `@room.messages
/// .find(id)`. A second gate for `rewrite_call_site` beside
/// `mentions_assoc_constructor`, and the reason it is a separate one is
/// the measurement recorded there: widening that gate to everything the
/// seed arm handles turns `domain.origins.count` in a per-row view from
/// a correct cached read into an N+1. The lookup family has no such
/// trade — `Array` answers none of these methods, so the site is a
/// NoMethodError today and a query after.
pub fn mentions_assoc_lookup(expr: &Expr, assocs: &AssocRegistry) -> bool {
    let mut found = false;
    fn walk(e: &Expr, assocs: &AssocRegistry, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*e.node {
            if is_relation_terminal(method.as_str(), args, block.as_ref())
                && matches!(method.as_str(), "find" | "find_by" | "find_by!")
            {
                if let ExprNode::Send { method: aname, args: aargs, block: None, .. } = &*r.node {
                    if aargs.is_empty() && assocs.is_has_many_name(aname) {
                        *found = true;
                        return;
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, assocs, found));
    }
    walk(expr, assocs, &mut found);
    found
}

/// True when `expr` calls a REGISTERED association-scoped class method
/// through an association read (`user.sessions.start!`). Gate for
/// `rewrite_call_site`, alongside `mentions_assoc_constructor`: such a
/// body may name no scope and start no model chain, and without a gate
/// it never reaches the rewriter at all.
pub fn mentions_assoc_class_method(
    expr: &Expr,
    assocs: &AssocRegistry,
    scopes: &ScopeRegistry,
    acm: &AssocClassMethods,
) -> bool {
    if acm.is_empty() {
        return false;
    }
    let names = all_scope_names(scopes);
    let mut demand: HashSet<(Symbol, Symbol)> = HashSet::new();
    collect_assoc_method_demand(expr, assocs, &names, &mut demand);
    demand
        .iter()
        .any(|(_, m)| acm.values().any(|per_model| per_model.contains_key(m)))
}

pub fn mentions_model_chain_start(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
            if (is_relation_chain_method(method.as_str()) || method.as_str() == "all")
                && const_model(r, models).is_some()
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, models, found));
    }
    walk(expr, models, &mut found);
    found
}

/// True if `expr` (or a descendant) calls a relation chain method with no
/// receiver (or on explicit `self` — same thing spelled out) — the
/// implicit-self query root a model's own class method uses
/// (`self.where(key: key)` in `Keystore.value_for`). Only meaningful for
/// bodies rewritten with `class_self` set.
pub fn mentions_bare_chain_start(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv, method, .. } = &*e.node {
            let self_rooted = match recv {
                None => true,
                Some(r) => matches!(&*r.node, ExprNode::SelfRef),
            };
            if self_rooted
                && (is_relation_chain_method(method.as_str()) || method.as_str() == "all")
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
    found
}

/// Relation methods that return a Relation — calls on them stay on the
/// receiver and the chain keeps its model. Terminals / Enumerable methods
/// (`to_a`/`first`/`map`/`pluck`/…) are deliberately absent: they end the
/// relation, so model-tracking stops after them (a scope can't follow).
fn is_relation_chain_method(name: &str) -> bool {
    matches!(
        name,
        "where"
            | "not"
            | "order"
            | "limit"
            | "offset"
            | "group"
            | "having"
            | "joins"
            | "left_outer_joins"
            | "select"
            | "distinct"
            | "includes"
            | "preload"
            | "eager_load"
            | "references"
            | "merge"
            | "none"
            // `excluding` (Rails 7 `where.not(id: …)` in one hop).
            // Listed here so an association read that continues into it
            // is ROOTED as a relation: campfire's
            // `user.searches.excluding(…).destroy_all` otherwise left
            // the has_many reader answering an Array, and every hop
            // after it wanted a different method — `excluding`, then
            // `destroy_all`, then the next. Chasing those onto Array is
            // how ActiveRecord semantics leak into core; rooting the
            // chain once puts all of them on Relation, which already
            // defines them.
            | "excluding"
    )
}

/// Shared lookup tables; `scope_body` is `Some((self_model, rel_param))`
/// when rewriting a scope's own body (so implicit-self query roots thread
/// the relation parameter), `None` at every other call site.
pub struct Ctx<'a> {
    pub scopes: &'a ScopeRegistry,
    pub models: &'a HashSet<ClassId>,
    pub assocs: &'a AssocRegistry,
    /// Class methods that take an association's relation as their
    /// implicit scope (see [`AssocClassMethods`]). Empty for scope
    /// bodies, which are rewritten before the registry is resolved.
    pub assoc_class_methods: &'a AssocClassMethods,
    pub scope_body: Option<(ClassId, Symbol)>,
    /// Read `<Model>.count` in this body as the implicit self it stands
    /// for (see [`INGEST_QUALIFIED_AR_CALLS`]).
    ///
    /// True ONLY for the association-scoped class methods, where the
    /// receiver is ingest's writing rather than the author's. A DECLARED
    /// scope's body keeps the constant's class-level reading: lobsters'
    /// `StoryText.cached?` spells `StoryText.find_by(id: story)` by
    /// hand, and re-rooting that would scope a query the author wrote
    /// unscoped.
    pub self_const_is_implicit: bool,
    /// `Some(model)` when rewriting a model's own CLASS method (a
    /// user-written `def self.x`): a bare `where(...)`/`all` there is an
    /// implicit-self query root (`Keystore.value_for`'s `where(key:
    /// key).limit(1)`), so it seeds `Relation.new(Model)` — like a scope
    /// body, but with no `__rel` parameter to thread.
    pub class_self: Option<ClassId>,
    /// Name-keyed relation return types of user instance methods (see
    /// `build_user_method_returns`).
    pub user_returns: &'a UserMethodReturns,
    /// `Some(model)` when rewriting a model's own INSTANCE method:
    /// `self.<has_many>` there is an association read whose target model
    /// is known, so a scope following it can seed a Relation from the
    /// association's foreign key (`recent_threads`' `self.comments.
    /// accessible_to_user(u)`). Unlike `class_self`, bare sends are NOT
    /// query roots (no implicit scoping on an instance).
    pub instance_self: Option<ClassId>,
}

impl Ctx<'_> {
    fn scope_of(&self, model: &ClassId, method: &Symbol) -> bool {
        self.scopes.get(model).is_some_and(|s| s.contains_key(method))
    }
    /// The scope's own (user) params, so the rewriter can pad omitted
    /// leading args when threading the relation.
    fn scope_params(&self, model: &ClassId, method: &Symbol) -> Option<&Vec<Param>> {
        self.scopes.get(model).and_then(|s| s.get(method))
    }
    /// The method's own params when it is a class method taking an
    /// association scope — `Some` is also the "is registered" answer.
    fn assoc_class_method_params(&self, model: &ClassId, method: &Symbol) -> Option<&Vec<Param>> {
        self.assoc_class_methods
            .get(model)
            .and_then(|s| s.get(method))
            .map(|m| &m.params)
    }
    /// A copy of self for args / blocks / non-receiver subtrees.
    ///
    /// The scope-body relation SURVIVES: an argument is evaluated with
    /// the same `self` the receiver was, so a bare send there means the
    /// current scope exactly as much — `page_before(m) + [m] +
    /// page_after(m)` puts one of the two scope calls in receiver
    /// position and the other in an argument, and Rails scopes both.
    /// Dropping it here left the argument silently running against the
    /// whole table. A constant-rooted argument (`where(id: Story.where(
    /// …))`) is unaffected either way — the Const arm never consults
    /// the scope body.
    fn at_callsite(&self) -> Ctx<'_> {
        Ctx {
            scopes: self.scopes,
            models: self.models,
            assocs: self.assocs,
            assoc_class_methods: self.assoc_class_methods,
            scope_body: self.scope_body.clone(),
            self_const_is_implicit: self.self_const_is_implicit,
            class_self: self.class_self.clone(),
            instance_self: self.instance_self.clone(),
            user_returns: self.user_returns,
        }
    }
}

fn syn(span: crate::span::Span, node: ExprNode) -> Expr {
    Expr::new(span, node)
}

/// Append the threaded relation to a scope call's args so it lands in the
/// synthesized trailing `__rel` slot. A scope with leading OPTIONAL params
/// (`hottest(user = nil, exclude_tags = nil)`) called with fewer args than
/// it declares — e.g. bare `hottest` in `front_page` — would otherwise bind
/// the relation to the FIRST param. Pad the skipped leading params with
/// their own defaults (Ruby's behavior for the omitted call) before pushing
/// the relation, so `hottest` → `Story.hottest(nil, nil, __rel)` not
/// `Story.hottest(__rel)`. `leading` is the scope's user params (no `__rel`).
fn thread_rel(mut args: Vec<Expr>, rel: Expr, leading: Option<&Vec<Param>>, span: crate::span::Span) -> Vec<Expr> {
    // A trailing kwargs hash (`base(user, unmerged: unmerged)`) binds
    // the scope's KEYWORD params; the relation is a positional and must
    // land before it. Split it off, pad, thread, re-append.
    let kwargs_tail = match args.last() {
        Some(e) if matches!(&*e.node, ExprNode::Hash { kwargs: true, .. }) => args.pop(),
        _ => None,
    };
    if let Some(params) = leading {
        // Only positional params pad — keywords are bound by name via
        // the kwargs tail (or their own defaults).
        for p in params.iter().filter(|p| !p.keyword).skip(args.len()) {
            let filler = p
                .default
                .clone()
                .unwrap_or_else(|| syn(span, ExprNode::Lit { value: Literal::Nil }));
            args.push(filler);
        }
    }
    args.push(rel);
    args.extend(kwargs_tail);
    args
}

/// Relation TERMINALS our runtime implements — a seeded association
/// chain may end in one (`@story.merged_stories.ids`). Deliberately
/// excludes `each`/`map`/iteration: plain reader traversal stays on the
/// Array (and the preload cache).
///
/// The lookup family is here because Rails' `<owner>.<has_many>.find*`
/// is a QUERY, not a scan — `Current.user.memberships.find_by!(room_id:)`
/// is the shape every room-scoped controller opens with. `find_by` /
/// `find_by!` are unambiguous (Array answers neither), but `find` is
/// also `Enumerable#detect`: the block form and any arity but one are
/// left on the Array, where they already mean what Ruby means.
fn is_relation_terminal(name: &str, args: &[Expr], block: Option<&Expr>) -> bool {
    match name {
        "find" => args.len() == 1 && block.is_none(),
        "find_by" | "find_by!" => block.is_none(),
        _ => matches!(
            name,
            "ids" | "pluck" | "count" | "first" | "last" | "exists?" | "any?" | "empty?" | "size"
                | "length"
        ),
    }
}

/// `first(n)` / `last(n)` on a relation are DIFFERENT METHODS from the
/// bare forms — they answer an Array of up to n records where `first`
/// answers one record or nil. The runtime splits them (`first_n` /
/// `last_n`) instead of overloading on arity, because one method cannot
/// carry both return types on a strict target; this renames the call
/// site to match.
///
/// Only ever called where the receiver has already been proven to be a
/// relation. That gate is the whole point: `Array#first(n)` and
/// `String#split.last(n)` mean the same thing Rails does and must be
/// left alone — lobsters' `parsed.to_html.split.first(words * 2)` is
/// exactly the shape a receiver-blind rename would corrupt.
///
/// A block form is excluded: `first { … }` is Enumerable#detect, a
/// different method again.
fn counted_terminal(method: &Symbol, args: &[Expr], block: Option<&Expr>) -> Option<Symbol> {
    if args.len() != 1 || block.is_some() {
        return None;
    }
    match method.as_str() {
        "first" => Some(Symbol::from("first_n")),
        "last" => Some(Symbol::from("last_n")),
        _ => None,
    }
}

/// `@room` names the model `Room` — the naming convention
/// `apply_route_param_lowering`'s third signal and the view lowerer's
/// `ivar_ty` already commit to, used here as the middle rung between a
/// stamped owner type and the by-assoc-name fallback.
///
/// It is the rung that carries real apps. The by-NAME map answers only
/// when ONE model declares the association, and two models declaring the
/// same collection is ordinary Rails, not an exotic shape: campfire has
/// `has_many :messages` on both Room (`room_id`) and User (`creator_id`),
/// and `has_many :memberships` on both. That collision maps both names to
/// None, so every `@room.messages.<scope>` chain in the app silently kept
/// the arel-folded Array and NoMethodError'd at runtime.
///
/// Only consulted when the owner carries no stamped type. The guard is
/// two-sided: the name must be an ingested model AND that model must
/// declare this association, so `@room.messages` resolves to
/// (Message, room_id) while `@user.messages` resolves to
/// (Message, creator_id) — each right, neither guessed. That is also
/// why the same rule can read a local (`user.sessions`) and a one-hop
/// read (`Current.user.memberships`): a name that isn't a model, or a
/// model that doesn't declare the collection, answers None and the
/// chain keeps its source shape.
fn owner_model_from_name(owner: &Expr, models: &HashSet<ClassId>) -> Option<ClassId> {
    let name = match &*owner.node {
        ExprNode::Ivar { name } => name.as_str(),
        ExprNode::Var { name, .. } => name.as_str(),
        // A zero-arg send names its model the same way: the bareword a
        // template local parses as (`story.comments`, where prism can't
        // prove `story` is a local), and the READ form `Current.user` /
        // `@message.creator` — see `owner_reads_once` at the call site.
        ExprNode::Send { method, args, block: None, .. } if args.is_empty() => method.as_str(),
        _ => return None,
    };
    let id = ClassId(Symbol::from(crate::naming::camelize(name).as_str()));
    models.contains(&id).then_some(id)
}

/// The `(owner model, target, foreign key)` a `<owner>.<has_many>` read
/// resolves to — the three rungs above, with no judgment about whether
/// the seed can REPRODUCE the association (that is
/// `AssocRegistry::is_unseedable`, and only the rewriter needs it).
///
/// Public because the strong-params binding scan asks the same question
/// for a different purpose: `@room.messages.create_with_attachment!(
/// message_params)` names `Message.create_with_attachment!`'s parameter
/// exactly as `Message.create_with_attachment!(message_params)` would,
/// and one resolver keeps the two answers from drifting.
pub fn assoc_read_target(
    owner: &Expr,
    aname: &Symbol,
    models: &HashSet<ClassId>,
    assocs: &AssocRegistry,
) -> Option<(Option<ClassId>, ClassId, Symbol)> {
    let typed = match owner.ty.as_ref().map(|t| t.peel_nilable()) {
        Some(crate::ty::Ty::Class { id, .. }) => assocs
            .has_many_fk(id, aname)
            .cloned()
            .map(|hit| (Some(id.clone()), hit)),
        _ => None,
    };
    typed
        .or_else(|| {
            owner_model_from_name(owner, models)
                .and_then(|m| assocs.has_many_fk(&m, aname).cloned().map(|hit| (Some(m), hit)))
        })
        .or_else(|| assocs.has_many_by_name(aname).cloned().map(|hit| (None, hit)))
        .map(|(owner_model, (target, fk))| (owner_model, target, fk))
}

/// May the seed take this owner expression apart and read `<owner>.id`
/// from it?
///
/// The seed REPLACES the association read, so the owner is evaluated
/// exactly once either way — what has to hold is that it is a plain
/// READ, cheap and side-effect-free, because the value now sits inside
/// a where-hash instead of being the receiver of a reader call. A
/// zero-arg, block-less send off a constant, `self`, a local or an ivar
/// (`Current.user`, `@message.creator`, `user.account`) qualifies; a
/// send taking arguments, a block, or a deeper chain does not — those
/// keep their source shape rather than being duplicated into a query.
fn owner_reads_once(owner: &Expr) -> bool {
    let ExprNode::Send { recv, args, block: None, .. } = &*owner.node else { return false };
    if !args.is_empty() {
        return false;
    }
    match recv {
        None => true,
        Some(root) => matches!(
            &*root.node,
            ExprNode::Const { .. } | ExprNode::SelfRef | ExprNode::Var { .. } | ExprNode::Ivar { .. }
        ),
    }
}

/// Association resolution for the seed arm: `(target, fk, <owner>.id)`,
/// from the owner's stamped type, else the owner's NAME, else the assoc
/// name when it is a unique has_many across models.
fn assoc_owner_seed(
    ctx: &Ctx,
    aname: &Symbol,
    owner: &Expr,
    span: crate::span::Span,
) -> Option<(ClassId, Symbol, Expr)> {
    // Resolution is `assoc_read_target`'s three rungs — the owner's
    // stamped type (which disambiguates an assoc name declared on
    // several models), then the owner's NAME, then the assoc name when
    // it is unique across models.
    //
    // What is this function's OWN judgment is the seedability filter:
    // each rung carries the OWNER it resolved through so the check can
    // ask about that exact declaration; the by-NAME rung has no owner
    // and is checked against every declarer.
    assoc_read_target(owner, aname, ctx.models, ctx.assocs)
        .filter(|(owner_model, _, _)| !ctx.assocs.is_unseedable(owner_model.as_ref(), aname))
        .map(|(_, target, fk)| {
            let owner_id = syn(
                span,
                ExprNode::Send {
                    recv: Some(owner.clone()),
                    method: Symbol::from("id"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            (target, fk, owner_id)
        })
}

fn const_expr(span: crate::span::Span, model: &ClassId) -> Expr {
    let path: Vec<Symbol> = model.0.as_str().split("::").map(Symbol::from).collect();
    syn(span, ExprNode::Const { path })
}

fn var_expr(span: crate::span::Span, name: &Symbol) -> Expr {
    syn(span, ExprNode::Var { id: VarId(0), name: name.clone() })
}

/// `ActiveRecord::Relation.new(Model)`.
fn relation_new(span: crate::span::Span, model: &ClassId) -> Expr {
    let recv = syn(
        span,
        ExprNode::Const { path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")] },
    );
    syn(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("new"),
            args: vec![const_expr(span, model)],
            block: None,
            parenthesized: true,
        },
    )
}

/// In-place argument rewrites for a relation chain method once the chain's
/// model is known:
///
///   joins(:hidings)      -> joins("INNER JOIN hidden_stories ON …")
///   where(user: user)    -> where(user_id: user && user.id)
///   not(user: user)      -> likewise (the `where.not` lowering)
///
/// Unknown association names (and `:through`) are left untouched. A hash
/// key renames whenever it names a `belongs_to`; its VALUE is narrowed to
/// `v && v.id` only for plain reads (Var/Ivar — evaluating twice is free);
/// literals ride as-is, so `where(user: nil)` stays `user_id IS NULL`, and
/// call-expression values are left alone rather than double-evaluated.
/// Returns the association names whose join in the RECEIVER chain must
/// be re-emitted with its Rails alias — see the nested-hash arm.
#[must_use]
fn lower_relation_args(
    model: &ClassId,
    method: &Symbol,
    args: &mut [Expr],
    ctx: &Ctx,
) -> Vec<Symbol> {
    let mut aliases: Vec<Symbol> = Vec::new();
    match method.as_str() {
        "joins" | "left_outer_joins" => {
            let kind = if method.as_str() == "joins" { "INNER JOIN" } else { "LEFT OUTER JOIN" };
            for a in args {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else { continue };
                if let Some(tail) = ctx.assocs.join_tail(model, value) {
                    *a.node = ExprNode::Lit { value: Literal::Str { value: format!("{kind} {tail}") } };
                }
            }
        }
        "where" | "not" | "find_by" => {
            for a in args.iter_mut() {
                let span = a.span;
                let ExprNode::Hash { entries, .. } = &mut *a.node else { continue };
                for (k, v) in entries.iter_mut() {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                        continue;
                    };
                    // Nested-hash value: the key names conditions on the
                    // JOINED table (`Comment.joins(:story).where(story:
                    // {merged_story_id: id})`), not on this model's
                    // foreign key — renaming to the fk column produced
                    // the nonexistent `story_id.merged_story_id`. The
                    // runtime's hash_conditions already qualifies
                    // `<outer>.<inner>`; what the outer name must be is
                    // Rails' choice: keying off the association name
                    // aliases the joined table to that name, so the key
                    // rides through unchanged and the join gains the
                    // alias (`aliases` — applied to the receiver chain
                    // by the caller). Shapes with no aliased tail
                    // (`:through`) fall back to the table name.
                    if matches!(&*v.node, ExprNode::Hash { .. }) {
                        if ctx.assocs.aliased_join_tail(model, key).is_some() {
                            aliases.push(key.clone());
                        } else if let Some(table) = ctx.assocs.assoc_table(model, key) {
                            *k.node = ExprNode::Lit { value: Literal::Sym { value: table.clone() } };
                        }
                        continue;
                    }
                    let Some(fk) = ctx.assocs.belongs_to_fk(model, key) else { continue };
                    *k.node = ExprNode::Lit { value: Literal::Sym { value: fk.clone() } };
                    // The VALUE rides through unchanged. It used to be
                    // narrowed to `v && v.id` for plain reads, but an
                    // untyped scope param can carry a record OR a
                    // collection (`where(comment: comments)` — Rails'
                    // IN-of-records form, lobsters Vote.comments_flags),
                    // and `.id` on the collection was a compile stop
                    // under AOT. The runtime's column_predicate now
                    // dispatches: record → id, array → ids IN, nil →
                    // IS NULL. One statically-known case keeps a typed
                    // narrowing — a collection-typed value maps to ids
                    // here so the emitted SQL shape is visible.
                    if matches!(&*v.node, ExprNode::Var { .. } | ExprNode::Ivar { .. })
                        && matches!(
                            v.ty.as_ref(),
                            Some(crate::ty::Ty::Array { .. })
                                | Some(crate::ty::Ty::Relation { .. })
                        )
                    {
                        let val = std::mem::replace(
                            v,
                            syn(span, ExprNode::Lit { value: Literal::Nil }),
                        );
                        let x = Symbol::from("__rh_rec");
                        let id_read = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(syn(
                                    span,
                                    ExprNode::Var {
                                        id: crate::ident::VarId(0),
                                        name: x.clone(),
                                    },
                                )),
                                method: Symbol::from("id"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        );
                        let block = syn(
                            span,
                            ExprNode::Lambda {
                                params: vec![x],
                                block_param: None,
                                body: id_read,
                                block_style: BlockStyle::Brace,
                            },
                        );
                        *v = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(val),
                                method: Symbol::from("map"),
                                args: vec![],
                                block: Some(block),
                                parenthesized: false,
                            },
                        );
                    }
                }
            }
        }
        _ => {}
    }
    aliases
}

/// Re-emit an already-lowered `joins(:assoc)` in the receiver chain with
/// the association-name alias Rails uses once a `where` hash keys off
/// that name. Both spellings come from the registry, so the match is on
/// the exact string this pass generated — a hand-written join string
/// (or a chain with no join at all) is left alone.
fn alias_join_in_chain(expr: &mut Expr, plain: &str, aliased: &str) -> bool {
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else { return false };
    if matches!(method.as_str(), "joins" | "left_outer_joins") {
        let kind = if method.as_str() == "joins" { "INNER JOIN" } else { "LEFT OUTER JOIN" };
        for a in args.iter_mut() {
            let ExprNode::Lit { value: Literal::Str { value } } = &mut *a.node else { continue };
            if *value == format!("{kind} {plain}") {
                *value = format!("{kind} {aliased}");
                return true;
            }
        }
    }
    match recv {
        Some(r) => alias_join_in_chain(r, plain, aliased),
        None => false,
    }
}

/// Apply every alias `lower_relation_args` asked for to the receiver
/// chain the `where` hangs off.
fn apply_join_aliases(recv: &mut Expr, model: &ClassId, aliases: &[Symbol], ctx: &Ctx) {
    for assoc in aliases {
        let (Some(plain), Some(aliased)) =
            (ctx.assocs.join_tail(model, assoc), ctx.assocs.aliased_join_tail(model, assoc))
        else {
            continue;
        };
        alias_join_in_chain(recv, plain, aliased);
    }
}

/// If `expr` is a bare `Const([M])` for a known model, return that model.
fn const_model(expr: &Expr, models: &HashSet<ClassId>) -> Option<ClassId> {
    if let ExprNode::Const { path } = &*expr.node {
        let joined = ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        ));
        if models.contains(&joined) {
            return Some(joined);
        }
    }
    None
}

/// Local variable -> relation model, accumulated as a method body's
/// statements are processed in order (so `q = Story.base(u); q.not_deleted`
/// resolves `not_deleted` against `q`'s Story relation).
type Locals = HashMap<Symbol, ClassId>;

/// Rewrite scope chains in `expr` (in place). Returns the relation-model of
/// the whole expression when it evaluates to a Relation of a known model.
pub fn rewrite(expr: &mut Expr, ctx: &Ctx, locals: &mut Locals) -> Option<ClassId> {
    match &*expr.node {
        // Statement sequence: thread `locals` left-to-right; the Seq's value
        // (and model) is its last statement.
        ExprNode::Seq { .. } => {
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Seq { exprs } = node else { unreachable!() };
            let mut last = None;
            let mut out = Vec::with_capacity(exprs.len());
            for mut e in exprs {
                last = rewrite(&mut e, ctx, locals);
                out.push(e);
            }
            *expr.node = ExprNode::Seq { exprs: out };
            last
        }
        // `name = value`: record the local's relation model (if any).
        ExprNode::Assign { .. } => {
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Assign { target, mut value } = node else { unreachable!() };
            let m = rewrite(&mut value, ctx, locals);
            if let crate::expr::LValue::Var { name, .. } = &target {
                match &m {
                    Some(model) => {
                        locals.insert(name.clone(), model.clone());
                    }
                    None => {
                        locals.remove(name);
                    }
                }
            }
            *expr.node = ExprNode::Assign { target, value };
            m
        }
        ExprNode::Send { .. } => rewrite_send(expr, ctx, locals),
        _ => {
            // Any other node (If/BoolOp/Case/…): recurse children, keeping
            // the same ctx + locals so the relation thread survives across
            // branches (a scope body's `if … q.preload … else q.not_deleted`).
            expr.node.for_each_child_mut(&mut |c| {
                rewrite(c, ctx, locals);
            });
            None
        }
    }
}

fn rewrite_send(expr: &mut Expr, ctx: &Ctx, locals: &mut Locals) -> Option<ClassId> {
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, method, mut args, mut block, parenthesized } = node else {
        unreachable!()
    };

    // `self.where(...)` in a scope body / model class method is the same
    // implicit-self query root as bare `where(...)` — Ruby just makes the
    // receiver visible. Normalize to the receiver-less form so the None
    // arm's rooting logic serves both spellings (`Keystore.value_for`'s
    // `self.where(key: key)` seeds exactly like `where(key: key)`).
    let recv = match recv {
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {
            let self_model =
                ctx.scope_body.as_ref().map(|(m, _)| m).or(ctx.class_self.as_ref());
            match self_model {
                Some(m)
                    if is_relation_chain_method(method.as_str())
                        || method.as_str() == "all"
                        || ctx.scope_of(m, &method) =>
                {
                    None
                }
                _ => Some(r),
            }
        }
        // …and so is `Message.count` INSIDE a body that takes a
        // threaded relation. `ingest::app::
        // qualify_model_class_method_ar_calls` wrote that receiver, not
        // the author; here — and only here, where there is a `__rel` to
        // root on — it is read back as the implicit self it stands for.
        // Outside a scope body the constant keeps its class-level
        // reading, which is what the strict targets and every direct
        // `Message.paged?` call want.
        Some(r)
            if ctx.self_const_is_implicit
                && ctx.scope_body.as_ref().is_some_and(|(m, _)| {
                    INGEST_QUALIFIED_AR_CALLS.contains(&method.as_str())
                        && const_model(&r, ctx.models).as_ref() == Some(m)
                }) =>
        {
            None
        }
        other => other,
    };

    // Args + block are independent subtrees: they root at their own
    // constants (drop the scope-body relation), but may still read outer
    // locals.
    let arg_ctx = ctx.at_callsite();
    for a in &mut args {
        rewrite(a, &arg_ctx, locals);
    }
    if let Some(b) = &mut block {
        rewrite(b, &arg_ctx, locals);
    }

    let put = |span: crate::span::Span, recv, method, args, block, parenthesized| -> Expr {
        syn(span, ExprNode::Send { recv, method, args, block, parenthesized })
    };

    match recv {
        None => {
            if let Some((self_model, rel)) = &ctx.scope_body {
                // Bare `all` inside a scope body IS the current relation —
                // not `Model.all` (which would hit Base.all and return an
                // Array, breaking the chain). Replace with the rel param.
                if method.as_str() == "all" && args.is_empty() && block.is_none() {
                    *expr = var_expr(span, rel);
                    return Some(self_model.clone());
                }
                if ctx.scope_of(self_model, &method) {
                    let leading = ctx.scope_params(self_model, &method);
                    let new_args = thread_rel(args, var_expr(span, rel), leading, span);
                    *expr = put(span, Some(const_expr(span, self_model)), method, new_args, block, true);
                    return Some(self_model.clone());
                }
                if is_relation_chain_method(method.as_str()) {
                    // Receiver is the bare `__rel` param — any `joins`
                    // sits in a separate chain link handled below.
                    let _ = lower_relation_args(self_model, &method, &mut args, ctx);
                    *expr = put(span, Some(var_expr(span, rel)), method, args, block, parenthesized);
                    return Some(self_model.clone());
                }
                // A TERMINAL ends on the threaded relation too — `count`
                // in `Message.paged?` counts THIS association's rows, not
                // the table's. Safe here and only here: a scope-body
                // `self` IS the model class, so the bare name can mean
                // nothing but the relation method. Elsewhere (`class_self`
                // with no `__rel` to thread) the bare form stays put and
                // the arel pass folds it to a whole-table query, which is
                // what an unscoped `Model.count` means.
                if is_relation_terminal(method.as_str(), &args, block.as_ref()) {
                    let method = counted_terminal(&method, &args, block.as_ref()).unwrap_or(method);
                    *expr = put(span, Some(var_expr(span, rel)), method, args, block, parenthesized);
                    return None;
                }
            }
            if let Some(self_model) = ctx.class_self.clone() {
                // Implicit-self query root in a model's own class method:
                // `all` IS a fresh relation; a bare scope call is the
                // class-level form; a bare chain method seeds a new
                // relation (there's no `__rel` param here to thread).
                if method.as_str() == "all" && args.is_empty() && block.is_none() {
                    *expr = relation_new(span, &self_model);
                    return Some(self_model);
                }
                if ctx.scope_of(&self_model, &method) {
                    *expr =
                        put(span, Some(const_expr(span, &self_model)), method, args, block, true);
                    return Some(self_model);
                }
                if is_relation_chain_method(method.as_str()) {
                    let _ = lower_relation_args(&self_model, &method, &mut args, ctx);
                    let seed = relation_new(span, &self_model);
                    *expr = put(span, Some(seed), method, args, block, parenthesized);
                    return Some(self_model);
                }
            }
            *expr = put(span, None, method, args, block, parenthesized);
            None
        }
        Some(mut r) => {
            // Class-level call on a model constant.
            if let Some(m) = const_model(&r, ctx.models) {
                if ctx.scope_of(&m, &method) {
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(m);
                }
                if is_relation_chain_method(method.as_str()) || method.as_str() == "all" {
                    let seed = relation_new(span, &m);
                    if method.as_str() == "all" {
                        *expr = seed;
                    } else {
                        let _ = lower_relation_args(&m, &method, &mut args, ctx);
                        *expr = put(span, Some(seed), method, args, block, parenthesized);
                    }
                    return Some(m);
                }
                *expr = put(span, Some(r), method, args, block, parenthesized);
                return None;
            }

            // Scope/chain/terminal on an association read: `self.<has_many>.
            // <scope>(args)` or `@story.<has_many>.ids`. The assoc reader
            // returns the arel-inlined Array, and relation surface needs a
            // Relation — seed one from the association's foreign key
            // (`Relation.new(Target).where(fk: <owner>.id)`). Owner forms:
            // SelfRef (assoc resolved on the enclosing model via
            // instance_self) or Ivar/Var (assoc resolved by NAME when unique
            // across models; Ivar/Var only — a Send owner would re-evaluate
            // in the seed). Fires only when relation surface FOLLOWS the
            // read: registered scopes thread the seed, chain methods and
            // terminals ride it as receiver. Plain iteration (`each`/`map`)
            // keeps the Array and the reader's preload cache.
            if let ExprNode::Send { recv: Some(ir), method: aname, args: aargs, .. } = &*r.node {
                if aargs.is_empty() {
                    // (target, fk, owner-id expression)
                    let resolved: Option<(ClassId, Symbol, Expr)> = match &*ir.node {
                        ExprNode::SelfRef => ctx.instance_self.clone().and_then(|self_model| {
                            if ctx.assocs.is_unseedable(Some(&self_model), aname) {
                                return None;
                            }
                            ctx.assocs.has_many_fk(&self_model, aname).cloned().map(
                                |(target, fk)| {
                                    (target, fk, syn(span, ExprNode::Ivar { name: Symbol::from("id") }))
                                },
                            )
                        }),
                        // Var/Ivar, plus a zero-arg READ — the bareword a
                        // template local parses as (prism can't prove
                        // `story` is a local inside an ERB-ingested body,
                        // so it arrives as a Send), and the one-hop form
                        // `Current.user.memberships` that every
                        // room-scoped controller opens with.
                        ExprNode::Ivar { .. } | ExprNode::Var { .. } => {
                            assoc_owner_seed(ctx, aname, ir, span)
                        }
                        ExprNode::Send { .. } if owner_reads_once(ir) => {
                            assoc_owner_seed(ctx, aname, ir, span)
                        }
                        _ => None,
                    };
                    if let Some((target, fk, owner_id)) = resolved {
                        // CollectionProxy constructors: `story.comments
                        // .build(attrs)` constructs the target with the
                        // association's FK preset — `Comment.new(attrs…,
                        // story_id: story.id)`. Checked ahead of the
                        // scope/chain/terminal seeding: a constructor
                        // returns a record, not a relation. Only the
                        // kwargs-hash arg shapes rewrite (none, or one
                        // kwargs hash — the corpus forms; Rails merges
                        // the FK over caller attrs, so it appends last).
                        let ctor = match method.as_str() {
                            "build" => Some("new"),
                            "create" => Some("create"),
                            "create!" => Some("create!"),
                            _ => None,
                        };
                        if let Some(ctor_name) = ctor {
                            let fk_entry = (
                                syn(
                                    span,
                                    ExprNode::Lit {
                                        value: Literal::Sym { value: fk.clone() },
                                    },
                                ),
                                owner_id.clone(),
                            );
                            let merged: Option<Vec<Expr>> = if args.is_empty() {
                                Some(vec![syn(
                                    span,
                                    ExprNode::Hash { entries: vec![fk_entry], kwargs: true },
                                )])
                            } else if args.len() == 1 {
                                match &*args[0].node {
                                    ExprNode::Hash { entries, kwargs: true } => {
                                        let mut entries = entries.clone();
                                        entries.push(fk_entry);
                                        Some(vec![syn(
                                            span,
                                            ExprNode::Hash { entries, kwargs: true },
                                        )])
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            if let Some(merged) = merged {
                                *expr = put(
                                    span,
                                    Some(const_expr(span, &target)),
                                    Symbol::from(ctor_name),
                                    merged,
                                    block,
                                    true,
                                );
                                return None;
                            }
                        }
                        let is_scope = ctx.scope_of(&target, &method);
                        let is_chain = is_relation_chain_method(method.as_str());
                        let is_term =
                            is_relation_terminal(method.as_str(), &args, block.as_ref());
                        // A registered class method takes the same
                        // threaded relation a scope does, but reads it as
                        // its CREATE scope rather than as a filter — so
                        // the seed is spelled `where_scope`, which records
                        // the foreign key as `scope_attributes` too.
                        let assoc_cm = ctx.assoc_class_method_params(&target, &method);
                        if is_scope || is_chain || is_term || assoc_cm.is_some() {
                            let fk_hash = syn(
                                span,
                                ExprNode::Hash {
                                    entries: vec![(
                                        syn(
                                            span,
                                            ExprNode::Lit { value: Literal::Sym { value: fk } },
                                        ),
                                        owner_id,
                                    )],
                                    kwargs: true,
                                },
                            );
                            let seed_method =
                                if assoc_cm.is_some() { "where_scope" } else { "where" };
                            let seed = syn(
                                span,
                                ExprNode::Send {
                                    recv: Some(relation_new(span, &target)),
                                    method: Symbol::from(seed_method),
                                    args: vec![fk_hash],
                                    block: None,
                                    parenthesized: true,
                                },
                            );
                            if let Some(leading) = assoc_cm {
                                let new_args = thread_rel(args, seed, Some(leading), span);
                                *expr = put(
                                    span,
                                    Some(const_expr(span, &target)),
                                    method,
                                    new_args,
                                    block,
                                    true,
                                );
                                // The method returns whatever it built —
                                // a record, not a relation.
                                return None;
                            }
                            if is_scope {
                                let leading = ctx.scope_params(&target, &method);
                                let new_args = thread_rel(args, seed, leading, span);
                                *expr = put(
                                    span,
                                    Some(const_expr(span, &target)),
                                    method,
                                    new_args,
                                    block,
                                    true,
                                );
                                return Some(target);
                            }
                            // Chain method or terminal: stays on the seeded
                            // receiver. Chains keep the model; terminals end it.
                            let keeps_model = is_chain;
                            let method = counted_terminal(&method, &args, block.as_ref())
                                .unwrap_or(method);
                            *expr = put(span, Some(seed), method, args, block, parenthesized);
                            return keeps_model.then_some(target);
                        }
                    }
                }
            }

            // Receiver model: a local var holding a relation, else the
            // (rewritten) receiver chain's model, else a user instance
            // method with a registered (unique) relation return type
            // (`@story.merged_comments` → Comment).
            let r_model = match &*r.node {
                ExprNode::Var { name, .. } => locals.get(name).cloned(),
                _ => rewrite(&mut r, ctx, locals),
            };
            let r_model = r_model.or_else(|| match &*r.node {
                ExprNode::Send { method: rname, .. } => {
                    ctx.user_returns.get(rname).cloned().flatten()
                }
                _ => None,
            });

            if let Some(mr) = r_model {
                if let Some(counted) = counted_terminal(&method, &args, block.as_ref()) {
                    *expr = put(span, Some(r), counted, args, block, parenthesized);
                    return None;
                }
                if ctx.scope_of(&mr, &method) {
                    let leading = ctx.scope_params(&mr, &method);
                    let new_args = thread_rel(args, r, leading, span);
                    *expr = put(span, Some(const_expr(span, &mr)), method, new_args, block, true);
                    return Some(mr);
                }
                // A registered association-scoped class method takes the
                // relation the same way a scope does — the relation is
                // ALREADY in hand here (`messages = @room.messages
                // .with_creator …; messages.page_around(m)`), so it is
                // threaded as-is rather than seeded. Unlike a scope it
                // returns whatever its body returns — an Array, a boolean
                // — so the model does not survive the call.
                if let Some(leading) = ctx.assoc_class_method_params(&mr, &method) {
                    let new_args = thread_rel(args, r, Some(leading), span);
                    *expr = put(span, Some(const_expr(span, &mr)), method, new_args, block, true);
                    return None;
                }
                if is_relation_chain_method(method.as_str()) {
                    let aliases = lower_relation_args(&mr, &method, &mut args, ctx);
                    let mut r = r;
                    apply_join_aliases(&mut r, &mr, &aliases, ctx);
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(mr);
                }
                *expr = put(span, Some(r), method, args, block, parenthesized);
                return None;
            }

            *expr = put(span, Some(r), method, args, block, parenthesized);
            None
        }
    }
}

/// Rewrite a scope body: implicit-self query roots thread `rel_param`.
pub fn rewrite_scope_body(
    body: &mut Expr,
    self_model: &ClassId,
    rel_param: &Symbol,
    scopes: &ScopeRegistry,
    models: &HashSet<ClassId>,
    assocs: &AssocRegistry,
) {
    rewrite_relation_taking_body(body, self_model, rel_param, scopes, models, assocs, false);
}

/// The same rewrite for an ASSOCIATION-scoped class method's body, which
/// additionally reads `<Model>.count` as the implicit self it stands for
/// — there the receiver is ingest's writing, not the author's (see
/// [`Ctx::self_const_is_implicit`]).
pub fn rewrite_assoc_scope_body(
    body: &mut Expr,
    self_model: &ClassId,
    rel_param: &Symbol,
    scopes: &ScopeRegistry,
    models: &HashSet<ClassId>,
    assocs: &AssocRegistry,
) {
    rewrite_relation_taking_body(body, self_model, rel_param, scopes, models, assocs, true);
}

#[allow(clippy::too_many_arguments)]
fn rewrite_relation_taking_body(
    body: &mut Expr,
    self_model: &ClassId,
    rel_param: &Symbol,
    scopes: &ScopeRegistry,
    models: &HashSet<ClassId>,
    assocs: &AssocRegistry,
    self_const_is_implicit: bool,
) {
    // Scope bodies don't consult user-method return types (none
    // exercised there yet), nor the association-scoped class methods
    // (those are resolved from call-site demand, which is collected
    // after this runs) — conservative empty registries.
    let empty_returns = UserMethodReturns::new();
    let empty_assoc_cm = AssocClassMethods::new();
    let ctx = Ctx {
        scopes,
        models,
        assocs,
        assoc_class_methods: &empty_assoc_cm,
        scope_body: Some((self_model.clone(), rel_param.clone())),
        self_const_is_implicit,
        class_self: None,
        instance_self: None,
        user_returns: &empty_returns,
    };
    let mut locals = Locals::new();
    rewrite(body, &ctx, &mut locals);
}

/// Rewrite a non-scope-body expression (controller action, library-class
/// method, model instance method): scope chains root at a model constant.
/// `class_self` carries the model when the body is that model's own
/// class method, so bare implicit-self roots (`where(key: key)`) seed.
pub fn rewrite_call_site(
    expr: &mut Expr,
    regs: &Registries<'_>,
    class_self: Option<&ClassId>,
    instance_self: Option<&ClassId>,
) {
    let ctx = Ctx {
        scopes: regs.scopes,
        models: regs.models,
        assocs: regs.assocs,
        assoc_class_methods: regs.assoc_class_methods,
        scope_body: None,
        self_const_is_implicit: false,
        class_self: class_self.cloned(),
        instance_self: instance_self.cloned(),
        user_returns: regs.user_returns,
    };
    let mut locals = Locals::new();
    rewrite(expr, &ctx, &mut locals);
}

/// The whole-app registries a call-site rewrite reads, built once per
/// emit. Bundled so a new registry — this file has grown four — widens
/// one struct instead of every signature between here and the emitter.
pub struct Registries<'a> {
    pub scopes: &'a ScopeRegistry,
    pub models: &'a HashSet<ClassId>,
    pub assocs: &'a AssocRegistry,
    pub assoc_class_methods: &'a AssocClassMethods,
    pub user_returns: &'a UserMethodReturns,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn span() -> Span {
        Span::synthetic()
    }
    fn int_lit(n: i64) -> Expr {
        Expr::new(span(), ExprNode::Lit { value: Literal::Int { value: n } })
    }
    fn rel_marker() -> Expr {
        Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from("__rel") })
    }
    fn is_nil(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
    }
    fn is_rel(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "__rel")
    }

    #[test]
    fn thread_rel_pads_omitted_optional_leading_params() {
        // `hottest(user = nil, exclude_tags = nil)` called bare → the rel
        // must land in the 3rd (__rel) slot, with the two optionals padded.
        let leading = vec![
            Param::with_default(Symbol::from("user"), Expr::new(span(), ExprNode::Lit { value: Literal::Nil })),
            Param::with_default(Symbol::from("exclude_tags"), Expr::new(span(), ExprNode::Lit { value: Literal::Nil })),
        ];
        let out = thread_rel(vec![], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 3);
        assert!(is_nil(&out[0]) && is_nil(&out[1]) && is_rel(&out[2]));
    }

    #[test]
    fn thread_rel_pads_with_the_params_own_default() {
        // `low_scoring(max = 5)` bare → pad max with its default `5`, not nil.
        let leading = vec![Param::with_default(Symbol::from("max"), int_lit(5))];
        let out = thread_rel(vec![], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(matches!(&*out[0].node, ExprNode::Lit { value: Literal::Int { value: 5 } }));
        assert!(is_rel(&out[1]));
    }

    #[test]
    fn thread_rel_no_padding_when_all_supplied() {
        // `base(user)` — one required param supplied → just append the rel.
        let leading = vec![Param::positional(Symbol::from("user"))];
        let supplied = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("user") });
        let out = thread_rel(vec![supplied], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(is_rel(&out[1]));
    }

    #[test]
    fn thread_rel_scopeless_just_appends() {
        // A scope with no user params (`positive_ranked`) → append only.
        let out = thread_rel(vec![], rel_marker(), Some(&vec![]), span());
        assert_eq!(out.len(), 1);
        assert!(is_rel(&out[0]));
    }

    // ---- lower_relation_args ----------------------------------------

    fn story() -> ClassId {
        ClassId(Symbol::from("Story"))
    }

    /// Story: has_many :hidings (HiddenStory, fk story_id);
    /// HiddenStory: belongs_to :user (fk user_id).
    fn assoc_fixture() -> AssocRegistry {
        let mut reg = AssocRegistry::default();
        reg.join_tails.insert(
            (story(), Symbol::from("hidings")),
            "hidden_stories ON hidden_stories.story_id = stories.id".to_string(),
        );
        reg.belongs_to_fk.insert(
            (ClassId(Symbol::from("HiddenStory")), Symbol::from("user")),
            Symbol::from("user_id"),
        );
        reg
    }

    fn empty_returns() -> &'static UserMethodReturns {
        static EMPTY: std::sync::OnceLock<UserMethodReturns> = std::sync::OnceLock::new();
        EMPTY.get_or_init(UserMethodReturns::new)
    }

    fn empty_assoc_cm() -> &'static AssocClassMethods {
        static EMPTY: std::sync::OnceLock<AssocClassMethods> = std::sync::OnceLock::new();
        EMPTY.get_or_init(AssocClassMethods::new)
    }

    fn regs<'a>(
        scopes: &'a ScopeRegistry,
        models: &'a HashSet<ClassId>,
        assocs: &'a AssocRegistry,
    ) -> Registries<'a> {
        Registries {
            scopes,
            models,
            assocs,
            assoc_class_methods: empty_assoc_cm(),
            user_returns: empty_returns(),
        }
    }

    fn ctx_with<'a>(
        scopes: &'a ScopeRegistry,
        models: &'a HashSet<ClassId>,
        assocs: &'a AssocRegistry,
    ) -> Ctx<'a> {
        Ctx {
            scopes,
            models,
            assocs,
            assoc_class_methods: empty_assoc_cm(),
            scope_body: None,
            self_const_is_implicit: false,
            class_self: None,
            instance_self: None,
            user_returns: empty_returns(),
        }
    }

    fn sym_lit(s: &str) -> Expr {
        Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }

    #[test]
    fn joins_sym_expands_to_join_sql() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("hidings")];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str, got {:?}", args[0].node)
        };
        assert_eq!(value, "INNER JOIN hidden_stories ON hidden_stories.story_id = stories.id");
    }

    #[test]
    fn left_outer_joins_uses_left_outer_prefix() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("hidings")];
        lower_relation_args(&story(), &Symbol::from("left_outer_joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str")
        };
        assert!(value.starts_with("LEFT OUTER JOIN hidden_stories ON "));
    }

    #[test]
    fn joins_unknown_assoc_left_untouched() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("taggings")];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        assert!(matches!(
            &*args[0].node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "taggings"
        ));
    }

    #[test]
    fn where_belongs_to_key_renames_and_passes_untyped_value_through() {
        // HiddenStory scope `by`: where(user: user) → where(user_id: user).
        // The value rides RAW: an untyped scope param can be a record or
        // a collection (`where(comment: comments)` — lobsters
        // Vote.comments_flags), so the runtime's column_predicate
        // dispatches (record → id, array → ids IN). The old static
        // `user && user.id` narrowing broke the collection case.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let user_var = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("user") });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("user"), user_var)], kwargs: true },
        )];
        lower_relation_args(
            &ClassId(Symbol::from("HiddenStory")),
            &Symbol::from("where"),
            &mut args,
            &ctx,
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user_id"
        ));
        assert!(
            matches!(&*v.node, ExprNode::Var { name, .. } if name.as_str() == "user"),
            "value must pass through unchanged, got {:?}",
            v.node
        );
    }

    #[test]
    fn where_belongs_to_key_maps_collection_typed_value_to_ids() {
        // A value KNOWN to be a collection maps to ids statically —
        // `where(comment: comments)` with `comments: Array` becomes
        // `where(comment_id: comments.map { |r| r.id })`.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut comments_var =
            Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("comments") });
        comments_var.ty = Some(crate::ty::Ty::Array {
            elem: Box::new(crate::ty::Ty::Untyped),
        });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("user"), comments_var)], kwargs: true },
        )];
        lower_relation_args(
            &ClassId(Symbol::from("HiddenStory")),
            &Symbol::from("where"),
            &mut args,
            &ctx,
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (_, v) = &entries[0];
        let ExprNode::Send { method, block: Some(_), .. } = &*v.node else {
            panic!("expected `comments.map {{ ... }}`, got {:?}", v.node)
        };
        assert_eq!(method.as_str(), "map");
    }

    #[test]
    fn where_belongs_to_key_with_nil_value_renames_only() {
        // where(user: nil) → where(user_id: nil) — `user_id IS NULL`.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let nil = Expr::new(span(), ExprNode::Lit { value: Literal::Nil });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("user"), nil)], kwargs: true },
        )];
        lower_relation_args(
            &ClassId(Symbol::from("HiddenStory")),
            &Symbol::from("where"),
            &mut args,
            &ctx,
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user_id"
        ));
        assert!(is_nil(v));
    }

    #[test]
    fn where_non_assoc_key_untouched() {
        // where(id: x) on Story — `id` is no association; nothing changes.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let x = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("x") });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("id"), x)], kwargs: true },
        )];
        lower_relation_args(&story(), &Symbol::from("where"), &mut args, &ctx);
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "id"
        ));
        assert!(matches!(&*v.node, ExprNode::Var { .. }));
    }

    // ---- build_assoc_registry: has_many :through ----------------------

    fn ingest(src: &str, path: &str) -> crate::dialect::Model {
        crate::ingest::ingest_model(src.as_bytes(), path, &crate::schema::Schema::default())
            .expect("ingest")
            .expect("model")
    }

    #[test]
    fn var_assoc_read_scope_chain_seeds_relation() {
        // story.merged_stories.not_deleted → Story.not_deleted(
        //   Relation.new(Story).where(merged_story_id: story.id))
        let story = ingest(
            "class Story < ApplicationRecord\n  has_many :merged_stories, class_name: \"Story\", foreign_key: \"merged_story_id\"\n  scope :not_deleted, -> { where(is_deleted: false) }\nend\n",
            "app/models/story.rb",
        );
        let models_v = vec![story];
        let scopes = build_scope_registry(&models_v);
        let models = model_set(&models_v);
        let assocs = build_assoc_registry(&models_v);
        let var_story =
            Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from("story") });
        let assoc_read = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(var_story),
                method: Symbol::from("merged_stories"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let mut expr = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(assoc_read),
                method: Symbol::from("not_deleted"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        rewrite_call_site(&mut expr, &regs(&scopes, &models, &assocs), None, None);
        let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node else {
            panic!("expected Send, got {:?}", expr.node);
        };
        assert_eq!(method.as_str(), "not_deleted");
        assert!(
            matches!(&*r.node, ExprNode::Const { path } if path.last().unwrap().as_str() == "Story"),
            "receiver should be Story const, got {:?}",
            r.node
        );
        assert_eq!(args.len(), 1, "threaded seed arg expected: {:?}", args);
    }

    #[test]
    fn nested_hash_where_key_aliases_the_join_like_rails() {
        // lobsters' Story#merged_comments:
        //   Comment.joins(:story).where(story: {merged_story_id: id})
        // Rails aliases the joined table to the association name as
        // soon as the hash keys off it — `INNER JOIN "stories" "story"
        // … WHERE "story"."merged_story_id" = …` — and the app's own
        // SQL fragments are written against that alias. Keying off the
        // fk column instead produced `story_id.merged_story_id`.
        let comment = ingest(
            "class Comment < ApplicationRecord\n  belongs_to :story\nend\n",
            "app/models/comment.rb",
        );
        let story = ingest(
            "class Story < ApplicationRecord\n  has_many :comments\nend\n",
            "app/models/story.rb",
        );
        let models_v = vec![comment, story];
        let scopes = build_scope_registry(&models_v);
        let models = model_set(&models_v);
        let assocs = build_assoc_registry(&models_v);

        let comment_const = Expr::new(
            span(),
            ExprNode::Const { path: vec![Symbol::from("Comment")] },
        );
        let joins = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(comment_const),
                method: Symbol::from("joins"),
                args: vec![sym_lit("story")],
                block: None,
                parenthesized: false,
            },
        );
        let inner = Expr::new(
            span(),
            ExprNode::Hash {
                entries: vec![(
                    sym_lit("merged_story_id"),
                    Expr::new(span(), ExprNode::Lit { value: Literal::Int { value: 7 } }),
                )],
                kwargs: false,
            },
        );
        let mut expr = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(joins),
                method: Symbol::from("where"),
                args: vec![Expr::new(
                    span(),
                    ExprNode::Hash { entries: vec![(sym_lit("story"), inner)], kwargs: true },
                )],
                block: None,
                parenthesized: false,
            },
        );
        rewrite_call_site(&mut expr, &regs(&scopes, &models, &assocs), None, None);

        // The where key stays the association name (= the alias)…
        let ExprNode::Send { recv: Some(r), args, .. } = &*expr.node else {
            panic!("expected Send, got {:?}", expr.node);
        };
        let ExprNode::Hash { entries, .. } = &*args[0].node else {
            panic!("expected hash arg, got {:?}", args[0].node);
        };
        assert!(
            matches!(&*entries[0].0.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "story"),
            "where key should stay the association name: {:?}",
            entries[0].0.node
        );

        // …and the join underneath it gains that alias.
        let ExprNode::Send { args: join_args, .. } = &*r.node else {
            panic!("expected joins Send, got {:?}", r.node);
        };
        let ExprNode::Lit { value: Literal::Str { value } } = &*join_args[0].node else {
            panic!("expected join SQL, got {:?}", join_args[0].node);
        };
        assert_eq!(value, "INNER JOIN stories story ON story.id = comments.story_id");
    }

    #[test]
    fn registry_resolves_has_many_through_join_tails() {
        // Tag.joins(:stories) — owner-side two-hop tail through taggings.
        let tag = ingest(
            "class Tag < ApplicationRecord\n  has_many :taggings\n  has_many :stories, through: :taggings\nend\n",
            "app/models/tag.rb",
        );
        let tagging = ingest(
            "class Tagging < ApplicationRecord\n  belongs_to :tag\n  belongs_to :story\nend\n",
            "app/models/tagging.rb",
        );
        let reg = build_assoc_registry(&[tag, tagging]);
        assert_eq!(
            reg.join_tail(&ClassId(Symbol::from("Tag")), &Symbol::from("stories")),
            Some(
                &"taggings ON taggings.tag_id = tags.id \
                   INNER JOIN stories ON stories.id = taggings.story_id"
                    .to_string()
            )
        );
    }

    #[test]
    fn registry_through_source_rename_resolves_by_target_class() {
        // `has_many :upvoted_stories, through: :votes, source: :story` —
        // ingest folds `source:` into the target class (Story); the through
        // model's `belongs_to :story` supplies the source fk.
        let user = ingest(
            "class User < ApplicationRecord\n  has_many :votes\n  has_many :upvoted_stories, through: :votes, source: :story\nend\n",
            "app/models/user.rb",
        );
        let vote = ingest(
            "class Vote < ApplicationRecord\n  belongs_to :user\n  belongs_to :story\nend\n",
            "app/models/vote.rb",
        );
        let reg = build_assoc_registry(&[user, vote]);
        assert_eq!(
            reg.join_tail(&ClassId(Symbol::from("User")), &Symbol::from("upvoted_stories")),
            Some(
                &"votes ON votes.user_id = users.id \
                   INNER JOIN stories ON stories.id = votes.story_id"
                    .to_string()
            )
        );
    }

    #[test]
    fn registry_skips_unresolvable_through() {
        // Through model absent from the set → no tail; joins(:stories)
        // stays a visible runtime symbol, not a guessed JOIN.
        let tag = ingest(
            "class Tag < ApplicationRecord\n  has_many :taggings\n  has_many :stories, through: :taggings\nend\n",
            "app/models/tag.rb",
        );
        let reg = build_assoc_registry(&[tag]);
        assert!(reg.join_tail(&ClassId(Symbol::from("Tag")), &Symbol::from("stories")).is_none());
    }
}
