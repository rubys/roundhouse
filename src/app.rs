use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dialect::{
    Controller, Filter, Fixture, LibraryClass, Model, ModelBodyItem, RouteTable, TestModule,
    View,
};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::schema::Schema;
use crate::ty::Ty;

/// The top-level IR: a Rails application as data. This is the serializable
/// deliverable — the thing ingesters produce and emitters consume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct App {
    pub schema_version: u32,
    pub schema: Schema,
    pub models: Vec<Model>,
    /// Non-model classes living under `app/models/` (e.g. specialized
    /// has_many proxies). Classified at ingest time by superclass:
    /// extends ApplicationRecord/ActiveRecord::Base → `models`;
    /// otherwise → here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_classes: Vec<LibraryClass>,
    pub controllers: Vec<Controller>,
    pub routes: RouteTable,
    pub views: Vec<View>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_modules: Vec<TestModule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixtures: Vec<Fixture>,
    /// Body of `db/seeds.rb` as a typed expression (usually a
    /// `Seq` of AR-create calls with an early-return guard). The
    /// TS emitter wraps it in `async function run()` and the
    /// generated `main.ts` invokes it at startup when the DB is
    /// fresh. None when the app has no seeds file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeds: Option<Expr>,
    /// Pins from `config/importmap.rb`, expanded (each
    /// `pin_all_from` has been resolved into explicit per-file
    /// pins via `app/javascript/**` walking). Consumed by the
    /// `<%= javascript_importmap_tags %>` view-helper lowering.
    /// None when the app has no importmap.rb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importmap: Option<Importmap>,
    /// Logical stylesheet names discovered in `app/assets/stylesheets/`
    /// + `app/assets/builds/` (file stems without `.css`). When the
    /// ERB uses `stylesheet_link_tag :app, ...`, Rails with Propshaft
    /// + tailwindcss-rails expands to one `<link>` per stylesheet in
    /// these dirs; our emitter mirrors the expansion so the rendered
    /// head matches structurally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stylesheets: Vec<String>,
    /// User-authored RBS sidecars discovered under `sig/**/*.rbs` in
    /// the Rails app root. Keyed by fully-qualified class/module name
    /// (nested namespaces joined with `::`), inner map is method name
    /// → signature (`Ty::Fn`). The analyzer consults these when
    /// building `ClassInfo` so user methods the Rails conventions
    /// can't fully type (helpers, concerns, POROs) still flow types.
    /// Empty when the app ships no `sig/` directory.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub rbs_signatures: HashMap<ClassId, HashMap<Symbol, Ty>>,
    /// App-helper method registry: maps each method name defined in an
    /// `app/helpers/*.rb` module to the helper module (`ClassId`) that
    /// defines it. Rails mixes all helper modules into every view, so a
    /// bare `avatar_img(...)` in a template should resolve to the helper
    /// that declares it. The ruby emit-path helper-lowering pass uses this
    /// to (a) rewrite such bare calls to `<Module>.method(...)` and (b)
    /// emit the helper modules as module-functions. Last-writer-wins on a
    /// name collision (mirrors Rails include order). Empty when the app
    /// ships no helpers or only empty helper modules (the blog).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub helper_method_index: HashMap<Symbol, ClassId>,
    /// The app's `Rails::Application` subclass from
    /// `config/application.rb` (e.g. `Lobsters::Application`),
    /// reparented at ingest onto `Rails::Application` itself. Its
    /// instance methods are app config (`read_only?`, `name`, `domain`,
    /// `ssl?`) reached at runtime via `Rails.application.<m>` — the
    /// runtime shim memoizes `Rails::Application.new`, so emitting the
    /// class as a reopen makes them reachable regardless of require
    /// order (the app namespace is never referenced at runtime and
    /// drops out). None when the app has no config/application.rb or
    /// its class defines no methods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rails_application: Option<LibraryClass>,
    /// Filters declared inside a concern module's `included do` block
    /// (`AccountOwnedConcern` → its `before_action :set_account, …`
    /// lines), keyed by the module. Rails runs these as if written in
    /// each including class; analyze extends every includer's filter
    /// chain from this map so concern-seeded ivars (`@account`) resolve
    /// in actions and views. Populated by the concern-module arms of
    /// the app walk; empty for apps without concerns.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_filters: HashMap<ClassId, Vec<Filter>>,
    /// Model DSL declared inside a concern module's `included do`
    /// (`Account::Associations` → its `has_many :statuses` etc.),
    /// keyed by the module and classified as the same
    /// [`ModelBodyItem`]s a model body carries. Rails evaluates the
    /// block in each including model; analyze registers these items
    /// on every includer (associations as typed readers/writers,
    /// scopes as relation-returning class methods) so dispatch and
    /// completion see the mixed-in surface. Registry-level only for
    /// now — the items are deliberately NOT spliced into `Model.body`,
    /// keeping source round-trip exact; a transpile-grade splice (with
    /// item provenance) can follow when emission needs it.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_model_items: HashMap<ClassId, Vec<ModelBodyItem>>,
    /// Renderer view → the partial views it renders (`articles/show` →
    /// [`articles/_form`]), harvested from actual render sites as views
    /// are analyzed. The other half of the render graph that
    /// `view_feeders` closes over — persisted for related-file
    /// navigation (view ↔ its partials, partial ↔ its renderers).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub render_edges: HashMap<Symbol, Vec<Symbol>>,
    /// View name (`articles/show`, `articles/_form`, `layouts/application`)
    /// → controllers whose actions feed that view, recorded by analyze
    /// while it harvests the action→view ivar channel (explicit `render`
    /// targets and the implicit action-name convention), the effective-
    /// layout resolution, and the renderer→partial edges (a partial's
    /// feeders are the transitive union of its renderers'). This is the
    /// same linkage the ivar seeding used — persisted so consumers can
    /// trace a view-side symptom back to the controller responsible:
    /// diagnostic gap-attribution (an unresolved `@ivar` in a view whose
    /// feeder had an ingest gap is a coverage note, not a user error)
    /// and, later, controller↔view navigation. Sorted for determinism.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub view_feeders: HashMap<Symbol, Vec<ClassId>>,
    /// Source files read during ingest, indexed by `Span.file`
    /// (`FileId(n)` → `sources[n - 1]`; `FileId(0)` is the synthetic
    /// sentinel). Carries the parsed text so diagnostics can resolve
    /// byte-offset spans to file:line:col without re-reading disk —
    /// which the wasm ingest path couldn't do anyway. Empty for Apps
    /// built by hand in tests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::span::SourceFile>,
    /// Per-controller resolved request machinery, computed once by
    /// analyze's parent-chain walk and persisted (the self-describing-IR
    /// move: `run_typing_passes` already built these to seed ivars, and
    /// used to discard them). Keyed by controller class. Consumers —
    /// `ide::traceroute`, the MCP tool, gap attribution — compose over
    /// this instead of re-deriving inheritance + concern splicing.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub controller_resolutions: HashMap<ClassId, ControllerResolution>,
    /// App directory as passed to ingest (`fixtures/real-blog`), `""`
    /// for in-memory trees (map VFS, wasm). `sources` paths keep this
    /// prefix so diagnostics print compiler-cwd-relative (clickable)
    /// locations; consumers that need app-relative paths (source-map
    /// `sources` entries must not differ by ingest mode) strip it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root: String,
}

/// One controller's resolved request machinery: the full filter chain
/// as Rails would execute it (inheritance + concern splicing applied)
/// and the effective layout. Per-controller, not per-action — the
/// chain keeps each filter's `only:`/`except:` gating and any `skip_*`
/// entries, so the per-action view is a cheap filter over this record
/// (apply the gates, drop targets named by an applicable Skip) rather
/// than a duplicated copy per action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ControllerResolution {
    /// Filters in Rails execution order: ancestors' first (oldest
    /// ancestor's declarations first), then this controller's own body
    /// order with concern-contributed filters spliced at their
    /// `include` site. Includes `After` and `Skip` kinds — consumers
    /// pick the subset they care about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_chain: Vec<ResolvedFilter>,
    /// Effective layout view name (`layouts/application`), resolved by
    /// walking the inheritance chain; `None` records an explicit
    /// `layout false`. Convention default applies, so this may name a
    /// layout view the app doesn't ship — Rails would render bare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Symbol>,
}

/// One hop of a resolved filter chain: the declaration plus the
/// provenance and typed consequences analyze already knew when it
/// seeded ivars through this filter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFilter {
    /// The declaration as written (kind, target method, `only:`/
    /// `except:` gating, symbol-form `if:`/`unless:` guards).
    pub filter: crate::dialect::Filter,
    /// Class or concern module whose body declared this filter — the
    /// trace hop's "defined in AccountOwnedConcern", distinct from the
    /// controller whose chain it landed in.
    pub defined_in: ClassId,
    /// Ivars the target method's body assigns, with inferred types
    /// (`@account` → `Account`). Empty for `Skip` entries and for
    /// targets analyze couldn't see (e.g. framework-defined).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub assigns: HashMap<Symbol, Ty>,
    /// The target method's effect set (`DbRead`…), so a trace doubles
    /// as a static query profile without re-finding the method body.
    #[serde(default, skip_serializing_if = "crate::effect::EffectSet::is_pure")]
    pub effects: crate::effect::EffectSet,
}

/// A Rails-style importmap: one `<name>` → `<path>` entry per
/// pin, in declaration order (Rails preserves order for
/// modulepreload link emission).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Importmap {
    pub pins: Vec<ImportmapPin>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportmapPin {
    /// Module specifier the page imports (`"application"`,
    /// `"@hotwired/turbo-rails"`, `"controllers/hello_controller"`).
    pub name: String,
    /// Served asset path (`/assets/application.js`,
    /// `/assets/turbo.min.js`, …). Canonical (no fingerprint);
    /// real deployments sprinkle digests in here.
    pub path: String,
}

impl App {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            schema: Schema::default(),
            models: Vec::new(),
            library_classes: Vec::new(),
            controllers: Vec::new(),
            routes: RouteTable::default(),
            views: Vec::new(),
            test_modules: Vec::new(),
            fixtures: Vec::new(),
            seeds: None,
            importmap: None,
            stylesheets: Vec::new(),
            rbs_signatures: HashMap::new(),
            helper_method_index: HashMap::new(),
            rails_application: None,
            concern_filters: HashMap::new(),
            concern_model_items: HashMap::new(),
            render_edges: HashMap::new(),
            view_feeders: HashMap::new(),
            controller_resolutions: HashMap::new(),
            sources: Vec::new(),
            root: String::new(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
