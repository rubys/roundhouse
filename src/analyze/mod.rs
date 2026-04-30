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

pub use body::{BodyTyper, ClassInfo, Ctx};

use std::collections::{BTreeSet, HashMap};

use crate::adapter::{ArMethodKind, DatabaseAdapter, SqliteAdapter};
use crate::App;
use crate::dialect::{Action, Filter, FilterKind, RenderTarget};
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

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

            // Instance methods from schema-derived attributes.
            // These are per-model (column names differ across
            // models), so they stay outside the catalog — the
            // catalog is for per-receiver-kind AR methods, not
            // per-model schema projections.
            for (name, ty) in &model.attributes.fields {
                cls.instance_methods.insert(name.clone(), ty.clone());
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
            // Associations as instance methods (return types derived from cardinality).
            for assoc in model.associations() {
                use crate::dialect::Association;
                match assoc {
                    Association::BelongsTo { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Union {
                                variants: vec![
                                    Ty::Class { id: target.clone(), args: vec![] },
                                    Ty::Nil,
                                ],
                            },
                        );
                    }
                    Association::HasOne { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Union {
                                variants: vec![
                                    Ty::Class { id: target.clone(), args: vec![] },
                                    Ty::Nil,
                                ],
                            },
                        );
                    }
                    Association::HasMany { name, target, .. }
                    | Association::HasAndBelongsToMany { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Array {
                                elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }),
                            },
                        );
                    }
                }
            }

            classes.insert(model.name.clone(), cls);
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
            classes.entry(lc.name.clone()).or_default();
        }

        Self { classes, inferred_params: HashMap::new(), adapter }
    }

    /// Build a body-typer borrowing this analyzer's dispatch tables.
    /// Cheap — just a struct with a reference.
    fn body_typer(&self) -> BodyTyper<'_> {
        BodyTyper::new(&self.classes)
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

    /// One full typing pass over the whole app. Extracted from
    /// `analyze` so the fixpoint loop above can re-invoke it after
    /// each registry refinement. The Rails-aware orchestration
    /// (controller→view ivar channel, before_action seeding,
    /// per-model two-pass ivar discovery, partial locals threading)
    /// stays internal to this method; the fixpoint just calls it.
    fn run_typing_passes(&self, app: &mut App) {
        // Controller→view ivar channel: as each action is analyzed, we harvest
        // the ivars it sets and key them by the view that action renders.
        // When we reach the view pass below, the view's Ctx is seeded from
        // this map so `@article.title` in `articles/show.html.erb` types
        // against the `@article` bound in `ArticlesController#show`.
        let mut action_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        for controller in &mut app.controllers {
            let ctx = Ctx {
                self_ty: Some(Ty::Class {
                    id: controller
                        .parent
                        .clone()
                        .unwrap_or_else(|| ClassId(Symbol::from("ApplicationController"))),
                    args: vec![],
                }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                constants: HashMap::new(), annotate_self_dispatch: false,
            };
            let ctrl_name = controller.name.clone();

            // Snapshot every `before_action` on this controller once, so the
            // two-pass analysis below can consult the list without re-borrow.
            let before_filters: Vec<Filter> = controller
                .filters()
                .filter(|f| matches!(f.kind, FilterKind::Before))
                .cloned()
                .collect();

            // Pass A: analyze every action body once with no seed. Required
            // before we can harvest each action's produced ivar bindings —
            // the callback targets (`set_article`) are themselves actions.
            for action in controller.actions_mut() {
                self.body_typer().analyze_expr(&mut action.body, &ctx);
                action.effects = self.collect_effects(&mut action.body, &ctx);
            }

            // Snapshot each action's ivar_bindings. Used both to resolve
            // before_action targets (Pass B) and to seed view Ctx (below).
            let action_bindings: HashMap<Symbol, HashMap<Symbol, Ty>> = controller
                .actions()
                .map(|a| {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&a.body, &mut ivars);
                    (a.name.clone(), ivars)
                })
                .collect();

            // Pass B: re-analyze actions affected by a before_action with the
            // target's bindings pre-seeded into Ctx. Rails' `before_action`
            // runs before the action body, so any ivar the filter sets is in
            // scope for the whole body. Idempotent analyze means two passes
            // produce consistent types; cost is negligible for real
            // controllers.
            if !before_filters.is_empty() {
                for action in controller.actions_mut() {
                    let seed = merged_before_seed(&before_filters, &action.name, &action_bindings);
                    if !seed.is_empty() {
                        let inner_ctx = Ctx {
                            self_ty: ctx.self_ty.clone(),
                            ivar_bindings: seed,
                            local_bindings: HashMap::new(),
                            constants: HashMap::new(), annotate_self_dispatch: false,
                        };
                        self.body_typer().analyze_expr(&mut action.body, &inner_ctx);
                        action.effects = self.collect_effects(&mut action.body, &inner_ctx);
                    }
                }
            }

            // Build the per-view ivar map. Each view gets the action's own
            // assignments *plus* any before_action contribution (which isn't
            // syntactically present in the action body).
            for action in controller.actions() {
                if let Some(view_name) = view_name_for_action(&ctrl_name, action) {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&action.body, &mut ivars);
                    for filter in &before_filters {
                        if before_filter_applies(filter, &action.name) {
                            if let Some(fivars) = action_bindings.get(&filter.target) {
                                for (k, v) in fivars {
                                    ivars.entry(k.clone()).or_insert_with(|| v.clone());
                                }
                            }
                        }
                    }
                    action_ivars_by_view.insert(view_name, ivars);
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
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: class_ivars.clone(),
                local_bindings: HashMap::new(),
                constants: HashMap::new(), annotate_self_dispatch: false,
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
                    constants: HashMap::new(), annotate_self_dispatch: false,
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
        for lc in &mut app.library_classes {
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: lc.name.clone(), args: vec![] }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
                constants: HashMap::new(), annotate_self_dispatch: false,
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

            if !flow_ivars.is_empty() {
                let mut reseeded: HashMap<Symbol, Ty> = HashMap::new();
                for (name, ty) in flow_ivars {
                    reseeded.insert(name, Ty::Union { variants: vec![ty, Ty::Nil] });
                }
                let reseeded_ctx = Ctx {
                    self_ty: Some(Ty::Class { id: lc_name.clone(), args: vec![] }),
                    ivar_bindings: reseeded,
                    local_bindings: HashMap::new(),
                    constants: HashMap::new(), annotate_self_dispatch: false,
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

        // Phase 3a: non-partial views (action views + layouts). Analyze with
        // the controller→view ivar seed, then walk the body to record every
        // `render` call's effect on partial_locals_by_name.
        for view in &mut app.views {
            if is_partial_view_name(&view.name) {
                continue;
            }
            let mut view_ctx = Ctx::default();
            if let Some(ivars) = action_ivars_by_view.get(&view.name) {
                view_ctx.ivar_bindings = ivars.clone();
            }
            self.body_typer().analyze_expr(&mut view.body, &view_ctx);
            extract_partial_render_sites(&view.body, &view.name, &mut partial_locals_by_name);
        }

        // Phase 3b: partials. Seed local_bindings from the map built above,
        // then analyze.
        for view in &mut app.views {
            if !is_partial_view_name(&view.name) {
                continue;
            }
            let mut view_ctx = Ctx::default();
            if let Some(locals) = partial_locals_by_name.get(&view.name) {
                view_ctx.local_bindings = locals.clone();
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
            let ctx = Ctx::default();
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
                let Some(body_ty) = method.body.ty.clone() else { continue };
                if matches!(body_ty, Ty::Var { .. }) {
                    continue;
                }
                let target = match method.receiver {
                    crate::dialect::MethodReceiver::Instance => {
                        &mut self.classes.entry(class_id.clone()).or_default().instance_methods
                    }
                    crate::dialect::MethodReceiver::Class => {
                        &mut self.classes.entry(class_id.clone()).or_default().class_methods
                    }
                };
                Self::insert_inferred_return(target, &method.name, body_ty);
            }
        }
        for lc in &app.library_classes {
            let class_id = &lc.name;
            for method in &lc.methods {
                let Some(body_ty) = method.body.ty.clone() else { continue };
                if matches!(body_ty, Ty::Var { .. }) {
                    continue;
                }
                let target = match method.receiver {
                    crate::dialect::MethodReceiver::Instance => {
                        &mut self.classes.entry(class_id.clone()).or_default().instance_methods
                    }
                    crate::dialect::MethodReceiver::Class => {
                        &mut self.classes.entry(class_id.clone()).or_default().class_methods
                    }
                };
                Self::insert_inferred_return(target, &method.name, body_ty);
            }
        }
    }

    /// Conservative insertion: don't overwrite a `Ty::Fn` (RBS-sourced
    /// signature whose return is what dispatch already returns). Don't
    /// overwrite a more-concrete type with `Ty::Var`. Otherwise replace
    /// or insert. This is the join rule that keeps RBS-declared
    /// signatures authoritative while letting inference fill the rest.
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
                self.collect_send_sites(&method.body, &mut sites);
            }
            for scope in model.scopes() {
                self.collect_send_sites(&scope.body, &mut sites);
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                self.collect_send_sites(&method.body, &mut sites);
            }
        }
        for controller in &app.controllers {
            for action in controller.actions() {
                self.collect_send_sites(&action.body, &mut sites);
            }
        }
        for view in &app.views {
            self.collect_send_sites(&view.body, &mut sites);
        }
        if let Some(seeds) = &app.seeds {
            self.collect_send_sites(seeds, &mut sites);
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
        out: &mut Vec<(ClassId, Symbol, Vec<Ty>)>,
    ) {
        match &*expr.node {
            ExprNode::Send { recv, method, args, block, .. } => {
                let recv_class = match recv {
                    Some(r) => match r.ty.as_ref() {
                        Some(Ty::Class { id, .. }) => Some(id.clone()),
                        _ => None,
                    },
                    None => None,
                };
                if let Some(class_id) = recv_class {
                    let arg_tys: Vec<Ty> = args
                        .iter()
                        .map(|a| a.ty.clone().unwrap_or(Ty::Var { var: crate::ident::TyVar(0) }))
                        .collect();
                    out.push((class_id, method.clone(), arg_tys));
                }
                if let Some(r) = recv { self.collect_send_sites(r, out); }
                for a in args { self.collect_send_sites(a, out); }
                if let Some(b) = block { self.collect_send_sites(b, out); }
            }
            ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
                for e in exprs { self.collect_send_sites(e, out); }
            }
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    self.collect_send_sites(k, out);
                    self.collect_send_sites(v, out);
                }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.collect_send_sites(cond, out);
                self.collect_send_sites(then_branch, out);
                self.collect_send_sites(else_branch, out);
            }
            ExprNode::Case { scrutinee, arms } => {
                self.collect_send_sites(scrutinee, out);
                for arm in arms {
                    if let Some(g) = &arm.guard { self.collect_send_sites(g, out); }
                    self.collect_send_sites(&arm.body, out);
                }
            }
            ExprNode::BoolOp { left, right, .. }
            | ExprNode::RescueModifier { expr: left, fallback: right } => {
                self.collect_send_sites(left, out);
                self.collect_send_sites(right, out);
            }
            ExprNode::Let { value, body, .. } => {
                self.collect_send_sites(value, out);
                self.collect_send_sites(body, out);
            }
            ExprNode::Lambda { body, .. } => self.collect_send_sites(body, out),
            ExprNode::Apply { fun, args, block } => {
                self.collect_send_sites(fun, out);
                for a in args { self.collect_send_sites(a, out); }
                if let Some(b) = block { self.collect_send_sites(b, out); }
            }
            ExprNode::Assign { target, value } => {
                self.collect_send_sites(value, out);
                if let LValue::Attr { recv, .. } = target {
                    self.collect_send_sites(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    self.collect_send_sites(recv, out);
                    self.collect_send_sites(index, out);
                }
            }
            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.collect_send_sites(expr, out);
                    }
                }
            }
            ExprNode::Yield { args } => {
                for a in args { self.collect_send_sites(a, out); }
            }
            ExprNode::Raise { value } => self.collect_send_sites(value, out),
            ExprNode::Return { value } => self.collect_send_sites(value, out),
            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args { self.collect_send_sites(a, out); }
                }
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.collect_send_sites(body, out);
                for rc in rescues {
                    for c in &rc.classes { self.collect_send_sites(c, out); }
                    self.collect_send_sites(&rc.body, out);
                }
                if let Some(e) = else_branch { self.collect_send_sites(e, out); }
                if let Some(e) = ensure { self.collect_send_sites(e, out); }
            }
            ExprNode::Next { value } => {
                if let Some(v) = value { self.collect_send_sites(v, out); }
            }
            ExprNode::MultiAssign { value, .. } => self.collect_send_sites(value, out),
            ExprNode::While { cond, body, .. } => {
                self.collect_send_sites(cond, out);
                self.collect_send_sites(body, out);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { self.collect_send_sites(b, out); }
                if let Some(e) = end { self.collect_send_sites(e, out); }
            }
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
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
            ExprNode::Assign { target, value } => {
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
            ExprNode::Next { value } => {
                if let Some(v) = value { self.visit_effects(v, ctx, out); }
            }
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
        // lives in each target's runtime, not here.
        if id.0.as_str() == "ApplicationController" {
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
fn before_filter_applies(filter: &Filter, action_name: &Symbol) -> bool {
    if !filter.only.is_empty() {
        return filter.only.contains(action_name);
    }
    if !filter.except.is_empty() {
        return !filter.except.contains(action_name);
    }
    true
}

/// Merge ivar bindings from every before_action that applies to this action,
/// looking up each filter's `target` in the pre-computed per-action bindings
/// table. Later filters overwrite earlier ones on conflicting keys —
/// matches Rails' "last-registered wins" when the same ivar is set by
/// multiple callbacks.
fn merged_before_seed(
    before_filters: &[Filter],
    action_name: &Symbol,
    action_bindings: &HashMap<Symbol, HashMap<Symbol, Ty>>,
) -> HashMap<Symbol, Ty> {
    let mut seed: HashMap<Symbol, Ty> = HashMap::new();
    for filter in before_filters {
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
) {
    match &*expr.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            // Detect the `render` call shape (no explicit receiver, or the
            // receiver is an implicit context — Rails makes both work).
            if recv.is_none() && method.as_str() == "render" {
                if let Some((partial_name, locals)) = interpret_render_call(args, current_view) {
                    let entry = out.entry(partial_name).or_default();
                    for (k, v) in locals {
                        entry.insert(k, v);
                    }
                }
            }
            if let Some(r) = recv {
                extract_partial_render_sites(r, current_view, out);
            }
            for a in args {
                extract_partial_render_sites(a, current_view, out);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                extract_partial_render_sites(e, current_view, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                extract_partial_render_sites(k, current_view, out);
                extract_partial_render_sites(v, current_view, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_partial_render_sites(cond, current_view, out);
            extract_partial_render_sites(then_branch, current_view, out);
            extract_partial_render_sites(else_branch, current_view, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            extract_partial_render_sites(scrutinee, current_view, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    extract_partial_render_sites(g, current_view, out);
                }
                extract_partial_render_sites(&arm.body, current_view, out);
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            extract_partial_render_sites(left, current_view, out);
            extract_partial_render_sites(right, current_view, out);
        }
        ExprNode::Let { value, body, .. } => {
            extract_partial_render_sites(value, current_view, out);
            extract_partial_render_sites(body, current_view, out);
        }
        ExprNode::Lambda { body, .. } => {
            extract_partial_render_sites(body, current_view, out);
        }
        ExprNode::Apply { fun, args, block } => {
            extract_partial_render_sites(fun, current_view, out);
            for a in args {
                extract_partial_render_sites(a, current_view, out);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out);
            }
        }
        ExprNode::Assign { value, .. } => {
            extract_partial_render_sites(value, current_view, out);
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    extract_partial_render_sites(expr, current_view, out);
                }
            }
        }
        _ => {}
    }
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
    if let ExprNode::Hash { entries, .. } = &*first.node {
        let mut partial_name: Option<String> = None;
        let mut locals: HashMap<Symbol, Ty> = HashMap::new();
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
                _ => {}
            }
        }
        if let Some(name) = partial_name {
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
/// `ArticlesController` → `articles`. Strip the `Controller` suffix, then
/// snake_case what remains. Namespaced controllers (`Admin::UsersController`)
/// are handled by the current snake_case rule producing `admin::users`; when
/// a fixture forces namespaced views, we'll fix the rule to emit `/` instead.
fn controller_view_prefix(class_id: &ClassId) -> String {
    let name = class_id.0.as_str();
    let stripped = name.strip_suffix("Controller").unwrap_or(name);
    crate::naming::snake_case(stripped)
}

/// Determine which view path an action's RenderTarget names — `None` if
/// the action doesn't render a template (redirect, JSON, head).
fn view_name_for_action(controller: &ClassId, action: &Action) -> Option<Symbol> {
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
        ExprNode::Seq { exprs } => {
            for e in exprs {
                extract_ivar_assignments(e, out);
            }
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            extract_ivar_assignments(then_branch, out);
            extract_ivar_assignments(else_branch, out);
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
        // `@x ||= y` lowers to `BoolOp::Or(Ivar, Assign(Ivar, y))`.
        // The assignment lives inside the Or's right branch, so we
        // must descend or the memoization idiom's ivar never gets typed.
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

// Diagnostic emission -----------------------------------------------------

/// Re-exports: the shared diagnostic types live in `crate::diagnostic`
/// so the body-typer can annotate `Expr.diagnostic` without a
/// dependency cycle. External callers (tests, future CLIs) continue
/// to import them from `roundhouse::analyze` as before.
pub use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};

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
    out
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
        ExprNode::Assign { target, value } => {
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
        ExprNode::Next { value } => {
            if let Some(v) = value { diagnose_expr(v, out); }
        }
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
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
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
