//! The gem census: what the app's `Gemfile.lock` declares, and what the
//! analyzer makes of each gem.
//!
//! A real app's first `roundhouse check` used to read as a list of the
//! app's bugs when most of it was one gem the analyzer had never heard
//! of: the Rails tutorial's 23 errors were `will_paginate` — one
//! unmodeled `paginate` cascading into every view that read the ivar.
//! The census puts the gem list beside the diagnostics so the two can
//! be read together: which gems are Rails itself, which the analyzer
//! models, which never touch the analysis (servers, linters, test
//! drivers), and which it does not know — the last list is the
//! prioritization signal, and [`crate::analyze::attribution`] uses it
//! to label a dispatch failure on an unknown gem's surface as the
//! tool's coverage rather than the author's error.
//!
//! Only the lockfile's `DEPENDENCIES` (the gems the author wrote in
//! the Gemfile) are classified; transitive gems are counted, not
//! judged — nobody chose them. Classification is a table plus a few
//! family rules (`rubocop-*`, `opentelemetry-*`, `aws-sdk-*`); a gem
//! the table has never seen is `Unknown`, honestly.

use serde::{Deserialize, Serialize};

/// A parsed `Gemfile.lock`: every resolved spec and the direct
/// dependencies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    /// `GEM` / `PATH` / `GIT` specs: `(name, version)`, lock order.
    pub specs: Vec<(String, String)>,
    /// The `DEPENDENCIES` section — what the Gemfile names, lock order.
    pub dependencies: Vec<String>,
}

impl Lockfile {
    /// Parse the Bundler lockfile format: section headers at column 0,
    /// specs at four spaces under `specs:`, dependencies at two spaces
    /// under `DEPENDENCIES`. Deeper indentation is a spec's own
    /// dependency list and is skipped. Tolerant: an unfamiliar section
    /// is ignored, a malformed line is skipped.
    pub fn parse(text: &str) -> Lockfile {
        let mut lock = Lockfile::default();
        let mut section = "";
        let mut in_specs = false;
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                section = line.trim();
                in_specs = false;
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let body = line.trim();
            match section {
                "GEM" | "PATH" | "GIT" | "PLUGIN SOURCE" => {
                    if indent == 2 {
                        in_specs = body == "specs:";
                    } else if indent == 4 && in_specs {
                        // `rails (8.0.2)` — the version in parens; a
                        // platform suffix (`nokogiri (1.18.3-arm64-darwin)`)
                        // stays with the version.
                        let (name, rest) = body.split_once(' ').unwrap_or((body, ""));
                        let version = rest.trim().trim_start_matches('(').trim_end_matches(')');
                        lock.specs.push((name.to_string(), version.to_string()));
                    }
                }
                "DEPENDENCIES" => {
                    if indent == 2 {
                        // `rails (~> 8.0.2)` / `debug` / `rails!` (a
                        // git/path source marker).
                        let name = body.split(' ').next().unwrap_or(body).trim_end_matches('!');
                        if !name.is_empty() {
                            lock.dependencies.push(name.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        lock
    }

    /// Is `name` resolved in this lock (directly or transitively)?
    pub fn has(&self, name: &str) -> bool {
        self.specs.iter().any(|(n, _)| n == name)
    }

    pub fn version_of(&self, name: &str) -> Option<&str> {
        self.specs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// What the analyzer makes of a gem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GemFate {
    /// Rails itself and the pieces the analyzer treats as part of it
    /// (Hotwire, asset pipelines, the Solid adapters).
    Framework,
    /// Ruby's own default/bundled gems pinned in the Gemfile.
    Stdlib,
    /// A third-party gem whose app-facing surface the analyzer models
    /// — its DSL, its helpers, or its classes in the gem catalog.
    Modeled,
    /// Never enters the analysis: servers, linters, debuggers, test
    /// drivers, profilers, deploy tooling, database drivers (the schema
    /// is the model), monitoring.
    Infrastructure,
    /// Not in the table. Anything it adds to the app's classes is
    /// invisible to the analysis — a dispatch on its surface fails.
    Unknown,
}

impl GemFate {
    pub fn label(self) -> &'static str {
        match self {
            GemFate::Framework => "framework",
            GemFate::Stdlib => "stdlib",
            GemFate::Modeled => "modeled",
            GemFate::Infrastructure => "infrastructure",
            GemFate::Unknown => "unknown",
        }
    }
}

/// One classified direct dependency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GemEntry {
    pub name: String,
    pub version: Option<String>,
    pub fate: GemFate,
}

/// The census of an app's direct dependencies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GemCensus {
    /// Direct dependencies, lock order.
    pub gems: Vec<GemEntry>,
    /// Resolved specs beyond the direct ones.
    pub transitive: usize,
}

impl GemCensus {
    pub fn of(lock: &Lockfile) -> GemCensus {
        let gems: Vec<GemEntry> = lock
            .dependencies
            .iter()
            .map(|name| GemEntry {
                name: name.clone(),
                version: lock.version_of(name).map(|v| v.to_string()),
                fate: fate_of(name),
            })
            .collect();
        let transitive = lock.specs.len().saturating_sub(gems.len());
        GemCensus { gems, transitive }
    }

    pub fn count(&self, fate: GemFate) -> usize {
        self.gems.iter().filter(|g| g.fate == fate).count()
    }

    pub fn unknown(&self) -> impl Iterator<Item = &GemEntry> {
        self.gems.iter().filter(|g| g.fate == GemFate::Unknown)
    }

    /// The one-line summary every skin prints: `31 gems: 6 framework,
    /// 3 stdlib, 4 modeled, 14 infrastructure, 4 unknown (redcarpet,
    /// resque, rouge, front_matter_parser)`.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for fate in [
            GemFate::Framework,
            GemFate::Stdlib,
            GemFate::Modeled,
            GemFate::Infrastructure,
        ] {
            let n = self.count(fate);
            if n > 0 {
                parts.push(format!("{n} {}", fate.label()));
            }
        }
        let unknown: Vec<&str> = self.unknown().map(|g| g.name.as_str()).collect();
        if unknown.is_empty() {
            parts.push("0 unknown".to_string());
        } else {
            parts.push(format!("{} unknown ({})", unknown.len(), unknown.join(", ")));
        }
        format!("{} gems: {}", self.gems.len(), parts.join(", "))
    }
}

/// The table. Membership in `Modeled` is a claim about this tree —
/// each row names where the modeling lives, so a reader can check it.
/// Keep alphabetical within a fate.
const FATES: &[(&str, GemFate)] = &[
    // ── Framework ────────────────────────────────────────────────
    ("actioncable", GemFate::Framework),
    ("actionmailbox", GemFate::Framework),
    ("actionmailer", GemFate::Framework),
    ("actionpack", GemFate::Framework),
    ("actiontext", GemFate::Framework),
    ("actionview", GemFate::Framework),
    ("activejob", GemFate::Framework),
    ("activemodel", GemFate::Framework),
    ("activerecord", GemFate::Framework),
    ("activestorage", GemFate::Framework),
    ("activesupport", GemFate::Framework),
    ("cssbundling-rails", GemFate::Framework),
    ("dartsass-rails", GemFate::Framework),
    ("haml-rails", GemFate::Framework), // HAML templates are ingested (src/haml)
    ("importmap-rails", GemFate::Framework),
    ("i18n", GemFate::Framework), // catalog/gems: I18n
    ("jbuilder", GemFate::Framework), // `json` is a Jbuilder (analyze/registry/view)
    ("jsbundling-rails", GemFate::Framework),
    ("propshaft", GemFate::Framework),
    ("rails", GemFate::Framework),
    ("railties", GemFate::Framework),
    ("rails-i18n", GemFate::Framework),
    ("solid_cable", GemFate::Framework),
    ("solid_cache", GemFate::Framework),
    ("solid_queue", GemFate::Framework), // in-process ActiveJob queue (runtime/ruby)
    ("sprockets", GemFate::Framework),
    ("sprockets-rails", GemFate::Framework),
    ("stimulus-rails", GemFate::Framework),
    ("tailwindcss-rails", GemFate::Framework),
    ("turbo-rails", GemFate::Framework), // turbo_stream / broadcasts (lower/view_to_library)
    ("vite_rails", GemFate::Framework),
    // ── Stdlib (default/bundled gems pinned in a Gemfile) ────────
    ("base64", GemFate::Stdlib),
    ("benchmark", GemFate::Stdlib),
    ("bigdecimal", GemFate::Stdlib),
    ("cgi", GemFate::Stdlib),
    ("csv", GemFate::Stdlib),
    ("digest", GemFate::Stdlib),
    ("drb", GemFate::Stdlib),
    ("erb", GemFate::Stdlib),
    ("ipaddr", GemFate::Stdlib),
    ("irb", GemFate::Stdlib),
    ("json", GemFate::Stdlib),
    ("logger", GemFate::Stdlib),
    ("mutex_m", GemFate::Stdlib),
    ("net-http", GemFate::Stdlib),
    ("net-imap", GemFate::Stdlib),
    ("net-pop", GemFate::Stdlib),
    ("net-smtp", GemFate::Stdlib),
    ("observer", GemFate::Stdlib),
    ("ostruct", GemFate::Stdlib),
    ("racc", GemFate::Stdlib),
    ("rake", GemFate::Stdlib),
    ("rdoc", GemFate::Stdlib),
    ("rexml", GemFate::Stdlib),
    ("rss", GemFate::Stdlib),
    ("securerandom", GemFate::Stdlib),
    ("tsort", GemFate::Stdlib),
    ("uri", GemFate::Stdlib),
    ("yaml", GemFate::Stdlib),
    // ── Modeled (each row names where) ───────────────────────────
    ("addressable", GemFate::Modeled), // catalog/gems: Addressable::URI
    ("bcrypt", GemFate::Modeled),      // catalog/gems: BCrypt::*; has_secure_password
    ("devise", GemFate::Modeled),      // registry/controllers: scope helpers; routes: devise_for
    ("faker", GemFate::Modeled),       // catalog/gems: Faker::*
    ("geared_pagination", GemFate::Modeled), // registry/controllers: set_page_and_extract_portion_from
    ("image_processing", GemFate::Modeled), // active_storage variants seam
    ("kaminari", GemFate::Modeled),    // catalog: page / per / padding / without_count
    ("mail", GemFate::Modeled),        // catalog/gems: Mail::Address; ActionMailer
    ("mocha", GemFate::Modeled),       // lower/mocha bridge
    ("nokogiri", GemFate::Modeled),    // catalog/gems: Nokogiri
    ("pdf-reader", GemFate::Modeled),  // catalog/gems: PDF::Reader
    ("platform_agent", GemFate::Modeled), // useragent port
    ("pushover", GemFate::Modeled),    // catalog/gems: Pushover
    ("rack-mini-profiler", GemFate::Modeled), // catalog/gems: Rack::MiniProfiler
    ("rotp", GemFate::Modeled),        // catalog/gems: ROTP::*
    ("rqrcode", GemFate::Modeled),     // catalog/gems: RQRCode::QRCode
    ("ruby-vips", GemFate::Modeled),   // active_storage variants seam
    ("sidekiq", GemFate::Modeled),     // registry/library: Sidekiq::Worker
    ("telebugs", GemFate::Modeled),    // catalog/gems: Telebugs
    ("useragent", GemFate::Modeled),   // useragent port (runtime/ruby)
    ("web-push", GemFate::Modeled),    // WebPush pool (runtime/ruby)
    ("webpush", GemFate::Modeled),     // the gem's former name; same surface
    ("webmock", GemFate::Modeled),     // webmock double (runtime/ruby)
    ("will_paginate", GemFate::Modeled), // catalog: paginate; registry/view: will_paginate, page_entries_info
    // ── Infrastructure ───────────────────────────────────────────
    ("active_record_doctor", GemFate::Infrastructure),
    ("annotaterb", GemFate::Infrastructure),
    ("autotuner", GemFate::Infrastructure),
    ("better_errors", GemFate::Infrastructure),
    ("benchmark-perf", GemFate::Infrastructure),
    ("binding_of_caller", GemFate::Infrastructure),
    ("bootsnap", GemFate::Infrastructure),
    ("brakeman", GemFate::Infrastructure),
    ("bullet", GemFate::Infrastructure),
    ("bundler-audit", GemFate::Infrastructure),
    ("capybara", GemFate::Infrastructure),
    ("climate_control", GemFate::Infrastructure),
    ("connection_pool", GemFate::Infrastructure),
    ("database_consistency", GemFate::Infrastructure),
    ("debug", GemFate::Infrastructure),
    ("dockerfile-rails", GemFate::Infrastructure),
    ("dotenv", GemFate::Infrastructure),
    ("dotenv-rails", GemFate::Infrastructure),
    ("error_highlight", GemFate::Infrastructure),
    ("fabrication", GemFate::Infrastructure),
    ("factory_bot", GemFate::Infrastructure),
    ("factory_bot_rails", GemFate::Infrastructure),
    ("flamegraph", GemFate::Infrastructure),
    ("flatware-rspec", GemFate::Infrastructure),
    ("foreman", GemFate::Infrastructure),
    ("hiredis-client", GemFate::Infrastructure),
    ("hotwire-livereload", GemFate::Infrastructure),
    ("hotwire-spark", GemFate::Infrastructure),
    ("haml_lint", GemFate::Infrastructure),
    ("httplog", GemFate::Infrastructure),
    ("i18n-tasks", GemFate::Infrastructure),
    ("kamal", GemFate::Infrastructure),
    ("listen", GemFate::Infrastructure),
    ("lograge", GemFate::Infrastructure),
    ("logstash-event", GemFate::Infrastructure),
    ("logster", GemFate::Infrastructure),
    ("memory_profiler", GemFate::Infrastructure),
    ("mini_racer", GemFate::Infrastructure),
    ("minio_runner", GemFate::Infrastructure),
    ("mission_control-jobs", GemFate::Infrastructure),
    ("mysql2", GemFate::Infrastructure),
    ("newrelic_rpm", GemFate::Infrastructure),
    ("parallel_tests", GemFate::Infrastructure),
    ("pg", GemFate::Infrastructure),
    ("pghero", GemFate::Infrastructure),
    ("pitchfork", GemFate::Infrastructure),
    ("prometheus_exporter", GemFate::Infrastructure),
    ("pry", GemFate::Infrastructure),
    ("pry-rails", GemFate::Infrastructure),
    ("puma", GemFate::Infrastructure),
    ("rack-attack", GemFate::Infrastructure),
    ("rack-cors", GemFate::Infrastructure),
    ("rack-protection", GemFate::Infrastructure),
    ("rack-test", GemFate::Infrastructure),
    ("rails-controller-testing", GemFate::Infrastructure),
    ("rails-dom-testing", GemFate::Infrastructure),
    ("rb-fsevent", GemFate::Infrastructure),
    ("rbtrace", GemFate::Infrastructure),
    ("rtlcss", GemFate::Infrastructure),
    ("ruby-prof", GemFate::Infrastructure),
    ("ruby-progressbar", GemFate::Infrastructure),
    ("ruby-lsp", GemFate::Infrastructure),
    ("sassc-embedded", GemFate::Infrastructure),
    ("selenium-webdriver", GemFate::Infrastructure),
    ("shoulda-matchers", GemFate::Infrastructure),
    ("simplecov", GemFate::Infrastructure),
    ("skylight", GemFate::Infrastructure),
    ("spring", GemFate::Infrastructure),
    ("sqlite3", GemFate::Infrastructure),
    ("stackprof", GemFate::Infrastructure),
    ("standard", GemFate::Infrastructure),
    ("strong_migrations", GemFate::Infrastructure),
    ("super_diff", GemFate::Infrastructure),
    ("terser", GemFate::Infrastructure),
    ("test-prof", GemFate::Infrastructure),
    ("thruster", GemFate::Infrastructure),
    ("thor", GemFate::Infrastructure),
    ("tty-prompt", GemFate::Infrastructure),
    ("trilogy", GemFate::Infrastructure),
    ("tzinfo-data", GemFate::Infrastructure),
    ("unicorn", GemFate::Infrastructure),
    ("vcr", GemFate::Infrastructure),
    ("web-console", GemFate::Infrastructure),
    ("webdrivers", GemFate::Infrastructure),
    ("yard", GemFate::Infrastructure),
];

/// Family rules for gems the table doesn't list one by one. A prefix
/// match classifies every member (`rubocop-rails`, `rubocop-rspec`,
/// `opentelemetry-instrumentation-*`).
const FAMILIES: &[(&str, GemFate)] = &[
    ("aws-sdk-", GemFate::Infrastructure), // storage/service clients configured, not called, in app code
    ("capybara-", GemFate::Infrastructure),
    ("database_cleaner", GemFate::Infrastructure),
    ("guard", GemFate::Infrastructure),
    ("letter_opener", GemFate::Infrastructure),
    ("minitest", GemFate::Infrastructure),
    ("opentelemetry-", GemFate::Infrastructure),
    ("playwright", GemFate::Infrastructure),
    ("rspec", GemFate::Infrastructure),
    ("rubocop", GemFate::Infrastructure),
    ("ruby-lsp-", GemFate::Infrastructure),
    ("sentry-", GemFate::Infrastructure),
    ("simplecov-", GemFate::Infrastructure),
    ("standard-", GemFate::Infrastructure),
];

/// Classify a gem by name.
pub fn fate_of(name: &str) -> GemFate {
    if let Some((_, fate)) = FATES.iter().find(|(n, _)| *n == name) {
        return *fate;
    }
    if let Some((_, fate)) = FAMILIES.iter().find(|(prefix, _)| name.starts_with(prefix)) {
        return *fate;
    }
    GemFate::Unknown
}

/// Methods a popular *unmodeled* gem adds to the app's models,
/// controllers and views — the surface a dispatch failure lands on
/// when the gem is in the Gemfile. Attribution matches a failed
/// method name here (and the gem's presence in the lock) to label the
/// diagnostic as the tool's coverage gap. Deliberately the DSL and
/// helper names an author writes, not the gems' internals.
const SURFACES: &[(&str, &[&str])] = &[
    ("acts_as_list", &["acts_as_list", "move_to_top", "move_to_bottom", "move_higher", "move_lower", "insert_at", "move_to", "first?", "last?", "higher_item", "lower_item"]),
    ("acts-as-taggable-on", &["acts_as_taggable", "acts_as_taggable_on", "tag_list", "tag_list=", "tagged_with", "tag_counts", "all_tags"]),
    ("ancestry", &["has_ancestry", "ancestors", "descendants", "subtree", "siblings", "root", "root?", "is_root?", "has_children?", "child_ids", "depth", "path"]),
    ("audited", &["audited", "audits", "own_and_associated_audits", "audit_comment"]),
    ("cancancan", &["can?", "cannot?", "authorize!", "load_and_authorize_resource", "load_resource", "authorize_resource", "current_ability", "accessible_by", "check_authorization", "skip_authorization_check"]),
    ("carrierwave", &["mount_uploader", "mount_uploaders", "remove_avatar!", "store!"]),
    ("discard", &["discard", "discard!", "undiscard", "undiscard!", "discarded?", "undiscarded?", "kept", "discarded", "with_discarded", "discard_all", "undiscard_all"]),
    ("doorkeeper", &["doorkeeper_authorize!", "doorkeeper_token", "current_resource_owner"]),
    ("enumerize", &["enumerize"]),
    ("friendly_id", &["friendly_id", "friendly", "slug_candidates", "should_generate_new_friendly_id?", "normalize_friendly_id"]),
    ("geocoder", &["geocoded_by", "reverse_geocoded_by", "geocode", "reverse_geocode", "near", "distance_to", "distance_from", "bearing_to", "within_bounding_box"]),
    ("money-rails", &["monetize"]),
    ("pagy", &["pagy", "pagy_nav", "pagy_info", "pagy_array", "pagy_countless", "pagy_bootstrap_nav", "pagy_get_vars"]),
    ("paper_trail", &["has_paper_trail", "versions", "paper_trail", "whodunnit", "reify", "live?"]),
    ("paranoia", &["acts_as_paranoid", "with_deleted", "only_deleted", "without_deleted", "really_destroy!", "restore", "restore!", "paranoia_destroyed?", "deleted?"]),
    ("pundit", &["authorize", "policy_scope", "policy", "skip_authorization", "skip_policy_scope", "verify_authorized", "verify_policy_scoped", "pundit_user", "permitted_attributes", "authorized?"]),
    ("ransack", &["ransack", "ransackable_attributes", "ransackable_associations", "ransackable_scopes", "sort_link", "search_form_for", "sort_url"]),
    ("rolify", &["rolify", "resourcify", "has_role?", "add_role", "remove_role", "has_any_role?", "has_all_roles?", "with_role", "roles"]),
    ("simple_form", &["simple_form_for", "simple_fields_for", "simple_nested_form_for"]),
    ("state_machines", &["state_machine"]),
    ("state_machines-activerecord", &["state_machine"]),
    ("aasm", &["aasm", "aasm_state", "may_fire?"]),
    ("kredis", &["kredis_string", "kredis_integer", "kredis_boolean", "kredis_list", "kredis_unique_list", "kredis_set", "kredis_hash", "kredis_flag", "kredis_counter", "kredis_json", "kredis_datetime"]),
];

/// The gem whose surface a method name belongs to, if any of the
/// gems in `lock` claim it. `None` when no present gem claims the
/// name. Modeled gems never claim (their surface resolves).
pub fn gem_claiming_method(lock: &Lockfile, method: &str) -> Option<&'static str> {
    SURFACES
        .iter()
        .filter(|(gem, methods)| methods.contains(&method) && lock.has(gem))
        .map(|(gem, _)| *gem)
        .next()
}

/// The top-level Ruby constant a gem defines, by convention (`front_matter_parser`
/// → `FrontMatterParser`, `aws-sdk-s3` → `Aws`) with overrides for the
/// irregular ones. Used to attribute a failed dispatch on
/// `Redcarpet::Markdown` to the `redcarpet` gem.
pub fn namespace_of(gem: &str) -> String {
    const IRREGULAR: &[(&str, &str)] = &[
        ("aws-sdk-s3", "Aws"),
        ("aws-sdk-core", "Aws"),
        ("bcrypt", "BCrypt"),
        ("cancancan", "CanCan"),
        ("combine_pdf", "CombinePDF"),
        ("fast_excel", "FastExcel"),
        ("http", "HTTP"),
        ("i18n", "I18n"),
        ("jwt", "JWT"),
        ("mini_magick", "MiniMagick"),
        ("net-http-persistent", "Net"),
        ("nokogiri", "Nokogiri"),
        ("oj", "Oj"),
        ("pdf-reader", "PDF"),
        ("pg", "PG"),
        ("rack-attack", "Rack"),
        ("rqrcode", "RQRCode"),
        ("rotp", "ROTP"),
        ("ruby-vips", "Vips"),
        ("rubyzip", "Zip"),
        // sorbet-runtime's entire app-facing surface is `T` — `T.let`,
        // `T::Enum`, `T::Struct` — which the name-derived namespace
        // (`SorbetRuntime`) never matches, so nothing on `T` was
        // attributed to the gem that owns it.
        ("sorbet-runtime", "T"),
        ("sqlite3", "SQLite3"),
        ("twilio-ruby", "Twilio"),
        ("web-push", "WebPush"),
        ("zip_kit", "ZipKit"),
    ];
    if let Some((_, ns)) = IRREGULAR.iter().find(|(g, _)| *g == gem) {
        return (*ns).to_string();
    }
    camelize(gem)
}

/// The top-level constants a gem might define, best guess first: the
/// whole name camelized, then — for a dashed name — its first segment
/// alone.
///
/// The second candidate is what an org- or suite-prefixed gem needs.
/// `aws-sdk-s3` defines `Aws`, not `AwsSdkS3`; `rack-attack` extends
/// `Rack`; a house gem called `<org>-<thing>` defines `<Org>::<Thing>`.
/// The four entries in `IRREGULAR` that say exactly this were the
/// hand-written version of the rule, and every private gem that follows
/// the same convention needed its own entry to be attributable at all —
/// which, by definition, it cannot have here.
fn namespace_candidates(gem: &str) -> Vec<String> {
    let mut candidates = vec![namespace_of(gem)];
    if let Some((head, _)) = gem.split_once('-') {
        let first = camelize(head);
        if !candidates.contains(&first) {
            candidates.push(first);
        }
    }
    candidates
}

fn camelize(gem: &str) -> String {
    gem.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The unknown gem (in `lock`) whose namespace `constant_path` sits
/// under, if any: `Redcarpet::Markdown` → `redcarpet`.
pub fn gem_owning_constant<'a>(census: &'a GemCensus, constant_path: &str) -> Option<&'a str> {
    let head = constant_path.split("::").next().unwrap_or(constant_path);
    // Two passes, so a full-name match always beats a first-segment
    // one: two house gems under the same prefix both answer to `Acme`,
    // and the one that spells the whole constant is the better claim.
    census
        .unknown()
        .find(|g| namespace_of(&g.name) == head)
        .or_else(|| {
            census
                .unknown()
                .find(|g| namespace_candidates(&g.name).iter().any(|c| c == head))
        })
        .map(|g| g.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "\
GEM
  remote: https://rubygems.org/
  specs:
    actionpack (8.0.2)
      activesupport (= 8.0.2)
    nokogiri (1.18.3-arm64-darwin)
      racc (~> 1.4)
    pundit (2.4.0)
      activesupport (>= 3.0.0)
    rails (8.0.2)
    acme-core (2.1.0)
    acme-telemetry (1.0.0)
    redcarpet (3.6.1)
    sorbet-runtime (0.5.12374)
    will_paginate (4.0.1)

PLATFORMS
  arm64-darwin-24

DEPENDENCIES
  acme-core
  acme-telemetry
  pundit
  rails (~> 8.0.2)
  redcarpet
  sorbet-runtime
  will_paginate!

BUNDLED WITH
   2.6.2
";

    #[test]
    fn parses_specs_and_dependencies() {
        let lock = Lockfile::parse(LOCK);
        assert_eq!(
            lock.dependencies,
            ["acme-core", "acme-telemetry", "pundit", "rails", "redcarpet", "sorbet-runtime", "will_paginate"]
        );
        assert_eq!(lock.version_of("rails"), Some("8.0.2"));
        assert_eq!(lock.version_of("nokogiri"), Some("1.18.3-arm64-darwin"));
        assert!(lock.has("actionpack"), "transitive specs are resolved too");
        assert!(!lock.has("activesupport"), "a spec's own dependency lines are not specs");
    }

    #[test]
    fn census_classifies_and_summarizes() {
        let census = GemCensus::of(&Lockfile::parse(LOCK));
        let fates: Vec<(&str, GemFate)> = census.gems.iter().map(|g| (g.name.as_str(), g.fate)).collect();
        assert_eq!(
            fates,
            [
                ("acme-core", GemFate::Unknown),
                ("acme-telemetry", GemFate::Unknown),
                ("pundit", GemFate::Unknown),
                ("rails", GemFate::Framework),
                ("redcarpet", GemFate::Unknown),
                ("sorbet-runtime", GemFate::Unknown),
                ("will_paginate", GemFate::Modeled),
            ]
        );
        assert_eq!(census.transitive, 2);
        assert_eq!(
            census.summary(),
            "7 gems: 1 framework, 1 modeled, 5 unknown \
             (acme-core, acme-telemetry, pundit, redcarpet, sorbet-runtime)"
        );
    }

    #[test]
    fn families_and_namespaces() {
        assert_eq!(fate_of("rubocop-rails"), GemFate::Infrastructure);
        assert_eq!(fate_of("opentelemetry-instrumentation-pg"), GemFate::Infrastructure);
        assert_eq!(fate_of("never-heard-of-it"), GemFate::Unknown);
        assert_eq!(namespace_of("front_matter_parser"), "FrontMatterParser");
        assert_eq!(namespace_of("aws-sdk-s3"), "Aws");
        assert_eq!(namespace_of("rubyzip"), "Zip");
        let census = GemCensus::of(&Lockfile::parse(LOCK));
        assert_eq!(gem_owning_constant(&census, "Redcarpet::Markdown"), Some("redcarpet"));
        assert_eq!(namespace_of("sorbet-runtime"), "T");
        assert_eq!(
            gem_owning_constant(&census, "T::Enum"),
            Some("sorbet-runtime"),
            "a dispatch on `T::…` is the gem's, not the app's"
        );
        // A dashed gem answers to its first segment as well as to its
        // whole name, which is what `aws-sdk-s3 → Aws` says by hand.
        assert_eq!(gem_owning_constant(&census, "Acme::Client"), Some("acme-core"));
        assert_eq!(
            gem_owning_constant(&census, "AcmeTelemetry::Span"),
            Some("acme-telemetry"),
            "the whole name still wins where a gem spells it out"
        );
        assert_eq!(gem_owning_constant(&census, "Rails"), None, "framework gems don't claim");
        let lock = Lockfile::parse(LOCK);
        assert_eq!(gem_claiming_method(&lock, "policy_scope"), Some("pundit"));
        assert_eq!(gem_claiming_method(&lock, "friendly_id"), None, "friendly_id is not in this lock");
    }
}
