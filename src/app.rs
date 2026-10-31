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

use std::collections::{BTreeMap, BTreeSet};
use crate::det::HashMap;

use serde::{Deserialize, Serialize};

use crate::dialect::{
    Controller, Filter, Fixture, LibraryClass, MethodDef, Model, ModelBodyItem, RouteTable,
    TestModule, View,
};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::schema::Schema;
use crate::ty::Ty;

/// A condition on the including class, read from a concern's
/// `included do`: `self == A` / `self != A` / `in?([A, B])`, under `if`
/// or `unless`. `classes` are the constants as written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IncluderGuard {
    pub classes: Vec<String>,
    pub negate: bool,
}

impl IncluderGuard {
    /// Whether the guard holds for the includer named `includer` (its
    /// full name). A root-qualified constant (`::Account`) names exactly
    /// one class and matches only that full name. An unqualified one
    /// matches the full name or its trailing segments (`Account` admits
    /// `Billing::Account` only when written in that namespace; the ingest
    /// sees the bare name, so a suffix match on `::` boundaries is what
    /// it can do).
    pub fn admits(&self, includer: &str) -> bool {
        let hit = self.classes.iter().any(|c| match c.strip_prefix("::") {
            Some(rooted) => includer == rooted,
            None => includer == c || includer.ends_with(&format!("::{c}")),
        });
        hit != self.negate
    }
}

/// One item of [`App::concern_guarded_items`]: the guard, the item, and
/// `at`, the number of the module's unconditional items
/// (`concern_model_items`) declared before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConcernGuardedItem {
    pub guard: IncluderGuard,
    pub item: ModelBodyItem,
    pub at: usize,
}

/// The top-level IR: a Rails application as data. This is the serializable
/// deliverable — the thing ingesters produce and emitters consume.

/// A `validates` call kept for its custom keys (`ingest::each_validator`):
/// the model, the call's span and source text, and each key with the app
/// `EachValidator` class it names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EachValidatorCall {
    pub model: ClassId,
    pub span: Span,
    pub text: String,
    pub keys: Vec<(String, String)>,
}

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
    /// Framework calls still use their lowered ABI; native argument
    /// preservation requires a source library declaration.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_library_keywords: bool,
    /// Each gem-source file whose top level runs more than declarations
    /// when Ruby loads it (`ingest::library_class::file_program`): its
    /// declarations, statements and requires, in source order. The Ruby
    /// family emits each as one file (`app/models/roundhouse_files/…`),
    /// loaded where a carried file requires it, else at boot when
    /// Bundler requires its gem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_programs: Vec<FileProgram>,
    /// The gem-source files the boot loads, in load order: a literal
    /// `require` in config/boot.rb, application.rb or environment.rb,
    /// and `Bundler.require`'s entry file of each PATH or source gem
    /// (`lib/<name>.rb`, or what the Gemfile's `require:` names); then
    /// the gem files an app support file requires, as it loads at boot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boot_gem_entries: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia: Option<InertiaConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vite_manifest: Option<ViteManifest>,
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
    /// What a `sig/**/*.rbs` sidecar says a class INCLUDES, keyed by
    /// the declaring class and holding the module names as written.
    ///
    /// Ancestry is the one fact about a gem's base class that the tree
    /// cannot see for itself, and guessing it from a method name is
    /// what `docs/guide/transpile.md` says not to do. A sidecar is
    /// where an app states it.
    pub rbs_includes: HashMap<ClassId, Vec<ClassId>>,
    /// The app's `Gemfile.lock`, parsed, when the tree carries one.
    /// Read at ingest for the gem census ([`crate::gems`]) and the
    /// unknown-gem attribution of diagnostics, and by the view lowering
    /// for the one gem that changes a framework helper's markup: `lexxy`
    /// replaces Action Text's `rich_text_area`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gem_lock: Option<crate::gems::Lockfile>,
    /// The Rails frameworks `config/application.rb` loads, as top-level
    /// constant names (`rails/all` → every framework; individual
    /// `require "active_storage/engine"` lines → just those). `None`
    /// when the file is absent or unreadable, which is also what a
    /// non-Rails tree looks like. Read by `lower::defined_const_fold`,
    /// which answers `defined?(ActiveStorage)` from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_frameworks: Option<Vec<String>>,
    /// The environments whose `config/environments/<env>.rb` sets
    /// `config.assume_ssl = true` (TLS ends at a proxy in front of the app).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assume_ssl_envs: Vec<String>,
    /// The classes and method headers the locked gems' Tapioca RBIs
    /// declare ([`crate::gem_boundary`]). Declarations only, read at
    /// ingest when the tree (or the real location of its lockfile) has
    /// `sorbet/rbi/gems/`; empty otherwise.
    #[serde(default, skip_serializing_if = "crate::gem_boundary::GemBoundary::is_empty")]
    pub gem_boundary: crate::gem_boundary::GemBoundary,
    /// The gem and DSL facts of the extensions `roundhouse.yml` lists
    /// ([`crate::extensions`]), loaded at ingest for this app alone. Empty
    /// when it lists none.
    #[serde(default, skip_serializing_if = "crate::extensions::Extensions::is_empty")]
    pub extensions: crate::extensions::Extensions,
    /// `roundhouse.yml`'s `spinel.int_overflow` (`promote`, `wrap`), handed
    /// to Spinel as `--int-overflow`. Provisional: an app should not have
    /// to choose this, and the setting is to go once the need is decided
    /// without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spinel_int_overflow: Option<String>,
    /// Attributes the app (and its gems) add to Action Text's sanitizer
    /// allow-list at boot — `ActionText::ContentHelper.allowed_attributes`
    /// as the initializers leave it, minus the framework defaults the
    /// runtime already has. Resolved to literals at ingest
    /// (`ingest::app::content_helper_attribute_additions`); rendered into
    /// the runtime's generated `ContentHelper.app_allowed_attributes`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_helper_allowed_attributes: Vec<String>,
    /// Tags the app adds the same way — `allowed_tags=` in a
    /// `to_prepare` / initializer (campfire's
    /// `ContentFilters::EDITOR_FORMATTING_TAGS`). Rendered into
    /// `ContentHelper.app_allowed_tags`. Needed once sanitize runs on
    /// render: without it, editor markup (`s`/`u`/table) is stripped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_helper_allowed_tags: Vec<String>,
    /// Call-site-unified parameter types, written by the analyzer's
    /// fixpoint (`unify_params_from_call_sites`) as its last act and
    /// read by lowerings that build method signatures — the controller
    /// lowering's private-helper params were pinned `untyped` before
    /// this channel existed, which dissolved everything reached
    /// through such a param (campfire's `broadcast_create_room(room)`
    /// took the typed `Room` its one caller passes, and `room` still
    /// widened every chain under it to poly). Keyed like the
    /// analyzer's own table: (declaring class, method) → positional
    /// param types, `Ty::Var` where no call site contributed.
    #[serde(skip)]
    pub inferred_method_params: HashMap<(ClassId, Symbol), Vec<Ty>>,
    /// Source files the text pipeline cannot represent — every emitted
    /// file's content is a `String`, so an app's images, fonts and
    /// binary test fixtures were silently dropped on the floor. Carried
    /// as `(relative path, bytes)` and copied VERBATIM into the emitted
    /// tree; there is nothing to transpile in a JPEG. Compiled Propshaft
    /// assets use the same byte channel so binary and text assets are
    /// copied together.
    ///
    /// `serde(skip)`: these are a passthrough concern, not IR. The app
    /// JSON round-trip feeds the analyzer and the browser IDE, and
    /// neither has any use for blob bytes — encoding them as JSON number
    /// arrays would bloat every dump for nothing.
    #[serde(skip)]
    pub binary_assets: Vec<(String, Vec<u8>)>,
    /// `config/locales` for the default locale (see `crate::i18n`).
    #[serde(default, skip_serializing_if = "crate::i18n::Catalog::is_empty")]
    pub i18n: crate::i18n::Catalog,
    /// The sites of a `t` whose answer differs by locale
    /// (`lower::i18n_translate`), whose entries the ruby-family trees write
    /// to the translation table the runtime reads.
    #[serde(skip)]
    pub i18n_table: Vec<crate::lower::i18n_translate::TableSite>,
    /// The `t` calls left to `I18n.translate_runtime`, which reads the
    /// app's whole catalog (written beside the table) when it runs.
    #[serde(skip)]
    pub i18n_runtime_sites: Vec<crate::span::Span>,
    /// Every `t` that reads the translation table or the catalog when it
    /// runs: the Ruby-family runtime alone carries them.
    #[serde(skip)]
    pub i18n_table_sites: Vec<crate::span::Span>,
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
    /// Methods of a helper object the app builds at load time from a
    /// YAML table (`WebUrlHelpers = WebUrlHelpersFactory.create(
    /// YAML.load_file(…)["paths"])`), keyed by the constant. The names
    /// come from the table's keys and the factory's `define_method`
    /// suffixes — see [`crate::ingest::generated_helpers`]. A registry
    /// fact for the analyzer only: the constant's initializer still
    /// builds the object at runtime, so nothing is emitted for it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub generated_helper_methods: BTreeMap<ClassId, BTreeSet<Symbol>>,
    /// Model class names a `GlobalID::Locator.locate(gid, only: K)`
    /// call site names, collected by [`crate::lower::global_id_locate`]
    /// as it rewrites each site to a per-model `locate_<model>`.
    ///
    /// The generic `locate` takes its finder as a CLASS OBJECT, and a
    /// class object has no singleton on a strict target — `only.find`
    /// emits a call to a class method `ActiveRecord::Base` never
    /// defines (matz/spinel#4217). The set is what
    /// `project::apply_global_id_locate` needs to write one
    /// specialization per model into `runtime/global_id_locator.rb`,
    /// where the finder is spelled as a literal constant.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub global_id_locate_models: BTreeSet<Symbol>,
    /// Model class names a `GlobalID::Locator.locate_signed(sgid,
    /// only: K, for:)` call site names. Same monomorphize reason as
    /// [`Self::global_id_locate_models`]; the generated entry point is
    /// `locate_signed_<model>(sgid, purpose)`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub global_id_locate_signed_models: BTreeSet<Symbol>,
    /// How each job's arguments are written to an ActiveJob payload, or
    /// why the job keeps its Proc. Worked out by
    /// [`crate::lower::job_payload::plan_jobs`] from the job's call
    /// sites; read by the Ruby emitter's enqueue rewrite and by
    /// `project::apply_job_registry`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub job_plans: Vec<crate::lower::job_payload::JobPlan>,
    /// The ActiveJob queue adapter the app configures, as a name
    /// (`config.active_job.queue_adapter = :sidekiq` gives `"sidekiq"`).
    /// Read at ingest from `config/application.rb`, then
    /// `config/environments/production.rb`, then `config/initializers`,
    /// the last setting winning, then from `self.queue_adapter =` in
    /// `ApplicationJob`, which covers every job and so overrides them;
    /// `None` when nothing sets it or the last
    /// setting isn't a literal. Only `"sidekiq"` changes what is emitted
    /// (jobs are pushed to Redis on the Ruby family); any other adapter
    /// keeps the in-process queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_adapter: Option<String>,
    /// The Redis the app's `Sidekiq.configure_client` block names
    /// (`config.redis = { url: …, password: … }`), read at ingest from
    /// `config/initializers`. It is the source of truth for where jobs
    /// are pushed; `REDIS_PROVIDER`, `REDIS_URL` and localhost are only
    /// the fallback when it names no URL. Other keys are not read.
    #[serde(default, skip_serializing_if = "SidekiqRedisConfig::is_empty")]
    pub sidekiq_redis: SidekiqRedisConfig,
    /// The worker's defaults from `config/sidekiq.yml` (concurrency,
    /// timeout, queues), read at ingest by `ingest::sidekiq_yml`. The
    /// worker's own `-c`, `-t` and `-q` win over them.
    #[serde(default, skip_serializing_if = "SidekiqWorkerDefaults::is_empty")]
    pub sidekiq_worker: SidekiqWorkerDefaults,
    /// `config/cable.yml`'s production Redis, when its adapter is
    /// `redis`, read at ingest by `ingest::cable_yml`. A tree on
    /// `:sidekiq` then sends every broadcast through it, so one a
    /// worker makes reaches the web processes' sockets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable_redis: Option<CableRedisConfig>,
    /// Model names whose attachment sgid the app resolves even when
    /// its SIGNATURE fails — campfire's `%w[ User ]`, read by
    /// [`crate::ingest::on_load_reopen`] from the `from_node` reopen
    /// in its `lib/rails_ext/`, so that rotating `SECRET_KEY_BASE`
    /// does not orphan every @mention. In source order, as the app
    /// wrote them. `project::apply_attachable_locate` writes the list
    /// into the emitted `global_id_locator.rb` as
    /// `ActionText::Attachment.permitted_without_signature`; empty for
    /// every app without the reopen, where a tampered sgid is missing
    /// as it is in stock Rails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachable_unsigned_models: Vec<Symbol>,
    /// Pending `on_load(:active_storage_attachment)` includes-only
    /// reopens: `(model_name, module_paths)`. File order can visit the
    /// load-hook file before the model; ingest applies these once
    /// `app.models` is complete, just before the concern splice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_attachment_on_load: Vec<(Symbol, Vec<Symbol>)>,
    /// Modules `include`d inside `ActiveSupport.on_load(:active_record)`
    /// that provide class-method macros. Mixin instance methods are not
    /// installed. Expansion treats these as an explicit provider origin
    /// (not a seeded `include` set on every model).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_hook_class_macros: Vec<ClassId>,
    /// Modules `extend`ed inside `ActiveSupport.on_load(:active_record)`:
    /// their instance methods are class-method macros on every model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_hook_extended_macros: Vec<ClassId>,
    /// Class methods a model took from a module only it extends
    /// (`ingest::extend_splice`), by name.
    #[serde(default, skip_serializing_if = "std::collections::HashSet::is_empty")]
    pub spliced_class_methods: crate::det::HashSet<Symbol>,
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
    /// The same marks with the class that defines each (`(class,
    /// method)`), for a reader that knows the owner: two helper modules
    /// can define one name, and only one of them mark it.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub html_safe_method_owners: BTreeSet<(String, String)>,
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
    /// `X.prepend Y` / `X.include Y` registered by a
    /// `config/initializers/` file — the one initializer shape that
    /// CHANGES METHOD LOOKUP, and therefore the one a tree cannot
    /// silently drop.
    ///
    /// campfire's `turbo_streams_authorization.rb` is the case that
    /// forced this: `Turbo::StreamsChannel.prepend
    /// RoomStreamsAreAuthorized` is what makes `RoomMessagesChannel`
    /// the only door onto a room's message stream. The concern itself
    /// ingests (it lives in `app/channels/concerns/`), so dropping the
    /// prepend left the guard defined and unreachable — a security
    /// control present in the tree and not in the lookup chain.
    ///
    /// Recorded here rather than executed at ingest because the mixin
    /// only means something once BOTH constants exist in the emitted
    /// tree; `lower::module_mixins` drops the ones that cannot resolve
    /// and reports them, so a tree never carries a line that would
    /// NameError at boot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_mixins: Vec<ModuleMixin>,
    /// Generated body of `runtime/active_record/app_rake_tasks.rb` when
    /// `lib/tasks` prepends onto `ActiveRecord::Tasks::DatabaseTasks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rake_tasks_source: Option<String>,
    /// Why a `.rake` file was not installed. The sentence is the warning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rake_task_gaps: Vec<String>,
    /// `X.before_action :m[, only: …]` registered by a
    /// `config/initializers/` file — the mixin's companion: an app
    /// that mixes a guard INTO a framework controller also has to
    /// tell that controller to RUN it, and Rails apps write the two
    /// lines together. campfire's `active_storage_authentication.rb`
    /// includes `ActiveStorageAuthentication` into Active Storage's
    /// direct-upload controllers and adds the `before_action` that
    /// makes an anonymous upload a 401. Same standing as
    /// `module_mixins`: recorded at ingest, resolved by
    /// `lower::module_mixins`, which keeps only a filter onto a
    /// runtime controller that carries the seam and whose method a
    /// kept mixin supplies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initializer_filters: Vec<InitializerFilter>,
    /// The names a host initializer `def`s at its top level: methods on
    /// Object, which the emitted app does not define.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initializer_defs: Vec<crate::ident::Symbol>,
    /// Literal settings on a gem package's config receiver, from
    /// `config/initializers/` (`ingest::gem_package_settings`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gem_package_settings: Vec<GemPackageSetting>,
    /// Installed gems compiled from source because the app's code needs
    /// them and nothing models or ports them (`ingest::auto_source_gems`).
    /// The Spinel tree carries their code; the ruby family loads the real
    /// gem instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_source_gems: Vec<String>,
    /// `X.after_action(only: …) { response.set_header(k, v) }` from a
    /// `config/initializers/` file, bare or inside `ActiveSupport.on_load`:
    /// campfire marks the disk service's file responses cacheable this
    /// way. Resolved by `lower::module_mixins` against the same runtime
    /// seams as `initializer_filters`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initializer_response_headers: Vec<InitializerResponseHeader>,
    /// `ActionView::PartialRenderer.collection_cache = Store.reader` from
    /// a `config/initializers/` file (bare or inside `to_prepare`): the
    /// store cached collection renders read and write. campfire points
    /// it at its bounded `FragmentCache.store`. Recorded at ingest as the
    /// receiver constant and its argument-less class method (or
    /// `Rails.cache`), resolved by `lower::module_mixins`, written at the
    /// end of the ruby family's and spinel's `boot.rb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_cache_store: Option<CollectionCacheStore>,
    /// Standard libraries the app's own files require (`require
    /// "io/console"` at the top of a channel, `require "pty"` in a
    /// service), sorted and deduplicated. The emitted files do not carry
    /// their source's requires, so the ruby-family trees write these
    /// into `boot.rb`, which every program built from the tree loads:
    /// a production Rails app eager-loads every file at boot, so the
    /// libraries are loaded before any request either way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_requires: Vec<String>,
    /// SQL functions an initializer registers on every SQLite
    /// connection (`raw_connection.create_function("regexp", 2) do …`,
    /// `create_aggregate("stddev", 1) do step … finalize … end`).
    /// Lobsters' raw-SQL flag statistics call `stddev(…)`, so without
    /// them the query fails with "no such function". Each body is an
    /// ordinary method; installing them is per-lane runtime glue — see
    /// [`SqlFunction`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sql_functions: Vec<SqlFunction>,
    /// `config/database.yml`'s per-environment `pragmas:` for the
    /// primary database, as `PRAGMA name = value` statements the Db
    /// shims run on every connection after their own (see
    /// `ingest::database_yml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub database_pragmas: Vec<(String, Vec<String>)>,
    /// `config/database.yml`'s configurations, every environment's,
    /// resolved at build time ([`crate::database`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub databases: Vec<crate::database::Database>,
    /// Each model's database, read from source (`connects_to` through
    /// its ancestry). Empty when every model runs on the primary.
    #[serde(default, skip_serializing_if = "indexmap::IndexMap::is_empty")]
    pub model_databases: indexmap::IndexMap<ClassId, crate::database::ModelDatabase>,
    /// Database-identity facts the emitted runtime does not carry (a
    /// second database, unresolvable routing, and as warnings a
    /// configuration build-time resolution cannot read), reported by
    /// `analyze::database_identity`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub database_refusals: Vec<crate::database::DatabaseRefusal>,
    /// Support files (`lib/`, a gem's source, …) whose class ingest
    /// failed, each at the construct that failed it: the walk goes on
    /// without the file, and `analyze` reports it there
    /// (`error[ingest_unsupported]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ingest_drops: Vec<IngestDrop>,
    /// Each support file (`lib/`, a gem's source, …) with the classes and
    /// modules it declares outside a load-time conditional, by source path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_class_files: Vec<(String, Vec<ClassId>)>,
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
    /// graphql-ruby object types (`ingest::graphql_ruby`), for the
    /// analyzer only; their synthesized methods leave at lowering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphql_types: Vec<crate::dialect::GraphqlObjectType>,
    /// Signatures `ingest::graphql_ruby` declared in `rbs_signatures`
    /// (field arguments, input object readers), removed with the
    /// synthesized methods at lowering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphql_signatures: Vec<(ClassId, Symbol)>,
    /// Provenance for concern METHODS spliced into a controller:
    /// controller → (method name → the module it was cut from).
    /// `splice_concerns_into_controllers` copies a concern's methods
    /// into every includer; the copy is then typed against THAT
    /// controller's ivar environment, which is wrong when the concern
    /// is included high in the chain. campfire puts
    /// `include TrackedRoomVisit` on ApplicationController, whose
    /// `remember_last_room_visited` reads `@room` — a ivar
    /// ApplicationController never sets, because the concern's method
    /// runs as a before_action on the Rooms controllers below it. The
    /// honest seed is the union across includers, which
    /// `concern_ivar_env` already computes; this map is what lets the
    /// seeding site find it. Filters carry the same fact in
    /// `Filter::from_concern`; methods had nowhere to put it.
    ///
    /// Analysis-only, so it lives beside the app rather than on
    /// `Action`: no emitter needs it (the splice is already a verbatim
    /// copy by the time one runs), and a field on `Action` would touch
    /// every construction site including a dozen in tests.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_spliced_actions: HashMap<ClassId, HashMap<Symbol, ClassId>>,
    /// Templates no action, route or render reaches
    /// (`ingest::unreachable_views`): kept and emitted, but their errors
    /// are reported as warnings, since Rails never runs them.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unreachable_views: BTreeSet<Symbol>,
    /// The `app/helpers` modules' diagnostics, taken before the
    /// post-analyze lowerings rewrite their bodies; `None` until then,
    /// and on a path that never lowers (`check`), where they are
    /// computed from the bodies as they stand.
    #[serde(skip)]
    pub helper_diagnostics: Option<Vec<crate::diagnostic::Diagnostic>>,
    /// Every `app/helpers/**/*_helper.rb` module, including one whose
    /// method names a later helper all redefines (and so owns in
    /// `helper_method_index`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub helper_modules: BTreeSet<ClassId>,
    /// The class-side twin of `concern_spliced_actions`: concern methods
    /// `splice_concern_class_methods_into_includers` copied onto a model
    /// or library class, keyed includer → method → the source module.
    ///
    /// The module keeps its own copy of that `def` (dead there, since
    /// `include` never carries a singleton method), so ONE method now
    /// has two `MethodDef`s in two classes. Call sites are recorded
    /// against the receiver — `User.create_bot!` — and without this map
    /// nothing connects them to the module's copy, which is the one the
    /// sidecar beside `app/models/user/bot.rb` is written from. The two
    /// descriptions then disagree about the parameter types.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_spliced_class_methods: HashMap<ClassId, HashMap<Symbol, ClassId>>,
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
    /// Items an `included do` block declares behind a guard on the
    /// includer (`has_many :x if self == Account`, `unless in?([A, B])`),
    /// keyed by module. `included do` runs with `self` bound to each
    /// includer, so the guard is answered per includer when the module
    /// is spliced: the item goes to the includers it admits and no
    /// other. Kept apart from `concern_model_items`, which every
    /// includer receives; each item records where it stood among those,
    /// so the splice keeps source order.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_guarded_items: HashMap<ClassId, Vec<ConcernGuardedItem>>,
    /// Instance-method names a concern's `included do` block (re)defines
    /// (`def x`, `alias_method :x, ...`, `define_method`, `attr_*`), keyed by
    /// module. Those statements are not carried as `concern_model_items`, so
    /// this is the only record that an includer's reader `x` is replaced.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub concern_included_redefs: HashMap<ClassId, Vec<Symbol>>,
    /// `validates_with` calls an STI subclass declares, moved onto its
    /// model ancestor's body (`ingest::sti_validations`): each call's
    /// position and the subclass, whose `is_a?` guards the `validate` the
    /// call expands into. A list, not a map keyed by the position: JSON
    /// takes only string keys, and the ingested App round-trips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validates_with_guards: Vec<ValidatesWithGuard>,
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
    /// Rails' ParamsWrapper default for every controller: wrap a JSON
    /// body under the controller's model name. On when
    /// `config.load_defaults` is 7.0 or later (Rails sets
    /// `action_controller.wrap_parameters_by_default`), unless that key
    /// is set explicitly, or when an initializer calls
    /// `wrap_parameters format: [:json]` (the pre-7.0 generator).
    /// Read by `lower::controller_to_library::params_wrapper`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap_parameters_by_default: bool,
    /// The target's runtime defines `ActiveModel::Validator`
    /// (`runtime/ruby/active_model_validator.rb`, the ruby family), so a
    /// `validates_with` class keeps its base instead of being dropped as
    /// an unported Rails subclass. Set by `session::analyze_and_lower_for`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ports_active_model_validator: bool,
    /// The `validates` calls whose custom keys each name an app
    /// `EachValidator` (`ingest::each_validator`), which each target
    /// expands or reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub each_validator_calls: Vec<EachValidatorCall>,
    /// Rubydex answers for `sources`, resolved while ingest finished.
    /// The analyzer resolves the sources itself when this is absent.
    #[serde(skip)]
    pub const_resolver: crate::analyze::PreparedConstResolver,
    /// Ingested source apps cannot enter the standalone IR-only analyzer mode
    /// if a caller drops their source table. Persisted across serialization.
    #[serde(default)]
    pub source_index_required: bool,
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
    /// App-layer roots ingest walked, relative to `root`: `["app"]` for
    /// an ordinary Rails app, `["app", "packs/blog/app", …]` for a
    /// Packwerk app whose packages carry their own `app/` tree,
    /// `["app", "lib/billing/app"]` for one with an in-repo engine
    /// (`ingest::app::app_roots`). `app` is always first; the rest are
    /// sorted. Exists so a consumer (today, `check`'s summary line) can
    /// report what got walked without recomputing it from the VFS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_roots: Vec<String>,
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

/// One `X.prepend Y` / `X.include Y` an initializer registers.
///
/// The receiver is stored as WRITTEN (`Turbo::StreamsChannel`), not
/// resolved: a mixin onto a constant this tree does not define is a gap
/// to report, and reporting it needs the app's own spelling.
/// One SQL function an initializer registers with SQLite.
///
/// The block bodies become methods of the `SqlFunctions` module, each
/// taking the SQLite context as its first parameter under the block's
/// own name (`fn`), so a body reads exactly as the app wrote it:
/// `fn.result = …` sets the value, and an aggregate keeps its running
/// state in `fn[:key]`. A `next` that left the block early is a
/// `return` in the method. The context object is the lane's: the
/// sqlite3 gem's function proxy on CRuby.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlFunction {
    /// The SQL-visible name (`"stddev"`).
    pub name: String,
    /// Argument count SQLite dispatches on.
    pub arity: usize,
    pub kind: SqlFunctionKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SqlFunctionKind {
    /// `create_function` — one method, called per row.
    Scalar { method: crate::dialect::MethodDef },
    /// `create_aggregate` — `step` per row, `finalize` once.
    Aggregate { step: crate::dialect::MethodDef, finalize: crate::dialect::MethodDef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleMixin {
    /// The constant receiving the module, as the initializer spells it.
    pub target: Symbol,
    /// The module being mixed in.
    pub module: Symbol,
    pub kind: MixinKind,
}

/// `ActionView::PartialRenderer.collection_cache = <receiver>.<reader>`:
/// a constant and an argument-less class method on it, as the
/// initializer spells them (`FragmentCache` / `store`, `Rails` / `cache`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollectionCacheStore {
    pub receiver: Symbol,
    pub reader: Symbol,
}

/// `Rack::MiniProfiler.config.position = "bottom-left"`, kept as written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GemPackageSetting {
    pub gem: String,
    pub setter: String,
    pub statement: String,
    pub span: crate::span::Span,
}

/// `X.before_action :m, only: [:a, …]` from a `config/initializers/`
/// file. `only` empty means every action, as Rails reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InitializerFilter {
    /// The controller receiving the filter, as the initializer spells it.
    pub target: Symbol,
    /// The filter method, which a mixin onto the same target supplies.
    pub method: Symbol,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<Symbol>,
}

/// One header an initializer's `after_action` writes on a framework
/// controller's response. `only` empty means every action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InitializerResponseHeader {
    pub target: Symbol,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<Symbol>,
    pub name: String,
    pub value: String,
}

/// `prepend` inserts AHEAD of the target in the lookup chain, `include`
/// behind it. The distinction is the whole point of campfire's
/// `RoomStreamsAreAuthorized`, whose `subscribed` calls `super` — as an
/// `include` it would never run, because the class defines its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixinKind {
    Prepend,
    Include,
}

impl MixinKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MixinKind::Prepend => "prepend",
            MixinKind::Include => "include",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct InertiaConfig {
    pub version: String,
    pub encrypt_history: bool,
    pub always_include_errors_hash: bool,
    pub use_script_element_for_initial_page: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ViteManifest {
    pub public_dir: String,
    pub digest: String,
    pub chunks: std::collections::BTreeMap<String, ViteChunk>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ViteChunk {
    pub file: String,
    pub css: Vec<String>,
    pub imports: Vec<String>,
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
    /// Module → the one non-module class that includes it, directly or
    /// through another module; absent when none or several do. A
    /// concern's methods run on its includer, and with exactly one
    /// includer that class is what `self` means in them — the analyzer
    /// types the bodies against it and `lower::class_body_new` binds a
    /// bare `new` to it. Read from the same `include` lines both the
    /// registry and the emit are built from: a model's `include X`
    /// body items, a library class's `includes`.
    pub fn sole_includer_of_modules(
        &self,
    ) -> crate::det::HashMap<crate::ident::ClassId, crate::ident::ClassId> {
        use std::collections::{BTreeMap, BTreeSet};
        use crate::det::HashMap;
        let modules: BTreeSet<&crate::ident::ClassId> = self
            .library_classes
            .iter()
            .filter(|lc| lc.is_module)
            .map(|lc| &lc.name)
            .collect();
        if modules.is_empty() {
            return HashMap::default();
        }
        // Every class's DIRECT includes, modules included (for the
        // transitive walk).
        let mut direct: BTreeMap<crate::ident::ClassId, Vec<crate::ident::ClassId>> = BTreeMap::new();
        for lc in &self.library_classes {
            direct.insert(lc.name.clone(), lc.includes.clone());
        }
        for model in &self.models {
            let mut ids = Vec::new();
            for item in &model.body {
                if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                    if let crate::expr::ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
                        if method.as_str() == "include" {
                            for arg in args {
                                if let crate::expr::ExprNode::Const { path } = &*arg.node {
                                    ids.push(crate::analyze::mixin_path_id(path));
                                }
                            }
                        }
                    }
                }
            }
            direct.entry(model.name.clone()).or_default().extend(ids);
        }
        // Controllers include concerns too, and a module shared between
        // a controller and a channel (campfire's
        // `Authentication::SessionLookup`) has TWO includers, not the
        // one the library classes alone would show.
        for controller in &self.controllers {
            let ids = crate::analyze::controller_includes(controller);
            direct.entry(controller.name.clone()).or_default().extend(ids);
        }
        let mut includers: HashMap<crate::ident::ClassId, BTreeSet<crate::ident::ClassId>> = HashMap::default();
        for (id, includes) in &direct {
            if modules.contains(id) || includes.is_empty() {
                continue;
            }
            let mut queue = includes.clone();
            let mut seen: BTreeSet<crate::ident::ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if modules.contains(&m) {
                    includers.entry(m.clone()).or_default().insert(id.clone());
                }
                if let Some(nested) = direct.get(&m) {
                    for n in nested {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
            }
        }
        includers
            .into_iter()
            .filter_map(|(m, set)| (set.len() == 1).then(|| (m, set.into_iter().next().unwrap())))
            .collect()
    }

    /// A module that no class includes and exactly one class `extend`s
    /// (ancestry's `extend Ancestry::MaterializedPath` in the expanded
    /// `has_ancestry`): its instance methods run with that class as
    /// `self`, so they are typed as its class methods.
    pub fn sole_extender_of_modules(
        &self,
    ) -> crate::det::HashMap<crate::ident::ClassId, crate::ident::ClassId> {
        use std::collections::BTreeSet;
        use crate::det::{HashMap, HashSet};
        let modules: HashSet<&crate::ident::ClassId> =
            self.library_classes.iter().filter(|lc| lc.is_module).map(|lc| &lc.name).collect();
        let mixed = |exprs: &mut dyn Iterator<Item = &crate::expr::Expr>, mixin: &str, out: &mut Vec<crate::ident::ClassId>| {
            for expr in exprs {
                let crate::expr::ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else { continue };
                if method.as_str() != mixin {
                    continue;
                }
                for arg in args {
                    if let crate::expr::ExprNode::Const { path } = &*arg.node {
                        out.push(crate::analyze::mixin_path_id(path));
                    }
                }
            }
        };
        let mut included: HashSet<crate::ident::ClassId> = HashSet::default();
        let mut extenders: HashMap<crate::ident::ClassId, BTreeSet<crate::ident::ClassId>> = HashMap::default();
        for model in &self.models {
            let mut items = model.body.iter().filter_map(|item| match item {
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => Some(expr),
                _ => None,
            });
            let mut includes = Vec::new();
            mixed(&mut items, "include", &mut includes);
            included.extend(includes);
            let mut items = model.body.iter().filter_map(|item| match item {
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => Some(expr),
                _ => None,
            });
            let mut extends = Vec::new();
            mixed(&mut items, "extend", &mut extends);
            for m in extends {
                extenders.entry(m).or_default().insert(model.name.clone());
            }
        }
        for lc in self.library_classes.iter().filter(|lc| !lc.is_module) {
            included.extend(lc.includes.iter().cloned());
            let mut extends = Vec::new();
            mixed(&mut lc.unknown_calls.iter(), "extend", &mut extends);
            for m in extends {
                extenders.entry(m).or_default().insert(lc.name.clone());
            }
        }
        for controller in &self.controllers {
            included.extend(crate::analyze::controller_includes(controller));
            let mut items = controller.body.iter().filter_map(|item| match item {
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => Some(expr),
                _ => None,
            });
            let mut extends = Vec::new();
            mixed(&mut items, "extend", &mut extends);
            for m in extends {
                extenders.entry(m).or_default().insert(controller.name.clone());
            }
        }
        // A module's own include counts only when something uses that module: ancestry's
        // MaterializedPath2 includes MaterializedPath, and no model takes MaterializedPath2
        let mut live: Vec<crate::ident::ClassId> = included.iter().chain(extenders.keys()).cloned().collect();
        let mut seen: HashSet<crate::ident::ClassId> = live.iter().cloned().collect();
        while let Some(id) = live.pop() {
            for lc in self.library_classes.iter().filter(|lc| lc.is_module && lc.name == id) {
                for nested in &lc.includes {
                    included.insert(nested.clone());
                    if seen.insert(nested.clone()) {
                        live.push(nested.clone());
                    }
                }
                let mut extends = Vec::new();
                mixed(&mut lc.unknown_calls.iter(), "extend", &mut extends);
                for m in extends {
                    extenders.entry(m.clone()).or_default().insert(lc.name.clone());
                    if seen.insert(m.clone()) {
                        live.push(m);
                    }
                }
            }
        }
        extenders
            .into_iter()
            .filter(|(m, set)| modules.contains(m) && !included.contains(m) && set.len() == 1)
            .map(|(m, set)| (m, set.into_iter().next().unwrap()))
            .collect()
    }
    /// The path a span's file was read from (`sources[n - 1]`), for an app
    /// whose ingest has finished; `None` for the synthetic `FileId(0)`.
    pub fn source_path(&self, file: crate::span::FileId) -> Option<&str> {
        (file.0 as usize).checked_sub(1).and_then(|i| self.sources.get(i)).map(|s| s.path.as_str())
    }

    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            schema: Schema::default(),
            binary_assets: Vec::new(),
            i18n: crate::i18n::Catalog::default(),
            i18n_table: Vec::new(),
            i18n_runtime_sites: Vec::new(),
            i18n_table_sites: Vec::new(),
            models: Vec::new(),
            library_classes: Vec::new(),
            native_library_keywords: false,
            file_programs: Vec::new(),
            boot_gem_entries: Vec::new(),
            controllers: Vec::new(),
            routes: RouteTable::default(),
            views: Vec::new(),
            test_modules: Vec::new(),
            fixtures: Vec::new(),
            seeds: None,
            importmap: None,
            inertia: None,
            vite_manifest: None,
            stylesheets: Vec::new(),
            rbs_signatures: HashMap::default(),
            rbs_includes: HashMap::default(),
            gem_lock: None,
            loaded_frameworks: None,
            assume_ssl_envs: Vec::new(),
            gem_boundary: Default::default(),
            extensions: Default::default(),
            spinel_int_overflow: None,
            content_helper_allowed_attributes: Vec::new(),
            content_helper_allowed_tags: Vec::new(),
            inferred_method_params: HashMap::default(),
            helper_method_index: HashMap::default(),
            view_visible_controller_methods: BTreeSet::new(),
            generated_helper_methods: BTreeMap::new(),
            global_id_locate_models: BTreeSet::new(),
            global_id_locate_signed_models: BTreeSet::new(),
            job_plans: Vec::new(),
            queue_adapter: None,
            sidekiq_redis: SidekiqRedisConfig::default(),
            sidekiq_worker: SidekiqWorkerDefaults::default(),
            cable_redis: None,
            attachable_unsigned_models: Vec::new(),
            pending_attachment_on_load: Vec::new(),
            load_hook_class_macros: Vec::new(),
            load_hook_extended_macros: Vec::new(),
            spliced_class_methods: Default::default(),
            partial_local_types: HashMap::default(),
            view_ivar_types: HashMap::default(),
            html_safe_methods: BTreeSet::new(),
            html_safe_method_owners: BTreeSet::new(),
            time_formats: BTreeMap::new(),
            module_mixins: Vec::new(),
            rake_tasks_source: None,
            rake_task_gaps: Vec::new(),
            initializer_filters: Vec::new(),
            initializer_defs: Vec::new(),
            gem_package_settings: Vec::new(),
            auto_source_gems: Vec::new(),
            initializer_response_headers: Vec::new(),
            collection_cache_store: None,
            app_requires: Vec::new(),
            sql_functions: Vec::new(),
            database_pragmas: Vec::new(),
            databases: Vec::new(),
            model_databases: indexmap::IndexMap::new(),
            database_refusals: Vec::new(),
            ingest_drops: Vec::new(),
            library_class_files: Vec::new(),
            rails_application: None,
            concern_filters: HashMap::default(),
            graphql_types: Vec::new(),
            graphql_signatures: Vec::new(),
            concern_spliced_actions: HashMap::default(),
            unreachable_views: BTreeSet::new(),
            helper_diagnostics: None,
            helper_modules: BTreeSet::new(),
            concern_spliced_class_methods: HashMap::default(),
            concern_model_items: HashMap::default(),
            concern_guarded_items: HashMap::default(),
            concern_included_redefs: HashMap::default(),
            validates_with_guards: Vec::new(),
            current_attribute_classes: Vec::new(),
            render_edges: HashMap::default(),
            view_feeders: HashMap::default(),
            controller_resolutions: HashMap::default(),
            sources: Vec::new(),
            wrap_parameters_by_default: false,
            ports_active_model_validator: false,
            each_validator_calls: Vec::new(),
            const_resolver: Default::default(),
            source_index_required: false,
            root: String::new(),
            app_roots: Vec::new(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod includer_guard_tests {
    use super::IncluderGuard;

    fn guard(classes: &[&str], negate: bool) -> IncluderGuard {
        IncluderGuard { classes: classes.iter().map(|c| c.to_string()).collect(), negate }
    }

    #[test]
    fn a_root_qualified_constant_admits_only_that_class() {
        let g = guard(&["::Article"], false);
        assert!(g.admits("Article"));
        assert!(!g.admits("Billing::Article"));
        let g = guard(&["::Billing::Article"], false);
        assert!(g.admits("Billing::Article"));
        assert!(!g.admits("Article"));
        assert!(!g.admits("Shop::Billing::Article"));
        // `unless self == ::Article` keeps the namespaced includer.
        assert!(guard(&["::Article"], true).admits("Billing::Article"));
    }

    #[test]
    fn an_unqualified_constant_matches_on_segment_boundaries() {
        let g = guard(&["Article"], false);
        assert!(g.admits("Article"));
        assert!(g.admits("Billing::Article"));
        assert!(!g.admits("BillingArticle"));
    }
}

/// `App::sidekiq_redis`: the `url:` and `password:` of the app's
/// `Sidekiq.configure_client` Redis settings, each `None` when the block
/// doesn't give it in a form ingest reads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidekiqRedisConfig {
    pub url: Option<RedisSetting>,
    pub password: Option<RedisSetting>,
}

impl SidekiqRedisConfig {
    pub fn is_empty(&self) -> bool {
        self.url.is_none() && self.password.is_none()
    }
}

/// `config/cable.yml`'s production `adapter: redis` settings: where
/// broadcasts are published, and the prefix Action Cable puts on each
/// channel (`""` for none).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CableRedisConfig {
    pub url: RedisSetting,
    pub channel_prefix: String,
}

/// `config/sidekiq.yml`'s literal worker settings: each `None` (or the
/// queues empty) when the file doesn't give it in a form ingest reads.
/// A queue's weight is 0 when it has none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidekiqWorkerDefaults {
    pub concurrency: Option<i64>,
    pub timeout: Option<i64>,
    pub queues: Vec<(String, i64)>,
    /// The retry limit for a job whose `retry` is `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<i64>,
    /// The dead set's trims: how many jobs it keeps, and for how long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_max_jobs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_timeout_in_seconds: Option<i64>,
    /// The poller's interval: a fixed average, or the per-process one
    /// that is scaled by the number of Sidekiq processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_average: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_scheduled_poll_interval: Option<f64>,
}

impl SidekiqWorkerDefaults {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A Redis setting's value, in the forms ingest reads: a String literal,
/// `ENV["NAME"]`, or `ENV.fetch("NAME")` with or without a String
/// default. The emitted runtime reads the variable when it first pushes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RedisSetting {
    Literal(String),
    Env(String),
    EnvFetch { name: String, default: Option<String> },
}

/// A `validates_with` an STI subclass declares, by its call's position
/// (file id and start offset), and that subclass (`App::validates_with_guards`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatesWithGuard {
    pub file: u32,
    pub start: u32,
    pub class: String,
}

/// A gem-source file's file-level program ([`App::file_programs`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileProgram {
    /// The source file, as ingested.
    pub source: String,
    pub items: Vec<ProgramItem>,
}

/// One top-level statement of a [`FileProgram`], in source order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProgramItem {
    /// A `class`/`module` the library-class walk carried (these names),
    /// loaded here so the statements around it run in order.
    Declared(Vec<crate::ident::ClassId>),
    /// A statement Ruby runs as the file loads.
    Statement(crate::expr::Expr),
    /// `require "feature"` / `require_relative "path"`, resolved against
    /// the tree when emitted.
    Require { span: crate::span::Span, feature: String, relative: bool },
    /// Modules the file opens that this file's emitted declarations do
    /// not (a namespace that holds only autoloads, or nested modules),
    /// each as its nesting is written: `(segment, is_class)` from the
    /// outermost (`module Ak; class Client; module Schema`). Opened here,
    /// so the constant exists as the source's lines make it.
    Opened(Vec<Vec<(String, bool)>>),
    /// `autoload :Name, "feature"` in `owner`'s body (`Object` at the
    /// top level): Ruby loads the file the first time `owner::Name` is
    /// read.
    Autoload { span: crate::span::Span, owner: String, name: String, feature: String },
    /// A file-level `if`/`unless` holding declarations: its branches'
    /// items run only where Ruby runs the branch (an `elsif` is a branch
    /// in the `else` items).
    Branch {
        condition: crate::expr::Expr,
        negated: bool,
        then_items: Vec<ProgramItem>,
        else_items: Vec<ProgramItem>,
    },
    /// A statement the ingest cannot carry, reported as
    /// `file_statement_unsupported` and named when the file loads.
    NotCarried { span: crate::span::Span, message: String },
}

/// A support file the class ingest left out (`App::ingest_drops`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestDrop {
    /// The construct that failed it; the file's start when no frame
    /// noted one.
    pub span: crate::span::Span,
    pub message: String,
}
