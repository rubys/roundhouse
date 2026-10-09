//! Type inference for Roundhouse IR.
//!
//! Two-level organization:
//! - [`body`] — Rails-agnostic body-typer: walks an `Expr` against a
//!   dispatch table + local `Ctx` and populates every node's `ty`.
//!   Runtime-extraction code calls into this directly.
//! - This module — the Rails dialect layer: builds a
//!   `HashMap<ClassId, ClassInfo>` from `App.models` (schemas,
//!   associations, conventions), orchestrates before_action chains,
//!   and runs the effects pass.
//!
//! MVP scope: annotate expression nodes whose types are derivable
//! from the receiver + method name against a table of known Rails /
//! Ruby method signatures. Unknown expressions get `Ty::Var(0)` as a
//! placeholder; the analyzer never fails, it just produces partial
//! information.
//!
//! What's deliberately out of scope for this pass:
//! - Narrowing through nil / class checks (coming next)
//! - Method return type inference (bodies typed; returns tabulated)
//! - Row-polymorphic parameter types
//! - Generic instantiation beyond `Array<Post>` etc.
//!
//! Each of those comes when a fixture forces it.

mod alba;
mod enum_raw_input;
mod body;
mod class_configuration;
mod data;
pub(crate) use body::string_answers;
pub(crate) use body::ConstResolverTask;
pub use body::PreparedConstResolver;
pub mod async_color;
pub mod attribution;
pub mod preload;
pub mod block_refine;
pub mod mutates_self;
mod registry;
mod test_module;
mod render;
mod ivar_set;
pub(crate) use ivar_set::controller_name_of;
mod effects;
mod diagnostics;
pub(crate) mod forwarding;
mod filter_targets;
pub mod graphql;
mod harvest_return;
mod fixpoint_bound;
mod fixpoint_check;
mod fixpoint_rounds;
pub use fixpoint_rounds::{FixpointRounds, LoopEnd};
mod dirty_retype;
mod typing_mode;
mod inferred_types;
pub mod inquiry;
pub use inferred_types::inferred_types;
pub use inquiry::inquirer_methods;
pub use diagnostics::{diagnose, diagnose_with_coverage};

pub use body::{BodyTyper, ClassInfo, ConstScope, Ctx};
pub(crate) use body::PARAM_VALUE;
use render::{
    collect_action_render_views, collect_content_partial_literals,
    collect_dynamic_render_ivars, content_partial_view_name,
    extract_partial_render_sites, is_layout_view_name, is_partial_view_name,
};
pub(crate) use body::union_of;
pub use preload::{missing_preload_report, PreloadCoverage};

use std::collections::{BTreeSet, HashMap};
use rubydex::model::identity_maps::IdentityHashMap;
use rubydex::model::ids::DeclarationId;

use crate::adapter::{DatabaseAdapter, SqliteAdapter};
use crate::App;
use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, LayoutDecl, MethodDef, MethodReceiver,
    Model, ModelBodyItem, RenderTarget,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::{ParamKind, Row, Ty};

pub struct Analyzer {
    classes: HashMap<ClassId, ClassInfo>,
    /// Inferred parameter types per (class, method). Empty after
    /// `Analyzer::new`; populated by `unify_params_from_call_sites`
    /// during the fixpoint loop in `analyze`. Consulted when seeding
    /// a method body's `Ctx::local_bindings` so subsequent typing
    /// passes resolve `Var { name }` against the discovered type
    /// instead of falling back to `Ty::Var` (the unknown sentinel).
    /// The Symbol key is the method name; the Vec aligns positionally
    /// with `MethodDef.params`.
    inferred_params: HashMap<ParamKey, Vec<Ty>>,
    /// (class, method) pairs an author wrote a signature for, as opposed
    /// to a `Fn` the registry stamped from the def itself. Only in the
    /// first is an `untyped` slot a statement rather than an inference
    /// gap.
    declared_signatures: std::collections::HashSet<(ClassId, Symbol)>,
    /// Backend-specific effect classification. The analyzer consults
    /// this when deciding whether a Send on an AR model carries
    /// `DbRead` or `DbWrite`. Defaults to `SqliteAdapter` via
    /// `Analyzer::new`; `Analyzer::with_adapter` lets callers plug
    /// in a different backend (Postgres, IndexedDB, D1, …) once
    /// those adapters land in Phase 2.
    adapter: Box<dyn DatabaseAdapter>,
    /// Method names the concern fold copied onto each includer
    /// (instance-side, class-side), per class. Distinguishes "the
    /// includer's own/catalog entry" (never overwritten by the fold)
    /// from "a copy the fold wrote last iteration" (overwritten so each
    /// fixpoint round's refinement of the module's returns propagates).
    concern_folded: HashMap<ClassId, (BTreeSet<Symbol>, BTreeSet<Symbol>)>,
    /// Method names the host fold copied onto each MODULE from its
    /// includers (see `fold_host_surfaces`). Kept out of what the
    /// concern fold copies back down, and rewritten every round.
    host_folded: HashMap<ClassId, BTreeSet<Symbol>>,
    /// Methods whose body ends in `.inquiry` — the evidence
    /// [`inquiry::is_inquiry_predicate`] needs to answer
    /// `content_type.attachment?` as Bool on a `Str` receiver.
    inquirers: std::collections::HashSet<Symbol>,
    /// Per-(controller, method) ivar bindings AS REFINED by Phase B,
    /// carried across fixpoint rounds.
    ///
    /// Phase A harvests every binding by typing the body against an
    /// EMPTY ivar context, so any binding that depends on another ivar
    /// comes out `Var` — `MessagesController#set_message` is
    /// `@message = @room.messages.find(…)`, and `@room` is a
    /// before_action's. Phase B fixes that for the controller it is
    /// looking at, but into a local, and Phase A rebuilds the table
    /// from scratch next round, so the refinement never reached a
    /// SUBCLASS: `Messages::ByBotsController#create` calls `super` and
    /// then reads `@message`, and the whole chain it inherits said
    /// `Var`. Persisting it here lets the whole-program fixpoint carry
    /// the answer the way it carries method returns.
    refined_action_bindings: HashMap<(ClassId, Symbol), HashMap<Symbol, Ty>>,
    /// Resolved once from the source snapshot supplied to `Analyzer::new`.
    const_resolver: std::sync::Arc<body::ConstResolver>,
    /// Explicit source-free API mode never certifies source indexing.
    source_indexed: bool,
    /// Inferred values keyed by Rubydex declaration IDs, not by names.
    typed_constants: IdentityHashMap<DeclarationId, Ty>,
    /// Literal Data constants on library classes, keyed by source span.
    data_factories: HashMap<crate::span::Span, Ty>,
    /// The controller/mailer→view channel as the last production
    /// typing pass harvested it. See [`ViewSeeds`].
    view_seeds: Option<ViewSeeds>,
    /// Reverse call graph, class-level: which classes call
    /// `(receiver class, method)`. Rebuilt from scratch by every
    /// `unify_params_from_call_sites` — same reason
    /// `inferred_params` is, it is a function of the trees the last
    /// typing pass wrote. Read by `dirty_classes_for_retype` to take
    /// a round's changed returns back to the bodies that read them.
    callers_by_target: HashMap<(ClassId, Symbol), std::collections::HashSet<ClassId>>,
    /// Per-controller concern/action bindings and bodies from the last
    /// round that retyped that controller. Reused when a clean
    /// controller still rebuilds channel metadata but skips concern
    /// `analyze_expr` walks.
    controller_action_meta_cache:
        HashMap<ClassId, (HashMap<Symbol, HashMap<Symbol, Ty>>, HashMap<Symbol, Expr>)>,
    /// How the last [`Self::analyze`]'s fixpoint loops ended.
    fixpoint_rounds: FixpointRounds,
    /// What the opt-in fixpoint canaries saw (`fixpoint_check`).
    fixpoint_checks: fixpoint_check::Checks,
}

use dirty_retype::{DirtyHints, InferenceSig, dirty_classes_for_retype};
use typing_mode::{TypingMode, UnifyScope, ViewSeeds};

impl Analyzer {
    /// Build an analyzer with the default database adapter
    /// (`SqliteAdapter`). Matches pre-adapter-refactor behavior —
    /// every target that shipped before Phase 2 targets sqlite, so
    /// the default preserves the status quo.
    pub fn new(app: &App) -> Self {
        Self::with_adapter(app, Box::new(SqliteAdapter))
    }

    /// Build an analyzer with a specific database adapter. Use this
    /// once non-sqlite adapters exist and you want effect inference
    /// to reflect that backend's capability profile.
    pub fn with_adapter(app: &App, adapter: Box<dyn DatabaseAdapter>) -> Self {
        let mut classes: HashMap<ClassId, ClassInfo> = HashMap::new();

        // Module → its own `include`s, for chasing concern-of-concern
        // chains when registering concern-declared model DSL below.
        let module_include_map: HashMap<&ClassId, &Vec<ClassId>> = app
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .map(|lc| (&lc.name, &lc.includes))
            .collect();

        // Parent links for the AR-descent walk, built once: a model's
        // superclass is usually another model (`Story` → `Application
        // Record` → `ActiveRecord::Base`), so the walk needs every
        // model's parent before any single model is seeded.
        let model_parents: HashMap<&ClassId, Option<&ClassId>> =
            app.models.iter().map(|m| (&m.name, m.parent.as_ref())).collect();

        for model in &app.models {
            let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
            let array_of_self =
                Ty::Array { elem: Box::new(self_ty.clone()) };
            let relation_of_self = Ty::Relation { of: model.name.clone() };
            // Is this class actually an ActiveRecord model?
            //
            // Everything under `app/models` arrives here as a `Model`,
            // including plain objects that merely LIVE there — lobsters'
            // `Search` (`include ActiveModel::Validations`, `attr_accessor
            // :results, :page, …`) and campfire's `Opengraph::Location` /
            // `Opengraph::Metadata`. Seeding the AR query surface onto
            // those fabricates methods the class does not have, and the
            // fabrication is not inert: an instance receiver resolves
            // `class_methods` BEFORE `instance_methods` (the parent-chain
            // walk in `body/send.rs`), so `@search.page` reached the
            // class-side `page` builder instead of the `attr_accessor`.
            // That mistyped as `Array[Search]` for as long as chain starts
            // were Array-shaped — wrong, but renderable. Convergence
            // (docs/relation-convergence-plan.md C1) made it
            // `Relation[Search]`, which no emitter can render, so a latent
            // wrong answer became a hard error and the gap became visible.
            //
            // The discriminator is INHERITANCE, not the schema. "Has no
            // table in schema.rb" reads like the same question and is not:
            // plenty of genuine models are analyzed without a schema at
            // all (every fixture in tests/analyze.rs that doesn't bother
            // declaring one), and stripping their query surface breaks
            // real dispatch to fix nothing. Descent from `ActiveRecord::
            // Base` is the property that actually decides whether the
            // class has that API.
            //
            // What survives the gate is the catalog's own effect
            // classification: a non-AR class gets no DB-effecting method,
            // and `EffectClass::Pure` — `new`, `instantiate`,
            // `schema_column_names` — stays, which is what keeps
            // `Opengraph::Location.new(image)` typed.
            let is_ar_model = descends_from_active_record(&model.name, &model_parents);

            let mut cls = ClassInfo::default();
            cls.table = Some(model.table.clone());
            cls.attributes = model.attributes.clone();
            // The key's type as the schema declares it (#90): the
            // attribute row already carries it under the key column's
            // name — `id`, or the column `create_table primary_key:`
            // named — and `ids` / `id` follow it rather than assuming
            // an integer. No key in the row (a schema-less test model)
            // keeps Rails' default.
            let key_ty = app
                .schema
                .tables
                .get(&model.table.0)
                .and_then(|t| t.columns.iter().find(|c| c.primary_key))
                .and_then(|c| model.attributes.fields.get(&c.name).cloned())
                .unwrap_or(Ty::Int);

            // AR class-method signatures sourced from the shared
            // catalog (`crate::catalog::AR_CATALOG`). Each entry
            // with a declared `ReturnKind` gets instantiated
            // against this model's Self type and inserted into
            // `class_methods`. Entries with `return_kind = None`
            // are skipped — they exist in the catalog for effect
            // classification but don't (yet) declare their return
            // types. Centralizing the data source here eliminates
            // drift between the previous inline list and the
            // catalog; adding an AR method to the catalog with a
            // return_kind automatically enables it for type
            // inference downstream.
            use crate::catalog::{AR_CATALOG, ReceiverContext, ReturnKind};
            let instantiate =
                |kind: ReturnKind| -> Ty { instantiate_return_kind(kind, &model.name) };
            for entry in AR_CATALOG {
                if entry.receiver != ReceiverContext::Class {
                    continue;
                }
                // No table, no database surface — see `has_table`.
                if !is_ar_model && entry.effect != crate::catalog::EffectClass::Pure {
                    continue;
                }
                let Some(kind) = entry.return_kind else { continue };
                cls.class_methods.insert(Symbol::from(entry.name), instantiate(kind));
            }
            // AR class-side framework methods not yet in the catalog.
            // `Model.transaction { ... }` runs the block in a DB
            // transaction; we don't model the block-yield through
            // catalog metadata so it sits here. `connection` returns
            // an AR connection adapter (gradual). `establish_connection`
            // / `connection_pool` similarly. Block-yielding ones
            // return whatever the block returned, which we don't
            // statically track — Untyped is the gradual escape.
            cls.class_methods.insert(Symbol::from("transaction"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("connection"), registry::ar::connection_ty());
            cls.class_methods.insert(Symbol::from("connection_pool"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("establish_connection"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("table_name"), Ty::Str);
            // `Model.human_attribute_name(:col)` — the ActiveModel
            // translation entry point every form label and table header
            // goes through. Returns the humanized/localized String.
            cls.class_methods.insert(Symbol::from("human_attribute_name"), Ty::Str);
            cls.class_methods.insert(Symbol::from("primary_key"), Ty::Str);
            // Arel entry points: `Model.arel_table` is an `Arel::Table`
            // (`table[:col]` → attribute → predicate node); `Model.arel`
            // (and `relation.arel`, handled in send.rs) is the underlying
            // `Arel::SelectManager`. Typed (not `Untyped`) so advanced
            // scopes that drop into Arel stay typed end-to-end.
            cls.class_methods.insert(
                Symbol::from("arel_table"),
                Ty::Class { id: ClassId(Symbol::from("Arel::Table")), args: vec![] },
            );
            cls.class_methods.insert(
                Symbol::from("arel"),
                Ty::Class { id: ClassId(Symbol::from("Arel::SelectManager")), args: vec![] },
            );
            cls.class_methods.insert(Symbol::from("attribute_names"), Ty::Array { elem: Box::new(Ty::Str) });
            cls.class_methods.insert(Symbol::from("column_names"), Ty::Array { elem: Box::new(Ty::Str) });
            cls.class_methods.insert(Symbol::from("columns_hash"), Ty::Untyped);
            // The rest of the schema-reflection surface every model has:
            // `columns` is the list of `ActiveRecord::ConnectionAdapters::Column`
            // objects (not modelled, so their elements stay untyped), the
            // `sanitize_sql*` family builds a SQL fragment string, and
            // `base_class` is the STI root, a class.
            cls.class_methods.insert(Symbol::from("columns"), Ty::Array { elem: Box::new(Ty::Untyped) });
            for sanitizer in ["sanitize_sql", "sanitize_sql_array", "sanitize_sql_for_conditions", "sanitize_sql_like"] {
                cls.class_methods.insert(Symbol::from(sanitizer), Ty::Str);
            }
            cls.class_methods.insert(Symbol::from("base_class"), Ty::Class { id: ClassId(Symbol::from("Class")), args: vec![] });
            let class_ty = Ty::Class { id: ClassId(Symbol::from("Class")), args: vec![] };
            for family in ["descendants", "subclasses"] {
                cls.class_methods.insert(Symbol::from(family), Ty::Array { elem: Box::new(class_ty.clone()) });
            }
            for text in ["quoted_table_name", "inheritance_column", "sti_name"] {
                cls.class_methods.insert(Symbol::from(text), Ty::Str);
            }
            for flag in ["abstract_class?", "table_exists?"] {
                cls.class_methods.insert(Symbol::from(flag), Ty::Bool);
            }
            // `ActiveModel::Name`, the `Type::Value`s and the enum table are
            // objects the registry does not model.
            for opaque in ["model_name", "attribute_types", "type_for_attribute", "defined_enums", "reset_column_information"] {
                cls.class_methods.insert(Symbol::from(opaque), Ty::Untyped);
            }
            // The rest of the class-side query surface — everything
            // from here to the `ids` seed below reads or writes the
            // database, so it is gated on `is_ar_model` for the same
            // reason the DB-effecting catalog entries above are. The
            // catalog can't gate these because they aren't in it.
            if is_ar_model {
                // `Model.unscoped`/`Model.none` return a relation
                // (`Relation { of: Model }`) so chains through them stay
                // typed instead of leaking to `untyped`. (The block form
                // `unscoped { }` returns the block value, which we don't
                // track — the relation type is the better default for the
                // common bare/chained use.) `delete_all`/`update_all` return
                // Int (affected row count).
                cls.class_methods.insert(Symbol::from("unscoped"), relation_of_self.clone());
                cls.class_methods.insert(Symbol::from("none"), relation_of_self.clone());
                cls.class_methods.insert(Symbol::from("delete_all"), Ty::Int);
                cls.class_methods.insert(Symbol::from("update_all"), Ty::Int);

                // Chainable query-builder methods beyond the catalog set.
                // Each returns the relation (`Relation { of: Self }`, the
                // same representation the catalog's Class-context builders
                // use), so a scope or controller chain types end-to-end
                // rather than leaking to `untyped` at the first
                // uncatalogued link, and Relation-receiver dispatch in
                // `send.rs` resolves the next step.
                // `entry().or_insert` so a catalog entry or named scope still
                // wins. `not`/`missing` are really `WhereChain` methods
                // (`where.not(...)`/`where.missing(...)`); since `where`
                // already yields the relation, the chain lands on the
                // relation and these resolve there — typing `Model.not`
                // directly is harmless (not real code) and beats `untyped`.
                for builder in [
                    "or", "and", "rewhere", "reorder", "reselect", "regroup",
                    "except", "only", "unscope", "reverse_order", "left_joins",
                    "readonly", "lock", "from", "extending", "strict_loading",
                    "create_with", "annotate", "optimizer_hints",
                    "not", "missing",
                ] {
                    cls.class_methods
                        .entry(Symbol::from(builder))
                        .or_insert_with(|| relation_of_self.clone());
                }

                // Relation-terminal methods Rails delegates from the class to
                // `all` (`Story.find_each`, `Category.pluck(:name)`). Unlike the
                // builders above these don't return a relation, so they sit
                // outside that loop; their return types match the `Array<Self>`
                // (relation) dispatch in send.rs so a class-side call and the
                // equivalent `.all`-chained call agree. `entry().or_insert` so a
                // catalog entry or named scope still wins. `find_each` &
                // friends yield the element to a block and return the relation
                // for chaining; `pluck`/`pick` project column values (column
                // type unknowable from the name alone → `Array<Untyped>`);
                // `ids` projects primary keys.
                for batch in ["find_each", "find_in_batches", "in_batches"] {
                    cls.class_methods
                        .entry(Symbol::from(batch))
                        .or_insert_with(|| array_of_self.clone());
                }
                for proj in ["pluck", "pick"] {
                    cls.class_methods
                        .entry(Symbol::from(proj))
                        .or_insert_with(|| Ty::Array { elem: Box::new(Ty::Untyped) });
                }
                cls.class_methods
                    .entry(Symbol::from("ids"))
                    .or_insert_with(|| Ty::Array { elem: Box::new(key_ty.clone()) });
            } // end `if is_ar_model` — class-side query surface

            // Rails' `id` reads the primary-key attribute whatever the
            // column is called: a `create_table primary_key: "identifier"`
            // model has no `id` column, and `record.id` is still the
            // key. The row registered below carries the key under its
            // own name; `id` is aliased to it here.
            if is_ar_model {
                cls.instance_methods.entry(Symbol::from("id")).or_insert(key_ty.clone());
            }

            // Instance methods from schema-derived attributes.
            // These are per-model (column names differ across
            // models), so they stay outside the catalog — the
            // catalog is for per-receiver-kind AR methods, not
            // per-model schema projections.
            //
            // Each column `name` also produces Rails-generated
            // accessors: `name?` (presence predicate, Bool) and
            // `name=` (writer, returns the assigned value). Register
            // all three so `@user.is_admin` (column read),
            // `@user.is_admin?` (predicate), and `@user.user_id = x`
            // (writer) all resolve.
            for (name, ty) in &model.attributes.fields {
                let n = name.as_str();
                // Not the stored value: an enum's reader answers its label, or nil for a value no label names.
                let reader_ty = if crate::dialect::enum_reads_label(model, name) {
                    Ty::Union { variants: vec![Ty::Str, Ty::Nil] }
                } else {
                    ty.clone()
                };
                cls.instance_methods.insert(name.clone(), reader_ty);
                if model.enums.contains_key(name) {
                    cls.instance_methods.entry(Symbol::from(format!("{n}_before_type_cast"))).or_insert(ty.clone());
                }
                let predicate = Symbol::from(format!("{n}?"));
                cls.instance_methods.entry(predicate).or_insert(Ty::Bool);
                let writer = Symbol::from(format!("{n}="));
                cls.instance_methods.entry(writer).or_insert(ty.clone());
                // ActiveModel::Dirty per-attribute methods Rails generates
                // for every column: `<col>_changed?`,
                // `<col>_previously_changed?`, `saved_change_to_<col>?`
                // (predicates) and `<col>_was` (the prior value).
                for suffix in ["_changed?", "_previously_changed?"] {
                    cls.instance_methods
                        .entry(Symbol::from(format!("{n}{suffix}")))
                        .or_insert(Ty::Bool);
                }
                cls.instance_methods
                    .entry(Symbol::from(format!("saved_change_to_{n}?")))
                    .or_insert(Ty::Bool);
                // `will_save_change_to_<col>?` — the before-save twin of
                // `saved_change_to_<col>?`, the same question as
                // `<col>_changed?`.
                cls.instance_methods
                    .entry(Symbol::from(format!("will_save_change_to_{n}?")))
                    .or_insert(Ty::Bool);
                cls.instance_methods
                    .entry(Symbol::from(format!("{n}_was")))
                    .or_insert(ty.clone());
                // `<col>_previously_was` — the after-commit twin of
                // `<col>_was`, generated by ActiveModel::Dirty since
                // Rails 6.0 and SYNTHESIZED by
                // `model_to_library::schema` (which also gives it a
                // hydration baseline so it answers the prior value
                // rather than nil). It was the one member of the family
                // the registry did not name, so campfire's
                // `@membership.involvement_previously_was.inquiry
                // .invisible?` reported `no known method` against a
                // reader the emitted model has.
                cls.instance_methods
                    .entry(Symbol::from(format!("{n}_previously_was")))
                    .or_insert(ty.clone());
            }
            // `typed_store` (activerecord-typedstore) accessors: declared in a
            // DSL block, backed by a serialized column, so absent from the
            // schema-derived attributes above. Register them as typed methods.
            register_typed_store(&model.body, &mut cls.instance_methods);
            // `has_json` schema keys — same situation, JSON instead of
            // YAML, plus the one type the declaration erases.
            register_has_json(&model.body, &mut cls.instance_methods);
            // `serialize :col, coder: …` — the column reads back as the
            // coder's object, not the storage text.
            register_serialized_columns(&model.body, &mut cls.instance_methods);
            // `attribute :name, :type` virtual attributes (ActiveModel) —
            // backed by something other than a schema column, so absent
            // from `model.attributes` above.
            register_ar_attributes(&model.body, &mut cls.instance_methods);
            // Plain `attr_accessor :previewing, :vote, …` virtual attributes:
            // real methods at runtime, absent from the schema, untyped. Register
            // reader/writer as gradual (`Untyped`) so dispatch resolves them.
            register_attr_accessors(&model.body, &mut cls.instance_methods);
            // `has_secure_password` generates `password=`/
            // `password_confirmation=` writers + `authenticate`.
            register_has_secure_password(&model.body, &mut cls.instance_methods, &mut cls.class_methods, &self_ty);
            // `generates_token_for :purpose` — the token round-trip
            // Rails 7.1 added (the guide's unsubscribe link).
            register_generates_token_for(model, &mut cls.instance_methods, &mut cls.class_methods, &self_ty);
            // `has_rich_text :body` generates the reader/predicate/
            // writer and the scoped has_one behind them.
            register_has_rich_text(model, &mut cls.instance_methods);
            // Named plain-text association (`has_markdown`) — same surface.
            register_plain_text_attr(model, &app.schema, &mut cls.instance_methods);

            // Named scopes resolve as relation-returning class methods, so
            // `Story.active` types and chains like `Story.active.recent`
            // compose. A scope whose body tail is a query-builder chain
            // seeds `Ty::Relation { of: Self }` — the true lazy-relation
            // type, which Relation-receiver dispatch chains and
            // class-side delegation resolve. A body this classifier
            // can't recognize (terminal tail, block-taking hop,
            // cross-model root) keeps the legacy `Array[Self]` stand-in,
            // whose `Array[Class]` dispatch delegates the same way.
            // Scope bodies are typed separately; this only records the
            // call surface. `or_insert` so an explicit catalog method
            // still wins.
            let scope_names: std::collections::HashSet<Symbol> =
                model.scopes().map(|s| s.name.clone()).collect();
            // Materializing scopes propagate through sibling chains:
            // `scope :page_before, ->(m) { before(m).last_page }` ends in
            // a sibling that ends in `last(PAGE_SIZE)`, and answers that
            // sibling's Array, not a relation. Classify the terminal
            // scopes first, then let each scope whose tail is a
            // materializing sibling (on a relation chain rooted here)
            // inherit that sibling's seed — to a fixpoint, so a chain of
            // such scopes resolves whatever order the file declares them.
            let mut materializing: HashMap<Symbol, Ty> = HashMap::new();
            for scope in model.scopes() {
                if body_tail_terminal_kind(&scope.body, &model.name, &scope_names).is_some() {
                    let seed = scope_return_seed(&scope.body, &model.name, &scope_names);
                    materializing.insert(scope.name.clone(), seed);
                }
            }
            loop {
                let mut changed = false;
                for scope in model.scopes() {
                    if materializing.contains_key(&scope.name) {
                        continue;
                    }
                    if let Some(seed) =
                        scope_tail_materializing_sibling(&scope.body, &model.name, &scope_names, &materializing)
                    {
                        materializing.insert(scope.name.clone(), seed);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            for scope in model.scopes() {
                let seed = match materializing.get(&scope.name) {
                    Some(seed) => seed.clone(),
                    None => scope_return_seed(&scope.body, &model.name, &scope_names),
                };
                cls.class_methods.entry(scope.name.clone()).or_insert(seed);
                cls.relation_derived.insert(scope.name.clone());
                if materializing.contains_key(&scope.name) {
                    cls.materializing_scopes.insert(scope.name.clone());
                }
            }
            // `has_rich_text :body` declares two preload scopes beside
            // the association (`with_rich_text_body`,
            // `with_rich_text_body_and_embeds`). They are relation-
            // returning like any other scope; the bodies are synthesized
            // at the ruby emit seam.
            for name in crate::lower::rich_text::preload_scope_names(model) {
                cls.class_methods
                    .entry(name)
                    .or_insert(Ty::Relation { of: model.name.clone() });
            }
            if crate::lower::plain_text_attr::record_table_present(&app.schema) {
                for name in crate::lower::plain_text_attr::preload_scope_names(model) {
                    cls.class_methods
                        .entry(name)
                        .or_insert(Ty::Relation { of: model.name.clone() });
                }
            }
            // `has_one_attached :avatar` declares one the same way
            // (`with_attached_avatar`). Same registration, same reason.
            for name in crate::lower::attached::preload_scope_names(model) {
                cls.class_methods
                    .entry(name)
                    .or_insert(Ty::Relation { of: model.name.clone() });
            }
            // And the READER the same macro declares. `lower::attached`
            // synthesizes `def avatar; ActiveStorage::Attached.new(…);
            // end` at the emit seam — after this walk — so without the
            // registration here the method exists in the emitted tree
            // and not in the analyzer's world, which reports it as
            // missing. Registering a synthesized name in BOTH registries
            // is the rule; this one had only half of it.
            for (_span, attr) in crate::lower::attached::attached_attrs(model) {
                cls.instance_methods.entry(attr).or_insert(Ty::Class {
                    id: ClassId(Symbol::from("ActiveStorage::Attached")),
                    args: vec![],
                });
            }
            for (_span, attr) in crate::lower::attached::many_attached_attrs(model) {
                cls.instance_methods.entry(attr).or_insert(Ty::Class {
                    id: ClassId(Symbol::from("ActiveStorage::AttachedMany")),
                    args: vec![],
                });
            }
            // `ActiveStorage::Attachment` helpers synthesized by
            // `lower::attachment_model::push_attachment_record_methods`
            // at the emit seam — register here so `attachment.url` /
            // `.filename` resolve in check the same way the reader
            // macros do.
            if crate::lower::attachment_model::is_attachment_model(model) {
                let blob = Ty::Class {
                    id: ClassId(Symbol::from("ActiveStorage::Blob")),
                    args: vec![],
                };
                let filename = Ty::Class {
                    id: ClassId(Symbol::from("ActiveStorage::Filename")),
                    args: vec![],
                };
                let nilable = |ty: Ty| Ty::Union {
                    variants: vec![ty, Ty::Nil],
                };
                cls.instance_methods
                    .entry(Symbol::from("blob"))
                    .or_insert(nilable(blob));
                cls.instance_methods
                    .entry(Symbol::from("url"))
                    .or_insert(Ty::Str);
                cls.instance_methods
                    .entry(Symbol::from("filename"))
                    .or_insert(nilable(filename));
                cls.instance_methods
                    .entry(Symbol::from("content_type"))
                    .or_insert(nilable(Ty::Str));
            }
            // `attr_accessor :x` — and `attr_accessor *CONST`, which is
            // how campfire's `Opengraph::Metadata` names its four. The
            // reader/writer pair is synthesized by
            // `lower::model_to_library::markers::push_attr_accessor_methods`
            // at the emit seam, AFTER this walk, so the rule the
            // `attached` reader above states applies here too: a name
            // the pipeline writes must be registered where the analyzer
            // can see it, or the method exists in the emitted tree and
            // nowhere else. `self.title = sanitize(strip_tags(title))`
            // in that model's own `sanitize_fields` reported `no known
            // method title= on Class(Opengraph::Metadata)`.
            //
            // `declared_attr_names` is the LOWERING's list, CALLED
            // rather than re-derived: it is the only place that knows
            // the splat form names four fields, and a second copy would
            // go stale the first time either moved.
            //
            // `Untyped` because an `attr_*` declares no type — the same
            // answer the emitted RBS gives it (`attr_reader title:
            // untyped`), and the gradual escape rather than a Var so
            // chains off the reader resolve.
            for name in crate::lower::model_to_library::markers::declared_attr_names(model) {
                let writer = Symbol::from(format!("{}=", name.as_str()));
                cls.instance_methods.entry(name).or_insert(Ty::Untyped);
                cls.instance_methods.entry(writer).or_insert(Ty::Untyped);
            }
            // `attachable_sgid` — the signed GlobalID an
            // `ActionText::Attachable` model mints. Registered for every
            // model rather than only the attachable ones: this walk has
            // no `App`, and the method's TYPE is the same either way;
            // whether it EXISTS is decided at the emit seam, where the
            // include chain is resolvable (`lower::attachable`).
            cls.instance_methods
                .entry(Symbol::from("attachable_sgid"))
                .or_insert(Ty::Str);
            // `to_param` — every concrete model gets a synthesized
            // `id.to_s` (or its own override) at the emit seam
            // (`lower::model_to_library::markers::push_to_param_method`).
            // Register here so analyzer dispatch matches that surface
            // (campfire's `user.to_param` in AvatarsHelper) instead of
            // the Object-extension refusal for an unmodeled call.
            cls.instance_methods
                .entry(Symbol::from("to_param"))
                .or_insert(Ty::Str);
            // Core AR instance methods every model gets. Sourced
            // from the shared catalog — same mechanism as class
            // methods above. Covers mutation (save/update/destroy),
            // state reload, validity predicates, attributes, and
            // errors.
            for entry in AR_CATALOG {
                if entry.receiver != ReceiverContext::Instance {
                    continue;
                }
                let Some(kind) = entry.return_kind else { continue };
                cls.instance_methods.insert(Symbol::from(entry.name), instantiate(kind));
            }
            // AR instance methods not (yet) in the catalog: dirty-tracking
            // snapshots, mass assignment, marked-for-destruction, and the
            // timestamp/column writers. `Bool` for predicates/persistence
            // writers; `Untyped` (gradual) where the return is a
            // heterogeneous changes-hash. Mirrors the class-side Untyped
            // block above. `or_insert` so a catalog entry always wins.
            for (name, ty) in [
                ("marked_for_destruction?", Ty::Bool),
                ("mark_for_destruction", Ty::Bool),
                ("record_timestamps=", Ty::Bool),
                ("attributes=", Ty::Untyped),
                ("assign_attributes", Ty::Untyped),
                ("update_column", Ty::Bool),
                ("update_columns", Ty::Bool),
                ("saved_changes", Ty::Untyped),
                ("saved_changes?", Ty::Bool),
                ("changes", Ty::Untyped),
                ("previous_changes", Ty::Untyped),
                ("changed_attributes", Ty::Untyped),
                ("changed", Ty::Array { elem: Box::new(Ty::Str) }),
                // Turbo::Broadcastable, which turbo-rails mixes into
                // `ActiveRecord::Base` — so every model answers these,
                // and `lower::model_to_library::broadcasts` rewrites
                // each one to the `Broadcasts.<action>` call the
                // runtime implements. The rewrite reaches a controller
                // body as readily as a model's, which is where campfire
                // writes three of them
                // (`@boost.broadcast_append_to room, :messages, …`), so
                // the analyzer not knowing the name left a hard error
                // on a call the emitted tree gets right. They answer
                // the broadcast, which nothing consumes: `Nil`.
                ("broadcast_append_to", Ty::Nil),
                ("broadcast_prepend_to", Ty::Nil),
                ("broadcast_replace_to", Ty::Nil),
                ("broadcast_update_to", Ty::Nil),
                ("broadcast_remove_to", Ty::Nil),
            ] {
                cls.instance_methods.entry(Symbol::from(name)).or_insert(ty);
            }
            // Associations as instance methods (return types derived from
            // cardinality). Each also gets a writer `name=`: belongs_to/
            // has_one assign a record-or-nil, has_many/HABTM assign a
            // collection. The writer was previously absent, so
            // `comment.story = s` / `tag.category = c` failed dispatch.
            for assoc in model.associations() {
                let (name, ty) = association_member_ty(assoc);
                let writer = Symbol::from(format!("{}=", name.as_str()));
                // An association EXTENSION BLOCK's methods, keyed by
                // the pair, because neither class can hold them: the
                // owner does not answer `grant_to` and neither does
                // the target. `Untyped` is the honest value — the
                // method exists (`lower::model_to_library::
                // associations` flattens it onto the owner as
                // `<assoc>_<name>`), and its RETURN stays gradual
                // until the extension bodies are typed alongside the
                // association proxy work. campfire discards all three
                // (`grant_to`, `revoke_from`, `revise`).
                if let crate::dialect::Association::HasMany { extension, .. } = assoc {
                    for m in extension {
                        cls.assoc_extensions
                            .entry((name.clone(), m.name.clone()))
                            .or_insert(Ty::Untyped);
                    }
                    // Flat `<name>_loaded?` — same Bool `model_to_library`
                    // synthesizes for emit. Registered here so `check`
                    // (no post-analyze lower) can type
                    // `message.boosts.loaded?` via `assoc_loaded_ty`
                    // without cataloguing Relation `#loaded?`.
                    cls.instance_methods
                        .entry(Symbol::from(format!("{}_loaded?", name.as_str())))
                        .or_insert(Ty::Bool);
                    cls.instance_methods
                        .entry(Symbol::from(format!("reload_{}", name.as_str())))
                        .or_insert(ty.clone());
                }
                // The singular readers' flat `<name>_loaded?` (Rails'
                // `association(:name).loaded?`), synthesized beside them.
                if matches!(
                    assoc,
                    crate::dialect::Association::HasOne { .. }
                        | crate::dialect::Association::BelongsTo { polymorphic: false, .. }
                ) {
                    cls.instance_methods
                        .entry(Symbol::from(format!("{}_loaded?", name.as_str())))
                        .or_insert(Ty::Bool);
                }
                cls.instance_methods.insert(name, ty.clone());
                cls.instance_methods.entry(writer).or_insert(ty);
                for (name, ty) in association_builder_members(assoc) {
                    cls.instance_methods.entry(name).or_insert(ty);
                }
            }

            // Concern-declared model DSL: associations and scopes a
            // mixed-in module's `included do` contributes
            // (Account::Associations' `has_many :statuses`). Registered
            // exactly like the model's own declarations — typed readers
            // + writers for associations, relation-returning class
            // methods for scopes — with the model's own entries winning
            // on a name clash. Includes close transitively (concerns
            // include concerns).
            let includes = model_includes(model);
            {
                let mut queue: Vec<ClassId> = includes.clone();
                let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
                let mut qi = 0;
                while qi < queue.len() {
                    let m = queue[qi].clone();
                    qi += 1;
                    if let Some(nested) = module_include_map.get(&m) {
                        for n in nested.iter() {
                            if seen.insert(n.clone()) {
                                queue.push(n.clone());
                            }
                        }
                    }
                    let Some(items) = app.concern_model_items.get(&m) else { continue };
                    for item in items {
                        match item {
                            ModelBodyItem::Association { assoc, .. } => {
                                let (name, ty) = association_member_ty(assoc);
                                let writer = Symbol::from(format!("{}=", name.as_str()));
                                if matches!(
                                    assoc,
                                    crate::dialect::Association::HasMany { .. }
                                ) {
                                    cls.instance_methods
                                        .entry(Symbol::from(format!(
                                            "{}_loaded?",
                                            name.as_str()
                                        )))
                                        .or_insert(Ty::Bool);
                                }
                                cls.instance_methods.entry(name).or_insert(ty.clone());
                                cls.instance_methods.entry(writer).or_insert(ty);
                                for (name, ty) in association_builder_members(assoc) {
                                    cls.instance_methods.entry(name).or_insert(ty);
                                }
                            }
                            ModelBodyItem::Scope { scope, .. } => {
                                cls.class_methods
                                    .entry(scope.name.clone())
                                    .or_insert(array_of_self.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }

            // `include Account::FinderConcern` etc. — record the mixins
            // so the concern fold (harvest_returns_to_registry) can copy
            // the module's instance and `class_methods do` surfaces onto
            // this model.
            if !includes.is_empty() {
                cls.includes = includes;
            }

            // Not every parent: only another app model, so an abstract base's enums and methods reach the classes below it.
            if let Some(parent) = model.parent.as_ref().filter(|p| model_parents.contains_key(p)) {
                cls.parent = Some(parent.clone());
            }

            classes.insert(model.name.clone(), cls);
        }

        // ActiveRecord::Base literal class, CollectionProxy runtime helper,
        // ActiveRecord::AdapterInterface contract, and the Arel node family —
        // see `registry::ar`.
        registry::ar::register(&mut classes);
        registry::ar::register_action_text(&mut classes);
        registry::ar::register_action_cable(&mut classes);

        // ActiveModel::Validations / Model modules + the ActiveModel::Errors
        // collection and individual ActiveModel::Error classes — see
        // `registry::activemodel`.
        registry::activemodel::register(&mut classes);

        // ActionView view context — the FormBuilder, the mime-responds
        // Collector, the ActionView::Base flat-helper accumulator, and the
        // FlashHash class. See `registry::view`. Route helper names are
        // derived once here (via `registry::routes`) and shared with the
        // controller and library-class registrations below.
        let route_helper_names: Vec<String> = registry::routes::route_helper_names(app);
        registry::view::register(&mut classes, app, &route_helper_names);

        // Rails/Time/Date singletons, Ruby stdlib singletons, and the
        // gem-ecosystem catalog fold — see `registry::stdlib`.
        registry::stdlib::register(&mut classes);

        // ActionController::Base.helpers proxy + the hardcoded
        // ApplicationController surface (params/session/render, flash,
        // respond_to, route helpers, Devise scope helpers) — see
        // `registry::controllers`. Runs after view::register because the
        // Devise fold also augments ActionView::Base.
        registry::controllers::register(&mut classes, app, &route_helper_names);

        // User-authored RBS sidecars. Signatures discovered under
        // `sig/**/*.rbs` at ingest time apply on top of the hardcoded
        // catalog — later entries win, so RBS overrides conventions
        // when both declare the same method. All RBS methods land in
        // `instance_methods` since dispatch consults both tables and
        // parse_app_signatures doesn't yet distinguish singleton vs
        // instance; per-kind separation is a follow-up when it matters.
        for (class_id, methods) in &app.rbs_signatures {
            let cls = classes.entry(class_id.clone()).or_default();
            for (name, ty) in methods {
                cls.instance_methods.insert(name.clone(), ty.clone());
            }
        }

        // Library classes (route-helper/Singleton includes, superclass links),
        // ActionMailer classes, ActiveJob classes, and Sidekiq workers —
        // see `registry::library`.
        registry::library::register(&mut classes, app, &route_helper_names);

        // A helper object built from a YAML table
        // (`WebUrlHelpers = Factory.create(YAML.load_file(…)["paths"])`):
        // one path/URL String per generated name. See
        // `ingest::generated_helpers`.
        for (constant, names) in &app.generated_helper_methods {
            let cls = classes.entry(constant.clone()).or_default();
            for name in names {
                cls.class_methods.entry(name.clone()).or_insert(Ty::Str);
            }
        }

        // Controllers: register each as a known class so self-method
        // dispatch (a bare `find_story` inside an action) resolves against
        // the controller's own methods and walks the parent chain to the
        // hardcoded ApplicationController surface (params/session/render).
        // Return types are filled by `harvest_returns_to_registry` during
        // the fixpoint; here we only establish the class and its parent
        // link. `or_default` preserves the hardcoded ApplicationController
        // entry when a real `application_controller.rb` is also present —
        // we only set its parent, never clobber its methods.
        for controller in &app.controllers {
            let includes = controller_includes(controller);
            let cls = classes.entry(controller.name.clone()).or_default();
            if controller.parent.is_some() {
                cls.parent = controller.parent.clone();
            }
            // `include IntervalHelper` etc. — mixed-in helper methods
            // (e.g. `time_interval`) are callable via implicit self in
            // every action. Recording the mixin lets dispatch resolve
            // them against the helper's registered instance methods.
            if !includes.is_empty() {
                cls.includes = includes;
            }
        }

        // Classes a locked gem's RBI declares. After everything the app
        // and the catalog register, so it only ever adds.
        registry::gem_boundary::register(&mut classes, app);

        // Which registered classes the app itself declares: a bare name
        // reaches those through Ruby's constant lookup only (see
        // Rubydex).
        for id in app
            .models
            .iter()
            .map(|m| &m.name)
            .chain(app.controllers.iter().map(|c| &c.name))
            .chain(app.library_classes.iter().map(|lc| &lc.name))
        {
            if let Some(info) = classes.get_mut(id) {
                info.app_declared = true;
            }
        }
        // Not wholly the app's: declaring a constant an unmodeled gem defines reopens it.
        if let Some(census) = app.gem_lock.as_ref().map(crate::gems::GemCensus::of) {
            for (id, info) in classes.iter_mut() {
                if info.app_declared && info.parent.is_none() && crate::gems::gem_defining_constant(&census, id.0.as_str()).is_some() {
                    info.open = true;
                }
            }
        }

        for (id, method) in app.models.iter()
            .flat_map(|model| model.methods().map(move |method| (&model.name, method)))
            .chain(app.library_classes.iter()
                .flat_map(|class| class.methods.iter().map(move |method| (&class.name, method))))
        {
            if method.receiver == crate::dialect::MethodReceiver::Class
                && method.name.as_str() == "new"
            {
                classes.entry(id.clone()).or_default().declares_constructor = true;
            }
        }

        // A class named in a signature means the lexically nearest one
        // (`Capabilities::Charge` inside `ShopifyPayments::Capability`),
        // which is only knowable once every class is registered.
        let resolved: Vec<(ClassId, Symbol, Ty, Ty)> = app
            .rbs_signatures
            .iter()
            .flat_map(|(class_id, methods)| {
                methods.iter().map(|(name, ty)| {
                    let scoped = ty.map_class_ids(&|id| {
                        body::lexical_class(id, class_id.0.as_str(), &classes)
                            .unwrap_or_else(|| id.clone())
                    });
                    (class_id.clone(), name.clone(), ty.clone(), scoped)
                })
            })
            .filter(|(_, _, before, after)| before != after)
            .collect();
        for (class_id, name, before, after) in resolved {
            if let Some(slot) = classes
                .get_mut(&class_id)
                .and_then(|c| c.instance_methods.get_mut(&name))
            {
                if *slot == before {
                    *slot = after;
                }
            }
        }
        // Test helpers are source methods, not a surface invented by test
        // emission. Register their real identities and declarations before
        // source typing; inferred returns converge in the same registry as
        // ordinary application methods.
        test_module::register(&mut classes, app);
        for declaration in &app.library_classes {
            if let Some(info) = classes.get_mut(&declaration.name) { info.is_module = declaration.is_module; }
        }


        assert!(
            !app.source_index_required || !app.sources.is_empty(),
            "source_index_missing: ingested source app cannot use IR-only analysis"
        );

        let const_resolver = app.const_resolver.for_sources(&app.sources);
        let data_factories = data::register(app, &const_resolver, &mut classes);

        Self {
            classes,
            inferred_params: HashMap::new(),
            declared_signatures: app
                .rbs_signatures
                .iter()
                .flat_map(|(c, ms)| ms.keys().map(move |m| (c.clone(), m.clone())))
                .collect(),
            adapter,
            concern_folded: HashMap::new(),
            host_folded: HashMap::new(),
            refined_action_bindings: HashMap::new(),
            inquirers: inquiry::inquirer_methods(app),
            const_resolver,
            source_indexed: !app.sources.is_empty(),
            typed_constants: IdentityHashMap::default(),
            data_factories,
            view_seeds: None,
            callers_by_target: HashMap::new(),
            controller_action_meta_cache: HashMap::new(),
            fixpoint_rounds: FixpointRounds::default(),
            fixpoint_checks: fixpoint_check::Checks::default(),
        }
    }

    /// Build a body-typer borrowing this analyzer's dispatch tables.
    /// Cheap — just a struct with a reference.
    fn body_typer(&self) -> BodyTyper<'_> {
        let typer = BodyTyper::new(&self.classes)
            .with_inquirers(&self.inquirers)
            .with_typed_constants(&self.typed_constants)
            .with_data_factories(&self.data_factories);
        if self.source_indexed { typer.with_const_resolver(self.const_resolver.clone()) } else { typer }
    }

    /// The per-class member registry — schema columns, catalog-sourced
    /// AR surface, associations, scopes, and user-defined methods with
    /// their inferred returns (post-fixpoint when read after
    /// [`Self::analyze`]). This is the same table dispatch resolves
    /// against, exposed read-only so IDE consumers ([`crate::ide`]
    /// completion) can *enumerate* what dispatch can *resolve*.
    pub fn class_registry(&self) -> &HashMap<ClassId, ClassInfo> {
        &self.classes
    }

    /// Parameter types unified from call sites for `class#method`
    /// (post-fixpoint when read after [`Self::analyze`]), positionally
    /// aligned with the method's declared params. `None` when no call
    /// site contributed. Read-only companion to [`Self::class_registry`]
    /// for consumers assembling full candidate signatures (the gap
    /// footers' pre-filled RBS).
    pub fn inferred_param_types(&self, class: &ClassId, method: &Symbol) -> Option<&[Ty]> {
        self.params_row(class, method).map(|v| v.as_slice())
    }

    // Instance side first: a caller asking by name alone reads the `def` an instance runs, as it did before rows were keyed by side.
    fn params_row(&self, class: &ClassId, method: &Symbol) -> Option<&Vec<Ty>> {
        [MethodReceiver::Instance, MethodReceiver::Class]
            .into_iter()
            .find_map(|side| self.inferred_params.get(&(class.clone(), method.clone(), side)))
    }

    /// How the fixpoint loops of the last [`Self::analyze`] ended; all
    /// [`LoopEnd::NotRun`] before it.
    pub fn fixpoint_rounds(&self) -> FixpointRounds {
        self.fixpoint_rounds
    }

    /// Walk the app, annotating every expression's `ty` field, then
    /// populating the owning construct's `effects` by visiting the typed tree.
    ///
    /// Two-phase: an initial typing pass over the whole app, then a
    /// whole-program fixpoint loop that (a) harvests inferred return
    /// types from method bodies into the dispatch registry, (b) unifies
    /// parameter types across call sites, and (c) re-runs typing with
    /// the refined registry. Iterates to a fixed point (capped; see
    /// `FIXPOINT_CAP`) using a structural registry snapshot to detect convergence.
    pub fn analyze(&mut self, app: &mut App) {
        self.fixpoint_checks = fixpoint_check::Checks::start();
        // An unresolvable include is a load-time error, not an open method
        // surface. Keep it in the class-body ledger even when no method is called.
        for class in &mut app.library_classes {
            for included in &class.includes {
                let known = self.classes.contains_key(included)
                    || body::lexical_class(included, class.name.0.as_str(), &self.classes).is_some()
                    || body::RUBY_TOP_LEVEL.contains(&included.0.as_str());
                if known { continue; }
                let detail = format!("{} includes unresolved {}", class.name.0, included.0);
                if class.unknown_calls.iter().any(|call| matches!(&call.diagnostic,
                    Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, detail: old, .. })
                    if construct.as_str() == "include" && old == &detail)) { continue; }
                let mut refusal = crate::expr::Expr::new(crate::span::Span::synthetic(),
                    crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil });
                refusal.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                    target: None, construct: Symbol::from("include"), detail,
                });
                class.unknown_calls.push(refusal);
            }
        }
        for model in &mut app.models {
            for item in &mut model.body {
                let ModelBodyItem::Unknown { expr, .. } = item else { continue };
                let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
                if method.as_str() != "include" { continue; }
                if matches!(&expr.diagnostic, Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. }) if construct.as_str() == "include") {
                    expr.diagnostic = None;
                }
                let missing: Vec<_> = args.iter().filter_map(|arg| {
                    let ExprNode::Const { path } = &*arg.node else { return None };
                    let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                    let id = ClassId(Symbol::from(name.as_str()));
                    let known = self.classes.contains_key(&id)
                        || body::lexical_class(&id, model.name.0.as_str(), &self.classes).is_some()
                        || body::RUBY_TOP_LEVEL.contains(&name.as_str());
                    (!known).then_some(name)
                }).collect();
                if !missing.is_empty() {
                    expr.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None, construct: Symbol::from("include"),
                        detail: format!("{} includes unresolved {}", model.name.0, missing.join(", ")),
                    });
                }
            }
        }
        const FIXPOINT_CAP: usize = 12;
        // View-name and dynamic-render ivar sets are invariant across
        // fixpoint rounds — they read source views, not the registry.
        let mut dynamic_render_ivars: std::collections::HashSet<Symbol> =
            std::collections::HashSet::new();
        for view in &app.views {
            collect_dynamic_render_ivars(&view.body, &mut dynamic_render_ivars);
        }
        let existing_view_names: std::collections::HashSet<Symbol> =
            app.views.iter().map(|v| v.name.clone()).collect();
        // Module method tables and controller parent links are invariant
        // across fixpoint rounds — clone once instead of rebuilding them
        // on every `run_typing_passes` (Campfire: 1 initial + 8 rounds).
        let module_methods: HashMap<ClassId, Vec<MethodDef>> = app
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .map(|lc| (lc.name.clone(), lc.methods.clone()))
            .collect();
        let module_includes: HashMap<ClassId, Vec<ClassId>> = app
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .map(|lc| (lc.name.clone(), lc.includes.clone()))
            .collect();
        let parent_link_by_name: HashMap<ClassId, Option<ClassId>> = app
            .controllers
            .iter()
            .map(|c| (c.name.clone(), c.parent.clone()))
            .collect();

        crate::timings::phase("typing passes (initial)", || {
            self.run_typing_passes(
                app,
                &dynamic_render_ivars,
                &existing_view_names,
                &module_methods,
                &module_includes,
                &parent_link_by_name,
                TypingMode::Production { dirty: None },
            )
        });

        let mut rounds = FixpointRounds {
            production: LoopEnd::RanToCap,
            views_and_tests: LoopEnd::RanToCap,
            absorb: LoopEnd::NotRun,
        };
        // Whole-program fixpoint: harvest returns + unify params, re-type,
        // repeat until the registry signature stabilizes. Each round
        // carries a fact one link further, so the cap bounds the longest
        // CHAIN an app can have typed — not a cost most apps pay, since
        // the signature check ends the loop as soon as nothing moves.
        // It was 4, after Spinel's "1-2 iterations typically; 4 is a
        // safety net" (`~/git/spinel/spinel_codegen.rb:7459-7492`), and
        // lobsters never converged under it: `render json: @stories`
        // sits at the end of `.new` args → `initialize` → `@scope` →
        // `with_pagination_info` → `get` → `paginate` → the
        // `get_from_cache` block → its return → the destructuring, which
        // settles on round 9.
        let mut prev_hints = self.capture_dirty_hints();
        for round in 0..FIXPOINT_CAP {
            crate::timings::phase(format_args!("round {round}: harvest returns"), || {
                self.harvest_returns_to_registry(app, false)
            });
            crate::timings::phase(format_args!("round {round}: unify params"), || {
                self.unify_params_from_call_sites(app, UnifyScope::Production)
            });
            // Returns+params settle independently of block-value
            // verdicts (`DirtyHints`); both must be quiet before the
            // loop stops, or a newly block-valued method leaves its
            // callers on the registered return instead of the block.
            if self.inference_matches(&prev_hints.sig)
                && self.block_value_matches(&prev_hints)
            {
                rounds.production = LoopEnd::Settled(round);
                break;
            }
            // Re-type with the refined registry. Idempotent BodyTyper
            // means a second pass simply resolves dispatches and Var
            // bindings the first pass couldn't — which is also why a
            // class the harvest did not move is a class whose bodies
            // this round would retype to exactly what they already say.
            // Campfire's rounds shrink 191 → 89 → 44 → 29 → 13 → 2
            // classes, ending on the Opengraph/Current/Nokogiri chain,
            // so the later rounds retype the whole app for a handful of
            // returns. `dirty_classes_for_retype` is that handful plus
            // everything that reads it.
            let dirty = self.dirty_classes_for_retype(app, &prev_hints);
            prev_hints = self.capture_dirty_hints();
            crate::timings::phase(format_args!("round {round}: typing passes"), || {
                self.run_typing_passes(
                    app,
                    &dynamic_render_ivars,
                    &existing_view_names,
                    &module_methods,
                    &module_includes,
                    &parent_link_by_name,
                    TypingMode::Production { dirty: dirty.as_ref() },
                )
            });
        }
        let round_inputs = fixpoint_check::RoundInputs {
            dynamic_render_ivars: &dynamic_render_ivars,
            existing_view_names: &existing_view_names,
            module_methods: &module_methods,
            module_includes: &module_includes,
            parent_link_by_name: &parent_link_by_name,
        };
        if fixpoint_check::verify_on() {
            self.verify_round(app, &round_inputs, fixpoint_check::Loop::Production, None);
            prev_hints = self.capture_dirty_hints();
        }

        // Intermediate rounds skip views/tests: production does not
        // read test helper returns, and unifying from still-untyped view
        // trees for seven rounds was wasted work. After production
        // converges, type views once and then harvest/unify/retype
        // *tests* until helper chains settle. Later test rounds reuse
        // the production+view param snapshot instead of re-walking
        // those trees. If view/test sites moved a production param,
        // retype production until harvested returns stabilize. Nested
        // constructors live in serializer bodies (AuthorResource.new
        // inside ArticleResource.to_h), so each absorb pass must
        // unify WithViews again — Production unify would drop the
        // view sites, and skipping unify leaves nested initialize
        // params as Var. After helper returns absorb those params,
        // views are typed once more so template calls see the
        // harvested helper surface rather than leftover untyped.
        let production_sig = prev_hints.sig.clone();
        let mut production_view_params = None;
        for round in 0..FIXPOINT_CAP {
            crate::timings::phase(
                if round == 0 {
                    "typing passes (views after fixpoint)".to_string()
                } else {
                    format!("round {round}: tests")
                },
                || {
                    if round == 0 {
                        // Views only. The loop above broke because its
                        // harvest+unify moved nothing, so the production
                        // bodies are exactly what the last round typed —
                        // retyping them here was a pass spent confirming
                        // a fixpoint the loop had already proved.
                        self.run_typing_passes(
                            app,
                            &dynamic_render_ivars,
                            &existing_view_names,
                            &module_methods,
                            &module_includes,
                            &parent_link_by_name,
                            TypingMode::ViewsAndTests,
                        );
                    } else {
                        self.type_tests_only(app);
                    }
                },
            );
            self.harvest_returns_to_registry(app, true);
            crate::timings::phase(
                if round == 0 {
                    "unify params (after views)".to_string()
                } else {
                    format!("round {round}: unify params (tests)")
                },
                || {
                    if round == 0 {
                        self.unify_params_from_call_sites(app, UnifyScope::WithViews);
                        production_view_params = Some(self.inferred_params.clone());
                        self.overlay_test_params(app);
                    } else if let Some(snapshot) = production_view_params.as_ref() {
                        self.unify_test_params_onto(app, snapshot);
                    }
                },
            );
            if self.inference_matches(&prev_hints.sig)
                && self.block_value_matches(&prev_hints)
            {
                rounds.views_and_tests = LoopEnd::Settled(round);
                break;
            }
            prev_hints = self.capture_dirty_hints();
        }
        if fixpoint_check::verify_on() {
            self.verify_round(
                app,
                &round_inputs,
                fixpoint_check::Loop::ViewsAndTests,
                production_view_params.as_mut(),
            );
        }
        if !self.inference_matches(&production_sig) {
            rounds.absorb = LoopEnd::RanToCap;
            let mut absorb_hints = self.capture_dirty_hints();
            // The view/test rounds above moved signatures that
            // production bodies read, and the last production pass
            // predates every one of them — so the first absorb pass is
            // a full retype. Only the rounds after it can be narrowed
            // to what their predecessor moved.
            let mut absorb_dirty: Option<std::collections::HashSet<ClassId>> = None;
            for round in 0..FIXPOINT_CAP {
                crate::timings::phase(
                    if round == 0 {
                        "typing passes (after view unify)".to_string()
                    } else {
                        format!("round {round}: absorb production")
                    },
                    || {
                        self.run_typing_passes(
                            app,
                            &dynamic_render_ivars,
                            &existing_view_names,
                            &module_methods,
                            &module_includes,
                            &parent_link_by_name,
                            TypingMode::Production { dirty: absorb_dirty.as_ref() },
                        )
                    },
                );
                self.harvest_returns_to_registry(app, true);
                self.unify_params_from_call_sites(app, UnifyScope::WithViews);
                if let Some(snapshot) = production_view_params.as_mut() {
                    snapshot.clone_from(&self.inferred_params);
                }
                self.overlay_test_params(app);
                if self.inference_matches(&absorb_hints.sig)
                    && self.block_value_matches(&absorb_hints)
                {
                    rounds.absorb = LoopEnd::Settled(round);
                    break;
                }
                absorb_dirty = self.dirty_classes_for_retype(app, &absorb_hints);
                absorb_hints = self.capture_dirty_hints();
            }
        } else {
            // View unify did not move production signatures, but helper
            // *bodies* still need a pass against view-inferred params
            // (`highlight_searched_content`'s `content` is only called
            // from templates) before views are restamped.
            crate::timings::phase("typing passes (helpers after view unify)", || {
                self.run_typing_passes(
                    app,
                    &dynamic_render_ivars,
                    &existing_view_names,
                    &module_methods,
                    &module_includes,
                    &parent_link_by_name,
                    TypingMode::Production { dirty: None },
                )
            });
            self.harvest_returns_to_registry(app, true);
            // …and that harvest can still move a helper's return, which
            // production bodies read. The absorb branch above ends on a
            // signature check, so it leaves production converged; this
            // branch does not, so it pays the production pass the views
            // stamp below no longer carries.
            crate::timings::phase("typing passes (production after helper harvest)", || {
                self.run_typing_passes(
                    app,
                    &dynamic_render_ivars,
                    &existing_view_names,
                    &module_methods,
                    &module_includes,
                    &parent_link_by_name,
                    TypingMode::Production { dirty: None },
                )
            });
        }
        if fixpoint_check::verify_on() {
            self.verify_round(
                app,
                &round_inputs,
                fixpoint_check::Loop::Absorb,
                production_view_params.as_mut(),
            );
        }
        self.fixpoint_rounds = rounds;
        // Wave 12 types views once against production-only helper
        // returns, then unifies helper params from those sites. Helper
        // returns therefore settle only after the absorb/harvest above.
        // Stamp view trees again so template Sends are not leftover
        // `gradual_untyped` against the pre-unify helper registry
        // (Writebook `leafables/show` `highlight_searched_content`).
        crate::timings::phase("typing passes (views after helper harvest)", || {
            self.run_typing_passes(
                app,
                &dynamic_render_ivars,
                &existing_view_names,
                &module_methods,
                &module_includes,
                &parent_link_by_name,
                TypingMode::ViewsAndTests,
            )
        });
        // Effects are a function of the converged typed trees, not of
        // the fixpoint. Collecting inside every typing round walked
        // the same bodies two or three times per round for no harvest
        // or unify input.
        self.stamp_body_effects(app);

        // Publish the converged param table onto the App for lowerings
        // that build signatures after analysis (the controller lowering
        // reads it for private-helper params). Published AFTER the
        // fixpoint on purpose: mid-loop copies would carry a round's
        // under-informed answers.
        app.inferred_method_params = self
            .inferred_params
            .keys()
            .filter_map(|(c, m, _)| Some(((c.clone(), m.clone()), self.params_row(c, m)?.clone())))
            .collect();
        app.inferred_method_returns = app
            .controllers
            .iter()
            .flat_map(|controller| {
                let table = self.classes.get(&controller.name).map(|ci| &ci.instance_methods);
                controller.actions().filter_map(move |action| {
                    let ty = table?.get(&action.name)?;
                    let ty = match ty {
                        Ty::Fn { ret, .. } => (**ret).clone(),
                        other => other.clone(),
                    };
                    (!ty.mentions_unknown() && !matches!(ty, Ty::Bottom)).then(|| ((controller.name.clone(), action.name.clone()), ty))
                })
            })
            .collect();

        // Render sites inside `app/helpers` modules seed partial locals
        // too — lobsters' ApplicationHelper#link_post renders
        // `helpers/_link_post` with `link:` a URL String, and nothing
        // else renders that partial. Harvested only now, off converged
        // bodies, and only where no view site already said something.
        let helper_modules: std::collections::HashSet<ClassId> =
            app.helper_method_index.values().cloned().collect();
        let mut helper_sites: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
        for lc in app.library_classes.iter().filter(|lc| helper_modules.contains(&lc.name)) {
            for m in &lc.methods {
                let mut targets = Vec::new();
                render::extract_partial_render_sites(
                    &m.body,
                    &Symbol::from("application/_helper"),
                    &mut helper_sites,
                    &mut targets,
                );
            }
        }
        for (partial, locals) in helper_sites {
            let entry = app.partial_local_types.entry(partial).or_default();
            for (k, ty) in locals {
                if !ty.is_unknown() {
                    entry.entry(k).or_insert(ty);
                }
            }
        }

        if let Ok(name) = std::env::var("RH_DEBUG_CLASS") {
            let id = ClassId(Symbol::from(name.as_str()));
            if let Some(ci) = self.classes.get(&id) {
                eprintln!("DBG class {name} instance={:?}", ci.instance_methods);
                eprintln!("DBG class {name} class={:?}", ci.class_methods);
                for ((c, m, side), v) in &self.inferred_params {
                    if c == &id {
                        let sep = if *side == MethodReceiver::Class { "." } else { "#" };
                        eprintln!("DBG params {}{sep}{} = {:?}", c.0.as_str(), m.as_str(), v);
                    }
                }
            } else {
                eprintln!("DBG class {name} NOT REGISTERED");
            }
        }
        // Direct-helper bodies last: they are the one app-authored
        // expression the fixpoint above never touches, and typing them
        // needs the registry it produces.
        self.type_direct_helper_bodies(app);
        self.type_rails_application_body(app);

        self.stamp_inferred_method_signatures(app);
        self.report_fixpoint_checks(app);
    }

    /// Type the bodies of `direct :name do |…| … end` helpers, with the
    /// block parameters seeded from the helper's own CALL SITES.
    ///
    /// These bodies are app source — campfire's is `route_for
    /// :user_avatar, user.avatar_token, v: user.updated_at.to_fs(:number)`
    /// — but they live in `config/routes.rb`, so nothing in the fixpoint
    /// above reaches them and every node came out with `ty: None`. The
    /// generated helper then read `(untyped user, ?untyped options) ->
    /// untyped`, and, more expensively, `routes_to_library::
    /// string_segment_demand` had no type to read for the `route_for`
    /// argument: `user_avatar_path`'s `user_id` segment kept its
    /// name-based Integer default while the only thing that ever fills
    /// it is a signed token, and spinel refused the build.
    ///
    /// The seed is the call site because that is the only evidence there
    /// is — a `direct` block declares no types, and its parameter is
    /// whatever the views hand it (`fresh_user_avatar_path(@user)`,
    /// `(Current.user)`, `(member)`: all `User`). Same rule, and the
    /// same reason, as the `string_segment_demand` this feeds.
    ///
    /// Runs AFTER the fixpoint: the call sites are in views and helper
    /// bodies, and their types are exactly what the fixpoint spent its
    /// rounds establishing.
    /// Type `config/application.rb`'s methods.
    ///
    /// `App::rails_application` is a `LibraryClass` that EMITS — the
    /// app's own `Rails::Application` subclass, reparented at ingest —
    /// but it is not in `app.library_classes`, so the fixpoint above
    /// walks every other app-authored body and not this one. Its
    /// receivers stayed `Var`, and a pass that grounds by receiver type
    /// had nothing to ground: campfire's
    /// `ENV["APP_VERSION"].presence || ENV["GIT_REVISION"].presence ||
    /// "0"` reached spinel with the dynamic `presence` intact and
    /// compiled to `undefined method 'presence' for an instance of
    /// String`. Spinel KNEW the receiver was a String; we were the ones
    /// who had not looked.
    ///
    /// Solo, after the fixpoint, for the same reason
    /// `type_direct_helper_bodies` is: nothing calls these methods from
    /// inside the app (they are reached through the `Rails.application`
    /// shim), so there are no call sites to unify and no return the
    /// registry is waiting on — they need the registry, not the other
    /// way round.
    fn type_rails_application_body(&mut self, app: &mut App) {
        let Some(lc) = &mut app.rails_application else { return };
        let ctx = Ctx {
            self_ty: Some(Ty::Class { id: lc.name.clone(), args: vec![] }),
            ..Ctx::default()
        };
        let typer = self.body_typer();
        for method in &mut lc.methods {
            typer.analyze_expr(&mut method.body, &ctx);
        }
    }

    fn type_direct_helper_bodies(&mut self, app: &mut App) {
        if app.routes.direct_helpers.is_empty() {
            return;
        }
        // stem -> positional argument types, by index, unioned across
        // call sites. `_path` and `_url` are the same helper.
        let stems: Vec<Symbol> =
            app.routes.direct_helpers.iter().map(|d| d.name.clone()).collect();
        let mut seeds: HashMap<Symbol, Vec<Ty>> = HashMap::new();
        {
            let mut collect = |e: &crate::expr::Expr| {
                fn walk(
                    e: &crate::expr::Expr,
                    stems: &[Symbol],
                    out: &mut HashMap<Symbol, Vec<Ty>>,
                ) {
                    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
                        let m = method.as_str();
                        let stem = m
                            .strip_suffix("_path")
                            .or_else(|| m.strip_suffix("_url"))
                            .unwrap_or("");
                        if let Some(s) = stems.iter().find(|s| s.as_str() == stem) {
                            let slot = out.entry(s.clone()).or_default();
                            // Positionals only — a trailing kwargs hash
                            // is the options half the block takes last.
                            for (i, a) in args
                                .iter()
                                .filter(|a| {
                                    !matches!(&*a.node, ExprNode::Hash { kwargs: true, .. })
                                })
                                .enumerate()
                            {
                                let Some(t) = a.ty.clone() else { continue };
                                // An UNTYPED call site is absence of
                                // evidence, not evidence of untyped.
                                // Unioning it in would poison the seed:
                                // an `Untyped` arm ABSORBS dispatch, so
                                // `User | Untyped | Nil` answers
                                // `Untyped` for every method — which is
                                // exactly what campfire produced.
                                // MEASURED there: of eight call sites,
                                // five carry a clean `User`/`User | Nil`
                                // and two carry `Untyped`
                                // (`_direct.html.erb`'s `members =
                                // ….presence || [ … ]`), and unfiltered
                                // those two decided the whole seed.
                                // Same predicate the refined action
                                // bindings use.
                                if t.is_open() || !is_clean_binding(&t) {
                                    continue;
                                }
                                if slot.len() <= i {
                                    slot.resize(i + 1, Ty::Bottom);
                                }
                                slot[i] = crate::analyze::body::union_of(slot[i].clone(), t);
                            }
                        }
                    }
                    e.node.for_each_child(&mut |c| walk(c, stems, out));
                }
                walk(e, &stems, &mut seeds);
            };
            crate::lower::for_each_hook_body_ref(app, &mut collect);
            for view in &app.views {
                collect(&view.body);
            }
        }

        let mut helpers = std::mem::take(&mut app.routes.direct_helpers);
        for helper in &mut helpers {
            let mut local_bindings: HashMap<Symbol, Ty> = HashMap::new();
            // The LAST parameter is Rails' always-supplied options hash,
            // never one of the helper's own arguments — see
            // `dialect::DirectHelper`. Bind it as the hash it is so a
            // body that reads it dispatches, and map the rest
            // positionally onto what the call sites pass.
            let arity = helper.params.len();
            for (i, p) in helper.params.iter().enumerate() {
                if i + 1 == arity {
                    local_bindings.insert(
                        p.clone(),
                        Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) },
                    );
                    continue;
                }
                if let Some(t) = seeds.get(&helper.name).and_then(|v| v.get(i)) {
                    if !matches!(t, Ty::Bottom) {
                        local_bindings.insert(p.clone(), t.clone());
                    }
                }
            }
            let ctx = Ctx {
                self_ty: None,
                ivar_bindings: HashMap::new(),
                local_bindings,
                class_objects: Default::default(),
                constants: Default::default(),
                annotate_self_dispatch: false,
                in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };
            self.body_typer().analyze_expr(&mut helper.body, &ctx);
        }
        app.routes.direct_helpers = helpers;
    }

    /// Post-fixpoint: write what inference discovered into the
    /// `MethodDef.signature` of library methods and source test helpers, so the
    /// emitted RBS carries it (`errors_for: (Comment | Story | …) ->
    /// String?` instead of `(untyped) -> untyped`) and AOT targets can
    /// dispatch inside the body. Self-describing IR: the body typer
    /// already consumed these seeds during the fixpoint; this records
    /// the same fact where emitters read it.
    ///
    /// **This used to be scoped to helper modules**, on the reasoning
    /// that the view→helper channel is where params have no other
    /// typing source. That left every other plain class emitting a
    /// fully-`untyped` sidecar even though the analyzer had the shapes
    /// all along — lobsters' `CandidateId#to_s` registered as `String`
    /// in `self.classes` and shipped as `() -> untyped`, and the
    /// resulting poly `.to_s` widened a `Hash[String, String]` far
    /// enough downstream to produce two C errors. The registry is the
    /// same answer call sites resolve against, so sourcing the return
    /// from it keeps the sidecar and dispatch from disagreeing.
    ///
    /// Three things it will not do:
    ///
    /// * **Clobber an existing signature** (RBS-derived, façade, or
    ///   author-written). Hand-written wins, as everywhere else.
    /// * **Stamp `initialize`.** Its body type is whatever the last
    ///   assignment happened to be; `new` answers the class. Leaving it
    ///   unstamped renders the untyped fallback, which is true.
    /// * **Stamp when it learned nothing.** All-`Var` params and a
    ///   `Var`/`Untyped`/absent return render exactly what the untyped
    ///   fallback renders, so skipping keeps the output byte-identical
    ///   rather than routing it through a second code path.
    fn stamp_inferred_method_signatures(&self, app: &mut App) {
        for (owner, methods) in app.library_classes.iter_mut().map(|lc| (&lc.name, &mut lc.methods))
            .chain(app.test_modules.iter_mut().map(|module| (&module.name, &mut module.helpers))) {
            let registered = self.classes.get(owner);
            for method in methods {
                if method.signature.is_some() {
                    continue;
                }
                if method.name.as_str() == "initialize" {
                    continue;
                }
                let key = (owner.clone(), method.name.clone(), method.receiver);
                let inferred = self.inferred_params.get(&key);
                let has_params = inferred
                    .is_some_and(|v| !v.iter().all(|t| matches!(t, Ty::Var { .. })));

                // The registry first: `harvest_returns_to_registry`
                // already walked this class and wrote the answer every
                // call site resolves against. `effective_return_ty` is
                // the fallback for a method the harvest skipped (a
                // scope-shaped class method takes an early `continue`
                // there).
                let table = registered.map(|ci| match method.receiver {
                    crate::dialect::MethodReceiver::Instance => &ci.instance_methods,
                    crate::dialect::MethodReceiver::Class => &ci.class_methods,
                });
                let ret = table
                    .and_then(|t| t.get(&method.name))
                    .filter(|t| !matches!(t, Ty::Fn { .. }))
                    .cloned()
                    .or_else(|| effective_return_ty(&method.body))
                    .filter(|t| !matches!(t, Ty::Var { .. } | Ty::Untyped));

                if !has_params && ret.is_none() {
                    continue;
                }

                // Class-side `new` answers `Ty::SelfInstance` so inherited
                // factories stay receiver-dependent in the registry. The
                // MethodDef signature is what RBS emit reads, and emit
                // refuses a bare SelfInstance — pin it to the owner the
                // method is stamped on (same concrete shape
                // `concern_class_methods` requires).
                let owner_ty = Ty::Class { id: owner.clone(), args: Vec::new() };

                let params: Vec<crate::ty::Param> = method
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        // A rest slot stays untyped. `collect_send_sites`
                        // records argument types BY POSITION, so the
                        // slot under a `*streams` sees only whatever
                        // landed in that one position — the second and
                        // later varargs of the same call are recorded
                        // against positions that have no param at all.
                        // `Utils.silence_stream(STDOUT, STDERR)` unified
                        // to `*STDERR streams`, a declaration about
                        // every vararg made from one of them.
                        let ty = if p.rest {
                            Ty::Untyped
                        } else {
                            param_ty_with_default(inferred.and_then(|v| v.get(i)).cloned(), p)
                                .unwrap_or(Ty::Untyped)
                                .subst_self(&owner_ty)
                        };
                        // Kind must survive verbatim: the untyped
                        // fallback this replaces is kind-aware, and a
                        // `*streams` rendered positionally makes the
                        // sig disagree with the def. `Param::ty_kind` is
                        // the one copy of that rule.
                        crate::ty::Param { name: p.name.clone(), ty, kind: p.ty_kind() }
                    })
                    .collect();
                method.signature = Some(Ty::Fn {
                    params,
                    block: None,
                    ret: Box::new(ret.unwrap_or(Ty::Untyped).subst_self(&owner_ty)),
                    effects: method.effects.clone(),
                });
            }
        }
    }

    /// Type each app constant value. Rubydex already owns the constant
    /// names and their lexical resolution, so source reads use its
    /// `DeclarationId` to find these inferred types. The bare-name map
    /// remains only for generated expressions without Ruby source.
    /// The rounds continue until no value changes, so `B = A` can use
    /// the value of `A` from the round before.
    fn build_constant_registry(
        &self,
        app: &App,
    ) -> (HashMap<Symbol, Ty>, IdentityHashMap<DeclarationId, Ty>) {
        // Ingest can move a constant to another IR owner: a file-level
        // constant goes to the first class of its file. The declaration
        // ID comes from the source position, not from the owner.
        let declaration_id = |name: &Symbol, value: &Expr| {
            self.const_resolver.constant_declaration(value.span, name.as_str())
        };
        // (defining class, last-segment name, Rubydex ID, value,
        // eligible for the production generated-expression fallback).
        let mut entries: Vec<(Ty, Symbol, Option<DeclarationId>, Expr, bool)> = Vec::new();
        let mut push_const = |self_ty: Ty, expr: &Expr| {
            if let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node {
                if let Some(last) = path.last() {
                    let id = declaration_id(last, value);
                    entries.push((self_ty, last.clone(), id, value.clone(), true));
                }
            }
        };
        for model in &app.models {
            for item in &model.body {
                if let ModelBodyItem::Unknown { expr, .. } = item {
                    push_const(Ty::Class { id: model.name.clone(), args: vec![] }, expr);
                }
            }
        }
        for controller in &app.controllers {
            for item in &controller.body {
                if let ControllerBodyItem::Unknown { expr, .. } = item {
                    push_const(Ty::Class { id: controller.name.clone(), args: vec![] }, expr);
                }
            }
        }
        // Library classes carry theirs on a FIELD rather than as an
        // `Unknown` body item, so they need their own arm — and without it
        // every constant a CONCERN or a HELPER declares was invisible here.
        // That is not a rare corner: campfire keeps `CONNECTION_TTL` in
        // `Membership::Connectable`, `PAGE_SIZE` in `Message::Pagination`,
        // `REACTIONS` in `EmojiHelper` and `VERSIONS` in `AllowBrowser`,
        // and every read of the four fell to the `Ty::Class { id:
        // ConstName }` fallback — so `CONNECTION_TTL.ago` asked for `ago`
        // on a class named CONNECTION_TTL, `count > PAGE_SIZE` compared an
        // Int to one, and the two `each`es iterated one. Five of the
        // emit's type errors, one missing loop.
        for lc in &app.library_classes {
            for (name, value) in &lc.constants {
                let self_ty = Ty::Class { id: lc.name.clone(), args: vec![] };
                entries.push((self_ty, name.clone(), declaration_id(name, value), value.clone(), true));
            }
        }
        // Original source tests use the same DeclarationId contract.
        // Their constants participate in the value fixpoint, but must
        // not change the bare-name fallback used by production views.
        for module in &app.test_modules {
            for (owner, constants) in std::iter::once((&module.name, &module.constants))
                .chain(module.inner_classes.iter().map(|inner| (&inner.name, &inner.constants)))
            {
                for (name, value) in constants {
                    let self_ty = Ty::Class { id: owner.clone(), args: vec![] };
                    entries.push((self_ty, name.clone(), declaration_id(name, value), value.clone(), false));
                }
            }
        }

        let mut map: HashMap<Symbol, Ty> = HashMap::new();
        let mut ambiguous: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        let mut resolved: IdentityHashMap<DeclarationId, Ty> = IdentityHashMap::default();
        // Each round can type one more link of a `B = A` chain, so n
        // values need at most n + 1 rounds when no value changes after
        // it is typed. A cycle (`A = B`, `B = A`) never types and stays
        // unknown, and the read reports it. The bound also stops values
        // that change on each round.
        for _ in 0..=entries.len() {
            let mut next: HashMap<Symbol, Ty> = HashMap::new();
            let mut next_resolved: IdentityHashMap<DeclarationId, Ty> = IdentityHashMap::default();
            let shared = body::ConstScope::global(map.clone());
            let typer = BodyTyper::new(&self.classes)
                .with_inquirers(&self.inquirers)
                .with_typed_constants(&resolved)
                .with_data_factories(&self.data_factories);
            let typer = if self.source_indexed { typer.with_const_resolver(self.const_resolver.clone()) } else { typer };
            for (self_ty, name, id, value, production) in entries.iter_mut() {
                let ctx = Ctx {
                    self_ty: Some(self_ty.clone()),
                    ivar_bindings: HashMap::new(),
                    local_bindings: HashMap::new(),
                    class_objects: Default::default(),
                    constants: shared.clone(),
                    annotate_self_dispatch: false,
                    in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
                };
                let ty = typer.analyze_expr(value, &ctx);
                if matches!(ty, Ty::Var { .. }) {
                    continue;
                }
                if let Some(id) = id {
                    next_resolved.insert(*id, ty.clone());
                }
                if !*production || ambiguous.contains(name) {
                    continue;
                }
                match next.get(name) {
                    Some(prev) if *prev != ty => {
                        ambiguous.insert(name.clone());
                    }
                    _ => {
                        next.insert(name.clone(), ty);
                    }
                }
            }
            for name in &ambiguous {
                next.remove(name);
            }
            if next == map && next_resolved == resolved {
                break;
            }
            map = next;
            resolved = next_resolved;
        }
        // These source-declared constants have modeled factory-generated
        // object methods even though their initializer is not a retained class.
        for constant in app.generated_helper_methods.keys() {
            resolved.insert(
                rubydex::model::ids::declaration_id_from_lookup_name(constant.0.as_str()),
                Ty::Class { id: constant.clone(), args: vec![] },
            );
        }
        (map, resolved)
    }

    /// One full typing pass over the whole app. Extracted from
    /// `analyze` so the fixpoint loop above can re-invoke it after
    /// each registry refinement. The Rails-aware orchestration
    /// (controller→view ivar channel, before_action seeding,
    /// per-model two-pass ivar discovery, partial locals threading)
    /// stays internal to this method; the fixpoint just calls it.
    fn run_typing_passes(
        &mut self,
        app: &mut App,
        dynamic_render_ivars: &std::collections::HashSet<Symbol>,
        existing_view_names: &std::collections::HashSet<Symbol>,
        module_methods: &HashMap<ClassId, Vec<MethodDef>>,
        module_includes: &HashMap<ClassId, Vec<ClassId>>,
        parent_link_by_name: &HashMap<ClassId, Option<ClassId>>,
        mode: TypingMode<'_>,
    ) {
        // Source-backed constant reads use Rubydex declaration IDs.
        // A bare-name fallback remains for generated expressions without
        // a Ruby source reference. Built in either mode: a views pass
        // stamps templates against the same constant scope the
        // production bodies were typed with.
        let (fallback, resolved_values) = crate::timings::phase("typing: constants", || {
            self.build_constant_registry(app)
        });
        self.typed_constants = resolved_values;
        let global_constants = body::ConstScope::global(fallback);
        match mode {
            TypingMode::Production { dirty } => {
                self.type_production_bodies(
                    app,
                    dynamic_render_ivars,
                    existing_view_names,
                    module_methods,
                    module_includes,
                    parent_link_by_name,
                    &global_constants,
                    dirty,
                );
            }
            TypingMode::ViewsAndTests => {
                debug_assert!(self.view_seeds.is_some());
                self.type_views_and_tests(app, &global_constants);
            }
        }
    }

    /// The production half: models, controllers and library classes,
    /// plus the controller/mailer→view channel their bodies feed.
    /// Handed to the views half through `Analyzer::view_seeds`.
    ///
    /// `dirty`, when present, names the classes whose bodies this pass
    /// retypes — the ones whose inference moved last round and the ones
    /// that read them, per `dirty_classes_for_retype`. Only the
    /// `analyze_expr` calls are gated: every harvest below still walks
    /// the whole app, so a class the fixpoint has finished with
    /// contributes the types the last pass wrote rather than nothing.
    #[allow(clippy::too_many_arguments)]
    fn type_production_bodies(
        &mut self,
        app: &mut App,
        dynamic_render_ivars: &std::collections::HashSet<Symbol>,
        existing_view_names: &std::collections::HashSet<Symbol>,
        module_methods: &HashMap<ClassId, Vec<MethodDef>>,
        module_includes: &HashMap<ClassId, Vec<ClassId>>,
        parent_link_by_name: &HashMap<ClassId, Option<ClassId>>,
        global_constants: &body::ConstScope,
        dirty: Option<&std::collections::HashSet<ClassId>>,
    ) {
        // Type the actual initializer trees, not only registry clones.
        // Their lexical class references become load-time dependencies
        // when a source file's nested declarations emit into separate files.
        for class in &mut app.library_classes {
            if dirty.is_some_and(|d| !d.contains(&class.name)) {
                continue;
            }
            let ctx = Ctx {
                self_ty: Some(Ty::Class { id: class.name.clone(), args: vec![] }),
                constants: global_constants.clone(),
                class_side: true,
                ..Ctx::default()
            };
            for (_, value) in &mut class.constants {
                self.body_typer().analyze_expr(value, &ctx);
            }
        }
        // Controller→view ivar channel: as each action is analyzed, we harvest
        // the ivars it sets and key them by the view that action renders.
        // When we reach the view pass below, the view's Ctx is seeded from
        // this map so `@article.title` in `articles/show.html.erb` types
        // against the `@article` bound in `ArticlesController#show`.
        let mut action_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
        let models_by_ivar =
            ivar_set::models_by_conventional_ivar(app.models.iter().map(|m| &m.name));
        // Sibling record of the same channel, persisted onto
        // `App::view_feeders`: which controllers feed each view. Filled
        // wherever ivars flow view-ward (action targets below, effective
        // layouts, then closed over renderer→partial edges) so a view-side
        // diagnostic can be traced to the controller that seeded — or
        // failed to seed — its context.
        let mut view_feeders: HashMap<Symbol, BTreeSet<ClassId>> = HashMap::new();
        // Persisted onto `App::controller_resolutions`: the chained
        // filter list (with provenance) + effective layout that Phase B
        // resolves per controller — the same data the ivar seeding
        // consumes, kept instead of discarded so trace/attribution
        // consumers don't re-derive the ancestor walk.
        let mut controller_resolutions: HashMap<ClassId, crate::app::ControllerResolution> =
            HashMap::new();

        // Content-partial channel: the `render partial: @above` idiom.
        // `dynamic_render_ivars` is the set of ivars any view renders
        // dynamically (`@above`); `content_partial_ivars` keys a
        // partial view name (`home/_for_domain`) to the union of ivars
        // from every action that names it (`@above = 'for_domain'`).
        // Built during Pass B, consumed when seeding partials below.
        // The two sets are computed once in `analyze` and reused every
        // round — they do not depend on the refined registry.
        let mut content_partial_ivars: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        // Per-controller metadata captured during Pass A so Pass B
        // (below) can resolve parent-class filters + action bindings
        // without an inner re-borrow of `app.controllers`. Restructured
        // from a single loop to a two-phase loop because Rails'
        // `before_action :authenticate_user` on ApplicationController
        // applies to every subclass controller's actions — and the
        // target method (`authenticate_user`) is defined on the
        // parent, so resolving the seeded ivars (`@user = ...`)
        // requires looking up the parent's typed action bodies. The
        // first loop types each controller in isolation (no parent
        // inheritance), then the second loop walks the parent chain
        // using the captured metadata.
        struct ControllerMeta {
            self_ty: Ty,
            /// This controller's own segment of the filter chain in
            /// registration order, each entry tagged with the class or
            /// concern module that declared it. All kinds — the seeding
            /// paths read only Before/Around; the persisted
            /// `App::controller_resolutions` chain keeps After/Skip.
            sourced_filters: Vec<(Filter, ClassId)>,
            action_bindings: HashMap<Symbol, HashMap<Symbol, Ty>>,
            /// Typed body per method name — own actions/private helpers,
            /// block-filter bodies, and directly mixed-in concern
            /// methods. The body-carrying twin of `action_bindings`
            /// (same keys, unconditionally populated even when a
            /// method's OWN direct writes are empty, since a method
            /// like `authorize` — no direct `@x = ...` of its own —
            /// still needs its body on hand as a resolution target for
            /// [`collect_transitive_filter_ivars`] to walk *into*).
            /// Consumed only by Phase B's `chained_bodies` table.
            action_bodies: HashMap<Symbol, Expr>,
            /// Per-method effect sets (own actions + concern methods
            /// typed against this controller's self), for the persisted
            /// chain's per-hop effects.
            action_effects: HashMap<Symbol, EffectSet>,
            class_constants: body::ConstScope,
            layout: LayoutDecl,
        }
        let mut meta_by_name: HashMap<ClassId, ControllerMeta> = HashMap::new();
        // Parent links and concern-module tables are cloned once in
        // `analyze` and reused every round — they do not depend on the
        // refined registry.

        self.analyze_class_configuration(&mut app.controllers);

        // Models do not read `controller_ivar_env`. Type and harvest
        // each one before controllers so a round carries model returns
        // into controller bodies. Dispatch reads the registry.
        let _typing_models = crate::timings::begin("typing: models");
        for model in &mut app.models {
            // A model the round did not move feeds nothing but the
            // registry, and the harvest at the top of the round already
            // wrote what its (unchanged) trees say. Nothing below is a
            // seed anyone else reads, so the whole body is skippable.
            if dirty.is_some_and(|d| !d.contains(&model.name)) {
                continue;
            }
            // Seed class ivars for the body-typer. Three shapes in play:
            // 1. `@attributes` — the legacy Hash-storage access path
            //    (some transpiled patterns still use it).
            // 2. Per-schema-column ivars (`@title`, `@body`, ...) — the
            //    typed-field representation. `attr_accessor :title, ...`
            //    in a transpiled model generates accessors that read/
            //    write these ivars, but the generated methods aren't
            //    `def` nodes so flow-sensitive typing can't discover
            //    them — seed directly from schema metadata.
            // 3. Memoization ivars (`@_comments`) — discovered by the
            //    flow-sensitive pre-pass below.
            let mut class_ivars: HashMap<Symbol, Ty> = HashMap::new();
            class_ivars.insert(
                Symbol::from("attributes"),
                Ty::Hash {
                    key: Box::new(Ty::Sym),
                    value: Box::new(Ty::Var { var: crate::ident::TyVar(0) }),
                },
            );
            for (name, ty) in &model.attributes.fields {
                // Ivar reads may observe nil before the first write;
                // union with Nil reflects that. The column's declared
                // type from schema covers the post-initialization case.
                // union_of so an already-nilable column type dedups
                // instead of nesting.
                class_ivars.insert(
                    name.clone(),
                    crate::analyze::body::union_of(ty.clone(), Ty::Nil),
                );
            }
            // `attr_accessor :edit_user_id` virtual attributes: real
            // ivars, untyped, absent from the schema. Seed as gradual
            // so a direct `@edit_user_id` read resolves (don't clobber
            // a schema column of the same name).
            for name in collect_attr_accessor_names(&model.body) {
                class_ivars.entry(name).or_insert(Ty::Untyped);
            }

            // Phase 0: type the model's `Unknown` body items so the
            // RHS of in-class constant assignments (`FLAGGABLE_DAYS = 7`,
            // `MIN_KARMA_TO_SUGGEST = 50`, `COMMENT_REASONS = {...}`)
            // gets `value.ty` populated. Without this, the subsequent
            // const-table extraction sees `None` and the body-typer
            // falls through to `Ty::Class { id: ConstName }` for every
            // read — observable as `incompatible_binop` errors
            // (`Int > Class { MIN_KARMA }`) and `send_dispatch_failed`
            // (`days` on `Class { NEW_USER_DAYS }`).
            let const_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: class_ivars.clone(),
                local_bindings: HashMap::new(),
                class_objects: Default::default(),
                constants: global_constants.clone(),
                annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };
            for item in model.body.iter_mut() {
                if let ModelBodyItem::Unknown { expr, .. } = item {
                    self.body_typer().analyze_expr(expr, &const_ctx);
                }
            }
            // Own constants layered over the global registry (own shadows).
            let class_constants = global_constants.with_own(extract_const_assignments(&model.body));

            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: class_ivars.clone(),
                local_bindings: HashMap::new(),
                class_objects: Default::default(),
                constants: class_constants.clone(),
                annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };

            // Pass A: type every method body with only `@attributes`
            // seeded. Assignments inside bodies (e.g. `@_comments = ...`
            // in a memoizing getter) populate `value.ty` on those
            // assignments, which Pass B harvests.
            for scope in model.scopes_mut() {
                self.body_typer().analyze_expr(&mut scope.body, &class_ctx);
            }
            let model_name = model.name.clone();
            for method in model.methods_mut() {
                // A default is part of the parameter's type. Typed before
                // seeding so `value = nil` is `Nil` when no call site has
                // said otherwise, and a later site can union with it.
                for param in &mut method.params {
                    if let Some(default) = &mut param.default {
                        self.body_typer().analyze_expr(default, &class_ctx);
                    }
                }
                let mctx = self.seed_method_params(&class_ctx, &model_name, method, true);
                self.body_typer().analyze_expr(&mut method.body, &mctx);
            }

            // Pass B: gather every ivar assignment across the model's
            // methods. Each discovered `@x = value` seeds the ivar's
            // type for the second typing pass, so reads that occur
            // *before* the assignment lexically (e.g. the left side of
            // `@x ||= ...` lowered to `@x || (@x = ...)`) still resolve
            // cleanly.
            let mut flow_ivars: HashMap<Symbol, Ty> = HashMap::new();
            for method in model.methods() {
                extract_ivar_assignments(&method.body, &mut flow_ivars);
            }
            for scope in model.scopes() {
                extract_ivar_assignments(&scope.body, &mut flow_ivars);
            }

            if !flow_ivars.is_empty() {
                // Re-seed ctx with discovered ivars alongside @attributes.
                // Memoizing ivars become `Union<T, Nil>` to reflect that
                // the read can be nil before the first assignment.
                let mut reseeded = class_ivars;
                let initialized = ivars_initialized_by(model.methods());
                for (name, ty) in flow_ivars {
                    // `initialize` sets its own ivars before any other method
                    // runs (see the library-class pass).
                    let union_ty = if initialized.contains(&name) && !ty.is_open() {
                        ty
                    } else {
                        crate::analyze::body::union_of(ty, Ty::Nil)
                    };
                    reseeded.insert(name, union_ty);
                }
                let reseeded_ctx = Ctx {
                    self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                    ivar_bindings: reseeded,
                    local_bindings: HashMap::new(),
                    class_objects: Default::default(),
                    constants: class_constants.clone(),
                    annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
                };

                for scope in model.scopes_mut() {
                    self.body_typer().analyze_expr(&mut scope.body, &reseeded_ctx);
                }
                for method in model.methods_mut() {
                    let mctx = self.seed_method_params(&reseeded_ctx, &model_name, method, true);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
            }
            self.harvest_one_model(model);
        }
        drop(_typing_models);

        // ── Phase A: type Unknown body items + every action body
        // ── once per controller, with no parent inheritance.
        let _typing_controllers_a = crate::timings::begin("typing: controllers A");
        for controller in &mut app.controllers {
            // A controller the round did not move still has to produce
            // its metadata: Phase B resolves a dirty child's filters by
            // walking its ancestors' entries, and the view channel is
            // a union over every controller that feeds a template. So
            // only the `analyze_expr` calls are gated; the harvest below
            // reads the trees as the last pass that did walk them left
            // them, which is the same answer a retype would produce.
            let retype = dirty.is_none_or(|d| d.contains(&controller.name));
            // Phase 0: type the controller's `Unknown` body items so
            // in-class constants (`COMMENTS_PER_PAGE = 20`,
            // `TOTP_SESSION_TIMEOUT = (60 * 15)`, etc.) get
            // `value.ty` populated for the extract pass below. Same
            // rationale as the model loop.
            // Self is the controller's own class (registered in the class
            // registry with its parent link) so a bare sibling call like
            // `find_story` dispatches against this controller's methods and
            // walks the parent chain to the ApplicationController surface
            // (params/session/render). Previously self_ty was the *parent*,
            // which hid same-controller helpers from dispatch.
            let self_ty = Ty::Class {
                id: controller.name.clone(),
                args: vec![],
            };
            let const_ctx = Ctx {
                self_ty: Some(self_ty.clone()),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                class_objects: Default::default(),
                constants: global_constants.clone(),
                annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };
            if retype {
                for item in controller.body.iter_mut() {
                    if let ControllerBodyItem::Unknown { expr, .. } = item {
                        self.body_typer().analyze_expr(expr, &const_ctx);
                    }
                }
            }
            // Own constants layered over the global registry — a same-named
            // constant declared on this controller shadows another class's.
            let class_constants =
                global_constants.with_own(extract_controller_const_assignments(&controller.body));

            let ctx = Ctx {
                self_ty: Some(self_ty.clone()),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                class_objects: Default::default(),
                constants: class_constants.clone(),
                annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };

            // Snapshot this controller's own segment of the filter chain
            // (not yet including parent's — that's Phase B), provenance-
            // tagged and concern-spliced in class-body order. All kinds
            // ride along for the persisted chain; the seeding paths below
            // read only Before/Around — `before_action` runs before the
            // action and `around_action` assigns its ivars before `yield`
            // (the canonical `@story = Story.find(..); yield` shape), so
            // both contribute ivars the action and its view see, while
            // `after_action` runs after rendering. Block-form filters'
            // bodies were already typed by the Phase 0 pass above.
            let (sourced_filters, block_filter_bindings) = build_sourced_filter_chain(
                controller,
                app.concern_spliced_actions.get(&controller.name),
            );

            // Pass A: analyze every action body once. Helper-method
            // params (`period(query)`) are seeded from the inferred-
            // params table so their bodies — and thus their harvested
            // return types — resolve; routed actions have empty param
            // rows and seed nothing.
            let ctrl_id = controller.name.clone();
            let spliced_from = app.concern_spliced_actions.get(&ctrl_id).cloned();
            if retype {
                for action in controller.actions_mut() {
                    // A concern method spliced into this controller carries
                    // its call-site observations under the MODULE's key
                    // (`fold_concern_param_sites`), whichever includer the
                    // sites were in.
                    let origin = spliced_from.as_ref().and_then(|m| m.get(&action.name));
                    let mctx = self.seed_action_params(
                        &ctx,
                        &ctrl_id,
                        origin,
                        &action.name,
                        &action.params,
                        &action.kw_params,
                        action.block_param.as_ref(),
                    );
                    self.body_typer().analyze_expr(&mut action.body, &mctx);
                }
            }

            // The class-side methods (`def self.x`, `class << self`),
            // typed with `self` the class itself. Nothing reads their
            // ivars into a view; typing them is what resolves their own
            // bodies and their harvested return types.
            if retype {
                for method in controller.body.iter_mut().filter_map(|item| match item {
                    ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } => {
                        Some(method)
                    }
                    _ => None,
                }) {
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            self.body_typer().analyze_expr(default, &ctx);
                        }
                    }
                    let mctx = self.seed_method_params(&ctx, &ctrl_id, method, false);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
            }

            // Own-action name keys only — ivar bindings are harvested
            // once below after `action_bodies` is complete (so
            // `instance_variable_set "@#{helper}"` can fold sibling
            // methods). Parent actions are layered in by Phase B.
            let mut action_bindings: HashMap<Symbol, HashMap<Symbol, Ty>> = controller
                .actions()
                .map(|a| (a.name.clone(), HashMap::new()))
                .collect();
            // Body-carrying twin of `action_bindings` — see the field
            // doc on `ControllerMeta::action_bodies`. Seeded
            // unconditionally (every own action/private helper, not
            // just the ones with a nonempty direct binding) because a
            // filter target like `authorize` writes nothing itself but
            // still needs to be a resolvable call target for
            // `collect_transitive_filter_ivars`.
            let mut action_bodies: HashMap<Symbol, Expr> =
                controller.actions().map(|a| (a.name.clone(), a.body.clone())).collect();

            // Register each block filter's synthetic target so the seeding
            // lookups (`merged_before_seed`, the view-ivar build) resolve it.
            for (target, ivars) in block_filter_bindings {
                action_bindings.insert(target, ivars);
            }
            // Same registration for `action_bodies`: a block-form
            // filter's target resolves through `sourced_filters`'
            // `Filter::block` (the full call expr the block-form filter
            // synthesized in `build_sourced_filter_chain`) rather than
            // through `block_filter_bindings` above, which only carries
            // the already-extracted direct ivars, not the body itself.
            // Re-deriving the block body here (instead of threading it
            // out of `build_sourced_filter_chain`'s return) keeps this
            // change out of that function, which a sibling branch also
            // edits.
            for (filter, _source) in &sourced_filters {
                let Some(call_expr) = &filter.block else { continue };
                let ExprNode::Send { block: Some(block), .. } = &*call_expr.node else {
                    continue;
                };
                let body: &Expr = match &*block.node {
                    ExprNode::Lambda { body, .. } => body,
                    _ => block,
                };
                action_bodies.entry(filter.target.clone()).or_insert_with(|| body.clone());
            }

            // Mixed-in concerns: Rails evaluates a module's `included do`
            // in the including class and defines the module's methods on
            // it. The `included do` filters were already spliced into
            // `sourced_filters` at their `include` site above; here we
            // type each module method body against *this* controller's
            // self (matching Rails: the body runs with the controller as
            // `self`) so its ivar assignments (`@account = …` in
            // AccountOwnedConcern#set_account) land in the bindings table
            // the filter seeding consults. Includes close transitively
            // (concerns include concerns); the controller's own
            // definitions win on a name clash.
            let mut mixed_in: Vec<ClassId> = controller_includes(controller);
            let mut seen_modules: BTreeSet<ClassId> = mixed_in.iter().cloned().collect();
            let mut qi = 0;
            while qi < mixed_in.len() {
                let m = mixed_in[qi].clone();
                qi += 1;
                if let Some(nested) = module_includes.get(&m) {
                    for n in nested {
                        if seen_modules.insert(n.clone()) {
                            mixed_in.push(n.clone());
                        }
                    }
                }
            }
            if retype {
                for module_id in &mixed_in {
                    let Some(methods) = module_methods.get(module_id) else { continue };
                    for method in methods {
                        if action_bindings.contains_key(&method.name) {
                            continue;
                        }
                        // Reserve the name so a later concern cannot
                        // overwrite; bindings come from the harvest
                        // below once every body is registered.
                        action_bindings.entry(method.name.clone()).or_default();
                        let mut body = method.body.clone();
                        self.body_typer().analyze_expr(&mut body, &ctx);
                        // Unconditional: a concern method with no DIRECT
                        // write of its own (`authorize`) still needs its
                        // typed body as a resolution target for
                        // `collect_transitive_filter_ivars`.
                        action_bodies.entry(method.name.clone()).or_insert_with(|| body.clone());
                    }
                }
            } else if let Some((cached_bindings, cached_bodies)) =
                self.controller_action_meta_cache.get(&controller.name)
            {
                for module_id in &mixed_in {
                    let Some(methods) = module_methods.get(module_id) else { continue };
                    for method in methods {
                        if action_bindings.contains_key(&method.name) {
                            continue;
                        }
                        if let Some(ivars) = cached_bindings.get(&method.name) {
                            if !ivars.is_empty() {
                                action_bindings.insert(method.name.clone(), ivars.clone());
                            }
                        }
                        if let Some(body) = cached_bodies.get(&method.name) {
                            action_bodies
                                .entry(method.name.clone())
                                .or_insert_with(|| body.clone());
                        }
                    }
                }
            }

            // Harvest every method once `action_bodies` holds the full
            // includer table. `instance_variable_set "@#{helper}"` folds
            // against sibling methods (`instance_name`) and this
            // controller's class name.
            {
                let env = ivar_set::IvarNameEnv {
                    self_class: Some(&controller.name),
                    owned: Some(&action_bodies),
                    lookup: None,
                    models_by_ivar: Some(&models_by_ivar),
                };
                for (name, body) in &action_bodies {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments_in(body, &mut ivars, &env);
                    action_bindings.insert(name.clone(), ivars);
                }
                if retype {
                    self.controller_action_meta_cache.insert(
                        controller.name.clone(),
                        (action_bindings.clone(), action_bodies.clone()),
                    );
                }
            }

            // Effect sets for the persisted chain are stamped once after
            // the typing fixpoint (`stamp_body_effects`); this snapshot
            // is empty here and patched from the converged trees.
            let action_effects: HashMap<Symbol, EffectSet> = HashMap::new();

            let layout = controller.layout.clone();
            meta_by_name.insert(
                controller.name.clone(),
                ControllerMeta {
                    self_ty,
                    sourced_filters,
                    action_bindings,
                    action_bodies,
                    action_effects,
                    class_constants,
                    layout,
                },
            );
        }
        drop(_typing_controllers_a);

        // Controller→layout-view ivar channel: every action that
        // renders also flows its ivars into whatever layout wraps it
        // (resolved via the parent-chain walk below). Ivar reads in
        // the layout (e.g. `@current_user.name` in
        // `layouts/application.html.erb`) then type cleanly against
        // the union of all contributing actions' assignments.
        //
        // Convention: an action's effective layout is the nearest
        // ancestor's `layout` declaration. If every ancestor (and
        // self) is `Inherit`, the layout name defaults to
        // `application` per Rails convention. `LayoutDecl::None`
        // (an explicit `layout false`) suppresses the contribution.
        let mut layout_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        // Each controller's controller-wide ivar environment, kept so
        // the CONCERN MODULES it includes can be typed against it
        // further down. A concern's methods run on the includer and
        // read its ivars — `TrackedRoomVisit#remember_last_room_visited`
        // is `cookies.permanent[:last_room] = @room.id`, and `@room` is
        // RoomsController's `set_room` filter's. Phase A already types
        // a COPY of each concern method against the includer to harvest
        // its bindings, but the module's own body — the one `diagnose`
        // walks, and the one every emitted copy is cut from — was typed
        // with nothing.
        let mut controller_ivar_env: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();

        // Phase-B refinements, collected here and flushed into
        // `self.refined_action_bindings` after the loop — `self` is
        // borrowed immutably inside it.
        let mut refinements: Vec<((ClassId, Symbol), HashMap<Symbol, Ty>)> = Vec::new();

        // ── Phase B: walk each controller's parent chain to build
        // ── inherited (chained) filters + action bindings, then
        // ── re-analyze actions and harvest view ivars.
        //
        // Parent filters run BEFORE child filters (Rails semantics).
        // Action bindings are merged with NEAREST parent first so the
        // closest definition wins on name conflicts (mirrors Ruby
        // method-resolution order).
        let _typing_controllers_b = crate::timings::begin("typing: controllers B");
        for controller in &mut app.controllers {
            let ctrl_name = controller.name.clone();
            let Some(meta) = meta_by_name.get(&ctrl_name) else { continue };
            // Same gate as Phase A, for the same reason: the chained
            // tables, the resolution record and the view channel are
            // this pass's output whether or not the bodies moved.
            let retype = dirty.is_none_or(|d| d.contains(&ctrl_name));

            // Walk the parent chain to collect ancestor metadata,
            // using the pre-built `parent_link_by_name` map (built
            // before the analysis loops so it's available without
            // re-borrowing `app.controllers`). Walks all the way up
            // until hitting a class not registered as a Controller —
            // that's the boundary with framework-supplied parents
            // (e.g., `ActionController::Base`).
            //
            // Parents are recorded as written in source, so the nested
            // declaration style (`module Admin; class AccountsController
            // < BaseController`) records the unqualified `BaseController`
            // while the table keys `Admin::BaseController`. Resolve with
            // Ruby's lexical rule — qualify a single-segment parent
            // against the child's enclosing namespaces, innermost first,
            // falling back to top level. Without this the whole ancestor
            // walk (inherited filters AND inherited actions) silently
            // no-ops for every nested-style controller.
            let resolve_parent = |child: &ClassId, parent: &ClassId| -> ClassId {
                if meta_by_name.contains_key(parent) || parent.0.as_str().contains("::") {
                    return parent.clone();
                }
                let mut segs: Vec<&str> = child.0.as_str().split("::").collect();
                segs.pop(); // drop the class itself, keep enclosing modules
                while !segs.is_empty() {
                    let candidate = ClassId(Symbol::from(
                        format!("{}::{}", segs.join("::"), parent.0.as_str()).as_str(),
                    ));
                    if meta_by_name.contains_key(&candidate) {
                        return candidate;
                    }
                    segs.pop();
                }
                parent.clone()
            };
            let mut ancestors: Vec<(ClassId, &ControllerMeta)> = Vec::new();
            let mut current = ctrl_name.clone();
            let mut walk = controller.parent.clone();
            let mut visited: BTreeSet<ClassId> = BTreeSet::new();
            while let Some(parent_id) = walk {
                let parent_id = resolve_parent(&current, &parent_id);
                if !visited.insert(parent_id.clone()) {
                    // Defensive: cycles shouldn't exist in real Rails
                    // inheritance, but guard against pathological ingests.
                    break;
                }
                let Some(parent_meta) = meta_by_name.get(&parent_id) else { break };
                ancestors.push((parent_id.clone(), parent_meta));
                walk = parent_link_by_name.get(&parent_id).cloned().flatten();
                current = parent_id;
            }

            // Build chained filters: ancestors first (oldest → newest),
            // then self. Rails: `before_action` callbacks fire in
            // registration order, with parent's running before child's.
            // Each entry carries (declaration, defined_in, included_via)
            // — the segment owner is the ancestor (or self) whose class
            // body put the filter in the chain.
            let mut chained_filters: Vec<(Filter, ClassId, ClassId)> = Vec::new();
            for (aid, ancestor) in ancestors.iter().rev() {
                chained_filters.extend(
                    ancestor
                        .sourced_filters
                        .iter()
                        .map(|(f, d)| (f.clone(), d.clone(), aid.clone())),
                );
            }
            chained_filters.extend(
                meta.sourced_filters
                    .iter()
                    .map(|(f, d)| (f.clone(), d.clone(), ctrl_name.clone())),
            );

            // Build chained action_bindings: nearest parent's
            // overlay last so closer-defined targets win.
            let mut chained_bindings: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
            // Overlay each class's Phase-B refinements onto its Phase-A
            // harvest as we walk. Per-KEY, and only where the refined
            // value carries SHAPE — `is_unknown`, not `is_open`, because
            // `Untyped` is as empty an answer as `Var` and unioning it
            // into the controller-wide env widens every sibling binding
            // of the same name. Mastodon's `@account` came out
            // `Account | untyped` under the looser gate, which the IDE
            // smoke reads as a hover regression. This can add an answer,
            // never take one away.
            //
            // An override that calls `super` runs the overridden body
            // too, so its writes (`@message` in `MessagesController#create`
            // under `Messages::ByBotsController#create; super; ...`) stay
            // in the entry beside the override's own.
            let layer = |dst: &mut HashMap<Symbol, HashMap<Symbol, Ty>>,
                             owner: &ClassId,
                             name: &Symbol,
                             ivars: &HashMap<Symbol, Ty>,
                             calls_super: bool| {
                let mut merged = if calls_super {
                    dst.remove(name).unwrap_or_default()
                } else {
                    HashMap::new()
                };
                merged.extend(ivars.iter().map(|(k, v)| (k.clone(), v.clone())));
                if let Some(refined) =
                    self.refined_action_bindings.get(&(owner.clone(), name.clone()))
                {
                    for (k, v) in refined {
                        if is_clean_binding(v) {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                }
                dst.insert(name.clone(), merged);
            };
            let body_calls_super = |bodies: &HashMap<Symbol, Expr>, name: &Symbol| {
                bodies.get(name).is_some_and(expr_calls_super)
            };
            for (aid, ancestor) in ancestors.iter().rev() {
                for (name, ivars) in &ancestor.action_bindings {
                    let sup = body_calls_super(&ancestor.action_bodies, name);
                    layer(&mut chained_bindings, aid, name, ivars, sup);
                }
            }
            for (name, ivars) in &meta.action_bindings {
                let sup = body_calls_super(&meta.action_bodies, name);
                layer(&mut chained_bindings, &ctrl_name, name, ivars, sup);
            }

            // Body-carrying twin of `chained_bindings`, same flat
            // method-name → resolution table and the same
            // nearest-definition-wins ordering (ancestors oldest-first,
            // then self last). Unlike `chained_bindings` it carries no
            // Phase-B refinement layer — it exists only to give
            // `collect_transitive_filter_ivars` a body to resolve a
            // filter target's own receiverless calls into, immediately
            // below. Borrowed from `action_bodies` so we do not clone
            // every ancestor filter-target tree per controller per round.
            let mut chained_bodies: HashMap<Symbol, &Expr> = HashMap::new();
            for (_, ancestor) in ancestors.iter().rev() {
                for (name, body) in &ancestor.action_bodies {
                    chained_bodies.insert(name.clone(), body);
                }
            }
            for (name, body) in &meta.action_bodies {
                chained_bodies.insert(name.clone(), body);
            }

            // `self.class` in an inherited / mixed-in method is the
            // receiver class, so re-fold `instance_variable_set` names
            // against THIS controller. Parent harvest used the class
            // that wrote `include`, which is the wrong demodulize for
            // a subclass that only inherits the filter. Replace the
            // method's map (do not merge): a stale `@leaf_record` from
            // the parent's class name must not sit beside `@widget`.
            {
                let lookup = |n: &Symbol| chained_bodies.get(n).copied();
                let env = ivar_set::IvarNameEnv {
                    self_class: Some(&ctrl_name),
                    owned: None,
                    lookup: Some(&lookup),
                    models_by_ivar: Some(&models_by_ivar),
                };
                for (name, body) in &chained_bodies {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments_in(body, &mut ivars, &env);
                    ivars.retain(|_, v| !v.is_open());
                    if ivars.is_empty() {
                        continue;
                    }
                    // An override calling `super` keeps the overridden
                    // body's writes layered above; only its own are
                    // re-folded here.
                    if expr_calls_super(body) {
                        chained_bindings.entry(name.clone()).or_default().extend(ivars);
                    } else {
                        chained_bindings.insert(name.clone(), ivars);
                    }
                }
            }

            // Transitive filter-target ivar writes: a Before/Around
            // filter target whose own body writes no ivar directly, but
            // reaches one through its own receiverless calls (Procore's
            // `authorize` → `set_variables_in_authorize!` →
            // `set_project_variables!` → `@project = ...`, three
            // methods away), gets that write folded into its
            // `chained_bindings` entry here. This runs once per
            // controller, before either seeding sweep below, so both
            // the per-action `merged_before_seed` overlay and the
            // controller-wide union just below read the augmented
            // entry for free — no other change needed to either.
            // See `collect_transitive_filter_ivars` for the depth cap
            // and may-write/nilability rules.
            for (filter, _, _) in &chained_filters {
                if !matches!(filter.kind, FilterKind::Before | FilterKind::Around) {
                    continue;
                }
                let Some(target_body) = chained_bodies.get(&filter.target) else { continue };
                let mut visited: BTreeSet<Symbol> = BTreeSet::new();
                visited.insert(filter.target.clone());
                let transitive = collect_transitive_filter_ivars(
                    target_body,
                    &chained_bodies,
                    MAX_FILTER_CALL_DEPTH,
                    &mut visited,
                    true,
                );
                if transitive.is_empty() {
                    continue;
                }
                let entry = chained_bindings.entry(filter.target.clone()).or_default();
                for (k, v) in transitive {
                    let merged = match entry.remove(&k) {
                        Some(prev) => crate::analyze::body::union_of(prev, v),
                        None => v,
                    };
                    entry.insert(k, merged);
                }
            }

            // Controller-wide ivar environment: in Ruby, instance
            // variables are shared mutable state across every method
            // invoked during a request, not per-method locals. A
            // `before_action :find_story` sets `@story`; a private
            // helper (`load_user_votes`) or a sibling action then
            // reads it without any syntactic assignment in its own
            // body. The per-action `merged_before_seed` only seeds
            // routed actions gated by `only:`/`except:`, so those
            // helper reads — and reads of assignments buried inside a
            // branch (`if (@message = ...)`) earlier in the same
            // method — bottom out as `ivar_unresolved`.
            //
            // Build a controller-wide union of every ivar assignment
            // (own + inherited, across all methods/filters) and seed
            // it as the BASE layer of every method. The per-action
            // `merged_before_seed` overlays on top (more precise for
            // the action's actual entry state), and the body-typer's
            // own flow refines further per-statement.
            //
            // The Nil arm is stripped from each base type: across a
            // method boundary the type system can't see the
            // find-then-guard idiom (`@x = M.find_by(..); redirect
            // unless @x`) that makes these ivars non-nil on the path
            // that reaches the reader, and keeping the Nil arm would
            // only trade an `ivar_unresolved` for a `send_dispatch`
            // on the (unreachable) nil case. `Var`/`Bottom` carry no
            // usable shape and are dropped.
            // Two seeding sweeps: a filter method's own binding may
            // depend on an ivar *another* filter seeds — Mastodon's
            // `set_status` reads the concern-seeded `@account` — and
            // Pass A harvested every binding before any seed existed,
            // leaving such dependent bindings `Var`. After the first
            // re-analysis retypes the bodies with the first-round seed,
            // re-harvest the bindings and seed once more. One extra
            // sweep resolves one filter→filter dependency hop; deeper
            // chains stay unresolved until a real fixpoint earns its
            // cost.
            for sweep in 0..2 {
                let controller_wide: HashMap<Symbol, Ty> = {
                    let mut env: HashMap<Symbol, Ty> = HashMap::new();
                    for ivars in chained_bindings.values() {
                        for (k, v) in ivars {
                            if v.is_open() {
                                continue;
                            }
                            let merged = match env.remove(k) {
                                Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                                None => v.clone(),
                            };
                            env.insert(k.clone(), merged);
                        }
                    }
                    let out: HashMap<Symbol, Ty> = env.into_iter()
                        .map(|(k, v)| (k, v.strip_nil()))
                        .collect();
                    controller_ivar_env.insert(ctrl_name.clone(), out.clone());
                    out
                };

                // Pass B: re-analyze every method with the controller-wide
                // base seed plus any before_action-specific overlay. Every
                // method (routed action or private helper) is re-analyzed
                // so cross-method ivar reads resolve.
                if retype && (!controller_wide.is_empty() || !chained_filters.is_empty()) {
                    for action in controller.actions_mut() {
                        let mut seed = controller_wide.clone();
                        // Overlay the action's precise before_action seed:
                        // for an action that actually runs the filter, the
                        // filter's exact binding (including any Nil arm the
                        // action narrows itself) wins over the stripped base.
                        for (k, v) in
                            merged_before_seed(&chained_filters, &action.name, &chained_bindings)
                        {
                            seed.insert(k, v);
                        }
                        if seed.is_empty() {
                            continue;
                        }
                        let base_ctx = Ctx {
                            self_ty: Some(meta.self_ty.clone()),
                            ivar_bindings: seed,
                            local_bindings: HashMap::new(),
                            class_objects: Default::default(),
                            constants: meta.class_constants.clone(),
                            annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
                        };
                        // Seed helper-method params from the inferred-params
                        // table too, so `period(query)`'s body resolves on
                        // the re-analysis pass (matches Pass A).
                        let origin = app
                            .concern_spliced_actions
                            .get(&ctrl_name)
                            .and_then(|m| m.get(&action.name));
                        let inner_ctx = self.seed_action_params(
                            &base_ctx,
                            &ctrl_name,
                            origin,
                            &action.name,
                            &action.params,
                            &action.kw_params,
                            action.block_param.as_ref(),
                        );
                        self.body_typer().analyze_expr(&mut action.body, &inner_ctx);
                    }
                }

                if sweep == 1 {
                    break;
                }
                // Re-harvest bindings from the retyped bodies; only a
                // refinement (a previously Var/absent binding now
                // carrying shape) triggers the second sweep.
                let mut refined = false;
                let env = ivar_set::IvarNameEnv {
                    self_class: Some(&ctrl_name),
                    owned: None,
                    lookup: None,
                    models_by_ivar: Some(&models_by_ivar),
                };
                for action in controller.actions() {
                    let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                    extract_ivar_assignments_in(&action.body, &mut ivars, &env);
                    for (k, v) in ivars {
                        if v.is_open() {
                            continue;
                        }
                        let entry = chained_bindings.entry(action.name.clone()).or_default();
                        let stale = entry
                            .get(&k)
                            .is_none_or(|t| t.is_open());
                        if stale {
                            entry.insert(k, v);
                            refined = true;
                        }
                    }
                }
                if !refined {
                    break;
                }
            }

            // Persist this controller's refinements so a SUBCLASS
            // analyzed later — this round or the next — inherits them.
            // `chained_bindings` also holds the ancestors' entries, so
            // only names this controller actually defines are recorded;
            // an inherited entry stays attributed to the class that
            // owns it.
            {
                let own: BTreeSet<Symbol> =
                    controller.actions().map(|a| a.name.clone()).collect();
                for (name, ivars) in &chained_bindings {
                    if !own.contains(name) {
                        continue;
                    }
                    let shaped: HashMap<Symbol, Ty> = ivars
                        .iter()
                        .filter(|(_, v)| is_clean_binding(v))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    if shaped.is_empty() {
                        continue;
                    }
                    refinements.push(((ctrl_name.clone(), name.clone()), shaped));
                }
            }

            // Resolve this controller's effective layout view name by
            // walking the inheritance chain. First explicit decl wins;
            // an explicit `LayoutDecl::None` suppresses the layout
            // contribution entirely. If nothing is declared anywhere
            // up the chain, Rails convention falls back to
            // `layouts/application`.
            let effective_layout: Option<Symbol> = {
                let mut decl = &meta.layout;
                let mut iter = ancestors.iter();
                while matches!(decl, LayoutDecl::Inherit) {
                    match iter.next() {
                        Some((_, a)) => decl = &a.layout,
                        None => break,
                    }
                }
                match decl {
                    LayoutDecl::Name { name } => {
                        Some(Symbol::from(format!("layouts/{}", name.as_str())))
                    }
                    LayoutDecl::None { .. } => None,
                    LayoutDecl::Inherit => Some(Symbol::from("layouts/application")),
                }
            };

            // Persist what this walk just resolved — the chained filter
            // list with provenance and the effective layout — instead of
            // discarding it: `ide::traceroute` and gap attribution
            // compose over `App::controller_resolutions` rather than
            // re-deriving the ancestor walk. `assigns`/`effects` resolve
            // each filter target against the chained tables (nearest
            // definition wins; bindings are post-sweep). Skip entries
            // name a filter to remove, not code that runs, so they carry
            // neither.
            {
                let mut chained_effects: HashMap<Symbol, EffectSet> = HashMap::new();
                for (_, ancestor) in ancestors.iter().rev() {
                    for (name, eff) in &ancestor.action_effects {
                        chained_effects.insert(name.clone(), eff.clone());
                    }
                }
                for (name, eff) in &meta.action_effects {
                    chained_effects.insert(name.clone(), eff.clone());
                }
                // Own methods were re-analyzed by the sweeps above —
                // prefer their refreshed effect sets over the Phase A
                // snapshot in `meta`.
                for action in controller.actions() {
                    chained_effects.insert(action.name.clone(), action.effects.clone());
                }
                let noise = |t: &Ty| t.is_open();
                let filter_chain: Vec<crate::app::ResolvedFilter> = chained_filters
                    .iter()
                    .map(|(filter, defined_in, included_via)| {
                        let runs = !filter.kind.is_skip();
                        let assigns: HashMap<Symbol, Ty> = if runs {
                            chained_bindings
                                .get(&filter.target)
                                .map(|ivars| {
                                    ivars
                                        .iter()
                                        .filter(|(_, v)| !noise(v))
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect()
                                })
                                .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        let effects = if runs {
                            chained_effects.get(&filter.target).cloned().unwrap_or_default()
                        } else {
                            EffectSet::default()
                        };
                        crate::app::ResolvedFilter {
                            filter: filter.clone(),
                            defined_in: defined_in.clone(),
                            included_via: included_via.clone(),
                            assigns,
                            effects,
                        }
                    })
                    .collect();
                controller_resolutions.insert(
                    ctrl_name.clone(),
                    crate::app::ControllerResolution {
                        filter_chain,
                        layout: effective_layout.clone(),
                    },
                );
            }

            // Build the per-view ivar map. Each view gets the action's
            // own assignments *plus* any before_action contribution
            // (which isn't syntactically present in the action body) —
            // both own and inherited filters apply. The same merged
            // ivar set is also folded into the effective layout's map
            // (union of names, union of types across all contributing
            // actions and controllers).
            for action in controller.actions() {
                let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                let env = ivar_set::IvarNameEnv {
                    self_class: Some(&ctrl_name),
                    owned: None,
                    lookup: None,
                    models_by_ivar: Some(&models_by_ivar),
                };
                extract_ivar_assignments_in(&action.body, &mut ivars, &env);
                bind_framework_assigned_ivars(&action.body, &mut ivars);
                for (filter, _, _) in &chained_filters {
                    if matches!(filter.kind, FilterKind::Before | FilterKind::Around)
                        && before_filter_applies(filter, &action.name)
                    {
                        if let Some(fivars) = chained_bindings.get(&filter.target) {
                            for (k, v) in fivars {
                                ivars.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                    }
                }
                // Ivars written by same-controller helper methods this
                // action explicitly *calls* (`def standing; flag_warning;
                // end`). Ruby shares instance variables across the call,
                // but the write lives in the callee's body — invisible to
                // `extract_ivar_assignments` on the action, and not gated
                // by any `before_action` (that path is folded just above).
                // The callee's own bindings are already in
                // `chained_bindings` (its action/filter/parent snapshot,
                // resolved by the sweeps above), so fold them in for every
                // implicit-self call the action makes — the same
                // contribution a before_action to that method would give,
                // triggered by the call site instead. One level deep: a
                // helper that itself calls another ivar-writing helper is
                // not chased (the direct-call case is what recurs). Own
                // and before_action assignments already present win.
                let mut sites: Vec<SendSite> = Vec::new();
                // Only own-class sites are consumed below, so helper
                // attribution is irrelevant — an empty index keeps
                // this walk exactly as before.
                self.collect_send_sites(&action.body, Some(&ctrl_name), false, &HashMap::new(), &mut sites);
                for (class_id, method, _, _, _) in &sites {
                    if *class_id != ctrl_name {
                        continue;
                    }
                    if let Some(hivars) = chained_bindings.get(method) {
                        for (k, v) in hivars {
                            if v.is_open() {
                                continue;
                            }
                            ivars.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
                // An override calling `super` renders with what the
                // overridden action wrote too (its entry in
                // `chained_bindings` keeps both layers).
                if expr_calls_super(&action.body) {
                    if let Some(inherited) = chained_bindings.get(&action.name) {
                        for (k, v) in inherited {
                            if v.is_open() {
                                continue;
                            }
                            ivars.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
                if let Some(layout_name) = &effective_layout {
                    view_feeders
                        .entry(layout_name.clone())
                        .or_default()
                        .insert(ctrl_name.clone());
                    let layout_map = layout_ivars_by_view
                        .entry(layout_name.clone())
                        .or_default();
                    for (k, v) in &ivars {
                        // `Ty::Var` / `Ty::Untyped` carry no usable
                        // shape — they pollute the union without
                        // refining it. Drop them in either slot:
                        //   - new is noise → keep prev (or noise if no prev)
                        //   - prev is noise → take new
                        // Without this, N controllers each failing to
                        // type `@user` would either fan a Var into
                        // every union variant or be order-sensitive.
                        let noise = |t: &Ty| t.is_unknown();
                        let merged = match layout_map.remove(k) {
                            Some(prev) if noise(&prev) => v.clone(),
                            Some(prev) if noise(v) => prev,
                            Some(prev) if prev == *v => prev,
                            Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                            None => v.clone(),
                        };
                        layout_map.insert(k.clone(), merged);
                    }
                }
                // Content-partial seeding: when this action assigns a
                // string literal to a dynamic-render ivar
                // (`@above = 'for_domain'`) and a partial with the
                // resolved name exists, fold this action's ivars into
                // that partial's seed. Drives `@domain` / `@tag` /
                // `@categories` in the home content partials, which
                // `render partial: @above` only reaches at runtime.
                let prefix = controller_view_prefix(&ctrl_name);
                if !dynamic_render_ivars.is_empty() {
                    let mut literals = Vec::new();
                    collect_content_partial_literals(
                        &action.body,
                        &dynamic_render_ivars,
                        &mut literals,
                    );
                    for lit in literals {
                        let partial = content_partial_view_name(&lit, &prefix);
                        if !existing_view_names.contains(&partial) {
                            continue;
                        }
                        let entry = content_partial_ivars.entry(partial).or_default();
                        for (k, v) in &ivars {
                            if v.is_open() {
                                continue;
                            }
                            let merged = match entry.remove(k) {
                                Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                                None => v.clone(),
                            };
                            entry.insert(k.clone(), merged);
                        }
                    }
                }
                // Action→view ivar channel. An action's ivars seed
                // every full template it renders: its primary
                // RenderTarget plus any `render :action`/`:template`/
                // `render_to_string :action` buried in a block. Union
                // (not overwrite) across all actions that feed a given
                // view — multiple actions render `:action => "index"`,
                // and the shared template reads the union of their
                // ivars, exactly like the layout-ivar union above.
                let mut view_targets: Vec<Symbol> = Vec::new();
                if let Some(view_name) = view_name_for_action(&ctrl_name, action) {
                    view_targets.push(view_name);
                }
                // The CONVENTIONAL template, whenever one exists. An
                // explicit `render` anywhere in the body — campfire's
                // `create` has one in a `rescue` — makes
                // `view_name_for_action` answer THAT template and only
                // that one, so the action's own
                // `messages/create.turbo_stream.erb` (the whole
                // message-post response) was fed by nothing. Rails
                // renders the conventional template on any path that
                // does not render or redirect, which is exactly the
                // path a rescue's render is the exception to. Gated on
                // the view EXISTING, so no phantom entry is minted for
                // a private helper or a redirect-only action.
                let conventional =
                    Symbol::from(format!("{prefix}/{}", action.name.as_str()).as_str());
                if existing_view_names.contains(&conventional) {
                    view_targets.push(conventional);
                }
                collect_action_render_views(&action.body, &prefix, &mut view_targets);
                // FORMAT VARIANTS of each target. Rails picks
                // `messages/create.turbo_stream.erb` over
                // `messages/create.html.erb` by the request's format,
                // and either one is the SAME action's template — but
                // the variant's view name carries the format suffix
                // (`messages/create.turbo_stream`) and so matched no
                // seed at all. campfire's whole message-post response
                // is that template, and both its reads of `@message`
                // reported `has no known type` about an ivar the action
                // two lines up assigns.
                for target in view_targets.clone() {
                    let stem = format!("{}.", target.as_str());
                    view_targets.extend(
                        existing_view_names
                            .iter()
                            .filter(|v| v.as_str().starts_with(&stem))
                            .cloned(),
                    );
                }
                view_targets.sort();
                view_targets.dedup();
                for view_name in view_targets {
                    view_feeders.entry(view_name.clone()).or_default().insert(ctrl_name.clone());
                    let entry = action_ivars_by_view.entry(view_name).or_default();
                    for (k, v) in &ivars {
                        let noise = |t: &Ty| t.is_unknown();
                        let merged = match entry.remove(k) {
                            Some(prev) if noise(&prev) => v.clone(),
                            Some(prev) if noise(v) => prev,
                            Some(prev) if prev == *v => prev,
                            Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                            None => v.clone(),
                        };
                        entry.insert(k.clone(), merged);
                    }
                }
            }

            // Inherited actions: a subclass that defines no `show` of
            // its own still renders `<child_prefix>/show` through the
            // parent-defined action — Admin::Settings::DiscoveryController
            // renders admin/settings/discovery/show from
            // Admin::SettingsController#show. The own-actions loop above
            // keys views by the defining controller only, so
            // ancestor-defined actions seeded nothing under the child's
            // prefix. Walk ancestors nearest-first (Ruby MRO); the
            // existing-view gate keeps private-helper bindings (which
            // share the bindings table) from minting phantom entries.
            {
                let own_names: BTreeSet<Symbol> =
                    controller.actions().map(|a| a.name.clone()).collect();
                let prefix = controller_view_prefix(&ctrl_name);
                let mut seen_inherited: BTreeSet<Symbol> = BTreeSet::new();
                for (_, ancestor) in &ancestors {
                    for (name, binds) in &ancestor.action_bindings {
                        if own_names.contains(name) || !seen_inherited.insert(name.clone()) {
                            continue;
                        }
                        let view_name =
                            Symbol::from(format!("{prefix}/{}", name.as_str()).as_str());
                        if !existing_view_names.contains(&view_name) {
                            continue;
                        }
                        // The child's full filter chain applies when the
                        // inherited action runs in the child (same merge
                        // rule as the own-actions loop: action bindings
                        // win over filter contributions).
                        let mut ivars = binds.clone();
                        for (filter, _, _) in &chained_filters {
                            if matches!(filter.kind, FilterKind::Before | FilterKind::Around)
                                && before_filter_applies(filter, name)
                            {
                                if let Some(fivars) = chained_bindings.get(&filter.target) {
                                    for (k, v) in fivars {
                                        ivars.entry(k.clone()).or_insert_with(|| v.clone());
                                    }
                                }
                            }
                        }
                        view_feeders
                            .entry(view_name.clone())
                            .or_default()
                            .insert(ctrl_name.clone());
                        let entry = action_ivars_by_view.entry(view_name).or_default();
                        for (k, v) in &ivars {
                            let noise = |t: &Ty| t.is_unknown();
                            let merged = match entry.remove(k) {
                                Some(prev) if noise(&prev) => v.clone(),
                                Some(prev) if noise(v) => prev,
                                Some(prev) if prev == *v => prev,
                                Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                                None => v.clone(),
                            };
                            entry.insert(k.clone(), merged);
                        }
                    }
                }
            }

        }
        drop(_typing_controllers_b);
        // Flush Phase B's refinements. Later rounds of the whole-program
        // fixpoint read them back through `layer` above, which is what
        // carries a parent's refined binding down to a subclass.
        for (key, ivars) in refinements {
            let entry = self.refined_action_bindings.entry(key).or_default();
            for (k, v) in ivars {
                entry.insert(k, v);
            }
        }

        // The union of every includer's ivar environment, per concern.
        // Derived here because `controller_ivar_env` is only complete
        // now, and derived ONCE because both readers below — Phase B′
        // and the library-class loop — want the same answer and neither
        // writes `controller_ivar_env` again.
        let concern_ivar_env = concern_ivar_env_of(app, &controller_ivar_env, module_includes);

        // ── Phase B′: re-type the concern methods spliced into each
        // ── includer, against the CONCERN's ivar environment.
        //
        // `splice_concerns_into_controllers` copies a concern's methods
        // into the class that includes it, and Phase B above typed each
        // copy against THAT class's environment. For a concern included
        // high in the chain that is the wrong environment: campfire puts
        // `include TrackedRoomVisit` on ApplicationController, and its
        // `remember_last_room_visited` reads `@room` — which
        // ApplicationController never sets, because the method runs as a
        // before_action on the Rooms controllers BELOW it. The copy typed
        // `@room` as nothing and `diagnose` reported it at the concern's
        // own source line, which reads as a compiler bug rather than the
        // seeding gap it is.
        //
        // The honest seed is the union across includers — exactly what
        // `concern_ivar_env_of` computes, and it is available only HERE:
        // it is derived from `controller_ivar_env`, which the Phase B loop
        // is still filling until the line above. So this is a separate
        // pass rather than an overlay inside Phase B, and it is additive:
        // the controller's own binding still wins wherever it has one, and
        // only names the includer lacks are filled from the concern.
        if !app.concern_spliced_actions.is_empty() {
            let _typing_concerns = crate::timings::begin("typing: concern splice");
            let origins = app.concern_spliced_actions.clone();
            for controller in &mut app.controllers {
                let Some(by_method) = origins.get(&controller.name) else { continue };
                if dirty.is_some_and(|d| !d.contains(&controller.name)) {
                    continue;
                }
                let own_env = controller_ivar_env.get(&controller.name).cloned()
                    .unwrap_or_default();
                let self_ty = Ty::Class { id: controller.name.clone(), args: vec![] };
                let ctrl_name = controller.name.clone();
                // The same constants Phase B typed this controller's
                // bodies with. Passing an empty map here would make the
                // re-type LOSE a constant binding Phase B had already
                // established — this pass must only ever add.
                let class_constants =
                    global_constants.with_own(extract_controller_const_assignments(&controller.body));
                for action in controller.actions_mut() {
                    let Some(module) = by_method.get(&action.name) else { continue };
                    let Some(from_concern) = concern_ivar_env.get(module) else { continue };
                    let mut seed = own_env.clone();
                    let mut filled = false;
                    for (k, v) in from_concern {
                        // The includer's own answer wins; the concern only
                        // fills what that environment has nothing to say
                        // about. An `is_open` arm is nothing to say.
                        let vacant = seed.get(k).is_none_or(|t| t.is_open());
                        if vacant {
                            seed.insert(k.clone(), v.clone());
                            filled = true;
                        }
                    }
                    if !filled {
                        continue;
                    }
                    let base_ctx = Ctx {
                        self_ty: Some(self_ty.clone()),
                        ivar_bindings: seed,
                        local_bindings: HashMap::new(),
                        class_objects: Default::default(),
                        constants: class_constants.clone(),
                        annotate_self_dispatch: false,
                        in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
                    };
                    let origin = app
                        .concern_spliced_actions
                        .get(&ctrl_name)
                        .and_then(|m| m.get(&action.name));
                    let inner_ctx = self.seed_action_params(
                        &base_ctx,
                        &ctrl_name,
                        origin,
                        &action.name,
                        &action.params,
                        &action.kw_params,
                        action.block_param.as_ref(),
                    );
                    self.body_typer().analyze_expr(&mut action.body, &inner_ctx);
                }
            }
        }
        // Library classes (non-model classes under app/models/): mirror
        // the per-model body typing pass on a smaller surface — no
        // schema attributes, no associations, just methods. Two-pass
        // ivar discovery handles `def initialize(x); @x = x; end`
        // shapes where reads in subsequent methods (`@x.foo`) resolve
        // against the type written in initialize.
        // Mailer→view ivar channel: a mailer action's `@resource = …`
        // bindings seed its template the same way a controller action's
        // do (`UserMailer#welcome` renders `user_mailer/welcome.html.*`
        // — Rails' implicit template lookup, no render call in source).
        // Identify mailers by parent chain up front; the harvest itself
        // rides the library-class typing loop below, after each method
        // body has been typed once.
        let mailer_names: std::collections::HashSet<ClassId> = {
            let parent_of: HashMap<&ClassId, Option<&ClassId>> = app
                .library_classes
                .iter()
                .map(|lc| (&lc.name, lc.parent.as_ref()))
                .collect();
            app.library_classes
                .iter()
                .filter(|lc| {
                    let mut cur = Some(&lc.name);
                    let mut depth = 0usize;
                    while let Some(id) = cur {
                        // `Devise::Mailer` is itself an ActionMailer
                        // subclass living in the gem — app mailers that
                        // extend it (Mastodon's UserMailer) dead-end
                        // there, so accept it as a terminal too.
                        if matches!(id.0.as_str(), "ActionMailer::Base" | "Devise::Mailer") {
                            return true;
                        }
                        depth += 1;
                        if depth > 32 {
                            break;
                        }
                        cur = parent_of.get(id).copied().flatten();
                    }
                    false
                })
                .map(|lc| lc.name.clone())
                .collect()
        };

        // A `CurrentAttributes` ivar is written from OUTSIDE the class,
        // through the class-level forwarder `ingest::current_attributes`
        // synthesizes (`Current.session = session`). The syntactic
        // harvest below reads the class's OWN methods, where the only
        // write it can see is the generated `def session=(value); @session
        // = value; end` — a parameter with no type. So the whole class
        // was shapeless, and campfire routes essentially every
        // per-request read through it: `Current.user.rooms`, and every
        // ivar downstream of one, landed unresolved.
        //
        // Survey the app for those writes and let their VALUE types be
        // the seed. This is evidence, not convention: the type is
        // whatever the app actually assigns.
        //
        // The survey is a whole-app walk, so it is scoped to the
        // classes the loop below will actually read it for. Narrowed
        // dirty sets already include every `current_attribute_classes`
        // entry (see `dirty_retype`); mailers stay surveyed either way
        // because their ivar harvest feeds views.
        let current_attribute_writes: HashMap<ClassId, HashMap<Symbol, Ty>> = {
            let targets: std::collections::HashSet<&ClassId> = app
                .current_attribute_classes
                .iter()
                .filter(|id| {
                    dirty.is_none_or(|d| d.contains(id)) || mailer_names.contains(id)
                })
                .collect();
            let mut out: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
            if !targets.is_empty() {
                let mut collect = |body: &crate::expr::Expr| {
                    collect_const_attr_writes(body, &targets, &mut out);
                };
                crate::lower::for_each_hook_body_ref(app, &mut collect);
                for view in &app.views {
                    collect(&view.body);
                }
            }
            out
        };

        // A concern's methods RUN ON THE INCLUDER. Typed against the
        // module's own registry, `sessions.pluck(:ip_address)` in
        // `User::Bannable` had no `sessions` to resolve — the module
        // declares no association — and every send in the body came
        // out untyped: the `compact_blank` behind it was never
        // grounded, and on spinel the whole chain was `undefined method
        // 'each' for unknown` (campfire's ban flow, 5 tests). When
        // exactly one class includes the module (transitively — a
        // concern that includes a concern is still one includer's), its
        // bodies are typed with THAT class as `self`, which is the
        // class Ruby gives them. Several includers keep the module's
        // own view: a body typed against one includer would be wrong
        // for the others, and a union `self` is a poly cliff on every
        // send.
        let sole_includer = app.sole_includer_of_modules();

        // Parameterized mailers: `ProductMailer.with(product: self,
        // subscriber: subscriber).in_stock` makes `params[:product]` a
        // Product inside the mailer and its templates — the `.with`
        // hash IS the mailer's params, not the request's. Harvest every
        // call site's kwargs (typed by the pass that walked the caller;
        // the fixpoint re-runs this with refined types) into one row
        // per mailer, union per key across sites.
        let mailer_with_params = harvest_mailer_with_params(app, &mailer_names);
        // `.with` rows advance from (possibly retyped) callers without
        // marking the mailer dirty via the caller-closure. Force body
        // retype when the params row moved so `@x = params[:x]` ivars
        // in ViewSeeds stay aligned with template `params`.
        let params_sym = Symbol::from("params");
        let mailers_needing_retype: std::collections::HashSet<ClassId> = mailer_names
            .iter()
            .filter_map(|m| {
                let row = mailer_with_params.get(m)?;
                let new_ty = Ty::Record { row: row.clone() };
                let old = self
                    .classes
                    .get(m)
                    .and_then(|c| c.instance_methods.get(&params_sym));
                if old == Some(&new_ty) {
                    None
                } else {
                    Some(m.clone())
                }
            })
            .collect();
        let mut mailer_params_by_view: HashMap<Symbol, Ty> = HashMap::new();

        let _typing_library = crate::timings::begin("typing: library");
        for lc in &mut app.library_classes {
            if let Some(row) = mailer_with_params.get(&lc.name) {
                self.classes
                    .entry(lc.name.clone())
                    .or_default()
                    .instance_methods
                    .insert(params_sym.clone(), Ty::Record { row: row.clone() });
            }
            // Same gate as the controller loops, with one thing it still
            // owes the views half: a MAILER's ivar harvest seeds its
            // template, so the `flow_ivars` walk below runs for one even
            // when its bodies are left alone.
            let lc_name = lc.name.clone();
            let retype = dirty.is_none_or(|d| d.contains(&lc_name))
                || mailers_needing_retype.contains(&lc_name);
            if !retype && !mailer_names.contains(&lc_name) {
                continue;
            }
            let self_id = if lc.is_module {
                sole_includer.get(&lc.name).cloned().unwrap_or_else(|| lc.name.clone())
            } else {
                lc.name.clone()
            };
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: self_id, args: vec![] }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                class_objects: Default::default(),
                constants: Default::default(), annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
            };

            if retype {
                for initializer in &mut lc.class_ivar_initializers {
                    self.body_typer().analyze_expr(initializer, &class_ctx);
                }
                for (_, value) in &mut lc.constants {
                    if self.data_factories.contains_key(&value.span) {
                        self.body_typer().analyze_expr(value, &class_ctx);
                    }
                }
                for method in &mut lc.methods {
                    // A default is an expression of the class body too, and
                    // its type is half of what an optional parameter IS:
                    // `for_user = Current.user` is a User whenever the
                    // caller leaves it out. Typed here so `seed_method_params`
                    // and the stamped signature can fold it in. Use the
                    // seeded method ctx so claimed-macro templates
                    // (`has_markdown(name, strict_loading:
                    // strict_loading_by_default)`) do not ledger
                    // ActiveRecord::Base noise on the default expression.
                    let mctx = self.seed_method_params(&class_ctx, &lc_name, method, true);
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            self.body_typer().analyze_expr(default, &mctx);
                        }
                    }
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
            }

            let mut flow_ivars: HashMap<Symbol, Ty> = HashMap::new();
            for method in &lc.methods {
                extract_ivar_assignments(&method.body, &mut flow_ivars);
            }
            // A CONTROLLER CONCERN's ivars are the includer's. Its
            // methods run on the controller and read what the
            // controller's filters set, so the module's own body is
            // typed against the union of every includer's environment.
            // Its own assignments win — those are the concern's answer
            // about itself.
            if let Some(env) = concern_ivar_env.get(&lc_name) {
                for (k, v) in env {
                    flow_ivars.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            // The write sites win over the syntactic harvest: for a
            // CurrentAttributes attribute the harvest sees only the
            // generated writer's untyped parameter, which is the
            // absence of an answer, not a competing one.
            let is_current_attributes = current_attribute_writes.contains_key(&lc_name);
            if let Some(writes) = current_attribute_writes.get(&lc_name) {
                for (name, ty) in writes {
                    // The Nil arm is ADDED, unlike the controller-wide
                    // ivar seed's `strip_nil`: a CurrentAttributes
                    // attribute is nil until the request's setup writes
                    // it and is reset after, so nil is the attribute's
                    // own state, not evidence the write sites carry.
                    // They used to carry it by accident — `Current.session
                    // = session` under `if session = find_session_by_cookie`
                    // read `Session?` until the narrowing (fbd7f350) made
                    // it `Session` — and the day it left, `def signed_in?;
                    // Current.user.present?; end` folded to `true` against
                    // the non-nilable type: a correct fold of an incorrect
                    // type, campfire signed everyone in and the join-code
                    // page stopped 404ing (users_controller 6 -> 2). A lie
                    // the type system can act on is worse than a gap.
                    flow_ivars.insert(
                        name.clone(),
                        crate::analyze::body::union_of(ty.clone(), Ty::Nil),
                    );
                }
            }

            if mailer_names.contains(&lc_name) {
                let prefix = lc_name
                    .0
                    .as_str()
                    .split("::")
                    .map(crate::naming::snake_case)
                    .collect::<Vec<_>>()
                    .join("/");
                for method in &lc.methods {
                    if method.receiver != crate::dialect::MethodReceiver::Instance
                        || method.kind != crate::dialect::AccessorKind::Method
                        || method.name.as_str() == "initialize"
                    {
                        continue;
                    }
                    let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                    extract_ivar_assignments(&method.body, &mut ivars);
                    // Back-fill from the class-wide flow set, nil-widened:
                    // mailers set shared ivars in `before_action` filters
                    // (`set_instance` → `@instance`), which the ingest
                    // doesn't attribute per-action. The action's own
                    // precise bindings win; class-wide ones arrive as
                    // `T | Nil` since we can't prove the filter ran.
                    for (name, ty) in &flow_ivars {
                        ivars.entry(name.clone()).or_insert_with(|| {
                            crate::analyze::body::union_of(ty.clone(), Ty::Nil)
                        });
                    }
                    let view_name = Symbol::from(format!("{prefix}/{}", method.name.as_str()).as_str());
                    if let Some(row) = mailer_with_params.get(&lc_name) {
                        mailer_params_by_view.insert(view_name.clone(), Ty::Record { row: row.clone() });
                    }
                    if ivars.is_empty() {
                        continue;
                    }
                    action_ivars_by_view.entry(view_name).or_default().extend(ivars);
                }
            }

            let initialized = ivars_initialized_by(lc.methods.iter());
            if retype && !flow_ivars.is_empty() {
                let mut reseeded: HashMap<Symbol, Ty> = HashMap::new();
                for (name, ty) in flow_ivars {
                    // Nil-widening is right for a class whose ivars are
                    // set by SOME path through its own methods; it is
                    // wrong for CurrentAttributes, where the writes are
                    // the request's own setup and every read runs after
                    // them. Widening there would trade one
                    // `has no known type` for a `no known method … on
                    // Union { User, Nil }` on the very next hop — the
                    // same reasoning the controller-wide seed's
                    // `strip_nil` already carries.
                    // Nor for an ivar `initialize` assigns as one of its own
                    // statements: it is set before any other method can run,
                    // so `@total = T.let(attrs[:total], Money)` reads as a
                    // `Money` everywhere, not `Money?`. The union with the
                    // other writes already keeps a `nil` some method writes.
                    let seeded = if is_current_attributes || (initialized.contains(&name) && !ty.is_open()) {
                        ty
                    } else {
                        crate::analyze::body::union_of(ty, Ty::Nil)
                    };
                    reseeded.insert(name, seeded);
                }
                let reseeded_ctx = Ctx {
                    self_ty: class_ctx.self_ty.clone(),
                    ivar_bindings: reseeded,
                    local_bindings: HashMap::new(),
                    class_objects: Default::default(),
                    constants: Default::default(), annotate_self_dispatch: false, in_view: false, class_side: false, claimed_macro_template: false, instance_body: false,
                };
                for method in &mut lc.methods {
                    let mctx = self.seed_method_params(&reseeded_ctx, &lc_name, method, true);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
            }
        }
        drop(_typing_library);

        // Hand the channel to the views half. Every production pass
        // refreshes it, so the pass that stamps templates reads the
        // seeds of the bodies as they converged — not of the bodies a
        // round that is still moving happened to leave behind.
        self.view_seeds = Some(ViewSeeds {
            action_ivars_by_view,
            layout_ivars_by_view,
            content_partial_ivars,
            mailer_params_by_view,
            view_feeders,
            controller_resolutions,
        });
    }

    /// The views half: templates, partials, the original test scopes
    /// and `db/seeds.rb`. Seeds come out of the channel the production
    /// half harvested rather than from a fresh walk of the controller
    /// bodies, which is the whole point — by the time templates are
    /// stamped those bodies have converged.
    fn type_views_and_tests(&mut self, app: &mut App, global_constants: &body::ConstScope) {
        let ViewSeeds {
            action_ivars_by_view,
            mut layout_ivars_by_view,
            content_partial_ivars,
            mailer_params_by_view,
            mut view_feeders,
            controller_resolutions,
        } = self.view_seeds.take().expect("view seeds after production");

        // Partial-locals channel: we need action/top-level views analyzed first
        // so their expression types are known at each `render` call site. We
        // then harvest the locals each render passes to the target partial,
        // keying by the partial's view name, and analyze partials with that
        // seed. Nested partial-of-partial isn't handled here (would need a
        // fixpoint); real-blog's dependency graph is shallow enough to skip.
        let mut partial_locals_by_name: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        // Renderer → partials-it-renders edges, harvested as views are
        // walked. Drives the ivar propagation below.
        let mut render_edges: HashMap<Symbol, Vec<Symbol>> = HashMap::new();

        let view_ctx_for = |name: &Symbol, ivars: HashMap<Symbol, Ty>| {
            let mut view_ctx = Ctx::default();
            view_ctx.in_view = true; // `yield` here renders to a String
            view_ctx.self_ty = Some(Ty::Class {
                id: ClassId(Symbol::from("ActionView::Base")),
                args: vec![],
            });
            view_ctx.constants = global_constants.clone();
            view_ctx.ivar_bindings = ivars;
            if let Some(row) = mailer_params_by_view.get(name) {
                view_ctx.local_bindings.insert(Symbol::from("params"), row.clone());
            }
            view_ctx
        };

        // Phase 3a: action / mailer templates first (not layouts). Rails
        // renders the template, then the layout, in one view context, so
        // `@section_class = "wide"` in the template is visible in the
        // layout. Harvest those writes after typing the template and fold
        // them into the layout seed before layouts are typed.
        let _typing_views = crate::timings::begin("typing: views");
        for view in &mut app.views {
            if is_partial_view_name(&view.name) || is_layout_view_name(&view.name) {
                continue;
            }
            let ivars = action_ivars_by_view
                .get(&view.name)
                .cloned()
                .unwrap_or_default();
            let view_ctx = view_ctx_for(&view.name, ivars);
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
            let mut assigned = HashMap::new();
            extract_ivar_assignments(&view.body, &mut assigned);
            if !assigned.is_empty() {
                if let Some(feeders) = view_feeders.get(&view.name) {
                    for feeder in feeders {
                        let Some(res) = controller_resolutions.get(feeder) else {
                            continue;
                        };
                        let Some(layout_name) = &res.layout else { continue };
                        let layout_map = layout_ivars_by_view.entry(layout_name.clone()).or_default();
                        for (k, v) in &assigned {
                            let noise = |t: &Ty| t.is_unknown();
                            let merged = match layout_map.remove(k) {
                                Some(prev) if noise(&prev) => v.clone(),
                                Some(prev) if noise(v) => prev,
                                Some(prev) if prev == *v => prev,
                                Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                                None => v.clone(),
                            };
                            layout_map.insert(k.clone(), merged);
                        }
                    }
                }
            }
            let mut targets = Vec::new();
            extract_partial_render_sites(
                &view.body,
                &view.name,
                &mut partial_locals_by_name,
                &mut targets,
            );
            record_render_edges(&mut render_edges, &view.name, targets);
        }

        // The ivar context each view carries: action views key by their
        // own name, layouts fall through to the layout-ivar union (now
        // including template-assigned ivars).
        let view_ivar_seed = |name: &Symbol| -> HashMap<Symbol, Ty> {
            action_ivars_by_view
                .get(name)
                .or_else(|| layout_ivars_by_view.get(name))
                .cloned()
                .unwrap_or_default()
        };

        for view in &mut app.views {
            if !is_layout_view_name(&view.name) {
                continue;
            }
            let view_ctx = view_ctx_for(&view.name, view_ivar_seed(&view.name));
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
            let mut targets = Vec::new();
            extract_partial_render_sites(
                &view.body,
                &view.name,
                &mut partial_locals_by_name,
                &mut targets,
            );
            record_render_edges(&mut render_edges, &view.name, targets);
        }

        // Harvest partial→partial render edges too (comment trees etc.).
        // Partials aren't typed yet, so collection-form renders (`render
        // @x`) won't resolve here — but the string/`partial:` forms that
        // nest in practice resolve by name without types. The throwaway
        // locals map is discarded; only the edges matter.
        for view in &app.views {
            if !is_partial_view_name(&view.name) {
                continue;
            }
            let mut throwaway = HashMap::new();
            let mut targets = Vec::new();
            extract_partial_render_sites(&view.body, &view.name, &mut throwaway, &mut targets);
            record_render_edges(&mut render_edges, &view.name, targets);
        }
        // A jbuilder template renders its partials through
        // `json.partial!` / `json.array!` and the `partial:` option, not
        // `render`, and its partials read its ivars all the same.
        for (renderer, targets) in crate::lower::jbuilder_to_library::jbuilder_render_edges(app) {
            record_render_edges(&mut render_edges, &renderer, targets);
        }

        // Propagate each renderer's ivar context onto the partials it
        // renders, to a fixpoint so nested partials (a partial rendering a
        // partial) inherit transitively. A renderer's own ivars are its
        // seed (non-partial) or its accumulated partial ivars. `Var` /
        // `Untyped` are dropped on merge — they carry no shape and only
        // pollute the union (mirrors the layout-ivar merge above).
        let mut partial_ivars_by_name: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
        // Seed the content partials (`render partial: @above`) up front
        // so the fixpoint below propagates their ivars into any further
        // partials they render, just like a statically-resolved edge.
        for (partial, ivars) in content_partial_ivars {
            partial_ivars_by_name.insert(partial, ivars);
        }
        let noise = |t: &Ty| t.is_unknown();
        // Depth cap guards against a render cycle (`_a` renders `_b`
        // renders `_a`); 16 is far beyond any real partial nesting.
        for _ in 0..16 {
            let mut changed = false;
            for (renderer, partials) in &render_edges {
                let renderer_ivars = if is_partial_view_name(renderer) {
                    partial_ivars_by_name.get(renderer).cloned().unwrap_or_default()
                } else {
                    view_ivar_seed(renderer)
                };
                if renderer_ivars.is_empty() {
                    continue;
                }
                for partial in partials {
                    let entry = partial_ivars_by_name.entry(partial.clone()).or_default();
                    for (k, v) in &renderer_ivars {
                        // Noise pollutes a UNION and is dropped there —
                        // but dropping it when the partial has no entry
                        // at all is worse than keeping it. A `Var` ivar
                        // is not a diagnostic in the renderer (an
                        // unknown type is not an error); ABSENT is,
                        // because the read then has no binding and
                        // reports `@room has no known type` in a partial
                        // whose renderer is perfectly happy with it.
                        // campfire's `rooms/show` seeds `@room` as Var,
                        // and its `_invitation` partial — which reads
                        // the ivar rather than the local it is handed —
                        // was three errors for exactly this.
                        if noise(v) && entry.contains_key(k) {
                            continue;
                        }
                        let merged = match entry.get(k) {
                            Some(prev) if noise(prev) => v.clone(),
                            Some(prev) if prev == v => prev.clone(),
                            Some(prev) => crate::analyze::body::union_of(prev.clone(), v.clone()),
                            None => v.clone(),
                        };
                        if entry.get(k) != Some(&merged) {
                            entry.insert(k.clone(), merged);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // Record the ivar context every view renders against — the
        // action's own seed for a view, the propagated union for a
        // partial. Both were just computed to type the bodies; persisting
        // them lets the LOWERER name a `form_with model: @ivar` after the
        // record's own model instead of the view directory. See
        // `App::view_ivar_types`.
        let view_ivar_types: HashMap<Symbol, HashMap<Symbol, Ty>> = app
            .views
            .iter()
            .map(|view| {
                let ivars = if is_partial_view_name(&view.name) {
                    partial_ivars_by_name.get(&view.name).cloned().unwrap_or_default()
                } else {
                    view_ivar_seed(&view.name)
                };
                (view.name.clone(), ivars)
            })
            .filter(|(_, ivars)| !ivars.is_empty())
            .collect();
        app.view_ivar_types = view_ivar_types;

        // Close `view_feeders` over the same renderer→partial edges: a
        // partial is fed by whoever feeds its renderers, transitively
        // (same depth-capped fixpoint shape as the ivar propagation
        // above). Runs after the ivar fixpoint so it sees the full edge
        // set; runs regardless of ivar emptiness because feeders matter
        // even when a renderer contributed no typed ivars.
        for _ in 0..16 {
            let mut changed = false;
            for (renderer, partials) in &render_edges {
                let Some(feeders) = view_feeders.get(renderer).cloned() else { continue };
                if feeders.is_empty() {
                    continue;
                }
                for partial in partials {
                    let entry = view_feeders.entry(partial.clone()).or_default();
                    let before = entry.len();
                    entry.extend(feeders.iter().cloned());
                    changed |= entry.len() != before;
                }
            }
            if !changed {
                break;
            }
        }
        app.view_feeders = view_feeders
            .into_iter()
            .map(|(view, feeders)| (view, feeders.into_iter().collect()))
            .collect();
        // Persist the raw renderer→partial edges too — the un-closed
        // half of the render graph, for view↔partial navigation.
        app.render_edges = render_edges.clone();
        app.controller_resolutions = controller_resolutions;

        // Phase 3b: partials. Seed local_bindings from the render-site map
        // and ivar_bindings from the propagated controller context, then
        // analyze.
        let attachable_partial_bindings = crate::lower::attachable::attachable_partial_bindings(app);
        for view in &mut app.views {
            if !is_partial_view_name(&view.name) {
                continue;
            }
            let mut view_ctx = Ctx::default();
            view_ctx.in_view = true; // `yield` here renders to a String
            // The view body types against the ActionView context, so
            // implicit-self helper calls (`form_with`, …) dispatch there.
            view_ctx.self_ty = Some(Ty::Class {
                id: ClassId(Symbol::from("ActionView::Base")),
                args: vec![],
            });
            view_ctx.constants = global_constants.clone();
            if let Some(locals) = partial_locals_by_name.get(&view.name) {
                view_ctx.local_bindings = locals.clone();
            }
            // Partials the FRAMEWORK renders, so no site in the app seeds
            // them: `action_text:install` copies `active_storage/blobs/
            // _blob.html.erb` into every app and Action Text renders it
            // for each attachment node. The local is named `blob` after
            // the attachable's partial path, but what Action Text passes
            // is the `ActionText::Attachment` wrapping it — the node's
            // `caption` lives there, everything else delegates to the
            // Blob (`delegate_missing_to :attachable`). The convention is
            // the render site.
            if view.name.as_str() == "active_storage/blobs/_blob" {
                view_ctx.local_bindings.entry(Symbol::from("blob")).or_insert(Ty::Class {
                    id: ClassId(Symbol::from("ActionText::Attachment")),
                    args: vec![],
                });
            }
            // The same framework render site for the app's OWN
            // attachables: `render(partial: to_attachable_partial_path,
            // as: model_name.element)`, so `users/_mention` reads `user`
            // and campfire's `_opengraph_embed` reads `opengraph_embed`.
            // The local is the record (Rails hands the Attachment, which
            // delegates to it; the emitted partial takes the record, and
            // the Attachment's own readers a partial uses — `caption` —
            // are synthesized onto the class as delegations the other
            // way, see `lower::attachable::attachable_partial_bindings`).
            for binding in &attachable_partial_bindings {
                if crate::lower::attachable::partial_view_name(&binding.partial) != view.name.as_str() {
                    continue;
                }
                view_ctx
                    .local_bindings
                    .entry(Symbol::from(binding.local.as_str()))
                    .or_insert(Ty::Class { id: binding.class.clone(), args: vec![] });
            }
            if let Some(ivars) = partial_ivars_by_name.get(&view.name) {
                view_ctx.ivar_bindings = ivars.clone();
            }
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
        }

        // Record the render-site local types on the App. They already
        // seeded each partial's body typing above; persisting them is
        // what lets the view LOWERER stamp the same fact into the
        // emitted signature instead of re-guessing a param's type from
        // its name. See `App::partial_local_types`.
        app.partial_local_types = partial_locals_by_name
            .iter()
            .map(|(view, locals)| (view.clone(), locals.clone()))
            .collect();
        drop(_typing_views);

        // Type the ORIGINAL test scopes, before source contracts inspect
        // them. Emission later clones/rewrites these bodies; it is too late
        // for that typing to establish a source admission fact.

        let _typing_tests = crate::timings::begin("typing: tests");
        self.type_test_modules(app, &global_constants);

        // Seeds body (db/seeds.rb). Top-level Ruby: no `self`, no
        // ivars, no before-action scaffolding. Just an expression
        // that references model classes. Types so that Send effects
        // flow (DbWrite on `Article.create!`, DbRead on
        // `Article.count`), which the emitter uses for await
        // placement under async adapters. Effects themselves are
        // stamped once after the typing fixpoint.
        if let Some(expr) = app.seeds.as_mut() {
            let mut ctx = Ctx::default();
            ctx.constants = global_constants.clone();
            self.body_typer().analyze_expr(expr, &ctx);
        }
        drop(_typing_tests);
    }


    /// Build a per-method `Ctx` by cloning `base` and seeding
    /// `local_bindings` with parameter types harvested from
    /// `inferred_params`. When no entry exists for the (class, method)
    /// pair, the params stay unbound and the body-typer falls back to
    /// `Ty::Var` for `Var { name }` reads — same as before any
    /// inference ran. Each fixpoint iteration that refines a param's
    /// type makes the next typing pass see a more concrete binding.
    /// The parameter type a SIGNATURE declares for this method.
    ///
    /// `sig` blocks and `sig/**/*.rbs` land in the same table, so this
    /// covers both. Matched by name before position: a signature names
    /// its parameters and so does the `MethodDef`, and for keyword
    /// arguments the two orders can differ.
    ///
    /// `positional` is the parameter's index among the method's
    /// POSITIONAL parameters, or `None` for a keyword or rest one. The
    /// position fallback counts only the declared positional parameters:
    /// the table is keyed by name alone, so a signature may belong to a
    /// different arity (a concern's instance method and the class method
    /// it contributes share a name), and reading a declared KEYWORD
    /// parameter as the def's first positional one typed `variants` as
    /// `bool`.
    ///
    /// Used only where inference has nothing better — see the call
    /// sites. A declaration is worth reading where inference runs out,
    /// which for a parameter is the common case: its type is a fact
    /// about the CALLERS, and a private helper nobody calls from a
    /// typed site has none.
    fn declared_param_ty(
        &self,
        class_id: &ClassId,
        method: &Symbol,
        positional: Option<usize>,
        name: &Symbol,
    ) -> Option<Ty> {
        let cls = self.classes.get(class_id)?;
        let ty = cls
            .instance_methods
            .get(method)
            .or_else(|| cls.class_methods.get(method))?;
        let Ty::Fn { params, .. } = ty else { return None };
        let found = params.iter().find(|p| p.name == *name).or_else(|| {
            let n = positional?;
            params
                .iter()
                .filter(|p| matches!(p.kind, crate::ty::ParamKind::Required | crate::ty::ParamKind::Optional))
                .nth(n)
        })?;
        (!matches!(found.ty, Ty::Var { .. } | Ty::Untyped)).then(|| found.ty.clone())
    }

    /// Whether a signature says this parameter is `untyped`, in as many
    /// words. That is a statement, unlike an unannotated parameter (whose
    /// slot is an inference gap): the author opted the parameter out of
    /// typing, and what the call sites happen to pass says nothing about
    /// what the body must accept. `#: (untyped, untyped) -> bool` on a
    /// comparator that is handed Money, Integers and Strings is the shape.
    fn declared_untyped_param(
        &self,
        class_id: &ClassId,
        method: &Symbol,
        positional: Option<usize>,
        name: &Symbol,
    ) -> bool {
        if !self.declared_signatures.contains(&(class_id.clone(), method.clone())) {
            return false;
        }
        let Some(cls) = self.classes.get(class_id) else { return false };
        let Some(Ty::Fn { params, .. }) =
            cls.instance_methods.get(method).or_else(|| cls.class_methods.get(method))
        else {
            return false;
        };
        params
            .iter()
            .find(|p| p.name == *name)
            .or_else(|| {
                let n = positional?;
                params
                    .iter()
                    .filter(|p| matches!(p.kind, crate::ty::ParamKind::Required | crate::ty::ParamKind::Optional))
                    .nth(n)
            })
            .is_some_and(|p| matches!(p.ty, Ty::Untyped))
    }

    /// `dsl_macro_host`: model and library-class concern hosts may declare
    /// first-class DSL macros (`has_markdown` on `ActionText::HasMarkdown`).
    /// Controllers and test modules do not — keep their diagnostics.
    fn seed_method_params(
        &self,
        base: &Ctx,
        class_id: &ClassId,
        method: &crate::dialect::MethodDef,
        dsl_macro_host: bool,
    ) -> Ctx {
        let key = (class_id.clone(), method.name.clone(), method.receiver);
        let observed = self.inferred_params.get(&key);
        let mut ctx = base.clone();
        ctx.class_side = matches!(method.receiver, crate::dialect::MethodReceiver::Class);
        ctx.instance_body = !ctx.class_side;
        // Class-method bodies named after a first-class model DSL
        // (`has_markdown`, `has_rich_text`, …) are macro templates —
        // association/`scope` leftovers inside them are claimed at
        // call sites by the dedicated lowerer, not typed as live
        // ActiveRecord::Base sends. Library-class concerns are hosts
        // too (`ActionText::HasMarkdown`); controllers are not.
        ctx.claimed_macro_template = dsl_macro_host
            && ctx.class_side
            && matches!(
                method.name.as_str(),
                "generates_token_for"
                    | "has_one_attached"
                    | "has_many_attached"
                    | "has_rich_text"
                    | "has_markdown"
                    | "has_secure_token"
                    | "has_secure_password"
                    | "has_json"
                    | "typed_store"
                    | "broadcasts_to"
            );
        let mut positional_seen = 0usize;
        for (i, param) in method.params.iter().enumerate() {
            let is_positional = !param.keyword && !param.rest && !param.from_keyword;
            let positional = is_positional.then_some(positional_seen);
            positional_seen += usize::from(is_positional);
            if self.declared_untyped_param(class_id, &method.name, positional, &param.name) {
                ctx.local_bindings.insert(param.name.clone(), Ty::Untyped);
                continue;
            }
            let from_sites = observed.and_then(|v| v.get(i)).cloned();
            let seeded = param_ty_with_default(from_sites, param);
            let ty = prefer_declared(
                self.declared_param_ty(class_id, &method.name, positional, &param.name),
                seeded,
            );
            if let Some(ty) = ty {
                ctx.local_bindings.insert(param.name.clone(), ty);
            }
        }
        // An indexed module's Ruby callback gets its host as a Module object,
        // even when the hook is dormant and no explicit call site supplies types.
        if ctx.class_side && self.classes.get(class_id).is_some_and(|c| c.is_module)
            && matches!(method.name.as_str(), "included" | "prepended" | "append_features" | "prepend_features")
        {
            if let Some(param) = method.params.first().filter(|p| !p.keyword && !p.rest) {
                ctx.class_objects.insert(param.name.clone());
            }
        }
        if let Some(bp) = &method.block_param {
            ctx.local_bindings.insert(bp.name.clone(), captured_block_ty());
        }
        ctx
    }

    /// As `seed_method_params`, but for a controller `Action` — whose
    /// params are a `Row` (ordered name→Ty map) rather than a
    /// `MethodDef`. Controller helper methods (`period(query)`,
    /// `paginate(rel)`) take real Ruby params whose types are only
    /// known from their call sites; without seeding them the body
    /// types every param read as `Var`, so the method's return type
    /// (`query.where(...)`) never resolves. Routed actions have empty
    /// param rows, so this is a no-op for them.
    fn seed_action_params(
        &self,
        base: &Ctx,
        class_id: &ClassId,
        origin: Option<&ClassId>,
        action_name: &Symbol,
        params: &Row,
        kw_params: &[(Symbol, Option<crate::expr::Expr>)],
        block_param: Option<&Symbol>,
    ) -> Ctx {
        let own = self.inferred_params.get(&(class_id.clone(), action_name.clone(), MethodReceiver::Instance));
        let from_origin = origin
            .and_then(|m| self.inferred_params.get(&(m.clone(), action_name.clone(), MethodReceiver::Instance)));
        let mut ctx = base.clone();
        ctx.instance_body = true;
        for (i, name) in params.fields.keys().enumerate() {
            if self.declared_untyped_param(class_id, action_name, Some(i), name) {
                ctx.local_bindings.insert(name.clone(), Ty::Untyped);
                continue;
            }
            let observed = [own, from_origin]
                .into_iter()
                .flatten()
                .filter_map(|v| v.get(i).cloned())
                .filter(|t| !matches!(t, Ty::Var { .. }))
                .reduce(unify_param_ty);
            let ty = prefer_declared(
                self.declared_param_ty(class_id, action_name, Some(i), name),
                observed,
            );
            if let Some(ty) = ty {
                ctx.local_bindings.insert(name.clone(), ty);
            }
        }
        // Keyword params sit beside the positional row rather than in
        // it, and a call site passes them by NAME — so there is no
        // index to read an observation from. The declaration is the
        // only source, which is the case the signature readers exist
        // for.
        for (name, _) in kw_params.iter() {
            if ctx.local_bindings.contains_key(name) {
                continue;
            }
            if let Some(ty) =
                self.declared_param_ty(class_id, action_name, None, name)
            {
                ctx.local_bindings.insert(name.clone(), ty);
            }
        }
        if let Some(bp) = block_param {
            ctx.local_bindings.insert(bp.clone(), captured_block_ty());
        }
        ctx
    }

    fn capture_inference_sig(&self) -> InferenceSig {
        let mut instance = HashMap::with_capacity(self.classes.len());
        let mut class_methods = HashMap::with_capacity(self.classes.len());
        for (id, cls) in &self.classes {
            instance.insert(id.clone(), cls.instance_methods.clone());
            class_methods.insert(id.clone(), cls.class_methods.clone());
        }
        InferenceSig {
            instance,
            class_methods,
            params: self.inferred_params.clone(),
        }
    }

    fn capture_dirty_hints(&self) -> DirtyHints {
        let mut block_value = HashMap::with_capacity(self.classes.len());
        for (id, cls) in &self.classes {
            if !cls.block_value_methods.is_empty() {
                block_value.insert(id.clone(), cls.block_value_methods.clone());
            }
        }
        DirtyHints {
            sig: self.capture_inference_sig(),
            block_value,
        }
    }

    fn inference_matches(&self, prev: &InferenceSig) -> bool {
        if self.classes.len() != prev.instance.len() || self.inferred_params != prev.params {
            return false;
        }
        for (id, cls) in &self.classes {
            match prev.instance.get(id) {
                Some(methods) if methods == &cls.instance_methods => {}
                _ => return false,
            }
            match prev.class_methods.get(id) {
                Some(methods) if methods == &cls.class_methods => {}
                _ => return false,
            }
        }
        true
    }

    /// Block-value verdicts stay on [`DirtyHints`], not [`InferenceSig`],
    /// so convergence fingerprinting cannot accidentally absorb them.
    /// The fixpoint still waits on them: a missing prior entry matches
    /// only an empty current set.
    fn block_value_matches(&self, prev: &DirtyHints) -> bool {
        for (id, cls) in &self.classes {
            match prev.block_value.get(id) {
                Some(methods) if methods == &cls.block_value_methods => {}
                None if cls.block_value_methods.is_empty() => {}
                _ => return false,
            }
        }
        for id in prev.block_value.keys() {
            if !self.classes.contains_key(id) {
                return false;
            }
        }
        true
    }

    /// The classes a retype round actually has to walk: `None` for
    /// "walk everything", `Some(set)` for the change frontier the
    /// harvest just produced, closed over everything that reads it.
    ///
    /// What makes the narrowing sound is that the body typer is a
    /// function of (body, registry, seeds). A class whose own registry
    /// entry and param row did not move, and which calls nothing that
    /// moved, retypes to exactly what it already says — so the pass
    /// can skip its `analyze_expr` and still harvest the same types
    /// out of its trees. The frontier is therefore closed over:
    ///
    /// * **inheritance and inclusion**, because a class answers its
    ///   parent's and its modules' methods as its own (only the module
    ///   side is folded into the registry) and a subclass controller
    ///   SEEDS its actions from its ancestors' harvested bindings; and
    /// * **the reverse call graph**, because a caller reads a callee's
    ///   return. One hop of readers is enough: the registry only moves
    ///   at harvest time, so a reader's own callers see no new answer
    ///   until the next round harvests this one.
    ///
    /// Campfire's rounds move 191 → 89 → 44 → 29 → 13 → 2 classes, and
    /// the tail is the Opengraph/Current/Nokogiri chain — a full retype
    /// per link.
    fn dirty_classes_for_retype(
        &self,
        app: &App,
        prev: &DirtyHints,
    ) -> Option<std::collections::HashSet<ClassId>> {
        dirty_classes_for_retype(
            app,
            &self.classes,
            &self.inferred_params,
            &self.callers_by_target,
            prev,
            |id, parent| self.lexical_parent(id, parent),
        )
    }

    /// Record which classes call `(receiver class, method)`, for the
    /// slice of sites one method body contributed. Keyed by the class
    /// the receiver typed to rather than by the class whose `def` the
    /// call reaches, because that is the key the walk in
    /// `dirty_classes_for_retype` can resolve: dispatch chases the
    /// receiver's own chain, and so does the frontier.
    ///
    /// View and seed trees walked under [`UnifyScope::WithViews`] still
    /// contribute param observations, but their sends have no owning
    /// production [`ClassId`] and are intentionally not passed here —
    /// absorb dirty narrowing is production-class only today.
    ///
    /// A class's calls to ITSELF are dropped: when its own answer
    /// moves it is already dirty, and implicit-self sends are most of
    /// what a body contains.
    fn record_callers(
        &mut self,
        caller: &ClassId,
        sites: &[SendSite],
    ) {
        let mut seen: std::collections::HashSet<(&ClassId, &Symbol)> =
            std::collections::HashSet::new();
        for (class_id, method, _, _, _) in sites {
            if class_id == caller || !seen.insert((class_id, method)) {
                continue;
            }
            self.callers_by_target
                .entry((class_id.clone(), method.clone()))
                .or_default()
                .insert(caller.clone());
        }
    }

    /// Walk every model + library_class method body and write its
    /// inferred body type into `self.classes[class].instance_methods`
    /// (or `class_methods` for `def self.x`). Conservative on widening:
    /// only updates the registry when the harvested type is more
    /// specific than what's already there (concrete > Ty::Var; existing
    /// RBS-derived `Ty::Fn` is preserved — its return is already what
    /// dispatch resolves to via `unwrap_fn_ret`). Skip methods whose
    /// body is `Ty::Var` (no information gained).
    fn harvest_returns_to_registry(&mut self, app: &App, harvest_tests: bool) {
        self.harvest_method_returns(app, harvest_tests);
        // Rails' `helper_method :name` makes a controller (or concern)
        // method callable from templates. The names were ingested from
        // both spellings (`App::view_visible_controller_methods`); the
        // TYPES are the methods' harvested returns, copied onto the view
        // context each round so a template's `authenticated?` resolves
        // — the authentication generator's shape, in every Rails 8 app.
        let view_ctx = ClassId(Symbol::from("ActionView::Base"));
        for name in &app.view_visible_controller_methods {
            let owners = app
                .controllers
                .iter()
                .map(|c| &c.name)
                .chain(app.library_classes.iter().map(|lc| &lc.name));
            let ty = owners
                .filter_map(|cid| self.classes.get(cid))
                .find_map(|c| c.instance_methods.get(name).cloned());
            if let Some(ty) = ty {
                self.classes.entry(view_ctx.clone()).or_default().instance_methods.insert(name.clone(), ty);
            }
        }
    }

    fn harvest_one_model(&mut self, model: &Model) {
        let class_id = &model.name;
        let scope_names: std::collections::HashSet<Symbol> =
            model.scopes().map(|s| s.name.clone()).collect();
        for method in model.methods() {
            let ret = self.method_return_ty(class_id, method);
            let target = match method.receiver {
                crate::dialect::MethodReceiver::Instance => {
                    &mut self.classes.entry(class_id.clone()).or_default().instance_methods
                }
                crate::dialect::MethodReceiver::Class => {
                    &mut self.classes.entry(class_id.clone()).or_default().class_methods
                }
            };
            // A class method whose body tail is a query-builder
            // chain declares `Relation { of: Self }` to callers —
            // the same relation type a scope seeds — overriding
            // the body's `Array<Self>` typing (inside the body the
            // chain keeps the inline-chain Array representation;
            // the relation type is introduced at the boundary).
            // This is what lets `Story.recent.for_user(u)`
            // delegate `for_user` on the relation receiver.
            if method.receiver == crate::dialect::MethodReceiver::Class
                && body_tail_yields_relation(&method.body, class_id, &scope_names)
            {
                Self::insert_inferred_return(
                    target,
                    &method.name,
                    Ty::Relation { of: class_id.clone() },
                );
                self.classes
                    .entry(class_id.clone())
                    .or_default()
                    .relation_derived
                    .insert(method.name.clone());
                continue;
            }
            // …and the half that TERMINATES the chain rather than
            // extending it: `def self.original; order(:created_at)
            // .first; end` returns a record, not a relation, but it
            // is just as much a query over this model and Rails
            // delegates it on a relation receiver the same way.
            // Marked `relation_derived` so that delegation can tell
            // it apart from a class method that merely happens to
            // return a record.
            if method.receiver == crate::dialect::MethodReceiver::Class {
                if let Some(kind) =
                    body_tail_terminal_kind(&method.body, class_id, &scope_names)
                {
                    Self::insert_inferred_return(
                        target,
                        &method.name,
                        instantiate_return_kind(kind, class_id),
                    );
                    self.classes
                        .entry(class_id.clone())
                        .or_default()
                        .relation_derived
                        .insert(method.name.clone());
                    continue;
                }
            }
            Self::register_method_return(target, &method.name, ret.as_ref());
        }
    }

    fn harvest_method_returns(&mut self, app: &App, harvest_tests: bool) {
        for model in &app.models {
            self.harvest_one_model(model);
        }
        for lc in &app.library_classes {
            let class_id = &lc.name;
            for method in &lc.methods {
                let ret = self.method_return_ty(class_id, method);
                let target = match method.receiver {
                    crate::dialect::MethodReceiver::Instance => {
                        &mut self.classes.entry(class_id.clone()).or_default().instance_methods
                    }
                    crate::dialect::MethodReceiver::Class => {
                        &mut self.classes.entry(class_id.clone()).or_default().class_methods
                    }
                };
                // Register method existence even when the body can't be
                // typed: these classes are now ingested (their `def`s are
                // real), so a call resolves to the inferred return or to
                // Untyped (gradual) rather than "no known method". Unlike an
                // unregistered class, this doesn't mask a typo — the method
                // has to be defined in the file to land here.
                Self::register_method_return(target, &method.name, ret.as_ref());
            }
        }
        if harvest_tests {
            for module in &app.test_modules {
                for method in &module.helpers {
                    let ret = self.method_return_ty(&module.name, method);
                    let class = self.classes.entry(module.name.clone()).or_default();
                    let table = match method.receiver {
                        crate::dialect::MethodReceiver::Instance => &mut class.instance_methods,
                        crate::dialect::MethodReceiver::Class => &mut class.class_methods,
                    };
                    Self::register_method_return(table, &method.name, ret.as_ref());
                }
            }
        }
        // Controllers: harvest each action/helper method's return type so a
        // sibling call (`@story = find_story`) resolves. Conservative like
        // library classes — only concrete (non-Var) bodies are registered;
        // an untypeable helper stays unresolved rather than masking to
        // Untyped. All controller methods are instance methods (Action has
        // no class-receiver variant).
        for controller in &app.controllers {
            let class_id = &controller.name;
            // Class-side methods register like a library class's: the
            // method exists whether or not its body can be typed, so a
            // call to it resolves to the inferred return or to Untyped
            // rather than "no known method".
            for method in controller.class_methods() {
                let ret = self.method_return_ty(class_id, method);
                let target = &mut self.classes.entry(class_id.clone()).or_default().class_methods;
                Self::register_method_return(target, &method.name, ret.as_ref());
            }
            for action in controller.actions() {
                let Some(body_ty) =
                    tuple_return_ty(&action.body).or_else(|| effective_return_ty(&action.body))
                else {
                    continue;
                };
                if matches!(body_ty, Ty::Var { .. }) {
                    continue;
                }
                let target =
                    &mut self.classes.entry(class_id.clone()).or_default().instance_methods;
                Self::insert_inferred_return(target, &action.name, body_ty);
            }
        }

        self.harvest_block_value_methods(app);
        self.fold_concern_surfaces(app);
        self.fold_host_surfaces(app);
        self.fold_extended_modules(app);
        self.fold_current_attribute_forwarders(app);
    }

    /// Rebuild every class's `block_value_methods` from the bodies as
    /// this round typed them. Derived state, so rebuilt rather than
    /// accumulated (as `inferred_params` is, for the same reason); a
    /// method forwarding its block to a sibling that is one becomes one
    /// a round later, as its callee's verdict lands.
    fn harvest_block_value_methods(&mut self, app: &App) {
        let mut found: Vec<(ClassId, Symbol)> = Vec::new();
        for model in &app.models {
            for method in model.methods() {
                let bp = method.block_param.as_ref().map(|p| &p.name);
                if self.returns_block_value(&model.name, &method.body, bp) {
                    found.push((model.name.clone(), method.name.clone()));
                }
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                let bp = method.block_param.as_ref().map(|p| &p.name);
                if self.returns_block_value(&lc.name, &method.body, bp) {
                    found.push((lc.name.clone(), method.name.clone()));
                }
            }
        }
        for controller in &app.controllers {
            for action in controller.actions() {
                if self.returns_block_value(&controller.name, &action.body, action.block_param.as_ref()) {
                    found.push((controller.name.clone(), action.name.clone()));
                }
            }
        }
        for info in self.classes.values_mut() {
            info.block_value_methods.clear();
        }
        for (class_id, method) in found {
            self.classes.entry(class_id).or_default().block_value_methods.insert(method);
        }
    }

    /// A `CurrentAttributes` class-level forwarder answers exactly what
    /// its instance twin answers, because `ingest::current_attributes`
    /// wrote it that way (`def self.user; Current.instance.user; end`).
    /// Copy the answer across rather than letting the forwarder's body
    /// be typed, because that body cannot type: `Ty::Class { Current }`
    /// serves as both the class object and an instance in this type
    /// system, and the class-method table is consulted first — so
    /// `Current.instance.user` looks up the FORWARDER, finds the entry
    /// it is itself in the middle of computing, and settles on
    /// `Untyped`. Every class-level read app code writes went through
    /// one of these, so campfire's whole per-request surface
    /// (`Current.user.rooms`, and every ivar downstream) was gradual
    /// while the instance side had the shapes all along.
    ///
    /// Runs at the end of each fixpoint round, after the harvest that
    /// would otherwise overwrite it, so the next round types the
    /// forwarder's callers against the real answer.
    fn fold_current_attribute_forwarders(&mut self, app: &App) {
        for id in &app.current_attribute_classes {
            let Some(cls) = self.classes.get(id) else { continue };
            // Only names that exist on BOTH sides: `instance` and
            // `reset` have no instance twin and are typed from their own
            // bodies, and a hand-written `def self.x` that the synthesis
            // skipped is not a forwarder either.
            // Not setters: the instance writer returns its body's tail
            // (campfire's `session=` ends in `self.user = …`, a User?)
            // while the forwarder `def self.session=(value);
            // Current.instance.session = value; end` returns the
            // assignment's value — the Session. Its own body types that
            // correctly; copying the instance answer over it declared
            // `-> User?` on a function spinel compiled to return the
            // Session, and the C did not link.
            let copied: Vec<(Symbol, Ty)> = cls
                .class_methods
                .keys()
                .filter(|name| !is_setter_name(name))
                .filter_map(|name| {
                    cls.instance_methods.get(name).map(|ty| (name.clone(), ty.clone()))
                })
                .collect();
            let Some(cls) = self.classes.get_mut(id) else { continue };
            for (name, ty) in copied {
                cls.class_methods.insert(name, ty);
            }
        }
    }

    /// Concern fold: `include SomeConcern` makes the module's instance
    /// methods — and, via ActiveSupport::Concern's `class_methods do`,
    /// its class-side defs — callable on the includer
    /// (`Account.find_local!`). Copy both surfaces onto each includer,
    /// chasing module→module includes transitively. Runs at the end of
    /// every harvest so each fixpoint round's refinement of the module's
    /// returns propagates; `concern_folded` remembers which keys the
    /// fold wrote so refinements overwrite prior *copies* but never the
    /// includer's own or catalog entries. Folding into the registry
    /// (rather than chasing includes at dispatch time) means every
    /// consumer — dispatch, `ide::members_of`, completion — sees the
    /// mixed-in surface identically.

    /// `extend Mod` in a class body: Mod's INSTANCE methods become the
    /// class's singleton methods (`extend TrackCurrent` gives `User.current`,
    /// `User.with_current`). Folded the way an `include`d module's surface
    /// is, onto the class-side table, with the class's own methods winning.
    /// `extend self` is the module-function idiom, handled at ingest.
    fn fold_extended_modules(&mut self, app: &App) {
        let mut wanted: Vec<(ClassId, Vec<ClassId>)> = Vec::new();
        let extends_of = |exprs: &mut dyn Iterator<Item = &Expr>, owner: &ClassId| {
            let mut out: Vec<ClassId> = Vec::new();
            for expr in exprs {
                let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
                if method.as_str() != "extend" {
                    continue;
                }
                for arg in args {
                    let ExprNode::Const { path } = &*arg.node else { continue };
                    let written = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                    // Lexical resolution: the enclosing scopes outward.
                    let mut scope: Vec<&str> = owner.0.as_str().split("::").collect();
                    let mut resolved = None;
                    loop {
                        let candidate = if scope.is_empty() {
                            written.clone()
                        } else {
                            format!("{}::{written}", scope.join("::"))
                        };
                        if self.classes.contains_key(&ClassId(Symbol::from(candidate.as_str()))) {
                            resolved = Some(ClassId(Symbol::from(candidate.as_str())));
                            break;
                        }
                        if scope.is_empty() {
                            break;
                        }
                        scope.pop();
                    }
                    if let Some(id) = resolved {
                        out.push(id);
                    }
                }
            }
            out
        };
        for model in &app.models {
            let mut items = model.body.iter().filter_map(|item| match item {
                ModelBodyItem::Unknown { expr, .. } => Some(expr),
                _ => None,
            });
            let ext = extends_of(&mut items, &model.name);
            if !ext.is_empty() {
                wanted.push((model.name.clone(), ext));
            }
        }
        for lc in &app.library_classes {
            let ext = extends_of(&mut lc.unknown_calls.iter(), &lc.name);
            if !ext.is_empty() {
                wanted.push((lc.name.clone(), ext));
            }
        }
        let module_ids: std::collections::HashSet<&ClassId> =
            app.library_classes.iter().filter(|lc| lc.is_module).map(|lc| &lc.name).collect();
        for (id, modules) in wanted {
            let mut queue = modules;
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if !module_ids.contains(&m) {
                    continue;
                }
                let Some(module) = self.classes.get(&m) else { continue };
                let inst = module.instance_methods.clone();
                for n in module.includes.clone() {
                    if seen.insert(n.clone()) {
                        queue.push(n);
                    }
                }
                let folded = self.concern_folded.entry(id.clone()).or_default();
                let cls = self.classes.entry(id.clone()).or_default();
                for (name, ty) in inst {
                    if cls.class_methods.contains_key(&name) && !folded.1.contains(&name) {
                        continue;
                    }
                    cls.class_methods.insert(name.clone(), ty);
                    folded.1.insert(name);
                }
            }
        }
    }

    fn fold_concern_surfaces(&mut self, app: &App) {
        type Surface = (HashMap<Symbol, Ty>, HashMap<Symbol, Ty>, Vec<ClassId>);
        let module_surfaces: HashMap<ClassId, Surface> = app
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .filter_map(|lc| {
                let cls = self.classes.get(&lc.name)?;
                // What the host fold lent the module is the includers'
                // own surface; copying it back down would only hand an
                // includer a stale answer about itself.
                let mut inst = cls.instance_methods.clone();
                if let Some(lent) = self.host_folded.get(&lc.name) {
                    inst.retain(|name, _| !lent.contains(name));
                }
                Some((lc.name.clone(), (inst, cls.class_methods.clone(), cls.includes.clone())))
            })
            .collect();
        if module_surfaces.is_empty() {
            return;
        }

        let targets: Vec<(ClassId, Vec<ClassId>)> = self
            .classes
            .iter()
            .filter(|(_, c)| !c.includes.is_empty())
            .map(|(id, c)| (id.clone(), c.includes.clone()))
            .collect();
        for (id, includes) in targets {
            // Transitive closure over module includes.
            let mut queue = includes;
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                let Some((inst, class_side, nested)) = module_surfaces.get(&m) else {
                    continue;
                };
                for n in nested {
                    if seen.insert(n.clone()) {
                        queue.push(n.clone());
                    }
                }
                let folded = self.concern_folded.entry(id.clone()).or_default();
                let cls = self.classes.entry(id.clone()).or_default();
                for (name, ty) in inst {
                    if cls.instance_methods.contains_key(name) && !folded.0.contains(name) {
                        continue; // own/catalog entry wins
                    }
                    cls.instance_methods.insert(name.clone(), ty.clone());
                    folded.0.insert(name.clone());
                }
                for (name, ty) in class_side {
                    if cls.class_methods.contains_key(name) && !folded.1.contains(name) {
                        continue;
                    }
                    cls.class_methods.insert(name.clone(), ty.clone());
                    folded.1.insert(name.clone());
                }
            }
        }
    }

    /// Host fold: the other direction of `fold_concern_surfaces`. A
    /// concern's methods run on its includer, so a bare call in one
    /// (`resume_session` in `Impersonation`) may name a method the
    /// module never defines — the includer does, or a sibling concern
    /// the includer also mixes in. With ONE includer the module's
    /// bodies are typed with that class as `self` and the call resolves
    /// there. With several, `self` stays the module (a union `self` is
    /// a poly cliff on every send) and the call fell to `untyped`,
    /// which then spread: Rails' authentication concern split across
    /// two modules left `current_user` as `User | untyped` in every
    /// controller that read it.
    ///
    /// So lend the module the answer when there is exactly one: a name
    /// the module's bodies call on implicit `self`, that the module
    /// does not define, and that EVERY includer resolves to the same
    /// type. Includers that disagree, or one that lacks the method,
    /// leave the call as it was. Runs after the concern fold, so an
    /// includer's surface already carries its other concerns' methods.
    fn fold_host_surfaces(&mut self, app: &App) {
        let modules: BTreeSet<&ClassId> =
            app.library_classes.iter().filter(|lc| lc.is_module).map(|lc| &lc.name).collect();
        if modules.is_empty() {
            return;
        }
        // Module → the non-module classes that include it, transitively.
        let mut hosts: HashMap<ClassId, BTreeSet<ClassId>> = HashMap::new();
        for (id, cls) in &self.classes {
            if modules.contains(id) || cls.includes.is_empty() {
                continue;
            }
            let mut queue = cls.includes.clone();
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if modules.contains(&m) {
                    hosts.entry(m.clone()).or_default().insert(id.clone());
                }
                for n in self.classes.get(&m).map(|c| c.includes.as_slice()).unwrap_or_default() {
                    if seen.insert(n.clone()) {
                        queue.push(n.clone());
                    }
                }
            }
        }

        fn bare_calls(e: &Expr, out: &mut BTreeSet<Symbol>) {
            if let ExprNode::Send { recv: None, method, .. } = &*e.node {
                out.insert(method.clone());
            }
            e.node.for_each_child(&mut |c| bare_calls(c, out));
        }

        for lc in app.library_classes.iter().filter(|lc| lc.is_module) {
            let lent_before = self.host_folded.remove(&lc.name).unwrap_or_default();
            let mut lent = BTreeSet::new();
            let mut agreed: Vec<(Symbol, Ty)> = Vec::new();
            // A sole includer is `self` in the module's bodies already.
            if let Some(hosts) = hosts.get(&lc.name).filter(|h| h.len() > 1) {
                let mut called = BTreeSet::new();
                for method in &lc.methods {
                    bare_calls(&method.body, &mut called);
                }
                let own = self.classes.get(&lc.name);
                for name in called {
                    if own.is_some_and(|c| c.instance_methods.contains_key(&name))
                        && !lent_before.contains(&name)
                    {
                        continue; // the module's own answer
                    }
                    let mut answers = hosts.iter().map(|h| {
                        self.classes.get(h).and_then(|c| c.instance_methods.get(&name))
                    });
                    let Some(Some(first)) = answers.next() else { continue };
                    if first.is_unknown() || !answers.all(|a| a == Some(first)) {
                        continue;
                    }
                    agreed.push((name, first.clone()));
                }
            }
            let cls = self.classes.entry(lc.name.clone()).or_default();
            for name in &lent_before {
                cls.instance_methods.remove(name);
            }
            for (name, ty) in agreed {
                cls.instance_methods.insert(name.clone(), ty);
                lent.insert(name);
            }
            if !lent.is_empty() {
                self.host_folded.insert(lc.name.clone(), lent);
            }
        }
    }

    /// Conservative insertion: don't overwrite a `Ty::Fn` (RBS-sourced
    /// signature whose return is what dispatch already returns). Don't
    /// overwrite a more-concrete type with `Ty::Var`. Otherwise replace
    /// or insert. This is the join rule that keeps RBS-declared
    /// signatures authoritative while letting inference fill the rest.
    /// Register a model method's return type. A resolved body type is
    /// authoritative. When the body couldn't be typed (`Var`/`None`) after
    /// analysis, register the method's *existence* as `Untyped` (a gradual
    /// escape) so calls to it resolve — turning a dispatch error into a
    /// gradual warning rather than a hard "no known method". A real type
    /// found by any pass is never clobbered by the fallback.
    /// What `method`'s body returns — its tail. A setter is no
    /// exception: `def session=(value)` RETURNS its tail (campfire's
    /// ends in `self.user = session.user`, a User?) even though the
    /// call `x.session = v` EVALUATES to `v`; the body typer answers
    /// the call-site fact, the harvest the function's, and a compiled
    /// target (spinel) returns what the function returns.
    fn method_return_ty(&self, _class_id: &ClassId, method: &crate::dialect::MethodDef) -> Option<Ty> {
        // A declared signature (ingest-expanded DSL, RBS, …) wins over
        // body harvest: `delegated_type`'s `def page; leafable if page?;
        // end` body types as the full polymorphic union, but the useful
        // call-site answer is `Page | nil`.
        if let Some(Ty::Fn { ret, .. }) = &method.signature {
            return Some((**ret).clone());
        }
        self.class_object_return_ty(&method.body)
            .or_else(|| tuple_return_ty(&method.body))
            .or_else(|| effective_return_ty(&method.body))
    }

    /// A method whose every return value is a class constant returns
    /// the CLASS, not an instance of it: lobsters'
    /// `Search#searched_model` is `what == :stories ? Story : Comment`.
    /// The body typer gives a class constant the same `Ty::Class { C }`
    /// an instance has (deliberately, so `Story.where` dispatches), and
    /// harvesting that as the return declared `-> (Comment | Story)` in
    /// the emitted RBS — a lie spinel trusted, which hid a
    /// `searched_model.none` NoMethodError (#132). The boundary is where
    /// the distinction is recoverable, so it is drawn here, the way
    /// `Relation` is: `Class[C]`, which RBS spells `singleton(C)` and
    /// `dispatch` reads back as `C` for callers.
    ///
    /// All or nothing: a tail mixing a class with any other value, or a
    /// `return` off the tail, keeps the ordinary harvest.
    fn class_object_return_ty(&self, body: &Expr) -> Option<Ty> {
        fn tails<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>, returns: &mut usize) {
            match &*e.node {
                ExprNode::If { then_branch, else_branch, .. } => {
                    tails(then_branch, out, returns);
                    tails(else_branch, out, returns);
                }
                ExprNode::Case { arms, .. } => {
                    arms.iter().for_each(|a| tails(&a.body, out, returns))
                }
                ExprNode::Seq { exprs } if !exprs.is_empty() => {
                    tails(exprs.last().unwrap(), out, returns)
                }
                ExprNode::Return { value } => {
                    *returns += 1;
                    tails(value, out, returns);
                }
                // A raising arm returns nothing.
                ExprNode::Raise { .. } => {}
                _ => out.push(e),
            }
        }
        fn count_returns(e: &Expr) -> usize {
            let mut n = usize::from(matches!(&*e.node, ExprNode::Return { .. }));
            e.node.for_each_child(&mut |c| n += count_returns(c));
            n
        }
        let mut out = Vec::new();
        let mut tail_returns = 0;
        tails(body, &mut out, &mut tail_returns);
        // A `return` off the tail is a value this walk did not see.
        if out.is_empty() || count_returns(body) != tail_returns {
            return None;
        }
        let mut classes: Vec<Ty> = Vec::new();
        for t in out {
            let ExprNode::Const { path } = &*t.node else { return None };
            let Some(Ty::Class { id, args }) = &t.ty else { return None };
            // The constant must NAME the class it is typed as. A value
            // constant (`DEFAULT = Foo.new`) is typed by its value,
            // whose class is not its own name.
            let written = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            let named = id.0.as_str() == written
                || id.0.as_str().ends_with(&format!("::{written}"));
            if !args.is_empty() || !named || !self.classes.contains_key(id) {
                return None;
            }
            let singleton = Ty::Class {
                id: ClassId(Symbol::from("Class")),
                args: vec![Ty::Class { id: id.clone(), args: vec![] }],
            };
            if !classes.contains(&singleton) {
                classes.push(singleton);
            }
        }
        Some(match classes.len() {
            1 => classes.pop().unwrap(),
            _ => Ty::Union { variants: classes },
        })
    }

    /// True when every value `body` can return is the value of the
    /// block the method was called with: `yield`, or a call handing
    /// `block_param` on to a method that itself returns its block's
    /// value (see `ClassInfo::block_value_methods`). A raising arm
    /// returns nothing and does not count against it; a `return` off
    /// the tail must pass the same test, or the walk declines.
    fn returns_block_value(&self, owner: &ClassId, body: &Expr, block_param: Option<&Symbol>) -> bool {
        let leaves = return_leaves(body);
        !leaves.is_empty()
            && leaves.iter().all(|leaf| match &*leaf.node {
                ExprNode::Yield { .. } => true,
                ExprNode::Send { recv, method, block: Some(b), .. } => {
                    let forwards = matches!(
                        (&*b.node, block_param),
                        (ExprNode::Var { name, .. }, Some(bp)) if name == bp
                    );
                    // The callee's own verdict, asked through the same
                    // dispatch rule a call site uses, with a stand-in
                    // informative block type.
                    let recv_ty = match recv {
                        Some(r) => r.ty.clone(),
                        None => Some(Ty::Class { id: owner.clone(), args: vec![] }),
                    };
                    // `Rails.cache` by its spelling too: app analysis
                    // does not type it (the runtime's RBS is not in the
                    // app registry), and `fetch` on the framework's
                    // store answers the block's value, fresh or cached.
                    let rails_cache = method.as_str() == "fetch"
                        && recv.as_ref().is_some_and(|r| crate::lower::rails_cache::is_rails_cache(r));
                    forwards
                        && (rails_cache
                            || crate::analyze::body::BodyTyper::new(&self.classes)
                                .block_value_return(recv_ty.as_ref(), method, Some(&Ty::Nil))
                                .is_some())
                }
                _ => false,
            })
    }

    fn register_method_return(
        table: &mut HashMap<Symbol, Ty>,
        method: &Symbol,
        body_ty: Option<&Ty>,
    ) {
        match body_ty {
            Some(t) if !matches!(t, Ty::Var { .. }) => {
                Self::insert_inferred_return(table, method, t.clone());
            }
            _ => {
                if !matches!(table.get(method), Some(t) if !matches!(t, Ty::Var { .. })) {
                    table.insert(method.clone(), Ty::Untyped);
                }
            }
        }
    }

    fn insert_inferred_return(
        table: &mut HashMap<Symbol, Ty>,
        method: &Symbol,
        ty: Ty,
    ) {
        harvest_return::insert_inferred_return(table, method, ty);
    }

    /// Walk every Send across the app, look up each call's target
    /// method, and unify the argument types into
    /// `self.inferred_params` for that (class, method). Mirrors
    /// Spinel's `detect_poly_params` (`spinel_codegen.rb:6928-7052`)
    /// at a higher level — we work with structured `Ty` values rather
    /// than string fingerprints, so unification is direct: same type →
    /// keep; nil + T → T?; otherwise → union widen.
    fn unify_params_from_call_sites(&mut self, app: &App, scope: UnifyScope) {
        // Rebuilt from scratch every fixpoint round. The table is pure
        // derived state — a function of the types the last typing pass
        // wrote onto the call-site argument expressions — and carrying
        // it forward makes it MONOTONIC in the wrong direction: round 1
        // sees `self.id = generate_id` before `generate_id` is known
        // and records `Untyped`; round 2 sees `Str`; unify fuses them
        // into `Str | Untyped`, which is `untyped` with extra steps.
        // The refined round strictly dominates the one before it, so
        // the earlier observation has nothing to contribute.
        self.inferred_params.clear();
        let helpers = &app.helper_method_index;
        // A CALLEE'S KEYWORD PARAMETER IS NOT ITS KWARGS HASH. The
        // fourth slot carries a call's trailing `kwargs: true` entries
        // by NAME so they can be placed on the keyword params they
        // actually bind to; without it the whole hash lands in the
        // position the hash occupies, and a `def f(room,
        // involvement:)` called as `f(room, involvement: str)` is
        // declared `involvement: Hash[Symbol, String?]` — a second
        // description of the argument list that disagrees with the
        // `def`. campfire's `next_involvement_for` emitted exactly
        // that, and spinel gave the param an `sp_SymPolyHash *`.
        let mut sites: Vec<SendSite> = Vec::new();
        let params_by_method = Self::param_shapes(app);
        let defined = Self::defined_methods(app);
        // Class-level reverse call graph, rebuilt alongside the param
        // table off the same walk — each `collect_send_sites` call
        // already knows the class it is walking, so the edges are the
        // slice of sites it appended. See `record_callers`.
        self.callers_by_target.clear();
        for model in &app.models {
            for method in model.methods() {
                let from = sites.len();
                self.collect_send_sites(&method.body, Some(&model.name), method.receiver == MethodReceiver::Class, helpers, &mut sites);
                self.record_callers(&model.name, &sites[from..]);
            }
            for scope_item in model.scopes() {
                let from = sites.len();
                self.collect_send_sites(&scope_item.body, Some(&model.name), true, helpers, &mut sites);
                self.record_callers(&model.name, &sites[from..]);
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                let from = sites.len();
                self.collect_send_sites(&method.body, Some(&lc.name), method.receiver == MethodReceiver::Class, helpers, &mut sites);
                self.record_callers(&lc.name, &sites[from..]);
            }
        }
        for controller in &app.controllers {
            for action in controller.actions() {
                let from = sites.len();
                self.collect_send_sites(&action.body, Some(&controller.name), false, helpers, &mut sites);
                self.record_callers(&controller.name, &sites[from..]);
            }
            for method in controller.class_methods() {
                let from = sites.len();
                self.collect_send_sites(&method.body, Some(&controller.name), true, helpers, &mut sites);
                self.record_callers(&controller.name, &sites[from..]);
            }
            // A class-body macro call (`preload_site_configs %w[a], only:
            // :show`) is the call site that types the class method.
            for item in &controller.body {
                if let crate::dialect::ControllerBodyItem::ClassIvarInit { expr, .. } = item {
                    let from = sites.len();
                    self.collect_send_sites(expr, Some(&controller.name), true, helpers, &mut sites);
                    self.record_callers(&controller.name, &sites[from..]);
                }
            }
        }
        // WithViews: param sites only — no `record_callers` (no ClassId).
        if matches!(scope, UnifyScope::WithViews) {
            for view in &app.views {
                self.collect_send_sites(&view.body, None, false, helpers, &mut sites);
            }
            if let Some(seeds) = &app.seeds {
                self.collect_send_sites(seeds, None, false, helpers, &mut sites);
            }
        }

        self.apply_param_sites(sites, &params_by_method, &defined);
        self.apply_current_attribute_write_sites(app);
        // Production signatures keep their production callers' shape.
        // Fold before adding test-owned observations: a same-named test
        // helper must not feed an included production concern either.
        self.fold_concern_param_sites(app);
    }

    /// Attribute writers synthesized for CurrentAttributes have their
    /// only useful parameter observations at class-level assignment
    /// sites (`Current.request = request`), not ordinary method calls
    /// discoverable by `collect_send_sites`. Feed those observed RHS
    /// types into the same parameter table so the generated instance
    /// writer and its class-level forwarder retain one typed contract.
    fn apply_current_attribute_write_sites(&mut self, app: &App) {
        let targets: std::collections::HashSet<&ClassId> =
            app.current_attribute_classes.iter().collect();
        if targets.is_empty() {
            return;
        }
        let mut writes = HashMap::new();
        let mut collect = |body: &crate::expr::Expr| {
            collect_const_attr_writes(body, &targets, &mut writes);
        };
        crate::lower::for_each_hook_body_ref(app, &mut collect);
        for view in &app.views {
            collect(&view.body);
        }

        for (class, attrs) in writes {
            for (attr, ty) in attrs {
                // Both sides: the instance writer and its class-level
                // forwarder share the observed contract.
                for side in [MethodReceiver::Instance, MethodReceiver::Class] {
                    let key = (class.clone(), Symbol::from(format!("{attr}=")), side);
                    let entry = self.inferred_params.entry(key).or_default();
                    if entry.is_empty() {
                        entry.push(ty.clone());
                    } else {
                        entry[0] =
                            fixpoint_bound::bound(unify_param_ty(entry[0].clone(), ty.clone()));
                    }
                }
            }
        }
    }

    /// Replay production+view param observations, then overlay typed
    /// test call sites. Test-only retype rounds do not walk view trees
    /// again — those bodies did not change.
    fn unify_test_params_onto(
        &mut self,
        app: &App,
        snapshot: &HashMap<ParamKey, Vec<Ty>>,
    ) {
        self.inferred_params.clone_from(snapshot);
        self.overlay_test_params(app);
    }

    fn overlay_test_params(&mut self, app: &App) {
        let helpers = &app.helper_method_index;
        let params_by_method = Self::param_shapes(app);
        let defined = Self::defined_methods(app);
        let test_sites = self.collect_test_param_sites(app, helpers);
        self.apply_param_sites(test_sites, &params_by_method, &defined);
    }

    fn collect_test_param_sites(
        &self,
        app: &App,
        helpers: &HashMap<Symbol, ClassId>,
    ) -> Vec<SendSite> {
        let mut test_sites = Vec::new();
        for module in &app.test_modules {
            if let Some(setup) = &module.setup {
                self.collect_send_sites(setup, Some(&module.name), false, helpers, &mut test_sites);
            }
            for body in module.helpers.iter().flat_map(|method| {
                method.params.iter().filter_map(|param| param.default.as_ref())
                    .chain(std::iter::once(&method.body))
            })
                .chain(module.tests.iter().map(|test| &test.body)) {
                self.collect_send_sites(body, Some(&module.name), false, helpers, &mut test_sites);
            }
        }
        test_sites.into_iter().filter_map(|(class, method, args, kwargs, recv)| {
            self.test_helper_owner(app, &class, &method)
                .map(|owner| (owner, method, args, kwargs, recv))
        }).collect()
    }

    fn apply_param_sites(
        &mut self,
        sites: Vec<SendSite>,
        params_by_method: &HashMap<ParamKey, ParamShape>,
        defined: &BTreeSet<ParamKey>,
    ) {
        for (class_id, method, arg_tys, kw_tys, recv) in sites {
            let want = if recv == SiteRecv::Instance { MethodReceiver::Instance } else { MethodReceiver::Class };
            let on_side = self.inherited_param_owner(defined, class_id.clone(), &method, Some(want));
            // Not the nearest `def` of either side: `Child.fetch` reaches `Grand.fetch` past a `Base#fetch`.
            let class_id = if defined.contains(&(on_side.clone(), method.clone(), want)) {
                on_side
            } else {
                self.inherited_param_owner(defined, class_id, &method, None)
            };
            let Some(side) = Self::param_side(defined, &class_id, &method, recv) else { continue };
            let key = (class_id, method, side);
            let arg_tys = Self::place_keyword_args(params_by_method.get(&key), arg_tys, kw_tys);
            let arity = arg_tys.len();
            let entry = self
                .inferred_params
                .entry(key)
                .or_insert_with(|| (0..arity).map(|_| Ty::Var { var: crate::ident::TyVar(0) }).collect());
            if entry.len() < arity {
                entry.resize(arity, Ty::Var { var: crate::ident::TyVar(0) });
            }
            for (slot, observed) in entry.iter_mut().zip(arg_tys.into_iter()) {
                *slot = fixpoint_bound::bound(unify_param_ty(slot.clone(), observed));
            }
        }
    }

    /// Every `(class, method)` the app defines, by name.
    fn defined_methods(app: &App) -> BTreeSet<ParamKey> {
        let mut defined: BTreeSet<ParamKey> = BTreeSet::new();
        for lc in &app.library_classes {
            for m in &lc.methods {
                defined.insert((lc.name.clone(), m.name.clone(), m.receiver));
            }
        }
        for model in &app.models {
            for m in model.methods() {
                defined.insert((model.name.clone(), m.name.clone(), m.receiver));
            }
        }
        for c in &app.controllers {
            for a in c.actions() {
                defined.insert((c.name.clone(), a.name.clone(), MethodReceiver::Instance));
            }
            // A subclass's class-body macro call reaches the class method
            // on the controller that defines it.
            for m in c.class_methods() {
                defined.insert((c.name.clone(), m.name.clone(), MethodReceiver::Class));
            }
        }
        defined
    }

    // Not the receiver's side alone: `UserMailer.welcome(user)` runs an instance `def`, and `self.class.get` beside only `def get` is a gem's (HTTParty's).
    fn param_side(defined: &BTreeSet<ParamKey>, owner: &ClassId, method: &Symbol, recv: SiteRecv) -> Option<MethodReceiver> {
        let has = |side| defined.contains(&(owner.clone(), method.clone(), side));
        match (has(MethodReceiver::Instance), has(MethodReceiver::Class)) {
            (true, true) if recv == SiteRecv::Instance => Some(MethodReceiver::Instance),
            (_, true) => Some(MethodReceiver::Class),
            (true, false) if recv == SiteRecv::DotClass => None,
            _ => Some(MethodReceiver::Instance),
        }
    }

    /// The class whose `def` a call keyed to `class` reaches.
    ///
    /// A receiverless call is keyed to the class it is written in, and
    /// the `def` may sit on an ancestor: a base controller defines
    /// `sign_in_and_render(user)` and only its subclasses call it, so
    /// the observation matched no `def` and the parameter stayed `Var`.
    ///
    /// Walks Ruby's lookup order: the class, the modules it includes,
    /// then its parent. A class that defines the method keeps the site.
    /// A site that reaches an included module first stays where it is —
    /// `fold_concern_param_sites` owns that case — as does a chain with
    /// no definer.
    fn inherited_param_owner(
        &self,
        defined: &BTreeSet<ParamKey>,
        class: ClassId,
        method: &Symbol,
        side: Option<MethodReceiver>,
    ) -> ClassId {
        let defines = |c: &ClassId| {
            [MethodReceiver::Instance, MethodReceiver::Class]
                .into_iter()
                .filter(|s| side.is_none_or(|want| want == *s))
                .any(|s| defined.contains(&(c.clone(), method.clone(), s)))
        };
        let mut cur = class.clone();
        for _ in 0..32 {
            if defines(&cur) {
                return cur;
            }
            let Some(info) = self.classes.get(&cur) else { break };
            let mut modules: Vec<&ClassId> = info.includes.iter().collect();
            let mut seen: BTreeSet<&ClassId> = BTreeSet::new();
            while let Some(module) = modules.pop() {
                if !seen.insert(module) {
                    continue;
                }
                if defines(module) {
                    return class;
                }
                if let Some(m) = self.classes.get(module) {
                    modules.extend(m.includes.iter());
                }
            }
            let Some(parent) = &info.parent else { break };
            cur = self.lexical_parent(&cur, parent);
        }
        class
    }

    /// Parents are recorded as written, so `module Api; class
    /// AuthsController < BaseController` records `BaseController` for
    /// the class the registry keys `Api::BaseController`. Qualify a
    /// single-segment parent against the child's enclosing namespaces,
    /// innermost first — Ruby's lexical rule.
    fn lexical_parent(&self, child: &ClassId, parent: &ClassId) -> ClassId {
        if self.classes.contains_key(parent) || parent.0.as_str().contains("::") {
            return parent.clone();
        }
        let mut segs: Vec<&str> = child.0.as_str().split("::").collect();
        segs.pop();
        while !segs.is_empty() {
            let candidate =
                ClassId(Symbol::from(format!("{}::{}", segs.join("::"), parent.0.as_str()).as_str()));
            if self.classes.contains_key(&candidate) {
                return candidate;
            }
            segs.pop();
        }
        parent.clone()
    }

    /// The param-table twin of `fold_concern_surfaces`.
    ///
    /// `User.create_bot!(bot_params.to_attrs)` records its argument
    /// under `(User, create_bot!)` — the receiver — while the `def`
    /// lives in the concern, `(User::Bot, create_bot!)`. Nothing joined
    /// the two, so campfire emitted `user.rbs` declaring
    /// `(Hash[Symbol, untyped] attributes)` and `bot.rbs` declaring
    /// `(untyped attributes)` for THE SAME METHOD — and the sidecar
    /// beside the body is the one spinel compiles, so `attributes` was
    /// poly, `attributes.merge(…)` dispatched over every `merge` in the
    /// program, and `ActionDispatch::Flash#merge`'s `sp_StrStrHash *`
    /// took a `sp_SymPolyHash *`.
    ///
    /// Copies the includer's observations onto the defining module,
    /// chasing module→module includes the same way the return-type fold
    /// does. **A method the includer defines ITSELF is skipped**: then
    /// the call sites are evidence about that def, and the module's
    /// same-named method is a different one Ruby never reaches.
    fn fold_concern_param_sites(&mut self, app: &App) {
        let mut module_methods: HashMap<ClassId, BTreeSet<Symbol>> = HashMap::new();
        let mut owned: BTreeSet<(ClassId, Symbol)> = BTreeSet::new();
        for lc in &app.library_classes {
            let names: BTreeSet<Symbol> = lc.methods.iter().map(|m| m.name.clone()).collect();
            for n in &names {
                owned.insert((lc.name.clone(), n.clone()));
            }
            if lc.is_module {
                module_methods.insert(lc.name.clone(), names);
            }
        }
        if module_methods.is_empty() {
            return;
        }
        for model in &app.models {
            for m in model.methods() {
                owned.insert((model.name.clone(), m.name.clone()));
            }
        }

        // A class reaches a module through its own `include`s AND its
        // ancestors' — `SessionsController` calls
        // `start_new_session_for(user)`, defined in `Authentication`,
        // which `ApplicationController` includes. Without the parent
        // walk the site was keyed to the subclass, matched nothing, and
        // the concern's `user` parameter stayed unresolved (the
        // authentication generator's shape, in every Rails 8 app).
        let targets: Vec<(ClassId, Vec<ClassId>)> = self
            .classes
            .iter()
            .map(|(id, _)| {
                let mut includes: Vec<ClassId> = Vec::new();
                let mut cur = Some(id.clone());
                let mut depth = 0;
                while let Some(cid) = cur {
                    let Some(c) = self.classes.get(&cid) else { break };
                    for inc in &c.includes {
                        if !includes.contains(inc) {
                            includes.push(inc.clone());
                        }
                    }
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    cur = c.parent.clone();
                }
                (id.clone(), includes)
            })
            .filter(|(_, includes)| !includes.is_empty())
            .collect();
        // `self.classes` is a HashMap: without a fixed order the
        // includers' observations reach the module's slot in a
        // different order every run, and a union's variant order (or
        // any order-sensitive join) leaks into the emitted signature.
        let mut targets = targets;
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        let mut adds: Vec<(ParamKey, Vec<Ty>)> = Vec::new();
        for (id, includes) in targets {
            let mut queue = includes;
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if let Some(ci) = self.classes.get(&m) {
                    for n in &ci.includes {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
                let Some(names) = module_methods.get(&m) else { continue };
                for name in names {
                    // The includer's OWN def wins — unless it is this
                    // module's def, spliced in verbatim
                    // (`splice_concern_class_methods_into_includers`).
                    // Then it is one method with two `MethodDef`s and
                    // the observations belong to both.
                    let spliced_from_here = app
                        .concern_spliced_class_methods
                        .get(&id)
                        .and_then(|per| per.get(name))
                        == Some(&m);
                    if !spliced_from_here && owned.contains(&(id.clone(), name.clone())) {
                        continue;
                    }
                    for side in [MethodReceiver::Instance, MethodReceiver::Class] {
                        if let Some(tys) = self.inferred_params.get(&(id.clone(), name.clone(), side)) {
                            adds.push(((m.clone(), name.clone(), side), tys.clone()));
                        }
                    }
                }
            }
        }
        for (key, tys) in adds {
            let entry = self.inferred_params.entry(key).or_default();
            if entry.len() < tys.len() {
                entry.resize(tys.len(), Ty::Var { var: crate::ident::TyVar(0) });
            }
            for (slot, observed) in entry.iter_mut().zip(tys.into_iter()) {
                *slot = fixpoint_bound::bound(unify_param_ty(slot.clone(), observed));
            }
        }
    }

    /// Every (class, method) pair's parameter names and canonical kinds
    /// — the shape `place_keyword_args` needs to put a
    /// call's kwargs on the right slots.
    ///
    /// Keyed the way `inferred_params` is. Two `def`s of one name on
    /// one side (a reopened class) whose shapes differ POISON the entry
    /// rather than pick one: placing keywords from the wrong `def` is
    /// worse than leaving the hash where it sits, which is the
    /// behaviour that stood before this table existed.
    fn param_shapes(app: &App) -> HashMap<ParamKey, ParamShape> {
        let mut out: HashMap<ParamKey, Option<ParamShape>> = HashMap::new();
        let mut record = |class: &ClassId, m: &crate::dialect::MethodDef| {
            let shape = ParamShape {
                slots: m.params.iter().map(|p| (p.name.clone(), p.ty_kind())).collect(),
                keywords_by_kind: false,
            };
            out.entry((class.clone(), m.name.clone(), m.receiver))
                .and_modify(|slot| {
                    if slot.as_ref() != Some(&shape) {
                        *slot = None;
                    }
                })
                .or_insert(Some(shape));
        };
        for model in &app.models {
            for m in model.methods() {
                record(&model.name, m);
            }
        }
        for lc in &app.library_classes {
            for m in &lc.methods {
                record(&lc.name, m);
            }
        }
        for module in &app.test_modules {
            for method in &module.helpers {
                record(&module.name, method);
            }
        }

        // A controller helper's keywords are call-site evidence too:
        // without its shape, `describe(name: "gear", count: 2)` against
        // `def describe(name:, count:)` typed the first slot with the
        // whole kwargs Hash. Same slot order the controller lowering
        // builds: positionals, optionals, keywords, `**rest`.
        for controller in &app.controllers {
            // Copied Concern class methods keep their source keywords
            // (`ingest::class_attribute`), so a key binds by kind too.
            for m in controller.class_methods() {
                let shape = ParamShape {
                    slots: m.params.iter().map(|p| (p.name.clone(), p.ty_kind())).collect(),
                    keywords_by_kind: true,
                };
                out.entry((controller.name.clone(), m.name.clone(), MethodReceiver::Class))
                    .and_modify(|slot| {
                        if slot.as_ref() != Some(&shape) {
                            *slot = None;
                        }
                    })
                    .or_insert(Some(shape));
            }
            for a in controller.actions() {
                let mut shape: Vec<(Symbol, ParamKind)> =
                    a.params.fields.iter().map(|(n, _)| (n.clone(), ParamKind::Required)).collect();
                shape.extend(a.opt_params.iter().map(|(n, _)| (n.clone(), ParamKind::Optional)));
                shape.extend(
                    a.kw_params.iter().map(|(n, d)| (n.clone(), ParamKind::Keyword { required: d.is_none() })),
                );
                if let Some(n) = &a.kwrest_param {
                    shape.push((n.clone(), ParamKind::KeywordRest));
                }
                // Controller lowering keeps every keyword a keyword, so a
                // key binds by kind here, never to a same-named positional.
                let shape = ParamShape { slots: shape, keywords_by_kind: true };
                out.entry((controller.name.clone(), a.name.clone(), MethodReceiver::Instance))
                    .and_modify(|slot| {
                        if slot.as_ref() != Some(&shape) {
                            *slot = None;
                        }
                    })
                    .or_insert(Some(shape));
            }
        }
        out.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()
    }

    /// Place a call's trailing keyword arguments on the parameter slots
    /// they bind to, replacing the single slot the kwargs hash occupies.
    ///
    /// The gate is that EVERY key names a declared parameter — keyword
    /// OR positional. Positional too because `ingest::library_class`
    /// lowers a helper's optional keyword to a positional-with-default
    /// (`room_display_name(room, for_user = Current.user)`) and
    /// `lower::helper_kwargs` moves the call sites to match only AFTER
    /// analysis; in between, a keyword-matching rule bound the whole
    /// `{for_user: nil}` hash to the positional slot and the parameter
    /// was typed `Hash[Symbol, nil]`, which made `room.users.without(
    /// for_user)` a hash condition and the helper's chain untyped. A
    /// `**attributes`-style helper — `link_to_room(room, id: …,
    /// class: …)` binding one Hash to a positional `attributes` — has
    /// no parameter named `id` or `class`, so it is untouched by
    /// construction; this is the same names-not-types rule
    /// `lower::helper_kwargs` applies to the call sites themselves.
    ///
    /// Keywords the site omits keep their `Var` slot: a call that does
    /// not pass an optional keyword is no evidence about its type, and
    /// `unify_param_ty` reads `Var` as exactly that.
    fn place_keyword_args(
        shape: Option<&ParamShape>,
        mut arg_tys: Vec<Ty>,
        kw: SiteKeywords,
    ) -> Vec<Ty> {
        if let Some(shape) = shape.filter(|s| s.keywords_by_kind || kw.anonymous_forward) {
            if kw.group {
                if let Some(placed) =
                    Self::bind_keyword_group(shape, &arg_tys, &kw.keys, kw.splat.as_ref())
                {
                    return placed;
                }
            }
        }
        let kw_tys = kw.keys;
        if kw_tys.is_empty() {
            // Ingest erases ** forwarding to the Hash expression itself.
            // A whole bundle cannot prove an individual named keyword's
            // value. Mask only those slots, without moving observations:
            // keyword-rest DOES bind the Hash, and positional/default/rest
            // parameters keep their existing inference contract.
            if let Some(shape) = shape {
                for (observed, (_, kind)) in arg_tys.iter_mut().zip(&shape.slots) {
                    if matches!(kind, ParamKind::Keyword { .. }) {
                        *observed = Ty::Var { var: crate::ident::TyVar(0) };
                    }
                }
            }
            return arg_tys;
        }
        let Some(shape) = shape else {
            return arg_tys;
        };
        let params = &shape.slots;
        // Where a shape keeps its keywords as keywords (a controller
        // helper's), Ruby binds a key only to a keyword slot:
        // `h("text", a: 2)` against `def h(a, **opts)` is `a = "text"`,
        // `opts = {a: 2}`. Positional slots still count for the index.
        let slot_of = |key: &Symbol| {
            params.iter().position(|(n, kind)| {
                n == key && (!shape.keywords_by_kind || matches!(kind, ParamKind::Keyword { .. }))
            })
        };
        // A key no parameter names still leaves the hash alone, unless
        // the slot it would land on is a named keyword: Ruby never binds
        // a kwargs hash to one, so `tagged(label: x, tone: y)` against
        // `def tagged(label:, **rest)` types `label` from `x` and leaves
        // `tone` to the `**rest` it binds to. `**rest` itself is not
        // such a slot: when the hash lands there (`h(1, x: 2)` against
        // `def h(a, **opts)`), Ruby binds `opts` to exactly that hash,
        // so the observation stays where it is. Every slot from the
        // hash's own position on counts, not just that one: with no
        // positional passed, `h(label: 1, tone: 2)` against `def h(prefix
        // = nil, label:, **rest)` binds `label`, never `prefix`.
        let supplied_positionals = arg_tys.len().saturating_sub(1);
        let lands_on_keyword = params
            .iter()
            .skip(supplied_positionals)
            .any(|(_, kind)| matches!(kind, ParamKind::Keyword { .. }));
        if !lands_on_keyword && !kw_tys.iter().all(|(k, _)| slot_of(k).is_some()) {
            return arg_tys;
        }
        arg_tys.pop();
        if arg_tys.len() < params.len() {
            arg_tys.resize(params.len(), Ty::Var { var: crate::ident::TyVar(0) });
        }
        for (k, t) in kw_tys {
            if let Some(i) = slot_of(&k) {
                arg_tys[i] = t;
            }
        }
        arg_tys
    }

    /// A controller helper's call whose last argument is its keyword
    /// group, placed the way Ruby binds it: the positionals fill the
    /// positional slots in order, a key fills the keyword slot that
    /// names it, and the group as a whole never fills a positional.
    /// `spread(1, x: 2)` against `def spread(a, b = nil, **rest)` is
    /// `b = nil`, `rest = {x: 2}`; `forwarded(**opts)` against `def
    /// forwarded(prefix = nil, label:, **rest)` leaves `prefix` at its
    /// default. The Hash goes to `**rest` only when no named keyword
    /// can take part of it, so `**rest` binds exactly that hash, as in
    /// `h(1, x: 2)` against `def h(a, **opts)`; a `**splat`'s unknown
    /// keys leave every named keyword `Var`.
    ///
    /// `None` when the shape takes no keywords at all: Ruby then passes
    /// the group as one positional Hash (`def p(a, b = nil)` called
    /// `p(1, x: 2)` is `b = {x: 2}`), the rule the caller already has.
    fn bind_keyword_group(
        shape: &ParamShape,
        arg_tys: &[Ty],
        keys: &[(Symbol, Ty)],
        splat: Option<&Ty>,
    ) -> Option<Vec<Ty>> {
        let params = &shape.slots;
        let is_named = |kind: &ParamKind| matches!(kind, ParamKind::Keyword { .. });
        let has_named = params.iter().any(|(_, kind)| is_named(kind));
        let rest = params
            .iter()
            .position(|(_, kind)| matches!(kind, ParamKind::KeywordRest));
        if rest.is_none() && !has_named {
            return None;
        }
        let (hash, positionals) = arg_tys.split_last()?;
        let unseen = Ty::Var {
            var: crate::ident::TyVar(0),
        };
        let mut out = vec![unseen; params.len()];
        let positional_slots = params
            .iter()
            .enumerate()
            .filter(|(_, (_, kind))| matches!(kind, ParamKind::Required | ParamKind::Optional))
            .map(|(i, _)| i);
        for (i, t) in positional_slots.zip(positionals) {
            out[i] = t.clone();
        }
        // A keyword the literal does not name may still come from the
        // splat's Hash; when the key is absent it keeps its default,
        // which `param_ty_with_default` joins in.
        if let Some(v) = splat {
            for (i, (_, kind)) in params.iter().enumerate() {
                if is_named(kind) {
                    out[i] = v.clone();
                }
            }
        }
        // The splat merges over the literal, so a named key may still
        // take the splat's value.
        for (key, t) in keys {
            if let Some(i) = params
                .iter()
                .position(|(n, kind)| n == key && is_named(kind))
            {
                out[i] = match splat {
                    Some(v) => crate::analyze::body::union_of(t.clone(), v.clone()),
                    None => t.clone(),
                };
            }
        }
        if let (Some(i), false) = (rest, has_named) {
            out[i] = hash.clone();
        }
        Some(out)
    }

    /// Walk one expression tree, collecting (class_id, method, arg_tys)
    /// for every Send whose receiver type is known. Used by
    /// `unify_params_from_call_sites`. The receiver's type was set by
    /// the most recent typing pass, so call sites whose receivers
    /// resolve to a class flow their args back here; bare-name Sends
    /// against implicit-self use the enclosing class.
    fn collect_send_sites(
        &self,
        expr: &Expr,
        self_class: Option<&ClassId>,
        class_body: bool,
        helpers: &HashMap<Symbol, ClassId>,
        out: &mut Vec<SendSite>,
    ) {
        match &*expr.node {
            ExprNode::Send { recv, method, args, block, .. } => {
                // Resolve the receiver class: an explicit receiver
                // typed as a class, or — for an implicit-self call
                // (`period(query)` inside a controller) — the
                // enclosing class. The latter is what lets a sibling
                // method's params be inferred from its self-call sites.
                // A bare send with NO enclosing class is a template
                // body (`errors_for(comment)` in a view): Rails mixes
                // every app helper into every view, so the helper
                // index is the dispatch — attributing the site to the
                // defining helper module is what lets helper params
                // unify from their template call sites.
                let mut via_helper_index = false;
                // Expand Relation / Union / Array elem the way dispatch
                // chases — the reverse call graph now decides retype,
                // so Class-only receivers would leave callers of
                // `records.first.foo` off the frontier.
                let recv_classes: Vec<ClassId> = match recv {
                    Some(r) => r
                        .ty
                        .as_ref()
                        .map(class_ids_for_call_receiver)
                        .unwrap_or_default(),
                    None => self_class
                        .cloned()
                        .or_else(|| {
                            via_helper_index = true;
                            helpers.get(method).cloned()
                        })
                        .into_iter()
                        .collect(),
                };
                let site_recv = match recv.as_ref().map(|r| &*r.node) {
                    None | Some(ExprNode::SelfRef) if class_body => SiteRecv::Class,
                    Some(ExprNode::Const { .. }) => SiteRecv::Class,
                    Some(ExprNode::Send { method: m, args, .. }) if m.as_str() == "class" && args.is_empty() => {
                        SiteRecv::DotClass
                    }
                    _ => SiteRecv::Instance,
                };
                if !recv_classes.is_empty() {
                    let arg_tys: Vec<Ty> = args
                        .iter()
                        .map(|a| {
                            let t = a.ty.clone().unwrap_or(Ty::Var { var: crate::ident::TyVar(0) });
                            // Template call sites are evidence of what
                            // callers pass; an `Untyped` arg there
                            // (`errors_for f.object`) means the CALLER's
                            // type is unknown — no evidence — not that
                            // the param accepts anything. Recording it
                            // as Untyped would absorb the whole union
                            // (gradual absorption in dispatch) and erase
                            // the typed sites' information. Scoped to
                            // the helper channel; explicit-receiver and
                            // own-class channels keep their semantics.
                            if via_helper_index && matches!(t, Ty::Untyped) {
                                Ty::Var { var: crate::ident::TyVar(0) }
                            } else {
                                t
                            }
                        })
                        .collect();
                    // The trailing `kwargs: true` hash, harvested by
                    // NAME. Only literal-symbol keys: a `**splat` or a
                    // computed key says nothing about which parameter
                    // the value binds to, and one such key disqualifies
                    // the whole hash (an incomplete map would place
                    // some keywords and silently drop the rest).
                    let keys = match args.last().map(|a| &*a.node) {
                        Some(ExprNode::Hash { entries, kwargs: true }) => {
                            let mut pairs = Vec::with_capacity(entries.len());
                            let mut all_sym = true;
                            for (k, v) in entries {
                                match &*k.node {
                                    ExprNode::Lit { value: Literal::Sym { value } } => {
                                        let t = v
                                            .ty
                                            .clone()
                                            .unwrap_or(Ty::Var { var: crate::ident::TyVar(0) });
                                        let t = if via_helper_index && matches!(t, Ty::Untyped) {
                                            Ty::Var { var: crate::ident::TyVar(0) }
                                        } else {
                                            t
                                        };
                                        pairs.push((value.clone(), t));
                                    }
                                    _ => {
                                        all_sym = false;
                                        break;
                                    }
                                }
                            }
                            if all_sym { pairs } else { Vec::new() }
                        }
                        // The anonymous packet is merged after these
                        // pairs, so it can overwrite every explicit key.
                        // Keep the group as unknown keyword evidence below
                        // instead of inferring the explicit values as final.
                        _ => Vec::new(),
                    };
                    // Whether the last argument is the call's keyword
                    // group at all: a `k: v` list or one with a
                    // `**splat`, never a positional `{…}` literal.
                    let group = matches!(
                        args.last().map(|a| &*a.node),
                        Some(ExprNode::Hash { kwargs: true, .. }
                            | ExprNode::KeywordSplat { .. }
                            | ExprNode::ForwardKeywords
                            | ExprNode::ForwardKeywordsWithPairs { .. })
                    );
                    // The splat merges over the literal, so each literal
                    // key may take the splat's value too.
                    let (keys, splat) = match args.last().map(|a| &*a.node) {
                        Some(ExprNode::KeywordSplat { value }) => keyword_splat(value)
                            .map_or((keys, None), |(pairs, v)| {
                                let joined = pairs
                                    .into_iter()
                                    .map(|(k, t)| (k, union_of(t, v.clone())))
                                    .collect();
                                (joined, Some(v))
                            }),
                        Some(ExprNode::ForwardKeywordsWithPairs { .. }) => {
                            // The opaque forwarded packet merges after the
                            // literal entries and can override each key.
                            // Its values are unavailable, so the signature
                            // binder leaves named parameters uninferred.
                            (Vec::new(), None)
                        }
                        _ => (keys, None),
                    };
                    let anonymous_forward = matches!(
                        args.last().map(|a| &*a.node),
                        Some(ExprNode::ForwardKeywords | ExprNode::ForwardKeywordsWithPairs { .. })
                    );
                    let kw_tys = SiteKeywords { group, keys, splat, anonymous_forward };
                    // `Klass.new(a, b)` hands its arguments to
                    // `initialize` — that is all `Class#new` does with
                    // them — so the site is evidence for the
                    // constructor's params too. Recorded under `new`
                    // alone, `initialize` was seeded with nothing and
                    // every `@x = x` it made came out untyped: lobsters'
                    // `StoriesPaginator.new(scope, …)` passed a typed
                    // `Relation[Story]` and the paginator's `@scope`
                    // still answered `Array[untyped]` to every caller.
                    // Only a CONSTANT receiver: an instance answering
                    // `new` is some other method entirely.
                    let record_initialize = method.as_str() == "new"
                        && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. }));
                    for class_id in recv_classes {
                        if record_initialize {
                            out.push((
                                class_id.clone(),
                                Symbol::from("initialize"),
                                arg_tys.clone(),
                                kw_tys.clone(),
                                SiteRecv::Instance,
                            ));
                        }
                        out.push((class_id, method.clone(), arg_tys.clone(), kw_tys.clone(), site_recv));
                    }
                }
                if let Some(r) = recv { self.collect_send_sites(r, self_class, class_body, helpers, out); }
                for a in args { self.collect_send_sites(a, self_class, class_body, helpers, out); }
                if let Some(b) = block { self.collect_send_sites(b, self_class, class_body, helpers, out); }
            }
            ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
                for e in exprs { self.collect_send_sites(e, self_class, class_body, helpers, out); }
            }
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    self.collect_send_sites(k, self_class, class_body, helpers, out);
                    self.collect_send_sites(v, self_class, class_body, helpers, out);
                }
            }
            ExprNode::ForwardKeywordsWithPairs { entries } => {
                for (key, value) in entries {
                    self.collect_send_sites(key, self_class, class_body, helpers, out);
                    self.collect_send_sites(value, self_class, class_body, helpers, out);
                }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.collect_send_sites(cond, self_class, class_body, helpers, out);
                self.collect_send_sites(then_branch, self_class, class_body, helpers, out);
                self.collect_send_sites(else_branch, self_class, class_body, helpers, out);
            }
            ExprNode::Case { scrutinee, arms } => {
                self.collect_send_sites(scrutinee, self_class, class_body, helpers, out);
                for arm in arms {
                    if let Some(g) = &arm.guard { self.collect_send_sites(g, self_class, class_body, helpers, out); }
                    self.collect_send_sites(&arm.body, self_class, class_body, helpers, out);
                }
            }
            ExprNode::CaseMatch { scrutinee, arms, else_body } => {
                self.collect_send_sites(scrutinee, self_class, class_body, helpers, out);
                for arm in arms {
                    arm.pattern.for_each_expr(&mut |e| self.collect_send_sites(e, self_class, class_body, helpers, out));
                    if let Some((_, g)) = &arm.guard { self.collect_send_sites(g, self_class, class_body, helpers, out); }
                    self.collect_send_sites(&arm.body, self_class, class_body, helpers, out);
                }
                if let Some(e) = else_body { self.collect_send_sites(e, self_class, class_body, helpers, out); }
            }
            ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
                self.collect_send_sites(value, self_class, class_body, helpers, out);
                pattern.for_each_expr(&mut |e| self.collect_send_sites(e, self_class, class_body, helpers, out));
            }
            ExprNode::BoolOp { left, right, .. }
            | ExprNode::RescueModifier { expr: left, fallback: right } => {
                self.collect_send_sites(left, self_class, class_body, helpers, out);
                self.collect_send_sites(right, self_class, class_body, helpers, out);
            }
            ExprNode::Let { value, body, .. } => {
                self.collect_send_sites(value, self_class, class_body, helpers, out);
                self.collect_send_sites(body, self_class, class_body, helpers, out);
            }
            ExprNode::Lambda { body, .. } => self.collect_send_sites(body, self_class, class_body, helpers, out),
            ExprNode::MethodRef { recv, .. } => {
                if let Some(r) = recv {
                    self.collect_send_sites(r, self_class, class_body, helpers, out);
                }
            }
            ExprNode::Apply { fun, args, block } => {
                self.collect_send_sites(fun, self_class, class_body, helpers, out);
                for a in args { self.collect_send_sites(a, self_class, class_body, helpers, out); }
                if let Some(b) = block { self.collect_send_sites(b, self_class, class_body, helpers, out); }
            }
            ExprNode::Assign { target, value }
            | ExprNode::OpAssign { target, value, .. } => {
                self.collect_send_sites(value, self_class, class_body, helpers, out);
                if let LValue::Attr { recv, .. } = target {
                    self.collect_send_sites(recv, self_class, class_body, helpers, out);
                }
                if let LValue::Index { recv, index } = target {
                    self.collect_send_sites(recv, self_class, class_body, helpers, out);
                    self.collect_send_sites(index, self_class, class_body, helpers, out);
                }
            }
            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.collect_send_sites(expr, self_class, class_body, helpers, out);
                    }
                }
            }
            ExprNode::Yield { args } => {
                for a in args { self.collect_send_sites(a, self_class, class_body, helpers, out); }
            }
            ExprNode::Raise { value } => self.collect_send_sites(value, self_class, class_body, helpers, out),
            ExprNode::Return { value } => self.collect_send_sites(value, self_class, class_body, helpers, out),
            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args { self.collect_send_sites(a, self_class, class_body, helpers, out); }
                }
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.collect_send_sites(body, self_class, class_body, helpers, out);
                for rc in rescues {
                    for c in &rc.classes { self.collect_send_sites(c, self_class, class_body, helpers, out); }
                    self.collect_send_sites(&rc.body, self_class, class_body, helpers, out);
                }
                if let Some(e) = else_branch { self.collect_send_sites(e, self_class, class_body, helpers, out); }
                if let Some(e) = ensure { self.collect_send_sites(e, self_class, class_body, helpers, out); }
            }
            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { self.collect_send_sites(v, self_class, class_body, helpers, out); }
            }
            ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => {
                self.collect_send_sites(value, self_class, class_body, helpers, out)
            }
            ExprNode::MultiAssign { value, .. } => {
                self.collect_send_sites(value, self_class, class_body, helpers, out)
            }
            ExprNode::While { cond, body, .. } => {
                self.collect_send_sites(cond, self_class, class_body, helpers, out);
                self.collect_send_sites(body, self_class, class_body, helpers, out);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { self.collect_send_sites(b, self_class, class_body, helpers, out); }
                if let Some(e) = end { self.collect_send_sites(e, self_class, class_body, helpers, out); }
            }
            ExprNode::Cast { value, .. } => self.collect_send_sites(value, self_class, class_body, helpers, out),
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::Retry
            | ExprNode::Redo
            | ExprNode::ForwardArgs
            | ExprNode::ForwardKeywords
            | ExprNode::Defined { .. }
            | ExprNode::SelfRef => {}
        }
    }


    /// Does the catalog classify `method` as a Relation-builder
    /// chain step (e.g., `where`, `limit`, `order`)? True only for
    /// methods with `ChainKind::Builder` in the catalog; falls to
    /// false for Terminal / NotApplicable / unclassified.
    ///
    /// Used by `contribute_send_effect` to skip effect attachment
    /// on Builder Sends — the Relation is lazy, no SQL executes,
    /// and emitting `await` would produce one spurious round-trip
    /// per chain link under async backends.
    fn is_builder_chain(&self, method: &str) -> bool {
        // Relation-context entries excluded for the same reason as
        // `SqliteAdapter::classify_ar_method`: this name-only search
        // must not reclassify names that only exist under the
        // Relation context until Relation-receiver dispatch consumes
        // them (`page`, `per`, `not`, … becoming Builder here would
        // silently drop effect attachment on today's Sends).
        crate::catalog::lookup_any(method)
            .filter(|e| e.receiver != crate::catalog::ReceiverContext::Relation)
            .any(|entry| {
                matches!(entry.chain, crate::catalog::ChainKind::Builder)
            })
    }

}

// AR-method classification moved to `crate::adapter::SqliteAdapter`.
// `Analyzer::contribute_send_effect` consults `self.adapter` instead
// of free helpers; alternate backends plug in via
// `Analyzer::with_adapter`.

/// Does `filter` apply to the action named `action_name`? Rails scopes:
/// - `only: [...]` limits to the listed actions
/// - `except: [...]` excludes the listed actions
/// - both empty → applies to all actions on the controller
/// Does this scope/class-method body *return* a relation? True iff
/// the body's tail expression is a query-builder chain: the outermost
/// (tail) hop is a chain builder (a Relation-context `Builder` in the
/// catalog), a same-model scope call, or a relation root
/// (`all`/`unscoped`/`none`), and the receiver spine walks down
/// through such hops to a recognizable root — implicit self, `self`,
/// or a constant naming this model. Deliberately conservative:
/// a hop carrying a literal block (`select { |s| … }` is
/// `Array#select` — it materializes), a terminal tail
/// (`count`/`pluck`/`first`), a cross-model constant root, or
/// anything unrecognizable returns false, and the caller keeps
/// today's `Array<Self>` typing.
///
/// This is the *typing* twin of `lower::scope_chain`'s
/// `mentions_bare_chain_start`: that predicate decides which class
/// methods get `__rel` threading (a mention anywhere qualifies);
/// this one decides which bodies *return* the relation (only the
/// tail position counts), because only those may declare
/// `Ty::Relation { of: Self }`.
fn body_tail_yields_relation(
    body: &Expr,
    model_id: &ClassId,
    scope_names: &std::collections::HashSet<Symbol>,
) -> bool {
    let mut e = match &*body.node {
        ExprNode::Seq { exprs } => match exprs.last() {
            Some(last) => last,
            None => return false,
        },
        _ => body,
    };
    loop {
        let ExprNode::Send { recv, method, block, .. } = &*e.node else {
            return false;
        };
        let is_builder = crate::catalog::lookup(
            method.as_str(),
            crate::catalog::ReceiverContext::Relation,
        )
        .map(|entry| matches!(entry.chain, crate::catalog::ChainKind::Builder))
        .unwrap_or(false);
        let is_scope = scope_names.contains(method);
        let is_root_call = matches!(method.as_str(), "all" | "unscoped");
        if !(is_builder || is_scope || is_root_call) {
            return false;
        }
        if block.is_some() {
            return false;
        }
        match recv {
            None => return true,
            Some(r) => match &*r.node {
                ExprNode::SelfRef => return true,
                // A constant root must name THIS model — a body tail
                // rooted at another model returns a relation over
                // that model, which `Relation { of: Self }` would
                // mistype. (Cross-model class-method returns are the
                // instance-method `UserMethodReturns` family's
                // territory, not this seed's.)
                ExprNode::Const { path } => {
                    return path.last().is_some_and(|last| {
                        model_id
                            .0
                            .as_str()
                            .rsplit("::")
                            .next()
                            .is_some_and(|own| own == last.as_str())
                    });
                }
                _ => e = r,
            },
        }
    }
}

/// What a `scope :name, -> { … }` call returns.
///
/// Three answers, in order:
///
///  1. Body tail is a BUILDER chain rooted at this model
///     (`where(…).order(…)`) → `Relation { of: Self }`, the true lazy
///     relation that chains and class-side delegation resolve.
///  2. Body tail is a relation TERMINAL (`order(:created_at).first`,
///     `where(…).count`) → whatever the catalog says that terminal
///     returns, instantiated against this model. campfire's
///     `scope :original, -> { order(:created_at).first }` is the shape:
///     it yields a Room, and calling it `Array[Room]` is not merely
///     imprecise — it made `room_url(user.rooms.original)` render
///     `/rooms/#<Room:0x…>`, because the route-helper id projection
///     asks the type and an Array is not a record.
///  3. Anything else (block-taking hop, cross-model root, unrecognized
///     tail) → the legacy `Array[Self]` stand-in, whose `Array[Class]`
///     dispatch delegates the same way.
///
/// Shared with the LOWERING-side registry (`build_class_info`), which
/// is what types test bodies. The analyzer's registry and that one are
/// two different maps over the same fact, and a second copy of this
/// rule is exactly how `user.rooms.opens.last` came to type in a
/// controller and not in a test.
pub(crate) fn scope_return_seed(
    body: &Expr,
    model_id: &ClassId,
    scope_names: &std::collections::HashSet<Symbol>,
) -> Ty {
    if body_tail_yields_relation(body, model_id, scope_names) {
        return Ty::Relation { of: model_id.clone() };
    }
    if let Some(kind) = body_tail_terminal_kind(body, model_id, scope_names) {
        return instantiate_return_kind(kind, model_id);
    }
    Ty::Array { elem: Box::new(Ty::Class { id: model_id.clone(), args: vec![] }) }
}

/// Did [`scope_return_seed`] actually classify this body, or fall back?
///
/// The fallback is `Array[Self]`, which is a safe stand-in for a SCOPE
/// (a scope is a query by construction) but a fabrication for a
/// hand-written class method that happens to live on a model. Callers
/// registering the latter ask this first.
pub(crate) fn body_is_relation_query(
    body: &Expr,
    model_id: &ClassId,
    scope_names: &std::collections::HashSet<Symbol>,
) -> bool {
    body_tail_yields_relation(body, model_id, scope_names)
        || body_tail_terminal_kind(body, model_id, scope_names).is_some()
}

/// The catalog return kind of a scope body's tail TERMINAL, when the
/// receiver it terminates is itself a relation over this model.
///
/// `order(:created_at).first` → `first` is a `ChainKind::Terminal` in
/// `ReceiverContext::Relation` with `ReturnKind::SelfOrNil`, and its
/// receiver `order(:created_at)` is a builder chain rooted here — so
/// the scope returns `Self | Nil`. Reusing `body_tail_yields_relation`
/// for the receiver check is what keeps the two rules from drifting;
/// the guard it applies (no blocks, constant root must name THIS model)
/// applies here unchanged.
fn body_tail_terminal_kind(
    body: &Expr,
    model_id: &ClassId,
    scope_names: &std::collections::HashSet<Symbol>,
) -> Option<crate::catalog::ReturnKind> {
    let tail = match &*body.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => body,
    };
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*tail.node else {
        return None;
    };
    let entry = crate::catalog::lookup(method.as_str(), crate::catalog::ReceiverContext::Relation)?;
    if !matches!(entry.chain, crate::catalog::ChainKind::Terminal) {
        return None;
    }
    if !body_tail_yields_relation(recv, model_id, scope_names) {
        return None;
    }
    // The COUNTED form: `first(n)` / `last(n)` answer an Array of up to
    // n records, where the bare form the catalog names answers one or
    // nil. The count is any single positional argument — campfire's
    // `scope :last_page, -> { ordered.last(PAGE_SIZE) }` passes a
    // concern constant, and the scope seeded `Message | nil`, which
    // every `@messages` in the app then carried into its views. Same
    // rule as `body::send::counted_first_last`, which types the call
    // where it can see the argument's type; here only its presence is
    // needed.
    if matches!(method.as_str(), "first" | "last" | "take") && args.len() == 1 {
        return Some(crate::catalog::ReturnKind::ArrayOfSelf);
    }
    entry.return_kind
}

/// Record a template's renderer→partial edges under its view name,
/// UNIONED with any already there. A view name is format-blind —
/// campfire's `messages/_message.html.erb` and
/// `messages/_message.json.jbuilder` are both `messages/_message` —
/// and a plain insert let whichever template was walked last win: the
/// jbuilder's empty list erased the ERB's three partials, so
/// `_actions` and `_presentation` had no renderer, no ivar context,
/// and no feeder, and the dead-view walk called them dead.
fn record_render_edges(
    render_edges: &mut HashMap<Symbol, Vec<Symbol>>,
    view: &Symbol,
    targets: Vec<Symbol>,
) {
    let entry = render_edges.entry(view.clone()).or_default();
    for t in targets {
        if !entry.contains(&t) {
            entry.push(t);
        }
    }
}

/// A scope body whose tail is a call to a MATERIALIZING sibling scope
/// on a relation chain rooted at this model — `before(m).last_page` —
/// answers that sibling's seed. The receiver check is the same
/// `body_tail_yields_relation` the other classifiers use.
fn scope_tail_materializing_sibling(
    body: &Expr,
    model_id: &ClassId,
    scope_names: &std::collections::HashSet<Symbol>,
    materializing: &HashMap<Symbol, Ty>,
) -> Option<Ty> {
    let tail = match &*body.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => body,
    };
    let ExprNode::Send { recv, method, block: None, .. } = &*tail.node else {
        return None;
    };
    let seed = materializing.get(method)?;
    let rooted_here = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef)
            || body_tail_yields_relation(r, model_id, scope_names),
    };
    rooted_here.then(|| seed.clone())
}

/// Does `start` descend from `ActiveRecord::Base`?
///
/// `app/models` is a directory, not a type: a class in it is an
/// ActiveRecord model only if it says so by inheritance. Rails' own
/// convention is the whole signal — `ApplicationRecord <
/// ActiveRecord::Base`, and every model under it.
///
/// Conservative in both unknown directions, because a false negative
/// deletes a real model's entire query surface while a false positive
/// only restores today's behavior: an ancestor this map doesn't know
/// (a gem base class) answers YES, and so does a chain long enough to
/// hit the depth cap. Only a chain that terminates in a class with no
/// superclass at all — `class Search`, `class Opengraph::Location` —
/// answers NO.
fn descends_from_active_record(
    start: &ClassId,
    parents: &HashMap<&ClassId, Option<&ClassId>>,
) -> bool {
    let mut current = start;
    for _ in 0..32 {
        let Some(parent) = parents.get(current) else {
            // Unmodeled ancestor — can't prove it isn't AR.
            return true;
        };
        let Some(parent) = parent else {
            // A class with no superclass. Not a model.
            return false;
        };
        if matches!(parent.0.as_str(), "ActiveRecord::Base" | "ApplicationRecord") {
            return true;
        }
        current = parent;
    }
    true
}

/// Instantiate a catalog [`crate::catalog::ReturnKind`] against a
/// concrete model class. Shared between the per-model registry seeding
/// in `with_adapter` (Class/Instance receiver contexts, where `self_id`
/// is the model being seeded) and Relation-receiver dispatch in
/// `body/send.rs` (where "Self" denotes the relation's *element* model
/// — `Relation { of }`'s `of`).
pub(crate) fn instantiate_return_kind(
    kind: crate::catalog::ReturnKind,
    self_id: &ClassId,
) -> Ty {
    use crate::catalog::ReturnKind;
    let self_ty = || Ty::Class { id: self_id.clone(), args: vec![] };
    match kind {
        ReturnKind::SelfType => self_ty(),
        ReturnKind::ArrayOfSelf => Ty::Array { elem: Box::new(self_ty()) },
        ReturnKind::SelfOrNil => Ty::Union { variants: vec![self_ty(), Ty::Nil] },
        ReturnKind::Int => Ty::Int,
        ReturnKind::IntOrNil => Ty::Union { variants: vec![Ty::Int, Ty::Nil] },
        ReturnKind::Bool => Ty::Bool,
        ReturnKind::HashSymStr => Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        },
        ReturnKind::ArrayOfSym => Ty::Array { elem: Box::new(Ty::Sym) },
        ReturnKind::Str => Ty::Str,
        ReturnKind::ClassRef(path) => Ty::Class {
            id: ClassId(Symbol::from(path)),
            args: vec![],
        },
        ReturnKind::RelationOfSelf => Ty::Relation { of: self_id.clone() },
        ReturnKind::ArrayOfInt => Ty::Array { elem: Box::new(Ty::Int) },
        ReturnKind::ArrayOfUntyped => Ty::Array { elem: Box::new(Ty::Untyped) },
        ReturnKind::Untyped => Ty::Untyped,
    }
}

pub(crate) fn before_filter_applies(filter: &Filter, action_name: &Symbol) -> bool {
    if !filter.only.is_empty() {
        return filter.only.contains(action_name);
    }
    if !filter.except.is_empty() {
        return !filter.except.contains(action_name);
    }
    true
}

/// Merge ivar bindings from every before/around filter that applies to
/// this action, looking up each filter's `target` in the pre-computed
/// per-action bindings table. Later filters overwrite earlier ones on
/// conflicting keys — matches Rails' "last-registered wins" when the
/// same ivar is set by multiple callbacks. The chain carries all filter
/// kinds (for `App::controller_resolutions`); only Before/Around run
/// ahead of the action and contribute ivars here.
/// Cap on the number of call-chain hops [`collect_transitive_filter_ivars`]
/// will follow from a filter's target method. Four hops covers every
/// filter → helper → helper → helper chain seen in practice — Procore's
/// `authorize` → `set_variables_in_authorize!` → `set_project_variables!`
/// → `@project = ...` is two hops from the filter target, and
/// `set_project_variables!` → `set_provider_variables!` → `@domain =
/// ...` a third — while still bounding the walk against a runaway or
/// accidentally-cyclic call graph. A chain longer than the cap simply
/// stays unresolved past the cut, same as today's zero-hop behavior.
const MAX_FILTER_CALL_DEPTH: usize = 4;

/// Ivar writes reachable from `expr` (a filter target's own body, or a
/// method it transitively calls) by following every receiverless
/// self-call (`foo`, `self.foo`, `foo(args)`) into its resolved body,
/// recursively, up to `depth` hops. `bodies` is the flat method-name →
/// typed-body table built in `run_typing_passes` as `chained_bodies` —
/// the same resolution order dispatch itself uses: the controller's own
/// methods, its ancestors (nearest first), and every directly or
/// transitively mixed-in concern.
///
/// This is may-write semantics — the same kind the direct (non-
/// transitive) filter-body seeding in [`extract_ivar_assignments`]
/// already performs within ONE method body (union whatever every
/// branch writes; never evaluate which branch actually runs) — carried
/// across call boundaries instead of stopping at them:
///
///   - A call reached unconditionally (the top level of a `Seq`, or
///     every alternative of a branching construct) contributes its
///     callee's writes as-is, unioned with whatever else is reachable.
///   - A call reached on only SOME alternative of an `If`/`Case`/
///     `BoolOp`/loop/rescue (the other alternative(s) don't reach an
///     equivalent write) contributes its writes with a `Ty::Nil` arm
///     unioned in for the alternatives that don't write it — see
///     [`merge_alternative_branches`]. This is what turns Procore's
///     `@project` (written only on the `if self.class.project_area?`
///     arm of `set_variables_in_authorize!`, with `company_area?` /
///     `super_area?` / an `else` that logs and returns) into
///     `Project?` rather than a false unconditional `Project`: the
///     union of "`Project` on one arm" and "nothing on the other
///     three" is nilable, not `Project`. That is strictly better than
///     today's `ivar_unresolved` on `@project` and does not misreport
///     the one case that matters (a caller that reads `@project`
///     without a nil check on a path where it truly is always set
///     still gets `Project?`, a conservative widening, never a false
///     `Project` that would hide a real nil).
///   - Class-side predicates on the controller's own class
///     (`self.class.project_area?`, `self.class.company_area?`, ...)
///     and conditions behind feature flags or other calls this
///     analyzer can't evaluate (`unless feature_active?(...)`) are
///     NEVER evaluated for their value — this function only ever reads
///     a branching node's *branches*, not its condition's truthiness,
///     so "both/every arm may run" falls out automatically rather than
///     needing a special case per condition shape.
///
/// `visited` is the current call STACK (pushed on entry to a callee,
/// popped on return), not a global "ever seen" set: a diamond-shaped
/// call graph (two different callees that both reach a common helper)
/// still gets that helper's contribution on both paths; only a genuine
/// cycle (mutual or self-recursion) is cut off, with the depth cap as
/// the backstop for a long-but-non-cyclic chain.
fn collect_transitive_filter_ivars(
    expr: &Expr,
    bodies: &HashMap<Symbol, &Expr>,
    depth: usize,
    visited: &mut BTreeSet<Symbol>,
    own: bool,
) -> HashMap<Symbol, Ty> {
    if depth == 0 {
        return HashMap::new();
    }
    match &*expr.node {
        ExprNode::Seq { exprs } => {
            let mut out = HashMap::new();
            for e in exprs {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(e, bodies, depth, visited, own));
            }
            out
        }
        // The condition is never evaluated (see doc above) — only
        // walked for calls of its own (`unless feature_active?(...)`
        // is itself a receiverless call, followed like any other).
        // `then_branch`/`else_branch` are alternatives: Ruby's `if`
        // with no `else` types the missing branch as a Nil-producing
        // no-op, which `merge_alternative_branches` needs as an
        // explicit empty contribution to make an only-one-arm write
        // nilable — it's already exactly that shape here because
        // `else_branch` is a literal `nil` expression when the source
        // omitted one, and walking it yields `{}`.
        ExprNode::If { cond, then_branch, else_branch } => {
            let mut out = collect_transitive_filter_ivars(cond, bodies, depth, visited, own);
            let branches = vec![
                collect_transitive_filter_ivars(then_branch, bodies, depth, visited, own),
                collect_transitive_filter_ivars(else_branch, bodies, depth, visited, own),
            ];
            union_ivar_maps(&mut out, merge_alternative_branches(branches));
            out
        }
        ExprNode::Case { scrutinee, arms } => {
            let mut out = collect_transitive_filter_ivars(scrutinee, bodies, depth, visited, own);
            let mut branches: Vec<HashMap<Symbol, Ty>> = arms
                .iter()
                .map(|arm| collect_transitive_filter_ivars(&arm.body, bodies, depth, visited, own))
                .collect();
            // No `when`/pattern may match — Ruby's `case` with nothing
            // matching (and no `else`) evaluates to nil — so an
            // implicit empty alternative is added even when every
            // explicit arm agrees, otherwise an exhaustive-looking
            // `case` would wrongly type as non-nilable.
            branches.push(HashMap::new());
            union_ivar_maps(&mut out, merge_alternative_branches(branches));
            out
        }
        ExprNode::BoolOp { left, right, .. } => {
            let mut out = collect_transitive_filter_ivars(left, bodies, depth, visited, own);
            let right_out = collect_transitive_filter_ivars(right, bodies, depth, visited, own);
            // `right` only evaluates if `left` doesn't short-circuit
            // the operator — may-not-run, same treatment as an `If`
            // with no `else`.
            union_ivar_maps(&mut out, merge_alternative_branches(vec![right_out, HashMap::new()]));
            out
        }
        ExprNode::While { cond, body, .. } => {
            let mut out = collect_transitive_filter_ivars(cond, bodies, depth, visited, own);
            let body_out = collect_transitive_filter_ivars(body, bodies, depth, visited, own);
            // The body may run zero times.
            union_ivar_maps(&mut out, merge_alternative_branches(vec![body_out, HashMap::new()]));
            out
        }
        ExprNode::RescueModifier { expr: e, fallback } => {
            let mut out = collect_transitive_filter_ivars(e, bodies, depth, visited, own);
            let fb = collect_transitive_filter_ivars(fallback, bodies, depth, visited, own);
            union_ivar_maps(&mut out, merge_alternative_branches(vec![fb, HashMap::new()]));
            out
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            let mut out = collect_transitive_filter_ivars(body, bodies, depth, visited, own);
            let mut alt: Vec<HashMap<Symbol, Ty>> = rescues
                .iter()
                .map(|r| collect_transitive_filter_ivars(&r.body, bodies, depth, visited, own))
                .collect();
            alt.push(HashMap::new()); // no rescue triggers
            union_ivar_maps(&mut out, merge_alternative_branches(alt));
            if let Some(e) = else_branch {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(e, bodies, depth, visited, own));
            }
            if let Some(e) = ensure {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(e, bodies, depth, visited, own));
            }
            out
        }
        // The call-following core: a receiverless send (`foo`,
        // `foo(args)`) or an explicit-self send (`self.foo`) resolves
        // against `bodies` exactly like `authorize` → `set_variables_
        // in_authorize!` in the doc comment above. Any other receiver
        // (`@provider.tools`, `Project.find(...)`) is not followed —
        // only OWN-class dispatch is in scope here — but is still
        // walked structurally so a self-call buried in its receiver,
        // args, or block is still found.
        ExprNode::Send { recv, method, args, block, .. } => {
            let mut out = HashMap::new();
            let is_self_call = match recv {
                None => true,
                Some(r) => matches!(&*r.node, ExprNode::SelfRef),
            };
            if is_self_call {
                if let Some(callee_body) = bodies.get(method) {
                    if visited.insert(method.clone()) {
                        // One recursive call finds BOTH the callee's own
                        // direct writes and whatever it further calls —
                        // the Assign/OpAssign/MultiAssign arms below
                        // record a direct write with the same
                        // branch-aware nilability as everything else in
                        // this function. Do not swap this for a plain
                        // `extract_ivar_assignments(callee_body, ..)`:
                        // that function's own `If`/`Case` arms union
                        // every branch's writes WITHOUT a Nil arm for
                        // "this branch didn't write it" (correct for
                        // its existing direct, single-body callers,
                        // which never needed that distinction) — using
                        // it here silently discarded the nilability
                        // this function exists to add, which is exactly
                        // how `set_variables_in_authorize!`'s `@project`
                        // (real one arm of an if/elsif/elsif/else) was
                        // first observed coming out as non-nilable
                        // `Project` instead of `Project?`.
                        union_ivar_maps(
                            &mut out,
                            collect_transitive_filter_ivars(
                                callee_body,
                                bodies,
                                depth - 1,
                                visited,
                                false,
                            ),
                        );
                        visited.remove(method);
                    }
                }
            }
            if let Some(r) = recv {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(r, bodies, depth, visited, own));
            }
            for a in args {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(a, bodies, depth, visited, own));
            }
            if let Some(b) = block {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(b, bodies, depth, visited, own));
            }
            out
        }
        ExprNode::Lambda { body, .. } => collect_transitive_filter_ivars(body, bodies, depth, visited, own),
        ExprNode::Let { value, body, .. } => {
            let mut out = collect_transitive_filter_ivars(value, bodies, depth, visited, own);
            union_ivar_maps(&mut out, collect_transitive_filter_ivars(body, bodies, depth, visited, own));
            out
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => {
            collect_transitive_filter_ivars(value, bodies, depth, visited, own)
        }
        // A direct ivar write, recorded the same way
        // `extract_ivar_assignments` records one — from `value.ty`,
        // unioned with anything already known for that name — except
        // this arm sits inside the branch-aware walk above, so a write
        // that only some `If`/`Case` alternatives reach still gets its
        // Nil arm from `merge_alternative_branches` at the enclosing
        // branch node, not lost the way going through
        // `extract_ivar_assignments` on the whole callee body would.
        ExprNode::Assign { target: LValue::Ivar { name }, value }
        | ExprNode::OpAssign { target: LValue::Ivar { name }, value, .. } => {
            let mut out = collect_transitive_filter_ivars(value, bodies, depth, visited, own);
            if !own {
                if let Some(ty) = value.ty.clone() {
                    union_ivar_maps(&mut out, HashMap::from([(name.clone(), ty)]));
                }
            }
            out
        }
        // `@a, @b = expr` — same per-position typing
        // `extract_ivar_assignments` uses for the non-transitive case.
        ExprNode::MultiAssign { targets, value } => {
            let mut out = collect_transitive_filter_ivars(value, bodies, depth, visited, own);
            for (i, target) in targets.iter().enumerate() {
                if let (LValue::Ivar { name }, false) = (target, own) {
                    if let Some(ty) = body::multiassign_target_ty(&value.ty, i) {
                        union_ivar_maps(&mut out, HashMap::from([(name.clone(), ty)]));
                    }
                }
            }
            out
        }
        // Any other assignment target (local var, constant, attribute,
        // `@hash[k] = v` index write). Only the RHS/index can hide a
        // further call or a nested ivar write; the target itself
        // contributes nothing here (the Hash-value-widening
        // `extract_ivar_assignments` does for `@hash[k] = v` is a
        // refinement this transitive walk doesn't attempt — out of
        // scope for the filter-chain gap this function targets).
        ExprNode::Assign { target, value } | ExprNode::OpAssign { target, value, .. } => {
            let mut out = collect_transitive_filter_ivars(value, bodies, depth, visited, own);
            if let LValue::Index { recv, index } = target {
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(recv, bodies, depth, visited, own));
                union_ivar_maps(&mut out, collect_transitive_filter_ivars(index, bodies, depth, visited, own));
            }
            out
        }
        _ => HashMap::new(),
    }
}

/// Union `incoming` into `out`, joining any overlapping key with
/// [`union_of`] (accumulate, matching `extract_ivar_assignments`'s
/// treatment of repeated writes — never last-write-wins).
fn union_ivar_maps(out: &mut HashMap<Symbol, Ty>, incoming: HashMap<Symbol, Ty>) {
    for (k, v) in incoming {
        let merged = match out.remove(&k) {
            Some(prev) => crate::analyze::body::union_of(prev, v),
            None => v,
        };
        out.insert(k, merged);
    }
}

/// Merge mutually-exclusive branch contributions (the arms of an `if`/
/// `case`/loop/rescue): a key written on every branch keeps the union
/// of its per-branch types; a key written on only SOME branches gets
/// `Ty::Nil` unioned in for the branches that don't write it, because
/// only one alternative actually runs at request time and it might be
/// one of the ones that doesn't. `union_of(_, Ty::Nil)` widens whatever
/// the other branches contributed into a nilable union.
fn merge_alternative_branches(branches: Vec<HashMap<Symbol, Ty>>) -> HashMap<Symbol, Ty> {
    let mut keys: BTreeSet<Symbol> = BTreeSet::new();
    for b in &branches {
        keys.extend(b.keys().cloned());
    }
    let mut out = HashMap::new();
    for k in keys {
        let mut ty: Option<Ty> = None;
        for b in &branches {
            let contribution = b.get(&k).cloned().unwrap_or(Ty::Nil);
            ty = Some(match ty {
                Some(prev) => crate::analyze::body::union_of(prev, contribution),
                None => contribution,
            });
        }
        if let Some(t) = ty {
            out.insert(k, t);
        }
    }
    out
}

fn merged_before_seed(
    chained_filters: &[(Filter, ClassId, ClassId)],
    action_name: &Symbol,
    action_bindings: &HashMap<Symbol, HashMap<Symbol, Ty>>,
) -> HashMap<Symbol, Ty> {
    let mut seed: HashMap<Symbol, Ty> = HashMap::new();
    for (filter, _, _) in chained_filters {
        if !matches!(filter.kind, FilterKind::Before | FilterKind::Around) {
            continue;
        }
        if before_filter_applies(filter, action_name) {
            if let Some(fivars) = action_bindings.get(&filter.target) {
                for (k, v) in fivars {
                    seed.insert(k.clone(), v.clone());
                }
            }
        }
    }
    seed
}

/// Build one controller's own segment of the resolved filter chain,
/// provenance-tagged (each entry carries the class or concern module
/// that declared it) and in Rails registration order: the class body is
/// walked top-to-bottom, so `before_action` lines land where written
/// and a concern's `included do` filters splice in at the `include`
/// site (Rails runs the block at include time). Concern includes close
/// transitively, dependencies first — ActiveSupport::Concern includes a
/// concern's own dependencies before running its `included` block —
/// and dedupe across the walk (Ruby `include` is idempotent). All
/// filter kinds are kept: the seeding paths read only Before/Around,
/// but the persisted `App::controller_resolutions` chain wants
/// After/Skip entries too.
///
/// Block-form filters (`before_action { @page = page }`) name no
/// method, so they survive ingest as `Unknown` body items (preserving
/// round-trip) rather than `Filter`s. Each is synthesized in place with
/// a sentinel target that can't collide with a real method (so it never
/// resolves a view) and carries the call itself in `Filter::block`, so
/// the trace can place and name it; the second return value carries its
/// harvested ivar bindings — the bodies were already typed by the Phase
/// 0 `Unknown`-item pass — for registration alongside real targets. A
/// block that assigns nothing still gets its chain entry: campfire's
/// `before_action do Current.request = request end` runs whether or not
/// it touches an ivar, and a trace that omits it is wrong. A block the
/// splice carried in from a concern is attributed to that concern
/// through `spliced_origin` (keyed by the same sentinel).
/// `only:`/`except:` on a block filter gate it like a named one.
fn build_sourced_filter_chain(
    controller: &Controller,
    spliced_origin: Option<&HashMap<Symbol, ClassId>>,
) -> (Vec<(Filter, ClassId)>, Vec<(Symbol, HashMap<Symbol, Ty>)>) {
    let own_id = controller.name.clone();
    let mut chain: Vec<(Filter, ClassId)> = Vec::new();
    let mut block_bindings: Vec<(Symbol, HashMap<Symbol, Ty>)> = Vec::new();

    for (idx, item) in controller.body.iter().enumerate() {
        match item {
            ControllerBodyItem::Filter { filter, .. } => {
                let source = filter.from_concern.clone().unwrap_or_else(|| own_id.clone());
                chain.push((filter.clone(), source));
            }
            ControllerBodyItem::Unknown { expr, .. } => {
                let ExprNode::Send { recv: None, method, args: _, block, .. } = &*expr.node
                else {
                    continue;
                };
                match method.as_str() {
                    // `include` needs no arm: ingest's
                    // `splice_concerns_into_controllers` already copied the
                    // module's `included do` filters into this body, each
                    // tagged with `from_concern` for provenance. Splicing
                    // again here would double every concern filter.
                    "around_action" => {
                        let Some(block) = block else { continue };
                        // The attached block is a Lambda whose body is
                        // the filter code.
                        let body = match &*block.node {
                            ExprNode::Lambda { body, .. } => body,
                            _ => block,
                        };
                        let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                        extract_ivar_assignments(body, &mut ivars);
                        let target =
                            Symbol::from(format!("__{}_block_{idx}__", method.as_str()));
                        let (only, except) = block_filter_gates(expr);
                        let from_concern =
                            spliced_origin.and_then(|m| m.get(&target)).cloned();
                        let source = from_concern.clone().unwrap_or_else(|| own_id.clone());
                        chain.push((
                            Filter {
                                target_span: crate::span::Span::synthetic(),
                                kind: FilterKind::Around,
                                target: target.clone(),
                                from_concern,
                                only,
                                except,
                                only_style: crate::expr::ArrayStyle::default(),
                                except_style: crate::expr::ArrayStyle::default(),
                                if_cond: None,
                                unless_cond: None,
                                if_cond_expr: None,
                                unless_cond_expr: None,
                                block: Some(expr.clone()),
                                prepend: false,
                            },
                            source,
                        ));
                        block_bindings.push((target, ivars));
                    }
                    // `before_action`/`after_action`/`prepend_before_action`
                    // whose target is a lambda/proc/block literal instead
                    // of a Symbol — the block-attached form handled above
                    // for `around_action`, or `before_action -> { … },
                    // only: […]`'s argument form, which has no attached
                    // block at all (`ingest::controller::
                    // lambda_filter_target` recognizes both surfaces so
                    // this arm and `report_unrecognized_controller_macros`
                    // / `build_filter_preamble` agree on what counts).
                    "before_action" | "after_action" | "prepend_before_action" => {
                        let Some(target_info) =
                            crate::ingest::controller::lambda_filter_target(expr)
                        else {
                            continue;
                        };
                        let kind = if method.as_str() == "after_action" {
                            FilterKind::After
                        } else {
                            FilterKind::Before
                        };
                        let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                        extract_ivar_assignments(&target_info.body, &mut ivars);
                        let target =
                            Symbol::from(format!("__{}_block_{idx}__", method.as_str()));
                        let from_concern =
                            spliced_origin.and_then(|m| m.get(&target)).cloned();
                        let source = from_concern.clone().unwrap_or_else(|| own_id.clone());
                        chain.push((
                            Filter {
                                target_span: crate::span::Span::synthetic(),
                                kind,
                                target: target.clone(),
                                from_concern,
                                only: target_info.only,
                                except: target_info.except,
                                only_style: crate::expr::ArrayStyle::default(),
                                except_style: crate::expr::ArrayStyle::default(),
                                if_cond: target_info.if_cond,
                                unless_cond: target_info.unless_cond,
                                if_cond_expr: target_info.if_cond_expr,
                                unless_cond_expr: target_info.unless_cond_expr,
                                block: Some(expr.clone()),
                                prepend: method.as_str() == "prepend_before_action",
                            },
                            source,
                        ));
                        block_bindings.push((target, ivars));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (chain, block_bindings)
}

/// `only:` / `except:` of a block-form filter call, from its keyword
/// hash: `before_action only: :show do … end`. Symbols or a symbol
/// array; anything else contributes nothing.
fn block_filter_gates(call: &Expr) -> (Vec<Symbol>, Vec<Symbol>) {
    let mut only = Vec::new();
    let mut except = Vec::new();
    let ExprNode::Send { args, .. } = &*call.node else { return (only, except) };
    let syms = |e: &Expr| -> Vec<Symbol> {
        let one = |e: &Expr| match &*e.node {
            ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        };
        match &*e.node {
            ExprNode::Array { elements, .. } => elements.iter().filter_map(one).collect(),
            _ => one(e).into_iter().collect(),
        }
    };
    for a in args {
        let ExprNode::Hash { entries, .. } = &*a.node else { continue };
        for (k, v) in entries {
            let ExprNode::Lit { value: crate::expr::Literal::Sym { value: key } } = &*k.node
            else {
                continue;
            };
            match key.as_str() {
                "only" => only = syms(v),
                "except" => except = syms(v),
                _ => {}
            }
        }
    }
    (only, except)
}

/// Unify a stored param type with a freshly observed argument type.
/// Mirrors Spinel's `detect_poly_in_node` (`spinel_codegen.rb:6961-7000`)
/// joinrules at a higher level — we operate on `Ty` directly, so the
/// rules are:
/// - same type → keep
/// - one side is `Ty::Var` (no info yet) → take the other, including
///   when the other is `Untyped` (`Var` is the bottom of the join)
/// - one side is `Untyped` (an argument nobody could type) → take the
///   other: an untyped observation says nothing about the value, and
///   letting it into the union turns every concrete observation into
///   `untyped` downstream (gradual absorption at dispatch). campfire's
///   `start_new_session_for(user)` has three callers passing `User`
///   and one passing the result of a relation-delegated concern
///   finder the registry answers `untyped`; the parameter is a User.
/// - one side is `Nil` and the other is concrete → nullable union (T?)
/// - already a Union containing `observed` → keep
/// - otherwise → widen via `union_of`
fn unify_param_ty(stored: Ty, observed: Ty) -> Ty {
    if stored == observed {
        return stored;
    }
    // `Var` is checked on both sides before `Untyped` so the join is
    // commutative: `Var` (no observation) is below `Untyped` (an
    // observed argument nobody could type), and `Untyped` is below a
    // concrete type. Testing `Var | Untyped` together on `stored`
    // first made `unify(Untyped, Var) = Var` but `unify(Var, Untyped)
    // = Untyped`, so the result depended on the order call sites
    // arrived in (#209).
    if matches!(stored, Ty::Var { .. }) {
        return observed;
    }
    if matches!(observed, Ty::Var { .. }) {
        return stored;
    }
    if matches!(stored, Ty::Untyped) {
        return observed;
    }
    if matches!(observed, Ty::Untyped) {
        return stored;
    }
    // T + Nil → Union<T, Nil>; same for the symmetric case. Skip
    // double-wrapping if `stored` already encodes the nullable form.
    if matches!(observed, Ty::Nil) {
        if let Ty::Union { variants } = &stored {
            if variants.contains(&Ty::Nil) {
                return stored;
            }
        }
        return crate::analyze::body::union_of(stored, Ty::Nil);
    }
    if matches!(stored, Ty::Nil) {
        return crate::analyze::body::union_of(observed, Ty::Nil);
    }
    // Union<T, ...> already containing observed → keep stored.
    if let Ty::Union { variants } = &stored {
        if variants.contains(&observed) {
            return stored;
        }
    }
    crate::analyze::body::union_of(stored, observed)
}

/// Convert a controller class name into the view-path prefix.
/// `ArticlesController` → `articles`; namespaced controllers map each
/// module segment to a path segment (`Admin::UsersController` →
/// `admin/users`), matching Rails' template lookup. Strip the
/// `Controller` suffix, then snake_case per segment. Before the
/// per-segment split, `Admin::…` produced `admin::users` — no view is
/// ever named that, so namespaced controllers seeded nothing and every
/// ivar in their views went unresolved (131 Mastodon view files).
pub(crate) fn controller_view_prefix(class_id: &ClassId) -> String {
    let name = class_id.0.as_str();
    let stripped = name.strip_suffix("Controller").unwrap_or(name);
    stripped
        .split("::")
        .map(crate::naming::snake_case)
        .collect::<Vec<_>>()
        .join("/")
}

/// Determine which view path an action's RenderTarget names — `None` if
/// the action doesn't render a template (redirect, JSON, head).
pub(crate) fn view_name_for_action(controller: &ClassId, action: &Action) -> Option<Symbol> {
    let prefix = controller_view_prefix(controller);
    match &action.renders {
        RenderTarget::Inferred => {
            Some(Symbol::from(format!("{}/{}", prefix, action.name.as_str())))
        }
        RenderTarget::Template { name, .. } => {
            let n = name.as_str();
            if n.contains('/') {
                Some(Symbol::from(n))
            } else {
                Some(Symbol::from(format!("{}/{}", prefix, n)))
            }
        }
        RenderTarget::Redirect { .. }
        | RenderTarget::Json { .. }
        | RenderTarget::Head { .. } => None,
    }
}

/// Walk an action body collecting every `@ivar = expr` assignment into
/// `out`, keyed by ivar name → expression type. Used to seed the view's
/// Ctx so that `@post.title` in the template resolves against the action
/// that renders it.
///
/// Walks through branching constructs (If, RescueModifier) so ivars set
/// conditionally still show up. Deliberately does NOT walk into blocks
/// (Lambda bodies): ivars assigned inside iteration are run-time per-element
/// state, not the "data the controller passes to the view."
/// Walk a model's `Vec<ModelBodyItem>` collecting every in-class
/// constant assignment (`FLAGGABLE_DAYS = 7`, `COMMENT_REASONS =
/// {...}`, etc.) into a name→type table the body-typer's
/// `Ctx::constants` map consumes. Returns only those constants whose
/// RHS has been typed (Pass 0 in the model loop populates
/// `value.ty` by running the body-typer over each `Unknown` item
/// before this extraction runs).
///
/// Constants land in `ModelBodyItem::Unknown` because the model-body
/// classifier doesn't have a `Constant` variant — they're just bare
/// `Assign { LValue::Const, value }` expressions sitting at class
/// scope. The name comes from the LValue's path (last segment for
/// the common single-name case; qualified writes `Foo::BAR = 1` use
/// the joined path as their key, matching how the body-typer's
/// Const-read arm looks up `path.last()`).
/// Register a hardcoded stdlib/library class into the dispatch registry
/// with the given class (singleton) and instance method return types.
/// Never clobbers an app-defined method or class of the same name —
/// `.or_insert` means a real `def` always wins, so this only fills gaps
/// the app didn't define. Used for the Ruby stdlib catalog (SecureRandom,
/// File, Dir, Math, CGI, ERB::Util, Digest::*, URI, Set) in `Analyzer::new`.
pub(crate) fn extract_const_assignments(body: &[ModelBodyItem]) -> HashMap<Symbol, Ty> {
    let mut out: HashMap<Symbol, Ty> = HashMap::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        record_const(expr, &mut out);
    }
    out
}

/// Controller analog of [`extract_const_assignments`] — same shape,
/// different body-item enum. Controllers like `comments_controller.rb`
/// declare in-class constants (`COMMENTS_PER_PAGE = 20`,
/// `TOTP_SESSION_TIMEOUT = (60 * 15)`) the same way models do; the
/// body-typer needs the resulting name→type table to avoid the
/// `Ty::Class { id: ConstName }` fallback when method bodies
/// reference these constants.
pub(crate) fn extract_controller_const_assignments(
    body: &[ControllerBodyItem],
) -> HashMap<Symbol, Ty> {
    let mut out: HashMap<Symbol, Ty> = HashMap::new();
    for item in body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        record_const(expr, &mut out);
    }
    out
}

/// Collect the modules a controller mixes in via top-level
/// `include X` / `include X, Y` calls (round-tripped as `Unknown`
/// body items). Each becomes a `ClassId` whose registered instance
/// methods dispatch will consult for the controller. `include` with a
/// non-constant argument (rare metaprogramming) is skipped.
/// Reader type for an association, derived from cardinality:
/// `belongs_to`/`has_one` → `Target?` (nil before assignment / on a
/// missing optional), `has_many`/HABTM → `Array[Target]` (the
/// chainable relation stand-in). The writer twin (`name=`) accepts the
/// same shape. Shared by the model's own declarations and by
/// concern-`included do` declarations so both register identically.
fn association_member_ty(assoc: &crate::dialect::Association) -> (Symbol, Ty) {
    use crate::dialect::Association;
    match assoc {
        // Polymorphic belongs_to with a resolved implementor set —
        // the reader yields any implementor (or nil); dispatch on the
        // union resolves member-wise. Unresolved (no inverse `as:`
        // decls found) falls through to the phantom-target arm below.
        Association::BelongsTo { name, polymorphic: true, polymorphic_targets, .. }
            if !polymorphic_targets.is_empty() =>
        {
            let mut variants: Vec<Ty> = polymorphic_targets
                .iter()
                .map(|t| Ty::Class { id: t.clone(), args: vec![] })
                .collect();
            variants.push(Ty::Nil);
            (name.clone(), Ty::Union { variants })
        }
        Association::BelongsTo { name, target, .. }
        | Association::HasOne { name, target, .. } => (
            name.clone(),
            Ty::Union {
                variants: vec![Ty::Class { id: target.clone(), args: vec![] }, Ty::Nil],
            },
        ),
        Association::HasMany { name, target, .. }
        | Association::HasAndBelongsToMany { name, target, .. } => (
            name.clone(),
            Ty::Array { elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }) },
        ),
    }
}

/// The singular-association BUILDERS Rails generates beside the reader:
/// `build_<name>` / `create_<name>` / `create_<name>!` answer the new
/// target record (never nil — `create_` returns the unsaved record on
/// failure, `create_!` raises), and `reload_<name>` re-reads the
/// association, nil when there is none. campfire's `User::Bot` does
/// `user.create_webhook!(url:)` on a `has_one :webhook`. Collections
/// build through their proxy (`user.posts.build`), which the Array
/// representation already answers, so `has_many` contributes nothing
/// here.
fn association_builder_members(assoc: &crate::dialect::Association) -> Vec<(Symbol, Ty)> {
    use crate::dialect::Association;
    let (name, target) = match assoc {
        Association::BelongsTo { name, target, polymorphic: false, .. }
        | Association::HasOne { name, target, .. } => (name, target),
        _ => return Vec::new(),
    };
    let record = Ty::Class { id: target.clone(), args: vec![] };
    let n = name.as_str();
    vec![
        (Symbol::from(format!("build_{n}")), record.clone()),
        (Symbol::from(format!("create_{n}")), record.clone()),
        (Symbol::from(format!("create_{n}!")), record.clone()),
        (
            Symbol::from(format!("reload_{n}")),
            Ty::Union { variants: vec![record, Ty::Nil] },
        ),
    ]
}

/// Class ids a typed call receiver contributes to param unify and the
/// reverse call graph. Mirrors the receivers dispatch actually chases:
/// bare Class, Relation→model, Array elem, and Class arms of a Union.
fn class_ids_for_call_receiver(ty: &Ty) -> Vec<ClassId> {
    match ty {
        Ty::Class { id, .. } => vec![id.clone()],
        Ty::Relation { of } => vec![of.clone()],
        Ty::Array { elem } => class_ids_for_call_receiver(elem),
        Ty::Union { variants } => {
            let mut out = Vec::new();
            for v in variants {
                for id in class_ids_for_call_receiver(v) {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The model-side twin of [`controller_includes`]: modules a model mixes
/// in via top-level `include X` calls (round-tripped as `Unknown` body
/// items).
pub(crate) fn model_includes(model: &crate::dialect::Model) -> Vec<ClassId> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "include" {
            continue;
        }
        for arg in args {
            if let ExprNode::Const { path } = &*arg.node {
                // Framework MARKER mixins — `ActiveModel::*`,
                // `ActionView::Helpers::*` — drop here, in the single
                // shared home feeding every MODEL's `lc.includes`,
                // keeping real app mixins (`include IntervalHelper`)
                // intact. See `is_framework_marker_include` for what
                // supplies each family instead; the library-class twin
                // calls the same predicate from ingest's decl walk.
                let segs: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
                if crate::ingest::util::is_active_model_marker_include(&segs)
                    || crate::ingest::util::is_view_helper_marker_include(&segs)
                {
                    continue;
                }
                let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                out.push(ClassId(Symbol::from(joined)));
            }
        }
    }
    out
}

/// `**{k: v, …}.merge(h)`, `h` a `Hash[Symbol, V]` and the literal's
/// keys all symbols: the literal's pairs, and `V`. Ingest writes
/// `f(k: v, **h)` this way. A bare `**h` stays unplaced, as before: its
/// keys are not known at the call.
fn keyword_splat(value: &Expr) -> Option<(Vec<(Symbol, Ty)>, Ty)> {
    let symbol_values = |ty: &Option<Ty>| match ty {
        Some(Ty::Hash { key, value }) if matches!(**key, Ty::Sym) => Some((**value).clone()),
        _ => None,
    };
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*value.node {
        if method.as_str() == "merge" {
            let (ExprNode::Hash { entries, .. }, [other]) = (&*recv.node, args.as_slice()) else {
                return None;
            };
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: k } } = &*k.node else {
                    return None;
                };
                pairs.push((k.clone(), v.ty.clone().unwrap_or(Ty::Var { var: crate::ident::TyVar(0) })));
            }
            return Some((pairs, symbol_values(&other.ty)?));
        }
    }
    None
}

// Keyed by side, not by `(class, name)`: `def self.get` and `def get` are two methods whose rows must not mix.
pub(super) type ParamKey = (ClassId, Symbol, MethodReceiver);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SiteRecv {
    Instance,
    Class,
    // Not `Class`: `x.class` also reaches class methods a gem defines, which no `def` here owns.
    DotClass,
}

type SendSite = (ClassId, Symbol, Vec<Ty>, SiteKeywords, SiteRecv);

/// A call's trailing keyword arguments as `collect_send_sites` saw them.
#[derive(Clone, Debug)]
struct SiteKeywords {
    /// The last argument is the keyword group (`k: v` or `**splat`).
    group: bool,
    /// Its `key: value` pairs, by name; empty when any key is not a
    /// literal symbol (a `**splat` included).
    keys: Vec<(Symbol, Ty)>,
    /// `**{k: v}.merge(h)` with `h: Hash[Symbol, V]`: `V`, the most any
    /// keyword the literal does not name can receive. `keys` then holds
    /// the literal's pairs.
    splat: Option<Ty>,
    /// The opaque final `**` can override the explicit pairs, so they are
    /// not type evidence for the receiving named parameters.
    anonymous_forward: bool,
}

/// A method's declared parameter slots, in declaration order, as
/// `place_keyword_args` reads them.
#[derive(Debug, PartialEq)]
struct ParamShape {
    slots: Vec<(Symbol, ParamKind)>,
    /// True when every keyword in `slots` is still declared a keyword,
    /// so a call's key binds only to a `ParamKind::Keyword` slot. False
    /// for shapes whose optional keywords ingest may have lowered to
    /// positionals-with-default, which a key must still find by name.
    keywords_by_kind: bool,
}

/// A parameter's type from what the call sites passed AND what its
/// default is: an optional parameter no caller passes IS its default,
/// and one some callers pass is the union. `None` when neither says
/// anything (an observation of `Var` is no observation — see
/// `place_keyword_args`).
/// The type of a captured block parameter (`def switch_locale(&action)`):
/// a callable whose arguments and answer are unknown here. `action.call`
/// answers `untyped`, and passing it on (`I18n.with_locale(locale,
/// &action)`) is a read of a bound local rather than of nothing.
fn captured_block_ty() -> Ty {
    Ty::Fn {
        params: vec![],
        block: None,
        ret: Box::new(Ty::Untyped),
        effects: crate::effect::EffectSet::default(),
    }
}

/// What a parameter is typed as inside its method, given what its
/// signature declares and what the call sites were seen to pass.
///
/// A declaration that is fully spelled out IS the parameter's type: the
/// method body is written against it, and a call site that disagrees
/// (a `nil` the analysis inferred because the only value it saw assigned
/// was `nil`, an empty `[]` literal) is a fact about the caller, not
/// about the parameter. Before this, the observation won whenever there
/// was one, so `#: (FatalError error) -> void` above a helper that one
/// caller passed a nil-typed attribute to typed `error` as `nil`, and
/// `error.message` failed to dispatch on it.
///
/// A declaration that leaves something `untyped` (`Hash[Symbol,
/// untyped]`, a bare `Array`) is a hint the observation may sharpen, so
/// there the observation still wins when it is something, and the
/// declaration fills in where the call sites said nothing.
fn prefer_declared(declared: Option<Ty>, observed: Option<Ty>) -> Option<Ty> {
    match (declared, observed) {
        (Some(d), _) if !d.mentions_unknown() => Some(d),
        (_, Some(o)) if !matches!(o, Ty::Var { .. }) => Some(o),
        (d, o) => d.or(o),
    }
}

fn param_ty_with_default(observed: Option<Ty>, param: &crate::dialect::Param) -> Option<Ty> {
    let observed = observed.filter(|t| !matches!(t, Ty::Var { .. }));
    let default = param
        .default
        .as_ref()
        .and_then(|d| d.ty.clone())
        .filter(|t| !matches!(t, Ty::Var { .. }));
    match (observed, default) {
        (Some(o), Some(d)) => Some(crate::analyze::body::union_of(o, d)),
        (Some(o), None) => Some(o),
        (None, Some(d)) => Some(d),
        (None, None) => None,
    }
}

pub(crate) fn controller_includes(controller: &Controller) -> Vec<ClassId> {
    controller_include_groups(controller).into_iter().flatten().collect()
}

/// The controller's `include` (and `prepend`) statements, one inner
/// list per statement in source order, each in the order its arguments
/// were written. The grouping is what the filter-registration order
/// needs: Ruby processes one statement's arguments last-first, but
/// statements first-to-last.
///
/// `prepend Mod` is folded in here too — same membership (a
/// prepended module's directly-defined instance methods, and any
/// concern filters it exports, become reachable exactly like an
/// included module's), but NOT the same priority: Ruby puts a
/// prepended module AHEAD of the class in the method-resolution
/// order, so it should win a name collision with the class's own
/// method, and this walk doesn't distinguish that — a prepended
/// module that redefines a name the controller also defines itself
/// resolves to the controller's own version here, the wrong one. See
/// `CONSUMED_CONTROLLER_MACROS`'s `"prepend"` entry for why that's an
/// accepted gap rather than a blocker: no controller in this
/// codebase's fixtures collides that way.
pub(crate) fn controller_include_groups(controller: &Controller) -> Vec<Vec<ClassId>> {
    let mut out = Vec::new();
    for item in &controller.body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if !matches!(method.as_str(), "include" | "prepend") {
            continue;
        }
        let mut group = Vec::new();
        for arg in args {
            if let ExprNode::Const { path } = &*arg.node {
                let joined =
                    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                group.push(ClassId(Symbol::from(joined)));
            }
        }
        if !group.is_empty() {
            out.push(group);
        }
    }
    out
}

fn record_const(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
    let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node else {
        return;
    };
    let Some(last) = path.last() else { return };
    if let Some(ty) = value.ty.clone() {
        out.insert(last.clone(), ty);
    }
}

/// Remove the `Nil` arm from a union. A bare `Nil` (or a union that
/// was nothing but `Nil`) is preserved — there's no non-nil shape to
/// fall back to. Used when building the controller-wide ivar base,
/// where the find-then-guard idiom makes nilable ivars effectively
/// non-nil on the path that reaches a cross-method reader.
/// A method's return type is the union of every `return X` value type
/// reachable in its body PLUS the tail (implicit-return) expression's
/// type. The body-typer types a `return` *expression* as `Bottom` — it
/// diverges at that source position — so a method whose tail diverges
/// (every path `return`s, or the tail is a `case`/`begin` whose arms
/// all return) reports `body.ty == Bottom` even though the early
/// `return`s carry the real type. Reading only `body.ty` then harvests
/// `Bottom`, and a caller's `result[:k]` fails dispatch on `Bottom`.
/// Collect the returns and union them with the non-`Bottom` tail.
/// `name=` — an attribute writer, as distinct from the comparison
/// operators that also end in `=`.
pub(crate) fn is_setter_name(name: &Symbol) -> bool {
    let n = name.as_str();
    n.ends_with('=')
        && !matches!(n, "==" | "!=" | "<=" | ">=" | "===" | "[]=")
        && n.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

/// A body that is nothing but `raise NotImplementedError` — the abstract
/// hook a concern or base class declares for its includer or subclass
/// to implement (`def set_class; raise NotImplementedError, "..."; end`).
fn is_abstract_body(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.len() == 1 && is_abstract_body(&exprs[0]),
        ExprNode::Send { recv: None, method, args, .. } if matches!(method.as_str(), "raise" | "fail") => {
            let names_it = |e: &Expr| match &*e.node {
                ExprNode::Const { path } => path.last().is_some_and(|s| s.as_str() == "NotImplementedError"),
                ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "new" => {
                    matches!(&*r.node, ExprNode::Const { path } if path.last().is_some_and(|s| s.as_str() == "NotImplementedError"))
                }
                _ => false,
            };
            args.first().is_some_and(names_it)
        }
        _ => false,
    }
}

fn effective_return_ty(body: &Expr) -> Option<Ty> {
    let mut tys: Vec<Ty> = Vec::new();
    let mut saw_return = false;
    collect_return_types(body, &mut tys, &mut saw_return);
    // Tail (implicit return). Drop `Bottom` so an all-diverging tail
    // doesn't poison the union; keep everything else.
    if let Some(t) = &body.ty {
        if !matches!(t, Ty::Bottom) {
            tys.push(t.clone());
        }
    }
    if tys.is_empty() {
        // A method that reaches a `return` does not diverge, whatever
        // the returned value's shape. Harvesting `Bottom` here is not
        // a gap but a LIE, and dispatch acts on it: campfire's
        // `Fetch#fetch_content_type` is `request(url, Head, ip:) { |r|
        // return r["Content-Type"] }` over an untyped `r`, so every
        // collected type was open and the tail (`raise`) was `Bottom`
        // — the caller's `…&.downcase` then saw a receiver typed
        // exactly `Nil` and failed dispatch. Answer `unknown` instead,
        // which is what "it returns something we can't name" means.
        if saw_return && matches!(body.ty, Some(Ty::Bottom)) {
            return Some(crate::analyze::body::unknown());
        }
        // An abstract hook returns whatever its implementation does, not
        // `Bottom`: harvesting that made every call on the result (`set_class.new`)
        // a dispatch failure on an unreachable type.
        if !saw_return && is_abstract_body(body) {
            return Some(Ty::Untyped);
        }
        // Nothing usable collected — preserve prior behavior so the
        // `Var`/`Bottom`/`None` fallbacks downstream are unchanged.
        return body.ty.clone();
    }
    Some(crate::analyze::body::union_many(tys))
}

/// Collect the value type of every `return X` reachable from `expr`
/// without crossing a closure boundary. `Bottom`/`Var` values are
/// skipped (no usable shape).
///
/// A block attached to a call is NOT such a boundary. `return` inside
/// `do…end` / `{ }` exits the enclosing METHOD, and that is how a
/// value escapes a yielding helper: campfire's
/// `Opengraph::Fetch#fetch_document` is `request(url, Get, ip:) { |r|
/// return body_if_acceptable(r) }`, and `request` itself ends in
/// `raise TooManyRedirectsError`. Reading only the call's own type
/// harvested `Bottom`, so `location.read_html.force_encoding` and
/// `…fetch_content_type&.downcase` both failed dispatch on `Nil`.
///
/// The boundary is a `Lambda` in VALUE position (`scope :x, -> { … }`,
/// an argument, an assignment) — a stabby lambda's `return` returns
/// from the lambda. Blocks and lambdas share one IR node, so the
/// distinction is structural: a `Lambda` reached as a call's `block`
/// is a block; a `Lambda` reached any other way is a closure. Two
/// receivers make a `{ }` block a closure anyway and are named:
/// `lambda` (a lambda's `return` is lambda-local) and `define_method`
/// (the `return` belongs to the method being defined).
fn collect_return_types(expr: &Expr, out: &mut Vec<Ty>, saw: &mut bool) {
    match &*expr.node {
        ExprNode::Return { value } => {
            *saw = true;
            if let Some(t) = &value.ty {
                if !t.is_open() {
                    out.push(t.clone());
                }
            }
            collect_return_types(value, out, saw);
        }
        // A `Lambda` reached here is in value position — a closure
        // boundary. The `Send`/`Apply` arms below step past the node
        // for the block case, so this arm never sees a block.
        ExprNode::Lambda { .. } => {}

        ExprNode::Send { recv, args, block, method, .. } => {
            if let Some(r) = recv {
                collect_return_types(r, out, saw);
            }
            for a in args {
                collect_return_types(a, out, saw);
            }
            // `lambda { return }` is lambda-local, and
            // `define_method(:x) { return }` belongs to the method
            // being defined. Every other block's `return` is ours.
            if !matches!(method.as_str(), "lambda" | "define_method") {
                if let Some(b) = block {
                    collect_block_return_types(b, out, saw);
                }
            }
        }
        ExprNode::Apply { fun, args, block } => {
            collect_return_types(fun, out, saw);
            for a in args {
                collect_return_types(a, out, saw);
            }
            if let Some(b) = block {
                collect_block_return_types(b, out, saw);
            }
        }

        _ => expr.node.for_each_child(&mut |c| collect_return_types(c, out, saw)),
    }
}

/// Walk a call's block body for `return`s that belong to the enclosing
/// method. Steps past the `Lambda` node deliberately: reaching the body
/// through `collect_return_types` would hit the closure-boundary arm.
fn collect_block_return_types(block: &Expr, out: &mut Vec<Ty>, saw: &mut bool) {
    match &*block.node {
        ExprNode::Lambda { body, .. } => collect_return_types(body, out, saw),
        // A block passed as `&blk` (or any non-literal block operand)
        // has no body here.
        _ => {}
    }
}

/// Ivars a FRAMEWORK method assigns, which no `@x = …` in the action
/// body can show.
///
/// `extract_ivar_assignments` reads syntax, and the syntax is honest
/// about everything the app writes. It cannot see a runtime method that
/// assigns on the controller's behalf: geared_pagination's
/// `set_page_and_extract_portion_from` sets `@page` inside
/// `runtime/ruby/action_controller/pagination.rb`, the gem exposes no
/// `page` reader, and the VIEW is the only consumer — so every template
/// reading `@page.records` / `@page.last?` / `@page.next_param` reported
/// `@page has no known type`. Six of campfire's, across four templates.
///
/// One entry, deliberately: this is a table of framework methods whose
/// whole purpose is the assignment, not a general effect analysis. A
/// method that merely *might* touch an ivar does not belong here — the
/// binding it writes is trusted downstream.
fn bind_framework_assigned_ivars(body: &Expr, ivars: &mut HashMap<Symbol, Ty>) {
    fn walk(e: &Expr, out: &mut HashMap<Symbol, Ty>) {
        if let ExprNode::Send { method, .. } = &*e.node {
            if method.as_str() == "set_page_and_extract_portion_from" {
                out.entry(Symbol::from("page")).or_insert_with(|| Ty::Class {
                    id: ClassId(Symbol::from("ActionController::Page")),
                    args: vec![],
                });
            }
        }
        e.node.for_each_child(&mut |c| walk(c, out));
    }
    walk(body, ivars);
}

/// `<Const>.<name> = <value>` where `<Const>` is one of `targets`, keyed
/// class → attribute → the value's type. Unions across write sites and
/// ignores a write whose value carries no shape, so one untyped setter
/// call cannot erase what another site established.
fn collect_const_attr_writes(
    expr: &crate::expr::Expr,
    targets: &std::collections::HashSet<&ClassId>,
    out: &mut HashMap<ClassId, HashMap<Symbol, Ty>>,
) {
    // `Current.user = bot` arrives as a SEND of `user=`, not as an
    // `Assign` — prism spells a receiver-ful attribute write as a call,
    // and only the compound forms (`||=`) become `LValue::Attr`.
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node {
        if let (Some(attr), 1, ExprNode::Const { path }) =
            (method.as_str().strip_suffix('='), args.len(), &*recv.node)
        {
            let id = ClassId(Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str(),
            ));
            if let Some(target) = targets.get(&id) {
                if let Some(ty) = args[0].ty.as_ref().filter(|t| !t.is_open()) {
                    let entry = out.entry((*target).clone()).or_default();
                    let name = Symbol::from(attr);
                    let merged = match entry.remove(&name) {
                        Some(prev) if prev == *ty => prev,
                        Some(prev) => crate::analyze::body::union_of(prev, ty.clone()),
                        None => ty.clone(),
                    };
                    entry.insert(name, merged);
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_const_attr_writes(c, targets, out));
}

/// A binding worth carrying to a SUBCLASS: one that is an answer.
///
/// `is_unknown` is not enough. `Account | untyped` is neither `Var` nor
/// `Untyped`, so it passes that test — and then it unions into the
/// subclass's controller-wide environment and widens a sibling binding
/// of the same name that WAS clean. Mastodon's `@account` hovered as
/// `Account` before this persistence existed and as `Account | untyped`
/// after, which is the IDE's whole product getting worse to close a
/// campfire error.
///
/// So: no open or gradual arm anywhere in a top-level union, and not
/// one itself. The persistence exists to carry a shaped answer down an
/// inheritance chain; a half-answer is what the Phase-A harvest already
/// provides, and layering one can only subtract.
fn is_clean_binding(ty: &Ty) -> bool {
    match ty {
        Ty::Union { variants } => variants.iter().all(|v| !v.is_unknown()),
        t => !t.is_unknown(),
    }
}

/// The ivars `initialize` assigns as statements of its own, across `methods`.
fn ivars_initialized_by<'a>(
    methods: impl Iterator<Item = &'a crate::dialect::MethodDef>,
) -> std::collections::HashSet<Symbol> {
    methods
        .filter(|m| {
            m.name.as_str() == "initialize"
                && m.receiver == crate::dialect::MethodReceiver::Instance
        })
        .flat_map(|m| ivars_assigned_by_statement(&m.body))
        .collect()
}

/// The ivars a method body assigns as statements of its own -- not inside
/// a branch, loop or block, where the write may never run. After the method
/// returns, each of these is set.
fn ivars_assigned_by_statement(body: &Expr) -> Vec<Symbol> {
    let statements: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in statements {
        match &*stmt.node {
            ExprNode::Assign { target: LValue::Ivar { name }, .. } => out.push(name.clone()),
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    if let LValue::Ivar { name } = t {
                        out.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Harvest `@ivar = expr` / OpAssign / MultiAssign writes from a typed
/// body, union-merging repeated assignments. Used by the analyzer's
/// two-pass library typing and by the Spinel AR RBS probe.
pub fn extract_ivar_assignments(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
    extract_ivar_assignments_in(expr, out, &ivar_set::IvarNameEnv::NONE);
}

pub(crate) fn extract_ivar_assignments_in(
    expr: &Expr,
    out: &mut HashMap<Symbol, Ty>,
    env: &ivar_set::IvarNameEnv<'_>,
) {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            if let Some(ty) = value.ty.clone() {
                // Union with existing entry so repeated assignments to
                // the same ivar accumulate (rather than the last write
                // winning). Mirrors the simple flow-sensitive join.
                let merged = match out.remove(name) {
                    Some(prev) => crate::analyze::body::union_of(prev, ty),
                    None => ty,
                };
                out.insert(name.clone(), fixpoint_bound::bound(merged));
            }
        }
        // Short-circuit compound assignment to an ivar (`@x ||= y`,
        // `@x &&= y`) — the memoization idiom. Recorded the same way as
        // a plain assignment so a controller's `@story ||= Story.find(..)`
        // still flows its type to before_action seeds and views.
        ExprNode::OpAssign { target: LValue::Ivar { name }, value, .. } => {
            if let Some(ty) = value.ty.clone() {
                let merged = match out.remove(name) {
                    Some(prev) => crate::analyze::body::union_of(prev, ty),
                    None => ty,
                };
                out.insert(name.clone(), fixpoint_bound::bound(merged));
            }
        }
        // `@a, @b = expr` — destructuring assignment. Each ivar target
        // takes its per-position type from the RHS (Array element /
        // Tuple slot / Untyped escape) so a controller's
        // `@stories, @show_more = paginate(...)` flows `@stories` into
        // the view-ivar seed and the controller-wide ivar union.
        // Without this arm the targets are invisible to every harvest.
        ExprNode::MultiAssign { targets, value } => {
            for (i, target) in targets.iter().enumerate() {
                if let LValue::Ivar { name } = target {
                    if let Some(ty) =
                        crate::analyze::body::multiassign_target_ty(&value.ty, i)
                    {
                        let merged = match out.remove(name) {
                            Some(prev) => crate::analyze::body::union_of(prev, ty),
                            None => ty,
                        };
                        out.insert(name.clone(), fixpoint_bound::bound(merged));
                    }
                }
            }
            extract_ivar_assignments_in(value, out, env);
        }
        // `@hash[k] ||= v` / `@hash[k] = v` in the OpAssign / Assign
        // Index forms (the `||=` accumulator idiom — `@hat_groups[k] ||=
        // []` — and plain index-assign). Widen the ivar hash's value
        // type from the written element so a cross-method or view read
        // (`@hat_groups[hg].sort_by`) sees `Array`, not `Var`. The plain
        // `[]=` Send form is handled by the arm below.
        ExprNode::Assign { target: LValue::Index { recv, index }, value }
        | ExprNode::OpAssign { target: LValue::Index { recv, index }, value, .. } => {
            if let ExprNode::Ivar { name } = &*recv.node {
                if let Some(v_ty) = &value.ty {
                    widen_hash_ivar_value(out, name, v_ty);
                }
            }
            extract_ivar_assignments_in(recv, out, env);
            extract_ivar_assignments_in(index, out, env);
            extract_ivar_assignments_in(value, out, env);
        }
        // `@hash[k] = v` parses as Send to `[]=` with @hash as the
        // receiver. The Hash literal `@hash = {}` only seeds key/value
        // as fresh type variables; the actual stored value-type lives
        // in the `[]=` writes. Widen so downstream reads (`raw =
        // @hash[k]`) get a concrete element type instead of TyVar.
        // Mirrors `crystal::library::collect_ivar_assignments`.
        ExprNode::Send { recv: Some(recv), method, args, block, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Ivar { name } = &*recv.node {
                if let Some(v_ty) = &args[1].ty {
                    widen_hash_ivar_value(out, name, v_ty);
                }
            }
            extract_ivar_assignments_in(recv, out, env);
            for a in args {
                extract_ivar_assignments_in(a, out, env);
            }
            if let Some(b) = block {
                extract_ivar_assignments_in(b, out, env);
            }
        }
        // Walk into other Send forms so nested `[]=` writes (e.g.
        // inside a method-chain receiver or arg expression) still
        // get found. Cheap; the special-case above already handles
        // the widening — this is purely recursive descent.
        //
        // `instance_variable_set` is Kernel, not `@ivar =`, so the
        // Assign arm never sees it. Fold a statically resolvable name
        // here so the controller→view channel harvests the write.
        ExprNode::Send { recv, method, args, block, .. } => {
            ivar_set::harvest_ivar_set(recv, method, args, env, out);
            if let Some(r) = recv {
                extract_ivar_assignments_in(r, out, env);
            }
            for a in args {
                extract_ivar_assignments_in(a, out, env);
            }
            if let Some(b) = block {
                extract_ivar_assignments_in(b, out, env);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                extract_ivar_assignments_in(e, out, env);
            }
        }
        // The condition is walked too: `if (@message = Model.find(..))`
        // assigns the ivar inside the test, a common `find_*` filter
        // idiom. Without visiting `cond`, that ivar never gets typed.
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_ivar_assignments_in(cond, out, env);
            extract_ivar_assignments_in(then_branch, out, env);
            extract_ivar_assignments_in(else_branch, out, env);
        }
        // `while cond; body; end` — body may contain `@hash[k] = v`
        // (Parameters' initialize loop). Without this arm, ivar
        // value-type widening from `[]=` writes inside loops is
        // invisible. The condition is walked for the same
        // assignment-in-test reason as `If`.
        ExprNode::While { cond, body, .. } => {
            extract_ivar_assignments_in(cond, out, env);
            extract_ivar_assignments_in(body, out, env);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            extract_ivar_assignments_in(expr, out, env);
            extract_ivar_assignments_in(fallback, out, env);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms {
                extract_ivar_assignments_in(&arm.body, out, env);
            }
        }
        // `a && (@x = y)` / `a || (@x = y)` — an ivar assigned inside a
        // boolean chain (the `find_*` guard idiom). Descend both sides
        // so the buried assignment still gets typed. (Compound `@x ||= y`
        // is `OpAssign`, handled by its own arm above — not `BoolOp`.)
        ExprNode::BoolOp { left, right, .. } => {
            extract_ivar_assignments_in(left, out, env);
            extract_ivar_assignments_in(right, out, env);
        }
        // Rescue/ensure and lifecycle constructs may also contain
        // assignments; recurse to catch them.
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            extract_ivar_assignments_in(body, out, env);
            for r in rescues {
                extract_ivar_assignments_in(&r.body, out, env);
            }
            if let Some(e) = else_branch {
                extract_ivar_assignments_in(e, out, env);
            }
            if let Some(e) = ensure {
                extract_ivar_assignments_in(e, out, env);
            }
        }
        ExprNode::Lambda { body, .. } => extract_ivar_assignments_in(body, out, env),
        ExprNode::Return { value } => extract_ivar_assignments_in(value, out, env),
        // Any other assignment target (local var, constant, attribute) that
        // wasn't matched by the ivar/index arms above. We record no ivar for
        // the target itself, but the RHS can still assign ivars inside a
        // block — `content = Rails.cache.fetch(k) { @newest = ...;
        // @users_by_parent = ... }` (lobsters' `UsersController#tree`).
        // Without descending here, those ivars are invisible to the
        // controller→view channel and read as `ivar_unresolved` in the view.
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            extract_ivar_assignments_in(value, out, env);
        }
        // `let x = <expr with block> in body` — same reasoning as the local
        // assignment above; walk both the bound value and the body.
        ExprNode::Let { value, body, .. } => {
            extract_ivar_assignments_in(value, out, env);
            extract_ivar_assignments_in(body, out, env);
        }
        _ => {}
    }
}

/// Widen an existing Hash ivar's value-type to include `incoming`.
///
/// Only fires when the existing entry is `Hash { .. }` — if the ivar
/// was assigned a typed class instance (e.g. `@hash = Foo.new`), the
/// class's own `[]=` method shouldn't retype the ivar to a generic
/// Hash. The widening exists to grow empty-Hash-literal types from
/// observed `[]=` writes, not to retype class instances.
///
/// When the existing value-side is a fresh type variable (`Ty::Var`),
/// it's replaced rather than unioned — the variable came from the
/// empty-literal `{}` and carries no information. Same for the key
/// side: a TyVar key collapses to `Str` since `[]=` writes use
/// `key.to_s` strings in the runtime conventions here.
fn widen_hash_ivar_value(out: &mut HashMap<Symbol, Ty>, name: &Symbol, incoming: &Ty) {
    let Some(existing) = out.get(name) else {
        // No prior entry — seed a fresh Hash[Str, incoming]. Matches
        // the Crystal collector's "fresh entry" branch.
        out.insert(
            name.clone(),
            fixpoint_bound::bound(Ty::Hash { key: Box::new(Ty::Str), value: Box::new(incoming.clone()) }),
        );
        return;
    };
    let Ty::Hash { key, value } = existing else {
        return;
    };
    let key = if matches!(**key, Ty::Var { .. }) {
        Box::new(Ty::Str)
    } else {
        key.clone()
    };
    let value = if matches!(**value, Ty::Var { .. }) {
        Box::new(incoming.clone())
    } else {
        // The general widening is exactly the canonical type join.
        Box::new(crate::analyze::body::union_of((**value).clone(), incoming.clone()))
    };
    out.insert(name.clone(), fixpoint_bound::bound(Ty::Hash { key, value }));
}

// Diagnostic emission -----------------------------------------------------

/// Re-exports: the shared diagnostic types live in `crate::diagnostic`
/// so the body-typer can annotate `Expr.diagnostic` without a
/// dependency cycle. External callers (tests, future CLIs) continue
/// to import them from `roundhouse::analyze` as before.
pub use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};

/// Register `typed_store` accessors (the `activerecord-typedstore` gem) as
/// typed instance methods. A `typed_store :col do |s| s.string :name … end`
/// block declares attributes backed by one serialized column — real methods
/// at runtime, but absent from `db/schema.rb`, so the schema-derived
/// attribute pass never sees them. Each `s.<type> :name` adds a getter
/// (`name`), a setter (`name=`), and for booleans a predicate (`name?`).
/// Purely additive — fires only for models that declare such a block.
/// Register plain `attr_accessor` / `attr_reader` / `attr_writer`
/// declarations in a model body as instance methods. These are virtual
/// attributes (e.g. `attr_accessor :previewing, :vote` on Story) — real
/// methods at runtime but absent from `db/schema.rb` and untyped, so they
/// resolve to `Untyped` (the gradual escape). `attr_reader` registers a
/// getter, `attr_writer` a setter, `attr_accessor` both. Additive:
/// `or_insert` so a schema column, typed_store, or harvested method of the
/// same name keeps its more precise type.
/// Collect the ivar names declared by `attr_accessor` / `attr_reader`
/// / `attr_writer` in a model body. These are virtual attributes
/// (`attr_accessor :edit_user_id`) — real ivars at runtime, absent
/// from the schema, of unknown (gradual) type. Seeding them lets a
/// direct `@edit_user_id` read in a model method resolve as `Untyped`
/// rather than `Var`. (`register_attr_accessors` registers the
/// reader/writer *methods*; this is the ivar-seed companion.)
fn collect_attr_accessor_names(body: &[ModelBodyItem]) -> Vec<Symbol> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv, method, args, .. } = &*expr.node else { continue };
        if recv.is_some() {
            continue;
        }
        if !matches!(method.as_str(), "attr_accessor" | "attr_reader" | "attr_writer") {
            continue;
        }
        for arg in args {
            if let Some(name) = symbol_arg(arg) {
                out.push(name.clone());
            }
        }
    }
    out
}

/// `Mailer.with(k: v, …)` call sites across the app, folded to one
/// [`Row`] per mailer: each key's type is the union over sites of the
/// argument's inferred type (untyped arguments contribute nothing, so
/// a site the current pass has not typed yet is simply absent until
/// the fixpoint re-runs). The receiver must name a mailer class
/// exactly — `Mailer.with` chained onto anything else is not this.
fn harvest_mailer_with_params(
    app: &App,
    mailers: &std::collections::HashSet<ClassId>,
) -> HashMap<ClassId, crate::ty::Row> {
    use crate::expr::Literal;
    let mut out: HashMap<ClassId, crate::ty::Row> = HashMap::new();
    let mut visit = |e: &Expr| {
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else { return };
        if method.as_str() != "with" {
            return;
        }
        let ExprNode::Const { path } = &*recv.node else { return };
        let id = ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str(),
        ));
        if !mailers.contains(&id) {
            return;
        }
        let row = out.entry(id).or_insert_with(crate::ty::Row::closed);
        for arg in args {
            let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { continue };
                let Some(ty) = v.ty.clone() else { continue };
                if matches!(ty, Ty::Var { .. }) {
                    continue;
                }
                let merged = match row.fields.get(key) {
                    Some(prev) => crate::analyze::body::union_of(prev.clone(), ty),
                    None => ty,
                };
                row.fields.insert(key.clone(), merged);
            }
        }
    };
    let mut walk_all = |body: &Expr| {
        walk_expr(body, &mut visit);
    };
    for model in &app.models {
        for method in model.methods() {
            walk_all(&method.body);
        }
    }
    for controller in &app.controllers {
        for action in controller.actions() {
            walk_all(&action.body);
        }
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            walk_all(&method.body);
        }
    }
    out
}

/// Pre-order walk over an expression tree.
fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| walk_expr(c, f));
}

/// Register the methods `has_secure_password` generates. The macro
/// (default attribute `:password`, or a custom one passed as the first
/// symbol) adds a write-only virtual attribute and an authenticator:
///   - `<attr>=` / `<attr>_confirmation=` — writers taking the plaintext
///     (Str); they return the assigned value.
///   - `authenticate` (default) / `authenticate_<attr>` (custom) — checks
///     the plaintext against the digest, returning the record on success
///     or false; typed as the model instance (the dominant truthy use).
/// `or_insert`, so a real `def` of the same name still wins.
fn register_has_secure_password(
    body: &[ModelBodyItem],
    methods: &mut HashMap<Symbol, Ty>,
    class_methods: &mut HashMap<Symbol, Ty>,
    self_ty: &Ty,
) {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "has_secure_password" {
            continue;
        }
        // First positional symbol is the attribute name (kwargs like
        // `validations: false` are Hash args, skipped by symbol_arg);
        // default is `password`.
        let attr = args
            .iter()
            .find_map(|a| symbol_arg(a))
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "password".to_string());
        methods
            .entry(Symbol::from(format!("{attr}=")))
            .or_insert(Ty::Str);
        methods
            .entry(Symbol::from(format!("{attr}_confirmation=")))
            .or_insert(Ty::Str);
        let auth = if attr == "password" {
            "authenticate".to_string()
        } else {
            format!("authenticate_{attr}")
        };
        methods.entry(Symbol::from(auth)).or_insert(self_ty.clone());
        // Rails 7.1+: the password-reset token round-trip the
        // authentication generator's PasswordsController and mailer
        // use — `user.password_reset_token` (a signed token, Str), its
        // lifetime, `authenticate_password`, and the class-side finders
        // (`find_by_password_reset_token!` raises, the bang-less form
        // answers nil).
        methods
            .entry(Symbol::from(format!("{attr}_reset_token")))
            .or_insert(Ty::Str);
        methods
            .entry(Symbol::from(format!("{attr}_reset_token_expires_in")))
            .or_insert(Ty::Int);
        methods
            .entry(Symbol::from(format!("authenticate_{attr}")))
            .or_insert(self_ty.clone());
        methods
            .entry(Symbol::from(format!("{attr}_salt")))
            .or_insert(Ty::Str);
        methods
            .entry(Symbol::from(format!("{attr}_challenge=")))
            .or_insert(Ty::Str);
        class_methods
            .entry(Symbol::from(format!("find_by_{attr}_reset_token")))
            .or_insert(Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] });
        class_methods
            .entry(Symbol::from(format!("find_by_{attr}_reset_token!")))
            .or_insert(self_ty.clone());
    }
}

/// Register the methods `generates_token_for :purpose` (Rails 7.1)
/// generates: `record.generate_token_for(:purpose)` answers a signed
/// String, `Model.find_by_token_for(:purpose, token)` the record or
/// nil, and the bang form the record (raising). One claimed
/// declaration is enough — the purpose is an argument, not part of the
/// method name.
fn register_generates_token_for(
    model: &crate::dialect::Model,
    methods: &mut HashMap<Symbol, Ty>,
    class_methods: &mut HashMap<Symbol, Ty>,
    self_ty: &Ty,
) {
    // Only the declarations lower::generates_token_for expands: typing
    // one it leaves unclaimed would quiet `check` over a method nothing
    // defines.
    if crate::lower::generates_token_for::token_for_decls(model).is_empty() {
        return;
    }
    methods.entry(Symbol::from("generate_token_for")).or_insert(Ty::Str);
    class_methods
        .entry(Symbol::from("find_by_token_for"))
        .or_insert(Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] });
    class_methods.entry(Symbol::from("find_by_token_for!")).or_insert(self_ty.clone());
}

/// Register the methods `has_rich_text :body` generates, method for
/// method as `lower::rich_text::push_owner_methods` expands them:
/// `rich_text_body` / `build_rich_text_body` (the scoped has_one and
/// its builder) and `body` answer the `ActionText::RichText` record;
/// `body?` a Bool; `body=` takes a String or a Content and answers what
/// it was given. The expansion happens at LOWERING; without this
/// registration a typed read of `message.body` — which the
/// `collection:` render of the message partial now produces — failed
/// dispatch on `Message`, where before the local was untyped and the
/// read a silent gradual escape. `or_insert`, so a real `def` wins.
fn register_has_rich_text(model: &crate::dialect::Model, methods: &mut HashMap<Symbol, Ty>) {
    use crate::lower::rich_text;
    // The synthesized record class itself: its `body` reads back as a
    // Content (`serialize :body, coder: ActionText::Content`) and it
    // delegates the Content surface — `push_record_methods`' list.
    if rich_text::is_record_model(model) {
        let content = Ty::Class { id: rich_text::content_class(), args: vec![] };
        methods.insert(Symbol::from("body"), content);
        methods.entry(Symbol::from("body=")).or_insert(Ty::Str);
        for (name, ret) in [
            ("to_s", Ty::Str),
            ("to_plain_text", Ty::Str),
            ("to_html", Ty::Str),
            ("to_trix_html", Ty::Str),
            ("blank?", Ty::Bool),
            ("empty?", Ty::Bool),
            ("present?", Ty::Bool),
        ] {
            methods.entry(Symbol::from(name)).or_insert(ret);
        }
        return;
    }
    let record = Ty::Class { id: rich_text::record_class(), args: vec![] };
    for (_, attr) in rich_text::rich_text_attrs(model) {
        let a = attr.as_str();
        for name in [format!("rich_text_{a}"), format!("build_rich_text_{a}"), a.to_string()] {
            methods.entry(Symbol::from(name)).or_insert(record.clone());
        }
        methods.entry(Symbol::from(format!("{a}?"))).or_insert(Ty::Bool);
        methods.entry(Symbol::from(format!("rich_text_{a}_loaded?"))).or_insert(Ty::Bool);
        methods.entry(Symbol::from(format!("{a}="))).or_insert(Ty::Untyped);
    }
}

/// Register methods `has_markdown :name` generates — mirror of
/// [`register_has_rich_text`] for the plain-text association lowerer.
fn register_plain_text_attr(
    model: &crate::dialect::Model,
    schema: &crate::schema::Schema,
    methods: &mut HashMap<Symbol, Ty>,
) {
    use crate::lower::plain_text_attr;
    if plain_text_attr::is_record_model(model) || !plain_text_attr::record_table_present(schema) {
        return;
    }
    let record = Ty::Class { id: plain_text_attr::record_class(), args: vec![] };
    for (_, attr) in plain_text_attr::plain_text_attrs(model) {
        let a = attr.as_str();
        for name in [format!("markdown_{a}"), format!("build_markdown_{a}"), a.to_string()] {
            methods.entry(Symbol::from(name)).or_insert(record.clone());
        }
        methods.entry(Symbol::from(format!("{a}?"))).or_insert(Ty::Bool);
        methods.entry(Symbol::from(format!("{a}="))).or_insert(Ty::Untyped);
    }
}

fn register_attr_accessors(body: &[ModelBodyItem], methods: &mut HashMap<Symbol, Ty>) {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv, method, args, .. } = &*expr.node else { continue };
        if recv.is_some() {
            continue;
        }
        let (reader, writer) = match method.as_str() {
            "attr_accessor" => (true, true),
            "attr_reader" => (true, false),
            "attr_writer" => (false, true),
            _ => continue,
        };
        for arg in args {
            let Some(name) = symbol_arg(arg) else { continue };
            if reader {
                methods.entry(name.clone()).or_insert(Ty::Untyped);
            }
            if writer {
                let setter = Symbol::from(format!("{}=", name.as_str()));
                methods.entry(setter).or_insert(Ty::Untyped);
            }
        }
    }
}

/// `attribute :name, :type` (ActiveModel::Attributes) declares a typed
/// virtual attribute backed by something other than a schema column
/// (a casted form field, a default-valued non-persisted value, …). It's
/// absent from the schema-derived attributes, so register reader, writer,
/// and presence predicate typed per the `:type` symbol — same shape as
/// `typed_store`, reusing its type map. A bare `attribute :name` with no
/// type, or an unrecognized type, falls back to `Untyped` (gradual).
fn register_ar_attributes(body: &[ModelBodyItem], methods: &mut HashMap<Symbol, Ty>) {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv, method, args, .. } = &*expr.node else { continue };
        if recv.is_some() || method.as_str() != "attribute" {
            continue;
        }
        let Some(name) = args.first().and_then(symbol_arg) else { continue };
        let ty = args
            .get(1)
            .and_then(symbol_arg)
            .and_then(|t| typed_store_ty(t.as_str()))
            .unwrap_or(Ty::Untyped);
        methods.entry(name.clone()).or_insert(ty.clone());
        let setter = Symbol::from(format!("{}=", name.as_str()));
        methods.entry(setter).or_insert(ty.clone());
        let predicate = Symbol::from(format!("{}?", name.as_str()));
        methods.entry(predicate).or_insert(Ty::Bool);
    }
}

/// Register the flat accessors `lower::has_json` synthesizes for a
/// `has_json :settings, key: default` declaration, and re-register the
/// column reader itself as GRADUAL.
///
/// The split is deliberate. Analyze runs on source-shaped IR, where the
/// call is still Rails' two-hop `account.settings.foo?` and the object
/// between the hops has no class of its own — nothing to register a
/// method surface on. `Untyped` is the gradual escape that lets that
/// hop resolve; `lower::has_json` then rewrites it to
/// `account.settings_foo?`, whose type IS registered here, so nothing
/// untyped survives to a target. The one visible consequence: the
/// EMITTED `settings` reader returns the serialized column text
/// (`Ty::Str`) where analyze called it untyped. That is the divergence
/// the erased accessor object costs, and it is recorded in
/// `docs/pipeline/runtime.md`.
///
/// `insert`, not `or_insert`, on the column: the schema-derived pass
/// above already registered the storage type and this is deliberately
/// overriding it.
fn register_has_json(body: &[ModelBodyItem], methods: &mut HashMap<Symbol, Ty>) {
    for decl in crate::lower::has_json::has_json_decls(body) {
        methods.insert(decl.column.clone(), Ty::Untyped);
        for a in &decl.attrs {
            let flat = crate::lower::has_json::flat_name(&decl.column, &a.name);
            let ty = a.scalar.ty();
            methods.entry(flat.clone()).or_insert(ty.clone());
            methods
                .entry(Symbol::from(format!("{}?", flat.as_str())))
                .or_insert(Ty::Bool);
            methods
                .entry(Symbol::from(format!("{}=", flat.as_str())))
                .or_insert(ty);
        }
    }
}

/// `serialize :settings, coder: JSON` (Rails' attribute serialization)
/// keeps the column's storage type in the schema (`text`/`string`) but
/// the attribute reads back as whatever the coder loads: a Hash, an
/// Array, a value object. Typing the reader and writer as the storage
/// `String` makes every use of the deserialized value (`line_items.map`,
/// `settings.fetch`) a dispatch failure on `String?`. The coder is an
/// arbitrary object (`JSON`, `VersionedSerializer.column(…)`, a custom
/// class), so the honest type is the gradual one. `insert`, not
/// `or_insert`: this deliberately overrides the schema-derived type.
fn register_serialized_columns(body: &[ModelBodyItem], methods: &mut HashMap<Symbol, Ty>) {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "serialize" {
            continue;
        }
        for name in args.iter().map_while(symbol_arg) {
            methods.insert(name.clone(), Ty::Untyped);
            methods.insert(Symbol::from(format!("{}=", name.as_str())), Ty::Untyped);
        }
    }
}

fn register_typed_store(body: &[ModelBodyItem], methods: &mut HashMap<Symbol, Ty>) {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { method, block: Some(block), .. } = &*expr.node else { continue };
        if method.as_str() == "typed_store" {
            register_typed_store_decls(block, methods);
        }
    }
}

/// Walk a `typed_store` block, registering each `s.<type> :name` declaration.
/// A recursive walk (rather than assuming the block's exact node shape) finds
/// the declarations wherever they sit; only `s.<known-type> :symbol` calls
/// match, so nothing else in the block is picked up.
fn register_typed_store_decls(expr: &Expr, methods: &mut HashMap<Symbol, Ty>) {
    if let ExprNode::Send { method, args, .. } = &*expr.node {
        if let (Some(elem_ty), Some(name)) =
            (typed_store_ty(method.as_str()), args.first().and_then(symbol_arg))
        {
            // `array: true` stores a list of the column type. `any` stays
            // `Untyped` even as an array — the element is unknown, so the
            // gradual escape covers every call (`push`/`reject!`/`each`/…)
            // without depending on the Array method registry.
            let ty = if typed_store_is_array(args) && !matches!(elem_ty, Ty::Untyped) {
                Ty::Array { elem: Box::new(elem_ty.clone()) }
            } else {
                elem_ty
            };
            methods.entry(name.clone()).or_insert(ty.clone());
            let setter = Symbol::from(format!("{}=", name.as_str()));
            methods.entry(setter).or_insert(ty);
            // typedstore generates a `name?` presence predicate for every
            // column, regardless of type — same as the schema-column loop.
            let predicate = Symbol::from(format!("{}?", name.as_str()));
            methods.entry(predicate).or_insert(Ty::Bool);
        }
    }
    expr.node.for_each_child(&mut |child| register_typed_store_decls(child, methods));
}

/// A `typed_store` column type → its Roundhouse `Ty`. `any` is the
/// untyped escape (`Ty::Untyped`); `datetime`/`time`/`date` fold into the
/// first-class `Ty::Time`. Anything unrecognized returns `None` and
/// stays unregistered.
pub(crate) fn typed_store_ty(type_method: &str) -> Option<Ty> {
    Some(match type_method {
        "string" | "text" => Ty::Str,
        "boolean" => Ty::Bool,
        "integer" | "big_integer" => Ty::Int,
        "float" | "decimal" => Ty::Float,
        "any" => Ty::Untyped,
        "datetime" | "time" | "date" => Ty::Time,
        _ => return None,
    })
}

/// True when a `typed_store` declaration carries `array: true` — the
/// column stores a list of its element type rather than a scalar.
pub(crate) fn typed_store_is_array(args: &[Expr]) -> bool {
    args.iter().any(|a| {
        let ExprNode::Hash { entries, .. } = &*a.node else { return false };
        entries.iter().any(|(k, v)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "array")
                && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
        })
    })
}

fn symbol_arg(expr: &Expr) -> Option<&Symbol> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}


#[cfg(test)]
mod typed_store_tests {
    use super::*;
    use crate::span::Span;

    fn send(method: &str, args: Vec<Expr>, block: Option<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block,
                parenthesized: false,
            },
        )
    }
    fn sym(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
        )
    }
    fn unknown_item(expr: Expr) -> ModelBodyItem {
        ModelBodyItem::Unknown { expr, leading_comments: vec![], leading_blank_line: false }
    }

    #[test]
    fn registers_string_and_boolean_accessors() {
        // typed_store :settings do |s|
        //   s.string :twitter_username
        //   s.boolean :email_replies
        // end
        let block = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    send("string", vec![sym("twitter_username")], None),
                    send("boolean", vec![sym("email_replies")], None),
                ],
            },
        );
        let body = vec![unknown_item(send("typed_store", vec![sym("settings")], Some(block)))];

        let mut methods: HashMap<Symbol, Ty> = HashMap::new();
        register_typed_store(&body, &mut methods);

        // string → getter + setter + presence predicate (typedstore
        // generates `name?` for every column, like a schema column).
        assert_eq!(methods.get(&Symbol::from("twitter_username")), Some(&Ty::Str));
        assert_eq!(methods.get(&Symbol::from("twitter_username=")), Some(&Ty::Str));
        assert_eq!(methods.get(&Symbol::from("twitter_username?")), Some(&Ty::Bool));
        // boolean → getter + setter + predicate
        assert_eq!(methods.get(&Symbol::from("email_replies")), Some(&Ty::Bool));
        assert_eq!(methods.get(&Symbol::from("email_replies=")), Some(&Ty::Bool));
        assert_eq!(methods.get(&Symbol::from("email_replies?")), Some(&Ty::Bool));
    }

    #[test]
    fn ignores_unknown_items_without_typed_store() {
        let body = vec![unknown_item(send("some_macro", vec![sym("x")], None))];
        let mut methods: HashMap<Symbol, Ty> = HashMap::new();
        register_typed_store(&body, &mut methods);
        assert!(methods.is_empty());
    }

    #[test]
    fn registers_any_and_array_columns() {
        // typed_store :settings do |s|
        //   s.any :keybase_signatures, array: true
        //   s.string :tags, array: true
        // end
        let array_kw = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    sym("array"),
                    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: true } }),
                )],
                kwargs: true,
            },
        );
        let block = Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![
                    send("any", vec![sym("keybase_signatures"), array_kw.clone()], None),
                    send("string", vec![sym("tags"), array_kw], None),
                ],
            },
        );
        let body = vec![unknown_item(send("typed_store", vec![sym("settings")], Some(block)))];

        let mut methods: HashMap<Symbol, Ty> = HashMap::new();
        register_typed_store(&body, &mut methods);

        // `any` stays the gradual escape even with `array: true` — element
        // is unknown, so Untyped (not Array<Untyped>) keeps every call live.
        assert_eq!(methods.get(&Symbol::from("keybase_signatures")), Some(&Ty::Untyped));
        assert_eq!(methods.get(&Symbol::from("keybase_signatures=")), Some(&Ty::Untyped));
        assert_eq!(methods.get(&Symbol::from("keybase_signatures?")), Some(&Ty::Bool));
        // a typed `array: true` column wraps the element type.
        assert_eq!(
            methods.get(&Symbol::from("tags")),
            Some(&Ty::Array { elem: Box::new(Ty::Str) })
        );
    }

    #[test]
    fn method_return_fallback_is_clobber_safe() {
        use crate::ident::TyVar;
        let mut t: HashMap<Symbol, Ty> = HashMap::new();

        // Resolved body → register the real return type.
        Analyzer::register_method_return(&mut t, &Symbol::from("to_html"), Some(&Ty::Str));
        assert_eq!(t.get(&Symbol::from("to_html")), Some(&Ty::Str));

        // Unresolved (None or Var) → register existence as a gradual escape.
        Analyzer::register_method_return(&mut t, &Symbol::from("current_vote"), None);
        assert_eq!(t.get(&Symbol::from("current_vote")), Some(&Ty::Untyped));
        Analyzer::register_method_return(
            &mut t,
            &Symbol::from("enabled"),
            Some(&Ty::Var { var: TyVar(0) }),
        );
        assert_eq!(t.get(&Symbol::from("enabled")), Some(&Ty::Untyped));

        // The fallback must never clobber a real type from another pass…
        Analyzer::register_method_return(&mut t, &Symbol::from("to_html"), None);
        assert_eq!(t.get(&Symbol::from("to_html")), Some(&Ty::Str));
        // …but a real type does upgrade a prior gradual fallback.
        Analyzer::register_method_return(&mut t, &Symbol::from("current_vote"), Some(&Ty::Bool));
        assert_eq!(t.get(&Symbol::from("current_vote")), Some(&Ty::Bool));
    }
}

#[cfg(test)]
mod rbs_ingestion_tests {
    use super::*;

    fn fn_ty_returning(ret: Ty) -> Ty {
        Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(ret),
            effects: EffectSet::default(),
        }
    }

    #[test]
    fn analyzer_applies_rbs_signatures_to_user_class() {
        // A user class not in any Rails convention: `Settings`.
        // RBS declares `theme` returns String.
        let mut app = App::new();
        let mut settings_methods: HashMap<Symbol, Ty> = HashMap::new();
        settings_methods.insert(Symbol::from("theme"), fn_ty_returning(Ty::Str));
        app.rbs_signatures
            .insert(ClassId(Symbol::from("Settings")), settings_methods);

        let analyzer = Analyzer::new(&app);
        let settings = analyzer
            .classes
            .get(&ClassId(Symbol::from("Settings")))
            .expect("Settings class is in the analyzer's table");
        let theme = settings
            .instance_methods
            .get(&Symbol::from("theme"))
            .expect("theme method from RBS is in Settings's instance_methods");

        // Returned Ty is the Ty::Fn — the whole method type, since
        // parameterless method dispatch preserves this shape today.
        let Ty::Fn { ret, .. } = theme else {
            panic!("expected Ty::Fn for theme");
        };
        assert_eq!(**ret, Ty::Str);
    }

    #[test]
    fn analyzer_rbs_signatures_overlay_the_hardcoded_catalog() {
        // If RBS declares a method that also exists in the Rails
        // catalog, RBS wins (inserted last). Demonstrate by
        // overriding `find` on a model.
        let mut app = App::new();
        let model_name = ClassId(Symbol::from("Article"));
        let mut article_methods: HashMap<Symbol, Ty> = HashMap::new();
        // Pretend Article is a user class with a custom `find` that
        // returns a plain String (nonsense, but easy to detect).
        article_methods.insert(Symbol::from("find"), fn_ty_returning(Ty::Str));
        app.rbs_signatures.insert(model_name.clone(), article_methods);

        let analyzer = Analyzer::new(&app);
        let article = analyzer
            .classes
            .get(&model_name)
            .expect("Article class is in the analyzer's table");
        let find = article
            .instance_methods
            .get(&Symbol::from("find"))
            .expect("find method from RBS is in Article's instance_methods");

        // The RBS override is present with the user-declared return.
        let Ty::Fn { ret, .. } = find else {
            panic!("expected Ty::Fn for find override");
        };
        assert_eq!(**ret, Ty::Str);
    }

    #[test]
    fn analyzer_with_no_rbs_signatures_is_unchanged() {
        // Regression guard: an App with an empty rbs_signatures
        // produces the same analyzer state as a default App.
        let app = App::new();
        let analyzer = Analyzer::new(&app);
        // Just confirm the hardcoded entries survived.
        assert!(analyzer
            .classes
            .contains_key(&ClassId(Symbol::from("ApplicationController"))));
        assert!(analyzer
            .classes
            .contains_key(&ClassId(Symbol::from("ActiveModel::Errors"))));
    }
}

/// module → the union of every INCLUDING controller's ivar environment,
/// closed transitively over nested includes (the same closure the Phase A
/// splice walks).
///
/// One copy, called from two places: the concern module's own body typing,
/// and the reseed of the copies `splice_concerns_into_controllers` cut into
/// each includer. A second copy is how the rule drifts — and the two
/// callers genuinely need the same answer, because the module body and its
/// spliced copies are the same code.
fn concern_ivar_env_of(
    app: &App,
    controller_ivar_env: &HashMap<ClassId, HashMap<Symbol, Ty>>,
    module_includes: &HashMap<ClassId, Vec<ClassId>>,
) -> HashMap<ClassId, HashMap<Symbol, Ty>> {
            let mut out: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
        let by_name: HashMap<&ClassId, &Controller> =
            app.controllers.iter().map(|c| (&c.name, c)).collect();
        for controller in &app.controllers {
            let Some(env) = controller_ivar_env.get(&controller.name) else { continue };
            // The includer may be an ANCESTOR. campfire puts
            // `include TrackedRoomVisit` on ApplicationController,
            // which has no `@room` — but the concern's
            // `remember_last_room_visited` is a before_action on
            // the Rooms controllers, which do. A concern included
            // high in the chain runs on every controller below it,
            // so every one of those environments is a source.
            let mut queue: Vec<ClassId> = controller_includes(controller);
            {
                let mut cursor = controller.parent.clone();
                for _ in 0..32 {
                    let Some(pid) = cursor else { break };
                    let Some(parent) = by_name.get(&pid) else { break };
                    queue.extend(controller_includes(parent));
                    cursor = parent.parent.clone();
                }
            }
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if let Some(nested) = module_includes.get(&m) {
                    for n in nested {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
                let entry = out.entry(m).or_default();
                for (k, v) in env {
                    if v.is_open() {
                        continue;
                    }
                    let merged = match entry.remove(k) {
                        Some(prev) if prev == *v => prev,
                        Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                        None => v.clone(),
                    };
                    entry.insert(k.clone(), merged);
                }
            }
        }
    out
}

/// Register the stdlib class surface into a caller-built registry — the
/// same table [`Analyzer::new`] folds in.
///
/// Exists for a gate rather than for the pipeline:
/// `tests/runtime_src_integration.rs` types every `runtime/ruby/` body
/// against a registry it assembles from the paired `.rbs` files, and
/// without this that registry is strictly poorer than the one those
/// same files meet once they land in an emitted tree. The gap it
/// invents is not hypothetical — `rescue => e; e.message` typed as
/// unknown there and as `String` everywhere else, which reads as "the
/// runtime file is wrong" when the registry was.
pub fn register_stdlib_classes(
    classes: &mut std::collections::HashMap<crate::ident::ClassId, ClassInfo>,
) {
    registry::stdlib::register(classes);
}

/// Every expression whose value a method body can return: the tail,
/// through `if`/`case`/`begin`-`rescue` arms, plus the value of each
/// `return` anywhere in the body outside a block. A raising arm returns
/// nothing and contributes no leaf.
pub(crate) fn return_leaves(body: &Expr) -> Vec<&Expr> {
    fn tails<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        match &*e.node {
            ExprNode::If { then_branch, else_branch, .. } => {
                tails(then_branch, out);
                tails(else_branch, out);
            }
            ExprNode::Case { arms, .. } => arms.iter().for_each(|a| tails(&a.body, out)),
            ExprNode::Seq { exprs } if !exprs.is_empty() => tails(exprs.last().unwrap(), out),
            ExprNode::BeginRescue { body, rescues, else_branch, .. } => {
                tails(else_branch.as_ref().unwrap_or(body), out);
                rescues.iter().for_each(|r| tails(&r.body, out));
            }
            ExprNode::Return { .. } => {}
            ExprNode::Raise { .. } => {}
            _ => out.push(e),
        }
    }
    fn returns<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        match &*e.node {
            // A block's `return`/`next` is not the method's.
            ExprNode::Lambda { .. } => {}
            ExprNode::Return { value } => {
                tails(value, out);
                returns(value, out);
            }
            _ => e.node.for_each_child(&mut |c| returns(c, out)),
        }
    }
    let mut out = Vec::new();
    tails(body, &mut out);
    returns(body, &mut out);
    out
}

/// A method that returns a fixed-length array whose positions hold
/// DIFFERENT types — `[cache_votes(scope), show_more]` — returns a
/// tuple, and callers destructure it that way (`@stories, @show_more =
/// paginate(…)`). The literal itself stays `Array[A | B]` inside the
/// body; only the boundary says which position is which, the way the
/// harvest draws `Relation` and `Class[C]` there. Without it the union
/// reaches every target of the destructuring: lobsters' `@stories`
/// was `Array[Story] | bool`, and `render json: @stories` had nothing
/// to serialize.
///
/// All or nothing: every return leaf must be an array literal of the
/// same length (two or more), with no splat, and some position must
/// actually differ from another — a uniform literal is an ordinary
/// `Array[T]`.
pub(crate) fn tuple_return_ty(body: &Expr) -> Option<Ty> {
    let leaves = return_leaves(body);
    let mut positions: Option<Vec<Ty>> = None;
    for leaf in leaves {
        let ExprNode::Array { elements, .. } = &*leaf.node else { return None };
        if elements.len() < 2 || elements.iter().any(|e| matches!(&*e.node, ExprNode::Splat { .. })) {
            return None;
        }
        let tys: Vec<Ty> = elements.iter().map(|e| e.ty.clone()).collect::<Option<_>>()?;
        if tys.iter().any(|t| matches!(t, Ty::Var { .. })) {
            return None;
        }
        positions = Some(match positions {
            None => tys,
            Some(prev) if prev.len() == tys.len() => {
                prev.into_iter().zip(tys).map(|(a, b)| crate::analyze::body::union_of(a, b)).collect()
            }
            Some(_) => return None,
        });
    }
    let elems = positions?;
    let first = &elems[0];
    if elems.iter().all(|t| t == first) {
        return None;
    }
    Some(Ty::Tuple { elems })
}

#[cfg(test)]
mod keyword_splat_tests {
    use super::*;

    /// `f(**{kind: :x, keys: codes}.merge(options))` against
    /// `def f(kind:, keys:, only: nil)`: the literal's keys are typed from
    /// the literal, but the splat merges over them, so each also takes the
    /// splat's value type; an unnamed keyword takes only the splat's.
    #[test]
    fn a_merged_splat_joins_the_literal_keys_and_fills_the_rest() {
        let shape = ParamShape {
            slots: vec![
                (Symbol::from("kind"), ParamKind::Keyword { required: true }),
                (Symbol::from("keys"), ParamKind::Keyword { required: true }),
                (Symbol::from("only"), ParamKind::Keyword { required: false }),
            ],
            keywords_by_kind: true,
        };
        let hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) };
        let keys = vec![
            (Symbol::from("kind"), Ty::Sym),
            (Symbol::from("keys"), Ty::Array { elem: Box::new(Ty::Str) }),
        ];
        let placed = Analyzer::bind_keyword_group(&shape, &[hash], &keys, Some(&Ty::Bool))
            .expect("placed");
        assert_eq!(
            placed,
            vec![
                union_of(Ty::Sym, Ty::Bool),
                union_of(Ty::Array { elem: Box::new(Ty::Str) }, Ty::Bool),
                Ty::Bool,
            ]
        );
        let unsplatted = Analyzer::bind_keyword_group(&shape, &[Ty::Untyped], &keys, None)
            .expect("placed");
        assert_eq!(unsplatted[0], Ty::Sym, "without a splat the literal stands alone");
    }
}

/// Does `body` call `super` anywhere (bare or with arguments)?
fn expr_calls_super(body: &Expr) -> bool {
    let mut found = matches!(&*body.node, ExprNode::Super { .. });
    body.node.for_each_child(&mut |c| found = found || expr_calls_super(c));
    found
}
