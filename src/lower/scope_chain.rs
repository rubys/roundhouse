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

/// model class id -> (scope name -> the scope's user params, in order).
/// The params are the lambda's own parameters (NOT the synthesized trailing
/// `__rel`); the rewriter reads them to pad omitted leading args so a
/// threaded relation lands in the `__rel` slot (see `thread_rel`).
pub type ScopeRegistry = HashMap<ClassId, HashMap<Symbol, Vec<Param>>>;

/// Per-model UNIQUE key column sets — the conflict targets an
/// `insert_all` has to skip on (see the inlining in [`rewrite`]).
///
/// Only indexes whose every column is NOT NULL are carried. A unique
/// index over a nullable column is a conflict target the guard cannot
/// read: `where(col: nil)` asks for rows whose column IS NULL, which is
/// a different question from "no value here" and, in SQLite, is not
/// even the question the index answers (NULLs compare distinct, so such
/// rows never conflict). Dropping those indexes leaves the guard
/// conservative — it skips only rows it positively found.
pub type UniqueKeys = HashMap<ClassId, Vec<Vec<Symbol>>>;

/// Read the unique keys off the schema, keyed by model.
pub fn build_unique_keys(models: &[Model], schema: &crate::schema::Schema) -> UniqueKeys {
    let mut out: UniqueKeys = HashMap::new();
    for m in models {
        let Some(table) = schema.tables.get(&m.table.0) else { continue };
        let not_null = |name: &Symbol| {
            table.columns.iter().any(|c| &c.name == name && !c.nullable)
        };
        let keys: Vec<Vec<Symbol>> = table
            .indexes
            .iter()
            .filter(|i| i.unique && !i.columns.is_empty())
            .filter(|i| i.columns.iter().all(not_null))
            .map(|i| i.columns.clone())
            .collect();
        if !keys.is_empty() {
            out.insert(m.name.clone(), keys);
        }
    }
    out
}

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

/// The MODEL whose relation `expr` is, when the expression is a chain
/// of relation-preserving hops rooted at the model's own constant and
/// at least ONE hop deep: `User.active`, `Room.where(open: true)
/// .ordered`. The depth requirement is the whole point — a bare
/// `User.some_method` is a plain class-level call with no scope to
/// carry, and registering it would grow a parameter for nothing.
///
/// Loose in the same way [`assoc_read_name`] is loose: a hop is a
/// Relation built-in or a name SOME model declares as a scope. This
/// only decides whether to ASK about a method; the ask is resolved
/// precisely in [`build_assoc_class_methods`].
fn model_relation_root(
    expr: &Expr,
    models: &HashSet<ClassId>,
    scope_names: &HashSet<Symbol>,
) -> Option<ClassId> {
    let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return None };
    if !(is_relation_chain_method(method.as_str())
        || method.as_str() == "all"
        || scope_names.contains(method))
    {
        return None;
    }
    const_model(r, models).or_else(|| model_relation_root(r, models, scope_names))
}

/// `(model, method)` for every call whose receiver is a model-rooted
/// RELATION — the scope-chain twin of [`collect_assoc_method_demand`].
///
/// Rails runs `User.active.find_by_transfer_id(id)` with the relation
/// as the current scope, exactly as it runs the association form. The
/// association half has always been surveyed; the scope half was not,
/// so the call reached a Relation that has no such method and died on
/// `undefined method` — campfire's session-transfer page, whose
/// `find_by_transfer_id` is a `class_methods do` block on User.
pub fn collect_relation_class_method_demand(
    expr: &Expr,
    models: &HashSet<ClassId>,
    scope_names: &HashSet<Symbol>,
    out: &mut HashSet<(ClassId, Symbol)>,
) {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if let Some(m) = model_relation_root(r, models, scope_names) {
            out.insert((m, method.clone()));
        }
    }
    expr.node.for_each_child(&mut |c| {
        collect_relation_class_method_demand(c, models, scope_names, out)
    });
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
    let models_set = model_set(&app.models);
    let mut demand: HashSet<(Symbol, Symbol)> = HashSet::new();
    let mut model_demand: HashSet<(ClassId, Symbol)> = HashSet::new();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        collect_assoc_method_demand(body, assocs, &scope_names, &mut demand);
        collect_relation_class_method_demand(body, &models_set, &scope_names, &mut model_demand);
    });
    for view in &app.views {
        collect_assoc_method_demand(&view.body, assocs, &scope_names, &mut demand);
        collect_relation_class_method_demand(
            &view.body,
            &models_set,
            &scope_names,
            &mut model_demand,
        );
    }
    build_assoc_class_methods(&app.models, assocs, scopes, &demand, &model_demand)
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
    model_demand: &HashSet<(ClassId, Symbol)>,
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
    // The scope-chain channel: a class method called on a relation the
    // MODEL'S OWN constant roots (`User.active.find_by_transfer_id`).
    // Same resolution, same registry — only the way the target model was
    // named differs, so there is no association to check for seedability
    // and nothing to look up.
    let mut wanted_models: Vec<&(ClassId, Symbol)> = model_demand.iter().collect();
    wanted_models
        .sort_by(|a, b| (a.0 .0.as_str(), a.1.as_str()).cmp(&(b.0 .0.as_str(), b.1.as_str())));
    for (target, mname) in wanted_models {
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
            // A body that neither constructs nor queries at implicit
            // self is INDIFFERENT to the scope — but the call still has
            // to reach it, and a Relation has no such method. Register
            // it with both halves false: it grows the `__rel` parameter
            // (defaulted, so direct `Model.x` calls are unchanged) and
            // the call site re-roots at the constant, which is what
            // makes the send resolve at all.
            AssocScopeShape::None => {
                reg.entry(target.clone()).or_default().insert(
                    mname.clone(),
                    AssocScopedMethod {
                        params: method.params.clone(),
                        creates: false,
                        queries: false,
                    },
                );
            }
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
    /// (model, association name) -> the association target's CLASS.
    /// A NESTED join (`joins(user: :memberships)`) resolves its second
    /// hop against the first hop's target, and only a ClassId can be
    /// keyed back into these maps — `assoc_table` answers a table name,
    /// which is a dead end for a second lookup.
    assoc_target: HashMap<(ClassId, Symbol), ClassId>,
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
    /// model -> its REAL table name, straight off the ingested `Model`.
    ///
    /// Not `pluralize_snake(class_name)`, which is a second copy of a
    /// rule that has already drifted once: Rails DEMODULIZES
    /// (`Push::Subscription` is `subscriptions`, not
    /// `push::subscriptions`) and prepends a module parent's
    /// `table_name_prefix`, which is why `app/models/push.rb` exists at
    /// all. Ingest reads both; deriving the name a second time here
    /// emitted `DELETE FROM push::subscriptions` — "unrecognized token
    /// ':'" — from `user.push_subscriptions.delete_all`.
    tables: HashMap<ClassId, String>,
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
    /// `(association name, extension method)` for every `has_many :x do
    /// def m … end end` in the app. The model lowerer FLATTENS those onto
    /// the owner as `<assoc>_<method>` instance methods
    /// (`synth_assoc_extension_methods`); this is the call-site half —
    /// `room.memberships.grant_to(users)` -> `room.memberships_grant_to(
    /// users)`.
    ///
    /// Keyed by association name WITHOUT the owner model, because that
    /// is what the call site can always answer: the flattened name is
    /// derived from the association alone, so an owner whose type is
    /// unknown still rewrites correctly. Two models declaring the same
    /// extension method on the same association name agree on the
    /// flattened name by construction; declaring it on DIFFERENT
    /// association names produces different flattened names, which is
    /// also correct.
    assoc_extension: std::collections::HashSet<(Symbol, Symbol)>,
}

impl AssocRegistry {
    /// The table `model` stores in. Falls back to Rails' derivation for
    /// a class that is not an ingested model (a `class_name:` naming
    /// something outside `app/models/`), which is the best guess
    /// available and matches what the emitted `table_name` would say.
    fn table_for(&self, model: &ClassId) -> String {
        match self.tables.get(model) {
            Some(t) => t.clone(),
            None => crate::naming::rails_table_name(model.0.as_str()),
        }
    }
    fn join_tail(&self, model: &ClassId, assoc: &Symbol) -> Option<&String> {
        self.join_tails.get(&(model.clone(), assoc.clone()))
    }
    fn belongs_to_fk(&self, model: &ClassId, assoc: &Symbol) -> Option<&Symbol> {
        self.belongs_to_fk.get(&(model.clone(), assoc.clone()))
    }
    fn assoc_table(&self, model: &ClassId, assoc: &Symbol) -> Option<&Symbol> {
        self.assoc_table.get(&(model.clone(), assoc.clone()))
    }
    fn assoc_target(&self, model: &ClassId, assoc: &Symbol) -> Option<&ClassId> {
        self.assoc_target.get(&(model.clone(), assoc.clone()))
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
    /// See `assoc_extension`: is `<assoc>.<method>` an association
    /// extension the model lowerer flattened onto the owner?
    fn is_assoc_extension(&self, assoc: &Symbol, method: &Symbol) -> bool {
        self.assoc_extension.contains(&(assoc.clone(), method.clone()))
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

/// A bare association read, rewritten to its explicit-`self` form so
/// the owner-shape match below has one case instead of two.
///
/// Only fires with an enclosing model (`instance_self`) that actually
/// declares a has_many by that name — a bare zero-arg call is otherwise
/// just a method call, and turning one into an association read would
/// be guessing.
fn self_qualified_assoc_read(r: Expr, ctx: &Ctx) -> Expr {
    {
        let ExprNode::Send { recv: None, method, args, block, .. } = &*r.node else {
            return r;
        };
        if !args.is_empty() || block.is_some() {
            return r;
        }
        let Some(self_model) = ctx.instance_self.as_ref() else { return r };
        if ctx.assocs.has_many_fk(self_model, method).is_none() {
            return r;
        }
    }
    let mut out = r;
    let span = out.span;
    let ExprNode::Send { method, args, block, parenthesized, .. } = &*out.node else {
        unreachable!()
    };
    let replacement = ExprNode::Send {
        recv: Some(syn(span, ExprNode::SelfRef)),
        method: method.clone(),
        args: args.clone(),
        block: block.clone(),
        parenthesized: *parenthesized,
    };
    *out.node = replacement;
    out
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
        reg.tables.insert(m.name.clone(), m.table.0.as_str().to_string());
    }
    for m in models {
        let own = reg.table_for(&m.name);
        for a in m.associations() {
            match a {
                Association::BelongsTo { name, target, foreign_key, .. } => {
                    let t = reg.table_for(target);
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.id = {own}.{foreign_key}"),
                    );
                    reg.belongs_to_fk
                        .insert((m.name.clone(), name.clone()), foreign_key.clone());
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    reg.assoc_target
                        .insert((m.name.clone(), name.clone()), target.clone());
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
                    extension,
                    ..
                } => {
                    for x in extension {
                        reg.assoc_extension.insert((name.clone(), x.name.clone()));
                    }
                    if as_interface.is_some()
                        || scope.as_ref().is_some_and(|s| !scope_is_row_preserving(s))
                    {
                        reg.has_many_unseedable.insert((m.name.clone(), name.clone()));
                    }
                    let t = reg.table_for(target);
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
                    reg.assoc_target
                        .insert((m.name.clone(), name.clone()), target.clone());
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
                    let t = reg.table_for(target);
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.id"),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    reg.assoc_target
                        .insert((m.name.clone(), name.clone()), target.clone());
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
                    let thr_table = reg.table_for(thr_target);
                    let target_table = reg.table_for(target);
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!(
                            "{thr_table} ON {thr_table}.{thr_fk} = {own}.id \
                             INNER JOIN {target_table} ON {target_table}.id = {thr_table}.{src_fk}"
                        ),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(target_table.as_str()));
                    reg.assoc_target
                        .insert((m.name.clone(), name.clone()), target.clone());
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
///
/// The bulk WRITES join on exactly that test, not by analogy: `Array`
/// answers no `destroy_by`/`delete_by`/`update_all` either, and none of
/// them can degrade a cached read into an N+1 because none of them is a
/// read. campfire's `memberships.destroy_by user: users` is the caller
/// that showed the gate was narrower than its own reasoning.
/// Does this body park an association read on a name and then chain
/// relation surface off that name? The broken-chain twin of
/// [`mentions_assoc_lookup`], which requires the read and the call to
/// be one expression.
///
/// Two passes, because one would have to guess: collect the names bound
/// to a `<owner>.<assoc>` read, then look for one of them as the
/// RECEIVER of something relation-shaped. A body that merely holds an
/// association in a variable and iterates it does not qualify — that
/// case must keep its Array and its preload cache.
///
/// Approximate on purpose, and only in the safe direction: the method
/// test here has no target to resolve scopes against, so it asks the
/// target-free half of the question. `rewrite_send`'s own check is the
/// precise one, and it runs per call site.
pub fn mentions_assoc_alias(expr: &Expr, assocs: &AssocRegistry) -> bool {
    fn bound_names(e: &Expr, assocs: &AssocRegistry, out: &mut HashSet<Symbol>) {
        if let ExprNode::Assign { target, value } = &*e.node {
            let key = match target {
                crate::expr::LValue::Var { name, .. } => Some(alias_key(name, false)),
                crate::expr::LValue::Ivar { name } => Some(alias_key(name, true)),
                _ => None,
            };
            if let Some(key) = key {
                if let ExprNode::Send { recv: Some(_), method: aname, args, block: None, .. } =
                    &*value.node
                {
                    if args.is_empty() && assocs.is_has_many_name(aname) {
                        out.insert(key);
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| bound_names(c, assocs, out));
    }
    fn chains_off(e: &Expr, names: &HashSet<Symbol>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*e.node {
            let key = match &*r.node {
                ExprNode::Var { name, .. } => Some(alias_key(name, false)),
                ExprNode::Ivar { name } => Some(alias_key(name, true)),
                _ => None,
            };
            if let Some(key) = key {
                if names.contains(&key)
                    && (is_relation_chain_method(method.as_str())
                        || is_relation_terminal(method.as_str(), args, block.as_ref())
                        || matches!(method.as_str(), "build" | "create" | "create!"))
                {
                    *found = true;
                    return;
                }
            }
        }
        e.node.for_each_child(&mut |c| chains_off(c, names, found));
    }
    let mut names = HashSet::new();
    bound_names(expr, assocs, &mut names);
    if names.is_empty() {
        return false;
    }
    let mut found = false;
    chains_off(expr, &names, &mut found);
    found
}

pub fn mentions_assoc_lookup(expr: &Expr, assocs: &AssocRegistry) -> bool {
    let mut found = false;
    fn walk(e: &Expr, assocs: &AssocRegistry, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*e.node {
            // `where` joins on the same test the note above states, not
            // by analogy: `Array` answers no `where` either, so the site
            // is a NoMethodError today and a query after. It is the one
            // member of the family that is a CHAIN method rather than a
            // terminal, so it is admitted beside the terminal check
            // instead of through it — with an argument and no block,
            // which is the only spelling that reaches a relation.
            let is_where = method.as_str() == "where" && !args.is_empty() && block.is_none();
            if is_where
                || is_relation_terminal(method.as_str(), args, block.as_ref())
                    && matches!(
                        method.as_str(),
                        "find"
                            | "find_by"
                            | "find_by!"
                            | "destroy_by"
                            | "delete_by"
                            | "destroy_all"
                            | "delete_all"
                            | "update_all"
                    )
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

/// True when `expr` calls an association EXTENSION method through an
/// association read (`room.memberships.grant_to users`). Gate for
/// `rewrite_call_site`, the same reason as its two siblings: such a body
/// may name no scope and start no model chain, so without a gate the
/// rewriter never sees it. `Array` answers none of these names either,
/// so the site is a NoMethodError today and a call after.
pub fn any_assoc_extensions(assocs: &AssocRegistry) -> bool {
    !assocs.assoc_extension.is_empty()
}

pub fn mentions_assoc_extension(expr: &Expr, assocs: &AssocRegistry) -> bool {
    if assocs.assoc_extension.is_empty() {
        return false;
    }
    let mut found = false;
    fn walk(e: &Expr, assocs: &AssocRegistry, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
            if let ExprNode::Send { method: aname, args: aargs, block: None, .. } = &*r.node {
                if aargs.is_empty() && assocs.is_assoc_extension(aname, method) {
                    *found = true;
                    return;
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
            // Chain methods and `all` open a chain; so do the terminals
            // that need a seeded Relation at a Const root. Leaving the
            // latter out is a gate that closes over the very shape
            // `CLASS_ROOT_TERMINALS` exists to rewrite — a body whose
            // ONLY relation surface is `Push::Subscription.destroy_by(…)`
            // never reached the rewriter at all.
            if (is_relation_chain_method(method.as_str())
                || method.as_str() == "all"
                || CLASS_ROOT_TERMINALS.contains(&method.as_str()))
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

/// True when `expr` ends a chain in a terminal that has no home on the
/// model CLASS (`Push::Subscription.destroy_by(…)`) — the
/// [`CLASS_ROOT_TERMINALS`] set, on a model constant.
///
/// A WHOLE-APP gate, separate from `mentions_model_chain_start`'s
/// per-body one, because `apply_scope_lowering` returns early for an app
/// with no scopes, no association-scoped class methods and no
/// association extensions. That early return is right for everything
/// else it guards — those all need a registry to be non-empty — and
/// wrong for these: `destroy_by` on a class reaches nothing whether or
/// not the app declares a single scope.
///
/// Kept to this one set on purpose. Asking the same question about
/// `where`-family chains would make the early return vacuous for
/// essentially every app.
pub fn mentions_class_root_terminal(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
            if CLASS_ROOT_TERMINALS.contains(&method.as_str())
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

/// True when `expr` calls `<Model>.insert_all(rows)`. Gate for the
/// call-site expansion in `rewrite_send`.
pub fn mentions_model_insert_all(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
            if method.as_str() == "insert_all"
                && args.len() == 1
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
            // `without` is `excluding`'s ALIAS, on Relation and on
            // Enumerable both, and Rails apps write whichever reads
            // better at the site. Listing one and not the other is the
            // shape that drifts: campfire's refreshes controller writes
            // `@room.messages.without(@new_messages).with_creator`, and
            // with only `excluding` here the chain stayed on the
            // reader's folded Array — where `without` exists (the AS
            // core_ext) and answers an Array, so the NEXT hop
            // (`with_creator`, a scope) was the one that failed.
            | "without"
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
    /// UNIQUE key column sets per model — what an inlined `insert_all`
    /// must skip on (see [`UniqueKeys`]). Empty where a caller has no
    /// schema to read, which makes the guard vanish and the inline
    /// behave as it did before.
    pub unique_keys: &'a UniqueKeys,
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
    /// The ONE model that declares `name` as a scope — `None` when no
    /// model does, and `None` when two or more do.
    ///
    /// Uniqueness is the same standard `owner_model_from_name` holds
    /// association names to, and for the same reason: a name two models
    /// share names nothing.
    fn sole_scope_owner(&self, name: &Symbol) -> Option<&ClassId> {
        let mut found: Option<&ClassId> = None;
        for (model, declared) in self.scopes.iter() {
            if declared.contains_key(name) {
                if found.is_some() {
                    return None;
                }
                found = Some(model);
            }
        }
        found
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
            unique_keys: self.unique_keys,
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
    //
    // ONLY when the callee declares keywords. Ruby hands `f(a: 1)` to a
    // `def f(h)` as the POSITIONAL hash `h` — which is how campfire's
    // `messages.create_with_attachment!(creator:, attachment:)` reaches
    // its one `attributes` param, and how a `**opts` param (ingested as
    // a positional defaulting to `{}`) is fed. Splitting it off there
    // padded that param with `nil` and pushed the hash PAST `__rel`,
    // handing three positionals to a method taking one or two. Left in
    // place it is an ordinary argument, so it counts toward the padding
    // below — and its `kwargs` flag has to go, or the emitter renders it
    // bare and Ruby reads keywords ahead of the `__rel` positional.
    let takes_keywords = leading.map_or(true, |ps| ps.iter().any(|p| p.keyword));
    let kwargs_tail = match args.last() {
        Some(e) if matches!(&*e.node, ExprNode::Hash { kwargs: true, .. }) => {
            if takes_keywords {
                args.pop()
            } else {
                let tail = args.pop().expect("matched last");
                let ExprNode::Hash { entries, .. } = &*tail.node else { unreachable!() };
                args.push(syn(tail.span, ExprNode::Hash { entries: entries.clone(), kwargs: false }));
                None
            }
        }
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

/// Relation terminals that need a seeded Relation when they sit
/// directly on a model CONST (`Rooms::Open.pluck(:id)` — campfire's
/// `grant_membership_to_open_rooms`). Rails delegates every terminal
/// from the class to `all`; here only these two need it, because the
/// arel pass owns `count` / `exists?` / `find_by` / `all` at a Const
/// root and re-rooting those would move work out from under it —
/// `Comment.count` is an inlined `SELECT COUNT(*)`, not a Relation.
///
/// `pluck` and `ids` have no such home: they were left as a call on
/// the class, which no class answers.
///
/// `destroy_by` / `delete_by` join them for exactly that reason. Both
/// are `where` + a write, the arel pass claims neither at a Const root,
/// and `Base` defines neither — so campfire's
/// `Push::Subscription.destroy_by(endpoint:, user_id:)` reached nothing
/// at all, with the analyzer saying so (`send_dispatch_failed: no known
/// method `destroy_by` on Class { Push::Subscription }`).
const CLASS_ROOT_TERMINALS: &[&str] = &["pluck", "ids", "destroy_by", "delete_by"];

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
        // Bulk WRITES are terminals too — they end the chain and answer
        // a count / an Array of destroyed records, never a relation.
        // `Array` answers none of them, so an association-read receiver
        // is a NoMethodError today and a scoped statement after
        // (campfire's `memberships.destroy_by user: users`).
        "destroy_by" | "delete_by" => block.is_none(),
        _ => matches!(
            name,
            "ids" | "pluck" | "count" | "first" | "last" | "exists?" | "any?" | "empty?" | "size"
                | "length"
                | "destroy_all"
                | "delete_all"
                | "update_all"
                // Reads the relation and, on a miss, WRITES through it —
                // so it is a terminal on both counts. Listing it here is
                // what lets `assoc_scope_shape` see a class method whose
                // body is `find_or_create_by(...)` as one that runs
                // against the caller's scope: campfire's `Search.record`,
                // reached as `user.searches.record(q)`, has to create the
                // row FOR THAT USER. The scope merge itself is the
                // runtime's (`Relation#find_or_create_by`), not
                // `merge_scope_attributes`', because the query half needs
                // the same conditions anyway.
                | "find_or_create_by"
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
/// Is `value` a SEEDABLE association read — the `<owner>.<assoc>`
/// shape the seed arm in `rewrite_send` knows how to turn into
/// `Relation.new(Target).where(fk: <owner>.id)`? Returns the target it
/// resolved to.
///
/// The owner forms and the seedability filter are deliberately the same
/// three the seed arm itself matches on, so a name recorded here is one
/// the arm will accept when the expression is swapped back in. Anything
/// else — an arg-carrying call, a block, an owner that would re-evaluate
/// — is not recorded at all rather than recorded and later declined.
fn seedable_assoc_read(ctx: &Ctx, value: &Expr, span: crate::span::Span) -> Option<ClassId> {
    let ExprNode::Send { recv: Some(owner), method: aname, args, block: None, .. } = &*value.node
    else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let resolved = match &*owner.node {
        ExprNode::SelfRef => ctx.instance_self.clone().and_then(|self_model| {
            if ctx.assocs.is_unseedable(Some(&self_model), aname) {
                return None;
            }
            ctx.assocs.has_many_fk(&self_model, aname).cloned().map(|(target, _)| target)
        }),
        ExprNode::Ivar { .. } | ExprNode::Var { .. } => {
            assoc_owner_seed(ctx, aname, owner, span).map(|(target, _, _)| target)
        }
        ExprNode::Send { .. } if owner_reads_once(owner) => {
            assoc_owner_seed(ctx, aname, owner, span).map(|(target, _, _)| target)
        }
        _ => None,
    };
    resolved
}

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
/// Wrap an inlined `insert_all` row-save in the conflict guard Rails'
/// `ON CONFLICT DO NOTHING` stands for:
///
/// ```text
///   <save>  ->  <save> unless <Model>.where(<key cols>).exists?
/// ```
///
/// One `unless` per unique key, OR'd, so a table with two unique
/// indexes skips a row conflicting on either. A model with no usable
/// unique key (none declared, or every one of them nullable — see
/// [`UniqueKeys`]) keeps the bare save: there is nothing to conflict on
/// that this can read.
fn guard_on_unique_keys(
    span: crate::span::Span,
    model: &ClassId,
    attrs: &Symbol,
    save: Expr,
    ctx: &Ctx<'_>,
) -> Expr {
    let Some(keys) = ctx.unique_keys.get(model) else { return save };
    let mut cond: Option<Expr> = None;
    for key in keys {
        // `<Model>.where(col: __attrs[:col], …).exists?`
        let entries = key
            .iter()
            .map(|col| {
                let k = syn(span, ExprNode::Lit { value: Literal::Sym { value: col.clone() } });
                let v = syn(
                    span,
                    ExprNode::Send {
                        recv: Some(var_expr(span, attrs)),
                        method: Symbol::from("[]"),
                        args: vec![syn(
                            span,
                            ExprNode::Lit { value: Literal::Sym { value: col.clone() } },
                        )],
                        block: None,
                        parenthesized: true,
                    },
                );
                (k, v)
            })
            .collect::<Vec<_>>();
        let where_call = syn(
            span,
            ExprNode::Send {
                recv: Some(relation_new(span, model)),
                method: Symbol::from("where"),
                args: vec![syn(span, ExprNode::Hash { entries, kwargs: true })],
                block: None,
                parenthesized: true,
            },
        );
        let mut exists = syn(
            span,
            ExprNode::Send {
                recv: Some(where_call),
                method: Symbol::from("exists?"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        );
        exists.ty = Some(crate::ty::Ty::Bool);
        cond = Some(match cond {
            None => exists,
            Some(prev) => syn(
                span,
                ExprNode::BoolOp {
                    op: crate::expr::BoolOpKind::Or,
                    surface: crate::expr::BoolOpSurface::Word,
                    left: prev,
                    right: exists,
                },
            ),
        });
    }
    let Some(cond) = cond else { return save };
    // `if !exists?` rather than an If with the branches swapped: the
    // swapped form emits a `nil` then-branch every target has to render,
    // and `!` is the negation spelling the other lowerings already use
    // (exclude_predicate, and_return, typed_store).
    let mut negated = syn(
        span,
        ExprNode::Send {
            recv: Some(cond),
            method: Symbol::from("!"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    negated.ty = Some(crate::ty::Ty::Bool);
    syn(
        span,
        ExprNode::If {
            cond: negated,
            then_branch: save,
            else_branch: syn(span, ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

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
/// `where(col: a..b)` → `where("<table>.col >= ? AND <table>.col <= ?", a, b)`.
///
/// Rails renders a Range condition as a comparison (`>=` / `<=`, `<`
/// for an exclusive end, and either half alone for a beginless or
/// endless range). The runtime `Relation` cannot: `column_predicate`
/// dispatches on Relation / Array / nil / Base and falls through to
/// `col = <value>` for everything else, so a Range compared equal to a
/// column matches nothing — silently, which is the worst kind.
///
/// Converted at the CALL SITE rather than taught to the runtime
/// because there is no `Ty::Range` in the type system and no target
/// ships a Range: a `val.is_a?(Range)` arm in `runtime/ruby` would
/// have to compile on all eleven. Here the range is a literal, so what
/// crosses into the runtime is the raw fragment + binds it already
/// understands (`substitute_binds`).
///
/// Returns `None` for anything it will not convert — a multi-key hash
/// (the fragment would have to compose the other predicates), a
/// non-Symbol key, or a range with neither end.
fn range_condition_fragment(
    model: &ClassId,
    arg: &Expr,
    ctx: &Ctx,
) -> Option<(Expr, Vec<Expr>)> {
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let [(k, v)] = &entries[..] else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
    // The registry's table, not a second derivation of it — see
    // `AssocRegistry::tables`.
    let table = ctx.assocs.table_for(model);
    let col = format!("{table}.{key}");
    let mut binds: Vec<Expr> = Vec::new();
    let sql = match &*v.node {
        ExprNode::Range { .. } => range_clause(&col, v, &mut binds)?,
        // `where(connected_at: [ nil, ...CONNECTION_TTL.ago ])` — an
        // ARRAY of alternatives, which Rails ORs together, splitting
        // `nil` out as `IS NULL`. The runtime's `column_predicate`
        // renders an Array as `IN (…)`, so the Range inside one is the
        // silent no-match of the bare-Range case wearing a different
        // hat: campfire's `Membership.disconnected` selected the rows
        // whose `connected_at` equalled the literal string "nil" or a
        // Range object, i.e. none.
        //
        // Only `nil` and Range members are claimed. A mixed array with
        // scalars would need `IN (…)` composed alongside the OR arms,
        // and nothing in the corpus writes one.
        ExprNode::Array { elements, .. } => {
            let mut arms: Vec<String> = Vec::new();
            for el in elements {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Nil } => {
                        arms.push(format!("{col} IS NULL"));
                    }
                    ExprNode::Range { .. } => arms.push(range_clause(&col, el, &mut binds)?),
                    _ => return None,
                }
            }
            if arms.len() < 2 {
                return None;
            }
            // Parenthesized: the runtime ANDs this fragment with every
            // other condition on the relation, and an unwrapped OR
            // would bind looser than that AND.
            format!("({})", arms.join(" OR "))
        }
        _ => return None,
    };
    let fragment = Expr::new(arg.span, ExprNode::Lit { value: Literal::Str { value: sql } });
    Some((fragment, binds))
}

/// One Range's comparison SQL, appending its bound expressions to
/// `binds` in the order the `?` placeholders appear.
///
/// Rails renders a Range as `>=` / `<=`, `<` for an exclusive end, and
/// either half alone for a beginless or endless one. A range with
/// NEITHER end (`nil..nil`) has no comparison to make and declines.
fn range_clause(col: &str, e: &Expr, binds: &mut Vec<Expr>) -> Option<String> {
    let ExprNode::Range { begin, end, exclusive } = &*e.node else { return None };
    let mut clauses: Vec<String> = Vec::new();
    if let Some(b) = begin {
        clauses.push(format!("{col} >= ?"));
        binds.push(b.clone());
    }
    if let Some(x) = end {
        // `a...b` excludes its end; `a..b` includes it.
        clauses.push(format!("{col} {} ?", if *exclusive { "<" } else { "<=" }));
        binds.push(x.clone());
    }
    if clauses.is_empty() {
        return None;
    }
    // Both halves present: parenthesize so the pair survives being
    // ORed with a sibling arm.
    Some(if clauses.len() == 1 {
        clauses.remove(0)
    } else {
        format!("({})", clauses.join(" AND "))
    })
}

/// One `joins` argument as SQL, or None when any hop is unresolvable.
///
/// A Symbol is one hop off `model`. A HASH is Rails' NESTED form —
/// `Push::Subscription.joins(user: :memberships)` joins users off the
/// subscription and then memberships off the USER, which is why the
/// registry has to answer a target CLASS and not just a table. Values
/// nest the same three ways Rails allows (a symbol, an array of them,
/// another hash), so the recursion mirrors the spec's own shape.
///
/// ALL-OR-NOTHING per argument: a spec whose second hop has no entry
/// (an STI subclass, a habtm, an unresolvable `through`) answers None
/// and the argument keeps its source shape, which the runtime's
/// `join_fragment` turns into a raise naming the association. Half a
/// join would silently answer the wrong rows — the same standard the
/// single-hop form has always held.
fn join_spec_sql(model: &ClassId, spec: &Expr, kind: &str, ctx: &Ctx) -> Option<String> {
    match &*spec.node {
        ExprNode::Lit { value: Literal::Sym { value } } => {
            Some(format!("{kind} {}", ctx.assocs.join_tail(model, value)?))
        }
        ExprNode::Hash { entries, .. } if !entries.is_empty() => {
            let mut out = String::new();
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
                    return None;
                };
                let head = ctx.assocs.join_tail(model, name)?;
                let next = ctx.assocs.assoc_target(model, name)?.clone();
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&format!("{kind} {head}"));
                out.push(' ');
                out.push_str(&join_spec_sql(&next, v, kind, ctx)?);
            }
            Some(out)
        }
        ExprNode::Array { elements, .. } if !elements.is_empty() => {
            let mut out = String::new();
            for el in elements {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&join_spec_sql(model, el, kind, ctx)?);
            }
            Some(out)
        }
        _ => None,
    }
}

fn lower_relation_args(
    model: &ClassId,
    method: &Symbol,
    args: &mut Vec<Expr>,
    ctx: &Ctx,
) -> Vec<Symbol> {
    let mut aliases: Vec<Symbol> = Vec::new();
    match method.as_str() {
        "joins" | "left_outer_joins" => {
            let kind = if method.as_str() == "joins" { "INNER JOIN" } else { "LEFT OUTER JOIN" };
            for a in args {
                if let Some(sql) = join_spec_sql(model, a, kind, ctx) {
                    *a.node = ExprNode::Lit { value: Literal::Str { value: sql } };
                }
            }
        }
        // Every method taking a CONDITION HASH, not just the three the
        // corpus reached first. `destroy_by`/`delete_by` are `where`'s
        // arguments with a terminal attached, and `find_by!`/`exists?`
        // are `find_by`'s — a list that names some of them renames
        // `user:` to `user_id:` on one spelling and emits the
        // nonexistent `memberships.user` column on the next.
        "where" | "not" | "find_by" | "find_by!" | "destroy_by" | "delete_by" | "exists?" => {
            // `where(connected_at: TTL.ago..)` — a RANGE value. Rails
            // renders `>=` / `<=` / `<` / BETWEEN; the runtime's
            // `column_predicate` has no Range arm and falls through to
            // `col = <the range object>`, which matches nothing and
            // says nothing. Converted HERE, where the range is a
            // literal, because the alternative is teaching every
            // target's runtime what a Range is — there is no `Ty::Range`
            // and no target ships one.
            //
            // Single-entry hashes only: a mixed hash would have to
            // compose the fragment with the other keys' predicates, and
            // half a conversion is worse than none (the multi-key form
            // keeps its current, honest, matches-nothing behavior until
            // a corpus app writes one).
            if matches!(method.as_str(), "where" | "not" | "find_by" | "find_by!")
                && args.len() == 1
            {
                if let Some((fragment, binds)) = range_condition_fragment(model, &args[0], ctx) {
                    args[0] = fragment;
                    args.extend(binds);
                    return aliases;
                }
            }
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

/// What the names in scope stand for, accumulated as a method body's
/// statements are processed in order.
#[derive(Default)]
pub(crate) struct Locals {
    /// Local variable -> relation model, so `q = Story.base(u);
    /// q.not_deleted` resolves `not_deleted` against `q`'s Story
    /// relation.
    rel: HashMap<Symbol, ClassId>,
    /// Variable OR IVAR -> the association read it was assigned from,
    /// as the `(target, fk, owner-id)` triple the seed arm below wants.
    ///
    /// The seed arm matches `<owner>.<assoc>.<method>` as ONE
    /// expression. Rails code routinely breaks that chain over an
    /// assignment — campfire's push-subscriptions controller opens
    /// every action with `@push_subscriptions = Current.user
    /// .push_subscriptions` and then calls `.find_by` / `.create!` /
    /// `.destroy_by` on the ivar — and the association read alone
    /// arel-folds to an eager Array, so those calls reached
    /// `undefined method 'find_by' for an instance of Array`. Recording
    /// what the name was assigned FROM lets the same seed fire on the
    /// later read.
    ///
    /// Locals and ivars share this map, so the key is built by
    /// [`alias_key`] rather than being the bare Symbol: ingest strips
    /// the `@` off an ivar name, which would let `@room` and a local
    /// `room` overwrite each other.
    ///
    /// The value is the association-read expression itself plus the
    /// target it resolved to. Storing the EXPRESSION (rather than the
    /// seed triple) is what keeps this cheap: the later read swaps it
    /// back in as the receiver and the existing seed arm then handles
    /// the call verbatim, one code path for both spellings.
    assoc: HashMap<Symbol, (ClassId, Expr)>,
}

/// Alias-map key for a name. Ivars and locals share one map, so the
/// ivar spelling is prefixed — `@room` and a local `room` are different
/// bindings and must not overwrite each other.
fn alias_key(name: &Symbol, ivar: bool) -> Symbol {
    if ivar { Symbol::from(format!("@{}", name.as_str())) } else { name.clone() }
}

/// Rewrite scope chains in `expr` (in place). Returns the relation-model of
/// the whole expression when it evaluates to a Relation of a known model.
pub(crate) fn rewrite(expr: &mut Expr, ctx: &Ctx, locals: &mut Locals) -> Option<ClassId> {
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
        // `name = value`: record what the name now stands for — a
        // relation model, or the association read it was assigned from.
        ExprNode::Assign { .. } => {
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Assign { target, mut value } = node else { unreachable!() };
            let m = rewrite(&mut value, ctx, locals);
            if let crate::expr::LValue::Var { name, .. } = &target {
                match &m {
                    Some(model) => {
                        locals.rel.insert(name.clone(), model.clone());
                    }
                    None => {
                        locals.rel.remove(name);
                    }
                }
            }
            // Association alias, for both spellings. ALWAYS write or
            // clear: a name reassigned to something that is not an
            // association read must lose the old binding, or a later
            // call would seed off a relation the name no longer holds.
            if let Some(key) = match &target {
                crate::expr::LValue::Var { name, .. } => Some(alias_key(name, false)),
                crate::expr::LValue::Ivar { name } => Some(alias_key(name, true)),
                _ => None,
            } {
                match seedable_assoc_read(ctx, &value, expr.span) {
                    Some(model) => {
                        locals.assoc.insert(key, (model, value.clone()));
                    }
                    None => {
                        locals.assoc.remove(&key);
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
        Some(r) => {
            // Class-level call on a model constant.
            if let Some(m) = const_model(&r, ctx.models) {
                // `<Model>.insert_all(rows)` — Rails' bulk insert over an
                // Array of attribute Hashes. INLINED at the call site
                // rather than synthesized as a per-model method: an
                // untyped attribute Hash is exactly the shape the
                // has_json work established no target's Hash surface
                // resolves portably, and a synthesized `insert_all` would
                // land on every model of every app to serve the handful
                // that call it.
                //
                //   Membership.insert_all(rows)
                //     -> rows.each { |__attrs|
                //          Membership.new(__attrs).save_after_validation }
                //
                // `save_after_validation` is the seam Rails' own
                // validation-skipping writes already enter at, so this
                // reuses one definition of "write without validating"
                // instead of adding a second. The divergence it leaves —
                // Rails skips the SAVE callbacks too and issues ONE
                // multi-row INSERT — is recorded in
                // docs/pipeline/runtime.md.
                //
                // CONFLICTS ARE SKIPPED, because that is what `insert_all`
                // MEANS. Measured against ActiveRecord 8.1.3: `insert_all`
                // renders `INSERT … ON CONFLICT DO NOTHING`, so inserting a
                // row that already exists is a silent no-op; only
                // `insert_all!` raises. A bare per-row save raises, i.e. it
                // implements `insert_all!` under the other name — which is
                // how campfire's `revise` (re-granting a membership to a
                // user who already has one) died on a UNIQUE index where
                // Rails does nothing at all. So each row is guarded by an
                // existence check on the table's unique keys:
                //
                //   rows.each { |__attrs|
                //     Membership.new(__attrs).save_after_validation unless
                //       Relation.new(Membership)
                //         .where(room_id: __attrs[:room_id],
                //                user_id: __attrs[:user_id]).exists? }
                //
                // A pre-check, not the database's atomic DO NOTHING: it is
                // the same read-then-write shape `increment!` already
                // carries, and under single-threaded dispatch the window
                // it opens is not observable. Ledgered with the rest.
                if method.as_str() == "insert_all" && args.len() == 1 && block.is_none() {
                    let attrs = Symbol::from("__attrs");
                    let build = syn(
                        span,
                        ExprNode::Send {
                            recv: Some(const_expr(span, &m)),
                            method: Symbol::from("new"),
                            args: vec![var_expr(span, &attrs)],
                            block: None,
                            parenthesized: true,
                        },
                    );
                    let save = syn(
                        span,
                        ExprNode::Send {
                            recv: Some(build),
                            method: Symbol::from("save_after_validation"),
                            args: vec![],
                            block: None,
                            parenthesized: true,
                        },
                    );
                    let save = guard_on_unique_keys(span, &m, &attrs, save, ctx);
                    let mut args = args;
                    *expr = syn(
                        span,
                        ExprNode::Send {
                            recv: Some(args.remove(0)),
                            method: Symbol::from("each"),
                            args: vec![],
                            block: Some(syn(
                                span,
                                ExprNode::Lambda {
                                    params: vec![attrs],
                                    block_param: None,
                                    body: save,
                                    block_style: BlockStyle::Brace,
                                },
                            )),
                            parenthesized: false,
                        },
                    );
                    return None;
                }
                if ctx.scope_of(&m, &method) {
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(m);
                }
                if is_relation_chain_method(method.as_str())
                    || method.as_str() == "all"
                    || CLASS_ROOT_TERMINALS.contains(&method.as_str())
                {
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
            //
            // A BARE association read (`bans.create!(…)` — implicit
            // self) is spelled `Send { recv: None }` and reads as the
            // same association `self.bans` does, so it is normalized to
            // the SelfRef form before the match below rather than given
            // a parallel arm. Model bodies write both spellings and
            // concerns write the bare one almost exclusively;
            // campfire's `User::Bannable#create_bans_from_sessions` is
            // `bans.create!(ip_address: ip)` and kept the reader's
            // folded Array until this normalization existed.
            let mut r = self_qualified_assoc_read(r, ctx);
            // ...and the same read, one ASSIGNMENT later. Rails code
            // routinely parks an association on a name and chains off
            // that: campfire's push-subscriptions controller opens
            // every action with `@push_subscriptions = Current.user
            // .push_subscriptions` and then calls `.find_by`,
            // `.create!`, `.destroy_by` on the ivar. The read alone
            // arel-folds to an eager Array, so those reached
            // `undefined method 'find_by' for an instance of Array`.
            //
            // Swap the recorded read back in as the receiver and the
            // arm below seeds it exactly as if the chain had never been
            // broken. Gated on relation surface actually FOLLOWING, for
            // the same reason the arm itself is: plain iteration
            // (`each`/`map`) must keep the Array and the reader's
            // preload cache, and re-querying there would drop both.
            if let Some((target, read)) = match &*r.node {
                ExprNode::Var { name, .. } => locals.assoc.get(&alias_key(name, false)),
                ExprNode::Ivar { name } => locals.assoc.get(&alias_key(name, true)),
                _ => None,
            } {
                if ctx.scope_of(target, &method)
                    || is_relation_chain_method(method.as_str())
                    || is_relation_terminal(method.as_str(), &args, block.as_ref())
                    || ctx.assoc_class_method_params(target, &method).is_some()
                    // CollectionProxy constructors too: `@subscriptions
                    // .create!(attrs)` wants the association's foreign
                    // key preset exactly as `user.subscriptions
                    // .create!(attrs)` does. They are not relation
                    // surface — the arm below answers them with a
                    // record — but they are the same association hop,
                    // and `mentions_assoc_alias` already counts them.
                    || matches!(method.as_str(), "build" | "create" | "create!")
                {
                    r = read.clone();
                }
            }
            if let ExprNode::Send { recv: Some(ir), method: aname, args: aargs, .. } = &*r.node {
                // Association EXTENSION (`has_many :memberships do def
                // grant_to … end end`): the model lowerer flattened it
                // onto the owner as `<assoc>_<method>`, so the call
                // drops the association hop and keeps the owner as
                // receiver. Ahead of the seed arm and independent of it
                // — an extension call wants the owner RECORD, not a
                // relation, so seedability (`as:`, a row-changing scope)
                // has no bearing on it.
                if aargs.is_empty() && ctx.assocs.is_assoc_extension(aname, &method) {
                    let flat = Symbol::from(format!(
                        "{}_{}",
                        aname.as_str(),
                        method.as_str()
                    ));
                    // `self.<assoc>.<m>` inside the owner's own body
                    // flattens to a bare implicit-self call.
                    let new_recv = match &*ir.node {
                        ExprNode::SelfRef => None,
                        _ => Some(ir.clone()),
                    };
                    *expr = put(span, new_recv, flat, args, block, parenthesized);
                    // Returns whatever the extension body returns — a
                    // record, a count, nil. Never a relation.
                    return None;
                }
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
                                    // `<anything>.merge(k: v)` — the
                                    // foreign key joins the LITERAL the
                                    // merge already carries. The value
                                    // being merged INTO is opaque here
                                    // and stays that way; what makes
                                    // this safe is that `merge` with a
                                    // kwargs literal is a Hash by
                                    // construction, whichever way the
                                    // receiver went. campfire's
                                    // `subscriptions.create!(params
                                    // .to_attrs.merge(user_agent: …))`
                                    // is the shape, and without this it
                                    // fell into the `Blocked` case that
                                    // declines a bare parameter.
                                    ExprNode::Send { recv: Some(r), method: m, args: margs, block: None, .. }
                                        if m.as_str() == "merge" && margs.len() == 1 =>
                                    {
                                        match &*margs[0].node {
                                            ExprNode::Hash { entries, kwargs: true } => {
                                                let mut entries = entries.clone();
                                                entries.push(fk_entry);
                                                Some(vec![syn(
                                                    span,
                                                    ExprNode::Send {
                                                        recv: Some(r.clone()),
                                                        method: m.clone(),
                                                        args: vec![syn(
                                                            span,
                                                            ExprNode::Hash {
                                                                entries,
                                                                kwargs: true,
                                                            },
                                                        )],
                                                        block: None,
                                                        parenthesized: true,
                                                    },
                                                )])
                                            }
                                            _ => None,
                                        }
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
                            //
                            // Both take the same condition-hash lowering the
                            // Const arm applies (`user: user` -> `user_id:
                            // user && user.id`, association joins). It is the
                            // TARGET's associations that resolve here — the
                            // hash keys conditions on the seeded relation, not
                            // on the owner. Skipping it emitted `WHERE
                            // memberships.user = …` for campfire's
                            // `memberships.destroy_by user: users`, a column
                            // that does not exist; the Const arm has always
                            // called this and the seed arm never did.
                            let mut args = args;
                            let _ = lower_relation_args(&target, &method, &mut args, ctx);
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
                ExprNode::Var { name, .. } => locals.rel.get(name).cloned(),
                _ => rewrite(&mut r, ctx, locals),
            };
            let r_model = r_model.or_else(|| match &*r.node {
                ExprNode::Send { method: rname, .. } => {
                    ctx.user_returns.get(rname).cloned().flatten()
                }
                _ => None,
            });

            if let Some(mr) = r_model {
                // `<relation>.new(…)` — Rails builds a record THROUGH a
                // relation (`User.active_bots.new`), seeded from the
                // relation's create-scope. There is no `Relation#new`
                // and there cannot be one: under spinel a class's
                // constructor is already `sp_Relation_new`, so an
                // instance method of that name is a duplicate C symbol
                // and the whole program stops compiling (relation.rb
                // says so at the hole it leaves, and
                // docs/pipeline/runtime.md ledgers it). The association
                // form is already served by `merge_scope_attributes`
                // inside a threaded class-method body; this is the SCOPE
                // form, and it is the same rewrite one layer out —
                //
                //   User.active_bots.new          ->  User.new(__r.scope_attributes)
                //   room.memberships.new(attrs)   ->  Membership.new(__r.scope_attributes.merge(attrs))
                //
                // with the caller's own attributes on the OUTSIDE of the
                // merge, because Rails assigns them after the scope's.
                // The receiver stays where it is: a relation is lazy, so
                // evaluating it to read `scope_attributes` runs no query.
                //
                // Same argument shapes `merge_scope_attributes` admits —
                // absent, a literal Hash, or a bare variable. Anything
                // else (a positional id, a splat) is left alone rather
                // than merged against a value whose shape is unknown
                // here.
                if method.as_str() == "new" && block.is_none() {
                    let arg_is_attrs = args.len() == 1
                        && matches!(
                            &*args[0].node,
                            ExprNode::Hash { .. } | ExprNode::Var { .. }
                        );
                    if args.is_empty() || arg_is_attrs {
                        let scope_attrs = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(r),
                                method: Symbol::from("scope_attributes"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        );
                        let seed = if args.is_empty() {
                            scope_attrs
                        } else {
                            syn(
                                span,
                                ExprNode::Send {
                                    recv: Some(scope_attrs),
                                    method: Symbol::from("merge"),
                                    args: vec![args.remove(0)],
                                    block: None,
                                    parenthesized: true,
                                },
                            )
                        };
                        *expr = put(
                            span,
                            Some(const_expr(span, &mr)),
                            method,
                            vec![seed],
                            None,
                            true,
                        );
                        return None;
                    }
                }
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

            // `first(n)` / `last(n)` on a receiver whose own type never
            // resolved, but whose OUTERMOST call names a scope.
            //
            // A scope returns a relation by construction — that is what
            // a scope IS — so the receiver is one whatever the chain
            // below it typed as. campfire's search page reads
            // `Current.user.reachable_messages.search(query).last(100)`,
            // where `Current.user` is untyped at harvest (an ivar on a
            // lowered CurrentAttributes class) and takes the whole chain
            // with it. At RUN TIME every link answers a real Relation;
            // only this rename was missing, so the call landed on the
            // runtime's zero-arg `last` — `wrong number of arguments
            // (given 1, expected 0)` in three of that file's five tests.
            //
            // The guard is `sole_scope_owner`: a name TWO models declare
            // names nothing, and a receiver-blind rename would corrupt
            // `Array#first(n)` (lobsters' `split.first(words * 2)`),
            // which is the hazard `counted_terminal`'s own note names.
            if let Some(counted) = counted_terminal(&method, &args, block.as_ref()) {
                let names_a_scope = matches!(&*r.node, ExprNode::Send { method: rname, .. }
                    if ctx.sole_scope_owner(rname).is_some());
                if names_a_scope {
                    *expr = put(span, Some(r), counted, args, block, parenthesized);
                    return None;
                }
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
    // A scope body's `insert_all` (none in the corpus) keeps the
    // unguarded inline: this entry point takes registries, not the app,
    // so there is no schema here to read a conflict target from.
    let empty_unique = UniqueKeys::new();
    let ctx = Ctx {
        scopes,
        models,
        assocs,
        unique_keys: &empty_unique,
        assoc_class_methods: &empty_assoc_cm,
        scope_body: Some((self_model.clone(), rel_param.clone())),
        self_const_is_implicit,
        class_self: None,
        instance_self: None,
        user_returns: &empty_returns,
    };
    let mut locals = Locals::default();
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
        unique_keys: regs.unique_keys,
        assoc_class_methods: regs.assoc_class_methods,
        scope_body: None,
        self_const_is_implicit: false,
        class_self: class_self.cloned(),
        instance_self: instance_self.cloned(),
        user_returns: regs.user_returns,
    };
    let mut locals = Locals::default();
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
    pub unique_keys: &'a UniqueKeys,
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
    fn thread_rel_keeps_a_kwargs_tail_positional_when_the_callee_takes_no_keywords() {
        // `create_with_attachment!(attributes)` called as
        // `create_with_attachment!(creator: u, attachment: f)` — Ruby
        // binds the hash to `attributes`, so it stays a positional
        // BEFORE `__rel` (and loses its bare-kwargs rendering), rather
        // than padding `attributes` with nil and landing after.
        let leading = vec![Param::positional(Symbol::from("attributes"))];
        let kw = Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(int_lit(1), int_lit(2))], kwargs: true },
        );
        let out = thread_rel(vec![kw], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(matches!(&*out[0].node, ExprNode::Hash { kwargs: false, .. }));
        assert!(is_rel(&out[1]));
    }

    #[test]
    fn thread_rel_splits_a_kwargs_tail_when_the_callee_declares_keywords() {
        // `base(user, unmerged: false)` — the hash binds a KEYWORD param,
        // so the relation goes before it and the bare rendering stays.
        let leading = vec![
            Param::positional(Symbol::from("user")),
            Param::keyword(Symbol::from("unmerged"), None),
        ];
        let user = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("user") });
        let kw = Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(int_lit(1), int_lit(2))], kwargs: true },
        );
        let out = thread_rel(vec![user, kw], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 3);
        assert!(is_rel(&out[1]));
        assert!(matches!(&*out[2].node, ExprNode::Hash { kwargs: true, .. }));
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
        reg.assoc_target
            .insert((story(), Symbol::from("hidings")), ClassId(Symbol::from("HiddenStory")));
        reg.belongs_to_fk.insert(
            (ClassId(Symbol::from("HiddenStory")), Symbol::from("user")),
            Symbol::from("user_id"),
        );
        reg.join_tails.insert(
            (ClassId(Symbol::from("HiddenStory")), Symbol::from("user")),
            "users ON users.id = hidden_stories.user_id".to_string(),
        );
        reg.assoc_target.insert(
            (ClassId(Symbol::from("HiddenStory")), Symbol::from("user")),
            ClassId(Symbol::from("User")),
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

    fn empty_unique_keys() -> &'static UniqueKeys {
        static EMPTY: std::sync::OnceLock<UniqueKeys> = std::sync::OnceLock::new();
        EMPTY.get_or_init(UniqueKeys::new)
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
            unique_keys: empty_unique_keys(),
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
            unique_keys: empty_unique_keys(),
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

    fn hash_arg(entries: Vec<(Expr, Expr)>) -> Expr {
        Expr::new(span(), ExprNode::Hash { entries, kwargs: true })
    }

    /// Rails' NESTED form: `joins(hidings: :user)` joins hidden_stories
    /// off the story and then users off the HIDDEN STORY. The second hop
    /// is resolved against the first's target class, which is the fact
    /// `assoc_target` exists to answer.
    #[test]
    fn joins_nested_hash_expands_both_hops() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![hash_arg(vec![(sym_lit("hidings"), sym_lit("user"))])];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str, got {:?}", args[0].node)
        };
        assert_eq!(
            value,
            "INNER JOIN hidden_stories ON hidden_stories.story_id = stories.id \
             INNER JOIN users ON users.id = hidden_stories.user_id"
        );
    }

    /// Every hop takes the call's own join kind.
    #[test]
    fn left_outer_joins_nested_hash_is_outer_at_every_hop() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![hash_arg(vec![(sym_lit("hidings"), sym_lit("user"))])];
        lower_relation_args(&story(), &Symbol::from("left_outer_joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str")
        };
        assert_eq!(value.matches("LEFT OUTER JOIN").count(), 2, "{value}");
        assert!(!value.contains("INNER JOIN"), "{value}");
    }

    /// ALL-OR-NOTHING: an unresolvable SECOND hop leaves the whole
    /// argument alone, so the runtime raises and names the association
    /// rather than answering a half-joined row set.
    #[test]
    fn joins_nested_hash_with_an_unknown_inner_hop_is_left_untouched() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![hash_arg(vec![(sym_lit("hidings"), sym_lit("taggings"))])];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        assert!(matches!(&*args[0].node, ExprNode::Hash { .. }), "{:?}", args[0].node);
    }

    /// `joins(hidings: [:user])` — an array value is several hops off
    /// the same parent.
    #[test]
    fn joins_nested_array_value_expands_each_hop() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let arr = Expr::new(
            span(),
            ExprNode::Array { elements: vec![sym_lit("user")], style: Default::default() },
        );
        let mut args = vec![hash_arg(vec![(sym_lit("hidings"), arr)])];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str, got {:?}", args[0].node)
        };
        assert!(value.ends_with("INNER JOIN users ON users.id = hidden_stories.user_id"), "{value}");
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

    // ---- where(col: <range>) ------------------------------------------

    fn range(begin_: Option<Expr>, end_: Option<Expr>, exclusive: bool) -> Expr {
        Expr::new(span(), ExprNode::Range { begin: begin_, end: end_, exclusive })
    }

    fn cond_hash(key: &str, value: Expr) -> Expr {
        Expr::new(
            span(),
            ExprNode::Hash {
                entries: vec![(
                    Expr::new(
                        span(),
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(key) } },
                    ),
                    value,
                )],
                kwargs: true,
            },
        )
    }

    fn cutoff(name: &str) -> Expr {
        Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
    }

    /// A `Ctx` with EMPTY registries, which is what these fragment
    /// tests want: with no ingested model behind the name,
    /// `table_for` falls back to Rails' own derivation, so the
    /// expected SQL is the same one a real `Membership` produces and
    /// the test does not have to build a whole app to say so.
    fn fragment_ctx<'a>(
        scopes: &'a ScopeRegistry,
        models: &'a HashSet<ClassId>,
        assocs: &'a AssocRegistry,
    ) -> Ctx<'a> {
        Ctx {
            scopes,
            models,
            assocs,
            unique_keys: empty_unique_keys(),
            assoc_class_methods: empty_assoc_cm(),
            scope_body: None,
            self_const_is_implicit: false,
            class_self: None,
            user_returns: empty_returns(),
            instance_self: None,
        }
    }

    fn fragment_sql(f: &Expr) -> &str {
        match &*f.node {
            ExprNode::Lit { value: Literal::Str { value } } => value.as_str(),
            other => panic!("expected a SQL string literal, got {other:?}"),
        }
    }

    /// A Range value compared for EQUALITY matched nothing, silently —
    /// `column_predicate` has no Range arm and falls through to
    /// `col = <the range object>`. Both ends, one end, and neither.
    #[test]
    fn a_range_condition_becomes_comparisons() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::new(), HashSet::new(), AssocRegistry::default());
        let ctx = fragment_ctx(&sc, &md, &ac);

        let (sql, binds) = range_condition_fragment(
            &m,
            &cond_hash("connected_at", range(Some(cutoff("a")), Some(cutoff("b")), false)),
            &ctx,
        )
        .expect("both ends");
        assert_eq!(
            fragment_sql(&sql),
            "(memberships.connected_at >= ? AND memberships.connected_at <= ?)"
        );
        assert_eq!(binds.len(), 2);

        // `a...b` excludes its end.
        let (sql, _) = range_condition_fragment(
            &m,
            &cond_hash("connected_at", range(Some(cutoff("a")), Some(cutoff("b")), true)),
            &ctx,
        )
        .expect("exclusive end");
        assert!(fragment_sql(&sql).contains("< ?"), "{}", fragment_sql(&sql));

        // Endless — the half that `Membership.connected` writes.
        let (sql, binds) = range_condition_fragment(
            &m,
            &cond_hash("connected_at", range(Some(cutoff("a")), None, false)),
            &ctx,
        )
        .expect("endless");
        assert_eq!(fragment_sql(&sql), "memberships.connected_at >= ?");
        assert_eq!(binds.len(), 1);

        // Neither end has no comparison to make.
        assert!(
            range_condition_fragment(&m, &cond_hash("connected_at", range(None, None, false)), &ctx)
                .is_none()
        );
    }

    /// `where(connected_at: [ nil, ...cutoff ])` — Rails ORs the
    /// alternatives and splits `nil` out as `IS NULL`. The runtime
    /// renders an Array as `IN (…)`, so this is the same silent
    /// no-match wearing a different hat (campfire's
    /// `Membership.disconnected`).
    #[test]
    fn an_array_of_alternatives_becomes_an_or() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::new(), HashSet::new(), AssocRegistry::default());
        let ctx = fragment_ctx(&sc, &md, &ac);
        let value = Expr::new(
            span(),
            ExprNode::Array {
                elements: vec![
                    Expr::new(span(), ExprNode::Lit { value: Literal::Nil }),
                    range(None, Some(cutoff("cutoff")), true),
                ],
                style: Default::default(),
            },
        );
        let (sql, binds) = range_condition_fragment(&m, &cond_hash("connected_at", value), &ctx)
            .expect("nil + range");
        assert_eq!(
            fragment_sql(&sql),
            "(memberships.connected_at IS NULL OR memberships.connected_at < ?)"
        );
        assert_eq!(binds.len(), 1);
    }

    /// A member this rewrite does not reproduce declines the whole
    /// condition rather than composing half of it: a scalar would need
    /// `IN (…)` alongside the OR arms.
    #[test]
    fn an_array_with_a_scalar_member_declines() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::new(), HashSet::new(), AssocRegistry::default());
        let ctx = fragment_ctx(&sc, &md, &ac);
        let value = Expr::new(
            span(),
            ExprNode::Array {
                elements: vec![
                    Expr::new(span(), ExprNode::Lit { value: Literal::Nil }),
                    Expr::new(span(), ExprNode::Lit { value: Literal::Int { value: 3 } }),
                ],
                style: Default::default(),
            },
        );
        assert!(range_condition_fragment(&m, &cond_hash("connected_at", value), &ctx).is_none());
    }

    // ---- build_assoc_registry: has_many :through ----------------------

    fn ingest(src: &str, path: &str) -> crate::dialect::Model {
        crate::ingest::ingest_model(
            src.as_bytes(),
            path,
            &crate::schema::Schema::default(),
            &Default::default(),
        )
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

    /// A BARE association read is the same read `self.<assoc>` is —
    /// the spelling a model CONCERN uses almost exclusively.
    /// `bans.create!(ip_address: ip)` in `User::Bannable` kept the
    /// reader's folded Array until `self_qualified_assoc_read`
    /// normalized it.
    #[test]
    fn bare_assoc_constructor_resolves_through_instance_self() {
        let user = ingest(
            "class User < ApplicationRecord\n  has_many :bans\nend\n",
            "app/models/user.rb",
        );
        let ban = ingest(
            "class Ban < ApplicationRecord\n  belongs_to :user\nend\n",
            "app/models/ban.rb",
        );
        let models_v = vec![user, ban];
        let scopes = build_scope_registry(&models_v);
        let models = model_set(&models_v);
        let assocs = build_assoc_registry(&models_v);
        let bare_read = Expr::new(
            span(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("bans"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let mut expr = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(bare_read),
                method: Symbol::from("create!"),
                args: vec![Expr::new(
                    span(),
                    ExprNode::Hash {
                        entries: vec![(
                            Expr::new(
                                span(),
                                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("ip_address") } },
                            ),
                            Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from("ip") }),
                        )],
                        kwargs: true,
                    },
                )],
                block: None,
                parenthesized: true,
            },
        );
        let owner = ClassId(Symbol::from("User"));
        rewrite_call_site(&mut expr, &regs(&scopes, &models, &assocs), None, Some(&owner));

        // `Ban.create!(ip_address: ip, user_id: @id)`
        let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node else {
            panic!("expected Send, got {:?}", expr.node);
        };
        assert_eq!(method.as_str(), "create!");
        assert!(
            matches!(&*r.node, ExprNode::Const { path } if path.last().unwrap().as_str() == "Ban"),
            "receiver should be the target model, got {:?}",
            r.node
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else {
            panic!("expected a kwargs hash, got {:?}", args[0].node);
        };
        assert_eq!(entries.len(), 2, "caller attrs plus the association's foreign key");
        assert!(
            matches!(&*entries[1].0.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user_id"),
            "the appended key should be the foreign key, got {:?}",
            entries[1].0.node
        );
    }

    /// A bare zero-arg call that is NOT an association stays a plain
    /// method call — the normalization must not guess.
    #[test]
    fn a_bare_non_assoc_read_is_left_alone() {
        let user = ingest(
            "class User < ApplicationRecord\n  has_many :bans\nend\n",
            "app/models/user.rb",
        );
        let models_v = vec![user];
        let scopes = build_scope_registry(&models_v);
        let models = model_set(&models_v);
        let assocs = build_assoc_registry(&models_v);
        let bare_read = Expr::new(
            span(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("whatever"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let mut expr = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(bare_read),
                method: Symbol::from("create!"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        );
        let owner = ClassId(Symbol::from("User"));
        rewrite_call_site(&mut expr, &regs(&scopes, &models, &assocs), None, Some(&owner));
        let ExprNode::Send { recv: Some(r), .. } = &*expr.node else {
            panic!("expected Send, got {:?}", expr.node);
        };
        assert!(
            matches!(&*r.node, ExprNode::Send { recv: None, .. }),
            "a non-association bare read keeps its shape, got {:?}",
            r.node
        );
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
