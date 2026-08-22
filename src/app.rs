//! The whole-program IR root: a Rails application as one data
//! structure. Ingest produces an `App`, analyze annotates it in place,
//! the post-analyze lowerings reshape it, and every emitter consumes
//! it — this struct is the deliverable each stage hands the next. It
//! is serde-serializable end to end because tests, the wasm build, and
//! the IR dump round-trip it as JSON (`schema_version` names the
//! shape), so a field that can't serialize can't join the IR. Beyond
//! the core sections (schema, models, controllers, routes, views),
//! the trailing maps persist facts analyze already computed — partial
//! local types, view ivar contexts, render edges — so lowerers and
//! IDE consumers read them instead of re-deriving.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::dialect::{
    Controller, Filter, Fixture, LibraryClass, MethodDef, Model, ModelBodyItem, RouteTable,
    TestModule, View,
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
    /// Source files the text pipeline cannot represent — every emitted
    /// file's content is a `String`, so an app's images, fonts and
    /// binary test fixtures were silently dropped on the floor. Carried
    /// as `(relative path, bytes)` and copied VERBATIM into the emitted
    /// tree; there is nothing to transpile in a JPEG.
    ///
    /// `serde(skip)`: these are a passthrough concern, not IR. The app
    /// JSON round-trip feeds the analyzer and the browser IDE, and
    /// neither has any use for blob bytes — encoding them as JSON number
    /// arrays would bloat every dump for nothing.
    #[serde(skip)]
    pub binary_assets: Vec<(String, Vec<u8>)>,
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
    /// Controller methods the app declared view-visible with Rails'
    /// `helper_method :name`. A view lowers to a module function with no
    /// controller instance, so a bare call to one of these routes
    /// through the per-dispatch `ActionController::Current.controller`
    /// — the same seam `flash` and `cookies` already use.
    ///
    /// Distinct from [`Self::helper_method_index`], which maps a name to
    /// the `app/helpers/` MODULE that defines it. This one names methods
    /// that live on the CONTROLLER.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub view_visible_controller_methods: BTreeSet<Symbol>,
    /// Partial → local name → type, harvested by the analyzer from the
    /// RENDER SITES that pass each local (`render partial: "form",
    /// locals: { new_message: @new_message }` with `@new_message` typed
    /// `Message` by the controller's `Message.new`).
    ///
    /// The analyzer already computed this to seed each partial's body
    /// typing; recording it on the App is what lets the LOWERER stamp
    /// the same fact into the emitted signature. Without it the view
    /// lowerer can only guess a param's type from its NAME (`user` →
    /// `User`), so a local whose name isn't a model — lobsters'
    /// `new_message` — emits `untyped` and every read off it refuses on
    /// the strict targets, even though the analyzer knew the type all
    /// along. Empty for apps whose partials take no locals.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub partial_local_types: HashMap<Symbol, HashMap<Symbol, Ty>>,
    /// View → ivar name → type: the CONTROLLER-side ivar context each
    /// view renders against (`settings/index` sees `@edit_user: User`
    /// because the action assigns `@user.dup`), propagated transitively
    /// onto partials.
    ///
    /// The naming convention gets an ivar's type right whenever the name
    /// IS the model (`@user` → User) and cannot possibly get it right
    /// otherwise. `form_with model: @edit_user` is the case that bites:
    /// Rails names its fields from the RECORD's `param_key`
    /// (`user[username]`), and with no type for `@edit_user` the lowerer
    /// falls back to the view directory (`setting[username]`) — every
    /// field name and id on /settings wrong. The analyzer already
    /// computed this context to type each view body; recording it is
    /// what lets the lowerer name the form after the record.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub view_ivar_types: HashMap<Symbol, HashMap<Symbol, Ty>>,
    /// Methods whose result Rails would treat as an html-safe buffer,
    /// because their body ends in `<e>.html_safe` (lobsters'
    /// `Hat#to_html_label`). The mark is a VALUE-level fact in Rails,
    /// carried by a String subclass the shared runtime cannot have, so
    /// `lower::html_safe` records it here and erases the call. The view
    /// lowerer consults it before wrapping an interpolation in
    /// `html_escape` — escaping a marked result ships literal
    /// `&lt;span&gt;` markup to the page.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub html_safe_methods: BTreeSet<Symbol>,
    /// App-registered `to_fs` formats from `config/initializers/`, in
    /// both spellings Rails accepts: the current
    /// `ActiveSupport::TimeFormats.register(:name, fmt)` and the
    /// deprecated `Time::DATE_FORMATS[:name] = fmt` (campfire's
    /// `time_formats.rb` defines `:epoch` as
    /// `->(time) { (time.to_f * 1000).to_i }`).
    ///
    /// Recorded because `to_fs(:name)` is otherwise unknowable and
    /// Rails' fallback for an unknown format is `to_s` — a completely
    /// different value, silently. `lower::time_current` expands each
    /// call site from this. Empty for apps that register no formats.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub time_formats: BTreeMap<Symbol, TimeFormat>,
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
    /// Classes that were `ActiveSupport::CurrentAttributes` subclasses
    /// before `ingest::current_attributes` flattened them. Recorded
    /// because that pass CLEARS the parent — nothing downstream could
    /// recognize them afterwards — and the dispatch scaffold needs the
    /// names to emit a per-request `reset`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_attribute_classes: Vec<ClassId>,
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
    /// Chain segment that carried the filter in: the controller (or
    /// ancestor) whose class body — an `include` line for concern
    /// filters, the declaration itself otherwise — put this entry in
    /// the chain. Equals `defined_in` for directly-declared filters;
    /// for concern filters it's the includer (`set_locale`: defined_in
    /// `Localized`, included_via `ApplicationController`). Lets
    /// consumers group contiguous runs under two-level headers without
    /// reconstructing segment boundaries from order.
    pub included_via: ClassId,
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

/// What an app registered a `to_fs` format AS. Rails accepts both,
/// and `Time#to_fs` picks between them by asking the value whether it
/// responds to `call`:
///
/// ```ruby
/// formatter.respond_to?(:call) ? formatter.call(self).to_s : strftime(formatter)
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TimeFormat {
    /// A strftime string — `register(:month_and_year, "%B %Y")`.
    Strftime { format: String },
    /// A `->(t) { … }`, as a one-parameter method whose body is the
    /// lambda's. Inlined with the receiver substituted for the
    /// parameter, then `.to_s`, which is what Rails applies to the
    /// call's result above.
    Lambda { method: MethodDef },
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
            binary_assets: Vec::new(),
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
            view_visible_controller_methods: BTreeSet::new(),
            partial_local_types: HashMap::new(),
            view_ivar_types: HashMap::new(),
            html_safe_methods: BTreeSet::new(),
            time_formats: BTreeMap::new(),
            rails_application: None,
            concern_filters: HashMap::new(),
            concern_model_items: HashMap::new(),
            current_attribute_classes: Vec::new(),
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
