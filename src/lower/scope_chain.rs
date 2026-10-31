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

pub(crate) mod query_arguments;

use crate::det::{HashMap, HashSet};

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
pub type UniqueKeys = HashMap<ClassId, Vec<UniqueKey>>;

/// One unique index the guard reads: its columns, and the predicate of
/// a partial one (`where:`), which only conflicts with the rows it
/// selects. The predicate is the one the SQLite DDL renders
/// ([`crate::emit::shared::schema_sql::Dialect::index_predicate`]), so
/// the guard and the index agree; an index whose predicate SQLite isn't
/// trusted to run is unique over every row in both.
#[derive(Clone, Debug, PartialEq)]
pub struct UniqueKey {
    pub columns: Vec<Symbol>,
    pub predicate: Option<String>,
}

/// Read the unique keys off the schema, keyed by model.
pub fn build_unique_keys(models: &[Model], schema: &crate::schema::Schema) -> UniqueKeys {
    let mut out: UniqueKeys = HashMap::default();
    for m in models {
        let Some(table) = schema.tables.get(&m.table.0) else { continue };
        let not_null = |name: &Symbol| {
            table.columns.iter().any(|c| &c.name == name && !c.nullable)
        };
        let mut keys: Vec<UniqueKey> = table
            .indexes
            .iter()
            .filter(|i| i.unique && !i.columns.is_empty())
            .filter(|i| i.columns.iter().all(not_null))
            .map(|i| UniqueKey {
                columns: i.columns.clone(),
                predicate: crate::emit::shared::schema_sql::Dialect::Sqlite
                    .index_predicate(table, i)
                    .map(str::to_string),
            })
            .collect();
        // The primary key conflicts too: a row that supplies its key
        // (`insert_all([{ id: 910, … }])`) writes it, and Rails skips one
        // already taken. A row without one reads `nil`, which matches none.
        let primary: Vec<Symbol> = if table.primary_key_columns.is_empty() {
            table.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect()
        } else {
            table.primary_key_columns.clone()
        };
        if !primary.is_empty() && !keys.iter().any(|k| k.columns == primary && k.predicate.is_none()) {
            keys.push(UniqueKey { columns: primary, predicate: None });
        }
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
    let mut reg: ScopeRegistry = HashMap::default();
    for m in models {
        let map = reg.entry(m.name.clone()).or_default();
        for item in &m.body {
            match item {
                ModelBodyItem::Scope { scope, .. } => {
                    map.insert(scope.name.clone(), scope.params.clone());
                }
                ModelBodyItem::Method { method, .. }
                    if method.receiver == crate::dialect::MethodReceiver::Class
                        // Relation threading cannot append a formal to `...`
                        // or treat its nameless packet as fixed arity.
                        && !method.params.iter().any(|p| p.forwarding)
                        && mentions_bare_chain_start(&method.body) =>
                {
                    // Declared scopes win on a name collision.
                    map.entry(method.name.clone())
                        .or_insert_with(|| method.params.clone());
                }
                _ => {}
            }
        }
        // The scopes RAILS declares beside `has_one_attached` /
        // `has_rich_text` — `with_attached_<attr>`,
        // `with_rich_text_<attr>[_and_embeds]`. Their bodies are
        // synthesized later (identity, see `attached::
        // push_preload_scope_methods`), so neither arm above can see
        // them: they are not a `ModelBodyItem::Scope`, and an identity
        // body does not "mention a bare chain start".
        //
        // WITHOUT THIS ENTRY THE RELATION IS SILENTLY DROPPED, and it
        // is not a performance difference — it is the wrong rows.
        // campfire writes
        //
        //   scope :with_attachment_details, -> {
        //     with_rich_text_body_and_embeds
        //     with_attached_attachment.includes(attachment_blob: …)
        //   }
        //
        // and Rails evaluates a scope lambda with `self` as the current
        // relation, so the bare `with_attached_attachment` chains onto
        // it. Unregistered here, the call fell through every arm, took
        // its own `__rel` default — a FRESH relation — and the
        // `where(room_id:)` the controller had built vanished. `/rooms/1`
        // rendered the last 40 messages in the whole table: room 5's.
        // Every tag count matched Rails because every room is seeded the
        // same shape, which is exactly why a tag-tally oracle could not
        // see it.
        //
        // No user params: these take `__rel` alone.
        for name in crate::lower::attached::preload_scope_names(m)
            .into_iter()
            .chain(crate::lower::rich_text::preload_scope_names(m))
            .chain(crate::lower::plain_text_attr::preload_scope_names(m))
        {
            map.entry(name).or_default();
        }
    }
    // Not only the model's own scopes: one declared on an abstract base answers on the subclass (`article.comments.approved`), the child's own declaration winning.
    let parents: HashMap<ClassId, ClassId> =
        models.iter().filter_map(|m| m.parent.clone().map(|p| (m.name.clone(), p))).collect();
    for m in models {
        let mut current = parents.get(&m.name).cloned();
        for _ in 0..32 {
            let Some(p) = current else { break };
            let inherited: Vec<(Symbol, Vec<Param>)> =
                reg.get(&p).map(|s| s.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
            let own = reg.entry(m.name.clone()).or_default();
            for (name, params) in inherited {
                own.entry(name).or_insert(params);
            }
            current = parents.get(&p).cloned();
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
///
/// `class` is the same fact for a model's CLASS methods, keyed by the
/// model as well, because the receiver there is the constant itself:
/// lobsters' `Comment.recent_threads(user)` ends in `Comment.joins(…)
/// .where(…)`, and without the entry the chain after it
/// (`.merge(…).for_presentation.joins(:story)`) lost its model — the
/// scope stayed a bare method call and the association join reached
/// the runtime unresolved.
#[derive(Default)]
pub struct UserMethodReturns {
    pub instance: HashMap<Symbol, Option<ClassId>>,
    pub class: HashMap<(ClassId, Symbol), ClassId>,
    /// Class methods defined on that model itself. Ancestors stay on
    /// the ancestor: [`UserMethodReturns::defines_class_method`] walks
    /// [`Self::parents`], the same question `ClassInfo::source_class_method`
    /// answers for the typer. Copying every ancestor method onto each
    /// descendant made the two indexes disagree about who defined it.
    pub class_methods: HashSet<(ClassId, Symbol)>,
    pub parents: HashMap<ClassId, ClassId>,
    /// Scopes whose body answers something OTHER than a relation (see
    /// [`non_relation_scopes`]). A call to one ends the chain's model.
    pub non_relation_scopes: HashSet<(ClassId, Symbol)>,
}

impl UserMethodReturns {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source `def self.<method>` on `model` or one of its parents.
    pub fn defines_class_method(&self, model: &ClassId, method: &Symbol) -> bool {
        let mut current = Some(model);
        for _ in 0..32 {
            let Some(id) = current else { return false };
            if self.class_methods.contains(&(id.clone(), method.clone())) {
                return true;
            }
            current = self.parents.get(id);
        }
        false
    }
}

pub fn build_user_method_returns(models: &[Model]) -> UserMethodReturns {
    let model_ids: HashSet<ClassId> = models.iter().map(|m| m.name.clone()).collect();
    let mut reg = UserMethodReturns::new();
    for m in models {
        for item in &m.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            if method.receiver == crate::dialect::MethodReceiver::Class {
                reg.class_methods.insert((m.name.clone(), method.name.clone()));
                if let Some(target) = relation_return_model(&method.body, &model_ids) {
                    reg.class.insert((m.name.clone(), method.name.clone()), target);
                }
                continue;
            }
            if method.receiver != crate::dialect::MethodReceiver::Instance {
                continue;
            }
            let Some(target) = relation_return_model(&method.body, &model_ids) else {
                continue;
            };
            reg.instance.entry(method.name.clone())
                .and_modify(|t| {
                    if t.as_ref() != Some(&target) {
                        *t = None;
                    }
                })
                .or_insert(Some(target));
        }
    }
    for model in models {
        if let Some(parent) = &model.parent {
            reg.parents.insert(model.name.clone(), parent.clone());
        }
    }
    reg.non_relation_scopes = non_relation_scopes(models, &model_ids);
    reg
}

/// Scopes that answer a non-relation VALUE — campfire's `scope
/// :last_page, -> { last_page_of(PAGE_SIZE) }`, whose class method ends
/// in `Page.load(…)`, an Array subclass. Rails returns a scope body's
/// value as-is when it is not nil, so such a call is not a relation and
/// `page.first(2)` on it is `Array#first`, not the counted terminal.
///
/// Proved, not guessed: the body's tail — followed through same-model
/// bare calls to scopes and class methods — must end in a call on a
/// constant that names no model. Anything else stays a relation, as
/// before.
fn non_relation_scopes(models: &[Model], model_ids: &HashSet<ClassId>) -> HashSet<(ClassId, Symbol)> {
    let mut out = HashSet::default();
    for m in models {
        let mut bodies: HashMap<&Symbol, &Expr> = HashMap::default();
        for item in &m.body {
            match item {
                ModelBodyItem::Scope { scope, .. } => {
                    bodies.insert(&scope.name, &scope.body);
                }
                ModelBodyItem::Method { method, .. }
                    if method.receiver == crate::dialect::MethodReceiver::Class =>
                {
                    bodies.entry(&method.name).or_insert(&method.body);
                }
                _ => {}
            }
        }
        for item in &m.body {
            let ModelBodyItem::Scope { scope, .. } = item else { continue };
            if tail_is_foreign_const_call(&scope.body, &bodies, model_ids, 0) {
                out.insert((m.name.clone(), scope.name.clone()));
            }
        }
    }
    out
}

fn tail_is_foreign_const_call(
    body: &Expr,
    bodies: &HashMap<&Symbol, &Expr>,
    model_ids: &HashSet<ClassId>,
    depth: usize,
) -> bool {
    if depth > 8 {
        return false;
    }
    let tail = match &*body.node {
        ExprNode::Seq { exprs } => match exprs.last() {
            Some(e) => e,
            None => return false,
        },
        _ => body,
    };
    let ExprNode::Send { recv, method, .. } = &*tail.node else {
        return false;
    };
    match recv.as_ref().map(|r| &*r.node) {
        Some(ExprNode::Const { path }) => {
            let name = path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
            let last = path.last().map(|p| p.as_str()).unwrap_or("");
            !model_ids.iter().any(|id| {
                id.0.as_str() == name || id.0.as_str().rsplit("::").next() == Some(last)
            })
        }
        // A bare/self call, or a scope reached on a relation chain
        // (`before(m).last_page`): follow the named sibling's body.
        _ => bodies
            .get(method)
            .is_some_and(|b| tail_is_foreign_const_call(b, bodies, model_ids, depth + 1)),
    }
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
#[derive(Clone)]
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
    if method_def.params.iter().any(|p| p.forwarding) {
        return AssocScopeShape::Blocked("full forwarding cannot use the relation-threading argument ABI".into());
    }
    let mut found = false;
    let mut queries = false;
    let mut blocked: Option<String> = None;
    // Parameters DECLARED to be an attribute hash. `params_merge` stamps
    // them when every call site agreed the argument is one, which is
    // what makes `create!(attributes)` mergeable without guessing.
    let hash_params: HashSet<Symbol> = match &method_def.signature {
        Some(crate::ty::Ty::Fn { params, .. }) => params
            .iter()
            .filter(|p| matches!(*p.ty, crate::ty::Ty::Hash { .. }))
            .map(|p| p.name.clone())
            .collect(),
        _ => HashSet::default(),
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
    let mut assoc_locals: HashMap<Symbol, Symbol> = HashMap::default();
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
        // ...and any receiver the analyzer typed as one of the model's
        // relations, however it was built: a through-association
        // (campfire's `Current.user.reachable_messages.search(q)
        // .last_page_of_matches(100)`) has no foreign key to seed from
        // and no model constant at its root, so neither syntactic
        // channel sees it. The relation delegate answers it instead.
        // The analyzer types a scope's result as `Array[Model]` as often
        // as `Relation[Model]` (the catalog does not yet tell them
        // apart), so both count. Demand only registers a class method
        // the model really has; an Array receiver still has no such
        // method at runtime, as in Rails.
        let typed_model = match &r.ty {
            Some(crate::ty::Ty::Relation { of }) => Some(of),
            Some(crate::ty::Ty::Array { elem }) => match &**elem {
                crate::ty::Ty::Class { id, .. } => Some(id),
                _ => None,
            },
            _ => None,
        };
        if let Some(of) = typed_model {
            if models.contains(of) {
                out.insert((of.clone(), method.clone()));
            }
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
    let mut demand: HashSet<(Symbol, Symbol)> = HashSet::default();
    let mut model_demand: HashSet<(ClassId, Symbol)> = HashSet::default();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        collect_assoc_method_demand(body, assocs, &scope_names, &mut demand);
        collect_relation_class_method_demand(body, &models_set, &scope_names, &mut model_demand);
    });
    // A gem's class method a relation may reach where no call site is typed (`relation_class_calls`).
    for (name, owner) in crate::lower::relation_class_calls::spliced_owners(app) {
        // Not a writer: `relation.x = v` is no call a relation delegates, and the delegate would not parse
        if !name.as_str().ends_with('=') {
            model_demand.insert((owner, name));
        }
    }
    for view in &app.views {
        collect_assoc_method_demand(&view.body, assocs, &scope_names, &mut demand);
        collect_relation_class_method_demand(
            &view.body,
            &models_set,
            &scope_names,
            &mut model_demand,
        );
    }
    // A scope body runs with its relation as the current scope, so a
    // class method it calls at implicit self runs there too: campfire's
    // `scope :last_page, -> { last_page_of(PAGE_SIZE) }`
    // (basecamp/once-campfire#292). Without the demand, `last_page_of`
    // never took the relation and paged the whole table. The same holds
    // one call further for a class method that itself takes the
    // relation, so the demand closes over the registered bodies.
    // Keyed by the defining model, as the registry is: demand on
    // `Article` for a method `ApplicationRecord` defines is demand on it.
    let explicit_demand: HashSet<(ClassId, Symbol)> = model_demand
        .iter()
        .map(|(target, name)| match class_method_owner(&app.models, target, name) {
            Some((owner, _)) => (owner.name.clone(), name.clone()),
            None => (target.clone(), name.clone()),
        })
        .collect();
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Scope { scope, .. } = item {
                collect_implicit_self_sends(&scope.body, &model.name, &mut model_demand);
            }
        }
    }
    loop {
        let (mut reg, declined) =
            build_assoc_class_methods(&app.models, assocs, scopes, &demand, &model_demand);
        // A callee found only at implicit self needs the relation only
        // when its body uses it. A helper that neither queries nor
        // creates (campfire's `match_terms`, which quotes search words)
        // keeps its own signature: no call site reaches it through a
        // relation, so there is nothing to answer.
        for (model, per_model) in reg.iter_mut() {
            per_model.retain(|name, entry| {
                entry.creates
                    || entry.queries
                    || explicit_demand.contains(&(model.clone(), name.clone()))
            });
        }
        reg.retain(|_, per_model| !per_model.is_empty());
        let before = model_demand.len();
        for model in &app.models {
            let Some(per_model) = reg.get(&model.name) else { continue };
            for item in &model.body {
                if let ModelBodyItem::Method { method, .. } = item {
                    if method.receiver == crate::dialect::MethodReceiver::Class
                        && per_model.contains_key(&method.name)
                    {
                        collect_implicit_self_sends(&method.body, &model.name, &mut model_demand);
                    }
                }
            }
        }
        if model_demand.len() == before {
            return (inherit_down(&app.models, reg), declined);
        }
    }
}

/// Each entry also under every model that inherits it without defining the
/// method itself: `Article.order(:id).base` reaches `ApplicationRecord`'s
/// `def self.base`, so the call-site rewrite and the relation delegate,
/// which look a call up by the relation's model, find it there and call
/// `Article.base(rel)`, which Ruby resolves to the ancestor's def.
fn inherit_down(models: &[Model], mut reg: AssocClassMethods) -> AssocClassMethods {
    let mut added: Vec<(ClassId, Symbol, AssocScopedMethod)> = Vec::new();
    let declares_scope = |id: &ClassId, name: &Symbol| {
        models.iter().filter(|m| &m.name == id).flat_map(|m| &m.body).any(|item| {
            matches!(item, ModelBodyItem::Scope { scope, .. } if scope.name == *name)
        })
    };
    for model in models {
        // The model and the ancestors passed so far: a scope on any of
        // them is nearer than the ancestor's def.
        let mut below = vec![model.name.clone()];
        let mut current = model.parent.clone();
        for _ in 0..32 {
            let Some(id) = current else { break };
            if let Some(per) = reg.get(&id) {
                for (name, entry) in per {
                    // The nearest def is this ancestor's: an override
                    // in between (or on the model) is its own method.
                    let reaches = class_method_owner(models, &model.name, name).is_some_and(|(m, _)| m.name == id)
                        && !below.iter().any(|b| declares_scope(b, name));
                    if reaches && !added.iter().any(|(m, n, _)| m == &model.name && n == name) {
                        added.push((model.name.clone(), name.clone(), entry.clone()));
                    }
                }
            }
            current = models.iter().find(|m| m.name == id).and_then(|m| m.parent.clone());
            below.push(id);
        }
    }
    for (model, name, entry) in added {
        reg.entry(model).or_default().insert(name, entry);
    }
    reg
}

/// `(model, method)` for every receiver-less call in a body that runs
/// at the model's class level. Most name a scope or a Relation builtin,
/// which `build_assoc_class_methods` passes over; what it keeps are the
/// model's own class methods.
fn collect_implicit_self_sends(expr: &Expr, model: &ClassId, out: &mut HashSet<(ClassId, Symbol)>) {
    if let ExprNode::Send { recv: None, method, .. } = &*expr.node {
        out.insert((model.clone(), method.clone()));
    }
    expr.node.for_each_child(&mut |c| collect_implicit_self_sends(c, model, out));
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

/// The model that defines `mname` as a class method for `target`: the
/// target itself, or the nearest ancestor among the models (an abstract
/// `ApplicationRecord`'s `def self.x`), which Ruby's method lookup
/// reaches from the subclass.
fn class_method_owner<'a>(
    models: &'a [Model],
    target: &ClassId,
    mname: &Symbol,
) -> Option<(&'a Model, &'a crate::dialect::MethodDef)> {
    let mut current = Some(target.clone());
    for _ in 0..32 {
        let model = models.iter().find(|m| Some(&m.name) == current.as_ref())?;
        let found = model.body.iter().find_map(|item| match item {
            ModelBodyItem::Method { method, .. }
                if method.receiver == crate::dialect::MethodReceiver::Class && method.name == *mname =>
            {
                Some(method)
            }
            _ => None,
        });
        if let Some(method) = found {
            return Some((model, method));
        }
        current = model.parent.clone();
    }
    None
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
    let mut reg: AssocClassMethods = HashMap::default();
    let mut declined: Vec<DeclinedAssocScope> = Vec::new();
    let mut seen: HashSet<(ClassId, Symbol)> = HashSet::default();
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
            let Some((owner_model, method)) = class_method_owner(models, target, mname) else { continue };
            let target = &owner_model.name;
            if !seen.insert((target.clone(), mname.clone())) {
                continue;
            }
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
        let Some(target_model) = models.iter().find(|m| m.name == *target) else { continue };
        // `generates_token_for`'s finders, which the model lowering
        // synthesizes after this survey (lower::generates_token_for): no
        // body to read here, but each looks its record up with an
        // implicit-self `find_by` / `find`, so reached through a relation
        // it queries that relation, as Rails' scoping makes it
        // (`User.active.find_by_transfer_id` -> `find_by_token_for`).
        if matches!(mname.as_str(), "find_by_token_for" | "find_by_token_for!")
            && !crate::lower::generates_token_for::token_for_decls(target_model).is_empty()
            && !target_model.body.iter().any(|item| matches!(item,
                ModelBodyItem::Method { method, .. } if method.name == *mname))
        {
            if !seen.insert((target.clone(), mname.clone())) {
                continue;
            }
            reg.entry(target.clone()).or_default().insert(
                mname.clone(),
                AssocScopedMethod {
                    params: vec![
                        crate::dialect::Param::positional(Symbol::from("purpose")),
                        crate::dialect::Param::positional(Symbol::from("token")),
                    ],
                    creates: false,
                    queries: true,
                },
            );
            continue;
        }
        // The defining model: `target` itself or an ancestor
        // (`ApplicationRecord`), which then grows the parameter.
        let Some((owner_model, method)) = class_method_owner(models, target, mname) else { continue };
        let target = &owner_model.name;
        if !seen.insert((target.clone(), mname.clone())) {
            continue;
        }
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
    /// (model, belongs_to name) pairs declared `polymorphic: true`: their
    /// condition also constrains `<name>_type` (see `lower_relation_args`).
    polymorphic_belongs_to: HashMap<(ClassId, Symbol), (Symbol, Vec<ClassId>)>,
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
    has_many_unseedable: crate::det::HashSet<(ClassId, Symbol)>,
    /// `(model, has_many name)` -> the OWNER column the foreign key
    /// matches, for every has_many declaring `primary_key:` (absent
    /// means `id`). The seed's `where(fk => owner.<key>)` and the
    /// constructors' preset foreign key read it, so a has_many keyed on
    /// a non-id column seeds the rows Rails would. The by-NAME rung has
    /// no owner to ask, so a name that is keyed anywhere is declined
    /// there (`is_unseedable`).
    has_many_owner_key: HashMap<(ClassId, Symbol), Symbol>,
    /// `(model, has_many name)` -> the methods of its `has_many :x do
    /// def m … end end` extension. The model lowerer FLATTENS those onto
    /// the owner as `<assoc>_<method>` instance methods
    /// (`synth_assoc_extension_methods`); this is the call-site half —
    /// `room.memberships.grant_to(users)` -> `room.memberships_grant_to(
    /// users)`.
    ///
    /// An owner typed as a model asks its own declaration (see
    /// `is_assoc_extension`). One whose type is unknown asks by
    /// association name alone, which still rewrites correctly: the
    /// flattened name is derived from the association, so two models
    /// declaring the same extension method on the same association name
    /// agree on it by construction, and declaring it on DIFFERENT
    /// association names produces different flattened names, which is
    /// also correct.
    assoc_extension: HashMap<(ClassId, Symbol), Vec<Symbol>>,
    /// `(model, has_many name)` -> the target's `belongs_to` that Rails
    /// finds as the association's AUTOMATIC inverse: named after the
    /// declaring model (`Room has_many :messages` -> `Message
    /// belongs_to :room`), on the same foreign key, pointing back at
    /// the declaring model. `room.messages.new(...)` hands that
    /// `belongs_to` the owner OBJECT, so `message.room` is `room`
    /// itself rather than a re-read of the row: a singleton method or
    /// an unsaved change on the owner is what the record sees.
    ///
    /// Rails' own conditions for finding one, as far as this registry
    /// can see them: no `through:`, no `as:`, no scope, no explicit
    /// `foreign_key:` on either side, and an `id` owner key.
    has_many_inverse: HashMap<(ClassId, Symbol), Symbol>,
    /// (model, has_many name) pairs whose model holds what is built on or
    /// pushed onto it (`ingest::has_many_hold`'s `__hold_<name>` /
    /// `__push_<name>`, the ruby family): `build` hands its record there
    /// and `<<` goes there.
    has_many_held: HashSet<(ClassId, Symbol)>,
    /// `(model, association name)` -> what the controller's parent-ivar
    /// rewrite expands `@parent.<name>.build` / `.find` to (see
    /// [`Self::build_target`]). Every declaration has one, whatever its
    /// kind, so the nearest class declaring a name is the one whose
    /// reader `model.<name>` runs.
    ///
    /// One entry per owner, like every other `(model, name)` fact here:
    /// a declaration repeated on the same model (a concern's `has_many`
    /// spliced at the `include`, then the model's own) replaces the
    /// earlier one, as it does in Rails and in the reader the model
    /// lowerer emits last (see `forget`).
    assoc_builds: HashMap<(ClassId, Symbol), AssocBuild>,
    /// model -> its superclass model, so `Article < Publication` builds
    /// and finds through the associations its abstract base declares.
    model_parents: HashMap<ClassId, ClassId>,
    has_many_poly: HashMap<(ClassId, Symbol), (Symbol, String)>,
}

/// One association declaration, as the parent-ivar rewrite reads it
/// (see [`AssocRegistry::build_target`]).
struct AssocBuild {
    /// The `(target, foreign key)` when, its scope aside, `<target>.new(
    /// <foreign_key> => parent.id)` is what Rails builds through it, and
    /// `<target>.find` checked against the key what it finds. `None`
    /// otherwise.
    expansion: Option<(ClassId, Symbol)>,
    /// Its scope lambda, which each call judges for itself (see
    /// [`scope_is_build_neutral`]).
    scope: Option<Expr>,
    /// The methods its extension block defines. The proxy calls those
    /// in place of Rails' own, so a call by one of these names is not
    /// expanded.
    extension: Vec<Symbol>,
    /// The target model's key is `id` (no `self.primary_key =`), so a
    /// `find_by` naming `id` matches at most one row.
    target_keyed_by_id: bool,
}

impl AssocBuild {
    /// The expansion of `<parent>.<assoc>.<call>`: none when the
    /// extension defines `call`, or the scope changes what `call`
    /// builds or finds. A `find_by` naming the target's `id`
    /// (`by_id`) matches at most one row, so an ordering scope cannot
    /// change which it takes: the scope judges it as it judges `find`.
    fn expansion_for(&self, call: &Symbol, by_id: bool) -> Option<&(ClassId, Symbol)> {
        let find = Symbol::from("find");
        let judged = if by_id && self.target_keyed_by_id && call.as_str() == "find_by" { &find } else { call };
        self.expansion.as_ref().filter(|_| {
            !self.extension.contains(call) && self.scope.as_ref().is_none_or(|s| scope_is_build_neutral(s, judged))
        })
    }
}

/// What `@<parent>.<assoc>.<call>` builds or finds through, for the
/// controller's parent-ivar rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssocBuildTarget {
    /// The declared class and foreign key.
    Declared { target: ClassId, foreign_key: Symbol },
    /// Declared in a way the rewrite does not reproduce for this call,
    /// not declared on the parent's model (or on one of the models a
    /// union-typed parent may hold), or declared differently on several
    /// models when neither the parent's type nor its name picks one.
    Declined,
    /// No model declares it (one from a `with_options` block or a
    /// custom macro, which the registry does not see).
    Undeclared,
}

impl AssocRegistry {
    /// Does some model hold what `<name>` builds or is pushed (see the
    /// field)?
    fn is_held_name(&self, name: &Symbol) -> bool {
        self.has_many_held.iter().any(|(_, n)| n == name)
    }

    /// The automatic-inverse `belongs_to` of `owner.<assoc>` (see the
    /// field). None for an owner the call site could not resolve.
    /// With no owner (the by-NAME rung), only when every model
    /// declaring `has_many <assoc>` has the same inverse.
    fn has_many_inverse(&self, owner: Option<&ClassId>, assoc: &Symbol) -> Option<&Symbol> {
        match owner {
            Some(m) => self.has_many_inverse.get(&(m.clone(), assoc.clone())),
            None => {
                let mut found: Option<&Symbol> = None;
                for (m, a) in self.has_many_fk.keys() {
                    if a != assoc {
                        continue;
                    }
                    let inv = self.has_many_inverse.get(&(m.clone(), a.clone()))?;
                    if found.is_some_and(|f| f != inv) {
                        return None;
                    }
                    found = Some(inv);
                }
                found
            }
        }
    }
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
    /// `(type column, resolved implementor classes)` of a polymorphic
    /// belongs_to; the set is empty when no `as:` inverse resolved it.
    fn polymorphic_belongs_to(&self, model: &ClassId, assoc: &Symbol) -> Option<&(Symbol, Vec<ClassId>)> {
        self.polymorphic_belongs_to.get(&(model.clone(), assoc.clone()))
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
    /// extension the model lowerer flattened onto the owner? `owners`
    /// are the models the owner may be (see `owner_models`), each of
    /// which must read a declaration defining it; without them, any
    /// model's declaration answers.
    fn is_assoc_extension(&self, owners: Option<&[&ClassId]>, assoc: &Symbol, method: &Symbol) -> bool {
        let defines = |class: &ClassId| {
            self.assoc_extension
                .get(&(class.clone(), assoc.clone()))
                .is_some_and(|methods| methods.contains(method))
        };
        match owners {
            Some(models) => models.iter().all(|m| self.declaring_class(m, assoc).is_some_and(defines)),
            None => self.assoc_extension.keys().any(|(class, a)| a == assoc && defines(class)),
        }
    }
    /// See `has_many_unseedable`. Checked against every model declaring
    /// the name, not just the resolved owner: the by-NAME rung answers
    /// without an owner, so a name that is unseedable ANYWHERE has to be
    /// declined there too.
    fn is_unseedable(&self, owner: Option<&ClassId>, assoc: &Symbol) -> bool {
        match owner {
            Some(m) => self.has_many_unseedable.contains(&(m.clone(), assoc.clone())),
            None => {
                self.has_many_unseedable.iter().any(|(_, a)| a == assoc)
                    || self.has_many_owner_key.keys().any(|(_, a)| a == assoc)
                    || self.has_many_poly.keys().any(|(_, a)| a == assoc)
            }
        }
    }
    /// The owner column `model.<assoc>`'s foreign key matches
    /// (`primary_key:`), `id` when undeclared.
    fn has_many_owner_key(&self, model: Option<&ClassId>, assoc: &Symbol) -> Symbol {
        model
            .and_then(|m| self.has_many_owner_key.get(&(m.clone(), assoc.clone())))
            .cloned()
            .unwrap_or_else(|| Symbol::from("id"))
    }
    fn has_many_poly(&self, model: Option<&ClassId>, assoc: &Symbol) -> Option<&(Symbol, String)> {
        model.and_then(|m| self.has_many_poly.get(&(m.clone(), assoc.clone())))
    }
        fn is_model(&self, class: &ClassId) -> bool {
        self.tables.contains_key(class)
    }
    /// The models `owner` may be, `nil` aside, for a `<owner>.<assoc>`
    /// read: its type's class, or every arm of a union. `None` unless
    /// each is a model, which is when only their own declarations answer
    /// for it.
    ///
    /// A union can be narrowed by the owner's name (see
    /// [`Self::named_arm`]): `@room` typed `Membership | Room` is the
    /// Room, as campfire's `@membership, @room = RecordCache.fetch(…)`
    /// types it.
    fn owner_models<'t>(&self, owner: &'t Expr, assoc: &Symbol) -> Option<Vec<&'t ClassId>> {
        use crate::ty::Ty;
        let arms: Vec<&Ty> = match owner.ty.as_ref()? {
            Ty::Union { variants } => variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect(),
            other => vec![other],
        };
        let models = arms
            .into_iter()
            .map(|arm| match arm {
                Ty::Class { id, .. } if self.is_model(id) => Some(id),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        if let Some(arm) = self.named_arm(owner, &models, assoc) {
            return Some(vec![arm]);
        }
        (!models.is_empty()).then_some(models)
    }
    /// The arm of a union-typed owner that its name camelizes to, when
    /// that arm's chain declares `assoc` and no other arm declares it
    /// differently. An arm declaring nothing by the name has no such
    /// reader, so it is not the owner of a `<owner>.<assoc>` read that
    /// runs, and it does not stand in the way. An arm declaring it
    /// differently might be, and keeps the union undecided.
    fn named_arm<'t>(&self, owner: &Expr, models: &[&'t ClassId], assoc: &Symbol) -> Option<&'t ClassId> {
        if models.len() < 2 {
            return None;
        }
        let named = owner_model_from_name(owner, self)?;
        let arm = *models.iter().find(|m| **m == named)?;
        let class = self.declaring_class(arm, assoc)?;
        models
            .iter()
            .all(|m| self.declaring_class(m, assoc).is_none_or(|c| c == class))
            .then_some(arm)
    }
    /// The class in `model`'s superclass chain, `model` first, nearest
    /// declaring an association named `assoc`: the one whose declaration
    /// `model.<assoc>` reads.
    fn declaring_class<'a>(&'a self, model: &'a ClassId, assoc: &Symbol) -> Option<&'a ClassId> {
        std::iter::successors(Some(model), |class| self.model_parents.get(*class))
            .take(self.model_parents.len() + 1)
            .find(|class| self.assoc_builds.contains_key(&((*class).clone(), assoc.clone())))
    }
    /// For an owner of no known model, the class whose declaration its
    /// name says `<owner>.<assoc>` reads: the model the name camelizes
    /// to (`@story`, `story`, `Current.user`'s `user`), when that model
    /// or a superclass declares the association. Two-sided, as
    /// `owner_model_from_name` explains: a name that is no model, or
    /// names a model without the association, answers `None`, and the
    /// caller falls back on the declaration every declarer agrees on.
    /// The rewrite ([`Self::build_target`]) and the seed
    /// (`assoc_read_target`) both resolve an untyped owner through it,
    /// so the two cannot pick different declarations for one call.
    fn declaring_class_by_name(&self, owner: &Expr, assoc: &Symbol) -> Option<&ClassId> {
        let model = owner_model_from_name(owner, self)?;
        self.declaring_class(model, assoc)
    }
    /// What `<parent>.<assoc>.<call>` (`build`, `new`, `create`,
    /// `create!`, `find`, `find_by`) builds or finds through, given the
    /// parent expression and its analyzed type.
    ///
    /// A name no model declares is `Undeclared`, whatever the parent:
    /// the rewrite keeps its name-derived form for it.
    ///
    /// A parent typed as a model reads the declaration on that model or
    /// the nearest superclass declaring it. A model none of whose chain
    /// declares it has no such association, whatever other models
    /// declare: `@tag.comments` is not Article's, keyed on `article_id`.
    /// A parent typed as a union of models (`@owner` assigned a Tag or
    /// an Article) is each of them, and expands only when every one
    /// reads the same expansion, from whichever declaration (the seed,
    /// which names the owner model, needs one declaring class for all of
    /// them; see `assoc_read_target`). One named after an arm declaring
    /// the association (`@article` assigned a Tag or an Article) is that
    /// arm, unless another arm declares it differently (see
    /// [`Self::owner_models`]).
    ///
    /// A parent of no known model resolves as the seed resolves it: by
    /// its name when that names a model whose chain declares the
    /// association (see [`Self::declaring_class_by_name`]), else by the
    /// declaration every declarer agrees on, each owner counted once by
    /// the declaration it keeps. A name declared differently on several
    /// models, with neither the type nor the name to choose by, is not
    /// guessed at.
    ///
    /// The answer is per call: an extension defining `find` declines
    /// `find`, so the extension runs, and leaves `build` expanded; an
    /// ordering scope declines `find_by` alone (see
    /// [`scope_is_build_neutral`]), unless it names the target's `id`
    /// (`by_id`).
    pub fn build_target(&self, owner: &Expr, assoc: &Symbol, call: &Symbol, by_id: bool) -> AssocBuildTarget {
        if !self.assoc_builds.keys().any(|(_, name)| name == assoc) {
            return AssocBuildTarget::Undeclared;
        }
        let read = |class: &ClassId| self.assoc_builds[&(class.clone(), assoc.clone())].expansion_for(call, by_id);
        let expansions: Vec<Option<&(ClassId, Symbol)>> = match self.owner_models(owner, assoc) {
            Some(models) => {
                models.into_iter().map(|model| self.declaring_class(model, assoc).and_then(read)).collect()
            }
            None => match self.declaring_class_by_name(owner, assoc) {
                Some(class) => vec![read(class)],
                None => self
                    .assoc_builds
                    .iter()
                    .filter(|((_, name), _)| name == assoc)
                    .map(|(_, b)| b.expansion_for(call, by_id))
                    .collect(),
            },
        };
        let Some((first, rest)) = expansions.split_first() else {
            return AssocBuildTarget::Declined;
        };
        match first {
            Some((target, foreign_key)) if rest.iter().all(|e| e == first) => {
                AssocBuildTarget::Declared { target: target.clone(), foreign_key: foreign_key.clone() }
            }
            _ => AssocBuildTarget::Declined,
        }
    }
    /// Drop every fact recorded for `model.<name>`: a later declaration
    /// of the name replaces the earlier one whatever either's kind, and
    /// records only what it declares itself, which may be nothing (an
    /// unresolvable `through:`). Lists every field so a new one is
    /// decided here.
    fn forget(&mut self, model: &ClassId, name: &Symbol) {
        let Self {
            join_tails,
            belongs_to_fk,
            polymorphic_belongs_to,
            assoc_target,
            assoc_table,
            aliased_join_tails,
            has_many_fk,
            has_many_by_name: _,
            tables: _,
            has_many_unseedable,
            has_many_owner_key,
            assoc_extension,
            has_many_inverse,
            has_many_held,
            assoc_builds,
            model_parents: _,
            has_many_poly,
        } = self;
        let key = (model.clone(), name.clone());
        join_tails.remove(&key);
        belongs_to_fk.remove(&key);
        polymorphic_belongs_to.remove(&key);
        assoc_target.remove(&key);
        assoc_table.remove(&key);
        aliased_join_tails.remove(&key);
        has_many_fk.remove(&key);
        has_many_unseedable.remove(&key);
        has_many_owner_key.remove(&key);
        has_many_held.remove(&key);
        assoc_extension.remove(&key);
        has_many_inverse.remove(&key);
        assoc_builds.remove(&key);
        has_many_poly.remove(&key);
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
pub(crate) fn scope_is_row_preserving(scope: &Expr) -> bool {
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

/// Does this association-scope lambda leave what `<assoc>.<call>`
/// builds (`build`, `new`, `create`, `create!`) or finds (`find`,
/// `find_by`) as it would be without it?
///
/// Rails builds through an association with the scope's `where`
/// equalities and `create_with` values, and finds within its rows.
/// Eager loading and `references` change neither, so this admits them
/// for every call, as [`scope_is_row_preserving`] admits eager loads.
/// So does `readonly`, which changes neither which row is found nor
/// what is built; the expanded record is not marked readonly, as it
/// was not by the name-derived rewrite this replaced. `order` and
/// `reorder` decide which match `find_by` takes (the first in that
/// order), so they decline `find_by` alone: what `build` makes and the
/// row `find(id)` names do not depend on them. Anything else declines,
/// as there.
pub(crate) fn scope_is_build_neutral(scope: &Expr, call: &Symbol) -> bool {
    fn walk(e: &Expr, takes_first: bool) -> bool {
        match &*e.node {
            ExprNode::Send { recv, method, .. } => {
                let admitted = match method.as_str() {
                    "includes" | "preload" | "eager_load" | "readonly" | "references" => true,
                    "order" | "reorder" => !takes_first,
                    _ => false,
                };
                admitted && recv.as_ref().is_none_or(|r| walk(r, takes_first))
            }
            // The chain root a receiver-less lambda body lowers to.
            ExprNode::SelfRef => true,
            _ => false,
        }
    }
    walk(scope, call.as_str() == "find_by")
}

/// Rails' `association_primary_key` for a `belongs_to`: the declaration's
/// `primary_key:`, else the target model's own primary key, else `id`.
pub(crate) fn association_primary_key(
    models: &[Model],
    target: &ClassId,
    declared: Option<&Symbol>,
) -> Symbol {
    declared
        .cloned()
        .or_else(|| models.iter().find(|o| &o.name == target).and_then(|o| o.primary_key.clone()))
        .unwrap_or_else(|| Symbol::from("id"))
}

/// Build the association registry. Table names use the same
/// `pluralize_snake` the synthesized `table_name` methods use, so the
/// generated SQL and the runtime agree by construction.
/// The `belongs_to` on the target that Rails finds as `owner`'s
/// `has_many` AUTOMATIC inverse (see `AssocRegistry::has_many_inverse`):
/// named after the declaring model, on the same foreign key, pointing
/// back at it. Rails' conditions, as far as the declarations show them:
/// no `through:`, no `as:`, no scope, no explicit `foreign_key:` on
/// either side, and an `id` owner key.
pub fn automatic_inverse(models: &[Model], owner: &Model, assoc: &Association) -> Option<Symbol> {
    let Association::HasMany {
        target,
        foreign_key,
        foreign_key_explicit: false,
        through: None,
        as_interface: None,
        scope: None,
        ..
    } = assoc
    else {
        return None;
    };
    if assoc.owner_key().as_str() != "id" {
        return None;
    }
    let inverse = crate::naming::underscore(crate::naming::demodulize(owner.name.0.as_str()));
    models.iter().find(|tm| &tm.name == target)?.associations().find_map(|ta| match ta {
        Association::BelongsTo {
            name: bt,
            target: bt_target,
            foreign_key: bt_fk,
            polymorphic: false,
            primary_key: None,
            ..
        } if bt.as_str() == inverse
            && bt_target == &owner.name
            && bt_fk == foreign_key
            && bt_fk.as_str() == format!("{inverse}_id") =>
        {
            Some(bt.clone())
        }
        _ => None,
    })
}

pub fn build_assoc_registry(models: &[Model]) -> AssocRegistry {
    let mut reg = AssocRegistry::default();
    for m in models {
        reg.tables.insert(m.name.clone(), m.table.0.as_str().to_string());
        if let Some(parent) = &m.parent {
            reg.model_parents.insert(m.name.clone(), parent.clone());
        }
    }
    for m in models {
        let own = reg.table_for(&m.name);
        for a in m.associations() {
            reg.forget(&m.name, a.name());
            reg.assoc_builds.insert((m.name.clone(), a.name().clone()), assoc_build(a, models));
            match a {
                Association::BelongsTo {
                    name, target, foreign_key, primary_key, polymorphic, polymorphic_targets, foreign_type, ..
                } => {
                    if *polymorphic {
                        let type_col = foreign_type
                            .clone()
                            .unwrap_or_else(|| Symbol::from(format!("{}_type", name.as_str())));
                        reg.polymorphic_belongs_to
                            .insert((m.name.clone(), name.clone()), (type_col, polymorphic_targets.clone()));
                    }
                    let t = reg.table_for(target);
                    // Rails' `association_primary_key`: the declaration's
                    // `primary_key:`, else the target's own primary key.
                    // A `belongs_to :product, foreign_key:
                    // "product_uuid", primary_key: "uuid"` joined on
                    // `products.id` (a uuid) against a varchar fk.
                    let pk = association_primary_key(models, target, primary_key.as_ref());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{pk} = {own}.{foreign_key}"),
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
                            format!("{t} {name} ON {name}.{pk} = {own}.{foreign_key}"),
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
                    if !extension.is_empty() {
                        reg.assoc_extension.insert(
                            (m.name.clone(), name.clone()),
                            extension.iter().map(|x| x.name.clone()).collect(),
                        );
                    }
                    let owner_key = a.owner_key();
                    if let Some(bt) = automatic_inverse(models, m, a) {
                        reg.has_many_inverse.insert((m.name.clone(), name.clone()), bt);
                    }
                    if m.methods().any(|d| d.name.as_str() == format!("__hold_{name}")) {
                        reg.has_many_held.insert((m.name.clone(), name.clone()));
                    }
                    if owner_key.as_str() != "id" {
                        reg.has_many_owner_key
                            .insert((m.name.clone(), name.clone()), owner_key.clone());
                    }
                    if scope.as_ref().is_some_and(|s| !scope_is_row_preserving(s)) {
                        reg.has_many_unseedable.insert((m.name.clone(), name.clone()));
                    }
                    if let Some(interface) = as_interface {
                        // Rails stores the base class: an STI subclass's rows name its table's root model
                        let mut base = m;
                        while let Some(parent) = base
                            .parent
                            .as_ref()
                            .and_then(|p| models.iter().find(|x| &x.name == p))
                            .filter(|p| p.table == base.table)
                        {
                            base = parent;
                        }
                        reg.has_many_poly.insert(
                            (m.name.clone(), name.clone()),
                            (Symbol::from(format!("{}_type", interface.as_str())), base.name.0.as_str().to_string()),
                        );
                    }
                    let t = reg.table_for(target);
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.{owner_key}"),
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
                            format!("{t} {name} ON {name}.{foreign_key} = {own}.{owner_key}"),
                        );
                    }
                }
                Association::HasOne { name, target, foreign_key, .. } => {
                    let owner_key = a.owner_key();
                    let t = reg.table_for(target);
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.{owner_key}"),
                    );
                    reg.assoc_table
                        .insert((m.name.clone(), name.clone()), Symbol::from(t.as_str()));
                    reg.assoc_target
                        .insert((m.name.clone(), name.clone()), target.clone());
                    if name.as_str() != t {
                        reg.aliased_join_tails.insert(
                            (m.name.clone(), name.clone()),
                            format!("{t} {name} ON {name}.{foreign_key} = {own}.{owner_key}"),
                        );
                    }
                }
                // `has_many :through`: two hops, owner-side direction
                // (`Tag.joins(:stories)` → JOIN taggings ON tag_id, JOIN
                // stories ON story_id). Same fk resolution as the
                // through-reader lowering: the through association names
                // the join model; its source association, found by name
                // (`Model::through_source`), supplies the source fk when
                // it is a `belongs_to`.
                Association::HasMany { name, target, through: Some(thr_name), .. } => {
                    // A keyed hop is refused at synthesis; no join tail
                    // is better than one that joins on `id`.
                    if super::assoc_primary_key::unsupported_reason(m, models, a).is_some() {
                        continue;
                    }
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
                    let Some(Association::BelongsTo {
                        foreign_key: src_fk, primary_key: src_pk, polymorphic: false, ..
                    }) = thr_model.through_source(a)
                    else {
                        continue;
                    };
                    let thr_table = reg.table_for(thr_target);
                    let target_table = reg.table_for(target);
                    let src_pk = association_primary_key(models, target, src_pk.as_ref());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!(
                            "{thr_table} ON {thr_table}.{thr_fk} = {own}.id \
                             INNER JOIN {target_table} ON {target_table}.{src_pk} = {thr_table}.{src_fk}"
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
    scope_join_tails(&mut reg, models);
    // By name once every owner is down to the declaration it keeps, so a
    // model redeclaring a name is one declarer, not two disagreeing ones.
    for ((_, name), entry) in &reg.has_many_fk {
        reg.has_many_by_name
            .entry(name.clone())
            .and_modify(|t| {
                if t.as_ref() != Some(entry) {
                    *t = None;
                }
            })
            .or_insert_with(|| Some(entry.clone()));
    }
    // A declaration with no direct foreign key (`through:`, `has_one`,
    // `belongs_to`, `has_and_belongs_to_many`) is not in `has_many_fk`,
    // so the loop above never sees it. One direct `has_many :comments`
    // beside `has_many :comments, through: :posts` would otherwise look
    // unique, and the seed would filter `Comment` by `article_id` after
    // the controller rewrite had declined the same call. The name is
    // ambiguous. A direct `has_many` that merely cannot be seeded
    // (`primary_key:`, a row-changing scope) stays: it is in
    // `has_many_fk`, and `is_unseedable` is what refuses the seed.
    let hidden: Vec<Symbol> = reg
        .assoc_builds
        .iter()
        .filter(|((model, name), _)| !reg.has_many_fk.contains_key(&(model.clone(), name.clone())))
        .map(|((_, name), _)| name.clone())
        .collect();
    for name in hidden {
        if reg.has_many_by_name.contains_key(&name) {
            reg.has_many_by_name.insert(name, None);
        }
    }
    reg
}

/// A join into a model with a default scope carries the scope's
/// predicate in its ON clause, as Rails' association join does
/// (`Shelf.joins(:gadgets)` reads `… ON gadgets.shelf_id = shelves.id
/// AND gadgets.archived = FALSE`). A `through:` join touching a scoped
/// model, or a scope this cannot render (one read when the relation is
/// built), keeps no tail: the join is not composed rather than composed
/// without the scope.
fn scope_join_tails(reg: &mut AssocRegistry, models: &[Model]) {
    use super::default_scope::{default_scope, sql_predicate, DefaultScope};
    let scoped: HashMap<ClassId, Vec<(Symbol, Literal)>> = models
        .iter()
        .filter_map(|m| {
            default_scope(m).map(|s| match s {
                DefaultScope::Static(pairs) => (m.name.clone(), pairs),
                // No SQL here can read it: an empty list is the
                // "cannot render" arm below.
                DefaultScope::Dynamic(_) => (m.name.clone(), Vec::new()),
            })
        })
        .collect();
    if scoped.is_empty() {
        return;
    }
    let keys: Vec<((ClassId, Symbol), ClassId)> =
        reg.assoc_target.iter().map(|(k, t)| (k.clone(), t.clone())).collect();
    for ((owner, name), target) in keys {
        let through = models
            .iter()
            .find(|m| m.name == owner)
            .and_then(|m| {
                m.associations().find_map(|a| match a {
                    Association::HasMany { name: n, through: Some(thr), .. } if *n == name => {
                        m.associations().find_map(|t| match t {
                            Association::HasMany { name: tn, target: tt, .. } if tn == thr => Some(tt.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                })
            });
        let key = (owner.clone(), name.clone());
        let through_scoped = through.as_ref().is_some_and(|t| scoped.contains_key(t));
        let Some(preds) = scoped.get(&target) else {
            if through_scoped {
                reg.join_tails.remove(&key);
                reg.aliased_join_tails.remove(&key);
            }
            continue;
        };
        if through.is_some() {
            reg.join_tails.remove(&key);
            reg.aliased_join_tails.remove(&key);
            continue;
        }
        let table = reg.table_for(&target);
        let render = |alias: &str| -> Option<String> {
            if preds.is_empty() {
                return None;
            }
            preds
                .iter()
                .map(|(col, lit)| sql_predicate(alias, col, lit).map(|p| format!(" AND {p}")))
                .collect()
        };
        match (render(&table), render(name.as_str())) {
            (Some(plain), Some(aliased)) => {
                if let Some(tail) = reg.join_tails.get_mut(&key) {
                    tail.push_str(&plain);
                }
                if let Some(tail) = reg.aliased_join_tails.get_mut(&key) {
                    tail.push_str(&aliased);
                }
            }
            _ => {
                reg.join_tails.remove(&key);
                reg.aliased_join_tails.remove(&key);
            }
        }
    }
}

/// What the parent-ivar rewrite reads off one declaration (see
/// [`AssocRegistry::build_target`]). A `has_many` the rewrite's
/// `<Target>.new(fk => parent.id)` cannot reproduce has no expansion:
/// a custom `primary_key:` (the key matches another owner column),
/// `through:`, `as:` (the rows are keyed by the type column too). Its
/// scope is kept for each call to judge, since one that only orders
/// changes `find_by` and not `build` (Rails builds with a scope's
/// `where` values; see [`scope_is_build_neutral`]). A scope lambda
/// taking the owner is dropped at ingest, so it reads as no scope here.
/// Nor does any other kind: a `has_one` or `belongs_to` reader is never
/// a collection proxy, so `.build` / `.find` on it is not an association
/// call, and a `has_and_belongs_to_many` builds through its join table.
fn assoc_build(a: &Association, models: &[Model]) -> AssocBuild {
    match a {
        Association::HasMany { target, foreign_key, through, as_interface, scope, extension, .. } => {
            let reproducible = through.is_none() && as_interface.is_none() && a.owner_key().as_str() == "id";
            AssocBuild {
                expansion: reproducible.then(|| (target.clone(), foreign_key.clone())),
                scope: scope.clone(),
                extension: extension.iter().map(|x| x.name.clone()).collect(),
                // `self.primary_key = "id"` names the default key.
                target_keyed_by_id: models.iter().any(|m| {
                    &m.name == target && m.primary_key.as_ref().is_none_or(|key| key.as_str() == "id")
                }),
            }
        }
        _ => AssocBuild { expansion: None, scope: None, extension: Vec::new(), target_keyed_by_id: false },
    }
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

/// Literal reflective hops on Rails' PUBLIC generated association/scope
/// surface must be visible to the ordinary relation-threading pass.
/// Never erase reflection on arbitrary user methods: `send` can call a
/// private helper where `public_send` and a direct receiver call cannot.
/// Ingest refuses visibility changes on generated DSL methods without a
/// local MethodDef; local overrides are vetoed by `app_method` below.
pub fn ground_literal_model_dispatch(expr: &mut Expr, app: &crate::App, assocs: &AssocRegistry) {
    expr.node
        .for_each_child_mut(&mut |child| ground_literal_model_dispatch(child, app, assocs));
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &mut *expr.node
    else {
        return;
    };
    if !matches!(method.as_str(), "send" | "__send__" | "public_send") {
        return;
    }
    let Some(Expr { node, .. }) = args.first() else {
        return;
    };
    let name = match &**node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => value.clone(),
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Symbol::from(value.as_str()),
        _ => return,
    };
    let Some(ty) = recv.ty.as_ref().map(|ty| ty.peel_nilable()) else {
        return;
    };
    let (id, association) = match ty {
        crate::ty::Ty::Class { id, .. } => (id, !matches!(&*recv.node, ExprNode::Const { .. })),
        crate::ty::Ty::Relation { of } => (of, false),
        crate::ty::Ty::Array { elem } => {
            let crate::ty::Ty::Class { id, .. } = elem.peel_nilable() else {
                return;
            };
            // An arbitrary Array[Model] is not a Rails collection proxy.
            // Only the association read the seed arm can reproduce is.
            let ExprNode::Send {
                recv: Some(owner),
                method: aname,
                args: aargs,
                block: None,
                ..
            } = &*recv.node
            else {
                return;
            };
            if !aargs.is_empty() {
                return;
            }
            let owner_id = owner.ty.as_ref().and_then(|ty| match ty.peel_nilable() {
                crate::ty::Ty::Class { id, .. } => Some(id),
                _ => None,
            });
            if !owner_id
                .and_then(|owner| assocs.has_many_fk(owner, aname))
                .is_some_and(|(target, _)| target == id)
            {
                return;
            }
            // Replacing the read with a relation seed bypasses this
            // reader. It is safe only while the macro-generated reader
            // still owns the name throughout the recorded ancestry.
            if owner_id.is_some_and(|owner| {
                app_method(app, owner, aname, crate::dialect::MethodReceiver::Instance)
            }) {
                return;
            }
            (id, false)
        }
        _ => return,
    };
    let Some(model) = app.models.iter().find(|m| &m.name == id) else {
        return;
    };
    let side = if association {
        crate::dialect::MethodReceiver::Instance
    } else {
        crate::dialect::MethodReceiver::Class
    };
    // Both calls being collapsed must still be framework-owned. A
    // dispatcher override changes `send` itself; a target override means
    // the literal does not select the macro-generated public method.
    if app_method(app, id, method, side) || app_method(app, id, &name, side) {
        return;
    }
    let generated = if association {
        args.len() == 1 && model.associations().any(|a| a.name() == &name)
    } else {
        model.scopes().any(|s| s.name == name)
    };
    if generated {
        *method = name;
        args.remove(0);
    }
}

/// Whether app metadata proves that `id`'s lookup chain contains a
/// user-defined method. This deliberately walks models, library classes,
/// all reopenings, includes, parents and initializer-installed mixins.
/// Unknown framework roots terminate the walk.
pub(crate) fn app_method(
    app: &crate::App,
    id: &ClassId,
    name: &Symbol,
    side: crate::dialect::MethodReceiver,
) -> bool {
    fn visit(
        app: &crate::App,
        id: &ClassId,
        name: &Symbol,
        side: crate::dialect::MethodReceiver,
        seen: &mut HashSet<ClassId>,
    ) -> bool {
        if !seen.insert(id.clone()) {
            return false;
        }
        for model in app.models.iter().filter(|m| &m.name == id) {
            if model
                .methods()
                .any(|m| m.name == *name && m.receiver == side)
            {
                return true;
            }
            // Not the framework's public surface once the app changed its visibility.
            if crate::lower::model_to_library::inherited_visibility(model)
                .iter()
                .any(|(class_side, n, _)| *n == name.as_str() && *class_side == (side == crate::dialect::MethodReceiver::Class))
            {
                return true;
            }
            if crate::analyze::model_includes(model)
                .iter()
                .any(|inc| visit(app, inc, name, side, seen))
            {
                return true;
            }
            if model
                .parent
                .as_ref()
                .is_some_and(|p| visit(app, p, name, side, seen))
            {
                return true;
            }
        }
        for class in app.library_classes.iter().filter(|c| &c.name == id) {
            if class
                .methods
                .iter()
                .any(|m| m.name == *name && m.receiver == side)
            {
                return true;
            }
            if class
                .includes
                .iter()
                .any(|inc| visit(app, inc, name, side, seen))
            {
                return true;
            }
            if class
                .parent
                .as_ref()
                .is_some_and(|p| visit(app, p, name, side, seen))
            {
                return true;
            }
        }
        // Both retained `include` and `prepend` change instance lookup,
        // not the singleton side. Their order cannot make the proof safer:
        // the optimization needs every represented override to be absent.
        side == crate::dialect::MethodReceiver::Instance
            && app.module_mixins.iter()
                .filter(|m| m.target == id.0)
                .any(|m| visit(app, &ClassId(m.module.clone()), name, side, seen))
    }
    visit(app, id, name, side, &mut HashSet::default())
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
            // `<<` too, onto a has_many a model holds (`__push_<name>`).
            if matches!(method.as_str(), "build" | "create" | "create!") || method.as_str() == "<<" {
                if let ExprNode::Send { method: aname, args, .. } = &*r.node {
                    let named = if method.as_str() == "<<" { assocs.is_held_name(aname) } else { assocs.is_has_many_name(aname) };
                    if args.is_empty() && named {
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
    let mut names = HashSet::default();
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
            // …and every other chain method an Array does not answer.
            // Current lobsters pages its inbox with
            // `@user.notifications.offset(n).limit(25).order(…)`; with
            // only `where` admitted, a chain that STARTS elsewhere was
            // left on the has_many reader's Array and died on `offset`.
            // `select` stays out: Array answers it (with a block), so
            // the site could mean either.
            let is_where = (method.as_str() == "where" && !args.is_empty() && block.is_none())
                || (block.is_none()
                    && matches!(
                        method.as_str(),
                        "offset" | "limit" | "order" | "reorder" | "joins" | "left_outer_joins"
                            | "left_joins" | "includes" | "preload" | "eager_load" | "references" | "group"
                            | "having" | "merge" | "unscope"
                    ));
            if is_where
                || is_relation_terminal(method.as_str(), args, block.as_ref())
                    && matches!(
                        method.as_str(),
                        "find"
                            | "find_by"
                            | "find_by!"
                            | "find_sole_by"
                            | "destroy_by"
                            | "delete_by"
                            | "destroy_all"
                            | "delete_all"
                            | "update_all"
                            | "find_each"
                            | "find_in_batches"
                            | "touch_all"
                            | "to_sql"
                            | "in_batches"
                            | "maximum"
                            | "minimum"
                            // Array answers neither: acts_as_follower's `follows.find_or_create_by(…)`
                            | "find_or_create_by"
                            | "find_or_initialize_by"
                            | "create_or_find_by"
                            | "create_or_find_by!"
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
                if aargs.is_empty() && assocs.is_assoc_extension(None, aname, method) {
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
    scope_names: &HashSet<Symbol>,
    acm: &AssocClassMethods,
) -> bool {
    if acm.is_empty() {
        return false;
    }
    let mut demand: HashSet<(Symbol, Symbol)> = HashSet::default();
    collect_assoc_method_demand(expr, assocs, scope_names, &mut demand);
    demand
        .iter()
        .any(|(_, m)| acm.values().any(|per_model| per_model.contains_key(m)))
}

/// True when a model constant starts a Relation chain or a terminal that
/// requires a Relation seed; used by both whole-app and per-body gates.
pub fn mentions_model_chain_start(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*e.node {
            // Chain methods and `all` open a chain; so do the terminals
            // that need a seeded Relation at a Const root. Leaving the
            // latter out is a gate that closes over the very shape
            // `CLASS_ROOT_TERMINALS` exists to rewrite — a body whose
            // ONLY relation surface is `Push::Subscription.destroy_by(…)`
            // never reached the rewriter at all. Likewise the counted
            // `Developer.first(2)` / `.last(2)`.
            if (is_relation_chain_method(method.as_str())
                || method.as_str() == "all"
                || CLASS_ROOT_TERMINALS.contains(&method.as_str())
                || counted_terminal(method, args, block.as_ref()).is_some())
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

/// The columns `<Model>.insert_all!(rows, returning: …)` reads back, when
/// `returning:` is left out (the primary key) or is a literal column list.
/// `None` declines the inlining.
fn insert_all_bang_returning(arg: Option<&Expr>) -> Option<Vec<String>> {
    match arg.map(|a| &*a.node) {
        None => Some(vec!["id".to_string()]),
        Some(ExprNode::Hash { entries, .. }) if entries.len() == 1 => {
            let (k, v) = &entries[0];
            let key_ok = matches!(&*k.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "returning");
            match (&*v.node, key_ok) {
                (ExprNode::Array { elements, .. }, true) => elements
                    .iter()
                    .map(|e| match &*e.node {
                        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            }
        }
        _ => None,
    }
}

/// True when `scope_chain` inlines `<recv>.insert_all(args)` /
/// `insert_all!(args)`: `recv` names a model, as `const_model` reads it,
/// and the call has the shape the inlining takes. The strict-target
/// survey reports only the forms this declines, which reach the
/// Ruby-family runtime's methods.
pub fn inlines_insert_all(recv: &Expr, method: &str, args: &[Expr], has_block: bool, models: &HashSet<ClassId>) -> bool {
    !has_block
        && const_model(recv, models).is_some()
        && match method {
            "insert_all" => args.len() == 1,
            "insert_all!" => (1..=2).contains(&args.len()) && insert_all_bang_returning(args.get(1)).is_some(),
            _ => false,
        }
}

/// True when `expr` calls `<Model>.insert_all(rows)` (or `insert_all!`). Gate for the
/// call-site expansion in `rewrite_send`.
pub fn mentions_model_insert_all(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
            if ((method.as_str() == "insert_all" && args.len() == 1)
                || (method.as_str() == "insert_all!" && (1..=2).contains(&args.len())))
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
        if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
            let self_rooted = match recv {
                None => true,
                Some(r) => matches!(&*r.node, ExprNode::SelfRef),
            };
            if self_rooted
                && (is_relation_chain_method(method.as_str())
                    || method.as_str() == "all"
                    || (recv.is_none() && counted_terminal(method, args, block.as_ref()).is_some()))
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
/// receiver and the chain keeps its model. Membership is the catalog's
/// [`crate::catalog::ChainKind::Builder`] names plus [`crate::catalog::BUILDER_ALIASES`]
/// (`left_joins`, `where!`, `order!`, `without`). Terminals / Enumerable
/// methods (`to_a`/`first`/`map`/`pluck`/…) are absent: they end the
/// relation, so model-tracking stops after them (a scope can't follow).
///
/// `all` is a terminal in the catalog and is checked separately at the
/// call sites that seed a relation from it.
pub(crate) fn is_relation_chain_method(name: &str) -> bool {
    crate::catalog::is_relation_builder_name(name)
}

/// True when `e` is a Relation this pass has already rooted — the
/// threaded `__rel`, a local holding one, `ActiveRecord::Relation.new`,
/// or a chain hop off either. Gates `count > n` → `more_than?(n)` so
/// `Array#count` is left alone.
fn is_relation_expr(e: &Expr, ctx: &Ctx, locals: &Locals) -> bool {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            locals.rel.contains_key(name)
                || ctx.scope_body.as_ref().is_some_and(|(_, rel)| rel == name)
        }
        ExprNode::Ivar { name } => locals.rel.contains_key(&alias_key(name, true)),
        ExprNode::Send { method, recv: Some(r), args, block: None, .. } => {
            if let Some(m) = const_model(r, ctx.models) {
                if ctx.scope_of(&m, method) {
                    return true;
                }
            }
            let name = method.as_str();
            if name == "new"
                && args.len() == 1
                && matches!(
                    &*r.node,
                    ExprNode::Const { path }
                        if path.len() == 2
                            && path[0].as_str() == "ActiveRecord"
                            && path[1].as_str() == "Relation"
                )
            {
                return true;
            }
            let hop = is_relation_chain_method(name)
                || crate::catalog::is_relation_context_builder(name)
                || matches!(name, "where_scope" | "preloaded" | "skip_preloading!" | "page" | "per");
            hop && is_relation_expr(r, ctx, locals)
        }
        _ => false,
    }
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
    /// [`Registries::owner_objects`].
    pub owner_objects: bool,
}

impl Ctx<'_> {
    /// Does a call to scope `method` on `model` keep the relation? False
    /// for a scope proved to answer some other value
    /// ([`UserMethodReturns::non_relation_scopes`]).
    fn scope_keeps_relation(&self, model: &ClassId, method: &Symbol) -> bool {
        !self
            .user_returns
            .non_relation_scopes
            .contains(&(model.clone(), method.clone()))
    }
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
            owner_objects: self.owner_objects,
            user_returns: self.user_returns,
        }
    }
}

/// The records `<<` adds when it's given an Array: an Array literal's
/// elements, nested literals flattened, an Array of records as it is, and
/// anything not typed as one record `[value].flatten`, which is the record
/// alone or the Array's records. None for one record.
fn pushed_records(arg: &Expr) -> Option<Expr> {
    use crate::ty::Ty;
    fn flat(e: &Expr, out: &mut Vec<Expr>) {
        match &*e.node {
            ExprNode::Array { elements, .. } => elements.iter().for_each(|x| flat(x, out)),
            _ => out.push(e.clone()),
        }
    }
    match (&*arg.node, arg.ty.as_ref()) {
        (ExprNode::Array { style, .. }, _) => {
            let mut elements = Vec::new();
            flat(arg, &mut elements);
            let elem = elements.first().and_then(|e| e.ty.clone());
            let mut list = Expr::new(arg.span, ExprNode::Array { elements, style: style.clone() });
            if let Some(elem) = elem {
                list.ty = Some(Ty::Array { elem: elem.into() });
            }
            Some(list)
        }
        (_, Some(Ty::Class { .. })) => None,
        (_, Some(Ty::Array { elem })) if matches!(&**elem, Ty::Class { .. }) => Some(arg.clone()),
        _ => {
            let wrapped = syn(arg.span, ExprNode::Array { elements: vec![arg.clone()], style: Default::default() });
            Some(syn(
                arg.span,
                ExprNode::Send { recv: Some(wrapped), method: Symbol::from("flatten"), args: vec![], block: None, parenthesized: false },
            ))
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
    // A callee with a REST param (`*tags`) cannot take `__rel` as a
    // trailing optional positional — an optional positional can't
    // follow a splat, which is why `insert_rel_param` makes `__rel` a
    // keyword param instead for exactly this shape. Thread it the same
    // way here: no positional padding (there is no fixed slot to pad
    // toward — Ruby fills a bare call's REST with nothing, not a
    // specific param), and pass the relation as `__rel: rel`, a keyword
    // argument.
    let has_rest = leading.is_some_and(|ps| ps.iter().any(|p| p.rest && !p.keyword));

    // A trailing kwargs hash (`base(user, unmerged: unmerged)`) binds
    // the scope's KEYWORD params; the relation is a positional and must
    // land before it. Split it off, pad, thread, re-append.
    //
    // ONLY when the callee declares keywords (or, now, has a rest param
    // — its `__rel` is itself a keyword, so the same split applies).
    // Ruby hands `f(a: 1)` to a `def f(h)` as the POSITIONAL hash `h` —
    // which is how campfire's `messages.create_with_attachment!(creator:,
    // attachment:)` reaches its one `attributes` param, and how a
    // `**opts` param (ingested as a positional defaulting to `{}`) is
    // fed. Splitting it off there padded that param with `nil` and
    // pushed the hash PAST `__rel`, handing three positionals to a
    // method taking one or two. Left in place it is an ordinary
    // argument, so it counts toward the padding below — and its
    // `kwargs` flag has to go, or the emitter renders it bare and Ruby
    // reads keywords ahead of the `__rel` positional.
    let takes_keywords = has_rest || leading.map_or(true, |ps| ps.iter().any(|p| p.keyword));
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

    if has_rest {
        // No padding: the pad loop below doesn't exclude a rest param
        // itself (it only filters out `p.keyword`), so a bare chained
        // call to a rest callee — `widget.active.those_tagged` — used
        // to pad a spurious positional `nil` into `*tags` before
        // appending the relation. Skipping padding entirely for a rest
        // callee fixes that latent bug along with the syntax error:
        // `Widget.those_tagged(__rel: Widget.active)`, not
        // `Widget.those_tagged(nil, __rel: Widget.active)`.
        let rel_key = syn(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("__rel") } });
        let mut entries: Vec<(Expr, Expr)> = match kwargs_tail {
            Some(tail) => {
                let ExprNode::Hash { entries, .. } = *tail.node else {
                    unreachable!("kwargs_tail is always a kwargs Hash")
                };
                entries
            }
            None => Vec::new(),
        };
        entries.push((rel_key, rel));
        args.push(syn(span, ExprNode::Hash { entries, kwargs: true }));
        return args;
    }

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
/// `pluck`, `pick`, and `ids` have no such home: they were left as a
/// call on the class, which no class answers. `pick` is delegated the
/// same way as `pluck`. A source `def self.pick` is left on the class
/// before this list is consulted.
///
/// `destroy_by` / `delete_by` join them for exactly that reason. Both
/// are `where` + a write, the arel pass claims neither at a Const root,
/// and `Base` defines neither — so campfire's
/// `Push::Subscription.destroy_by(endpoint:, user_id:)` reached nothing
/// at all, with the analyzer saying so (`send_dispatch_failed: no known
/// method `destroy_by` on Class { Push::Subscription }`).
///
/// `maximum` / `minimum` likewise: `Base` defines neither, and the arel
/// pass leaves them to `Relation`'s SQL extrema.
const CLASS_ROOT_TERMINALS: &[&str] = &[
    "pluck", "pick", "ids", "destroy_by", "delete_by", "maximum", "minimum",
];

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
        "find_by" | "find_by!" | "find_sole_by" => block.is_none(),
        // Bulk WRITES are terminals too — they end the chain and answer
        // a count / an Array of destroyed records, never a relation.
        // `Array` answers none of them, so an association-read receiver
        // is a NoMethodError today and a scoped statement after
        // (campfire's `memberships.destroy_by user: users`).
        "destroy_by" | "delete_by" => block.is_none(),
        // Batch iteration: a query Rails runs against the association's
        // relation (`room.messages.find_each(&:destroy)`), where the
        // association read alone is an Array with no `find_each`.
        "find_each" | "find_in_batches" => true,
        _ => matches!(
            name,
            "ids" | "pluck" | "count" | "first" | "last" | "exists?" | "any?" | "empty?" | "size"
                | "length"
                | "destroy_all"
                | "delete_all"
                | "update_all"
                | "touch_all"
                | "to_sql"
                | "in_batches"
                | "maximum"
                | "minimum"
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
                // The same lookup, building rather than saving on a miss:
                // `Relation#find_or_initialize_by` gives the new record the
                // scope's attributes, as Rails' does through an
                // association. Campfire's push subscription controller
                // looks its endpoint up on a parked `Current.user
                // .push_subscriptions` read this way since c8f74a0.
                | "find_or_initialize_by"
                | "create_or_find_by"
                | "create_or_find_by!"
        ),
    }
}

/// `first(n)` / `last(n)` on a relation are different methods from the
/// bare forms — they answer an Array of up to n records where `first`
/// answers one record or nil. The runtime splits them (`first_n` /
/// `last_n`). The spelling is [`crate::catalog::counted_relation_terminal`].
///
/// Only ever called where the receiver has already been proven to be a
/// relation. That gate is the whole point: `Array#first(n)` and
/// `String#split.last(n)` mean the same thing Rails does and must be
/// left alone — lobsters' `parsed.to_html.split.first(words * 2)` is
/// exactly the shape a receiver-blind rename would corrupt.
///
/// A block form is excluded: `first { … }` is Enumerable#detect, a
/// different method again.
pub(crate) fn counted_terminal(method: &Symbol, args: &[Expr], block: Option<&Expr>) -> Option<Symbol> {
    crate::catalog::counted_relation_terminal(method.as_str(), args.len(), block.is_some())
        .map(Symbol::from)
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
/// Only consulted when the owner's stamped type is neither a model nor
/// a union of models. The guard is two-sided: the name must be an
/// ingested model AND that model (itself or a superclass) must declare
/// this association (`AssocRegistry::declaring_class_by_name`), so
/// `@room.messages` resolves to
/// (Message, room_id) while `@user.messages` resolves to
/// (Message, creator_id) — each right, neither guessed. That is also
/// why the same rule can read a local (`user.sessions`) and a one-hop
/// read (`Current.user.memberships`): a name that isn't a model, or a
/// model that doesn't declare the collection, answers None and the
/// chain keeps its source shape.
fn owner_model_from_name<'a>(owner: &Expr, assocs: &'a AssocRegistry) -> Option<&'a ClassId> {
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
    assocs.tables.get_key_value(&id).map(|(model, _)| model)
}

/// The `(owner model, target, foreign key)` a `<owner>.<has_many>` read
/// resolves to — the three rungs above (a stamped model, or the model
/// the owner's name gives, is read up its superclass chain, and the
/// class declaring the association is the owner model; a stamped union
/// of models needs one such class for all of them), with no judgment
/// about whether the seed can REPRODUCE the association (that is
/// `AssocRegistry::is_unseedable`, and only the rewriter needs it).
/// The controller's parent-ivar rewrite resolves the same way
/// (`AssocRegistry::build_target`), except that a union there needs
/// only the same target and key on every arm, as it names no owner.
///
/// Public because the strong-params binding scan asks the same question
/// for a different purpose: `@room.messages.create_with_attachment!(
/// message_params)` names `Message.create_with_attachment!`'s parameter
/// exactly as `Message.create_with_attachment!(message_params)` would,
/// and one resolver keeps the two answers from drifting.
pub fn assoc_read_target(
    owner: &Expr,
    aname: &Symbol,
    assocs: &AssocRegistry,
) -> Option<(Option<ClassId>, ClassId, Symbol)> {
    // An owner typed as a model reads its own chain's declaration and
    // nothing else: a model none of whose chain declares the name does
    // not have it, and the other two rungs would hand it another
    // model's foreign key (`@tag.comments` filtered by `article_id`).
    // One typed as a union of models reads the declaration they all
    // share, or none, once its name has had the chance to pick the arm
    // it is (`owner_models`).
    if let Some(owners) = assocs.owner_models(owner, aname) {
        let mut classes = owners.into_iter().map(|m| assocs.declaring_class(m, aname));
        let class = classes.next().flatten()?;
        if !classes.all(|c| c == Some(class)) {
            return None;
        }
        let (target, fk) = assocs.has_many_fk(class, aname)?.clone();
        return Some((Some(class.clone()), target, fk));
    }
    // The name rung decides when it names a declaring model, so the
    // seed and the rewrite never resolve one owner differently.
    if let Some(class) = assocs.declaring_class_by_name(owner, aname) {
        let (target, fk) = assocs.has_many_fk(class, aname)?.clone();
        return Some((Some(class.clone()), target, fk));
    }
    assocs.has_many_by_name(aname).cloned().map(|(target, fk)| (None, target, fk))
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
pub(crate) fn owner_reads_once(owner: &Expr) -> bool {
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

#[allow(clippy::too_many_arguments)]
fn build_association_seed(
    span: crate::span::Span,
    target: &ClassId,
    fk: Symbol,
    owner_id: Expr,
    type_entry: Option<(Expr, Expr)>,
    owner: &Expr,
    name: &Symbol,
    scope_attributes: bool,
) -> Expr {
    // An owner key other than `id` can be NULL, and
    // `where(fk: nil)` would match the child rows
    // whose fk is NULL. Rails never matches a nil
    // owner key, so the seed excludes them.
    let keyed_guard = !matches!(
        &*owner_id.node,
        ExprNode::Ivar { name } if name.as_str() == "id"
    ) && !matches!(
        &*owner_id.node,
        ExprNode::Send { method, .. } if method.as_str() == "id"
    );
    let guard_sql = format!("{} IS NOT NULL", fk.as_str());
    let fk_hash = syn(
        span,
        ExprNode::Hash {
            entries: std::iter::once((
                syn(
                    span,
                    ExprNode::Lit { value: Literal::Sym { value: fk } },
                ),
                owner_id,
            ))
            .chain(type_entry.clone())
            .collect(),
            kwargs: true,
        },
    );
    let seed_method =
        if scope_attributes { "where_scope" } else { "where" };
    let mut seed = syn(
        span,
        ExprNode::Send {
            recv: Some(relation_new(span, target)),
            method: Symbol::from(seed_method),
            args: vec![fk_hash],
            block: None,
            parenthesized: true,
        },
    );
    if keyed_guard {
        seed = syn(
            span,
            ExprNode::Send {
                recv: Some(seed),
                method: Symbol::from("where"),
                args: vec![syn(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str { value: guard_sql },
                    },
                )],
                block: None,
                parenthesized: true,
            },
        );
    }
    // The seed REPLACES `<owner>.<assoc>`, and with
    // it the reader's eager-load cache: a scope
    // reached through a preloaded association
    // (`message.boosts.ordered`, with `includes
    // (boosts: :booster)` upstream) re-queried per
    // record -- the room page's last N+1. So the
    // relation is handed the cache too:
    // `.preloaded(<owner>.<assoc>_target,
    // <owner>.<assoc>_loaded?)` seeds its loaded
    // memo when the association was loaded and is a
    // no-op when it was not. An order-only scope
    // then sorts the memo in memory (Relation#order);
    // a filtering chain drops it and queries, as
    // before. Not for the create-scope spelling,
    // which builds a record rather than reading.
    if !scope_attributes {
        let owner_send = |m: String| {
            syn(
                span,
                ExprNode::Send {
                    recv: Some(owner.clone()),
                    method: Symbol::from(m),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )
        };
        seed = syn(
            span,
            ExprNode::Send {
                recv: Some(seed),
                method: Symbol::from("preloaded"),
                args: vec![
                    owner_send(format!("{}_target", name.as_str())),
                    owner_send(format!("{}_loaded?", name.as_str())),
                ],
                block: None,
                parenthesized: true,
            },
        );
    }
    seed
}

fn assoc_owner_seed(
    ctx: &Ctx,
    aname: &Symbol,
    owner: &Expr,
    span: crate::span::Span,
) -> Option<(ClassId, Symbol, Expr)> {
    // Resolution is `assoc_read_target`'s three rungs — the owner's
    // stamped type (which disambiguates an assoc name declared on
    // several models, and alone decides for a model), then the owner's
    // NAME (which alone decides when it names a model declaring the
    // association), then the assoc name when it is unique across models.
    //
    // What is this function's OWN judgment is the seedability filter:
    // each rung carries the OWNER it resolved through so the check can
    // ask about that exact declaration; the by-NAME rung has no owner
    // and is checked against every declarer.
    assoc_read_target(owner, aname, ctx.assocs)
        .filter(|(owner_model, _, _)| !ctx.assocs.is_unseedable(owner_model.as_ref(), aname))
        .map(|(owner_model, target, fk)| {
            let owner_id = syn(
                span,
                ExprNode::Send {
                    recv: Some(owner.clone()),
                    method: ctx.assocs.has_many_owner_key(owner_model.as_ref(), aname),
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
/// indexes skips a row conflicting on either. A partial index adds its
/// predicate, `.where("(revoked_at IS NULL)")`, so only a row it covers
/// counts as a conflict. A model with no usable unique key (none
/// declared, or every one of them nullable — see [`UniqueKeys`]) keeps
/// the bare save: there is nothing to conflict on that this can read.
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
        // `<Model>.where(col: __attrs[:col], …).exists?`, with a partial
        // index's predicate as a second `where`.
        let entries = key
            .columns
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
        let mut where_call = syn(
            span,
            ExprNode::Send {
                recv: Some(relation_new(span, model)),
                method: Symbol::from("where"),
                args: vec![syn(span, ExprNode::Hash { entries, kwargs: true })],
                block: None,
                parenthesized: true,
            },
        );
        if let Some(predicate) = &key.predicate {
            where_call = syn(
                span,
                ExprNode::Send {
                    recv: Some(where_call),
                    method: Symbol::from("where"),
                    args: vec![syn(
                        span,
                        ExprNode::Lit { value: Literal::Str { value: predicate.clone() } },
                    )],
                    block: None,
                    parenthesized: true,
                },
            );
        }
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
/// endless range). The ruby-family runtime's `column_predicate` renders
/// a Range too, for the dynamic call sites no lowering reaches, but the
/// strict targets' runtimes have no Range: compared equal to a column,
/// a range matches nothing — silently, which is the worst kind.
///
/// Converted at the CALL SITE rather than taught to every runtime
/// because there is no `Ty::Range` in the type system and no strict
/// target ships a Range. Here the range is a literal, so what
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
    // Not bare: a column named like a keyword (`order`) is a syntax error unquoted, and the Hash form the runtime renders quotes it.
    let col = format!("\"{table}\".\"{key}\"");
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
        clauses.push(bound_clause(col, ">=", b, binds)?);
    }
    if let Some(x) = end {
        // `a...b` excludes its end; `a..b` includes it.
        clauses.push(bound_clause(col, if *exclusive { "<" } else { "<=" }, x, binds)?);
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

/// One end's comparison. An end that may be nil at run time is no bound
/// there, as Rails reads `nil..b`: `(? IS NULL OR col >= ?)`, which binds
/// it twice, so only a plain read takes it and any other declines.
fn bound_clause(col: &str, op: &str, bound: &Expr, binds: &mut Vec<Expr>) -> Option<String> {
    let nilable = match bound.ty.as_ref() {
        Some(crate::ty::Ty::Nil) => true,
        Some(crate::ty::Ty::Union { variants }) => variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil)),
        _ => false,
    };
    if !nilable {
        binds.push(bound.clone());
        return Some(format!("{col} {op} ?"));
    }
    if !matches!(&*bound.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
        return None;
    }
    binds.push(bound.clone());
    binds.push(bound.clone());
    Some(format!("(? IS NULL OR {col} {op} ?)"))
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

/// The `<name>_type` value a polymorphic condition uses for a record that is
/// none of the association's implementors: matches no stored row.
const POLYMORPHIC_NO_TYPE: &str = "<not a polymorphic type>";

fn lower_relation_args(
    model: &ClassId,
    method: &Symbol,
    args: &mut Vec<Expr>,
    ctx: &Ctx,
) -> Vec<Symbol> {
    let mut aliases: Vec<Symbol> = Vec::new();
    // Not by the written name: lobsters' Search builds its query with `where!` and `joins!`, which left `story:` and `:user` unresolved.
    let canonical = crate::catalog::BUILDER_ALIASES
        .iter()
        .find(|(alias, _)| *alias == method.as_str())
        .map_or(method.as_str(), |(_, name)| name);
    // Not handed to the runtime as an Array: Rails flattens `order([a, b])` into `order(a, b)`, and the runtime's parts are the elements.
    if matches!(canonical, "order" | "reorder")
        && args.iter().any(|a| matches!(&*a.node, ExprNode::Array { .. }))
    {
        let mut flat = Vec::with_capacity(args.len());
        for arg in args.drain(..) {
            match &*arg.node {
                ExprNode::Array { elements, .. } => flat.extend(elements.iter().cloned()),
                _ => flat.push(arg),
            }
        }
        *args = flat;
    }
    match canonical {
        "joins" | "left_outer_joins" | "left_joins" => {
            let kind = if canonical == "joins" { "INNER JOIN" } else { "LEFT OUTER JOIN" };
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
        "where" | "not" | "find_by" | "find_by!" | "find_sole_by" | "destroy_by" | "delete_by"
        | "exists?" => {
            // `where(connected_at: TTL.ago..)` — a RANGE value. Rails
            // renders `>=` / `<=` / `<` / BETWEEN. Only the
            // ruby-family runtime's `column_predicate` has a Range arm;
            // compared for equality anywhere else, a range matches
            // nothing and says nothing. Converted HERE, where the range
            // is a literal, because the alternative is teaching every
            // target's runtime what a Range is — there is no `Ty::Range`
            // and no strict target ships one.
            //
            // Single-entry hashes only: a mixed hash would have to
            // compose the fragment with the other keys' predicates, and
            // half a conversion is worse than none (the multi-key form
            // keeps its current, honest, matches-nothing behavior until
            // a corpus app writes one).
            if matches!(canonical, "where" | "not" | "find_by" | "find_by!")
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
                hoist_nested_association_tables(model, entries, ctx);
                let mut type_conditions: Vec<(Expr, Expr)> = Vec::new();
                // (value, type column) of each polymorphic condition whose
                // value may be nil at run time; see the guard after the loop.
                let mut nil_guards: Vec<(Expr, Symbol)> = Vec::new();
                for (k, v) in entries.iter_mut() {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                        continue;
                    };
                    // A POLYMORPHIC belongs_to also constrains its type
                    // column: `where(voter: user)` is `voter_id = u.id AND
                    // voter_type = 'User'`. The type value is the SAME
                    // `case value when Target then "Target"` switch the
                    // polymorphic writer stores with (shared
                    // `polymorphic_type_switch`), so read and write cannot
                    // disagree: an STI subclass lands in its implementor's
                    // arm. A literal nil is IS NULL on both columns, as the
                    // plain `where(user: nil)` path. The value is read more
                    // than once, so only an effect-free read qualifies. Where
                    // the type half cannot be built (no resolved implementors
                    // — framework models such as ActionText::RichText#record
                    // — or a non-simple value) the key falls through to the
                    // plain rename to `<name>_id` below: the type stays
                    // unconstrained, exactly as before this lowering existed,
                    // never a reference to a column that does not exist.
                    if let Some((type_col, targets)) = ctx.assocs.polymorphic_belongs_to(model, key) {
                        let simple = match &*v.node {
                            ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef => true,
                            ExprNode::Send { recv: None, args, block: None, .. } => args.is_empty(),
                            _ => false,
                        };
                        if let Some(fk) = ctx.assocs.belongs_to_fk(model, key) {
                            if matches!(&*v.node, ExprNode::Lit { value: Literal::Nil }) {
                                // Rails: `where(voter: nil)` is `voter_id IS NULL`
                                // alone — no type condition (verified against
                                // Rails 8.1), so only the key is renamed.
                                *k.node = ExprNode::Lit { value: Literal::Sym { value: fk.clone() } };
                                continue;
                            }
                            if simple && !targets.is_empty() {
                                let type_value = crate::lower::model_to_library::polymorphic_type_switch(
                                    v.clone(),
                                    targets,
                                    |t| syn(span, ExprNode::Lit { value: Literal::Str { value: t.0.as_str().to_string() } }),
                                    // A value that is none of the implementors can
                                    // match no row (Rails compares to its
                                    // `polymorphic_name`, which no stored row has).
                                    // The writer stores a class name, or `""` for
                                    // nil; this sentinel is neither — `<` and a
                                    // space cannot appear in a constant name — so
                                    // it can collide with nothing, and it is not
                                    // nil, so it is not `IS NULL` either.
                                    Some(syn(span, ExprNode::Lit {
                                        value: Literal::Str { value: POLYMORPHIC_NO_TYPE.to_string() },
                                    })),
                                );
                                type_conditions.push((
                                    syn(span, ExprNode::Lit { value: Literal::Sym { value: type_col.clone() } }),
                                    type_value,
                                ));
                                nil_guards.push((v.clone(), type_col.clone()));
                                *k.node = ExprNode::Lit { value: Literal::Sym { value: fk.clone() } };
                                continue;
                            }
                        }
                    }
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
                    let Some(fk) = ctx.assocs.belongs_to_fk(model, key) else {
                        // Not this model's column: Rails reads `where(tags: tags)` on a has_many as the associated table's key (`tags.id IN (…)`).
                        if let Some(table) = ctx.assocs.assoc_table(model, key) {
                            let id = syn(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("id") } });
                            let value = std::mem::replace(v, syn(span, ExprNode::Lit { value: Literal::Nil }));
                            *v = syn(span, ExprNode::Hash { entries: vec![(id, value)], kwargs: false });
                            *k.node = ExprNode::Lit { value: Literal::Sym { value: table.clone() } };
                        }
                        continue;
                    };
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
                    // Not an Array of scalars: a list of plain ids (`find_by(article: ids)`) has no `.id` to read, and the runtime predicate takes it as is.
                    if matches!(&*v.node, ExprNode::Var { .. } | ExprNode::Ivar { .. })
                        && match v.ty.as_ref() {
                            Some(crate::ty::Ty::Array { elem }) => !matches!(
                                &**elem,
                                crate::ty::Ty::Int | crate::ty::Ty::Str | crate::ty::Ty::Float | crate::ty::Ty::Sym | crate::ty::Ty::Bool
                            ),
                            Some(crate::ty::Ty::Relation { .. }) => true,
                            _ => false,
                        }
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
                            ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
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
                entries.extend(type_conditions);
                // A value that is nil at run time must behave as the literal
                // `nil` does (Rails: `voter_id IS NULL`, no type condition),
                // not fall into the no-type sentinel. The type half cannot be
                // omitted from a hash conditionally, so the whole condition
                // hash is chosen by `v.nil?`; only one branch is evaluated, so
                // the other entries are not read twice. One guarded
                // polymorphic key per hash; a second is left on the switch.
                if let [(guard_value, type_col)] = nil_guards.as_slice() {
                    let without_type: Vec<(Expr, Expr)> = entries
                        .iter()
                        .filter(|(k, _)| {
                            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value == type_col)
                        })
                        .cloned()
                        .collect();
                    let guarded = syn(span, ExprNode::Send {
                        recv: Some(guard_value.clone()),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    });
                    // Braced: a brace-less (keyword) hash is not an expression
                    // inside an `if`.
                    let mut with_type = std::mem::replace(&mut *a.node, ExprNode::Lit { value: Literal::Nil });
                    if let ExprNode::Hash { kwargs, .. } = &mut with_type {
                        *kwargs = false;
                    }
                    *a.node = ExprNode::If {
                        cond: guarded,
                        then_branch: syn(span, ExprNode::Hash { entries: without_type, kwargs: false }),
                        else_branch: syn(span, with_type),
                    };
                }
            }
        }
        _ => {}
    }
    aliases
}

/// `where(story: { users: { username: x } })` after `joins(story: :user)`:
/// Rails resolves the inner keys against the association's tables, so the
/// condition is `users.username = x`. Left nested, the runtime read
/// `story.users` as a column.
fn hoist_nested_association_tables(model: &ClassId, entries: &mut Vec<(Expr, Expr)>, ctx: &Ctx) {
    let mut flat: Vec<(Expr, Expr)> = Vec::with_capacity(entries.len());
    for (k, v) in entries.drain(..) {
        let target = match (&*k.node, &*v.node) {
            (ExprNode::Lit { value: Literal::Sym { value: key } }, ExprNode::Hash { entries: inner, .. })
                if !inner.is_empty()
                    && inner.iter().all(|(ik, iv)| {
                        matches!(&*ik.node, ExprNode::Lit { value: Literal::Sym { .. } })
                            && matches!(&*iv.node, ExprNode::Hash { .. })
                    }) =>
            {
                ctx.assocs.assoc_target(model, key).cloned()
            }
            _ => None,
        };
        let Some(target) = target else {
            flat.push((k, v));
            continue;
        };
        let ExprNode::Hash { entries: inner, .. } = *v.node else { unreachable!() };
        for (mut ik, iv) in inner {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &mut *ik.node
                && let Some(table) = ctx.assocs.assoc_table(&target, value)
            {
                *value = table.clone();
            }
            flat.push((ik, iv));
        }
    }
    *entries = flat;
}

/// Re-emit an already-lowered `joins(:assoc)` in the receiver chain with
/// the association-name alias Rails uses once a `where` hash keys off
/// that name. Both spellings come from the registry, so the match is on
/// the exact string this pass generated — a hand-written join string
/// (or a chain with no join at all) is left alone.
fn alias_join_in_chain(expr: &mut Expr, plain: &str, aliased: &str) -> bool {
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else { return false };
    if matches!(method.as_str(), "joins" | "joins!" | "left_outer_joins" | "left_joins") {
        let kind = if method.as_str().starts_with("joins") { "INNER JOIN" } else { "LEFT OUTER JOIN" };
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
fn apply_join_aliases(recv: &mut Expr, model: &ClassId, aliases: &[Symbol], ctx: &Ctx) -> Vec<Symbol> {
    let mut unapplied = Vec::new();
    for assoc in aliases {
        let (Some(plain), Some(aliased)) =
            (ctx.assocs.join_tail(model, assoc), ctx.assocs.aliased_join_tail(model, assoc))
        else {
            continue;
        };
        if !alias_join_in_chain(recv, plain, aliased) {
            unapplied.push(assoc.clone());
        }
    }
    unapplied
}

/// `query.joins!(:article); query.where!(article: { title: t })`: the join
/// is another statement's, so it keeps the table name, and the condition
/// names the table too rather than an alias no join declares.
fn key_unapplied_aliases_by_table(args: &mut [Expr], model: &ClassId, unapplied: &[Symbol], ctx: &Ctx) {
    for a in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *a.node else { continue };
        for (k, _) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value } } = &mut *k.node else { continue };
            if unapplied.contains(value)
                && let Some(table) = ctx.assocs.assoc_table(model, value)
            {
                *value = table.clone();
            }
        }
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
            // Ivars too, under their `@` key: lobsters' moderations index
            // narrows `@moderations` through a `case` and then `.joins(:moderator)`.
            if let Some(key) = match &target {
                crate::expr::LValue::Var { name, .. } => Some(alias_key(name, false)),
                crate::expr::LValue::Ivar { name } => Some(alias_key(name, true)),
                _ => None,
            } {
                match &m {
                    Some(model) => {
                        locals.rel.insert(key, model.clone());
                    }
                    None => {
                        locals.rel.remove(&key);
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
        ExprNode::Send { .. } => {
            let model = rewrite_send(expr, ctx, locals);
            crate::lower::relation_counted_terminal::rewrite_count_gt_when(expr, |rel| {
                is_relation_expr(rel, ctx, locals)
            });
            model
        }
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
            // A terminal too, where there is a `__rel` to end on: Rails runs
            // the body inside the relation's `scoping`, so `self.find(id)`
            // is the scoped find a bare `find(id)` is.
            match self_model {
                Some(m)
                    if is_relation_chain_method(method.as_str())
                        || method.as_str() == "all"
                        || ctx.scope_of(m, &method)
                        || (ctx.scope_body.is_some() && is_relation_terminal(method.as_str(), &args, block.as_ref())) =>
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

    // Rebuilding the send drops `decisions`. A grounded `iso8601` is
    // marked `GROUNDED_TIME_FORMAT` because a later re-analysis can widen
    // the receiver (`minimum || Time.utc` → `String | Time`). Keep that
    // bit when this send is still the zero-arg format call.
    let grounded = expr.decisions & crate::expr::GROUNDED_TIME_FORMAT;
    let put = |span: crate::span::Span,
               recv: Option<Expr>,
               method: Symbol,
               args: Vec<Expr>,
               block: Option<Expr>,
               parenthesized: bool|
     -> Expr {
        let keep = grounded != 0 && method.as_str() == "to_rfc3339" && args.is_empty();
        let mut out = syn(span, ExprNode::Send { recv, method, args, block, parenthesized });
        if keep {
            out.decisions |= grounded;
        }
        out
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
                    let keeps = ctx.scope_keeps_relation(self_model, &method);
                    let new_args = thread_rel(args, var_expr(span, rel), leading, span);
                    *expr = put(span, Some(const_expr(span, self_model)), method, new_args, block, true);
                    return keeps.then(|| self_model.clone());
                }
                // One of the model's own class methods that takes the
                // relation (`last_page_of(PAGE_SIZE)` in campfire's
                // `last_page` scope): it runs against this scope, so it
                // gets this scope's relation, as a scope call does. What
                // it answers is the method's own business, not a relation.
                if let Some(leading) = ctx.assoc_class_method_params(self_model, &method) {
                    let new_args = thread_rel(args, var_expr(span, rel), Some(leading), span);
                    *expr = put(span, Some(const_expr(span, self_model)), method, new_args, block, true);
                    return None;
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
                    let keeps = ctx.scope_keeps_relation(&self_model, &method);
                    *expr =
                        put(span, Some(const_expr(span, &self_model)), method, args, block, true);
                    return keeps.then_some(self_model);
                }
                if is_relation_chain_method(method.as_str()) {
                    let _ = lower_relation_args(&self_model, &method, &mut args, ctx);
                    let seed = relation_new(span, &self_model);
                    *expr = put(span, Some(seed), method, args, block, parenthesized);
                    return Some(self_model);
                }
                // A bare `first(n)` / `last(n)` there: the same counted
                // terminal as `Model.first(n)` below.
                if let Some(counted) = counted_terminal(&method, &args, block.as_ref()) {
                    let seed = relation_new(span, &self_model);
                    *expr = put(span, Some(seed), counted, args, block, parenthesized);
                    return None;
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
                    // `_insert_row`: Rails' bulk insert runs neither
                    // validations nor callbacks (it is one INSERT), and
                    // neither does this. Timestamps fill, as Rails'
                    // `record_timestamps` default does.
                    let save = syn(
                        span,
                        ExprNode::Send {
                            recv: Some(build),
                            method: Symbol::from("_insert_row"),
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
                                ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
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
                // `<Model>.insert_all!(rows, returning: %w[id])` — the
                // raising twin: no conflict guard (a duplicate raises,
                // as `insert_all!` does), and its value is Rails'
                // `ActiveRecord::Result` of the RETURNING columns, one
                // row per insert (the primary key when `returning:` is
                // left out, SQLite's default). campfire's search test
                // reads the new ids with `.rows.flatten`.
                //
                //   ActiveRecord::Result.new(rows.map { |__attrs|
                //     __rec = Message.new(__attrs)
                //     __rec._insert_row
                //     { "id" => __rec.id } })
                //
                // Only a literal column list (Strings or Symbols) lowers;
                // anything else declines.
                if method.as_str() == "insert_all!" && !args.is_empty() && args.len() <= 2 && block.is_none() {
                    let returning = insert_all_bang_returning(args.get(1));
                    if let Some(columns) = returning {
                        let attrs = Symbol::from("__attrs");
                        let rec = Symbol::from("__rec");
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
                        let bind = syn(
                            span,
                            ExprNode::Assign {
                                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: rec.clone() },
                                value: build,
                            },
                        );
                        let insert = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(var_expr(span, &rec)),
                                method: Symbol::from("_insert_row"),
                                args: vec![],
                                block: None,
                                parenthesized: true,
                            },
                        );
                        let row = syn(
                            span,
                            ExprNode::Hash {
                                entries: columns
                                    .iter()
                                    .map(|c| {
                                        (
                                            syn(span, ExprNode::Lit { value: Literal::Str { value: c.clone() } }),
                                            syn(
                                                span,
                                                ExprNode::Send {
                                                    recv: Some(var_expr(span, &rec)),
                                                    method: Symbol::from(c.as_str()),
                                                    args: vec![],
                                                    block: None,
                                                    parenthesized: false,
                                                },
                                            ),
                                        )
                                    })
                                    .collect(),
                                kwargs: false,
                            },
                        );
                        let mut args = args;
                        let rows = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(args.remove(0)),
                                method: Symbol::from("map"),
                                args: vec![],
                                block: Some(syn(
                                    span,
                                    ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
                                        params: vec![attrs],
                                        block_param: None,
                                        body: syn(span, ExprNode::Seq { exprs: vec![bind, insert, row] }),
                                        block_style: BlockStyle::Brace,
                                    },
                                )),
                                parenthesized: false,
                            },
                        );
                        *expr = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(syn(
                                    span,
                                    ExprNode::Const {
                                        path: vec![Symbol::from("ActiveRecord"), Symbol::from("Result")],
                                    },
                                )),
                                method: Symbol::from("new"),
                                args: vec![rows],
                                block: None,
                                parenthesized: true,
                            },
                        );
                        return None;
                    }
                }
                if ctx.scope_of(&m, &method) {
                    let keeps = ctx.scope_keeps_relation(&m, &method);
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return keeps.then_some(m);
                }
                if matches!(method.as_str(), "pluck" | "pick")
                    && ctx.user_returns.defines_class_method(&m, &method)
                {
                    let returns = ctx.user_returns.class.get(&(m.clone(), method.clone())).cloned();
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return returns;
                }
                if is_relation_chain_method(method.as_str())
                    || method.as_str() == "all"
                    || CLASS_ROOT_TERMINALS.contains(&method.as_str())
                {
                    // An STI subclass's scope (`lower::sti_scope`) is the
                    // base's relation holding the subclass, so a row
                    // without `type` is built as the subclass, as Rails does.
                    let queried = super::sti_scope::sti_queried_class(method.as_str(), &args);
                    let seed = relation_new(span, queried.as_ref().unwrap_or(&m));
                    if method.as_str() == "all" {
                        *expr = seed;
                    } else {
                        let _ = lower_relation_args(&m, &method, &mut args, ctx);
                        *expr = put(span, Some(seed), method, args, block, parenthesized);
                    }
                    return Some(m);
                }
                // `Developer.first(2)` / `.last(2)`: the counted forms are
                // Relation terminals (`first_n` / `last_n`). The runtime's
                // class-side `first` takes no count, and Rails' class
                // method delegates to `all`, so the count rides a fresh
                // relation, as the chain methods above do. Answers an
                // Array, not a relation: no model to thread.
                if let Some(counted) = counted_terminal(&method, &args, block.as_ref()) {
                    *expr = put(span, Some(relation_new(span, &m)), counted, args, block, parenthesized);
                    return None;
                }
                let returns = ctx.user_returns.class.get(&(m.clone(), method.clone())).cloned();
                *expr = put(span, Some(r), method, args, block, parenthesized);
                return returns;
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
                let owners = || match &*ir.node {
                    ExprNode::SelfRef => ctx
                        .instance_self
                        .as_ref()
                        .filter(|m| ctx.assocs.is_model(m))
                        .map(|m| vec![m]),
                    _ => ctx.assocs.owner_models(ir, aname),
                };
                if aargs.is_empty()
                    && ctx.assocs.is_assoc_extension(None, aname, &method)
                    && ctx.assocs.is_assoc_extension(owners().as_deref(), aname, &method)
                {
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
                                    let key = ctx.assocs.has_many_owner_key(Some(&self_model), aname);
                                    (target, fk, syn(span, ExprNode::Ivar { name: key }))
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
                        // Any other owner EXPRESSION, for a constructor
                        // only: `Room.find(id).messages.create!(…)`.
                        // `owner_reads_once` guards the chain seeding,
                        // but a constructor's rewrite drops the original
                        // receiver and names the owner exactly once (as
                        // `<fk>: <owner>.id`), so the owner is evaluated
                        // once either way. Declined, the call stayed on
                        // the plain reader's Array — `create!` for an
                        // instance of Array, found by once-campfire-rust's
                        // model scenario (scripts/campfire-db-differential).
                        _ if matches!(method.as_str(), "build" | "create" | "create!") => {
                            assoc_owner_seed(ctx, aname, ir, span)
                        }
                        // The same for `<<` onto a has_many the owner holds:
                        // the push keeps the owner as its one receiver
                        // (`Article.find(id).comments << c`).
                        _ if method.as_str() == "<<"
                            && args.len() == 1
                            && assoc_read_target(ir, aname, ctx.assocs)
                                .and_then(|(owner, _, _)| owner)
                                .is_some_and(|m| ctx.assocs.has_many_held.contains(&(m, aname.clone()))) =>
                        {
                            assoc_owner_seed(ctx, aname, ir, span)
                        }
                        _ => None,
                    };
                    // The owner model the association resolved through,
                    // for its automatic inverse (constructors only).
                    let inverse = if !ctx.owner_objects {
                        None
                    } else {
                        match &*ir.node {
                            ExprNode::SelfRef => ctx.assocs.has_many_inverse(ctx.instance_self.as_ref(), aname),
                            _ => assoc_read_target(ir, aname, ctx.assocs).and_then(
                                |(owner_model, _, _)| ctx.assocs.has_many_inverse(owner_model.as_ref(), aname),
                            ),
                        }
                        .cloned()
                    };
                    if let Some((target, fk, owner_id)) = resolved {
                        let owner_model: Option<ClassId> = match &*ir.node {
                            ExprNode::SelfRef => ctx.instance_self.clone(),
                            _ => assoc_read_target(ir, aname, ctx.assocs).and_then(|(owner, _, _)| owner),
                        };
                        let type_entry = ctx.assocs.has_many_poly(owner_model.as_ref(), aname).map(|(col, owner_class)| {
                            (
                                syn(span, ExprNode::Lit { value: Literal::Sym { value: col.clone() } }),
                                syn(span, ExprNode::Lit { value: Literal::Str { value: owner_class.clone() } }),
                            )
                        });
                        // CollectionProxy constructors: `story.comments
                        // .build(attrs)` constructs the target with the
                        // association's FK preset — `Comment.new(attrs…,
                        // story_id: story.id)`. Checked ahead of the
                        // scope/chain/terminal seeding: a constructor
                        // returns a record, not a relation. Only the
                        // kwargs-hash arg shapes rewrite (none, or one
                        // kwargs hash — the corpus forms; Rails merges
                        // the FK over caller attrs, so it appends last).
                        let held = owner_model
                            .as_ref()
                            .is_some_and(|m| ctx.assocs.has_many_held.contains(&(m.clone(), aname.clone())));
                        // `<<` on a model that holds the association: its
                        // generated `__push_<name>` holds the record on a
                        // new owner, or sets the key and saves it.
                        // An Array is each of its records (Rails'
                        // `concat(records.flatten)`): `__push_all_<name>`.
                        if held && method.as_str() == "<<" && args.len() == 1 {
                            let (callee, arg) = match pushed_records(&args[0]) {
                                Some(records) => (format!("__push_all_{aname}"), records),
                                None => (format!("__push_{aname}"), args[0].clone()),
                            };
                            *expr = put(span, Some(ir.clone()), Symbol::from(callee), vec![arg], None, true);
                            return None;
                        }
                        let ctor = match method.as_str() {
                            "build" => Some("new"),
                            "create" => Some("create"),
                            "create!" => Some("create!"),
                            _ => None,
                        };
                        if let Some(ctor_name) = ctor {
                            // With an automatic inverse the OWNER goes in
                            // under the `belongs_to` name (`room: room`),
                            // as Rails sets the inverse: the record's
                            // `belongs_to` reader then answers the owner
                            // object itself. Its writer stores the same
                            // foreign key `fk: owner.id` would have.
                            let fk_entry = match &inverse {
                                Some(bt) => (
                                    syn(
                                        span,
                                        ExprNode::Lit { value: Literal::Sym { value: bt.clone() } },
                                    ),
                                    ir.clone(),
                                ),
                                None => (
                                    syn(
                                        span,
                                        ExprNode::Lit {
                                            value: Literal::Sym { value: fk.clone() },
                                        },
                                    ),
                                    owner_id.clone(),
                                ),
                            };
                            let preset: Vec<(Expr, Expr)> = std::iter::once(fk_entry).chain(type_entry.clone()).collect();
                            let merged: Option<Vec<Expr>> = if args.is_empty() {
                                Some(vec![syn(
                                    span,
                                    ExprNode::Hash { entries: preset.clone(), kwargs: true },
                                )])
                            } else if args.len() == 1 {
                                match &*args[0].node {
                                    ExprNode::Hash { entries, kwargs: true } => {
                                        let mut entries = entries.clone();
                                        entries.extend(preset.clone());
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
                                                entries.extend(preset.clone());
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
                                // A built record is the owner's to hold:
                                // validated with it and saved after it.
                                if held && ctor_name == "new" {
                                    let ty = expr.ty.clone();
                                    let built = std::mem::replace(expr, syn(span, ExprNode::Lit { value: Literal::Nil }));
                                    *expr = put(
                                        span,
                                        Some(ir.clone()),
                                        Symbol::from(format!("__hold_{aname}")),
                                        vec![built],
                                        None,
                                        true,
                                    );
                                    expr.ty = ty;
                                }
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
                            let seed = build_association_seed(
                                span, &target, fk, owner_id, type_entry.clone(), ir, aname,
                                assoc_cm.is_some(),
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
                                let keeps = ctx.scope_keeps_relation(&target, &method);
                                let new_args = thread_rel(args, seed, leading, span);
                                *expr = put(
                                    span,
                                    Some(const_expr(span, &target)),
                                    method,
                                    new_args,
                                    block,
                                    true,
                                );
                                return keeps.then_some(target);
                            }
                            // `find_sole_by` on a seeded association —
                            // `gadget.widgets.find_sole_by(created_at:
                            // a..b)` — is routed through `where(...).sole`
                            // rather than kept on its own spelling, so its
                            // single condition hash takes the ordinary
                            // `where` path: `lower_relation_args` only
                            // converts a Range VALUE ("where(connected_at:
                            // TTL.ago..)", above) for "where"/"not"/
                            // "find_by"/"find_by!", so a Range reaching it
                            // as "find_sole_by"'s own argument passed
                            // through unconverted and compiled to `col =
                            // <the range object>` — matches nothing, same
                            // failure the comment above describes. Only
                            // the single-hash shape `find_sole_by` is
                            // actually called with is rewritten; anything
                            // else falls through to the general lowering
                            // below unchanged.
                            if method.as_str() == "find_sole_by" && args.len() == 1 {
                                let mut where_args = args;
                                let _ = lower_relation_args(
                                    &target,
                                    &Symbol::from("where"),
                                    &mut where_args,
                                    ctx,
                                );
                                let where_call = syn(
                                    span,
                                    ExprNode::Send {
                                        recv: Some(seed),
                                        method: Symbol::from("where"),
                                        args: where_args,
                                        block: None,
                                        parenthesized: true,
                                    },
                                );
                                *expr = syn(
                                    span,
                                    ExprNode::Send {
                                        recv: Some(where_call),
                                        method: Symbol::from("sole"),
                                        args: vec![],
                                        block: None,
                                        parenthesized: false,
                                    },
                                );
                                return None;
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
                ExprNode::Ivar { name } => locals.rel.get(&alias_key(name, true)).cloned(),
                _ => rewrite(&mut r, ctx, locals),
            };
            let r_model = r_model.or_else(|| match &*r.node {
                ExprNode::Send { method: rname, .. } => {
                    ctx.user_returns.instance.get(rname).cloned().flatten()
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
                    let keeps = ctx.scope_keeps_relation(&mr, &method);
                    let new_args = thread_rel(args, r, leading, span);
                    *expr = put(span, Some(const_expr(span, &mr)), method, new_args, block, true);
                    return keeps.then_some(mr);
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
                if is_relation_chain_method(method.as_str())
                    || crate::catalog::is_relation_context_builder(method.as_str())
                {
                    let aliases = lower_relation_args(&mr, &method, &mut args, ctx);
                    let mut r = r;
                    let unapplied = apply_join_aliases(&mut r, &mr, &aliases, ctx);
                    key_unapplied_aliases_by_table(&mut args, &mr, &unapplied, ctx);
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(mr);
                }
                // Not passed through as written, nor lowered in place: the condition keys off the target's associations (`comments.user` was read), and a lowered range or record list needs `where`'s binds, which a one-argument terminal does not take.
                let after_where = match method.as_str() {
                    "find_by" => Some("first"),
                    "find_by!" => Some("first!"),
                    "find_sole_by" => Some("sole"),
                    "destroy_by" => Some("destroy_all"),
                    "delete_by" => Some("delete_all"),
                    "exists?" => Some("exists?"),
                    _ => None,
                };
                if let Some(terminal) = after_where
                    && block.is_none()
                    && matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::Hash { .. }))
                {
                    let where_name = Symbol::from("where");
                    let aliases = lower_relation_args(&mr, &where_name, &mut args, ctx);
                    let mut r = r;
                    let unapplied = apply_join_aliases(&mut r, &mr, &aliases, ctx);
                    key_unapplied_aliases_by_table(&mut args, &mr, &unapplied, ctx);
                    if matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::Hash { .. })) {
                        *expr = put(span, Some(r), method, args, block, parenthesized);
                        return None;
                    }
                    let filtered = put(span, Some(r), where_name, args, None, true);
                    *expr = put(span, Some(filtered), Symbol::from(terminal), vec![], None, false);
                    return None;
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
                // A scope proved to answer something else (a `Page`)
                // names no relation, wherever it sits.
                let names_a_scope = matches!(&*r.node, ExprNode::Send { method: rname, .. }
                    if ctx.sole_scope_owner(rname).is_some_and(|owner| ctx.scope_keeps_relation(owner, rname)));
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

/// Hand `rel_param` to every receiver-less call of one of `self_model`'s
/// class methods that takes the relation — the step the scope-body
/// rewrite cannot take, because it runs before the call-site demand
/// that registers those methods is known. Applied at the emit seam to
/// scope bodies and to the bodies of class methods that take the
/// relation themselves: in Rails both run with that relation as the
/// current scope, so a class method they call does too
/// (basecamp/once-campfire#292's `last_page` -> `last_page_of`).
pub fn thread_rel_into_class_method_calls(
    body: &mut Expr,
    self_model: &ClassId,
    rel_param: &Symbol,
    assoc_class_methods: &AssocClassMethods,
) {
    let Some(per_model) = assoc_class_methods.get(self_model) else { return };
    fn walk(e: &mut Expr, rel: &Symbol, per_model: &HashMap<Symbol, AssocScopedMethod>) {
        e.node.for_each_child_mut(&mut |c| walk(c, rel, per_model));
        let span = e.span;
        if let ExprNode::Send { recv: None, method, args, .. } = &mut *e.node {
            if let Some(entry) = per_model.get(method) {
                let already = args.iter().any(|a| matches!(&*a.node, ExprNode::Var { name, .. } if name == rel));
                if !already {
                    let taken = std::mem::take(args);
                    *args = thread_rel(taken, var_expr(span, rel), Some(&entry.params), span);
                }
            }
        }
    }
    walk(body, rel_param, per_model);
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
    let empty_assoc_cm = AssocClassMethods::default();
    // A scope body's `insert_all` (none in the corpus) keeps the
    // unguarded inline: this entry point takes registries, not the app,
    // so there is no schema here to read a conflict target from.
    let empty_unique = UniqueKeys::default();
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
        owner_objects: false,
    };
    let mut locals = Locals::default();
    rewrite(body, &ctx, &mut locals);
}

/// Ivars a class binds, in every method that assigns them, to one and the
/// same association read: campfire's push-subscriptions controller
/// opens each action with `@push_subscriptions = Current.user
/// .push_subscriptions` (its `before_action`, inlined per action) and
/// reads the ivar again in the private `register_while_signed_in`. Any
/// other assignment to the ivar, or two different reads, and it is left
/// out. Compared by emitted text, so an inlined `before_action` (one
/// source, copied into each action) agrees with itself.
pub fn class_ivar_assoc_bindings(
    methods: &[crate::dialect::MethodDef],
    assocs: &AssocRegistry,
) -> Vec<(Symbol, Expr)> {
    fn collect(e: &Expr, assocs: &AssocRegistry, out: &mut Vec<(Symbol, Option<Expr>)>) {
        if let ExprNode::Assign { target: crate::expr::LValue::Ivar { name }, value } = &*e.node {
            let assoc_read = matches!(&*value.node,
                ExprNode::Send { recv: Some(_), method, args, block: None, .. }
                    if args.is_empty() && assocs.is_has_many_name(method));
            out.push((name.clone(), assoc_read.then(|| value.clone())));
        }
        e.node.for_each_child(&mut |c| collect(c, assocs, out));
    }
    let mut seen: Vec<(Symbol, Option<Expr>)> = Vec::new();
    for m in methods {
        collect(&m.body, assocs, &mut seen);
    }
    let mut out: Vec<(Symbol, Expr)> = Vec::new();
    let mut names: Vec<Symbol> = seen.iter().map(|(n, _)| n.clone()).collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    names.dedup();
    for name in names {
        let values: Vec<&Option<Expr>> = seen.iter().filter(|(n, _)| *n == name).map(|(_, v)| v).collect();
        let Some(Some(first)) = values.first().map(|v| v.as_ref()) else { continue };
        let text = crate::emit::ruby::emit_expr(first);
        if values.iter().all(|v| v.as_ref().is_some_and(|e| crate::emit::ruby::emit_expr(e) == text)) {
            out.push((name, first.clone()));
        }
    }
    out
}

/// The bindings of `class_ivar_assoc_bindings` a body READS without
/// assigning: a helper the actions call after setting the ivar.
pub fn ivar_seeds_for(body: &Expr, bindings: &[(Symbol, Expr)]) -> Vec<(Symbol, Expr)> {
    fn reads(e: &Expr, name: &Symbol) -> bool {
        let mut found = matches!(&*e.node, ExprNode::Ivar { name: n } if n == name);
        e.node.for_each_child(&mut |c| found = found || reads(c, name));
        found
    }
    fn assigns(e: &Expr, name: &Symbol) -> bool {
        let mut found = matches!(&*e.node,
            ExprNode::Assign { target: crate::expr::LValue::Ivar { name: n }, .. } if n == name);
        e.node.for_each_child(&mut |c| found = found || assigns(c, name));
        found
    }
    bindings
        .iter()
        .filter(|(name, _)| reads(body, name) && !assigns(body, name))
        .cloned()
        .collect()
}

/// `rewrite_call_site` for a body that reads ivars its class bound
/// elsewhere (`ivar_seeds_for`): the alias the binding would have
/// recorded is recorded up front, so `@ivar.find_by(…)` seeds the same
/// relation it does in the action that assigned it.
pub fn rewrite_call_site_seeded(
    expr: &mut Expr,
    regs: &Registries<'_>,
    class_self: Option<&ClassId>,
    instance_self: Option<&ClassId>,
    seeds: &[(Symbol, Expr)],
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
        owner_objects: regs.owner_objects,
    };
    let mut locals = Locals::default();
    for (name, value) in seeds {
        if let Some(model) = seedable_assoc_read(&ctx, value, value.span) {
            locals.assoc.insert(alias_key(name, true), (model, value.clone()));
        }
    }
    rewrite(expr, &ctx, &mut locals);
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
        owner_objects: regs.owner_objects,
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
    /// The Ruby family: an association constructor hands its owner
    /// OBJECT to the automatic inverse (`room: room`) — see
    /// [`AssocRegistry::has_many_inverse`]. False keeps `room_id:
    /// room.id`, the only form a strict target's attrs bag can carry.
    pub owner_objects: bool,
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

    /// A trailing kwargs-Hash arg's entries, panicking with a readable
    /// message if `e` isn't one.
    fn kwargs_entries(e: &Expr) -> &[(Expr, Expr)] {
        match &*e.node {
            ExprNode::Hash { entries, kwargs: true } => entries,
            other => panic!("not a kwargs Hash: {other:?}"),
        }
    }

    /// `(key, value)` names `__rel` and the value is the threaded marker.
    fn is_rel_kwarg(entry: &(Expr, Expr)) -> bool {
        matches!(&*entry.0.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "__rel")
            && is_rel(&entry.1)
    }

    #[test]
    fn thread_rel_rest_callee_zero_args_threads_as_a_keyword_with_no_nil_pad() {
        // `def those_tagged(*tags)` called bare (`Widget.active.those_tagged`)
        // — the LATENT bug: the old pad loop didn't exclude a rest param
        // from `filter(|p| !p.keyword)`, so it padded a positional `nil`
        // into `*tags` before appending the relation. Fixed shape: no
        // padding at all, and the relation threads as a keyword —
        // `Widget.those_tagged(__rel: Widget.active)`, not
        // `Widget.those_tagged(nil, __rel: Widget.active)` and not
        // `Widget.those_tagged(__rel: Widget.active)` as a POSITIONAL
        // (which would be a syntax error on the def side).
        let leading = vec![Param::rest(Symbol::from("tags"))];
        let out = thread_rel(vec![], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 1, "no nil padding, no separate positional: {out:?}");
        let entries = kwargs_entries(&out[0]);
        assert_eq!(entries.len(), 1, "just __rel:, nothing else: {entries:?}");
        assert!(is_rel_kwarg(&entries[0]), "a single __rel: kwarg: {entries:?}");
    }

    #[test]
    fn thread_rel_rest_callee_with_supplied_args_appends_the_keyword() {
        // `Widget.active.those_tagged(:a)` → the splat-fed positional
        // stays exactly as the caller wrote it; `__rel:` is appended.
        let leading = vec![Param::rest(Symbol::from("tags"))];
        let a = Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from("a") } });
        let out = thread_rel(vec![a], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(matches!(&*out[0].node, ExprNode::Lit { value: Literal::Sym { .. } }));
        let entries = kwargs_entries(&out[1]);
        assert_eq!(entries.len(), 1);
        assert!(is_rel_kwarg(&entries[0]));
    }

    #[test]
    fn thread_rel_rest_callee_merges_into_an_existing_kwargs_tail() {
        // `def those_tagged(*tags, limit: 10)` called as
        // `those_tagged(:a, limit: 5)` — the existing `limit:` kwarg and
        // the new `__rel:` kwarg land in ONE trailing Hash, not two.
        let leading = vec![
            Param::rest(Symbol::from("tags")),
            Param::keyword(Symbol::from("limit"), Some(int_lit(10))),
        ];
        let a = Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from("a") } });
        let limit_key = Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from("limit") } });
        let kw = Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(limit_key, int_lit(5))], kwargs: true },
        );
        let out = thread_rel(vec![a, kw], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2, "one positional, ONE merged kwargs hash: {out:?}");
        let entries = kwargs_entries(&out[1]);
        assert_eq!(entries.len(), 2, "limit: AND __rel: in the same hash: {entries:?}");
        assert!(entries.iter().any(is_rel_kwarg), "entries {entries:?}");
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
        EMPTY.get_or_init(AssocClassMethods::default)
    }

    fn empty_unique_keys() -> &'static UniqueKeys {
        static EMPTY: std::sync::OnceLock<UniqueKeys> = std::sync::OnceLock::new();
        EMPTY.get_or_init(UniqueKeys::default)
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
            owner_objects: false,
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
            owner_objects: false,
        }
    }

    fn sym_lit(s: &str) -> Expr {
        Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }

    #[test]
    fn joins_sym_expands_to_join_sql() {
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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

    fn polymorphic_fixture(targets: Vec<ClassId>) -> AssocRegistry {
        let mut reg = AssocRegistry::default();
        let owner = ClassId(Symbol::from("ActionText::RichText"));
        reg.belongs_to_fk.insert((owner.clone(), Symbol::from("record")), Symbol::from("record_id"));
        reg.polymorphic_belongs_to
            .insert((owner, Symbol::from("record")), (Symbol::from("record_type"), targets));
        reg
    }

    fn where_record(value: Expr, reg: &AssocRegistry) -> Vec<(String, String)> {
        let (scopes, models) = (ScopeRegistry::default(), HashSet::default());
        let ctx = ctx_with(&scopes, &models, reg);
        let mut args = vec![hash_arg(vec![(sym_lit("record"), value)])];
        lower_relation_args(&ClassId(Symbol::from("ActionText::RichText")), &Symbol::from("where"), &mut args, &ctx);
        let hash = match &*args[0].node {
            ExprNode::If { else_branch, .. } => else_branch,
            _ => &args[0],
        };
        let ExprNode::Hash { entries, .. } = &*hash.node else { panic!("hash") };
        entries
            .iter()
            .map(|(k, v)| {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else { panic!("key") };
                (value.as_str().to_string(), format!("{:?}", v.node).chars().take(12).collect())
            })
            .collect()
    }

    fn var(name: &str) -> Expr {
        Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
    }

    #[test]
    fn polymorphic_without_resolved_targets_renames_to_id_only() {
        // ActionText::RichText#record / ActiveStorage::Attachment#record:
        // framework polymorphics whose implementors are not resolved keep
        // the pre-existing `record_id` rename, never an unknown `record`.
        let keys: Vec<String> =
            where_record(var("message"), &polymorphic_fixture(vec![])).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["record_id"]);
    }

    #[test]
    fn polymorphic_with_targets_adds_the_type_switch() {
        let reg = polymorphic_fixture(vec![ClassId(Symbol::from("Message"))]);
        let keys: Vec<String> = where_record(var("message"), &reg).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["record_id", "record_type"]);
        // The type switch has an otherwise-arm no stored row can match, so a
        // value that is none of the implementors finds nothing (not `IS NULL`).
        let (scopes, models) = (ScopeRegistry::default(), HashSet::default());
        let ctx = ctx_with(&scopes, &models, &reg);
        let mut args = vec![hash_arg(vec![(sym_lit("record"), var("message"))])];
        lower_relation_args(&ClassId(Symbol::from("ActionText::RichText")), &Symbol::from("where"), &mut args, &ctx);
        // A possibly-nil variable chooses the hash by `v.nil?`: the nil branch
        // is `record_id` alone (Rails), the other carries the type switch.
        let ExprNode::If { then_branch, else_branch, .. } = &*args[0].node else { panic!("nil guard") };
        let ExprNode::Hash { entries: nil_entries, .. } = &*then_branch.node else { panic!("hash") };
        assert_eq!(nil_entries.len(), 1);
        let ExprNode::Hash { entries, .. } = &*else_branch.node else { panic!("hash") };
        let ExprNode::Case { arms, .. } = &*entries[1].1.node else { panic!("case") };
        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[1].pattern, crate::expr::Pattern::Wildcard));
        assert!(matches!(
            &*arms[1].body.node,
            ExprNode::Lit { value: Literal::Str { value } } if value == POLYMORPHIC_NO_TYPE
        ));
        // A non-simple value cannot be read twice: id-only, as before.
        let call = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(var("x")),
                method: Symbol::from("message"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let keys: Vec<String> = where_record(call, &reg).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["record_id"]);
    }

    #[test]
    fn joins_nested_hash_expands_both_hops() {
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![hash_arg(vec![(sym_lit("hidings"), sym_lit("taggings"))])];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        assert!(matches!(&*args[0].node, ExprNode::Hash { .. }), "{:?}", args[0].node);
    }

    /// `joins(hidings: [:user])` — an array value is several hops off
    /// the same parent.
    #[test]
    fn joins_nested_array_value_expands_each_hop() {
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut comments_var =
            Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("comments") });
        comments_var.ty = Some(crate::ty::Ty::Array {
            elem: std::sync::Arc::new(crate::ty::Ty::Untyped),
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
        let (scopes, models, assocs) = (ScopeRegistry::default(), HashSet::default(), assoc_fixture());
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
            owner_objects: false,
        }
    }

    fn fragment_sql(f: &Expr) -> &str {
        match &*f.node {
            ExprNode::Lit { value: Literal::Str { value } } => value.as_str(),
            other => panic!("expected a SQL string literal, got {other:?}"),
        }
    }

    /// A Range value compared for EQUALITY matched nothing, silently —
    /// a strict runtime has no Range arm and falls through to
    /// `col = <the range object>`. Both ends, one end, and neither.
    #[test]
    fn a_range_condition_becomes_comparisons() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::default(), HashSet::default(), AssocRegistry::default());
        let ctx = fragment_ctx(&sc, &md, &ac);

        let (sql, binds) = range_condition_fragment(
            &m,
            &cond_hash("connected_at", range(Some(cutoff("a")), Some(cutoff("b")), false)),
            &ctx,
        )
        .expect("both ends");
        assert_eq!(
            fragment_sql(&sql),
            r#"("memberships"."connected_at" >= ? AND "memberships"."connected_at" <= ?)"#
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
        assert_eq!(fragment_sql(&sql), r#""memberships"."connected_at" >= ?"#);
        assert_eq!(binds.len(), 1);

        // Neither end has no comparison to make.
        assert!(
            range_condition_fragment(&m, &cond_hash("connected_at", range(None, None, false)), &ctx)
                .is_none()
        );
    }

    /// An end that may be nil at run time is no bound there (`nil..b` is
    /// beginless in Rails), not `col >= NULL`, which matches nothing.
    #[test]
    fn a_nilable_range_end_is_no_bound_when_nil() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::default(), HashSet::default(), AssocRegistry::default());
        let ctx = fragment_ctx(&sc, &md, &ac);
        let mut since = cutoff("since");
        since.ty = Some(crate::ty::Ty::Union { variants: vec![crate::ty::Ty::Int, crate::ty::Ty::Nil].into() });
        let (sql, binds) = range_condition_fragment(&m, &cond_hash("connected_at", range(Some(since), Some(cutoff("b")), false)), &ctx)
            .expect("nilable begin");
        assert_eq!(
            fragment_sql(&sql),
            r#"((? IS NULL OR "memberships"."connected_at" >= ?) AND "memberships"."connected_at" <= ?)"#
        );
        assert_eq!(binds.len(), 3);
    }

    /// `where(connected_at: [ nil, ...cutoff ])` — Rails ORs the
    /// alternatives and splits `nil` out as `IS NULL`. The runtime
    /// renders an Array as `IN (…)`, so this is the same silent
    /// no-match wearing a different hat (campfire's
    /// `Membership.disconnected`).
    #[test]
    fn an_array_of_alternatives_becomes_an_or() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::default(), HashSet::default(), AssocRegistry::default());
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
            r#"("memberships"."connected_at" IS NULL OR "memberships"."connected_at" < ?)"#
        );
        assert_eq!(binds.len(), 1);
    }

    /// A member this rewrite does not reproduce declines the whole
    /// condition rather than composing half of it: a scalar would need
    /// `IN (…)` alongside the OR arms.
    #[test]
    fn an_array_with_a_scalar_member_declines() {
        let m = ClassId(Symbol::from("Membership"));
        let (sc, md, ac) = (ScopeRegistry::default(), HashSet::default(), AssocRegistry::default());
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
    fn a_redeclared_has_many_seeds_from_the_later_declaration() {
        // A concern's `has_many :comments` spliced at the `include`, then
        // the model's own: Rails keeps the later one, and so does every
        // seed fact, not only the target and foreign key.
        let article = ingest(
            "class Article < ApplicationRecord\n  has_many :comments, -> { where(spam: false) }, primary_key: :uuid\n  has_many :comments, foreign_key: :legacy_article_id\nend\n",
            "app/models/article.rb",
        );
        let assocs = build_assoc_registry(&[article]);
        let (article, comments) = (ClassId(Symbol::from("Article")), Symbol::from("comments"));
        assert!(!assocs.is_unseedable(Some(&article), &comments));
        assert!(!assocs.is_unseedable(None, &comments));
        assert_eq!(assocs.has_many_owner_key(Some(&article), &comments).as_str(), "id");
        assert_eq!(
            assocs.has_many_by_name(&comments).map(|(_, fk)| fk.as_str()),
            Some("legacy_article_id")
        );
    }

    #[test]
    fn a_redeclared_association_drops_the_earlier_join_alias() {
        // The earlier declaration's `remarks` table needs an alias to be
        // joined as `comments`; the later one's `comments` table does
        // not, so `where(comments: …)` joins `comments`, not `remarks`.
        let article = ingest(
            "class Article < ApplicationRecord\n  has_many :comments, class_name: \"Remark\"\n  has_many :comments\nend\n",
            "app/models/article.rb",
        );
        let assocs = build_assoc_registry(&[article]);
        let (article, comments) = (ClassId(Symbol::from("Article")), Symbol::from("comments"));
        assert_eq!(assocs.aliased_join_tail(&article, &comments), None);
        assert_eq!(
            assocs.join_tail(&article, &comments).map(String::as_str),
            Some("comments ON comments.article_id = articles.id")
        );
        assert_eq!(assocs.assoc_table(&article, &comments).map(Symbol::as_str), Some("comments"));
        assert_eq!(assocs.assoc_target(&article, &comments).map(|c| c.0.as_str()), Some("Comment"));
    }

    #[test]
    fn a_redeclaration_that_records_nothing_still_drops_the_earlier_one() {
        // `through: :missing` names no association, so the later
        // declaration has no join of its own to record.
        let article = ingest(
            "class Article < ApplicationRecord\n  has_many :comments\n  has_many :comments, through: :missing\nend\n",
            "app/models/article.rb",
        );
        let assocs = build_assoc_registry(&[article]);
        let (article, comments) = (ClassId(Symbol::from("Article")), Symbol::from("comments"));
        assert_eq!(assocs.join_tail(&article, &comments), None);
        assert_eq!(assocs.assoc_table(&article, &comments), None);
        assert_eq!(assocs.assoc_target(&article, &comments), None);
        assert_eq!(assocs.has_many_fk(&article, &comments), None);
    }

    #[test]
    fn a_redeclaration_of_another_kind_drops_the_earlier_one() {
        let article = ingest(
            "class Article < ApplicationRecord\n  belongs_to :author, polymorphic: true\n  has_one :author\n  has_many :tags\n  has_and_belongs_to_many :tags\nend\n",
            "app/models/article.rb",
        );
        let assocs = build_assoc_registry(&[article]);
        let (article, author, tags) =
            (ClassId(Symbol::from("Article")), Symbol::from("author"), Symbol::from("tags"));
        assert_eq!(assocs.belongs_to_fk(&article, &author), None);
        assert_eq!(assocs.polymorphic_belongs_to(&article, &author), None);
        assert_eq!(assocs.has_many_fk(&article, &tags), None);
        assert_eq!(assocs.join_tail(&article, &tags), None);
    }

    #[test]
    fn a_subclass_redeclaring_a_name_as_another_kind_shadows_the_superclass() {
        // `Story#tags` is the join-table association, not Article's
        // `has_many`, for the rewrite and the seed alike.
        let article = ingest("class Article < ApplicationRecord\n  has_many :tags\nend\n", "app/models/article.rb");
        let story = ingest(
            "class Story < Article\n  has_and_belongs_to_many :tags\nend\n",
            "app/models/story.rb",
        );
        let assocs = build_assoc_registry(&[article, story]);
        let story = crate::ty::Ty::Class { id: ClassId(Symbol::from("Story")), args: vec![].into() };
        let tags = Symbol::from("tags");
        let mut owner = syn(crate::span::Span::synthetic(), ExprNode::Ivar { name: Symbol::from("story") });
        owner.ty = Some(story);
        assert_eq!(assocs.build_target(&owner, &tags, &Symbol::from("build"), false), AssocBuildTarget::Declined);
        assert_eq!(assoc_read_target(&owner, &tags, &assocs), None);
    }

    #[test]
    fn a_redeclared_has_many_keeps_only_the_later_inverse() {
        // An explicit `foreign_key:` rules out Rails' automatic inverse.
        let article = ingest(
            "class Article < ApplicationRecord\n  has_many :comments\n  has_many :comments, foreign_key: :article_id\nend\n",
            "app/models/article.rb",
        );
        let comment = ingest("class Comment < ApplicationRecord\n  belongs_to :article\nend\n", "app/models/comment.rb");
        let assocs = build_assoc_registry(&[article, comment]);
        let (article, comments) = (ClassId(Symbol::from("Article")), Symbol::from("comments"));
        assert_eq!(assocs.has_many_inverse(Some(&article), &comments), None);
        assert_eq!(assocs.has_many_inverse(None, &comments), None);
    }

    #[test]
    fn a_redeclared_has_many_keeps_only_the_later_extension() {
        let user = ingest(
            "class User < ApplicationRecord\n  has_many :memberships do\n    def grant_to(rooms)\n    end\n  end\n  has_many :memberships\nend\n",
            "app/models/user.rb",
        );
        let assocs = build_assoc_registry(&[user]);
        let (user, memberships, grant_to) =
            (ClassId(Symbol::from("User")), Symbol::from("memberships"), Symbol::from("grant_to"));
        assert!(!assocs.is_assoc_extension(Some(&[&user]), &memberships, &grant_to));
        assert!(!assocs.is_assoc_extension(None, &memberships, &grant_to));
        assert!(!any_assoc_extensions(&assocs));
    }

    #[test]
    fn an_extension_belongs_to_the_owner_declaring_it() {
        // Only Room's `memberships` defines `grant_to`; a User's is
        // Rails' own proxy, and an owner of unknown type may be either.
        let room = ingest(
            "class Room < ApplicationRecord\n  has_many :memberships do\n    def grant_to(users)\n    end\n  end\nend\n",
            "app/models/room.rb",
        );
        let user = ingest("class User < ApplicationRecord\n  has_many :memberships\nend\n", "app/models/user.rb");
        let assocs = build_assoc_registry(&[room, user]);
        let (room, user) = (ClassId(Symbol::from("Room")), ClassId(Symbol::from("User")));
        let (memberships, grant_to) = (Symbol::from("memberships"), Symbol::from("grant_to"));
        assert!(assocs.is_assoc_extension(Some(&[&room]), &memberships, &grant_to));
        assert!(!assocs.is_assoc_extension(Some(&[&user]), &memberships, &grant_to));
        assert!(!assocs.is_assoc_extension(Some(&[&room, &user]), &memberships, &grant_to));
        assert!(assocs.is_assoc_extension(None, &memberships, &grant_to));
    }

    #[test]
    fn a_union_typed_owner_seeds_only_from_a_declaration_every_arm_reads() {
        let article = ingest("class Article < ApplicationRecord\n  has_many :comments\nend\n", "app/models/article.rb");
        let story = ingest("class Story < Article\nend\n", "app/models/story.rb");
        let tag = ingest("class Tag < ApplicationRecord\nend\n", "app/models/tag.rb");
        let assocs = build_assoc_registry(&[article, story, tag]);
        let owner_of = |arms: &[&str]| {
            let mut owner = syn(
                crate::span::Span::synthetic(),
                ExprNode::Ivar { name: Symbol::from("owner") },
            );
            owner.ty = Some(crate::ty::Ty::Union {
                variants: arms
                    .iter()
                    .map(|m| crate::ty::Ty::Class { id: ClassId(Symbol::from(*m)), args: vec![].into() })
                    .chain(std::iter::once(crate::ty::Ty::Nil))
                    .collect(),
            });
            owner
        };
        let comments = Symbol::from("comments");
        assert_eq!(assoc_read_target(&owner_of(&["Tag", "Article"]), &comments, &assocs), None);
        assert_eq!(
            assoc_read_target(&owner_of(&["Story", "Article"]), &comments, &assocs)
                .map(|(owner, _, fk)| (owner.map(|o| o.0.as_str().to_string()), fk.as_str().to_string())),
            Some((Some("Article".to_string()), "article_id".to_string()))
        );
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

    /// The Ruby family (`owner_objects`) hands the owner object to the
    /// automatic inverse instead: `Ban.create!(ip_address: ip, user: self)`.
    #[test]
    fn ruby_family_assoc_constructor_hands_owner_to_inverse() {
        let user = ingest("class User < ApplicationRecord\n  has_many :bans\nend\n", "app/models/user.rb");
        let ban = ingest("class Ban < ApplicationRecord\n  belongs_to :user\nend\n", "app/models/ban.rb");
        let models_v = vec![user, ban];
        let scopes = build_scope_registry(&models_v);
        let models = model_set(&models_v);
        let assocs = build_assoc_registry(&models_v);
        let bare_read = Expr::new(
            span(),
            ExprNode::Send { recv: None, method: Symbol::from("bans"), args: vec![], block: None, parenthesized: false },
        );
        let mut expr = Expr::new(
            span(),
            ExprNode::Send {
                recv: Some(bare_read),
                method: Symbol::from("create!"),
                args: vec![Expr::new(span(), ExprNode::Hash { entries: vec![], kwargs: true })],
                block: None,
                parenthesized: true,
            },
        );
        let owner = ClassId(Symbol::from("User"));
        let mut r = regs(&scopes, &models, &assocs);
        r.owner_objects = true;
        rewrite_call_site(&mut expr, &r, None, Some(&owner));
        let ExprNode::Send { args, .. } = &*expr.node else { panic!("expected Send, got {:?}", expr.node) };
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected a hash, got {:?}", args[0].node) };
        let (k, v) = entries.last().expect("an appended entry");
        assert!(matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user"), "key {:?}", k.node);
        assert!(matches!(&*v.node, ExprNode::SelfRef), "value {:?}", v.node);
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
    fn registry_belongs_to_join_uses_declared_primary_key() {
        // `belongs_to :product, foreign_key: "product_uuid",
        // primary_key: "uuid"` — Rails joins on `products.uuid`; the
        // `products.id` join compared a PG uuid with a varchar fk.
        let price = ingest(
            "class Price < ApplicationRecord\n  belongs_to :product, foreign_key: \"product_uuid\", primary_key: \"uuid\"\nend\n",
            "app/models/price.rb",
        );
        let product = ingest("class Product < ApplicationRecord\nend\n", "app/models/product.rb");
        let reg = build_assoc_registry(&[price, product]);
        let price = ClassId(Symbol::from("Price"));
        let product = Symbol::from("product");
        assert_eq!(
            reg.join_tail(&price, &product),
            Some(&"products ON products.uuid = prices.product_uuid".to_string())
        );
        assert_eq!(
            reg.aliased_join_tail(&price, &product),
            Some(&"products product ON product.uuid = prices.product_uuid".to_string())
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
