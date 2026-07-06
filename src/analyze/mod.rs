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

mod body;
pub mod async_color;
pub mod attribution;
pub mod preload;
pub mod block_refine;
pub mod mutates_self;

pub use body::{BodyTyper, ClassInfo, Ctx};
pub use preload::{missing_preload_report, PreloadCoverage};
pub(crate) use body::union_of;

use std::collections::{BTreeSet, HashMap};

use crate::adapter::{ArMethodKind, DatabaseAdapter, SqliteAdapter};
use crate::App;
use crate::dialect::{
    Action, Controller, ControllerBodyItem, Filter, FilterKind, LayoutDecl, ModelBodyItem,
    RenderTarget,
};
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::{Row, Ty};

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
    inferred_params: HashMap<(ClassId, Symbol), Vec<Ty>>,
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
}


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

        for model in &app.models {
            let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
            let array_of_self =
                Ty::Array { elem: Box::new(self_ty.clone()) };

            let mut cls = ClassInfo::default();
            cls.table = Some(model.table.clone());
            cls.attributes = model.attributes.clone();

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
            let instantiate = |kind: ReturnKind| -> Ty {
                match kind {
                    ReturnKind::SelfType => self_ty.clone(),
                    ReturnKind::ArrayOfSelf => array_of_self.clone(),
                    ReturnKind::SelfOrNil => Ty::Union {
                        variants: vec![self_ty.clone(), Ty::Nil],
                    },
                    ReturnKind::Int => Ty::Int,
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
                }
            };
            for entry in AR_CATALOG {
                if entry.receiver != ReceiverContext::Class {
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
            cls.class_methods.insert(Symbol::from("connection"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("connection_pool"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("establish_connection"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("table_name"), Ty::Str);
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
            // `Model.unscoped`/`Model.none` return a relation
            // (`Array<Model>`, the chainable stand-in) so chains through
            // them stay typed instead of leaking to `untyped`. (The block
            // form `unscoped { }` returns the block value, which we don't
            // track — the relation type is the better default for the
            // common bare/chained use.) `delete_all`/`update_all` return
            // Int (affected row count).
            cls.class_methods.insert(Symbol::from("unscoped"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("none"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("delete_all"), Ty::Int);
            cls.class_methods.insert(Symbol::from("update_all"), Ty::Int);

            // Chainable query-builder methods beyond the catalog set.
            // Each returns the relation (modeled as `Array<Self>`, the
            // same chainable stand-in the catalog uses), so a scope or
            // controller chain types end-to-end rather than leaking to
            // `untyped` at the first uncatalogued link, and the
            // `Array<Self>` re-chain in `send.rs` resolves the next step.
            // `entry().or_insert` so a catalog entry or named scope still
            // wins. `not`/`missing` are really `WhereChain` methods
            // (`where.not(...)`/`where.missing(...)`); since `where`
            // already yields the relation, the chain lands on
            // `Array<Self>` and these resolve there — typing `Model.not`
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
                    .or_insert_with(|| array_of_self.clone());
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
                .or_insert_with(|| Ty::Array { elem: Box::new(Ty::Int) });

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
                cls.instance_methods.insert(name.clone(), ty.clone());
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
                cls.instance_methods
                    .entry(Symbol::from(format!("{n}_was")))
                    .or_insert(ty.clone());
            }
            // `typed_store` (activerecord-typedstore) accessors: declared in a
            // DSL block, backed by a serialized column, so absent from the
            // schema-derived attributes above. Register them as typed methods.
            register_typed_store(&model.body, &mut cls.instance_methods);
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
            register_has_secure_password(&model.body, &mut cls.instance_methods, &self_ty);

            // Named scopes resolve as relation-returning class methods, so
            // `Story.active` types and chains like `Story.active.recent`
            // compose. The relation type (`Array[Self]`) is what lets a scope
            // also chain on a relation (see the `Array[Class]` dispatch, which
            // delegates relation-returning class methods to the element model).
            // Scope bodies are typed separately; this only records the call
            // surface. `or_insert` so an explicit catalog method still wins.
            for scope in model.scopes() {
                cls.class_methods
                    .entry(scope.name.clone())
                    .or_insert_with(|| array_of_self.clone());
            }
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
                cls.instance_methods.insert(name, ty.clone());
                cls.instance_methods.entry(writer).or_insert(ty);
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
                                cls.instance_methods.entry(name).or_insert(ty.clone());
                                cls.instance_methods.entry(writer).or_insert(ty);
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

            classes.insert(model.name.clone(), cls);
        }

        // `ActiveRecord::Base` itself — the literal base class, called
        // directly as `ActiveRecord::Base.transaction { ... }` and
        // `ActiveRecord::Base.connection.exec_query(...)`. It sits at the
        // end of every model's parent chain but was never registered as a
        // class, so dispatch on the non-model receiver `Class
        // { ActiveRecord::Base }` found nothing and errored. `transaction`
        // runs the block in a DB transaction (return = the block value,
        // not statically tracked) and `connection` hands back a raw
        // connection adapter — both gradual (`Untyped`), exactly mirroring
        // the per-model class-side framework block above. `or_insert` so a
        // real `active_record/base.rb` library file (none in practice)
        // would still win.
        {
            let mut base = ClassInfo::default();
            for m in [
                "transaction",
                "connection",
                "connection_pool",
                "establish_connection",
            ] {
                base.class_methods.insert(Symbol::from(m), Ty::Untyped);
            }
            classes
                .entry(ClassId(Symbol::from("ActiveRecord::Base")))
                .or_insert(base);
        }

        // `ActionController::Base.helpers` — the view-helper proxy a model
        // or library reaches for to build paths/URLs outside a request
        // (`ActionController::Base.helpers.image_url(...)` in user.rb). The
        // literal class is unmodeled (controllers carry a hardcoded
        // surface, but `ActionController::Base` itself was never a
        // registered class), so the call errored. `helpers` returns the
        // proxy (gradual — its method surface is the full view-helper set);
        // the other entries are the framework class-side config readers
        // that occasionally appear on the bare base class.
        {
            let mut acb = ClassInfo::default();
            for m in ["helpers", "helper", "default_url_options"] {
                acb.class_methods.insert(Symbol::from(m), Ty::Untyped);
            }
            classes
                .entry(ClassId(Symbol::from("ActionController::Base")))
                .or_insert(acb);
        }

        // `ActiveModel::Validations` / `ActiveModel::Model` — mixed into
        // plain-Ruby form/query objects (`class Search; include
        // ActiveModel::Validations`). A class that includes them gains the
        // validation surface, resolved via the includer's `includes` and
        // `lookup_in_module`. Registered as module ClassInfos carrying that
        // surface. `ActiveModel::Model` bundles Validations + Conversion +
        // attribute assignment, so it gets the same predicates plus the
        // persistence-shape readers.
        {
            let errors_ty = Ty::Class {
                id: ClassId(Symbol::from("ActiveModel::Errors")),
                args: vec![],
            };
            let mut validations = ClassInfo::default();
            for (m, ty) in [
                ("valid?", Ty::Bool),
                ("invalid?", Ty::Bool),
                ("validate", Ty::Bool),
                ("validate!", Ty::Bool),
                ("errors", errors_ty.clone()),
            ] {
                validations.instance_methods.insert(Symbol::from(m), ty);
            }
            classes
                .entry(ClassId(Symbol::from("ActiveModel::Validations")))
                .or_insert(validations.clone());

            let mut model = validations;
            for (m, ty) in [
                ("persisted?", Ty::Bool),
                ("new_record?", Ty::Bool),
                ("to_model", Ty::Untyped),
                ("model_name", Ty::Untyped),
            ] {
                model.instance_methods.insert(Symbol::from(m), ty);
            }
            classes
                .entry(ClassId(Symbol::from("ActiveModel::Model")))
                .or_insert(model);
        }

        // ActiveModel::Errors — the collection returned by `model.errors`.
        // Supports count/[]/any?/each and flows a Error instance to blocks.
        let error_ty = Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Error")),
            args: vec![],
        };
        let mut errors_cls = ClassInfo::default();
        errors_cls
            .instance_methods
            .insert(Symbol::from("count"), Ty::Int);
        errors_cls
            .instance_methods
            .insert(Symbol::from("size"), Ty::Int);
        errors_cls
            .instance_methods
            .insert(Symbol::from("any?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("none?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("empty?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("include?"), Ty::Bool);
        errors_cls.instance_methods.insert(
            Symbol::from("full_messages"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        // `errors[:title]` returns an Array<String> of messages for that attribute.
        errors_cls.instance_methods.insert(
            Symbol::from("[]"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        errors_cls.instance_methods.insert(
            Symbol::from("messages_for"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        // `.each` yields an Error — registered via block_params_for below.
        errors_cls
            .instance_methods
            .insert(Symbol::from("each"), error_ty.clone());
        // `errors << "message"` is the transpiled-shape idiom for adding
        // errors from a model's `validate` method. Returns the errors
        // collection (same as Array#<<). `add` is the semantically-
        // equivalent Rails idiom.
        errors_cls.instance_methods.insert(
            Symbol::from("<<"),
            Ty::Class {
                id: ClassId(Symbol::from("ActiveModel::Errors")),
                args: vec![],
            },
        );
        errors_cls.instance_methods.insert(
            Symbol::from("add"),
            Ty::Class {
                id: ClassId(Symbol::from("ActiveModel::Errors")),
                args: vec![],
            },
        );
        errors_cls.instance_methods.insert(
            Symbol::from("clear"),
            Ty::Class {
                id: ClassId(Symbol::from("ActiveModel::Errors")),
                args: vec![],
            },
        );
        classes.insert(
            ClassId(Symbol::from("ActiveModel::Errors")),
            errors_cls,
        );

        // CollectionProxy — the runtime helper transpiled models use
        // for has_many associations. `new(...)` returns an instance;
        // iteration/build/create/count/size live on the instance.
        // Registered under the bare last-segment name because the
        // body-typer instantiates `Const { path }` using `path.last()`
        // — see ExprNode::Const branch in analyze/body/mod.rs.
        let cp_class = ClassId(Symbol::from("CollectionProxy"));
        let mut cp_cls = ClassInfo::default();
        cp_cls.class_methods.insert(
            Symbol::from("new"),
            Ty::Class { id: cp_class.clone(), args: vec![] },
        );
        cp_cls.instance_methods.insert(Symbol::from("size"), Ty::Int);
        cp_cls.instance_methods.insert(Symbol::from("length"), Ty::Int);
        cp_cls.instance_methods.insert(Symbol::from("count"), Ty::Int);
        cp_cls.instance_methods.insert(Symbol::from("empty?"), Ty::Bool);
        // `each`, `build`, `create` — return types depend on the target
        // class which isn't known from the proxy type alone. Leave as
        // unknown() placeholders; real resolution requires threading
        // association metadata through the ivar type, which is future
        // work.
        classes.insert(cp_class, cp_cls);

        // Individual Error with its Rails API.
        let mut error_cls = ClassInfo::default();
        error_cls
            .instance_methods
            .insert(Symbol::from("full_message"), Ty::Str);
        error_cls
            .instance_methods
            .insert(Symbol::from("message"), Ty::Str);
        error_cls
            .instance_methods
            .insert(Symbol::from("attribute"), Ty::Sym);
        error_cls
            .instance_methods
            .insert(Symbol::from("type"), Ty::Sym);
        classes.insert(
            ClassId(Symbol::from("ActiveModel::Error")),
            error_cls,
        );

        // `ActiveRecord::AdapterInterface` — the 9-method contract that
        // `runtime/ruby/active_record/base.rb` calls into via
        // `ActiveRecord.adapter.X`. Each per-target runtime ships its
        // own concrete impl (Rust trait + impls in `runtime/rust/`,
        // Crystal abstract class + SqliteAdapter, TS interface +
        // SqliteActiveRecordAdapter). On the
        // Ruby side there's no class declaration — the RBS for
        // `ActiveRecord.adapter` previously returned `untyped`, which
        // let TS get away with `any` but left rust2 emit producing
        // method calls on `serde_json::Value` (E0599 on
        // `.find/.where/.all/.insert/.update/.delete/.count/.exists/.truncate`).
        // Registering it here gives the body-typer a concrete class to
        // dispatch against; the RBS sidecar then references it as
        // `() -> AdapterInterface`.
        let hash_str_untyped = Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Untyped),
        };
        let row_ty = hash_str_untyped.clone();
        let nilable_row = Ty::Union {
            variants: vec![row_ty.clone(), Ty::Nil],
        };
        let array_of_rows = Ty::Array { elem: Box::new(row_ty.clone()) };
        let mut adapter_iface = ClassInfo::default();
        adapter_iface
            .instance_methods
            .insert(Symbol::from("all"), array_of_rows.clone());
        adapter_iface
            .instance_methods
            .insert(Symbol::from("find"), nilable_row.clone());
        adapter_iface
            .instance_methods
            .insert(Symbol::from("where"), array_of_rows.clone());
        adapter_iface
            .instance_methods
            .insert(Symbol::from("count"), Ty::Int);
        adapter_iface
            .instance_methods
            .insert(Symbol::from("exists?"), Ty::Bool);
        adapter_iface
            .instance_methods
            .insert(Symbol::from("insert"), Ty::Int);
        adapter_iface
            .instance_methods
            .insert(Symbol::from("update"), Ty::Nil);
        adapter_iface
            .instance_methods
            .insert(Symbol::from("delete"), Ty::Nil);
        adapter_iface
            .instance_methods
            .insert(Symbol::from("truncate"), Ty::Nil);
        classes.insert(
            ClassId(Symbol::from("ActiveRecord::AdapterInterface")),
            adapter_iface,
        );

        // Arel — the low-level SQL AST that advanced scopes reach for
        // (`Model.arel_table[:col].not_in(subquery)`, `relation.arel.exists`,
        // `Arel.sql(...)`). A small class family whose methods all return
        // Arel nodes (never `Untyped`), so a chain that drops into Arel
        // stays typed instead of collapsing to a gradual escape at the
        // first `arel_table`/`arel`/`Arel.sql` hop. Precision is coarse —
        // every predicate/combinator returns the same `Arel::Node`; the
        // win is that the chain resolves rather than which node it is.
        let arel_node = Ty::Class { id: ClassId(Symbol::from("Arel::Node")), args: vec![] };
        let arel_attribute_ty =
            Ty::Class { id: ClassId(Symbol::from("Arel::Attribute")), args: vec![] };
        let arel_select_mgr =
            Ty::Class { id: ClassId(Symbol::from("Arel::SelectManager")), args: vec![] };

        // `Arel.sql(...)` / `Arel.star` — module-level node constructors.
        let mut arel_mod = ClassInfo::default();
        arel_mod.class_methods.insert(Symbol::from("sql"), arel_node.clone());
        arel_mod.class_methods.insert(Symbol::from("star"), arel_node.clone());
        classes.insert(ClassId(Symbol::from("Arel")), arel_mod);

        // `Model.arel_table` → table; `table[:col]` → attribute. A table
        // also delegates query-builder calls to a select manager
        // (`table.project(Arel.star)`, `table.where(...)`).
        let mut arel_table = ClassInfo::default();
        arel_table.instance_methods.insert(Symbol::from("[]"), arel_attribute_ty.clone());
        for m in [
            "project", "where", "order", "group", "having", "join", "on",
            "take", "skip", "from", "distinct",
        ] {
            arel_table.instance_methods.insert(Symbol::from(m), arel_select_mgr.clone());
        }
        classes.insert(ClassId(Symbol::from("Arel::Table")), arel_table);

        // `Arel::Attribute` predicates → node.
        let mut arel_attribute = ClassInfo::default();
        for pred in [
            "eq", "not_eq", "in", "not_in", "gt", "gteq", "lt", "lteq",
            "matches", "does_not_match", "between", "eq_any", "in_any",
            "asc", "desc", "count", "sum", "minimum", "maximum", "average",
        ] {
            arel_attribute.instance_methods.insert(Symbol::from(pred), arel_node.clone());
        }
        classes.insert(ClassId(Symbol::from("Arel::Attribute")), arel_attribute);

        // `Arel::Node` boolean combinators chain into nodes; `where(node)`
        // already accepts any argument type.
        let mut arel_node_cls = ClassInfo::default();
        for m in ["and", "or", "not"] {
            arel_node_cls.instance_methods.insert(Symbol::from(m), arel_node.clone());
        }
        classes.insert(ClassId(Symbol::from("Arel::Node")), arel_node_cls);

        // `relation.arel` / `Model.arel` → select manager; `.exists` →
        // node; further builder calls stay on the manager.
        let mut arel_select = ClassInfo::default();
        arel_select.instance_methods.insert(Symbol::from("exists"), arel_node.clone());
        for m in ["where", "project", "join", "on", "group", "order", "take", "skip"] {
            arel_select.instance_methods.insert(Symbol::from(m), arel_select_mgr.clone());
        }
        classes.insert(ClassId(Symbol::from("Arel::SelectManager")), arel_select);

        // ActionView form builder — `form_with do |form| form.text_field
        // ... end`. `form_with` yields a FormBuilder whose field helpers
        // render to strings (`ActiveSupport::SafeBuffer`, modeled as Str).
        // Registered so both the block param AND the per-field calls type:
        // once `form` is a FormBuilder, an unregistered `form.x` would be a
        // dispatch *error*, so the field surface is covered here.
        let form_builder_id = ClassId(Symbol::from("ActionView::Helpers::FormBuilder"));
        let form_builder_ty = Ty::Class { id: form_builder_id.clone(), args: vec![] };
        let block_fn = |block_ty: &Ty, ret: Ty| Ty::Fn {
            params: vec![],
            block: Some(Box::new(block_ty.clone())),
            ret: Box::new(ret),
            effects: EffectSet::default(),
        };
        let mut form_builder = ClassInfo::default();
        for m in [
            "label", "submit", "button", "text_field", "text_area", "textarea",
            "hidden_field", "password_field", "email_field", "number_field",
            "url_field", "tel_field", "telephone_field", "phone_field",
            "search_field", "color_field", "range_field", "date_field",
            "time_field", "datetime_field", "datetime_local_field", "month_field",
            "week_field", "file_field", "check_box", "radio_button", "select",
            "collection_select", "grouped_collection_select", "time_zone_select",
            "collection_check_boxes", "collection_radio_buttons", "date_select",
            "time_select", "datetime_select", "rich_text_area", "weekday_select",
            "id", "to_s",
        ] {
            form_builder.instance_methods.insert(Symbol::from(m), Ty::Str);
        }
        // `form.object` is the form's model (unknown model → gradual);
        // nested `fields_for`/`fields` yield another builder.
        form_builder.instance_methods.insert(Symbol::from("object"), Ty::Untyped);
        for m in ["fields_for", "fields"] {
            form_builder
                .instance_methods
                .insert(Symbol::from(m), block_fn(&form_builder_ty, Ty::Str));
        }
        classes.insert(form_builder_id, form_builder);

        // `respond_to do |format| format.html { } format.json { } end` —
        // the block yields a mime Collector whose format methods return
        // nil. (`respond_to` itself is registered on ApplicationController.)
        let mut collector = ClassInfo::default();
        for m in [
            "html", "json", "xml", "js", "rss", "atom", "text", "csv", "any",
            "all", "none",
        ] {
            collector.instance_methods.insert(Symbol::from(m), Ty::Nil);
        }
        classes.insert(
            ClassId(Symbol::from("ActionController::MimeResponds::Collector")),
            collector,
        );

        // View context — the `self` a view body types against. `form_with`
        // lives here (flat view helpers — `link_to`/`render`/… — will join
        // it); the view loops set this as `self_ty` so implicit-self helper
        // calls dispatch against it.
        // Route URL helper names from the ingested route table — one
        // `<as_name>_path` / `<as_name>_url` per named route (same flattening
        // the route emitters use). Derived from real routes, not a `_path$`
        // name heuristic, so only declared routes resolve. Registered on
        // both the view context and ApplicationController below.
        let route_helper_names: Vec<String> = {
            let mut names = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            // Rails auto-names a `:as`-less route from its path's static
            // segments (`get "/settings"` → `settings_path`, `get
            // "/replies/unread"` → `replies_unread_path`). `flatten_routes`
            // (which also feeds *emit*) keeps its action-name fallback, so we
            // add the path-derived candidate here, on the analyze dispatch
            // surface ONLY — purely additive (extra `*_path` readers can only
            // resolve a call, never alter emitted output). A genuinely-named
            // route still registers its real `as_name` first.
            let path_candidate = |path: &str| -> String {
                path.split('/')
                    .filter(|seg| {
                        !seg.is_empty()
                            && !seg.starts_with(':')
                            && !seg.starts_with('*')
                            && seg.chars().all(|c| c.is_alphanumeric() || c == '_')
                    })
                    .collect::<Vec<_>>()
                    .join("_")
            };
            for route in crate::lower::flatten_routes(app) {
                for candidate in [route.as_name.clone(), path_candidate(&route.path)] {
                    if candidate.is_empty() {
                        continue;
                    }
                    if seen.insert(candidate.clone()) {
                        names.push(format!("{candidate}_path"));
                        names.push(format!("{candidate}_url"));
                    }
                }
            }
            names
        };

        let mut action_view = ClassInfo::default();
        action_view
            .instance_methods
            .insert(Symbol::from("form_with"), block_fn(&form_builder_ty, Ty::Str));
        // Flat view helpers — links, tags, asset/meta tags, text and number
        // formatting, dom ids, render, turbo helpers. All render to strings
        // (`ActiveSupport::SafeBuffer`, modeled as Str), so the implicit-self
        // call types and any `.html_safe`/`.gsub`/etc. chained on the result
        // resolves through `str_method`. (Route helpers `*_path`/`*_url`,
        // flash `notice`/`alert`, and jbuilder `json` are registered
        // elsewhere.)
        for helper in [
            // links / urls
            "link_to", "button_to", "link_to_if", "link_to_unless",
            "link_to_unless_current", "mail_to", "url_for",
            // tags / assets / meta
            "content_tag", "image_tag", "image_url", "image_path",
            "video_tag", "audio_tag", "asset_path", "asset_url",
            "favicon_link_tag", "stylesheet_link_tag", "stylesheet_path",
            "javascript_include_tag", "javascript_path",
            "javascript_importmap_tags", "javascript_tag",
            "stylesheet_pack_tag", "javascript_pack_tag", "csrf_meta_tags",
            "csrf_meta_tag", "csp_meta_tag", "auto_discovery_link_tag",
            "preload_link_tag", "action_cable_meta_tag",
            // text / number formatting
            "pluralize", "truncate", "simple_format", "highlight", "excerpt",
            "word_wrap", "sanitize", "sanitize_css", "strip_tags",
            "strip_links", "raw", "h", "html_escape", "concat", "safe_join",
            "cycle", "current_cycle", "number_to_currency", "number_to_human",
            "number_to_human_size", "number_to_percentage", "number_to_phone",
            "number_with_delimiter", "number_with_precision",
            // dates
            "time_ago_in_words", "distance_of_time_in_words",
            "distance_of_time_in_words_to_now",
            // i18n — the view-side translate/localize helpers (delegate
            // to I18n; lazy-lookup `t(".key")` included). Str like the
            // rest of the SafeBuffer-rendering surface.
            "t", "translate", "l", "localize",
            // Our own HAML lowering's dynamic-attribute helper
            // (`%div{opengraph_tags}` → `render_attrs(…)`, see
            // src/haml.rs) — renders an attribute string.
            "render_attrs",
            // dom / rendering / capture
            "dom_id", "dom_class", "render", "render_to_string", "capture",
            "content_for", "provide", "escape_javascript", "j",
            // turbo / hotwire
            "turbo_frame_tag", "turbo_stream_from", "turbo_refreshes_with",
            "turbo_include_tags", "turbo_page_requires_reload",
            // form option builders + FormTagHelper (all render to SafeBuffer
            // strings, like the tag helpers above).
            "options_for_select", "options_from_collection_for_select",
            "option_groups_from_collection_for_select", "grouped_options_for_select",
            "time_zone_options_for_select", "collection_select",
            "form_tag", "label_tag", "text_field_tag", "password_field_tag",
            "hidden_field_tag", "text_area_tag", "check_box_tag",
            "radio_button_tag", "select_tag", "submit_tag", "button_tag",
            "field_set_tag", "file_field_tag", "email_field_tag",
            "number_field_tag", "search_field_tag", "telephone_field_tag",
            "url_field_tag", "date_field_tag", "color_field_tag",
            "fields_for", "token_list", "class_names",
            // controller/request context exposed to views (and controllers,
            // registered there too) — both return the current name as Str.
            "action_name", "controller_name", "controller_path",
        ] {
            action_view
                .instance_methods
                .entry(Symbol::from(helper))
                .or_insert(Ty::Str);
        }
        // `tag` is the dynamic TagBuilder — `tag.div`/`tag.details` build an
        // element from the *method name*, so it can't be a fixed Str return
        // (that turns `tag.foo` into a dispatch error). Untyped (gradual):
        // both `tag("br")` and `tag.section` flow through without erroring.
        action_view.instance_methods.insert(Symbol::from("tag"), Ty::Untyped);
        // Flash convenience accessors — Rails 7 scaffolds emit bare
        // `notice`/`alert` in views; both read `flash[:notice]`/`[:alert]`.
        // Typed Str (not Str|Nil): consistent with the other Str-returning
        // helpers, and a nilable here trips Crystal's strict nil-concat
        // narrowing in `<%= notice %>`. `.present?` still resolves on Str.
        for m in ["notice", "alert"] {
            action_view.instance_methods.insert(Symbol::from(m), Ty::Str);
        }
        // `flash` — the FlashHash. Bare `flash` was unmodeled, so
        // `flash[:error]` / `flash.now[:error]` / `flash.each` / `flash.keep`
        // (pervasive in controllers and views) all bottomed out at Var. Type
        // it as a FlashHash whose surface is registered below; both the view
        // (instance) and controller (class-side) contexts get it.
        let flash_ty = Ty::Class {
            id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
            args: vec![],
        };
        action_view
            .instance_methods
            .insert(Symbol::from("flash"), flash_ty.clone());
        // jbuilder `json` builder (in `*.json.jbuilder` views) is dynamic —
        // `json.<field>`/`json.array!`/`json.partial!` build from the method
        // name, so Untyped (gradual) is the honest type and chains through
        // it without erroring.
        action_view.instance_methods.insert(Symbol::from("json"), Ty::Untyped);
        // Route URL helpers (view side).
        for name in &route_helper_names {
            action_view
                .instance_methods
                .entry(Symbol::from(name.as_str()))
                .or_insert(Ty::Str);
        }
        // Kaminari's view-side paginator renders to a SafeBuffer string,
        // like the tag helpers above.
        action_view
            .instance_methods
            .entry(Symbol::from("paginate"))
            .or_insert(Ty::Str);
        // `params` is exposed to templates too (same strong-params
        // surface the controller context declares).
        action_view.instance_methods.insert(
            Symbol::from("params"),
            Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) },
        );
        // SimpleForm's form builder — same shape as `form_with` but the
        // yielded builder (`f.input`, `f.association`, …) is a SimpleForm
        // class we don't model structurally, so the block param is the
        // gradual escape: `f.input :name` flows through instead of
        // bottoming out unresolved.
        for m in ["simple_form_for", "simple_fields_for"] {
            action_view
                .instance_methods
                .entry(Symbol::from(m))
                .or_insert_with(|| block_fn(&Ty::Untyped, Ty::Str));
        }
        // Helper-fold: Rails mixes EVERY module under app/helpers into
        // every view (`helpers :all` default). Declaring them as
        // `include`s of the view context lets `fold_concern_surfaces`
        // copy each helper's typed surface onto `ActionView::Base` at
        // every harvest round — so a bare `material_symbol(…)` in a
        // template resolves exactly like a concern method on a model,
        // refining as the fixpoint types helper bodies. Hardcoded
        // framework entries above win over a same-named app helper
        // (own-entry-wins in the fold); acceptable, both are Str-shaped
        // in practice.
        let helper_modules: BTreeSet<ClassId> =
            app.helper_method_index.values().cloned().collect();
        action_view.includes.extend(helper_modules);
        classes.insert(ClassId(Symbol::from("ActionView::Base")), action_view);

        // The FlashHash returned by `flash`. Values are messages (Str); `now`
        // is the same hash scoped to this request (so `flash.now[:x]` types);
        // `notice`/`alert`/`error`/`success` are the convenience readers Rails
        // generates; `keep`/`discard`/`each` return the hash for chaining;
        // predicates and `[]` round out the surface. Lookups not listed fall
        // through to "no known method" — extend as the corpus demands.
        {
            let mut flash = ClassInfo::default();
            let flash_self = Ty::Class {
                id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
                args: vec![],
            };
            for (m, ty) in [
                ("[]", Ty::Str),
                ("[]=", Ty::Nil),
                ("store", Ty::Nil),
                ("now", flash_self.clone()),
                ("notice", Ty::Str),
                ("alert", Ty::Str),
                ("error", Ty::Str),
                ("success", Ty::Str),
                ("notice=", Ty::Str),
                ("alert=", Ty::Str),
                ("delete", Ty::Str),
                ("keep", flash_self.clone()),
                ("discard", flash_self.clone()),
                ("each", flash_self.clone()),
                ("each_pair", flash_self.clone()),
                ("clear", flash_self.clone()),
                ("update", flash_self.clone()),
                ("merge!", flash_self.clone()),
                ("key?", Ty::Bool),
                ("has_key?", Ty::Bool),
                ("include?", Ty::Bool),
                ("any?", Ty::Bool),
                ("empty?", Ty::Bool),
                ("present?", Ty::Bool),
                ("blank?", Ty::Bool),
                ("keys", Ty::Array { elem: Box::new(Ty::Sym) }),
                ("values", Ty::Array { elem: Box::new(Ty::Str) }),
                ("to_h", Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }),
                ("to_hash", Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }),
            ] {
                flash.instance_methods.insert(Symbol::from(m), ty);
            }
            classes.insert(
                ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
                flash,
            );
        }

        // Rails singleton — `Rails.application` / `Rails.logger` /
        // `Rails.cache` / `Rails.env` / `Rails.root` are pervasive
        // call shapes in real Rails code. Each maps to a runtime
        // object that's not modeled structurally here; return
        // `Ty::Untyped` (gradual escape) so method chains off them
        // propagate through dispatch without bottoming out at Var.
        // `Rails.env` is the one we can type concretely as Str.
        let mut rails_cls = ClassInfo::default();
        rails_cls.class_methods.insert(Symbol::from("application"), Ty::Untyped);
        rails_cls.class_methods.insert(Symbol::from("logger"), Ty::Untyped);
        rails_cls.class_methods.insert(Symbol::from("cache"), Ty::Untyped);
        rails_cls.class_methods.insert(Symbol::from("configuration"), Ty::Untyped);
        rails_cls.class_methods.insert(Symbol::from("root"), Ty::Untyped);
        // `Rails.env` is an ActiveSupport::StringInquirer (a String
        // that also answers `development?`/`production?`/… as Bool),
        // not a plain Str — see the StringInquirer dispatch in send.rs.
        rails_cls.class_methods.insert(
            Symbol::from("env"),
            Ty::Class {
                id: ClassId(Symbol::from("ActiveSupport::StringInquirer")),
                args: vec![],
            },
        );
        classes.insert(ClassId(Symbol::from("Rails")), rails_cls);

        // Time singleton — `Time.now` (Ruby core) / `Time.current`
        // (Rails) / `Time.at` all yield a Time *value*, and `Time.zone`
        // is a TimeZone whose `.now`/`.at`/`.local` likewise yield Time,
        // so modeling it as Time too lets those chains resolve. Time
        // values are already modeled structurally (`time_method` in
        // send.rs) and AR datetime columns type as Time, so these
        // constructors join that same surface — `Time.now.to_i` → Int,
        // `Time.current.utc` → Time — instead of bottoming out at the
        // `Untyped` gradual escape (and dragging every chained call into
        // it). `Time - x` arithmetic still resolves to `Untyped` inside
        // `time_method` because receiver-only dispatch can't tell a
        // Duration arg (→ Time) from a Time arg (→ Float).
        let time_ty = || Ty::Class {
            id: ClassId(Symbol::from("Time")),
            args: vec![],
        };
        let mut time_cls = ClassInfo::default();
        time_cls.class_methods.insert(Symbol::from("current"), time_ty());
        time_cls.class_methods.insert(Symbol::from("now"), time_ty());
        time_cls.class_methods.insert(Symbol::from("zone"), time_ty());
        time_cls.class_methods.insert(Symbol::from("at"), time_ty());
        classes.insert(ClassId(Symbol::from("Time")), time_cls);

        // Date / DateTime singletons — analogous to Time. Same
        // rationale: structural typing of these classes hasn't been
        // wired, but the call shape needs to resolve.
        for name in ["Date", "DateTime"] {
            let mut cls = ClassInfo::default();
            cls.class_methods.insert(Symbol::from("current"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("today"), Ty::Untyped);
            cls.class_methods.insert(Symbol::from("now"), Ty::Untyped);
            classes.insert(ClassId(Symbol::from(name)), cls);
        }

        // Ruby stdlib singletons + Set — referenced by ~every Rails app but
        // not structurally modeled. Register the common call surface so
        // `File.read`, `SecureRandom.hex`, `CGI.escape`, `Set#<<` resolve to
        // a return type instead of "no known method". Return types follow
        // the official rbs gem core/stdlib signatures, narrowed to the
        // concrete cases; opaque/handle returns (`File.open`, `URI.parse`)
        // and unparameterized collection elements degrade to `Untyped` (the
        // gradual escape) so chained calls still flow. Hardcoded like the
        // Rails/Time/Date blocks above — `register_stdlib_class` never
        // clobbers an app-defined method/class of the same name.
        let str_arr = || Ty::Array { elem: Box::new(Ty::Str) };
        register_stdlib_class(&mut classes, "SecureRandom", &[
            ("hex", Ty::Str), ("base64", Ty::Str), ("urlsafe_base64", Ty::Str),
            ("base58", Ty::Str), ("uuid", Ty::Str), ("alphanumeric", Ty::Str),
            ("random_bytes", Ty::Str), ("random_number", Ty::Untyped),
        ], &[]);
        register_stdlib_class(&mut classes, "File", &[
            ("read", Ty::Str), ("binread", Ty::Str), ("write", Ty::Int),
            ("exist?", Ty::Bool), ("exists?", Ty::Bool), ("file?", Ty::Bool),
            ("directory?", Ty::Bool), ("open", Ty::Untyped),
            ("unlink", Ty::Int), ("delete", Ty::Int), ("rename", Ty::Int),
            ("join", Ty::Str), ("basename", Ty::Str), ("dirname", Ty::Str),
            ("extname", Ty::Str), ("expand_path", Ty::Str), ("size", Ty::Int),
        ], &[]);
        register_stdlib_class(&mut classes, "Dir", &[
            ("entries", str_arr()), ("glob", str_arr()), ("[]", str_arr()),
            ("exist?", Ty::Bool), ("exists?", Ty::Bool), ("mkdir", Ty::Int),
            ("pwd", Ty::Str), ("home", Ty::Str),
        ], &[]);
        register_stdlib_class(&mut classes, "Math", &[
            ("sqrt", Ty::Float), ("cbrt", Ty::Float), ("log", Ty::Float),
            ("log2", Ty::Float), ("log10", Ty::Float), ("exp", Ty::Float),
            ("sin", Ty::Float), ("cos", Ty::Float), ("tan", Ty::Float),
            ("atan", Ty::Float), ("atan2", Ty::Float), ("hypot", Ty::Float),
            ("pow", Ty::Float),
        ], &[]);
        register_stdlib_class(&mut classes, "CGI", &[
            ("escape", Ty::Str), ("unescape", Ty::Str),
            ("escapeHTML", Ty::Str), ("unescapeHTML", Ty::Str),
            ("escape_html", Ty::Str), ("unescape_html", Ty::Str),
        ], &[]);
        register_stdlib_class(&mut classes, "ERB::Util", &[
            ("html_escape", Ty::Str), ("h", Ty::Str),
            ("url_encode", Ty::Str), ("u", Ty::Str), ("json_escape", Ty::Str),
        ], &[]);
        for digest in ["Digest::MD5", "Digest::SHA1", "Digest::SHA256"] {
            register_stdlib_class(&mut classes, digest, &[
                ("hexdigest", Ty::Str), ("digest", Ty::Str),
                ("base64digest", Ty::Str),
            ], &[]);
        }
        // `URI.parse` returns a URI object we don't model; `Untyped` lets
        // chained `.scheme` / `.host` flow gradually instead of erroring.
        register_stdlib_class(&mut classes, "URI", &[
            ("parse", Ty::Untyped), ("join", Ty::Untyped),
            ("escape", Ty::Str), ("unescape", Ty::Str),
            ("encode_www_form", Ty::Str), ("decode_www_form", Ty::Untyped),
        ], &[]);
        // `Set` is a value type: `Set.new` yields `Class { Set }` (via the
        // universal `.new`), then these instance methods dispatch on it.
        // Mutators return the receiver (self) for chaining; element-typed
        // accessors are `Untyped` (Set isn't parameterized here).
        let set_self = Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] };
        register_stdlib_class(&mut classes, "Set", &[], &[
            ("<<", set_self.clone()), ("add", set_self.clone()),
            ("delete", set_self.clone()), ("merge", set_self.clone()),
            ("add?", Ty::Untyped), ("each", Ty::Untyped),
            ("map", Ty::Array { elem: Box::new(Ty::Untyped) }),
            ("include?", Ty::Bool), ("member?", Ty::Bool), ("empty?", Ty::Bool),
            ("size", Ty::Int), ("length", Ty::Int), ("count", Ty::Int),
            ("to_a", Ty::Array { elem: Box::new(Ty::Untyped) }),
            ("subset?", Ty::Bool), ("superset?", Ty::Bool),
        ]);

        // Gem / ecosystem catalog (`crate::catalog::gems`). Targeting
        // Rails realistically means targeting its gem ecosystem;
        // rather than enumerate every gem, we register the surface
        // apps actually call (Arel, ROTP, Nokogiri, …) by discovery.
        // Registered like the stdlib singletons — `or_insert`, so a
        // user class of the same name still wins.
        for gem in crate::catalog::GEM_CATALOG {
            let class_methods: Vec<(&str, Ty)> =
                gem.class_methods.iter().map(|(n, k)| (*n, k.to_ty())).collect();
            let instance_methods: Vec<(&str, Ty)> =
                gem.instance_methods.iter().map(|(n, k)| (*n, k.to_ty())).collect();
            register_stdlib_class(&mut classes, gem.name, &class_methods, &instance_methods);
        }

        // Hardcoded ApplicationController-ish surface. Real inheritance chains
        // and per-controller overrides land when a fixture forces them.
        let mut app_ctrl = ClassInfo::default();
        let params_ty = Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        };
        app_ctrl.class_methods.insert(Symbol::from("params"), params_ty);
        app_ctrl.class_methods.insert(Symbol::from("session"),
            Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) });
        app_ctrl.class_methods.insert(Symbol::from("render"), Ty::Nil);
        app_ctrl.class_methods.insert(Symbol::from("redirect_to"), Ty::Nil);
        app_ctrl.class_methods.insert(Symbol::from("head"), Ty::Nil);
        // HTTP cache-control declarations (`expires_in 3.minutes,
        // public: true`) — side-effecting header writes.
        app_ctrl.class_methods.insert(Symbol::from("expires_in"), Ty::Nil);
        app_ctrl.class_methods.insert(Symbol::from("expires_now"), Ty::Nil);
        // `flash` (FlashHash) and the current action/controller names are
        // available on the controller via implicit self, same as in views.
        app_ctrl.class_methods.insert(
            Symbol::from("flash"),
            Ty::Class {
                id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
                args: vec![],
            },
        );
        for m in ["action_name", "controller_name", "controller_path"] {
            app_ctrl.class_methods.insert(Symbol::from(m), Ty::Str);
        }
        // Route URL helpers (controller side — `redirect_to articles_url`).
        for name in &route_helper_names {
            app_ctrl
                .class_methods
                .entry(Symbol::from(name.as_str()))
                .or_insert(Ty::Str);
        }
        // `respond_to do |format| ... end` — yields the mime Collector
        // registered above, so the `format` block param (and `format.html`/
        // `format.json` calls) type. Block-yielding Fn; result is nil.
        app_ctrl.class_methods.insert(
            Symbol::from("respond_to"),
            block_fn(
                &Ty::Class {
                    id: ClassId(Symbol::from("ActionController::MimeResponds::Collector")),
                    args: vec![],
                },
                Ty::Nil,
            ),
        );
        // `request` / `response` / `logger` return framework objects
        // (ActionDispatch::Request, etc.) we don't model structurally.
        // Gradual `Untyped` so chains like `request.referer` /
        // `request.remote_ip` / `request.env[...]` flow through
        // dispatch instead of bottoming out at Var.
        app_ctrl.class_methods.insert(Symbol::from("request"), Ty::Untyped);
        app_ctrl.class_methods.insert(Symbol::from("response"), Ty::Untyped);
        app_ctrl.class_methods.insert(Symbol::from("logger"), Ty::Untyped);
        // Devise scope helpers. A model declaring the `devise` DSL
        // (`class User; devise :registerable, …`) makes Devise generate
        // `current_user` / `user_signed_in?` / `authenticate_user!` on
        // every controller — the app's own declaration is the fact
        // source, no convention guessing. `current_<scope>` is nilable
        // (no signed-in user); the session object is opaque. Without
        // this, `current_user` bottoms out unresolved and cascades into
        // every `@account = current_account`-style controller ivar
        // (343 sites in Mastodon).
        for model in &app.models {
            let declares_devise = model.body.iter().any(|item| {
                let ModelBodyItem::Unknown { expr, .. } = item else { return false };
                matches!(
                    &*expr.node,
                    ExprNode::Send { recv: None, method, .. } if method.as_str() == "devise"
                )
            });
            if !declares_devise {
                continue;
            }
            let scope = crate::naming::snake_case(
                model.name.0.as_str().rsplit("::").next().unwrap_or(""),
            );
            let model_ty = Ty::Class { id: model.name.clone(), args: vec![] };
            app_ctrl.class_methods.insert(
                Symbol::from(format!("current_{scope}").as_str()),
                Ty::Union { variants: vec![model_ty.clone(), Ty::Nil] },
            );
            app_ctrl.class_methods.insert(
                Symbol::from(format!("{scope}_signed_in?").as_str()),
                Ty::Bool,
            );
            app_ctrl.class_methods.insert(
                Symbol::from(format!("authenticate_{scope}!").as_str()),
                Ty::Nil,
            );
            app_ctrl.class_methods.insert(
                Symbol::from(format!("{scope}_session").as_str()),
                Ty::Untyped,
            );
            for m in ["sign_in", "sign_out", "bypass_sign_in"] {
                app_ctrl
                    .class_methods
                    .entry(Symbol::from(m))
                    .or_insert(Ty::Untyped);
            }
            // Devise marks `current_<scope>` / `<scope>_signed_in?` as
            // `helper_method`, so templates see them too — register on
            // the view context (inserted into `classes` above).
            if let Some(view_cls) =
                classes.get_mut(&ClassId(Symbol::from("ActionView::Base")))
            {
                view_cls.instance_methods.insert(
                    Symbol::from(format!("current_{scope}").as_str()),
                    Ty::Union { variants: vec![model_ty, Ty::Nil] },
                );
                view_cls.instance_methods.insert(
                    Symbol::from(format!("{scope}_signed_in?").as_str()),
                    Ty::Bool,
                );
            }
        }
        classes.insert(ClassId(Symbol::from("ApplicationController")), app_ctrl);

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

        // Library classes: non-model classes living under app/models/
        // (e.g. specialized has_many proxies). Register each as a known
        // class so references like `ArticleCommentsProxy.new(self)` from
        // model methods resolve. Method-by-method registration with
        // proper signatures is a follow-up; for now an empty ClassInfo
        // is enough to type the constructor reference.
        for lc in &app.library_classes {
            let cls = classes.entry(lc.name.clone()).or_default();
            // A helper module's own `include`s carry transitively to
            // any class that includes it; record them so dispatch can
            // chase nested mixins.
            cls.includes = lc.includes.clone();
            // `include Singleton` provides `.instance` returning the
            // singleton — the one stdlib mixin worth special-casing:
            // service objects use it pervasively
            // (`ActivityPub::TagManager.instance.uri_for(...)`) and the
            // module itself is stdlib, never ingested, so the concern
            // fold can't supply it.
            if lc.includes.iter().any(|i| i.0.as_str() == "Singleton") {
                cls.class_methods.entry(Symbol::from("instance")).or_insert(Ty::Class {
                    id: lc.name.clone(),
                    args: vec![],
                });
            }
            // Carry the superclass link so inheritance dispatch walks it.
            // Crucial for classes extending an *unmodeled* gem parent
            // (`TimeSeries < SVG::Graph::TimeSeries`): the walk reaches the
            // unknown ancestor and treats inherited methods as gradual
            // rather than erroring. `is_some` guard so we never clobber a
            // parent another pass established with `None`.
            if lc.parent.is_some() {
                cls.parent = lc.parent.clone();
            }
        }

        // ActionMailer classes: a mailer declares its actions as plain
        // instance `def`s (`def notify(user, …)`) but Rails invokes them
        // on the *class* and returns a deliverable
        // (`BanNotification.notify(…).deliver_now`). The library-class
        // ingest above captured those as instance methods + the
        // `ApplicationMailer < ActionMailer::Base` parent link, so here we
        // (a) identify mailer classes by walking the parent chain to
        // `ActionMailer::Base`, then (b) re-expose each public action as a
        // *class* method returning `ActionMailer::MessageDelivery`. Without
        // this, `Mailer.action` dispatches to "no known method" (no
        // class-side method exists). `entry().or_insert` so a real
        // class-side `def self.x` always wins.
        {
            let parent_of: HashMap<&ClassId, Option<&ClassId>> = app
                .library_classes
                .iter()
                .map(|lc| (&lc.name, lc.parent.as_ref()))
                .collect();
            let is_mailer = |start: &ClassId| -> bool {
                let mut cur = Some(start);
                let mut depth = 0usize;
                while let Some(id) = cur {
                    if id.0.as_str() == "ActionMailer::Base" {
                        return true;
                    }
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    cur = parent_of.get(id).copied().flatten();
                }
                false
            };
            let delivery_ty = Ty::Class {
                id: ClassId(Symbol::from("ActionMailer::MessageDelivery")),
                args: vec![],
            };
            for lc in &app.library_classes {
                if !is_mailer(&lc.name) {
                    continue;
                }
                let cls = classes.entry(lc.name.clone()).or_default();
                cls.parent = lc.parent.clone();
                for method in &lc.methods {
                    // Only source-defined instance actions become
                    // class-callable. Real `def self.x` (Class receiver),
                    // synthesized accessors, and `initialize` are not
                    // mailer actions.
                    if method.receiver != crate::dialect::MethodReceiver::Instance
                        || method.kind != crate::dialect::AccessorKind::Method
                        || method.name.as_str() == "initialize"
                    {
                        continue;
                    }
                    cls.class_methods
                        .entry(method.name.clone())
                        .or_insert_with(|| delivery_ty.clone());
                }
            }

            // The deliverable returned by a mailer action. `deliver_now`
            // sends synchronously (really returning the `Mail::Message`);
            // `deliver_later` enqueues an ActiveJob. We model *every*
            // `deliver_*` as returning the delivery itself — the actual
            // `Mail::Message` return is deliberately NOT modeled, because a
            // bare `Mail::Message` class would collide with an app `Message`
            // model under single-segment const resolution (a real lobsters
            // hazard: `Message.find` would resolve to the mail class). The
            // delivery result is invariably discarded at the call site, so
            // a concrete self-type both avoids that collision and keeps the
            // `.deliver_*` link off the gradual-escape (`Untyped`) path.
            let mut delivery_cls = ClassInfo::default();
            for m in [
                "deliver_now",
                "deliver_now!",
                "deliver",
                "deliver_later",
                "deliver_later!",
            ] {
                delivery_cls
                    .instance_methods
                    .insert(Symbol::from(m), delivery_ty.clone());
            }
            delivery_cls
                .instance_methods
                .insert(Symbol::from("processed?"), Ty::Bool);
            classes
                .entry(ClassId(Symbol::from("ActionMailer::MessageDelivery")))
                .or_insert(delivery_cls);
        }

        // Sidekiq workers: `include Sidekiq::Worker` grants the
        // class-side enqueue surface — the app defines an instance
        // `def perform(…)` but *calls* `FooWorker.perform_async(…)` /
        // `perform_in(delay, …)` / `perform_at(time, …)`, all of which
        // return the job id String (invariably discarded). Same shape
        // as the mailer pass above: identify workers by walking the
        // parent chain (Mastodon subclasses base workers, e.g.
        // `UpdateDistributionWorker < RawDistributionWorker`) checking
        // each level's `include` list, then register the enqueue
        // methods. `entry().or_insert` so a real `def self.` wins.
        {
            let lc_of: HashMap<&ClassId, &crate::dialect::LibraryClass> = app
                .library_classes
                .iter()
                .map(|lc| (&lc.name, lc))
                .collect();
            let is_worker = |start: &ClassId| -> bool {
                let mut cur = Some(start);
                let mut depth = 0usize;
                while let Some(id) = cur {
                    let Some(lc) = lc_of.get(id) else { break };
                    if lc
                        .includes
                        .iter()
                        .any(|inc| inc.0.as_str() == "Sidekiq::Worker")
                    {
                        return true;
                    }
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    cur = lc.parent.as_ref();
                }
                false
            };
            for lc in &app.library_classes {
                if !is_worker(&lc.name) {
                    continue;
                }
                let cls = classes.entry(lc.name.clone()).or_default();
                if cls.parent.is_none() {
                    cls.parent = lc.parent.clone();
                }
                for m in ["perform_async", "perform_in", "perform_at"] {
                    cls.class_methods
                        .entry(Symbol::from(m))
                        .or_insert(Ty::Str);
                }
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

        Self {
            classes,
            inferred_params: HashMap::new(),
            adapter,
            concern_folded: HashMap::new(),
        }
    }

    /// Build a body-typer borrowing this analyzer's dispatch tables.
    /// Cheap — just a struct with a reference.
    fn body_typer(&self) -> BodyTyper<'_> {
        BodyTyper::new(&self.classes)
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
        self.inferred_params.get(&(class.clone(), method.clone())).map(|v| v.as_slice())
    }

    /// Walk the app, annotating every expression's `ty` field, then
    /// populating the owning construct's `effects` by visiting the typed tree.
    ///
    /// Two-phase: an initial typing pass over the whole app, then a
    /// whole-program fixpoint loop that (a) harvests inferred return
    /// types from method bodies into the dispatch registry, (b) unifies
    /// parameter types across call sites, and (c) re-runs typing with
    /// the refined registry. Iterates to a fixed point (cap of 4 like
    /// Spinel) using a signature fingerprint to detect convergence.
    pub fn analyze(&mut self, app: &mut App) {
        self.run_typing_passes(app);

        // Whole-program fixpoint: harvest returns + unify params, re-type,
        // repeat until the registry signature stabilizes. Cap matches
        // Spinel's empirically-observed "1-2 iterations typically; 4 is a
        // safety net" — see `~/git/spinel/spinel_codegen.rb:7459-7492`.
        let mut prev_sig = self.inference_signature();
        for _ in 0..4 {
            self.harvest_returns_to_registry(app);
            self.unify_params_from_call_sites(app);
            let cur_sig = self.inference_signature();
            if cur_sig == prev_sig {
                break;
            }
            prev_sig = cur_sig;
            // Re-type the whole app with the refined registry. Idempotent
            // BodyTyper means a second pass simply resolves dispatches
            // and Var bindings the first pass couldn't.
            self.run_typing_passes(app);
        }
    }

    /// Collect every app-level constant (`CONST = <value>`) declared in a
    /// model or controller body, type its value, and build a global
    /// name→type registry keyed by the constant's last path segment — the
    /// shape `ExprNode::Const` dispatch consults (`Vote::COMMENT_REASONS`
    /// looks up `COMMENT_REASONS`). This is the cross-class channel: a
    /// constant declared in `Vote` resolves when referenced from a
    /// controller, a view, another model, or seeds — none of which the
    /// per-class `extract_*_const_assignments` tables reach.
    ///
    /// Typed as a small fixpoint so a constant defined in terms of another
    /// (`ALL_COMMENT_REASONS = COMMENT_REASONS.merge(...).freeze`) resolves
    /// once its dependency does. A name declared in two classes with
    /// conflicting types is dropped as ambiguous: a bare reference can't
    /// be disambiguated without lexical scope (mirrors `expand_bare_const`).
    /// `Ty::Var` results are skipped — uninformative, and registering them
    /// would only mask the `Const` fallback without adding signal.
    fn build_constant_registry(&self, app: &App) -> HashMap<Symbol, Ty> {
        // (defining class's self_ty, last-segment name, cloned value expr).
        let mut entries: Vec<(Ty, Symbol, Expr)> = Vec::new();
        let mut push_const = |self_ty: Ty, expr: &Expr| {
            if let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node {
                if let Some(last) = path.last() {
                    entries.push((self_ty, last.clone(), value.clone()));
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

        let mut map: HashMap<Symbol, Ty> = HashMap::new();
        let mut ambiguous: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        // Cap matches the outer analyze fixpoint; one level of constant
        // dependency needs two passes, the cap leaves slack.
        for _ in 0..4 {
            let mut next: HashMap<Symbol, Ty> = HashMap::new();
            for (self_ty, name, value) in entries.iter_mut() {
                if ambiguous.contains(name) {
                    continue;
                }
                let ctx = Ctx {
                    self_ty: Some(self_ty.clone()),
                    ivar_bindings: HashMap::new(),
                    local_bindings: HashMap::new(),
                    constants: map.clone(),
                    annotate_self_dispatch: false,
                    in_view: false,
                };
                let ty = self.body_typer().analyze_expr(value, &ctx);
                if matches!(ty, Ty::Var { .. }) {
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
            for a in &ambiguous {
                next.remove(a);
            }
            if next == map {
                break;
            }
            map = next;
        }
        map
    }

    /// One full typing pass over the whole app. Extracted from
    /// `analyze` so the fixpoint loop above can re-invoke it after
    /// each registry refinement. The Rails-aware orchestration
    /// (controller→view ivar channel, before_action seeding,
    /// per-model two-pass ivar discovery, partial locals threading)
    /// stays internal to this method; the fixpoint just calls it.
    fn run_typing_passes(&self, app: &mut App) {
        // Global constant registry (`Vote::COMMENT_REASONS` → `Hash[..]`,
        // `User::NEW_USER_DAYS` → `Int`), shared across every class, view,
        // and seeds so cross-class constant references resolve to the
        // value's type instead of the `Ty::Class { id: ConstName }`
        // fallback. Seeded under each class's own constants (own shadows
        // global on a name clash).
        let global_constants = self.build_constant_registry(app);
        // Controller→view ivar channel: as each action is analyzed, we harvest
        // the ivars it sets and key them by the view that action renders.
        // When we reach the view pass below, the view's Ctx is seeded from
        // this map so `@article.title` in `articles/show.html.erb` types
        // against the `@article` bound in `ArticlesController#show`.
        let mut action_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
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
        let dynamic_render_ivars: std::collections::HashSet<Symbol> = {
            let mut set = std::collections::HashSet::new();
            for view in &app.views {
                collect_dynamic_render_ivars(&view.body, &mut set);
            }
            set
        };
        let existing_view_names: std::collections::HashSet<Symbol> =
            app.views.iter().map(|v| v.name.clone()).collect();
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
            /// Per-method effect sets (own actions + concern methods
            /// typed against this controller's self), for the persisted
            /// chain's per-hop effects.
            action_effects: HashMap<Symbol, EffectSet>,
            class_constants: HashMap<Symbol, Ty>,
            layout: LayoutDecl,
        }
        let mut meta_by_name: HashMap<ClassId, ControllerMeta> = HashMap::new();
        // Snapshot parent links separately so Phase B can walk
        // arbitrary-depth chains without re-borrowing `app.controllers`
        // (which is mutably borrowed inside the analysis loops).
        let parent_link_by_name: HashMap<ClassId, Option<ClassId>> = app
            .controllers
            .iter()
            .map(|c| (c.name.clone(), c.parent.clone()))
            .collect();

        // Concern-module metadata for the mixed-in expansion inside
        // Phase A: each module's method defs (cloned so their bodies can
        // be typed against each includer's own self) and its `include`s
        // (concerns include concerns; the expansion chases the closure).
        // Filters captured from `included do` blocks ride on
        // `App::concern_filters`.
        let module_methods: HashMap<ClassId, Vec<crate::dialect::MethodDef>> = app
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
        let concern_filters_map = app.concern_filters.clone();

        // ── Phase A: type Unknown body items + every action body
        // ── once per controller, with no parent inheritance.
        for controller in &mut app.controllers {
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
                constants: global_constants.clone(),
                annotate_self_dispatch: false, in_view: false,
            };
            for item in controller.body.iter_mut() {
                if let ControllerBodyItem::Unknown { expr, .. } = item {
                    self.body_typer().analyze_expr(expr, &const_ctx);
                }
            }
            // Own constants layered over the global registry — a same-named
            // constant declared on this controller shadows another class's.
            let mut class_constants = global_constants.clone();
            class_constants.extend(extract_controller_const_assignments(&controller.body));

            let ctx = Ctx {
                self_ty: Some(self_ty.clone()),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                constants: class_constants.clone(),
                annotate_self_dispatch: false, in_view: false,
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
            let (sourced_filters, block_filter_bindings) =
                build_sourced_filter_chain(controller, &concern_filters_map, &module_includes);

            // Pass A: analyze every action body once. Helper-method
            // params (`period(query)`) are seeded from the inferred-
            // params table so their bodies — and thus their harvested
            // return types — resolve; routed actions have empty param
            // rows and seed nothing.
            let ctrl_id = controller.name.clone();
            for action in controller.actions_mut() {
                let mctx =
                    self.seed_action_params(&ctx, &ctrl_id, &action.name, &action.params);
                self.body_typer().analyze_expr(&mut action.body, &mctx);
                action.effects = self.collect_effects(&mut action.body, &mctx);
            }

            // Snapshot each action's ivar bindings (this controller's
            // own actions only — parent's actions get layered in by
            // Phase B's `chained_bindings` builder).
            let mut action_bindings: HashMap<Symbol, HashMap<Symbol, Ty>> = controller
                .actions()
                .map(|a| {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&a.body, &mut ivars);
                    (a.name.clone(), ivars)
                })
                .collect();

            // Register each block filter's synthetic target so the seeding
            // lookups (`merged_before_seed`, the view-ivar build) resolve it.
            for (target, ivars) in block_filter_bindings {
                action_bindings.insert(target, ivars);
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
            let mut concern_effects: HashMap<Symbol, EffectSet> = HashMap::new();
            for module_id in &mixed_in {
                let Some(methods) = module_methods.get(module_id) else { continue };
                for method in methods {
                    if action_bindings.contains_key(&method.name) {
                        continue;
                    }
                    let mut body = method.body.clone();
                    self.body_typer().analyze_expr(&mut body, &ctx);
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&body, &mut ivars);
                    if !ivars.is_empty() {
                        action_bindings.insert(method.name.clone(), ivars);
                    }
                    let effects = self.collect_effects(&mut body, &ctx);
                    if !effects.is_pure() {
                        concern_effects.entry(method.name.clone()).or_insert(effects);
                    }
                }
            }

            // Per-method effect snapshot for the persisted chain: own
            // methods (typed by Pass A) overwrite concern methods —
            // a shadowed module method never runs.
            let mut action_effects = concern_effects;
            for action in controller.actions() {
                action_effects.insert(action.name.clone(), action.effects.clone());
            }

            let layout = controller.layout.clone();
            meta_by_name.insert(
                controller.name.clone(),
                ControllerMeta {
                    self_ty,
                    sourced_filters,
                    action_bindings,
                    action_effects,
                    class_constants,
                    layout,
                },
            );
        }

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

        // ── Phase B: walk each controller's parent chain to build
        // ── inherited (chained) filters + action bindings, then
        // ── re-analyze actions and harvest view ivars.
        //
        // Parent filters run BEFORE child filters (Rails semantics).
        // Action bindings are merged with NEAREST parent first so the
        // closest definition wins on name conflicts (mirrors Ruby
        // method-resolution order).
        for controller in &mut app.controllers {
            let ctrl_name = controller.name.clone();
            let Some(meta) = meta_by_name.get(&ctrl_name) else { continue };

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
            for (_, ancestor) in ancestors.iter().rev() {
                for (name, ivars) in &ancestor.action_bindings {
                    chained_bindings.insert(name.clone(), ivars.clone());
                }
            }
            for (name, ivars) in &meta.action_bindings {
                chained_bindings.insert(name.clone(), ivars.clone());
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
                            if matches!(v, Ty::Var { .. } | Ty::Bottom) {
                                continue;
                            }
                            let merged = match env.remove(k) {
                                Some(prev) => crate::analyze::body::union_of(prev, v.clone()),
                                None => v.clone(),
                            };
                            env.insert(k.clone(), merged);
                        }
                    }
                    env.into_iter()
                        .map(|(k, v)| (k, strip_nil(v)))
                        .collect()
                };

                // Pass B: re-analyze every method with the controller-wide
                // base seed plus any before_action-specific overlay. Every
                // method (routed action or private helper) is re-analyzed
                // so cross-method ivar reads resolve.
                if !controller_wide.is_empty() || !chained_filters.is_empty() {
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
                            constants: meta.class_constants.clone(),
                            annotate_self_dispatch: false, in_view: false,
                        };
                        // Seed helper-method params from the inferred-params
                        // table too, so `period(query)`'s body resolves on
                        // the re-analysis pass (matches Pass A).
                        let inner_ctx = self.seed_action_params(
                            &base_ctx,
                            &ctrl_name,
                            &action.name,
                            &action.params,
                        );
                        self.body_typer().analyze_expr(&mut action.body, &inner_ctx);
                        action.effects = self.collect_effects(&mut action.body, &inner_ctx);
                    }
                }

                if sweep == 1 {
                    break;
                }
                // Re-harvest bindings from the retyped bodies; only a
                // refinement (a previously Var/absent binding now
                // carrying shape) triggers the second sweep.
                let mut refined = false;
                for action in controller.actions() {
                    let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                    extract_ivar_assignments(&action.body, &mut ivars);
                    for (k, v) in ivars {
                        if matches!(v, Ty::Var { .. } | Ty::Bottom) {
                            continue;
                        }
                        let entry = chained_bindings.entry(action.name.clone()).or_default();
                        let stale = entry
                            .get(&k)
                            .is_none_or(|t| matches!(t, Ty::Var { .. } | Ty::Bottom));
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
                    LayoutDecl::None => None,
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
                let noise = |t: &Ty| matches!(t, Ty::Var { .. } | Ty::Bottom);
                let filter_chain: Vec<crate::app::ResolvedFilter> = chained_filters
                    .iter()
                    .map(|(filter, defined_in, included_via)| {
                        let runs = !matches!(filter.kind, FilterKind::Skip);
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
                extract_ivar_assignments(&action.body, &mut ivars);
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
                        let noise = |t: &Ty| matches!(t, Ty::Var { .. } | Ty::Untyped);
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
                            if matches!(v, Ty::Var { .. } | Ty::Bottom) {
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
                collect_action_render_views(&action.body, &prefix, &mut view_targets);
                view_targets.sort();
                view_targets.dedup();
                for view_name in view_targets {
                    view_feeders.entry(view_name.clone()).or_default().insert(ctrl_name.clone());
                    let entry = action_ivars_by_view.entry(view_name).or_default();
                    for (k, v) in &ivars {
                        let noise = |t: &Ty| matches!(t, Ty::Var { .. } | Ty::Untyped);
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
                            let noise = |t: &Ty| matches!(t, Ty::Var { .. } | Ty::Untyped);
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
        for model in &mut app.models {
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
                class_ivars.insert(
                    name.clone(),
                    Ty::Union {
                        variants: vec![ty.clone(), Ty::Nil],
                    },
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
                constants: global_constants.clone(),
                annotate_self_dispatch: false, in_view: false,
            };
            for item in model.body.iter_mut() {
                if let ModelBodyItem::Unknown { expr, .. } = item {
                    self.body_typer().analyze_expr(expr, &const_ctx);
                }
            }
            // Own constants layered over the global registry (own shadows).
            let mut class_constants = global_constants.clone();
            class_constants.extend(extract_const_assignments(&model.body));

            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: class_ivars.clone(),
                local_bindings: HashMap::new(),
                constants: class_constants.clone(),
                annotate_self_dispatch: false, in_view: false,
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
                let mctx = self.seed_method_params(&class_ctx, &model_name, method);
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
                for (name, ty) in flow_ivars {
                    let union_ty = Ty::Union { variants: vec![ty, Ty::Nil] };
                    reseeded.insert(name, union_ty);
                }
                let reseeded_ctx = Ctx {
                    self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                    ivar_bindings: reseeded,
                    local_bindings: HashMap::new(),
                    constants: class_constants.clone(),
                    annotate_self_dispatch: false, in_view: false,
                };

                for scope in model.scopes_mut() {
                    self.body_typer().analyze_expr(&mut scope.body, &reseeded_ctx);
                }
                for method in model.methods_mut() {
                    let mctx = self.seed_method_params(&reseeded_ctx, &model_name, method);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                    method.effects = self.collect_effects(&mut method.body, &mctx);
                }
            } else {
                for method in model.methods_mut() {
                    let mctx = self.seed_method_params(&class_ctx, &model_name, method);
                    method.effects = self.collect_effects(&mut method.body, &mctx);
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

        for lc in &mut app.library_classes {
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: lc.name.clone(), args: vec![] }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                constants: HashMap::new(), annotate_self_dispatch: false, in_view: false,
            };

            let lc_name = lc.name.clone();
            for method in &mut lc.methods {
                let mctx = self.seed_method_params(&class_ctx, &lc_name, method);
                self.body_typer().analyze_expr(&mut method.body, &mctx);
            }

            let mut flow_ivars: HashMap<Symbol, Ty> = HashMap::new();
            for method in &lc.methods {
                extract_ivar_assignments(&method.body, &mut flow_ivars);
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
                        ivars.entry(name.clone()).or_insert_with(|| Ty::Union {
                            variants: vec![ty.clone(), Ty::Nil],
                        });
                    }
                    if ivars.is_empty() {
                        continue;
                    }
                    action_ivars_by_view
                        .entry(Symbol::from(
                            format!("{prefix}/{}", method.name.as_str()).as_str(),
                        ))
                        .or_default()
                        .extend(ivars);
                }
            }

            if !flow_ivars.is_empty() {
                let mut reseeded: HashMap<Symbol, Ty> = HashMap::new();
                for (name, ty) in flow_ivars {
                    reseeded.insert(name, Ty::Union { variants: vec![ty, Ty::Nil] });
                }
                let reseeded_ctx = Ctx {
                    self_ty: Some(Ty::Class { id: lc_name.clone(), args: vec![] }),
                    ivar_bindings: reseeded,
                    local_bindings: HashMap::new(),
                    constants: HashMap::new(), annotate_self_dispatch: false, in_view: false,
                };
                for method in &mut lc.methods {
                    let mctx = self.seed_method_params(&reseeded_ctx, &lc_name, method);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                    method.effects = self.collect_effects(&mut method.body, &mctx);
                }
            } else {
                for method in &mut lc.methods {
                    let mctx = self.seed_method_params(&class_ctx, &lc_name, method);
                    method.effects = self.collect_effects(&mut method.body, &mctx);
                }
            }
        }

        // Partial-locals channel: we need action/top-level views analyzed first
        // so their expression types are known at each `render` call site. We
        // then harvest the locals each render passes to the target partial,
        // keying by the partial's view name, and analyze partials with that
        // seed. Nested partial-of-partial isn't handled here (would need a
        // fixpoint); real-blog's dependency graph is shallow enough to skip.
        let mut partial_locals_by_name: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        // The ivar context each view carries: action views key by their
        // own name, layouts fall through to the layout-ivar union. Built
        // here so it can both seed non-partial views and be propagated to
        // the partials they render.
        let view_ivar_seed = |name: &Symbol| -> HashMap<Symbol, Ty> {
            action_ivars_by_view
                .get(name)
                .or_else(|| layout_ivars_by_view.get(name))
                .cloned()
                .unwrap_or_default()
        };

        // Renderer → partials-it-renders edges, harvested as views are
        // walked. Drives the ivar propagation below.
        let mut render_edges: HashMap<Symbol, Vec<Symbol>> = HashMap::new();

        // Phase 3a: non-partial views (action views + layouts). Analyze with
        // the controller→view ivar seed, then walk the body to record every
        // `render` call's effect on partial_locals_by_name.
        for view in &mut app.views {
            if is_partial_view_name(&view.name) {
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
            // Action views look up by view name (e.g. `articles/show`);
            // layout views (`layouts/application`) have no matching
            // action and fall through to the layout-ivar map, which is
            // the union of every action whose `effective_layout`
            // resolved to this layout.
            view_ctx.ivar_bindings = view_ivar_seed(&view.name);
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
            let mut targets = Vec::new();
            extract_partial_render_sites(
                &view.body,
                &view.name,
                &mut partial_locals_by_name,
                &mut targets,
            );
            render_edges.insert(view.name.clone(), targets);
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
            render_edges.insert(view.name.clone(), targets);
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
        let noise = |t: &Ty| matches!(t, Ty::Var { .. } | Ty::Untyped);
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
                        if noise(v) {
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
            if let Some(ivars) = partial_ivars_by_name.get(&view.name) {
                view_ctx.ivar_bindings = ivars.clone();
            }
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
        }

        // Seeds body (db/seeds.rb). Top-level Ruby: no `self`, no
        // ivars, no before-action scaffolding. Just an expression
        // that references model classes. Types so that Send effects
        // flow (DbWrite on `Article.create!`, DbRead on
        // `Article.count`), which the emitter uses for await
        // placement under async adapters.
        if let Some(expr) = app.seeds.as_mut() {
            let mut ctx = Ctx::default();
            ctx.constants = global_constants.clone();
            self.body_typer().analyze_expr(expr, &ctx);
            let _ = self.collect_effects(expr, &ctx);
        }
    }

    fn collect_effects(&self, expr: &mut Expr, ctx: &Ctx) -> EffectSet {
        let mut set = BTreeSet::new();
        self.visit_effects(expr, ctx, &mut set);
        EffectSet { effects: set }
    }

    /// Build a per-method `Ctx` by cloning `base` and seeding
    /// `local_bindings` with parameter types harvested from
    /// `inferred_params`. When no entry exists for the (class, method)
    /// pair, the params stay unbound and the body-typer falls back to
    /// `Ty::Var` for `Var { name }` reads — same as before any
    /// inference ran. Each fixpoint iteration that refines a param's
    /// type makes the next typing pass see a more concrete binding.
    fn seed_method_params(
        &self,
        base: &Ctx,
        class_id: &ClassId,
        method: &crate::dialect::MethodDef,
    ) -> Ctx {
        let key = (class_id.clone(), method.name.clone());
        let Some(types) = self.inferred_params.get(&key) else {
            return base.clone();
        };
        let mut ctx = base.clone();
        for (param, ty) in method.params.iter().zip(types.iter()) {
            if !matches!(ty, Ty::Var { .. }) {
                ctx.local_bindings.insert(param.name.clone(), ty.clone());
            }
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
        action_name: &Symbol,
        params: &Row,
    ) -> Ctx {
        let key = (class_id.clone(), action_name.clone());
        let Some(types) = self.inferred_params.get(&key) else {
            return base.clone();
        };
        let mut ctx = base.clone();
        for (name, ty) in params.fields.keys().zip(types.iter()) {
            if !matches!(ty, Ty::Var { .. }) {
                ctx.local_bindings.insert(name.clone(), ty.clone());
            }
        }
        ctx
    }

    /// Fingerprint of the data the fixpoint refines: per-class
    /// instance/class method return types in `self.classes` plus the
    /// parameter-type table in `self.inferred_params`. The fixpoint
    /// loop in `analyze` compares fingerprints between iterations and
    /// stops when they match. Order-independent so HashMap iteration
    /// order doesn't perturb results: keys are sorted before
    /// stringification.
    fn inference_signature(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut class_keys: Vec<&ClassId> = self.classes.keys().collect();
        class_keys.sort_by_key(|k| k.0.as_str().to_string());
        for cid in class_keys {
            let cls = &self.classes[cid];
            let mut method_keys: Vec<&Symbol> = cls.instance_methods.keys().collect();
            method_keys.sort_by_key(|k| k.as_str().to_string());
            for m in method_keys {
                parts.push(format!("{}#{}={:?}", cid.0.as_str(), m.as_str(), cls.instance_methods[m]));
            }
            let mut cmethod_keys: Vec<&Symbol> = cls.class_methods.keys().collect();
            cmethod_keys.sort_by_key(|k| k.as_str().to_string());
            for m in cmethod_keys {
                parts.push(format!("{}.{}={:?}", cid.0.as_str(), m.as_str(), cls.class_methods[m]));
            }
        }
        let mut param_keys: Vec<&(ClassId, Symbol)> = self.inferred_params.keys().collect();
        param_keys.sort_by_key(|(c, m)| (c.0.as_str().to_string(), m.as_str().to_string()));
        for k in param_keys {
            parts.push(format!("{}#{}~{:?}", k.0.0.as_str(), k.1.as_str(), self.inferred_params[k]));
        }
        parts.join("|")
    }

    /// Walk every model + library_class method body and write its
    /// inferred body type into `self.classes[class].instance_methods`
    /// (or `class_methods` for `def self.x`). Conservative on widening:
    /// only updates the registry when the harvested type is more
    /// specific than what's already there (concrete > Ty::Var; existing
    /// RBS-derived `Ty::Fn` is preserved — its return is already what
    /// dispatch resolves to via `unwrap_fn_ret`). Skip methods whose
    /// body is `Ty::Var` (no information gained).
    fn harvest_returns_to_registry(&mut self, app: &App) {
        for model in &app.models {
            let class_id = &model.name;
            for method in model.methods() {
                let target = match method.receiver {
                    crate::dialect::MethodReceiver::Instance => {
                        &mut self.classes.entry(class_id.clone()).or_default().instance_methods
                    }
                    crate::dialect::MethodReceiver::Class => {
                        &mut self.classes.entry(class_id.clone()).or_default().class_methods
                    }
                };
                Self::register_method_return(
                    target,
                    &method.name,
                    effective_return_ty(&method.body).as_ref(),
                );
            }
        }
        for lc in &app.library_classes {
            let class_id = &lc.name;
            for method in &lc.methods {
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
                Self::register_method_return(
                    target,
                    &method.name,
                    effective_return_ty(&method.body).as_ref(),
                );
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
            for action in controller.actions() {
                let Some(body_ty) = effective_return_ty(&action.body) else { continue };
                if matches!(body_ty, Ty::Var { .. }) {
                    continue;
                }
                let target =
                    &mut self.classes.entry(class_id.clone()).or_default().instance_methods;
                Self::insert_inferred_return(target, &action.name, body_ty);
            }
        }

        self.fold_concern_surfaces(app);
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
    fn fold_concern_surfaces(&mut self, app: &App) {
        type Surface = (HashMap<Symbol, Ty>, HashMap<Symbol, Ty>, Vec<ClassId>);
        let module_surfaces: HashMap<ClassId, Surface> = app
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .filter_map(|lc| {
                let cls = self.classes.get(&lc.name)?;
                Some((
                    lc.name.clone(),
                    (
                        cls.instance_methods.clone(),
                        cls.class_methods.clone(),
                        cls.includes.clone(),
                    ),
                ))
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
        match table.get(method) {
            Some(Ty::Fn { .. }) => return,
            Some(existing) if !matches!(existing, Ty::Var { .. }) && existing == &ty => return,
            _ => {}
        }
        table.insert(method.clone(), ty);
    }

    /// Walk every Send across the app, look up each call's target
    /// method, and unify the argument types into
    /// `self.inferred_params` for that (class, method). Mirrors
    /// Spinel's `detect_poly_params` (`spinel_codegen.rb:6928-7052`)
    /// at a higher level — we work with structured `Ty` values rather
    /// than string fingerprints, so unification is direct: same type →
    /// keep; nil + T → T?; otherwise → union widen.
    fn unify_params_from_call_sites(&mut self, app: &App) {
        let mut sites: Vec<(ClassId, Symbol, Vec<Ty>)> = Vec::new();
        for model in &app.models {
            for method in model.methods() {
                self.collect_send_sites(&method.body, Some(&model.name), &mut sites);
            }
            for scope in model.scopes() {
                self.collect_send_sites(&scope.body, Some(&model.name), &mut sites);
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                self.collect_send_sites(&method.body, Some(&lc.name), &mut sites);
            }
        }
        for controller in &app.controllers {
            for action in controller.actions() {
                self.collect_send_sites(&action.body, Some(&controller.name), &mut sites);
            }
        }
        for view in &app.views {
            self.collect_send_sites(&view.body, None, &mut sites);
        }
        if let Some(seeds) = &app.seeds {
            self.collect_send_sites(seeds, None, &mut sites);
        }

        for (class_id, method, arg_tys) in sites {
            // Cross-reference against MethodDef.params to know the
            // arity. If the called method's params can't be located,
            // still accumulate up to arg count under the same key —
            // RBS-only methods don't have a MethodDef but do have an
            // Fn signature, and inferred_params can extend either way.
            let arity = arg_tys.len();
            let entry = self
                .inferred_params
                .entry((class_id.clone(), method.clone()))
                .or_insert_with(|| (0..arity).map(|_| Ty::Var { var: crate::ident::TyVar(0) }).collect());
            if entry.len() < arity {
                entry.resize(arity, Ty::Var { var: crate::ident::TyVar(0) });
            }
            for (slot, observed) in entry.iter_mut().zip(arg_tys.into_iter()) {
                *slot = unify_param_ty(slot.clone(), observed);
            }
        }
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
        out: &mut Vec<(ClassId, Symbol, Vec<Ty>)>,
    ) {
        match &*expr.node {
            ExprNode::Send { recv, method, args, block, .. } => {
                // Resolve the receiver class: an explicit receiver
                // typed as a class, or — for an implicit-self call
                // (`period(query)` inside a controller) — the
                // enclosing class. The latter is what lets a sibling
                // method's params be inferred from its self-call sites.
                let recv_class = match recv {
                    Some(r) => match r.ty.as_ref() {
                        Some(Ty::Class { id, .. }) => Some(id.clone()),
                        _ => None,
                    },
                    None => self_class.cloned(),
                };
                if let Some(class_id) = recv_class {
                    let arg_tys: Vec<Ty> = args
                        .iter()
                        .map(|a| a.ty.clone().unwrap_or(Ty::Var { var: crate::ident::TyVar(0) }))
                        .collect();
                    out.push((class_id, method.clone(), arg_tys));
                }
                if let Some(r) = recv { self.collect_send_sites(r, self_class, out); }
                for a in args { self.collect_send_sites(a, self_class, out); }
                if let Some(b) = block { self.collect_send_sites(b, self_class, out); }
            }
            ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
                for e in exprs { self.collect_send_sites(e, self_class, out); }
            }
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    self.collect_send_sites(k, self_class, out);
                    self.collect_send_sites(v, self_class, out);
                }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.collect_send_sites(cond, self_class, out);
                self.collect_send_sites(then_branch, self_class, out);
                self.collect_send_sites(else_branch, self_class, out);
            }
            ExprNode::Case { scrutinee, arms } => {
                self.collect_send_sites(scrutinee, self_class, out);
                for arm in arms {
                    if let Some(g) = &arm.guard { self.collect_send_sites(g, self_class, out); }
                    self.collect_send_sites(&arm.body, self_class, out);
                }
            }
            ExprNode::BoolOp { left, right, .. }
            | ExprNode::RescueModifier { expr: left, fallback: right } => {
                self.collect_send_sites(left, self_class, out);
                self.collect_send_sites(right, self_class, out);
            }
            ExprNode::Let { value, body, .. } => {
                self.collect_send_sites(value, self_class, out);
                self.collect_send_sites(body, self_class, out);
            }
            ExprNode::Lambda { body, .. } => self.collect_send_sites(body, self_class, out),
            ExprNode::Apply { fun, args, block } => {
                self.collect_send_sites(fun, self_class, out);
                for a in args { self.collect_send_sites(a, self_class, out); }
                if let Some(b) = block { self.collect_send_sites(b, self_class, out); }
            }
            ExprNode::Assign { target, value }
            | ExprNode::OpAssign { target, value, .. } => {
                self.collect_send_sites(value, self_class, out);
                if let LValue::Attr { recv, .. } = target {
                    self.collect_send_sites(recv, self_class, out);
                }
                if let LValue::Index { recv, index } = target {
                    self.collect_send_sites(recv, self_class, out);
                    self.collect_send_sites(index, self_class, out);
                }
            }
            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.collect_send_sites(expr, self_class, out);
                    }
                }
            }
            ExprNode::Yield { args } => {
                for a in args { self.collect_send_sites(a, self_class, out); }
            }
            ExprNode::Raise { value } => self.collect_send_sites(value, self_class, out),
            ExprNode::Return { value } => self.collect_send_sites(value, self_class, out),
            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args { self.collect_send_sites(a, self_class, out); }
                }
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.collect_send_sites(body, self_class, out);
                for rc in rescues {
                    for c in &rc.classes { self.collect_send_sites(c, self_class, out); }
                    self.collect_send_sites(&rc.body, self_class, out);
                }
                if let Some(e) = else_branch { self.collect_send_sites(e, self_class, out); }
                if let Some(e) = ensure { self.collect_send_sites(e, self_class, out); }
            }
            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { self.collect_send_sites(v, self_class, out); }
            }
            ExprNode::Splat { value } => self.collect_send_sites(value, self_class, out),
            ExprNode::MultiAssign { value, .. } => {
                self.collect_send_sites(value, self_class, out)
            }
            ExprNode::While { cond, body, .. } => {
                self.collect_send_sites(cond, self_class, out);
                self.collect_send_sites(body, self_class, out);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { self.collect_send_sites(b, self_class, out); }
                if let Some(e) = end { self.collect_send_sites(e, self_class, out); }
            }
            ExprNode::Cast { value, .. } => self.collect_send_sites(value, self_class, out),
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::Retry
            | ExprNode::Redo
            | ExprNode::SelfRef => {}
        }
    }

    /// Walk a typed expression tree computing each node's *local* effects
    /// (those the node itself contributes — typically only non-empty for
    /// `Send` onto an effectful method) and writing them to `expr.effects`.
    /// The running aggregate `out` collects effects across the subtree so
    /// the caller can still populate per-action / per-method totals.
    ///
    /// Two-pass analyze (before_action seeding) calls this a second time
    /// with a richer ctx; every per-node `expr.effects` write here
    /// overwrites the earlier value, so annotations stay consistent with
    /// the final typed tree.
    fn visit_effects(&self, expr: &mut Expr, ctx: &Ctx, out: &mut BTreeSet<Effect>) {
        let mut local: BTreeSet<Effect> = BTreeSet::new();

        match &mut *expr.node {
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::Retry
            | ExprNode::Redo
            | ExprNode::SelfRef => {}

            ExprNode::Return { value } => self.visit_effects(value, ctx, out),

            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args {
                        self.visit_effects(a, ctx, out);
                    }
                }
            }

            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.visit_effects(body, ctx, out);
                for rc in rescues {
                    for c in &mut rc.classes {
                        self.visit_effects(c, ctx, out);
                    }
                    self.visit_effects(&mut rc.body, ctx, out);
                }
                if let Some(e) = else_branch {
                    self.visit_effects(e, ctx, out);
                }
                if let Some(e) = ensure {
                    self.visit_effects(e, ctx, out);
                }
            }

            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    self.visit_effects(k, ctx, out);
                    self.visit_effects(v, ctx, out);
                }
            }

            ExprNode::Array { elements, .. } => {
                for e in elements {
                    self.visit_effects(e, ctx, out);
                }
            }

            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.visit_effects(expr, ctx, out);
                    }
                }
            }

            ExprNode::BoolOp { left, right, .. } => {
                self.visit_effects(left, ctx, out);
                self.visit_effects(right, ctx, out);
            }

            ExprNode::RescueModifier { expr, fallback } => {
                self.visit_effects(expr, ctx, out);
                self.visit_effects(fallback, ctx, out);
            }

            ExprNode::Let { value, body, .. } => {
                self.visit_effects(value, ctx, out);
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Lambda { body, .. } => {
                // Lambda creation is pure; only invocation has effects. A
                // proper treatment requires first-class Fn types. Skip for now.
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Apply { fun, args, block } => {
                self.visit_effects(fun, ctx, out);
                for a in args { self.visit_effects(a, ctx, out); }
                if let Some(b) = block { self.visit_effects(b, ctx, out); }
            }
            ExprNode::Send { recv, method, args, block, .. } => {
                let recv_ty = match recv {
                    Some(r) => {
                        self.visit_effects(r, ctx, out);
                        r.ty.clone()
                    }
                    None => ctx.self_ty.clone(),
                };
                // Local effects for THIS Send — the dispatched method's
                // declared side-effect class, determined from the receiver
                // type + method name. Sub-expressions (receiver, args,
                // block) contribute their own local effects via their own
                // annotations; not folded into this node's `local`.
                if let Some(ty) = recv_ty {
                    self.contribute_send_effect(&ty, method, &mut local);
                }
                for a in args { self.visit_effects(a, ctx, out); }
                if let Some(b) = block { self.visit_effects(b, ctx, out); }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.visit_effects(cond, ctx, out);
                self.visit_effects(then_branch, ctx, out);
                self.visit_effects(else_branch, ctx, out);
            }
            ExprNode::Case { scrutinee, arms } => {
                self.visit_effects(scrutinee, ctx, out);
                for arm in arms {
                    if let Some(g) = &mut arm.guard { self.visit_effects(g, ctx, out); }
                    self.visit_effects(&mut arm.body, ctx, out);
                }
            }
            ExprNode::Seq { exprs } => {
                for e in exprs { self.visit_effects(e, ctx, out); }
            }
            ExprNode::Assign { target, value }
            | ExprNode::OpAssign { target, value, .. } => {
                self.visit_effects(value, ctx, out);
                if let LValue::Attr { recv, .. } = target {
                    self.visit_effects(recv, ctx, out);
                }
                if let LValue::Index { recv, index } = target {
                    self.visit_effects(recv, ctx, out);
                    self.visit_effects(index, ctx, out);
                }
            }
            ExprNode::Yield { args } => {
                for a in args { self.visit_effects(a, ctx, out); }
            }
            ExprNode::Raise { value } => {
                self.visit_effects(value, ctx, out);
                // Could record a Raises effect here once we track exception
                // class hierarchies. Skip for now.
            }
            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { self.visit_effects(v, ctx, out); }
            }
            ExprNode::Splat { value } => self.visit_effects(value, ctx, out),
            ExprNode::MultiAssign { targets, value } => {
                self.visit_effects(value, ctx, out);
                for target in targets.iter_mut() {
                    if let LValue::Attr { recv, .. } = target {
                        self.visit_effects(recv, ctx, out);
                    }
                    if let LValue::Index { recv, index } = target {
                        self.visit_effects(recv, ctx, out);
                        self.visit_effects(index, ctx, out);
                    }
                }
            }
            ExprNode::While { cond, body, .. } => {
                self.visit_effects(cond, ctx, out);
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { self.visit_effects(b, ctx, out); }
                if let Some(e) = end { self.visit_effects(e, ctx, out); }
            }
            ExprNode::Cast { value, .. } => self.visit_effects(value, ctx, out),
        }

        // Persist local effects onto this node and feed the running
        // aggregate. Overwrite rather than merge: the caller may re-invoke
        // (two-pass before_action seeding), and each pass computes local
        // effects from scratch against the current typed tree.
        out.extend(local.iter().cloned());
        expr.effects = EffectSet { effects: local };
    }

    fn contribute_send_effect(&self, recv_ty: &Ty, method: &Symbol, out: &mut BTreeSet<Effect>) {
        let Ty::Class { id, .. } = recv_ty else { return };
        let Some(cls) = self.classes.get(id) else { return };

        // AR methods on model classes: DbRead / DbWrite against the
        // bound table. The adapter owns the classification — swapping
        // adapters changes which methods produce effects (e.g., an
        // IndexedDB adapter can return Unknown for methods it can't
        // implement, making them silent at the effect level and
        // diagnostic-bearing downstream).
        //
        // Terminal-vs-builder gating: Relation-builder methods
        // (`where`, `limit`, `order`, `includes`, `joins`, `group`,
        // `having`, `preload`, `distinct`) return a lazy Relation
        // that hasn't executed SQL. Under an async backend, awaiting
        // each builder link would emit one round-trip per chain
        // step instead of the single round-trip the terminal call
        // actually triggers. Skipping the effect attachment here
        // means those builder Sends carry no effect in the IR — the
        // await machinery walks past them to the terminal step that
        // does. ChainKind::Terminal / NotApplicable / missing entry
        // all keep the effect; only explicit Builder skips.
        if let Some(table) = &cls.table {
            let kind = self.adapter.classify_ar_method(method.as_str());
            let is_builder_read =
                matches!(kind, ArMethodKind::Read) && self.is_builder_chain(method.as_str());
            if !is_builder_read {
                match kind {
                    ArMethodKind::Read => {
                        out.insert(Effect::DbRead { table: table.clone() });
                    }
                    ArMethodKind::Write => {
                        out.insert(Effect::DbWrite { table: table.clone() });
                    }
                    ArMethodKind::Unknown => {}
                }
            }
        }

        // Controller-side IO effects — Rails dialect, not adapter
        // territory. Every backend renders views and redirects the
        // same way at the effect level; the concrete implementation
        // lives in each target's runtime, not here. The receiver is the
        // controller's own class now (self_ty), so match any controller
        // by the Rails `*Controller` convention — `ApplicationController`,
        // `StoriesController`, etc. — not just the literal base. (View
        // renders dispatch with no receiver and never reach here.)
        if id.0.as_str().ends_with("Controller") {
            match method.as_str() {
                "render" | "redirect_to" | "head" => {
                    out.insert(Effect::Io);
                }
                _ => {}
            }
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
        crate::catalog::lookup_any(method).any(|entry| {
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
/// resolves a view); the second return value carries its harvested ivar
/// bindings — the bodies were already typed by the Phase 0
/// `Unknown`-item pass — for registration alongside real targets.
/// `only:`/`except:` scoping on a block filter is not modelled — the
/// form is rare and a missing guard only over-seeds an unread ivar.
fn build_sourced_filter_chain(
    controller: &Controller,
    concern_filters: &HashMap<ClassId, Vec<Filter>>,
    module_includes: &HashMap<ClassId, Vec<ClassId>>,
) -> (Vec<(Filter, ClassId)>, Vec<(Symbol, HashMap<Symbol, Ty>)>) {
    fn splice_concern(
        module_id: &ClassId,
        concern_filters: &HashMap<ClassId, Vec<Filter>>,
        module_includes: &HashMap<ClassId, Vec<ClassId>>,
        spliced: &mut BTreeSet<ClassId>,
        chain: &mut Vec<(Filter, ClassId)>,
    ) {
        if !spliced.insert(module_id.clone()) {
            return;
        }
        if let Some(nested) = module_includes.get(module_id) {
            for n in nested {
                splice_concern(n, concern_filters, module_includes, spliced, chain);
            }
        }
        if let Some(fs) = concern_filters.get(module_id) {
            chain.extend(fs.iter().map(|f| (f.clone(), module_id.clone())));
        }
    }

    let own_id = controller.name.clone();
    let mut chain: Vec<(Filter, ClassId)> = Vec::new();
    let mut block_bindings: Vec<(Symbol, HashMap<Symbol, Ty>)> = Vec::new();
    let mut spliced: BTreeSet<ClassId> = BTreeSet::new();

    for (idx, item) in controller.body.iter().enumerate() {
        match item {
            ControllerBodyItem::Filter { filter, .. } => {
                chain.push((filter.clone(), own_id.clone()));
            }
            ControllerBodyItem::Unknown { expr, .. } => {
                let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node
                else {
                    continue;
                };
                match method.as_str() {
                    "include" => {
                        for arg in args {
                            if let ExprNode::Const { path } = &*arg.node {
                                let joined = path
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::");
                                splice_concern(
                                    &ClassId(Symbol::from(joined)),
                                    concern_filters,
                                    module_includes,
                                    &mut spliced,
                                    &mut chain,
                                );
                            }
                        }
                    }
                    "before_action" | "around_action" => {
                        let Some(block) = block else { continue };
                        let kind = if method.as_str() == "before_action" {
                            FilterKind::Before
                        } else {
                            FilterKind::Around
                        };
                        // The attached block is a Lambda whose body is
                        // the filter code.
                        let body = match &*block.node {
                            ExprNode::Lambda { body, .. } => body,
                            _ => block,
                        };
                        let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                        extract_ivar_assignments(body, &mut ivars);
                        if ivars.is_empty() {
                            continue;
                        }
                        let target =
                            Symbol::from(format!("__{}_block_{idx}__", method.as_str()));
                        chain.push((
                            Filter {
                                kind,
                                target: target.clone(),
                                only: Vec::new(),
                                except: Vec::new(),
                                only_style: crate::expr::ArrayStyle::default(),
                                except_style: crate::expr::ArrayStyle::default(),
                                if_cond: None,
                                unless_cond: None,
                            },
                            own_id.clone(),
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

/// Unify a stored param type with a freshly observed argument type.
/// Mirrors Spinel's `detect_poly_in_node` (`spinel_codegen.rb:6961-7000`)
/// joinrules at a higher level — we operate on `Ty` directly, so the
/// rules are:
/// - same type → keep
/// - one side is `Ty::Var` (no info yet) → take the other
/// - one side is `Nil` and the other is concrete → nullable union (T?)
/// - already a Union containing `observed` → keep
/// - otherwise → widen via `union_of`
fn unify_param_ty(stored: Ty, observed: Ty) -> Ty {
    if stored == observed {
        return stored;
    }
    if matches!(stored, Ty::Var { .. }) {
        return observed;
    }
    if matches!(observed, Ty::Var { .. }) {
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

/// A view name identifies a partial when any path segment starts with `_`
/// (Rails convention: `app/views/articles/_article.html.erb` → view name
/// `articles/_article`).
fn is_partial_view_name(name: &Symbol) -> bool {
    name.as_str().split('/').any(|seg| seg.starts_with('_'))
}

/// Walk a view body collecting `render ...` call sites. For each recognized
/// shape, determine the target partial's view name and the locals the render
/// passes into it, merging into `out`.
///
/// Shapes recognized (matching real-blog + the common idioms):
/// - `render @collection` where `@collection` types as `Array<Class>` →
///   partial `pluralize(snake(Class))/_snake(Class)`, local `snake(Class)`.
/// - `render some_single_record` typing as `Class` → same partial path, local
///   bound to the record's type.
/// - `render "name", k1: v1, k2: v2` → partial name resolved relative to the
///   current view's directory (`articles/index` + `"form"` → `articles/_form`),
///   locals from the trailing kwarg hash.
/// - `render partial: "name", locals: { k: v }` → same resolution, locals
///   sourced from the `locals:` hash.
///
/// Call-site argument shapes outside these cases are skipped silently;
/// an unrecognized render just leaves the target partial seeded by other
/// sites (or unseeded).
fn extract_partial_render_sites(
    expr: &Expr,
    current_view: &Symbol,
    out: &mut HashMap<Symbol, HashMap<Symbol, Ty>>,
    targets: &mut Vec<Symbol>,
) {
    match &*expr.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            // Detect the `render` call shape (no explicit receiver, or the
            // receiver is an implicit context — Rails makes both work).
            if recv.is_none() && method.as_str() == "render" {
                if let Some((partial_name, locals)) = interpret_render_call(args, current_view) {
                    // Record the renderer→partial edge so the caller can
                    // propagate the renderer's ivar context to the partial
                    // (partials render in their parent's view context and
                    // read its `@ivars`).
                    targets.push(partial_name.clone());
                    let entry = out.entry(partial_name).or_default();
                    for (k, v) in locals {
                        entry.insert(k, v);
                    }
                }
            }
            if let Some(r) = recv {
                extract_partial_render_sites(r, current_view, out, targets);
            }
            for a in args {
                extract_partial_render_sites(a, current_view, out, targets);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out, targets);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                extract_partial_render_sites(e, current_view, out, targets);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                extract_partial_render_sites(k, current_view, out, targets);
                extract_partial_render_sites(v, current_view, out, targets);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_partial_render_sites(cond, current_view, out, targets);
            extract_partial_render_sites(then_branch, current_view, out, targets);
            extract_partial_render_sites(else_branch, current_view, out, targets);
        }
        ExprNode::Case { scrutinee, arms } => {
            extract_partial_render_sites(scrutinee, current_view, out, targets);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    extract_partial_render_sites(g, current_view, out, targets);
                }
                extract_partial_render_sites(&arm.body, current_view, out, targets);
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            extract_partial_render_sites(left, current_view, out, targets);
            extract_partial_render_sites(right, current_view, out, targets);
        }
        ExprNode::Let { value, body, .. } => {
            extract_partial_render_sites(value, current_view, out, targets);
            extract_partial_render_sites(body, current_view, out, targets);
        }
        ExprNode::Lambda { body, .. } => {
            extract_partial_render_sites(body, current_view, out, targets);
        }
        ExprNode::Apply { fun, args, block } => {
            extract_partial_render_sites(fun, current_view, out, targets);
            for a in args {
                extract_partial_render_sites(a, current_view, out, targets);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out, targets);
            }
        }
        ExprNode::Assign { value, .. } => {
            extract_partial_render_sites(value, current_view, out, targets);
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    extract_partial_render_sites(expr, current_view, out, targets);
                }
            }
        }
        _ => {}
    }
}

/// Collect the ivar names that views use as *dynamic* partial-render
/// targets — `render @above` or `render partial: @above`. These name
/// a content partial whose identity is only known at runtime (the
/// ivar holds a string literal like `'for_domain'` assigned by the
/// action). Pairing this set with the per-action string-literal
/// assignments (`@above = 'for_domain'`) lets the analyzer seed the
/// `_for_domain` partial with that action's ivars — the edge
/// `extract_partial_render_sites` can't resolve statically.
fn collect_dynamic_render_ivars(expr: &Expr, out: &mut std::collections::HashSet<Symbol>) {
    if let ExprNode::Send { recv, method, args, .. } = &*expr.node {
        if recv.is_none() && method.as_str() == "render" {
            for arg in args {
                match &*arg.node {
                    // `render @above`
                    ExprNode::Ivar { name } => {
                        out.insert(name.clone());
                    }
                    // `render partial: @above` (the kwarg-hash form)
                    ExprNode::Hash { entries, .. } => {
                        for (k, v) in entries {
                            let is_partial_key = matches!(
                                &*k.node,
                                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "partial"
                            );
                            if is_partial_key {
                                if let ExprNode::Ivar { name } = &*v.node {
                                    out.insert(name.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_dynamic_render_ivars(c, out));
}

/// Collect `@ivar = "string literal"` assignments whose ivar name is
/// in `targets` (the dynamic-render ivar set). Returns each literal
/// value — the content-partial basename the action wants rendered.
fn collect_content_partial_literals(
    expr: &Expr,
    targets: &std::collections::HashSet<Symbol>,
    out: &mut Vec<String>,
) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*expr.node {
        if targets.contains(name) {
            if let ExprNode::Lit { value: Literal::Str { value } } = &*value.node {
                out.push(value.clone());
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_content_partial_literals(c, targets, out));
}

/// Resolve a content-partial literal (`"for_domain"`, `"saved/subnav"`)
/// to a partial view name. A value with a `/` carries its own
/// directory (`saved/subnav` → `saved/_subnav`); a bare value is
/// relative to the rendering controller's view prefix (`for_domain` in
/// HomeController → `home/_for_domain`).
fn content_partial_view_name(literal: &str, prefix: &str) -> Symbol {
    match literal.rfind('/') {
        Some(idx) => {
            let (dir, base) = literal.split_at(idx);
            Symbol::from(format!("{}/_{}", dir, &base[1..]))
        }
        None => Symbol::from(format!("{}/_{}", prefix, literal)),
    }
}

/// Collect every full-template view an action renders via an explicit
/// `render :action => "x"` / `render :template => "x"` /
/// `render_to_string :action => "x"` call anywhere in its body —
/// including calls buried in a `respond_to`/`Rails.cache.fetch` block
/// that never surface as the action's primary `RenderTarget`. The
/// `tree` action's `render_to_string :action => "tree"` inside a cache
/// block is the motivating case: without this the `tree` view gets no
/// ivar seed at all. `:action` names resolve relative to the
/// controller's view prefix; `:template` names are taken verbatim
/// (they already carry their directory).
fn collect_action_render_views(expr: &Expr, prefix: &str, out: &mut Vec<Symbol>) {
    if let ExprNode::Send { recv, method, args, .. } = &*expr.node {
        if recv.is_none() && matches!(method.as_str(), "render" | "render_to_string") {
            for arg in args {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    for (k, v) in entries {
                        let key = match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                            _ => continue,
                        };
                        let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else {
                            continue;
                        };
                        match key {
                            "action" => {
                                let name = if value.contains('/') {
                                    value.clone()
                                } else {
                                    format!("{}/{}", prefix, value)
                                };
                                out.push(Symbol::from(name));
                            }
                            "template" => out.push(Symbol::from(value.clone())),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_action_render_views(c, prefix, out));
}

/// Figure out the target partial name and the locals a `render(...)` call
/// passes to it. Returns `None` for shapes not yet handled.
fn interpret_render_call(
    args: &[Expr],
    current_view: &Symbol,
) -> Option<(Symbol, HashMap<Symbol, Ty>)> {
    if args.is_empty() {
        return None;
    }
    let first = &args[0];

    // Collection / single-record render: `render @articles`, `render @article.comments`,
    // `render @article` — first arg types as Array<Class> or Class.
    if let Some(ty) = first.ty.as_ref() {
        if let Some((partial, local_name, elem_ty)) = partial_from_receiver_type(ty) {
            let mut locals = HashMap::new();
            locals.insert(Symbol::from(local_name.as_str()), elem_ty);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    // Named partial: `render "name", k: v, k: v` or `render "name"`.
    if let ExprNode::Lit { value: Literal::Str { value: name } } = &*first.node {
        let partial = resolve_partial_path(name, current_view);
        let mut locals = HashMap::new();
        for a in &args[1..] {
            if let ExprNode::Hash { entries, .. } = &*a.node {
                for (k, v) in entries {
                    if let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node {
                        if let Some(ty) = v.ty.clone() {
                            locals.insert(key.clone(), ty);
                        }
                    }
                }
            }
        }
        return Some((Symbol::from(partial.as_str()), locals));
    }

    // Hash form: `render partial: "name", locals: { k: v }` — first arg is a Hash.
    // The collection form rides the same hash: `render partial: "status",
    // collection: @statuses[, as: :status]` binds an implicit local named
    // after the partial's basename (or the `as:` override), typed as the
    // collection's element, plus Rails' `<name>_counter` index local.
    if let ExprNode::Hash { entries, .. } = &*first.node {
        let mut partial_name: Option<String> = None;
        let mut locals: HashMap<Symbol, Ty> = HashMap::new();
        let mut collection_ty: Option<Ty> = None;
        let mut as_name: Option<Symbol> = None;
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            match key.as_str() {
                "partial" => {
                    if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                        partial_name = Some(value.clone());
                    }
                }
                "locals" => {
                    if let ExprNode::Hash { entries: loc_entries, .. } = &*v.node {
                        for (lk, lv) in loc_entries {
                            if let ExprNode::Lit { value: Literal::Sym { value: loc_key } } =
                                &*lk.node
                            {
                                if let Some(ty) = lv.ty.clone() {
                                    locals.insert(loc_key.clone(), ty);
                                }
                            }
                        }
                    }
                }
                "collection" => {
                    collection_ty = v.ty.clone();
                }
                "as" => {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                        as_name = Some(value.clone());
                    }
                }
                _ => {}
            }
        }
        if let Some(name) = partial_name {
            if let Some(coll) = collection_ty {
                let elem_ty = match coll {
                    Ty::Array { elem } => *elem,
                    // Unknown/gradual collection still binds the local —
                    // gradual element beats an unresolved bare name.
                    _ => Ty::Untyped,
                };
                let local = as_name.unwrap_or_else(|| {
                    let base = name.rsplit('/').next().unwrap_or(&name);
                    Symbol::from(base.trim_start_matches('_'))
                });
                locals
                    .entry(Symbol::from(format!("{}_counter", local.as_str()).as_str()))
                    .or_insert(Ty::Int);
                locals.entry(local).or_insert(elem_ty);
            }
            let partial = resolve_partial_path(&name, current_view);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    None
}

/// If the receiver type implies a collection/single-record render target,
/// return (partial_view_name, local_name, element_ty). For `Array<Article>`:
/// partial `articles/_article`, local `article`, element `Article`.
fn partial_from_receiver_type(ty: &Ty) -> Option<(String, String, Ty)> {
    match ty {
        Ty::Array { elem } => match &**elem {
            Ty::Class { id, .. } => {
                let class_name = id.0.as_str();
                let local = crate::naming::snake_case(class_name);
                let folder = crate::naming::pluralize_snake(class_name);
                Some((format!("{folder}/_{local}"), local, (**elem).clone()))
            }
            _ => None,
        },
        Ty::Class { id, .. } => {
            let class_name = id.0.as_str();
            let local = crate::naming::snake_case(class_name);
            let folder = crate::naming::pluralize_snake(class_name);
            Some((format!("{folder}/_{local}"), local, ty.clone()))
        }
        _ => None,
    }
}

/// Resolve a partial name relative to the current view's directory.
/// `"form"` in `articles/index` → `articles/_form`; `"shared/nav"` (absolute,
/// contains `/`) → `shared/_nav`.
fn resolve_partial_path(name: &str, current_view: &Symbol) -> String {
    if let Some(idx) = name.rfind('/') {
        let (dir, file) = name.split_at(idx + 1);
        format!("{dir}_{file}")
    } else {
        let current = current_view.as_str();
        match current.rfind('/') {
            Some(idx) => format!("{}_{}", &current[..=idx], name),
            None => format!("_{name}"),
        }
    }
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
fn register_stdlib_class(
    classes: &mut HashMap<ClassId, ClassInfo>,
    name: &str,
    class_methods: &[(&str, Ty)],
    instance_methods: &[(&str, Ty)],
) {
    let cls = classes.entry(ClassId(Symbol::from(name))).or_default();
    for (m, ty) in class_methods {
        cls.class_methods
            .entry(Symbol::from(*m))
            .or_insert_with(|| ty.clone());
    }
    for (m, ty) in instance_methods {
        cls.instance_methods
            .entry(Symbol::from(*m))
            .or_insert_with(|| ty.clone());
    }
}

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
                let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                out.push(ClassId(Symbol::from(joined)));
            }
        }
    }
    out
}

pub(crate) fn controller_includes(controller: &Controller) -> Vec<ClassId> {
    let mut out = Vec::new();
    for item in &controller.body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "include" {
            continue;
        }
        for arg in args {
            if let ExprNode::Const { path } = &*arg.node {
                let joined =
                    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                out.push(ClassId(Symbol::from(joined)));
            }
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
fn strip_nil(ty: Ty) -> Ty {
    let Ty::Union { variants } = ty else { return ty };
    let kept: Vec<Ty> = variants
        .into_iter()
        .filter(|v| !matches!(v, Ty::Nil))
        .collect();
    match kept.len() {
        0 => Ty::Nil,
        1 => kept.into_iter().next().unwrap(),
        _ => Ty::Union { variants: kept },
    }
}

/// A method's return type is the union of every `return X` value type
/// reachable in its body PLUS the tail (implicit-return) expression's
/// type. The body-typer types a `return` *expression* as `Bottom` — it
/// diverges at that source position — so a method whose tail diverges
/// (every path `return`s, or the tail is a `case`/`begin` whose arms
/// all return) reports `body.ty == Bottom` even though the early
/// `return`s carry the real type. Reading only `body.ty` then harvests
/// `Bottom`, and a caller's `result[:k]` fails dispatch on `Bottom`.
/// Collect the returns and union them with the non-`Bottom` tail.
fn effective_return_ty(body: &Expr) -> Option<Ty> {
    let mut tys: Vec<Ty> = Vec::new();
    collect_return_types(body, &mut tys);
    // Tail (implicit return). Drop `Bottom` so an all-diverging tail
    // doesn't poison the union; keep everything else.
    if let Some(t) = &body.ty {
        if !matches!(t, Ty::Bottom) {
            tys.push(t.clone());
        }
    }
    if tys.is_empty() {
        // Nothing usable collected — preserve prior behavior so the
        // `Var`/`Bottom`/`None` fallbacks downstream are unchanged.
        return body.ty.clone();
    }
    Some(crate::analyze::body::union_many(tys))
}

/// Collect the value type of every `return X` reachable from `expr`
/// without crossing a closure boundary. `Bottom`/`Var` values are
/// skipped (no usable shape). Does not descend into `Lambda`: a stabby
/// `-> { return }` returns from the lambda, not the method (block
/// `do…end` returns do exit the method, but the two share the same IR
/// node, so skipping is the safe under-approximation).
fn collect_return_types(expr: &Expr, out: &mut Vec<Ty>) {
    match &*expr.node {
        ExprNode::Return { value } => {
            if let Some(t) = &value.ty {
                if !matches!(t, Ty::Bottom | Ty::Var { .. }) {
                    out.push(t.clone());
                }
            }
            collect_return_types(value, out);
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_return_types(e, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_return_types(cond, out);
            collect_return_types(then_branch, out);
            collect_return_types(else_branch, out);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms {
                collect_return_types(&arm.body, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_return_types(body, out);
            for r in rescues {
                collect_return_types(&r.body, out);
            }
            if let Some(e) = else_branch {
                collect_return_types(e, out);
            }
            if let Some(e) = ensure {
                collect_return_types(e, out);
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_return_types(left, out);
            collect_return_types(right, out);
        }
        ExprNode::While { body, .. } => collect_return_types(body, out),
        _ => {}
    }
}

pub(crate) fn extract_ivar_assignments(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
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
                out.insert(name.clone(), merged);
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
                out.insert(name.clone(), merged);
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
                        out.insert(name.clone(), merged);
                    }
                }
            }
            extract_ivar_assignments(value, out);
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
            extract_ivar_assignments(recv, out);
            extract_ivar_assignments(index, out);
            extract_ivar_assignments(value, out);
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
            extract_ivar_assignments(recv, out);
            for a in args {
                extract_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                extract_ivar_assignments(b, out);
            }
        }
        // Walk into other Send forms so nested `[]=` writes (e.g.
        // inside a method-chain receiver or arg expression) still
        // get found. Cheap; the special-case above already handles
        // the widening — this is purely recursive descent.
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                extract_ivar_assignments(r, out);
            }
            for a in args {
                extract_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                extract_ivar_assignments(b, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                extract_ivar_assignments(e, out);
            }
        }
        // The condition is walked too: `if (@message = Model.find(..))`
        // assigns the ivar inside the test, a common `find_*` filter
        // idiom. Without visiting `cond`, that ivar never gets typed.
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_ivar_assignments(cond, out);
            extract_ivar_assignments(then_branch, out);
            extract_ivar_assignments(else_branch, out);
        }
        // `while cond; body; end` — body may contain `@hash[k] = v`
        // (Parameters' initialize loop). Without this arm, ivar
        // value-type widening from `[]=` writes inside loops is
        // invisible. The condition is walked for the same
        // assignment-in-test reason as `If`.
        ExprNode::While { cond, body, .. } => {
            extract_ivar_assignments(cond, out);
            extract_ivar_assignments(body, out);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            extract_ivar_assignments(expr, out);
            extract_ivar_assignments(fallback, out);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms {
                extract_ivar_assignments(&arm.body, out);
            }
        }
        // `a && (@x = y)` / `a || (@x = y)` — an ivar assigned inside a
        // boolean chain (the `find_*` guard idiom). Descend both sides
        // so the buried assignment still gets typed. (Compound `@x ||= y`
        // is `OpAssign`, handled by its own arm above — not `BoolOp`.)
        ExprNode::BoolOp { left, right, .. } => {
            extract_ivar_assignments(left, out);
            extract_ivar_assignments(right, out);
        }
        // Rescue/ensure and lifecycle constructs may also contain
        // assignments; recurse to catch them.
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            extract_ivar_assignments(body, out);
            for r in rescues {
                extract_ivar_assignments(&r.body, out);
            }
            if let Some(e) = else_branch {
                extract_ivar_assignments(e, out);
            }
            if let Some(e) = ensure {
                extract_ivar_assignments(e, out);
            }
        }
        ExprNode::Lambda { body, .. } => extract_ivar_assignments(body, out),
        ExprNode::Return { value } => extract_ivar_assignments(value, out),
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
            Ty::Hash { key: Box::new(Ty::Str), value: Box::new(incoming.clone()) },
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
    } else if **value == *incoming {
        value.clone()
    } else {
        let mut variants: Vec<Ty> = match value.as_ref() {
            Ty::Union { variants } => variants.clone(),
            other => vec![other.clone()],
        };
        let incoming_variants: Vec<Ty> = match incoming {
            Ty::Union { variants } => variants.clone(),
            other => vec![other.clone()],
        };
        for v in incoming_variants {
            if !variants.contains(&v) {
                variants.push(v);
            }
        }
        if variants.len() == 1 {
            Box::new(variants.into_iter().next().unwrap())
        } else {
            Box::new(Ty::Union { variants })
        }
    };
    out.insert(name.clone(), Ty::Hash { key, value });
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
fn typed_store_ty(type_method: &str) -> Option<Ty> {
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
fn typed_store_is_array(args: &[Expr]) -> bool {
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

/// Walk an analyzed `App` collecting every position where typing failed
/// in a way that matters for downstream typed emission. Does not modify
/// the IR — purely a read pass.
///
/// Scope of what's reported:
/// - Ivar reads whose `ty` remained `Ty::Var(0)`.
/// - Send calls with a concrete receiver type whose method wasn't found.
///
/// Deliberately NOT reported (noise suppression):
/// - Bare-name Sends whose receiver is implicit-self / None. Views without
///   a self_ty call many helpers we don't model (e.g. `csrf_meta_tags`);
///   flagging each would drown real diagnostics. Once helpers land via
///   the dialect registry expansion, this filter can be relaxed.
/// - Sends whose receiver itself is unknown. The root cause is upstream;
///   reporting both duplicates signal.
pub fn diagnose(app: &App) -> Vec<Diagnostic> {
    diagnose_with_coverage(app).0
}

/// [`diagnose`] plus the missing-preload coverage triple, for report
/// skins that state the denominator (#64: "0 findings" must be
/// distinguishable from "couldn't check").
pub fn diagnose_with_coverage(app: &App) -> (Vec<Diagnostic>, PreloadCoverage) {
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            diagnose_expr(&action.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            diagnose_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            diagnose_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        diagnose_expr(&view.body, &mut out);
    }
    if let Some(seeds) = &app.seeds {
        diagnose_expr(seeds, &mut out);
    }

    // Static N+1 pass (#64): missing-preload warnings over the typed
    // query chains, same-procedure and through the controller→view
    // ivar channel.
    let (preload_diags, coverage) = preload::missing_preload_report(app);
    out.extend(preload_diags);

    // Collapse diagnostics that render to the same place with the same
    // text — same start position, same kind, same message. Method chains
    // whose links share a (not-yet-precise) start each emit there, so
    // `a.b`, `a.b.c`, `a.b.c.d` stack 2-5 squiggles of differing length
    // but identical tooltip on one spot. Key on `start` (what line:col
    // and the squiggle's anchor derive from), not the full range, so the
    // nested links collapse. `retain` keeps the first — and since the
    // walker emits the outer node before recursing, that's the longest,
    // outermost span. Self-correcting: once span preservation gives links
    // distinct starts, they survive on their own again.
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert((d.span.file, d.span.start, d.code(), d.message.clone())));
    (out, coverage)
}

/// A type is "unknown" if it's `None` or `Ty::Var(n)` (a placeholder the
/// analyzer set for positions it couldn't resolve). `Ty::Untyped` —
/// the gradual escape — counts as *known*: the author signed that
/// position out of checking.
fn is_unknown_ty(ty: Option<&Ty>) -> bool {
    match ty {
        None => true,
        Some(Ty::Var { .. }) => true,
        _ => false,
    }
}

/// Short label for what shape of expression resolved to `Untyped`.
/// Used for the `GradualUntyped` diagnostic message so a single
/// kind can name the syntactic position without each callsite
/// recomputing. Lowercase, grep-friendly.
fn expr_kind_label(expr: &Expr) -> &'static str {
    match &*expr.node {
        ExprNode::Send { .. } => "method call",
        ExprNode::Ivar { .. } => "ivar read",
        ExprNode::Var { .. } => "local read",
        ExprNode::Const { .. } => "constant read",
        ExprNode::Apply { .. } => "function call",
        ExprNode::Yield { .. } => "yield",
        _ => "expression",
    }
}

/// The identifier at an unresolved leaf position, for the
/// `UnresolvedType` message — the called method, read local, or
/// constant path. `None` for nameless positions (`yield`). An `Apply`
/// names its callee when that callee is itself a named leaf.
fn unresolved_name(expr: &Expr) -> Option<crate::ident::Symbol> {
    match &*expr.node {
        ExprNode::Send { method, .. } => Some(method.clone()),
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Const { path } => Some(crate::ident::Symbol::new(
            &path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        )),
        ExprNode::Apply { fun, .. } => unresolved_name(fun),
        _ => None,
    }
}

fn diagnose_expr(expr: &Expr, out: &mut Vec<Diagnostic>) {
    // Diagnostic annotations set by the body-typer during analyze.
    // These are the IR-carried path: detection happens once at the
    // point of typing, and every reader (including this walker) sees
    // the same set.
    if let Some(kind) = &expr.diagnostic {
        let message = match kind {
            DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty } => {
                format!(
                    "`{}` with incompatible operand types: {lhs_ty:?} {} {rhs_ty:?}",
                    op.as_str(),
                    op.as_str()
                )
            }
            DiagnosticKind::IvarUnresolved { name } => {
                format!("@{} has no known type", name.as_str())
            }
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                format!("no known method `{}` on {recv_ty:?}", method.as_str())
            }
            DiagnosticKind::GradualUntyped { expr_kind } => {
                format!("{} resolves to RBS `untyped` (gradual escape)", expr_kind.as_str())
            }
            DiagnosticKind::UnresolvedType { expr_kind, name } => {
                Diagnostic::unresolved_type_text(expr_kind, name.as_ref())
            }
            DiagnosticKind::Unsupported { target, construct, detail } => {
                let mut m = Diagnostic::unsupported_text(target.as_ref(), construct);
                if !detail.is_empty() {
                    m.push_str(": ");
                    m.push_str(detail);
                }
                m
            }
            // Parse diagnostics come from the ingest parse wrapper and
            // MissingPreload from the post-walk preload pass — neither
            // is carried as an `Expr.diagnostic` annotation; handled
            // defensively so the match stays exhaustive.
            DiagnosticKind::Parse { message } => format!("syntax error: {message}"),
            DiagnosticKind::MissingPreload { association, .. } => {
                format!("query does not preload :{}", association.as_str())
            }
        };
        out.push(Diagnostic {
            span: expr.span,
            kind: kind.clone(),
            severity: Diagnostic::default_severity(kind),
            message,
        });
    }

    // RBS-declared `untyped` reaches this site. Emit a GradualUntyped
    // warning so consumers can track gradual-escape coverage and so
    // strict-target emitters can elevate to Error at emit time. The
    // body-typer doesn't annotate `expr.diagnostic` for Untyped — the
    // walker is the natural place since every node's `.ty` already
    // carries the signal.
    if matches!(expr.ty.as_ref(), Some(Ty::Untyped)) {
        let kind = DiagnosticKind::GradualUntyped {
            expr_kind: crate::ident::Symbol::new(expr_kind_label(expr)),
        };
        out.push(Diagnostic {
            span: expr.span,
            severity: Diagnostic::default_severity(&kind),
            kind,
            message: format!(
                "{} resolves to RBS `untyped` (gradual escape)",
                expr_kind_label(expr)
            ),
        });
    }

    match &*expr.node {
        ExprNode::Ivar { name } => {
            if is_unknown_ty(expr.ty.as_ref()) {
                let kind = DiagnosticKind::IvarUnresolved { name: name.clone() };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!("@{} has no known type", name.as_str()),
                });
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            if !is_unknown_ty(r.ty.as_ref()) && is_unknown_ty(expr.ty.as_ref()) {
                let recv_ty = r.ty.clone().unwrap_or_else(|| Ty::Var { var: crate::ident::TyVar(0) });
                let kind = DiagnosticKind::SendDispatchFailed {
                    method: method.clone(),
                    recv_ty: recv_ty.clone(),
                };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!(
                        "no known method `{}` on {:?}",
                        method.as_str(),
                        recv_ty,
                    ),
                });
            }
        }
        _ => {}
    }

    // Residual unresolved positions the specific checks above don't
    // cover — the "silently unresolved" set. The body-typer left these
    // as an open inference variable (`Ty::Var`) or never stamped a type
    // (`None`), but no diagnostic fires today, so they pass invisibly:
    //   - implicit-self sends (`controller_name`, recv: None)
    //   - bare local and constant reads
    //   - function applies and yields
    // Ivars are reported by IvarUnresolved; explicit-receiver sends with
    // a *known* receiver by SendDispatchFailed. An explicit receiver that
    // is itself unresolved is reported on the receiver node when we
    // recurse, so the outer send is skipped here to avoid double-counting
    // the same root cause.
    if is_unknown_ty(expr.ty.as_ref()) {
        let report = matches!(
            &*expr.node,
            ExprNode::Send { recv: None, .. }
                | ExprNode::Var { .. }
                | ExprNode::Const { .. }
                | ExprNode::Apply { .. }
                | ExprNode::Yield { .. }
        );
        if report {
            let label = crate::ident::Symbol::new(expr_kind_label(expr));
            let name = unresolved_name(expr);
            let message = Diagnostic::unresolved_type_text(&label, name.as_ref());
            let kind = DiagnosticKind::UnresolvedType { expr_kind: label, name };
            out.push(Diagnostic {
                span: expr.span,
                severity: Diagnostic::default_severity(&kind),
                kind,
                message,
            });
        }
    }

    // Recurse into children so we surface every unresolved position.
    match &*expr.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                diagnose_expr(r, out);
            }
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                diagnose_expr(k, out);
                diagnose_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    diagnose_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            diagnose_expr(left, out);
            diagnose_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            diagnose_expr(cond, out);
            diagnose_expr(then_branch, out);
            diagnose_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            diagnose_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    diagnose_expr(g, out);
                }
                diagnose_expr(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            diagnose_expr(value, out);
            diagnose_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            diagnose_expr(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            diagnose_expr(fun, out);
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            diagnose_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                diagnose_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                diagnose_expr(recv, out);
                diagnose_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                diagnose_expr(a, out);
            }
        }
        ExprNode::Raise { value } => diagnose_expr(value, out),
        ExprNode::Return { value } => diagnose_expr(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    diagnose_expr(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            diagnose_expr(body, out);
            for rc in rescues {
                for c in &rc.classes {
                    diagnose_expr(c, out);
                }
                diagnose_expr(&rc.body, out);
            }
            if let Some(e) = else_branch {
                diagnose_expr(e, out);
            }
            if let Some(e) = ensure {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { diagnose_expr(v, out); }
        }
        ExprNode::Splat { value } => diagnose_expr(value, out),
        ExprNode::MultiAssign { targets, value } => {
            diagnose_expr(value, out);
            for target in targets {
                if let LValue::Attr { recv, .. } = target {
                    diagnose_expr(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    diagnose_expr(recv, out);
                    diagnose_expr(index, out);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            diagnose_expr(cond, out);
            diagnose_expr(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { diagnose_expr(b, out); }
            if let Some(e) = end { diagnose_expr(e, out); }
        }
        ExprNode::Cast { value, .. } => diagnose_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
    }
}

/// Collect every inferred type the body-typer stamped on the IR, paired with
/// its source span. Mirrors [`diagnose`]'s roots + [`diagnose_expr`]'s
/// (exhaustive) recursion so coverage matches the diagnostics walk. Spans may
/// be synthetic or point at non-source (lowered) nodes — callers filter. Used
/// by the browser playground to surface inferred-type hovers.
pub fn inferred_types(app: &App) -> Vec<(crate::span::Span, crate::ty::Ty)> {
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            collect_types_expr(&action.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            collect_types_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            collect_types_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        collect_types_expr(&view.body, &mut out);
    }
    if let Some(seeds) = &app.seeds {
        collect_types_expr(seeds, &mut out);
    }
    out
}

fn collect_types_expr(e: &Expr, out: &mut Vec<(crate::span::Span, crate::ty::Ty)>) {
    if let Some(ty) = &e.ty {
        out.push((e.span, ty.clone()));
    }
    // Recursion mirrors diagnose_expr exactly (exhaustive — a new ExprNode
    // variant breaks the build here too).
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_types_expr(r, out);
            }
            for a in args {
                collect_types_expr(a, out);
            }
            if let Some(b) = block {
                collect_types_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for x in exprs {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_types_expr(k, out);
                collect_types_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    collect_types_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            collect_types_expr(left, out);
            collect_types_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_types_expr(cond, out);
            collect_types_expr(then_branch, out);
            collect_types_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_types_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_types_expr(g, out);
                }
                collect_types_expr(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_types_expr(value, out);
            collect_types_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_types_expr(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_types_expr(fun, out);
            for a in args {
                collect_types_expr(a, out);
            }
            if let Some(b) = block {
                collect_types_expr(b, out);
            }
        }
        ExprNode::Assign { target, value } | ExprNode::OpAssign { target, value, .. } => {
            collect_types_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                collect_types_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                collect_types_expr(recv, out);
                collect_types_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_types_expr(a, out);
            }
        }
        ExprNode::Raise { value } => collect_types_expr(value, out),
        ExprNode::Return { value } => collect_types_expr(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_types_expr(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_types_expr(body, out);
            for rc in rescues {
                for c in &rc.classes {
                    collect_types_expr(c, out);
                }
                collect_types_expr(&rc.body, out);
            }
            if let Some(x) = else_branch {
                collect_types_expr(x, out);
            }
            if let Some(x) = ensure {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                collect_types_expr(v, out);
            }
        }
        ExprNode::Splat { value } => collect_types_expr(value, out),
        ExprNode::MultiAssign { targets, value } => {
            collect_types_expr(value, out);
            for target in targets {
                if let LValue::Attr { recv, .. } = target {
                    collect_types_expr(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    collect_types_expr(recv, out);
                    collect_types_expr(index, out);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            collect_types_expr(cond, out);
            collect_types_expr(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_types_expr(b, out);
            }
            if let Some(x) = end {
                collect_types_expr(x, out);
            }
        }
        ExprNode::Cast { value, .. } => collect_types_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
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
