//! Initializer ledger: every boot statement no recognizer consumes.
//!
//! Rails apps put much of their boot behaviour in `config/initializers/*.rb`,
//! in `Rails::Engine` class bodies and in `config/application.rb`. Ingest
//! reads that code through a growing list of per-shape recognizers
//! (inflections, `session_store`, time formats, module mixins, filters, SQL
//! functions, `Kaminari.configure`, `config.x.*`, …), each baking one fact
//! and ignoring the rest of the file. Everything a recognizer does not read
//! produced neither an effect nor a diagnostic: a build with zero errors
//! could compile an app that silently omits a boot statement (EngineeredAt's
//! `config.events.after_item_created { ArticleIngestionService.call(…) }` is
//! the line that turns fetched feed items into articles).
//!
//! This pass makes those drops visible and changes nothing else. It runs
//! after the recognizers, reads the same sources and emits one WARNING
//! ([`DiagnosticKind::UnconsumedInitializer`]) per unconsumed statement,
//! carrying the statement's span. No `App` field is touched.
//!
//! # The universe of statements
//!
//! * **Host initializers**: every `config/initializers/**/*.rb` statement.
//!   `to_prepare` / `to_run` blocks are unwrapped exactly as
//!   [`initializer_statements`](super::app::initializer_statements) does for
//!   the recognizers, and `Rails.application.configure do … end` is
//!   descended into because the `config.x` lift descends into it; every
//!   other block (`ActiveSupport.on_load { … }`,
//!   `config.after_initialize { … }`, a callback body) is ONE statement.
//!   The exception is a `Const.configure` / `Const.setup` block no
//!   recognizer reads: the container is not listed, its statements are
//!   (one level deep), because the setting inside it is the unit a reader
//!   acts on (`config.events.after_item_created { … }` in SourceMonitor's
//!   initializer is one of them).
//! * **Host `config/application.rb`**: the statements of the
//!   `Rails::Application` subclass body that are `config.*` calls,
//!   assignments and op-assignments (including `config.to_prepare`,
//!   `config.after_initialize`), or a receiverless boot hook (`initializer`,
//!   `to_prepare`, `after_initialize`, `before_initialize`, …). Methods in
//!   that body are consumed (they become `Rails::Application` methods).
//! * **Engines**: for every in-tree PATH gem whose `lib/` declares a
//!   `Rails::Engine` (its `app/` is ingested as host code whether or not its
//!   mount was admitted): every `config/initializers/**/*.rb` statement, and
//!   every statement of the engine class body. No recognizer reads an engine
//!   initializer directory (a mount whose engine has one is refused), and the
//!   engine class is dropped, so only a literal `isolate_namespace` of an
//!   ADMITTED engine is consumed.
//!
//! Not statements: comments, `class` / `module` definitions (ingested as
//! library classes, or deliberately dropped by `keep_initializer_defined`),
//! and `require` / `require_relative` (load edges: the file they name is
//! ingested or refused by the tree walk, and `config/application.rb`'s are
//! read by `loaded_frameworks`).
//!
//! # What "consumed" means
//!
//! A statement is consumed when an existing recognizer reads it. The
//! recognizers are not re-implemented here: [`RECOGNIZERS`] runs the
//! recognizer's own code (or a predicate the recognizer itself calls) on the
//! statement's text, so the ledger follows the recognizer when it changes.
//! A statement is consumed if a recognizer reads ANY part of it; a block is
//! split only where this file says so above. The table at [`RECOGNIZERS`]
//! names each recognizer, where it applies, and at what nesting.
//!
//! # Inert allowlist
//!
//! [`INERT`] lists statements that provably have no effect on a compiled
//! app, one reason each. An entry matches only when every argument is a
//! plain literal, so a side effect cannot hide in the value. A constant
//! named on its own line (campfire's `ActiveStorage::Transformers::Vips`,
//! there to force an autoload) is inert too.
//!
//! # Who a statement belongs to
//!
//! Each finding names its [`Holder`], read from what the statement calls or
//! writes and the app's `Gemfile.lock`: Rails behaviour the running app
//! answers without (`config.middleware.use`), a Rails setting only the build,
//! the loader or the logs read (`config.assets`), a gem with its fate in
//! [`crate::gems`] (`Devise.setup`'s settings are devise's), the app's own
//! code (`DiscourseEvent.on`, an `after_initialize` block) or unknown. A
//! conditional takes the weightiest holder among its branches. The holder
//! changes no severity: it is what deciding one per holder will read.

use crate::det::HashSet;
use std::path::{Path, PathBuf};

use ruby_prism::Node;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ident::Symbol;
use crate::span::{SourceFile, Span};
use crate::vfs::Vfs;

use super::app::{
    available_locales_line, default_locale_line, declares_rails_engine, fallbacks_line, extract_autoload_lib_ignores,
    extract_autoload_path_roots, extract_config_assignments, extract_config_time_zone,
    extract_collection_cache_store, extract_filter_parameters, extract_initializer_filters, extract_initializer_response_headers,
    extract_module_mixins,
    extract_time_formats, extract_variable_content_type_exclusions,
    extract_video_preview_arguments, extract_video_previewer_replacement,
    extract_vips_loader_policy, forgery_strategy_line, kaminari_default_per_page,
    load_defaults_version, queue_adapter_line, raise_on_missing_line, read_rb_files, wrap_parameters_line,
    FilterParametersLine, InflectEffect, VideoPreviewArgsExtract, FRAMEWORK_CONFIG_KEYS,
};
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes_with_scope,
    flatten_statements,
};
use super::{prism, sources, survey};

use crate::ingest::prism_api::PrismLocation;
/// The file label every probe passes the recognizers: a `<…>` label is never
/// registered as a source (see `ingest::sources`), so asking a recognizer
/// about one statement cannot disturb the app's source table.
const PROBE: &str = "<initializer_ledger>";

/// Where a statement lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    HostInitializer,
    HostApplication,
    EngineInitializer,
    EngineClassBody,
}

/// How far down the statement sits, which decides which recognizers can see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    /// A direct statement of the file (or of the `Application` class body).
    Top,
    /// Inside a `to_prepare` / `to_run` block the recognizers unwrap.
    Hook,
    /// Inside a `Rails.application.configure do … end` block.
    Nested,
    /// Inside `inflections(:en) do |inflect| … end` of `inflections.rb`,
    /// which only the inflections reader sees.
    Inflections,
    /// Inside an unconsumed `Const.configure do |c| … end`: no recognizer
    /// reads a statement of such a block on its own, so each is listed.
    Block,
}

const ANY: &[Level] = &[Level::Top, Level::Hook, Level::Nested];
const TOP: &[Level] = &[Level::Top];
const TOP_HOOK: &[Level] = &[Level::Top, Level::Hook];
const TOP_NESTED: &[Level] = &[Level::Top, Level::Nested];

const INIT: &[Origin] = &[Origin::HostInitializer];
const APP: &[Origin] = &[Origin::HostApplication];
const INIT_APP: &[Origin] = &[Origin::HostInitializer, Origin::HostApplication];

/// What a recognizer is asked about one statement.
struct Probe<'a> {
    /// The statement, through the start of the next one (so a heredoc body
    /// stays attached and the slice parses on its own).
    text: &'a str,
    node: &'a Node<'a>,
    /// The whole file: `node`'s offsets index this, not `text`, which
    /// starts at the statement.
    src: &'a str,
    /// `config.autoload_lib(ignore: …)` directories, before any filtering.
    lib_ignores: &'a [String],
}

/// One existing recognizer, how to ask it about a single statement, and
/// where it reads. `reads` calls the recognizer's own code.
struct Recognizer {
    name: &'static str,
    origins: &'static [Origin],
    levels: &'static [Level],
    reads: fn(&Probe<'_>) -> bool,
}

/// Every recognizer that reads initializer or `config/application.rb`
/// statements, in `ingest_app_with_vfs` order. A recognizer missing from
/// this table makes its statements show up as unconsumed, which is the safe
/// direction; `tests/initializer_ledger.rs` pins each entry against the
/// effect it has on the `App`.
const RECOGNIZERS: &[Recognizer] = &[
    Recognizer {
        name: "session_store (config.session_store key:)",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| {
            super::session_config::reads(p.text.as_bytes())
        },
    },
    Recognizer {
        name: "time_formats (Time::DATE_FORMATS[:k] = / ActiveSupport::TimeFormats.register)",
        origins: INIT,
        levels: TOP,
        reads: |p| !extract_time_formats(p.text.as_bytes(), PROBE).is_empty(),
    },
    Recognizer {
        name: "module_mixins (X.prepend Y / X.include Y)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| !extract_module_mixins(p.text.as_bytes(), PROBE).is_empty(),
    },
    Recognizer {
        name: "mapper_extension_block_methods (ActionDispatch::Routing::Mapper.prepend M)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| mapper_extension(p),
    },
    Recognizer {
        name: "collection_cache (ActionView::PartialRenderer.collection_cache = Store.reader)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| extract_collection_cache_store(p.text.as_bytes(), PROBE).is_some(),
    },
    Recognizer {
        name: "view_paths (X.prepend_view_path / X.append_view_path / X.view_paths =, bare or in ActiveSupport.on_load)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| super::view_paths::reads_initializer_statement(p.text.as_bytes()),
    },
    Recognizer {
        name: "initializer_filters (X.before_action :m, only:)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| !extract_initializer_filters(p.text.as_bytes(), PROBE).is_empty(),
    },
    Recognizer {
        name: "initializer_response_headers (X.after_action { response.set_header(k, v) }, bare or in ActiveSupport.on_load)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| !extract_initializer_response_headers(p.text.as_bytes(), PROBE).is_empty(),
    },
    Recognizer {
        name: "sql_functions (create_function / create_aggregate)",
        origins: INIT,
        levels: ANY,
        reads: |p| {
            (p.text.contains("create_function") || p.text.contains("create_aggregate"))
                && !super::sql_functions::scan_sql_functions(p.text.as_bytes(), PROBE).is_empty()
        },
    },
    Recognizer {
        name: "gem_package_settings (<package config receiver>.<attr> = literal, replayed after the Spinel package loads)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| super::gem_package_settings::extract(p.text.as_bytes(), PROBE).iter().any(|s| s.carried()),
    },
    Recognizer {
        name: "default_per_page (Kaminari.configure { |c| c.default_per_page = })",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| kaminari_default_per_page(p.text.as_bytes(), PROBE).0,
    },
    Recognizer {
        name: "inertia (InertiaRails.configure)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| {
            let mut config = crate::app::InertiaConfig::default();
            super::inertia::extract_inertia_config(p.text.as_bytes(), PROBE, None, &mut config)
        },
    },
    Recognizer {
        name: "content_helper allow lists (ActionText::ContentHelper.allowed_attributes= / allowed_tags=)",
        origins: INIT,
        levels: TOP_HOOK,
        reads: |p| content_helper_assignment(p),
    },
    Recognizer {
        name: "config_assignments (config.<key> = / config.x.<…> = lifted to Rails.application readers)",
        origins: INIT,
        levels: TOP_NESTED,
        reads: |p| config_lift(p),
    },
    Recognizer {
        name: "config_assignments (config.<key> = in the Application class body)",
        origins: APP,
        levels: TOP,
        reads: |p| config_lift(p),
    },
    Recognizer {
        name: "filter_parameters (config.filter_parameters += / = a Symbol or String list)",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| {
            let lines = extract_filter_parameters(p.text.as_bytes());
            !lines.is_empty() && lines.iter().all(|l| matches!(l, FilterParametersLine::Literal { .. }))
        },
    },
    Recognizer {
        name: "active_storage variable_content_types (config.active_storage.variable_content_types -=)",
        origins: INIT,
        levels: ANY,
        reads: |p| !extract_variable_content_type_exclusions(p.text.as_bytes()).is_empty(),
    },
    Recognizer {
        name: "active_storage video_preview_arguments",
        origins: INIT,
        levels: ANY,
        reads: |p| {
            !matches!(
                extract_video_preview_arguments(p.text.as_bytes()),
                VideoPreviewArgsExtract::Absent
            )
        },
    },
    Recognizer {
        name: "active_storage previewers (config.active_storage.previewers = ….map)",
        origins: INIT,
        levels: ANY,
        reads: |p| extract_video_previewer_replacement(p.text.as_bytes()).is_some(),
    },
    Recognizer {
        name: "vips loader policy (Vips.block_untrusted / Vips.block)",
        origins: INIT,
        levels: ANY,
        reads: |p| extract_vips_loader_policy(p.text.as_bytes()) != (false, Vec::new()),
    },
    Recognizer {
        name: "wrap_parameters_by_default (action_controller.wrap_parameters_by_default / wrap_parameters format:)",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| wrap_parameters_line(l).is_some()),
    },
    Recognizer {
        name: "queue_adapter :sidekiq (config.active_job.queue_adapter =)",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| {
            code_lines(p.text).any(|l| queue_adapter_line(l).flatten().as_deref() == Some("sidekiq"))
        },
    },
    Recognizer {
        name: "forgery_protection_verification_strategy",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| forgery_strategy_line(l).is_some()),
    },
    Recognizer {
        name: "action_cable origins (config.action_cable.allowed_request_origins / allow_same_origin_as_host / disable_request_forgery_protection =)",
        origins: INIT_APP,
        levels: ANY,
        reads: |p| super::action_cable_config::assignment(p.node).is_some(),
    },
    Recognizer {
        name: "autoload ignore (initializer that explicitly requires an ignored lib/ directory)",
        origins: INIT,
        levels: TOP,
        reads: |p| explicit_lib_require(p),
    },
    Recognizer {
        name: "load_defaults (config.load_defaults N: wrap_parameters and forgery defaults)",
        origins: APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| load_defaults_version(l).is_some()),
    },
    Recognizer {
        name: "time_zone (config.time_zone =)",
        origins: APP,
        levels: ANY,
        reads: |p| extract_config_time_zone(p.text.as_bytes()).is_some(),
    },
    Recognizer {
        name: "autoload_lib ignores (config.autoload_lib(ignore: %w[…]))",
        origins: APP,
        levels: ANY,
        reads: |p| !extract_autoload_lib_ignores(p.text.as_bytes()).is_empty(),
    },
    Recognizer {
        name: "autoload roots (config.autoload_paths / eager_load_paths / autoload_once_paths)",
        origins: APP,
        levels: ANY,
        reads: |p| !extract_autoload_path_roots(p.text.as_bytes()).is_empty(),
    },
    Recognizer {
        name: "i18n default_locale (config.i18n.default_locale =)",
        origins: APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| default_locale_line(l).is_some()),
    },
    Recognizer {
        name: "i18n available_locales (config.i18n.available_locales =)",
        origins: APP,
        levels: ANY,
        // A computed list is not read (the locales come from the app's files), so it stays reported.
        reads: |p| code_lines(p.text).any(|l| matches!(available_locales_line(l), Some(Ok(_)))),
    },
    Recognizer {
        name: "i18n fallbacks (config.i18n.fallbacks =)",
        origins: APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| fallbacks_line(l, "").is_some()),
    },
    Recognizer {
        name: "i18n raise_on_missing_translations",
        origins: APP,
        levels: ANY,
        reads: |p| code_lines(p.text).any(|l| raise_on_missing_line(l).is_some()),
    },
    Recognizer {
        name: "oj (Oj.mimic_JSON / Oj.optimize_rails: ingest/oj reads the mode)",
        origins: INIT,
        levels: ANY,
        reads: |p| {
            code_lines(p.text).any(|l| {
                let l = l.trim_start_matches("::");
                l == "Oj.mimic_JSON" || l.starts_with("Oj.mimic_JSON(") || l == "Oj.optimize_rails"
            })
        },
    },
    Recognizer {
        name: "content_security_policy (config.content_security_policy do |policy|)",
        origins: INIT,
        levels: ANY,
        reads: |p| super::app::content_security_policy_method(p.text).is_some(),
    },
];

/// Names of every recognizer the ledger consults, for the docs and tests.
pub fn recognizer_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = RECOGNIZERS.iter().map(|r| r.name).collect();
    names.push("inflections (config/initializers/inflections.rb: inflect.irregular/uncountable/acronym/plural/singular)");
    names.push("isolate_namespace (literal, in the class body of an admitted engine)");
    names
}

/// Trimmed, non-comment lines: the unit the line-scanning recognizers read.
fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.starts_with('#'))
}

/// An initializer statement that loads an ignored `lib/` directory itself
/// (campfire's `%w[ rails_ext ].each { |d| Dir["#{Rails.root}/lib/#{d}/*"].each { require … } }`),
/// the shape `lib_dir_is_explicitly_required` vouches for.
///
/// The recognizer's own test is two substring searches over the statement
/// (`require` and the directory name), so any long configuration block that
/// happens to say "require" and "assets" counts as a load of `lib/assets`
/// (EngineeredAt's `SourceMonitor.configure`, `Devise.setup`). The ledger
/// applies what the test is MEANT to find: a `require`/`load` CALL in the
/// statement and the directory named as a whole path segment of a string
/// literal. A statement the recognizer vouches for by accident is therefore
/// still listed.
fn explicit_lib_require(p: &Probe<'_>) -> bool {
    struct Scan {
        requires: bool,
        literals: Vec<String>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Scan {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none()
                && matches!(
                    constant_id_str(&node.name()),
                    "require" | "require_relative" | "require_dependency" | "load"
                )
            {
                self.requires = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
        fn visit_string_node(&mut self, node: &ruby_prism::StringNode<'pr>) {
            self.literals.push(String::from_utf8_lossy(node.unescaped()).into_owned());
        }
        fn visit_symbol_node(&mut self, node: &ruby_prism::SymbolNode<'pr>) {
            self.literals.push(String::from_utf8_lossy(node.unescaped()).into_owned());
        }
    }
    let mut scan = Scan { requires: false, literals: Vec::new() };
    ruby_prism::Visit::visit(&mut scan, p.node);
    scan.requires
        && p.lib_ignores.iter().any(|dir| {
            scan.literals.iter().any(|l| l.split(|c: char| c == '/' || c.is_whitespace()).any(|seg| seg == dir))
        })
}

/// `config.<key> = …` the lift to `Rails.application.<key>` reads: not a
/// framework key, which the lift skips as railtie noise.
fn config_lift(p: &Probe<'_>) -> bool {
    extract_config_assignments(p.text.as_bytes(), PROBE)
        .iter()
        .any(|(segments, _)| !FRAMEWORK_CONFIG_KEYS.contains(&segments.join("_").as_str()))
}

/// `ActionDispatch::Routing::Mapper.prepend(M)`, the shape
/// `mapper_extension_block_methods` reads.
fn mapper_extension(p: &Probe<'_>) -> bool {
    let Some(call) = p.node.as_call_node() else { return false };
    if !matches!(constant_id_str(&call.name()), "prepend" | "include") {
        return false;
    }
    let Some(recv) = call.receiver() else { return false };
    let Some(target) = super::app::constant_text(&recv, p.src) else { return false };
    target.trim_start_matches("::") == "ActionDispatch::Routing::Mapper"
        && call.arguments().is_some_and(|a| {
            a.arguments().iter().any(|arg| super::app::constant_text(&arg, p.src).is_some())
        })
}

/// `ActionText::ContentHelper.allowed_attributes = …` / `allowed_tags = …`
/// with one argument, the shape `content_helper_allow_list_additions`
/// reads. Operands it cannot resolve are survey gaps of their own.
fn content_helper_assignment(p: &Probe<'_>) -> bool {
    let Some(call) = p.node.as_call_node() else { return false };
    if !matches!(constant_id_str(&call.name()), "allowed_attributes=" | "allowed_tags=") {
        return false;
    }
    let Some(recv) = call.receiver() else { return false };
    if super::app::constant_text(&recv, p.src).as_deref() != Some("ActionText::ContentHelper") {
        return false;
    }
    call.arguments().is_some_and(|a| a.arguments().iter().count() == 1)
}

const ASSET_VERSION_INERT: &str = "without an active Propshaft reader, the emitted app does not use this asset digest salt";

/// The inert allowlist: `(normalized target, reason)`. Add an entry only when you can say why the statement is
/// inert for the COMPILED app: ingest never reads it, and the thing it
/// configures does not exist in the emitted program.
pub const INERT: &[(&str, &str)] = &[
    ("config.action_dispatch.cookies_serializer", "selects how Rails encodes cookie payloads on the wire (:json / :marshal); the emitted runtime owns one session codec and never reads the setting"),
    ("config.assets.version", ASSET_VERSION_INERT),
    ("config.generators", "consulted only by `rails generate`, which is not part of a compiled app (a block here only calls `g.<option>`)"),
    ("config.enable_reloading", "code reloading; the compiled app has no autoloader or reloader, every class is resolved at compile time"),
    ("config.cache_classes", "the pre-7.1 spelling of `enable_reloading = false`; same reason"),
    ("config.reload_classes_only_on_change", "tunes the code reloader's file watcher; no reloader exists in the compiled app"),
    ("config.eager_load", "whether Rails loads every class at boot; the compiled app has them all loaded by construction"),
    ("config.add_autoload_paths_to_load_path", "only adds autoload roots to $LOAD_PATH; the compiled app resolves constants at compile time and has no load path to extend"),
];

/// The entries whose statement may carry a block.
const INERT_BLOCK: &[&str] = &["config.generators"];

/// The reason for the allowlist entry covering `target` (whole, or as a
/// dotted prefix: `config.generators` also covers `config.generators.orm`).
fn inert_entry(target: &str, has_block: bool) -> Option<&'static str> {
    INERT.iter().find_map(|(t, reason)| {
        let covers = target == *t || target.strip_prefix(t).is_some_and(|rest| rest.starts_with('.'));
        let block_ok = !has_block || INERT_BLOCK.contains(t);
        (covers && block_ok).then_some(*reason)
    })
}

/// Normalized text of a receiver chain: no whitespace, no leading `::`,
/// `Rails.application.config` shortened to `config`.
fn normalize_target(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text.trim_start_matches("::");
    match text.strip_prefix("Rails.application.config") {
        Some(rest) => format!("config{rest}"),
        None => text.to_string(),
    }
}

/// A literal the allowlist may carry: no call, no interpolation, so no side
/// effect can ride along in the value.
fn is_plain_literal(node: &Node<'_>) -> bool {
    if node.as_symbol_node().is_some()
        || node.as_string_node().is_some()
        || node.as_integer_node().is_some()
        || node.as_float_node().is_some()
        || node.as_true_node().is_some()
        || node.as_false_node().is_some()
        || node.as_nil_node().is_some()
        || node.as_regular_expression_node().is_some()
        || node.as_constant_read_node().is_some()
        || node.as_constant_path_node().is_some()
    {
        return true;
    }
    if let Some(array) = node.as_array_node() {
        return array.elements().iter().all(|e| is_plain_literal(&e));
    }
    if let Some(parens) = node.as_parentheses_node() {
        return parens.body().is_none_or(|b| {
            flatten_statements(b).iter().all(is_plain_literal)
        });
    }
    if let Some(hash) = node.as_hash_node() {
        return hash.elements().iter().all(|e| assoc_is_plain(&e));
    }
    if let Some(hash) = node.as_keyword_hash_node() {
        return hash.elements().iter().all(|e| assoc_is_plain(&e));
    }
    false
}

fn assoc_is_plain(node: &Node<'_>) -> bool {
    node.as_assoc_node()
        .is_some_and(|a| is_plain_literal(&a.key()) && is_plain_literal(&a.value()))
}

const BARE_CONSTANT_INERT: &str = "a constant named on its own line only makes Rails autoload it; the compiled app has every class loaded before boot";

/// The inert-allowlist reason for `node`, if its target is listed and its
/// value is a plain literal, or if it is a bare constant.
fn inert_reason(node: &Node<'_>, src: &str) -> Option<&'static str> {
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() {
        return Some(BARE_CONSTANT_INERT);
    }
    let recv_text = |recv: Option<Node<'_>>| -> Option<String> {
        let recv = recv?;
        let loc = recv.location();
        Some(normalize_target(&src[loc.start_offset()..loc.end_offset()]))
    };
    let (target, values_plain, has_block) = if let Some(call) = node.as_call_node() {
        let name = constant_id_str(&call.name());
        let recv = recv_text(call.receiver())?;
        let args_plain = call
            .arguments()
            .is_none_or(|a| a.arguments().iter().all(|arg| is_plain_literal(&arg)));
        let has_block = call.block().is_some();
        match name.strip_suffix('=') {
            Some(base) if !base.is_empty() && base.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                (format!("{recv}.{base}"), args_plain, has_block)
            }
            // `config.assets.paths << x` writes through the reader.
            _ if name == "<<" => (recv, args_plain, has_block),
            _ => (format!("{recv}.{name}"), args_plain, has_block),
        }
    } else if let Some(write) = node.as_call_operator_write_node() {
        let recv = recv_text(write.receiver())?;
        (
            format!("{recv}.{}", constant_id_str(&write.read_name())),
            is_plain_literal(&write.value()),
            false,
        )
    } else if let Some(write) = node.as_call_or_write_node() {
        let recv = recv_text(write.receiver())?;
        (
            format!("{recv}.{}", constant_id_str(&write.read_name())),
            is_plain_literal(&write.value()),
            false,
        )
    } else {
        return None;
    };
    if !values_plain {
        return None;
    }
    inert_entry(&target, has_block)
}

/// What the ledger concluded about one statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not a boot statement (`require`, a definition).
    NotAStatement,
    /// An existing recognizer reads it.
    Consumed(&'static str),
    /// On the inert allowlist.
    Inert(&'static str),
    /// Nothing reads it.
    Unconsumed,
}

/// One statement as it moves through the ledger.
struct Stmt<'a> {
    node: Node<'a>,
    level: Level,
    /// The `Const.configure` a [`Level::Block`] statement sits in.
    within: Option<String>,
    /// Through the start of the next statement (or the end of the container).
    text: &'a str,
}

/// `X.to_prepare { … }` / `X.to_run { … }` with a statement body: the
/// wrapper `initializer_statements` unwraps.
fn hook_body<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let call = node.as_call_node()?;
    if !matches!(constant_id_str(&call.name()), "to_prepare" | "to_run") {
        return None;
    }
    call.block()?.as_block_node()?.body()?.as_statements_node().map(|s| s.as_node())
}

/// `Rails.application.configure do … end` (or a bare `configure`), the
/// container the `config.x` lift descends into.
fn configure_body<'a>(node: &Node<'a>, src: &str) -> Option<Node<'a>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != "configure" {
        return None;
    }
    if let Some(recv) = call.receiver() {
        let loc = recv.location();
        if normalize_target(&src[loc.start_offset()..loc.end_offset()]) != "Rails.application" {
            return None;
        }
    }
    call.block()?.as_block_node()?.body()
}

/// Flatten a statement list into `out`, unwrapping the containers the host
/// recognizers see through.
fn gather<'a>(
    stmts: Vec<Node<'a>>,
    limit: usize,
    src: &'a str,
    level: Level,
    unwrap: bool,
    out: &mut Vec<Stmt<'a>>,
) {
    let starts: Vec<usize> = stmts.iter().map(|n| n.location().start_offset()).collect();
    for (i, node) in stmts.into_iter().enumerate() {
        let own_end = node.location().end_offset();
        let end = starts.get(i + 1).copied().unwrap_or(limit).max(own_end).min(src.len());
        if unwrap && level == Level::Top {
            if let Some(body) = hook_body(&node) {
                let inner_limit = body.location().end_offset();
                gather(flatten_statements(body), inner_limit, src, Level::Hook, false, out);
                continue;
            }
            if let Some(body) = configure_body(&node, src) {
                let inner_limit = body.location().end_offset();
                gather(flatten_statements(body), inner_limit, src, Level::Nested, false, out);
                continue;
            }
        }
        let start = node.location().start_offset();
        out.push(Stmt { node, level, within: None, text: &src[start..end] });
    }
}

/// Is `node` a load edge or a definition rather than a boot statement?
fn is_not_a_statement(node: &Node<'_>) -> bool {
    if node.as_class_node().is_some() || node.as_module_node().is_some() {
        return true;
    }
    node.as_call_node().is_some_and(|c| {
        c.receiver().is_none() && matches!(constant_id_str(&c.name()), "require" | "require_relative")
    })
}

/// Judge one host statement against the recognizer table.
fn judge(origin: Origin, st: &Stmt<'_>, lib_ignores: &[String], src: &str) -> Verdict {
    if is_not_a_statement(&st.node) {
        return Verdict::NotAStatement;
    }
    let probe = Probe {
        text: st.text,
        node: &st.node,
        src,
        lib_ignores,
    };
    if st.level == Level::Inflections {
        // Re-ingesting the statement goes through `ingest_expr`, which in
        // survey mode records an unsupported construct as a gap; inflections
        // ingest already recorded it against the real file.
        let (effect, _) = prism::scope(|| survey::without_recording(|| inflections_effect(&probe)));
        match effect {
            Some(InflectEffect::Carried) => return Verdict::Consumed("inflections"),
            Some(InflectEffect::RegexRule) => {
                return Verdict::Consumed("inflections (regex rule, reported as its own survey gap)")
            }
            _ => {}
        }
    }
    // A survey-recording recognizer must not record twice, and a probe on
    // an isolated statement must not report a syntax error of its own.
    let (hit, _) = prism::scope(|| {
        survey::without_recording(|| {
            RECOGNIZERS
                .iter()
                .find(|r| r.origins.contains(&origin) && r.levels.contains(&st.level) && (r.reads)(&probe))
                .map(|r| r.name)
        })
    });
    if let Some(name) = hit {
        return Verdict::Consumed(name);
    }
    if let Some(reason) = inert_reason(&st.node, src) {
        return Verdict::Inert(reason);
    }
    Verdict::Unconsumed
}

/// `inflections.rb`: the `inflect.<m>` calls inside the statement, read by
/// the very function `ingest_inflections` applies. `Carried` wins over a
/// regex rule over an ignored call.
fn inflections_effect(p: &Probe<'_>) -> Option<InflectEffect> {
    use crate::expr::Expr;
    // The statement is a body of `inflections(:en) do |inflect| … end`
    // (descended into by the caller): rebind `inflect` as ingest sees it.
    let wrapped = format!("proc {{ |inflect|\n{}\n}}", p.text);
    let (program, _) = prism::scope(|| super::expr::ingest_ruby_program(&wrapped, PROBE));
    let program = program.ok()?;
    let mut sink = crate::naming::AppInflections::default();
    let mut best: Option<InflectEffect> = None;
    fn rank(e: InflectEffect) -> u8 {
        match e {
            InflectEffect::Carried => 3,
            InflectEffect::RegexRule => 2,
            InflectEffect::Ignored => 1,
            InflectEffect::NotInflect => 0,
        }
    }
    fn walk(e: &Expr, sink: &mut crate::naming::AppInflections, best: &mut Option<InflectEffect>) {
        let effect = super::app::apply_inflect_send(e, sink);
        if best.is_none_or(|b| rank(effect) > rank(b)) {
            *best = Some(effect);
        }
        e.node.for_each_child(&mut |c| walk(c, sink, best));
    }
    walk(&program, &mut sink, &mut best);
    best
}

/// One unconsumed statement, before it becomes a diagnostic.
pub struct Finding {
    pub owner: String,
    pub statement: String,
    /// What the statement writes to or calls, by syntax ([`targets_of`]);
    /// [`holder_of`] reads who owns it from these.
    pub targets: Vec<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub start: u32,
    pub end: u32,
}

/// The ledger of one source file: findings plus the verdict of every
/// statement it judged (for the tests and the report).
pub struct FileLedger {
    pub findings: Vec<Finding>,
    pub verdicts: Vec<(String, Verdict)>,
}

fn owner_text(origin: Origin, level: Level, name: &str, within: Option<&str>) -> String {
    let base = match origin {
        Origin::HostInitializer => "host initializer".to_string(),
        Origin::HostApplication => "host config/application.rb".to_string(),
        Origin::EngineInitializer => format!("engine {name} initializer"),
        Origin::EngineClassBody => format!("engine {name} class body"),
    };
    match level {
        Level::Top => base,
        Level::Hook => format!("{base}, inside to_prepare"),
        Level::Nested => format!("{base}, inside configure"),
        Level::Inflections => format!("{base}, inside inflections"),
        Level::Block => format!("{base}, inside {}", within.unwrap_or("a configure block")),
    }
}

/// A statement's head: `Receiver.method`, `NAME =`, `initializer "name"`,
/// or its first line when it is none of those. One line, bounded.
fn head_of(node: &Node<'_>, src: &str) -> String {
    let text_of = |n: &Node<'_>| {
        let loc = n.location();
        src[loc.start_offset()..loc.end_offset()].split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let head = if let Some(call) = node.as_call_node() {
        let name = constant_id_str(&call.name());
        let mut head = match call.receiver() {
            Some(recv) if name == "[]=" => format!("{}[] =", text_of(&recv)),
            Some(recv) => match name.strip_suffix('=') {
                Some(base) if !base.is_empty() && base.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                    format!("{}.{base} =", text_of(&recv))
                }
                _ => format!("{}.{name}", text_of(&recv)),
            },
            None => name.to_string(),
        };
        if call.receiver().is_none() {
            if let Some(first) = call.arguments().and_then(|a| a.arguments().iter().next()) {
                if first.as_string_node().is_some() || first.as_symbol_node().is_some() {
                    head = format!("{head} {}", text_of(&first));
                }
            }
        }
        head
    } else if let Some(write) = node.as_call_operator_write_node() {
        let recv = write.receiver().map(|r| text_of(&r)).unwrap_or_default();
        format!("{recv}.{} {}=", constant_id_str(&write.read_name()), constant_id_str(&write.binary_operator()))
    } else if let Some(write) = node.as_call_or_write_node() {
        let recv = write.receiver().map(|r| text_of(&r)).unwrap_or_default();
        format!("{recv}.{} ||=", constant_id_str(&write.read_name()))
    } else if let Some(write) = node.as_constant_write_node() {
        format!("{} =", constant_id_str(&write.name()))
    } else if let Some(def) = node.as_def_node() {
        match def.receiver() {
            Some(recv) => format!("def {}.{}", text_of(&recv), constant_id_str(&def.name())),
            None => format!("def {}", constant_id_str(&def.name())),
        }
    } else {
        let loc = node.location();
        let raw = &src[loc.start_offset()..loc.end_offset()];
        raw.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ")
    };
    head.chars().take(80).collect()
}

/// Judge every statement of `statements` and return the ledger.
fn judge_all(
    origin: Origin,
    owner_name: &str,
    path: &str,
    src: &str,
    statements: Vec<Stmt<'_>>,
    lib_ignores: &[String],
    admitted_isolate: bool,
) -> FileLedger {
    let file = SourceFile { path: path.to_string(), text: src.to_string() };
    let mut ledger = FileLedger { findings: Vec::new(), verdicts: Vec::new() };
    let mut pending: std::collections::VecDeque<Stmt<'_>> = statements.into();
    while let Some(st) = pending.pop_front() {
        let head = head_of(&st.node, src);
        let verdict = if origin == Origin::EngineInitializer || origin == Origin::EngineClassBody {
            if is_not_a_statement(&st.node) {
                Verdict::NotAStatement
            } else if admitted_isolate && is_literal_isolate_namespace(&st.node) {
                Verdict::Consumed("isolate_namespace (literal, admitted engine)")
            } else if let Some(reason) = inert_reason(&st.node, src) {
                Verdict::Inert(reason)
            } else {
                Verdict::Unconsumed
            }
        } else {
            judge(origin, &st, lib_ignores, src)
        };
        if verdict == Verdict::Unconsumed {
            // `Const.configure do |c| … end` is a container: nothing reads
            // it, and nothing reads the statements inside it either, so the
            // ledger names those (EngineeredAt's `config.events.
            // after_item_created { … }` is one of them). One level only: a
            // callback body (`do |event| … end`) is one registered handler,
            // not a list of settings.
            if st.level != Level::Block {
                if let Some(body) = config_block_body(&st.node) {
                    let mut inner = Vec::new();
                    let limit = body.location().end_offset();
                    gather(flatten_statements(body), limit, src, Level::Block, false, &mut inner);
                    if !inner.is_empty() {
                        for stmt in &mut inner {
                            stmt.within = Some(head.clone());
                        }
                        for stmt in inner.into_iter().rev() {
                            pending.push_front(stmt);
                        }
                        ledger.verdicts.push((head, Verdict::Unconsumed));
                        continue;
                    }
                }
            }
            let loc = st.node.location();
            let (line, column) = file.line_col(loc.start_offset() as u32);
            ledger.findings.push(Finding {
                owner: owner_text(origin, st.level, owner_name, st.within.as_deref()),
                statement: head.clone(),
                targets: match &st.within {
                    Some(within) => vec![format!("block:{within}")],
                    None => targets_of(&st.node, src),
                },
                path: path.to_string(),
                line,
                column,
                start: loc.start_offset() as u32,
                end: loc.end_offset() as u32,
            });
        }
        ledger.verdicts.push((head, verdict));
    }
    ledger
}

/// The body of `Const.configure do … end` / `Const.setup do … end`: a
/// constant receiver and a block with a statement body.
fn config_block_body<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let call = node.as_call_node()?;
    if !matches!(constant_id_str(&call.name()), "configure" | "setup") {
        return None;
    }
    let recv = call.receiver()?;
    if recv.as_constant_read_node().is_none() && recv.as_constant_path_node().is_none() {
        return None;
    }
    let body = call.block()?.as_block_node()?.body()?;
    body.as_statements_node().map(|s| s.as_node())
}

/// `isolate_namespace Const`, the one engine class-body statement the
/// admission check reads (and only when it is literal).
fn is_literal_isolate_namespace(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else { return false };
    if call.receiver().is_some() || constant_id_str(&call.name()) != "isolate_namespace" {
        return false;
    }
    call.arguments().is_some_and(|a| {
        let args: Vec<_> = a.arguments().iter().collect();
        args.len() == 1 && constant_path_of(&args[0]).is_some()
    })
}

/// A readable path for messages: relative to the app root when possible.
fn display_path(path: &Path) -> String {
    sources::relative_path(&path.display().to_string())
}

/// Statements of a host initializer or engine initializer file.
fn initializer_ledger(
    origin: Origin,
    owner_name: &str,
    path: &Path,
    bytes: &[u8],
    lib_ignores: &[String],
) -> FileLedger {
    let src = String::from_utf8_lossy(bytes).into_owned();
    let parsed = prism::parse_silent(bytes);
    let root = parsed.node();
    let mut stmts = Vec::new();
    if let Some(program) = root.as_program_node() {
        let list: Vec<Node<'_>> = program.statements().body().iter().collect();
        gather(list, src.len(), &src, Level::Top, origin == Origin::HostInitializer, &mut stmts);
    }
    // `inflections.rb`: descend into `inflections(:en) do |inflect| … end`,
    // whose `inflect.<m>` lines are the statements the recognizer reads.
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if origin == Origin::HostInitializer && file_name == "inflections.rb" {
        let mut expanded = Vec::new();
        for st in stmts {
            let block_body = st
                .node
                .as_call_node()
                .filter(|c| constant_id_str(&c.name()) == "inflections")
                .and_then(|c| c.block())
                .and_then(|b| b.as_block_node())
                .and_then(|b| b.body());
            match block_body {
                Some(body) => {
                    let limit = body.location().end_offset();
                    gather(flatten_statements(body), limit, &src, Level::Inflections, false, &mut expanded);
                }
                None => expanded.push(st),
            }
        }
        stmts = expanded;
    }
    judge_all(origin, owner_name, &display_path(path), &src, stmts, lib_ignores, false)
}

/// Statements of the `Rails::Application` subclass body in `application.rb`
/// that are configuration or boot hooks.
fn application_ledger(path: &Path, bytes: &[u8], lib_ignores: &[String]) -> FileLedger {
    let src = String::from_utf8_lossy(bytes).into_owned();
    let parsed = prism::parse_silent(bytes);
    let root = parsed.node();
    let mut stmts = Vec::new();
    for (_, class) in find_all_classes_with_scope(&root) {
        let is_application = class
            .superclass()
            .and_then(|s| constant_path_of(&s))
            .is_some_and(|p| p.join("::").trim_start_matches("::") == "Rails::Application");
        if !is_application {
            continue;
        }
        let Some(body) = class.body() else { continue };
        let limit = body.location().end_offset();
        let list: Vec<Node<'_>> =
            flatten_statements(body).into_iter().filter(|n| is_config_statement(n)).collect();
        gather(list, limit, &src, Level::Top, false, &mut stmts);
    }
    judge_all(
        Origin::HostApplication,
        "",
        &display_path(path),
        &src,
        stmts,
        lib_ignores,
        false,
    )
}

/// Boot hooks a class body may call without a receiver.
const HOOKS: &[&str] = &[
    "initializer",
    "to_prepare",
    "to_run",
    "after_initialize",
    "before_initialize",
    "before_configuration",
    "console",
    "runner",
    "rake_tasks",
    "generators",
    "server",
];

/// Is the receiver chain of `node` rooted at a `config` reader
/// (`config`, `Rails.application.config`, `self.config`)?
fn config_rooted(mut recv: Option<Node<'_>>) -> bool {
    while let Some(node) = recv {
        if let Some(call) = node.as_call_node() {
            if constant_id_str(&call.name()) == "config" {
                return true;
            }
            recv = call.receiver();
        } else {
            return false;
        }
    }
    false
}

/// A `config.*` call or (op-)assignment, or a receiverless boot hook.
fn is_config_statement(node: &Node<'_>) -> bool {
    if let Some(call) = node.as_call_node() {
        let name = constant_id_str(&call.name());
        return match call.receiver() {
            Some(recv) => {
                // `config` itself as the receiver, or any chain below it.
                recv.as_call_node().is_some_and(|c| constant_id_str(&c.name()) == "config")
                    || config_rooted(Some(recv))
            }
            None => HOOKS.contains(&name),
        };
    }
    if let Some(write) = node.as_call_operator_write_node() {
        return config_rooted(write.receiver());
    }
    if let Some(write) = node.as_call_or_write_node() {
        return config_rooted(write.receiver());
    }
    if let Some(write) = node.as_call_and_write_node() {
        return config_rooted(write.receiver());
    }
    false
}

/// Statements of every `Rails::Engine` subclass body in `source`.
fn engine_class_ledgers(
    path: &Path,
    bytes: &[u8],
    admitted: &HashSet<String>,
    lib_ignores: &[String],
) -> Vec<FileLedger> {
    let src = String::from_utf8_lossy(bytes).into_owned();
    let parsed = super::prism::parse_silent(bytes);
    let root = parsed.node();
    let mut out = Vec::new();
    for (scope, class) in find_all_classes_with_scope(&root) {
        let is_engine = class
            .superclass()
            .and_then(|s| constant_path_of(&s))
            .is_some_and(|p| p.len() == 2 && p[0] == "Rails" && p[1] == "Engine");
        if !is_engine {
            continue;
        }
        let mut qualified = scope;
        qualified.extend(class_name_path(&class).unwrap_or_default());
        let name = qualified.join("::");
        let Some(body) = class.body() else { continue };
        let limit = body.location().end_offset();
        let mut stmts = Vec::new();
        gather(flatten_statements(body), limit, &src, Level::Top, false, &mut stmts);
        out.push(judge_all(
            Origin::EngineClassBody,
            &name,
            &display_path(path),
            &src,
            stmts,
            lib_ignores,
            admitted.contains(&name),
        ));
    }
    out
}

/// Rails' own `config.<key>` surface, beyond the railtie a gem adds.
const RAILS_CONFIG_KEYS: &[&str] = &[
    "action_cable", "action_controller", "action_dispatch", "action_mailbox", "action_mailer",
    "action_text", "action_view", "active_job", "active_model", "active_record", "active_storage",
    "active_support", "add_autoload_paths_to_load_path", "api_only", "assets", "assume_ssl",
    "autoload_lib", "autoload_once_paths", "autoload_paths", "beginning_of_week", "cache_classes",
    "cache_store", "colorize_logging", "consider_all_requests_local", "content_security_policy",
    "content_security_policy_nonce_directives", "content_security_policy_nonce_generator",
    "content_security_policy_report_only", "debug_exception_response_format", "eager_load",
    "eager_load_namespaces", "eager_load_paths", "enable_reloading", "encoding", "exceptions_app",
    "file_watcher", "filter_parameters", "force_ssl", "generators", "host_authorization", "hosts",
    "i18n", "importmap", "load_defaults", "log_formatter", "log_level", "log_tags", "logger",
    "middleware", "paths", "permissions_policy", "public_file_server", "railties_order",
    "relative_url_root", "reload_classes_only_on_change", "reloader", "require_master_key",
    "secret_key_base", "session_options", "session_store", "ssl_options", "time_zone", "web_console",
];

/// The Rails boot hooks: their block is the app's own code, run at boot.
const RAILS_BOOT_HOOKS: &[&str] = &[
    "after_initialize", "after_routes_loaded", "before_configuration", "before_eager_load",
    "before_initialize", "to_prepare", "reloader.to_prepare",
];

/// Rails settings and calls that only change how the app is built, loaded,
/// logged or developed: nothing the running app answers depends on them.
const RAILS_BOOT_ONLY: &[&str] = &[
    "config.active_record.cache_query_log_tags", "config.active_record.dump_schema_after_migration",
    "config.active_record.migration_error", "config.active_record.query_log_tags",
    "config.active_record.query_log_tags_enabled", "config.active_record.verbose_query_logs",
    "config.active_support.deprecation", "config.active_support.disallowed_deprecation",
    "config.active_support.disallowed_deprecation_warnings", "config.active_support.report_deprecations",
    "config.action_view.annotate_rendered_view_with_filenames", "config.action_mailer.preview_paths",
    "config.action_mailer.show_previews", "config.active_job.verbose_enqueue_logs",
    "config.add_autoload_paths_to_load_path", "config.assets", "config.autoload_lib",
    "config.autoload_once_paths", "config.autoload_paths", "config.cache_classes",
    "config.colorize_logging", "config.eager_load", "config.eager_load_namespaces",
    "config.eager_load_paths", "config.enable_reloading", "config.file_watcher",
    "config.filter_parameters", "config.generators", "config.importmap.cache_sweepers",
    "config.log_formatter", "config.log_level", "config.log_tags", "config.logger", "config.paths",
    "config.reload_classes_only_on_change", "config.web_console", "ActiveRecord::Tasks",
    "ActiveSupport::Deprecation", "Rails.autoloaders", "Rails.backtrace_cleaner", "Rails.cache.logger",
    "Rails.logger", "Zeitwerk",
];

/// The constants Rails and the gems it is built from define.
const RAILS_NAMESPACES: &[&str] = &[
    "AbstractController", "ActionCable", "ActionController", "ActionDispatch", "ActionMailbox",
    "ActionMailer", "ActionText", "ActionView", "ActiveJob", "ActiveModel", "ActiveRecord",
    "ActiveStorage", "ActiveSupport", "Arel", "I18n", "Mime", "Rack", "Rails", "Zeitwerk",
];

/// Ruby's own constants an initializer calls on its own behalf.
const RUBY_CORE: &[&str] = &[
    "Dir", "ENV", "File", "GC", "Kernel", "Object", "Process", "Regexp", "STDERR", "STDOUT",
    "Signal", "String", "Thread", "Warning",
];

/// Who a dropped statement belongs to, so a reader can tell a Rails setting
/// the running app answers without from a gem the analysis does not model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// Rails behaviour the running app answers differently without.
    Rails(String),
    /// A Rails setting that only changes how the app is built, loaded or logged.
    RailsBootOnly(String),
    /// A gem's setting or call, with the gem's fate in [`crate::gems`].
    Gem { names: Vec<String>, fate: crate::gems::GemFate },
    /// The app's own code, or a Rails boot hook whose block is.
    App,
    Unknown,
}

impl Holder {
    /// The `holder` a diagnostic carries: `rails`, `rails-boot-only`,
    /// `gem:<name>[|<name>…]:<fate>`, `app` or `unknown`.
    pub fn label(&self) -> String {
        match self {
            Holder::Rails(_) => "rails".to_string(),
            Holder::RailsBootOnly(_) => "rails-boot-only".to_string(),
            Holder::Gem { names, fate } => format!("gem:{}:{}", names.join("|"), fate.label()),
            Holder::App => "app".to_string(),
            Holder::Unknown => "unknown".to_string(),
        }
    }

    /// Which of two holders a conditional reports: the one whose drop says most.
    fn weight(&self) -> u8 {
        use crate::gems::GemFate;
        match self {
            Holder::Rails(_) => 6,
            Holder::Gem { fate: GemFate::Modeled | GemFate::Framework, .. } => 5,
            Holder::App => 4,
            Holder::Gem { .. } => 3,
            Holder::Unknown => 2,
            Holder::RailsBootOnly(_) => 1,
        }
    }
}

/// The receiver-and-method target of each call or write in `node`, by
/// syntax: `config.middleware.use`, `Rack::Attack.throttle`, `self.at_exit`,
/// `def`, `const=NAME`. A conditional answers its branches' statements.
fn targets_of(node: &Node<'_>, src: &str) -> Vec<String> {
    let text_of = |n: &Node<'_>| {
        let loc = n.location();
        let text = normalize_target(&src[loc.start_offset()..loc.end_offset()]);
        match text.strip_prefix("Rails.configuration") {
            Some(rest) => format!("config{rest}"),
            None => text,
        }
    };
    let branches = |stmts: Option<Node<'_>>, out: &mut Vec<String>| {
        if let Some(stmts) = stmts {
            for inner in flatten_statements(stmts) {
                out.extend(targets_of(&inner, src));
            }
        }
    };
    let mut out = Vec::new();
    if let Some(call) = node.as_call_node() {
        let name = constant_id_str(&call.name());
        match call.receiver() {
            Some(recv) => out.push(format!("{}.{name}", text_of(&recv))),
            None => out.push(format!("self.{name}")),
        }
    } else if let Some(w) = node.as_call_operator_write_node() {
        out.push(format!("{}.{}", w.receiver().map(|r| text_of(&r)).unwrap_or_default(), constant_id_str(&w.read_name())));
    } else if let Some(w) = node.as_call_or_write_node() {
        out.push(format!("{}.{}", w.receiver().map(|r| text_of(&r)).unwrap_or_default(), constant_id_str(&w.read_name())));
    } else if let Some(w) = node.as_constant_write_node() {
        out.push(format!("const={}", constant_id_str(&w.name())));
    } else if let Some(w) = node.as_constant_path_write_node() {
        out.push(format!("const={}", text_of(&w.target().as_node())));
    } else if node.as_def_node().is_some() {
        out.push("def".to_string());
    } else if node.as_global_variable_write_node().is_some()
        || node.as_local_variable_write_node().is_some()
        || node.as_instance_variable_write_node().is_some()
    {
        out.push("var=".to_string());
    } else if let Some(c) = node.as_if_node() {
        branches(c.statements().map(|s| s.as_node()), &mut out);
        branches(c.subsequent(), &mut out);
    } else if let Some(c) = node.as_unless_node() {
        branches(c.statements().map(|s| s.as_node()), &mut out);
        branches(c.else_clause().map(|e| e.as_node()), &mut out);
    } else if let Some(e) = node.as_else_node() {
        branches(e.statements().map(|s| s.as_node()), &mut out);
    } else if let Some(c) = node.as_case_node() {
        for w in c.conditions().iter() {
            if let Some(w) = w.as_when_node() {
                branches(w.statements().map(|s| s.as_node()), &mut out);
            }
        }
        branches(c.else_clause().map(|e| e.as_node()), &mut out);
    } else if let Some(b) = node.as_begin_node() {
        branches(b.statements().map(|s| s.as_node()), &mut out);
    }
    out
}

/// Who owns a dropped statement, from its [`targets_of`]: the weightiest
/// owner among a conditional's branches.
pub fn holder_of(targets: &[String], lock: Option<&crate::gems::Lockfile>, app_namespaces: &HashSet<String>) -> Holder {
    targets
        .iter()
        .map(|t| holder_of_target(t, lock, app_namespaces))
        .max_by_key(Holder::weight)
        .unwrap_or(Holder::App)
}

fn holder_of_target(target: &str, lock: Option<&crate::gems::Lockfile>, app_namespaces: &HashSet<String>) -> Holder {
    let covers = |prefix: &str| {
        target == prefix || target.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.') || rest.starts_with("::"))
    };
    if let Some(within) = target.strip_prefix("block:") {
        // `Devise.setup do |config| … end`: its statements belong to `Devise`.
        let recv = within.rsplit_once('.').map_or(within, |(recv, _)| recv);
        // `within` is the head as written, not normalized: `::Devise.setup` keeps its `::`.
        return holder_of_target(recv.trim_start_matches("::"), lock, app_namespaces);
    }
    if target.starts_with("self.") || target == "def" || target == "var=" || target.starts_with("const=") {
        return Holder::App;
    }
    if let Some(rest) = target.strip_prefix("config.") {
        if RAILS_BOOT_HOOKS.iter().any(|h| rest == *h || rest.starts_with(&format!("{h}."))) {
            return Holder::App;
        }
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let key = &rest[..end];
        if key == "x" {
            return Holder::App;
        }
        if RAILS_BOOT_ONLY.iter().any(|p| covers(p)) {
            return Holder::RailsBootOnly(format!("config.{key}"));
        }
        if RAILS_CONFIG_KEYS.contains(&key) {
            return Holder::Rails(format!("config.{key}"));
        }
        // A gem's railtie namespace: `config.lograge`, `config.mission_control`.
        let railtie: Vec<String> = lock
            .map(|l| {
                l.specs
                    .iter()
                    .filter(|(n, _)| {
                        let n = n.replace('-', "_");
                        n == key || n.starts_with(&format!("{key}_"))
                    })
                    .map(|(n, _)| n.clone())
                    .collect()
            })
            .unwrap_or_default();
        return match gem_holder(railtie, lock) {
            Some(Holder::Gem { fate: crate::gems::GemFate::Framework, .. }) => Holder::Rails(format!("config.{key}")),
            Some(other) => other,
            None => Holder::Unknown,
        };
    }
    let end = target.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(target.len());
    let head = target[..end].trim_end_matches(':').to_string();
    // A local, a method of the file's own or a Ruby core constant: the app's code.
    if !head.starts_with(|c: char| c.is_ascii_uppercase()) || RUBY_CORE.contains(&head.as_str()) {
        return Holder::App;
    }
    if RAILS_BOOT_ONLY.iter().any(|p| covers(p)) {
        return Holder::RailsBootOnly(head);
    }
    let gems = lock.map(|l| crate::gems::lock_gems_owning_constant(l, &head)).unwrap_or_default();
    if let Some(holder) = gem_holder(gems.into_iter().map(str::to_string).collect(), lock) {
        match holder {
            Holder::Gem { fate: crate::gems::GemFate::Framework, .. } => return Holder::Rails(head),
            other => return other,
        }
    }
    let top = head.split("::").next().unwrap_or(&head);
    if RAILS_NAMESPACES.contains(&top) {
        return Holder::Rails(head);
    }
    if app_namespaces.contains(top) {
        return Holder::App;
    }
    Holder::Unknown
}

/// The gems of `names` as one holder: their fate when they agree.
fn gem_holder(names: Vec<String>, lock: Option<&crate::gems::Lockfile>) -> Option<Holder> {
    // As `GemCensus::of` labels it: a gem locked from a `PATH` source is in-repo.
    let fate_of = |n: &str| match lock {
        Some(l) if l.is_in_repo(n) => crate::gems::GemFate::InRepo,
        Some(l) if l.is_source(n) => crate::gems::GemFate::Source,
        _ => crate::gems::fate_of(n),
    };
    let first = names.first()?;
    let fate = fate_of(first);
    let fate = if names.iter().all(|n| fate_of(n) == fate) { fate } else { crate::gems::GemFate::Unknown };
    Some(Holder::Gem { names, fate })
}

/// Run the ledger over the host and every in-tree engine and push one
/// warning per unconsumed statement into the ingest diagnostic channel.
/// `admitted` is the set of engine class names whose mount was admitted
/// (`engine_route_sources`' keys).
pub(super) fn record<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
    admitted: &HashSet<String>,
    lock: Option<&crate::gems::Lockfile>,
    app_namespaces: &HashSet<String>,
) {
    let findings = ledger(vfs, dir, path_gems, admitted);
    for f in findings.into_iter().flat_map(|l| l.findings) {
        let holder = holder_of(&f.targets, lock, app_namespaces);
        let location = format!("{}:{}:{}", f.path, f.line, f.column);
        let file = sources::file_id(&absolute(dir, &f.path));
        let span = if file.0 == 0 { Span::synthetic() } else { Span { file, start: f.start, end: f.end } };
        let kind = DiagnosticKind::UnconsumedInitializer {
            owner: Symbol::from(f.owner.as_str()),
            statement: Symbol::from(f.statement.as_str()),
            location: Symbol::from(location.as_str()),
            holder: Symbol::from(holder.label().as_str()),
        };
        let message = Diagnostic::stub_text(&kind);
        prism::push(Diagnostic { span, severity: Diagnostic::default_severity(&kind), kind, message });
    }
}

/// `sources` registers absolute paths (`dir.join(rel)`); undo
/// [`display_path`].
fn absolute(dir: &Path, relative: &str) -> String {
    let p = Path::new(relative);
    if p.is_absolute() { relative.to_string() } else { dir.join(p).display().to_string() }
}

/// The whole ledger, file by file, in report order: host initializers,
/// `config/application.rb`, then each engine's initializers and class body.
pub fn ledger<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[PathBuf],
    admitted: &HashSet<String>,
) -> Vec<FileLedger> {
    let mut out = Vec::new();
    let application = dir.join("config/application.rb");
    let application_bytes = vfs.read(&application).ok();
    let lib_ignores =
        application_bytes.as_deref().map(extract_autoload_lib_ignores).unwrap_or_default();

    let init_dir = dir.join("config/initializers");
    if vfs.is_dir(&init_dir) {
        for entry in read_rb_files(vfs, &init_dir).unwrap_or_default() {
            let Ok(bytes) = vfs.read(&entry) else { continue };
            out.push(initializer_ledger(Origin::HostInitializer, "", &entry, &bytes, &lib_ignores));
        }
    }
    if let Some(bytes) = &application_bytes {
        out.push(application_ledger(&application, bytes, &lib_ignores));
    }
    if super::propshaft::version_is_consumed(vfs, dir) {
        for ledger in &mut out {
            for (_, verdict) in &mut ledger.verdicts {
                if matches!(verdict, Verdict::Inert(reason) if *reason == ASSET_VERSION_INERT) {
                    *verdict = Verdict::Consumed("Propshaft asset version");
                }
            }
        }
    }
    for engine_dir in path_gems {
        let lib = engine_dir.join("lib");
        if !declares_rails_engine(vfs, &lib) {
            continue;
        }
        let engine_name = display_path(engine_dir);
        let initializers = engine_dir.join("config/initializers");
        if vfs.is_dir(&initializers) {
            for entry in read_rb_files(vfs, &initializers).unwrap_or_default() {
                let Ok(bytes) = vfs.read(&entry) else { continue };
                out.push(initializer_ledger(
                    Origin::EngineInitializer,
                    &engine_name,
                    &entry,
                    &bytes,
                    &[],
                ));
            }
        }
        for entry in read_rb_files(vfs, &lib).unwrap_or_default() {
            let Ok(bytes) = vfs.read(&entry) else { continue };
            if !bytes.windows(10).any(|w| w == b"Rails::Eng") {
                continue;
            }
            out.extend(engine_class_ledgers(&entry, &bytes, admitted, &[]));
        }
    }
    out
}

/// Render the survey section for the ledger warnings in `diags`: a count and
/// the bucket per statement head, mirroring [`survey::render_report`].
pub fn render_summary(diags: &[Diagnostic]) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write;
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut holders: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for d in diags {
        if let DiagnosticKind::UnconsumedInitializer { statement, location, holder, .. } = &d.kind {
            total += 1;
            buckets.entry(statement.as_str().to_string()).or_default().push(location.as_str().to_string());
            // By fate, not by gem: the gem names are in each line.
            let kind = match holder.as_str().strip_prefix("gem:").and_then(|g| g.rsplit_once(':')) {
                Some((_, fate)) => format!("gem ({fate})"),
                None => holder.as_str().to_string(),
            };
            *holders.entry(kind).or_default() += 1;
        }
    }
    if total == 0 {
        return String::new();
    }
    let mut sorted: Vec<_> = buckets.iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let mut out = String::new();
    let _ = writeln!(
        out,
        "── Initializer ledger: {total} unconsumed statement(s), {} distinct head(s) ──",
        sorted.len()
    );
    let mut by_holder: Vec<_> = holders.into_iter().collect();
    by_holder.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let by_holder: Vec<String> = by_holder.iter().map(|(k, n)| format!("{k} {n}")).collect();
    let _ = writeln!(out, "  by owner: {}", by_holder.join(", "));
    for (head, locations) in sorted {
        let _ = writeln!(out, "  [{}×] {head}", locations.len());
        for l in locations.iter().take(4) {
            let _ = writeln!(out, "        {l}");
        }
        if locations.len() > 4 {
            let _ = writeln!(out, "        … and {} more", locations.len() - 4);
        }
    }
    out
}
