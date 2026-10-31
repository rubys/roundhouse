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
//!
//! Ruby-family gem require and demand tables also live here. Shared extension
//! validation reads them on both native and WASM builds.

use serde::{Deserialize, Serialize};

/// How an emitted body asks for a gem: a constant name or literal text.
pub(crate) enum Marker {
    Constant(&'static str),
    AnyText(&'static [&'static str]),
}

mod rack_mini_profiler;

/// Not a section of `gem_facades.rb`: a package keeps one gem's Ruby, signatures and assets under `runtime/gems/<dir>/`, so it moves as a unit.
pub(crate) struct GemPackage {
    pub gem: &'static str,
    pub dir: &'static str,
    pub entry: &'static str,
    pub config_receiver: Option<&'static str>,
    pub config_setters: &'static [&'static str],
}

pub(crate) const GEM_PACKAGES: &[GemPackage] = &[rack_mini_profiler::PACKAGE];

/// The emitted constructor call for declared Active Storage variants.
/// Project assembly uses it to select the image processor. An app without
/// variants never emits this call.
pub(crate) const VARIATION_MARKER: &str = "ActiveStorage::Variation.new(";

/// The gems every ruby-family tree guarded-requires, written once.
///
/// Under Rails, Bundler auto-requires these; the transpiled tree loads
/// them so app classes that reach gem constants at LOAD time (lobsters'
/// `html_encoder.rb` runs `HTMLEntities.new` in its class body;
/// campfire's `ApplicationPlatform < PlatformAgent` names its
/// superclass) or at request time (bcrypt behind the synthesized
/// `User#authenticate`, rotp behind 2FA, markly+nokogiri behind
/// `Markdowner.to_html`) resolve.
///
/// svg-graph loads by file path: the gem has no `svg-graph.rb` entry
/// file, and lobsters' Gemfile declares `require: "SVG/Graph/TimeSeries"`.
///
/// Kept in step with `RUNTIME_GEMS` — that table is the DEMAND (what
/// the emitted Gemfile declares, derived from the constants `app/`
/// names), this one is the LOAD.
pub(crate) const GEM_REQUIRES: &[&str] = &[
    "bcrypt",
    "htmlentities",
    "rotp",
    "markly",
    // Current lobsters' Markdowner renders through commonmarker 2.x (the
    // ruby-bench snapshot used markly).
    "commonmarker",
    "nokogiri",
    "parslet",
    "typeid",
    "rqrcode",
    "rqrcode_core",
    "SVG/Graph/TimeSeries",
    "sentry-ruby",
    "rails-html-sanitizer",
    "net/http/persistent",
    "web-push",
    "faraday",
    "faraday/multipart",
    "faraday/retry",
    "retryable",
    "jwt",
    // Not the gem's entry file: it loads the Railtie whenever `Rails::VERSION` is defined, which this tree's is, with no `Rails::Railtie` under it.
    "mini_profiler",
    "rest-client",
    "elastic_apm",
];

/// Requires in `GEM_REQUIRES` a tree loads only when `app/` names the
/// constant: (require, constant). Loading rest-client pulls in
/// http-cookie, whose circular require warns under `-w` in every app
/// that has the gem installed, used or not.
pub(crate) const DEMAND_REQUIRES: &[(&str, &str)] = &[("rest-client", "RestClient"), ("elastic_apm", "ElasticAPM")];

/// Whether the compiler's own tables have a Ruby-family tree declare or
/// require `gem`: the runtime gems the emitted Gemfile declares on demand
/// and the guarded requires in `gem_facades.rb`. Extensions validate a
/// `rails_dependent` fact against it (`crate::extensions`).
pub(crate) fn compiler_tree_loads_gem(gem: &str) -> bool {
    compiler_runtime_gems().iter().any(|(_, loaded)| *loaded == gem) || GEM_REQUIRES.contains(&gem)
}

/// The compiler's own runtime gem rows; an app's extensions append to them
/// in `apply_runtime_gem_wiring`.
pub(crate) fn compiler_runtime_gems() -> &'static [(Marker, &'static str)] {
    // Kept in step with the guarded list in `runtime/gem_facades.rb`:
    // (constant an emitted body names, gem that defines it). Only gems
    // whose absence is a RUNTIME error belong here — the list is the
    // façade's, not a survey of what an app might like.
    const RUNTIME_GEMS: [(Marker, &str); 25] = [
        (Marker::Constant("BCrypt"), "bcrypt"),
        (Marker::Constant("HTMLEntities"), "htmlentities"),
        (Marker::Constant("ROTP"), "rotp"),
        (Marker::Constant("Markly"), "markly"),
        (Marker::Constant("Commonmarker"), "commonmarker"),
        (Marker::Constant("Nokogiri"), "nokogiri"),
        (Marker::Constant("Parslet"), "parslet"),
        (Marker::Constant("TypeID"), "typeid"),
        (Marker::Constant("RQRCode"), "rqrcode"),
        (Marker::Constant("RQRCodeCore"), "rqrcode_core"),
        // campfire's web push builds one `Net::HTTP::Persistent` pool
        // per process (`WebPush::Pool`), and the gem went undeclared
        // exactly as bcrypt did — the façade names the constant and
        // nothing put it in the Gemfile.
        //
        // ITS ABSENCE IS FATAL TO A WRITE, not to a feature, and that
        // is why it belongs in this table rather than in a ledger. The
        // job adapter is `:inline`, so `Room::MessagePusher` runs
        // INSIDE the message-create request: with no gem, every
        // `POST /rooms/N/messages` answers 500 with
        // `uninitialized constant Net::HTTP::Persistent`. Found by the
        // ruby socket lane, which is the first harness to post a
        // message on this tree WITHOUT the walk's stubs spliced in —
        // the HTTP bench lanes only GET, and campfire-compare stubs the
        // pusher. A gem the app depends on is not a modeling gap; it is
        // a line in the Gemfile.
        (Marker::Constant("Net::HTTP::Persistent"), "net-http-persistent"),
        // The thread pools that same `WebPush::Pool` posts to. Ported for
        // spinel (runtime/spinel/concurrent.rb) and swapped for the gem on
        // this lane (`ruby_runtime_files`), so the tree must declare it —
        // it is only ever in the lock today as sentry-ruby's dependency.
        (Marker::Constant("Concurrent"), "concurrent-ruby"),
        // The gem those pools deliver through. `WEB_PUSH_STUB_REOPEN`
        // was written to sit on top of it — `alias_method
        // :payload_send_without_stub, :payload_send if method_defined?` —
        // and nothing had ever put the gem underneath: the app's
        // `WebPush::Pool#deliver` rescues `WebPush::ExpiredSubscription`,
        // a constant only the gem defines on this lane, and its suite
        // raises one through the slot. With the gem in the tree an
        // unstubbed `payload_send` is a real delivery.
        (Marker::Constant("WebPush"), "web-push"),
        // campfire's message helpers rescue through
        // `Sentry.capture_exception`, and the gem was standing in
        // `scripts/campfire-walk-stubs.rb` as a hand-written module. A
        // gem the app depends on is not a modeling gap; it is a line in
        // the Gemfile, which is what this table is for. MEASURED: the
        // real gem loads in the emitted tree under CRuby.
        //
        (Marker::Constant("Sentry"), "sentry-ruby"),
        // `platform_agent` USED TO BE HERE, and is deliberately not any
        // more: `runtime/ruby/user_agent.rb` ports it, along with the
        // `useragent` it delegates to. Requiring the gem beside the port
        // would redefine `PlatformAgent` and `UserAgent` on the CRuby
        // tree alone, so the ruby lane would be running the GEM while
        // every strict lane ran the port — and a sibling lane is
        // evidence only if it runs the same code. The port's own suite
        // (`runtime/ruby/test/user_agent_test.rb`) compares it against
        // the real gems instead, which is where that comparison belongs.
        // rails-html-sanitizer, and it is the first entry whose demand
        // is OURS rather than the app's: no campfire file names
        // `Rails::HTML`, but `ActionView::ViewHelpers.sanitize` is what
        // a bare `sanitize` / `strip_tags` / `auto_link` in an app body
        // lowers to, and the CRuby overlay serves those from the real
        // gem. So the marker is the emitted CALL, the same shape the
        // test table already uses for mocha.
        //
        // A Gemfile line rather than a port because the safe-list pass
        // is HTML5 tree construction, not filtering — see the header of
        // ruby_overlay/runtime/action_view_sanitize.rb. Its closure is
        // loofah + crass + nokogiri, and nokogiri is already declared by
        // an entry above for any app that reaches this one.
        (
            Marker::AnyText(&[
                "ActionView::ViewHelpers.sanitize(",
                "ActionView::ViewHelpers.strip_tags(",
                "ActionView::ViewHelpers.auto_link(",
            ]),
            "rails-html-sanitizer",
        ),
        // A declared variant is an image processor's demand: the
        // reopen `apply_image_processor_wiring` swaps in requires
        // "vips", and the marker is the constructor call every
        // `attachable.variant …` lowers to. The gem needs libvips
        // installed, which the app's own Gemfile (image_processing →
        // ruby-vips) already asked of the machine.
        (Marker::AnyText(&[VARIATION_MARKER]), "ruby-vips"),
        // The HTTP client an app calls. The real gem runs on this lane;
        // the strict targets refuse it (`lower::ruby_family_gems`). `Constant`
        // matches `Faraday.new` / `Faraday::…` / `rescue Faraday`; the
        // `AnyText` needles catch bare value uses (`CLIENT = Faraday`,
        // `register(Faraday)`) that `names_constant` skips — without
        // matching identifiers like `FaradayHolder`.
        (Marker::Constant("Faraday"), "faraday"),
        (Marker::AnyText(&["= Faraday", "(Faraday)", "Faraday,", ", Faraday"]), "faraday"),
        // Its middleware gems. `request :multipart` / `request :retry`
        // name no constant, and without the gem Faraday raises at the
        // first connection that registers them.
        (
            Marker::AnyText(&[
                "Faraday::Multipart",
                "Faraday::UploadIO",
                "Faraday::FilePart",
                "Faraday::ParamPart",
                "Faraday::CompositeReadIO",
                "request :multipart",
                "request(:multipart",
            ]),
            "faraday-multipart",
        ),
        (
            Marker::AnyText(&["Faraday::Retry", "request :retry", "request(:retry"]),
            "faraday-retry",
        ),
        // Retryable, JWT and ElasticAPM, as Faraday: the real gems on this lane,
        // refused on the strict targets (`lower::ruby_family_gems`).
        (Marker::Constant("Retryable"), "retryable"),
        (Marker::Constant("JWT"), "jwt"),
        // lobsters' `Rack::MiniProfiler.authorize_request` for every
        // moderator: the real gem on this lane, its package
        // (`GEM_PACKAGES`) on spinel.
        (Marker::Constant("Rack::MiniProfiler"), "rack-mini-profiler"),
        // RestClient, the same way.
        (Marker::Constant("RestClient"), "rest-client"),
        (Marker::Constant("ElasticAPM"), "elastic-apm"),
    ];
    &RUNTIME_GEMS
}

/// The `remote:` of every `PATH` section in a `Gemfile.lock`, lock
/// order — the directories of the app's path-sourced gems
/// (`gem "billing", path: "lib/billing"` locks as `remote: lib/billing`),
/// as written: relative to the lockfile unless the Gemfile gave an
/// absolute path.
pub fn lock_path_remotes(text: &str) -> Vec<String> {
    let mut remotes = Vec::new();
    let mut in_path = false;
    for line in text.lines() {
        if !line.starts_with(' ') {
            in_path = line.trim() == "PATH";
        } else if in_path {
            if let Some(remote) = line.strip_prefix("  remote: ") {
                remotes.push(remote.trim().to_string());
            }
        }
    }
    remotes
}

/// A parsed `Gemfile.lock`: every resolved spec and the direct
/// dependencies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    /// `GEM` / `PATH` / `GIT` specs: `(name, version)`, lock order.
    pub specs: Vec<(String, String)>,
    /// The `DEPENDENCIES` section — what the Gemfile names, lock order.
    pub dependencies: Vec<String>,
    /// The specs resolved from a `PATH` source: gems that live in this
    /// repository. Their source is part of the tree, so they are
    /// analyzed like the rest of the app rather than being a boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_repo: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<String>,
    /// Git repositories and locked revisions, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git: Vec<GitSource>,
    /// Runtime dependencies of each resolved spec, in lockfile order.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub spec_deps: std::collections::BTreeMap<String, Vec<String>>,
    /// The `PLATFORMS` section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    /// The app Gemfile's `require:` per gem (`gem "svg-graph", require:
    /// "SVG/Graph/TimeSeries"`): what Bundler loads in place of the gem's
    /// name. An empty list is `require: false`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub gemfile_requires: std::collections::BTreeMap<String, Vec<String>>,
}

/// Every `gem "<name>", …, require: <String | [String] | false>` in a
/// Gemfile, at its top level or inside a block (`group :x do … end`).
pub fn gemfile_requires(source: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    struct Gems(std::collections::BTreeMap<String, Vec<String>>);
    fn string(node: &ruby_prism::Node<'_>) -> Option<String> {
        node.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
    }
    impl<'pr> ruby_prism::Visit<'pr> for Gems {
        fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
            if call.receiver().is_none() && call.name().as_slice() == b"gem" {
                let args: Vec<ruby_prism::Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                if let Some(name) = args.first().and_then(string) {
                    for arg in &args[1..] {
                        let Some(hash) = arg.as_keyword_hash_node() else { continue };
                        for element in hash.elements().iter() {
                            let Some(pair) = element.as_assoc_node() else { continue };
                            let is_require = pair.key().as_symbol_node().is_some_and(|k| k.unescaped() == b"require");
                            if !is_require {
                                continue;
                            }
                            let value = pair.value();
                            let features = if value.as_false_node().is_some() {
                                Some(Vec::new())
                            } else if let Some(one) = string(&value) {
                                Some(vec![one])
                            } else {
                                value.as_array_node().and_then(|a| a.elements().iter().map(|e| string(&e)).collect())
                            };
                            if let Some(features) = features {
                                self.0.insert(name.clone(), features);
                            }
                        }
                    }
                }
            }
            ruby_prism::visit_call_node(self, call);
        }
    }
    let parsed = ruby_prism::parse(source.as_bytes());
    let mut gems = Gems(std::collections::BTreeMap::new());
    ruby_prism::Visit::visit(&mut gems, &parsed.node());
    gems.0
}

/// One `GIT` source from a Bundler lockfile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GitSource {
    pub remote: String,
    pub revision: String,
    pub specs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub submodules: bool,
}

impl GitSource {
    /// Bundler's installed checkout directory for this source.
    pub fn checkout_dir(&self) -> String {
        let base = self.remote.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or("");
        let base = base.strip_suffix(".git").unwrap_or(base);
        let revision: String = self.revision.chars().take(12).collect();
        format!("{base}-{revision}")
    }
}

impl Lockfile {
    /// Parse the Bundler lockfile format: section headers at column 0,
    /// specs at four spaces under `specs:`, dependencies at two spaces
    /// under `DEPENDENCIES`. Six-space lines name each spec's runtime
    /// dependencies. An unfamiliar section is ignored; malformed spec
    /// lines retain the existing tolerant parser behavior.
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
                if section == "GIT" {
                    lock.git.push(GitSource::default());
                }
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let body = line.trim();
            match section {
                "GEM" | "PATH" | "GIT" | "PLUGIN SOURCE" => {
                    if indent == 2 {
                        in_specs = body == "specs:";
                        if section == "GIT" {
                            let git = lock.git.last_mut().expect("GIT section");
                            if let Some(remote) = body.strip_prefix("remote:") {
                                git.remote = remote.trim().to_string();
                            } else if let Some(revision) = body.strip_prefix("revision:") {
                                git.revision = revision.trim().to_string();
                            } else if let Some(glob) = body.strip_prefix("glob:") {
                                git.glob = Some(glob.trim().to_string());
                            } else if let Some(submodules) = body.strip_prefix("submodules:") {
                                git.submodules = submodules.trim() == "true";
                            }
                        }
                    } else if indent == 4 && in_specs {
                        // `rails (8.0.2)` — the version in parens; a
                        // platform suffix (`nokogiri (1.18.3-arm64-darwin)`)
                        // stays with the version.
                        let (name, rest) = body.split_once(' ').unwrap_or((body, ""));
                        let version = rest.trim().trim_start_matches('(').trim_end_matches(')');
                        lock.specs.push((name.to_string(), version.to_string()));
                        if section == "PATH" {
                            lock.in_repo.push(name.to_string());
                        } else if section == "GIT" {
                            lock.git.last_mut().expect("GIT section").specs.push(name.to_string());
                        }
                    } else if indent == 6 && in_specs {
                        if let Some((spec, _)) = lock.specs.last() {
                            let dependency = body.split(' ').next().unwrap_or(body);
                            lock.spec_deps.entry(spec.clone()).or_default().push(dependency.to_string());
                        }
                    }
                }
                "PLATFORMS" => {
                    if indent == 2 {
                        lock.platforms.push(body.to_string());
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

    /// Does `name` live in this repository (a `PATH` source)?
    pub fn is_in_repo(&self, name: &str) -> bool {
        self.in_repo.iter().any(|n| n == name)
    }

    pub fn is_source(&self, name: &str) -> bool {
        self.source.iter().any(|n| n == name)
    }

    /// The repository and revision that provide `name`, if it is Git-backed.
    pub fn git_source_of(&self, name: &str) -> Option<&GitSource> {
        self.git.iter().find(|source| source.specs.iter().any(|spec| spec == name))
    }

    /// Git specs in the requested dependency closure, dependencies first.
    pub fn git_specs_in_closure(&self, roots: &[&str]) -> Vec<String> {
        let mut seen = crate::det::HashSet::default();
        let mut out = Vec::new();
        for root in roots {
            let mut pending = vec![(*root, false)];
            while let Some((name, expanded)) = pending.pop() {
                if expanded {
                    if self.is_from_git(name) {
                        out.push(name.to_string());
                    }
                } else if seen.insert(name) {
                    pending.push((name, true));
                    if let Some(dependencies) = self.spec_deps.get(name) {
                        pending.extend(dependencies.iter().rev().map(|dependency| (dependency.as_str(), false)));
                    }
                }
            }
        }
        out
    }

    /// Is `name` resolved from a `GIT` source?
    pub fn is_from_git(&self, name: &str) -> bool {
        self.git_source_of(name).is_some()
    }

    pub fn version_of(&self, name: &str) -> Option<&str> {
        self.specs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    // Not cut at the first `-`: `1.2.3-alpha` is a prerelease, only a listed platform is a suffix.
    pub fn release_version_of(&self, name: &str) -> Option<&str> {
        let version = self.version_of(name)?;
        Some(self.platforms.iter().find_map(|p| version.strip_suffix(p.as_str())?.strip_suffix('-')).unwrap_or(version))
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
    /// A component of this repository, locked from a `PATH` source. Its
    /// code is in the tree under analysis, so it is not a gem the
    /// analyzer has to model: a dispatch that fails on its constants is
    /// a fact about the analysis of that code, and is reported as such.
    InRepo,
    Source,
}

impl GemFate {
    pub fn label(self) -> &'static str {
        match self {
            GemFate::Framework => "framework",
            GemFate::Stdlib => "stdlib",
            GemFate::Modeled => "modeled",
            GemFate::Infrastructure => "infrastructure",
            GemFate::Unknown => "unknown",
            GemFate::InRepo => "in-repo",
            GemFate::Source => "source",
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
    /// Every resolved spec name, direct or not: which gem owns a
    /// constant is a question about all of them.
    #[serde(default)]
    pub resolved: Vec<String>,
}

impl GemCensus {
    pub fn of(lock: &Lockfile) -> GemCensus {
        let gems: Vec<GemEntry> = lock
            .dependencies
            .iter()
            .map(|name| GemEntry {
                name: name.clone(),
                version: lock.version_of(name).map(|v| v.to_string()),
                fate: if lock.is_in_repo(name) {
                    GemFate::InRepo
                } else if lock.is_source(name) {
                    GemFate::Source
                } else {
                    fate_of(name)
                },
            })
            .collect();
        let transitive = lock.specs.len().saturating_sub(gems.len());
        GemCensus { gems, transitive, resolved: lock.specs.iter().map(|(n, _)| n.clone()).collect() }
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
            GemFate::InRepo,
            GemFate::Source,
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
    ("acts_as_tenant", GemFate::Modeled), // ingest/acts_as_tenant: acts_as_tenant -> belongs_to + dynamic default_scope + create stamp; validates_uniqueness_to_tenant; ActsAsTenant as CurrentAttributes (current_tenant, with_tenant, without_tenant); set_current_tenant_through_filter (not has_global_records/through/polymorphic/require_tenant/subdomain/immutability)
    ("acts_as_votable", GemFate::Modeled), // ingest/acts_as_votable: ActsAsVotable::Vote on `votes`; acts_as_votable/acts_as_voter macros -> votes_for/votes + vote_by/liked_by/unvote_by/update_cached_votes (not vote_up_for/get_voted/association extensions)
    ("addressable", GemFate::Modeled), // catalog/gems: Addressable::URI
    ("bcrypt", GemFate::Modeled),      // catalog/gems: BCrypt::*; has_secure_password
    ("commonmarker", GemFate::Modeled), // catalog/gems: Commonmarker, Commonmarker::Node
    ("devise", GemFate::Modeled), // helpers; devise_for = static 4 mappings (not model-module-driven); no Warden route guards
    ("discard", GemFate::Modeled),
    ("elastic-apm", GemFate::Modeled), // catalog/gems: ElasticAPM, SpanHelpers; the real gem on ruby and jruby, lower/ruby_family_gems refuses every other target     // ingest/discard: include Discard::Model -> scopes + predicates + discard/undiscard (not discard_all/undiscard_all); Relation#unscope(where:)
    ("faker", GemFate::Modeled),       // catalog/gems: Faker::*
    ("friendly_id", GemFate::Modeled), // lower/friendly_id + registry: friendly_id/friendly/to_param, Relation#friendly_find (slugged [+history] finder only; slug generation on save, :scoped/:finders/:simple_i18n unmodeled)
    ("geared_pagination", GemFate::Modeled), // registry/controllers: set_page_and_extract_portion_from
    ("faraday", GemFate::Modeled),     // catalog/gems: Faraday; the real gem on ruby and jruby, lower/ruby_family_gems refuses every other target
    ("faraday-multipart", GemFate::Modeled), // catalog/gems: Faraday::Multipart, as faraday
    ("faraday-retry", GemFate::Modeled), // request :retry, as faraday
    ("htmlentities", GemFate::Modeled), // catalog/gems: HTMLEntities
    ("image_processing", GemFate::Modeled), // active_storage variants seam
    ("inertia_rails", GemFate::Modeled), // ingest/inertia: render inertia:, inertia_share; runtime action_controller/inertia
    ("invisible_captcha", GemFate::Modeled), // before_action + spam? on subtitle honeypot only
    ("jwt", GemFate::Modeled),         // catalog/gems: JWT; the real gem on ruby and jruby, lower/ruby_family_gems refuses every other target
    ("kaminari", GemFate::Modeled),    // Relation#page / per / paginate
    ("mail", GemFate::Modeled),        // catalog/gems: Mail::Address; ActionMailer
    ("mocha", GemFate::Modeled),       // lower/mocha bridge
    ("nokogiri", GemFate::Modeled),    // catalog/gems: Nokogiri
    ("oj", GemFate::Modeled),          // ingest/oj + lower/oj: under Oj.mimic_JSON, load/generate on ruby, jruby and spinel; dump and every other target refused
    ("pagy", GemFate::Modeled),        // registry/controllers + analyze/body/pagy + runtime action_controller/pagy.rb: pagy(scope[, limit: n]) offset paginator, Pagy#series_nav/info_tag/readers (English); other paginators/options/helpers refused
    ("pdf-reader", GemFate::Modeled),  // catalog/gems: PDF::Reader
    ("platform_agent", GemFate::Modeled), // useragent port
    ("pretender", GemFate::Modeled), // impersonates with local current_* → true_* wrap + impersonate_*; inherited-only stays unsupported
    ("pushover", GemFate::Modeled),    // catalog/gems: Pushover
    ("rack-mini-profiler", GemFate::Modeled), // catalog/gems: Rack::MiniProfiler
    ("representable", GemFate::Modeled), // ingest/representable: Decorator + JSON, property/collection subset
    ("rest-client", GemFate::Modeled), // catalog/gems: RestClient; the real gem on ruby and jruby, lower/ruby_family_gems refuses every other target
    ("retryable", GemFate::Modeled),   // catalog/gems: Retryable; the real gem on ruby and jruby, lower/ruby_family_gems refuses every other target
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
    // Not rack-attack, rack-cors or rack-protection: a middleware that
    // throttles a request, writes its CORS headers or refuses one it reads
    // as an attack changes what the app answers, and nothing in the app
    // calls it for a dispatch failure to show that it is missing.
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
    ("doorkeeper", &["doorkeeper_authorize!", "doorkeeper_token", "current_resource_owner"]),
    ("enumerize", &["enumerize"]),
    ("geocoder", &["geocoded_by", "reverse_geocoded_by", "geocode", "reverse_geocode", "near", "distance_to", "distance_from", "bearing_to", "within_bounding_box"]),
    ("money-rails", &["monetize"]),
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

/// Class-body DSLs whose methods are named by the author's own
/// arguments (`event :publish` → `publish!`, `may_publish?`), so no
/// fixed `SURFACES` list can name them. Attribution reads the names off
/// the declaration instead (`analyze::attribution::generated_methods`).
/// Each DSL lists the gems that provide it, most specific first.
pub(crate) const GENERATING_DSLS: &[(&str, &[&str])] = &[
    ("aasm", &["aasm"]),
    ("state_machine", &["state_machines-activerecord", "state_machines", "state_machine"]),
];

/// The gem in `lock` that provides a generating DSL, if any does.
pub(crate) fn gem_providing_dsl(lock: &Lockfile, dsl: &str) -> Option<&'static str> {
    GENERATING_DSLS
        .iter()
        .filter(|(name, _)| *name == dsl)
        .flat_map(|(_, gems)| gems.iter())
        .find(|gem| lock.has(gem))
        .copied()
}

pub fn claims_surface(gem: &str) -> bool {
    SURFACES.iter().any(|(name, _)| *name == gem)
}

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
        ("caxlsx", "Axlsx"),
        ("combine_pdf", "CombinePDF"),
        ("duckdb", "DuckDB"),
        ("elastic-apm", "ElasticAPM"),
        ("fast_excel", "FastExcel"),
        ("graphql", "GraphQL"),
        ("htmlentities", "HTMLEntities"),
        ("http", "HTTP"),
        ("i18n", "I18n"),
        ("jwt", "JWT"),
        ("mcp", "MCP"),
        ("mini_magick", "MiniMagick"),
        ("net-http-persistent", "Net"),
        ("nokogiri", "Nokogiri"),
        ("oj", "Oj"),
        ("pdf-reader", "PDF"),
        ("pg", "PG"),
        ("polars-df", "Polars"),
        ("rack-attack", "Rack"),
        ("rqrcode", "RQRCode"),
        ("rotp", "ROTP"),
        ("ruby-vips", "Vips"),
        ("ruby_llm", "RubyLLM"),
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

/// Whether `boot.rb` should `require` a locked unknown gem by its own name
/// once the emitted program names that gem's namespace.
///
/// A normal gem's entry file is `lib/<gem>.rb`, and the constant lowercases
/// to that name (`duckdb` → `DuckDB`). The same file exists for three
/// irregular constants: `caxlsx` (`lib/caxlsx.rb`), `polars-df`
/// (`lib/polars-df.rb`), and `ruby_llm` (`lib/ruby_llm.rb`). Requiring the
/// lowercased constant (`axlsx`, `polars`, `rubyllm`) is not that entry.
///
/// `rubyzip` is not in this set. `require "rubyzip"` is not a file; the gem
/// loads as `zip`.
pub fn loads_by_gem_name(gem: &str) -> bool {
    if namespace_of(gem).eq_ignore_ascii_case(gem) {
        return true;
    }
    matches!(gem, "caxlsx" | "polars-df" | "ruby_llm")
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

/// The resolved specs whose namespace `constant_path` sits under. A gem's
/// own spelling nests at its dashes (`rack-attack` → `Rack::Attack`); its
/// conventional namespace and first segment count too, below that. The
/// longest match wins, and ties at the same rank are all returned.
pub fn lock_gems_owning_constant<'a>(lock: &'a Lockfile, constant_path: &str) -> Vec<&'a str> {
    let path = constant_path.trim_start_matches("::");
    // Not case-sensitive: `OmniAuth` is the omniauth gem's, `PgHero` pghero's.
    let under = |ns: &str| {
        path.len() >= ns.len()
            && path.is_char_boundary(ns.len())
            && path[..ns.len()].eq_ignore_ascii_case(ns)
            && (path.len() == ns.len() || path[ns.len()..].starts_with("::"))
    };
    let mut best: Vec<&str> = Vec::new();
    let mut best_rank = (0usize, false);
    for (name, _) in &lock.specs {
        let nested = name.split('-').map(camelize).collect::<Vec<_>>().join("::");
        let rank = if under(&nested) {
            (nested.len(), true)
        } else if let Some(ns) = namespace_candidates(name).into_iter().filter(|ns| under(ns)).max_by_key(|ns| ns.len()) {
            (ns.len(), false)
        } else {
            continue;
        };
        if rank > best_rank {
            best_rank = rank;
            best.clear();
        }
        if rank == best_rank && !best.contains(&name.as_str()) {
            best.push(name.as_str());
        }
    }
    best
}

/// Namespace candidates of one immutable lockfile. Repeated constant asks
/// inspect only gems sharing the path's first segment, without rebuilding
/// every gem's candidate spellings. Bucket order is lock order.
pub(crate) struct LockGemIndex<'a> {
    entries: Vec<LockGemNamespaces<'a>>,
    by_head: crate::det::HashMap<String, Vec<usize>>,
}

struct LockGemNamespaces<'a> {
    name: &'a str,
    nested: String,
    alternatives: Vec<String>,
}

impl<'a> LockGemIndex<'a> {
    pub(crate) fn new(lock: &'a Lockfile) -> Self {
        let entries: Vec<_> = lock.specs.iter().map(|(name, _)| LockGemNamespaces {
            name,
            nested: name.split('-').map(camelize).collect::<Vec<_>>().join("::"),
            alternatives: namespace_candidates(name),
        }).collect();
        let mut by_head: crate::det::HashMap<String, Vec<usize>> = Default::default();
        for (i, entry) in entries.iter().enumerate() {
            let mut heads = Vec::new();
            for ns in std::iter::once(&entry.nested).chain(&entry.alternatives) {
                let head = ns.split("::").next().unwrap_or("").to_ascii_lowercase();
                if !heads.contains(&head) {
                    by_head.entry(head.clone()).or_default().push(i);
                    heads.push(head);
                }
            }
        }
        Self { entries, by_head }
    }

    pub(crate) fn owners(&self, constant_path: &str) -> Vec<&'a str> {
        let path = constant_path.trim_start_matches("::");
        let head = path.split("::").next().unwrap_or("").to_ascii_lowercase();
        let under = |ns: &str| {
            path.len() >= ns.len()
                && path.is_char_boundary(ns.len())
                && path[..ns.len()].eq_ignore_ascii_case(ns)
                && (path.len() == ns.len() || path[ns.len()..].starts_with("::"))
        };
        let mut best = Vec::new();
        let mut best_rank = (0usize, false);
        for &i in self.by_head.get(&head).into_iter().flatten() {
            let entry = &self.entries[i];
            // Nested spelling wins over this gem's alternatives, even if
            // an alternative is longer, exactly as in the original scan.
            let rank = if under(&entry.nested) {
                (entry.nested.len(), true)
            } else if let Some(ns) = entry.alternatives.iter().filter(|ns| under(ns)).max_by_key(|ns| ns.len()) {
                (ns.len(), false)
            } else {
                continue;
            };
            if rank > best_rank {
                best_rank = rank;
                best.clear();
            }
            if rank == best_rank && !best.contains(&entry.name) {
                best.push(entry.name);
            }
        }
        best
    }
}

/// The unique resolved unknown gem whose namespace `constant_path`
/// sits under, if any. Ambiguous prefixes must not pick lockfile order.
pub fn gem_owning_constant<'a>(census: &'a GemCensus, constant_path: &str) -> Option<&'a str> {
    let owners = gems_owning_constant(census, constant_path);
    (owners.len() == 1).then(|| owners[0])
}

/// Literal external-gem includes whose defining file also has a matching
/// top-level `require`. The lockfile identifies the gem and source names its
/// feature; other unresolved includes remain unsupported.
pub fn statically_resolved_external_includes(
    app: &crate::App,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let Some(lock) = app.gem_lock.as_ref() else {
        return Default::default();
    };
    let census = GemCensus::of(lock);
    let mut resolved = std::collections::BTreeMap::new();
    for class in &app.library_classes {
        let source = source_for_library_class(app, class);
        let Some(source) = source else { continue };
        for included in &class.includes {
            let Some(gem) = gem_owning_constant(&census, included.0.as_str()) else {
                continue;
            };
            // Fail-closed: Git-sourced gems have no Gemfile pin path that
            // preserves the git remote — keep their includes unresolved.
            if lock.is_from_git(gem) {
                continue;
            }
            // A gem a project extension says reaches Rails is never
            // required natively (`crate::extensions`).
            if app.extensions.is_rails_dependent(gem) {
                continue;
            }
            let gem_name = gem.to_ascii_lowercase();
            let namespace = namespace_of(gem).to_ascii_lowercase();
            let requires: Vec<String> = literal_file_level_requires(&source.text)
                .into_iter()
                .filter(|feature| {
                    feature
                        .split('/')
                        .next()
                        .is_some_and(|head| head == namespace || head == gem_name)
                })
                .collect();
            if !requires.is_empty() {
                resolved.insert(
                    format!("{} includes unresolved {}", class.name.0, included.0),
                    requires,
                );
            }
        }
    }
    resolved
}

/// Defining source for a library class when scanning for literal requires.
/// Prefer a real superclass / method / body span; include-only classes with
/// no superclass leave `parent_span` synthetic (`file.0 == 0`), so fall back
/// to a source file that declares the class or module by name.
fn source_for_library_class<'a>(
    app: &'a crate::App,
    class: &crate::dialect::LibraryClass,
) -> Option<&'a crate::span::SourceFile> {
    let from_file = |file: crate::span::FileId| {
        file.0
            .checked_sub(1)
            .and_then(|index| app.sources.get(index as usize))
    };
    from_file(class.parent_span.file)
        .or_else(|| {
            class
                .methods
                .iter()
                .find_map(|method| from_file(method.name_span.file))
        })
        .or_else(|| {
            class
                .unknown_calls
                .iter()
                .find_map(|call| from_file(call.span.file))
        })
        .or_else(|| {
            class
                .constants
                .iter()
                .find_map(|(_, expr)| from_file(expr.span.file))
        })
        .or_else(|| {
            class
                .class_ivar_initializers
                .iter()
                .find_map(|expr| from_file(expr.span.file))
        })
        .or_else(|| source_declaring_class(app, class.name.0.as_str()))
}

fn source_declaring_class<'a>(
    app: &'a crate::App,
    class_name: &str,
) -> Option<&'a crate::span::SourceFile> {
    let leaf = class_name.rsplit("::").next().unwrap_or(class_name);
    let candidates = [class_name, leaf];
    app.sources.iter().find(|source| {
        source.text.lines().any(|line| {
            let trimmed = line.trim_start();
            candidates.iter().any(|name| {
                for keyword in ["class ", "module "] {
                    let Some(rest) = trimmed.strip_prefix(keyword) else {
                        continue;
                    };
                    if rest == *name {
                        return true;
                    }
                    if let Some(after) = rest.strip_prefix(name) {
                        let boundary = after
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != ':');
                        if boundary {
                            return true;
                        }
                    }
                }
                false
            })
        })
    })
}

/// Namespaces owned by unknown gems that are pinned in this app's lockfile.
/// Ruby project emission uses these only when it also wires the exact gems.
/// A gem a project extension says reaches Rails is not wired, so its
/// constants stay refused (`crate::extensions`).
pub fn locked_external_gem_namespaces(app: &crate::App) -> std::collections::BTreeSet<String> {
    let Some(lock) = app.gem_lock.as_ref() else {
        return Default::default();
    };
    let mut namespaces: std::collections::BTreeSet<String> = GemCensus::of(lock)
        .unknown()
        .filter(|gem| !lock.is_from_git(&gem.name) && !app.extensions.is_rails_dependent(&gem.name))
        .map(|gem| namespace_of(&gem.name))
        .collect();
    namespaces.extend(native_extension_gem_namespaces(app));
    namespaces
}

/// Native extension constants available on both Ruby-family targets.
/// A lock entry alone does not add a namespace to this set.
pub fn native_extension_gem_namespaces(app: &crate::App) -> std::collections::BTreeSet<String> {
    let Some(lock) = app.gem_lock.as_ref() else { return Default::default() };
    let mut names = std::collections::BTreeSet::new();
    let gems = app.extensions.runtime_gems.iter().map(|row| row.gem.as_str())
        .chain(app.gem_boundary.loaded.iter().map(String::as_str));
    for gem in gems.filter(|gem| lock.has(gem) && !lock.is_in_repo(gem) && !app.extensions.is_rails_dependent(gem)) {
        match app.gem_boundary.namespaces.get(gem) {
            Some(owned) => names.extend(owned.iter().cloned()),
            None => { names.insert(namespace_of(gem)); }
        }
        for row in app.extensions.runtime_gems.iter().filter(|row| row.gem == gem) {
            if let crate::extensions::Demand::Constant(name) = &row.demand {
                names.insert(name.clone());
            }
        }
    }
    names
}

/// The extension fact that refuses the gem owning this constant.
/// RBI ownership takes precedence over a gem-name namespace estimate.
pub fn rails_dependent_owner<'a>(
    app: &'a crate::App,
    detail: &str,
) -> Option<&'a crate::extensions::RailsDependentGem> {
    let app_owned = AppConstants::of(app);
    app.extensions
        .rails_dependent
        .iter()
        .find(|row| extension_owns_constant(app, &app_owned, &row.gem, detail))
}

/// Constants the app itself defines, indexed once so that asking whether a
/// gem owns a path does not rescan every model, controller and class.
struct AppConstants {
    /// Names of models, controllers and library classes.
    classes: crate::det::HashSet<String>,
    /// `Owner::NAME` for each constant an app class or model body assigns.
    values: crate::det::HashSet<String>,
}

impl AppConstants {
    fn of(app: &crate::App) -> Self {
        let mut classes = crate::det::HashSet::default();
        let mut values = crate::det::HashSet::default();
        classes.extend(app.models.iter().map(|model| model.name.0.as_str().to_string()));
        classes.extend(app.controllers.iter().map(|controller| controller.name.0.as_str().to_string()));
        classes.extend(app.library_classes.iter().map(|class| class.name.0.as_str().to_string()));
        for class in app.library_classes.iter().chain(app.rails_application.iter()) {
            for (name, _) in &class.constants {
                values.insert(format!("{}::{}", class.name.0.as_str(), name.as_str()));
            }
        }
        for model in &app.models {
            for item in &model.body {
                if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                    if let crate::expr::ExprNode::Assign {
                        target: crate::expr::LValue::Const { path: assigned }, ..
                    } = &*expr.node
                    {
                        if let [name] = assigned.as_slice() {
                            values.insert(format!("{}::{}", model.name.0.as_str(), name.as_str()));
                        }
                    }
                }
            }
        }
        AppConstants { classes, values }
    }

    fn defines(&self, path: &str) -> bool {
        self.classes.contains(path) || self.values.contains(path)
    }
}

fn extension_owns_constant(app: &crate::App, app_owned: &AppConstants, gem: &str, path: &str) -> bool {
    let path = path.trim_start_matches("::");
    if crate::gem_boundary::RUBY_CORE.contains(&path) {
        return false;
    }
    if app_owned.defines(path) {
        return false;
    }
    if app.gem_boundary.classes.get(&crate::ident::ClassId(crate::ident::Symbol::from(path)))
        .is_some_and(|class| class.gem == gem)
    {
        return true;
    }
    match app.gem_boundary.declares_path(gem, path) {
        Some(declares) => declares,
        None => {
            let namespace = namespace_of(gem);
            path == namespace || path.strip_prefix(&namespace).is_some_and(|rest| rest.starts_with("::"))
        }
    }
}

/// What owns a constant path; the same path recurs across many references.
#[derive(Clone, Copy)]
enum Owner<'a> {
    None,
    RailsDependent(&'a crate::extensions::RailsDependentGem),
    Native(&'a str),
}

/// Answers "which gem owns this path" once per distinct path.
struct Refuser<'a> {
    app: &'a crate::App,
    target: Option<&'a str>,
    app_owned: AppConstants,
    owners: std::cell::RefCell<crate::det::HashMap<String, Owner<'a>>>,
}

impl<'a> Refuser<'a> {
    fn owner(&self, path: &str) -> Owner<'a> {
        if let Some(owner) = self.owners.borrow().get(path) {
            return *owner;
        }
        let app = self.app;
        let owner = if let Some(row) = app.extensions.rails_dependent.iter()
            .find(|row| extension_owns_constant(app, &self.app_owned, &row.gem, path))
        {
            Owner::RailsDependent(row)
        } else if self.target.is_some_and(|target| !matches!(target, "ruby" | "jruby")) {
            app.extensions.runtime_gems.iter().map(|row| row.gem.as_str())
                .chain(app.gem_boundary.loaded.iter().map(String::as_str))
                .find(|gem| extension_owns_constant(app, &self.app_owned, gem, path))
                .map_or(Owner::None, Owner::Native)
        } else {
            Owner::None
        };
        self.owners.borrow_mut().insert(path.to_string(), owner);
        owner
    }
}

/// Extension-owned constants need both their declared type and a runtime.
/// `None` checks source analysis; a target also checks native availability.
pub(crate) fn extension_gem_refusals(app: &crate::App, target: Option<&str>) -> Vec<crate::diagnostic::Diagnostic> {
    use crate::diagnostic::Diagnostic;
    use crate::expr::{Expr, ExprNode};
    use crate::span::Span;
    if app.extensions.is_empty() || matches!(target, Some("blog" | "futamura")) {
        return Vec::new();
    }
    let native_target = target.is_some_and(|target| !matches!(target, "ruby" | "jruby"));
    if app.extensions.rails_dependent.is_empty()
        && (!native_target || (app.extensions.runtime_gems.is_empty() && app.gem_boundary.loaded.is_empty()))
    {
        return Vec::new();
    }
    fn refusal(refuser: &Refuser, path: &str, span: Span) -> Option<Diagnostic> {
        match refuser.owner(path) {
            Owner::RailsDependent(row) => Some(Diagnostic::unsupported(
                span,
                None,
                "gem_requires_rails",
                format!("{path}: a project extension marks gem {} rails_dependent: {}", row.gem, row.why),
            )),
            Owner::Native(owner) => {
                let target = refuser.target?;
                Some(Diagnostic::unsupported(
                    span,
                    Some(crate::ident::Symbol::from(target)),
                    "native_extension_gem",
                    format!("{path} is provided by gem {owner}; this target does not load native Ruby gems"),
                ))
            }
            Owner::None => None,
        }
    }
    fn visit(
        expr: &Expr, refuser: &Refuser,
        defined: &impl Fn(Span, &[crate::ident::Symbol]) -> bool, out: &mut Vec<Diagnostic>,
    ) {
        if let ExprNode::Const { path } = &*expr.node {
            if expr.decisions & crate::expr::GENERATED_CONST_REF == 0 && defined(expr.span, path) {
                return;
            }
            let path = path.iter().map(|part| part.as_str()).collect::<Vec<_>>().join("::");
            if let Some(diagnostic) = refusal(refuser, &path, expr.span) {
                out.push(diagnostic);
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, refuser, defined, out));
    }
    let refuser = Refuser { app, target, app_owned: AppConstants::of(app), owners: Default::default() };
    let mut out = Vec::new();
    let resolver = app.const_resolver.for_sources(&app.sources);
    let defined = |span, path: &[crate::ident::Symbol]| resolver.has_definition(span, path);
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, &refuser, &defined, &mut out));
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                visit(expr, &refuser, &defined, &mut out);
            }
        }
    }
    let defined_names = resolver.defined_names();
    for class in app.library_classes.iter().chain(app.rails_application.iter()) {
        for parent in class.parent.iter().chain(&class.includes) {
            if defined_names.contains(parent.0.as_str()) {
                continue;
            }
            if let Some(diagnostic) = refusal(&refuser, parent.0.as_str(), class.parent_span) {
                out.push(diagnostic);
            }
        }
    }
    let mut seen = crate::det::HashSet::default();
    out.retain(|diagnostic| seen.insert((
        diagnostic.span.file.0, diagnostic.span.start, diagnostic.span.end, diagnostic.code(),
    )));
    out
}

/// True when `detail` names a constant under `root` (`Rouge::X`), not a
/// longer identifier that merely ends with the root (`NotRouge::X`).
pub fn detail_names_external_namespace(detail: &str, root: &str) -> bool {
    if detail.trim_start_matches("::") == root {
        return true;
    }
    let needle = format!("{root}::");
    let bytes = detail.as_bytes();
    let mut start = 0;
    while let Some(rel) = detail[start..].find(&needle) {
        let abs = start + rel;
        let boundary_ok = abs == 0 || !is_ruby_ident_byte(bytes[abs - 1]);
        if boundary_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Hash index over external gem namespace roots: answers
/// `roots.iter().any(|r| detail_names_external_namespace(detail, r))` by
/// probing the roots a detail could contain instead of scanning every root,
/// memoized per detail (the same details repeat across an app).
#[derive(Default)]
pub struct ExternalRootIndex {
    roots: crate::det::HashSet<String>,
    memo: crate::det::HashMap<String, bool>,
}

impl ExternalRootIndex {
    pub fn new<'a>(namespaces: impl IntoIterator<Item = &'a String>) -> Self {
        Self { roots: namespaces.into_iter().cloned().collect(), memo: Default::default() }
    }

    /// True when some root satisfies `detail_names_external_namespace`.
    pub fn names_any(&mut self, detail: &str) -> bool {
        if self.roots.is_empty() {
            return false;
        }
        if let Some(&hit) = self.memo.get(detail) {
            return hit;
        }
        let hit = self.probe(detail);
        self.memo.insert(detail.to_string(), hit);
        hit
    }

    fn probe(&self, detail: &str) -> bool {
        if self.roots.contains(detail.trim_start_matches("::")) {
            return true;
        }
        let bytes = detail.as_bytes();
        // Every `::` is where a root-plus-`::` needle could end.
        let seps: Vec<usize> = (0..bytes.len().saturating_sub(1))
            .filter(|&i| bytes[i] == b':' && bytes[i + 1] == b':')
            .collect();
        for abs in 0..bytes.len() {
            // The path-start rule of `detail_names_external_namespace`.
            if !detail.is_char_boundary(abs) || (abs > 0 && is_ruby_ident_byte(bytes[abs - 1])) {
                continue;
            }
            let first = seps.partition_point(|&p| p < abs);
            if seps[first..].iter().any(|&p| self.roots.contains(&detail[abs..p])) {
                return true;
            }
        }
        false
    }
}

fn is_ruby_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Literal `require "…"` lines at file-level scope (nesting depth 0).
/// Column-zero indentation alone is not enough — a require inside a
/// multiline `if`/`class`/… must not hoist as an unconditional feature.
fn literal_file_level_requires(text: &str) -> Vec<String> {
    let mut depth = 0i32;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if opens_ruby_nest(trimmed) {
            depth += 1;
        }
        if depth == 0 {
            if let Some(feature) = literal_top_level_require(line) {
                out.push(feature);
            }
        }
        if closes_ruby_nest(trimmed) {
            depth = depth.saturating_sub(1);
        }
    }
    out
}

fn opens_ruby_nest(trimmed: &str) -> bool {
    // Modifier forms (`foo if bar`) do not start with these keywords.
    if trimmed == "begin"
        || trimmed.starts_with("begin ")
        || trimmed.starts_with("begin;")
        || trimmed.starts_with("begin#")
    {
        return true;
    }
    if trimmed == "do"
        || trimmed.starts_with("do ")
        || trimmed.starts_with("do|")
        || trimmed.starts_with("do;")
    {
        return true;
    }
    const OPENERS: &[&str] = &[
        "class ",
        "module ",
        "def ",
        "if ",
        "unless ",
        "case ",
        "while ",
        "until ",
    ];
    OPENERS.iter().any(|op| trimmed.starts_with(op))
        || trimmed.starts_with("class<<")
        || trimmed.starts_with("class <<")
}

fn closes_ruby_nest(trimmed: &str) -> bool {
    trimmed == "end"
        || trimmed.starts_with("end ")
        || trimmed.starts_with("end;")
        || trimmed.starts_with("end#")
}

fn literal_top_level_require(line: &str) -> Option<String> {
    if line.starts_with(char::is_whitespace) || !line.starts_with("require ") {
        return None;
    }
    let argument = line["require ".len()..].trim();
    let quote = argument.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let end = argument[1..].find(quote)? + 1;
    let trailing = argument[end + 1..].trim();
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return None;
    }
    let feature = &argument[1..end];
    (!feature.is_empty() && !feature.contains('#')).then(|| feature.to_string())
}

/// All candidates in the best namespace-matching tier. Full/irregular
/// names beat dashed-prefix guesses (`Alba`: alba, not alba-inertia).
pub fn gems_owning_constant<'a>(census: &'a GemCensus, constant_path: &str) -> Vec<&'a str> {
    gems_owning_constant_with(census, constant_path, &|_, _| None)
}

pub fn gem_owning_constant_with<'a>(census: &'a GemCensus, constant_path: &str, declares: &dyn Fn(&str, &str) -> Option<bool>) -> Option<&'a str> {
    let owners = gems_owning_constant_with(census, constant_path, declares);
    (owners.len() == 1).then(|| owners[0])
}

// Not left to the head match, which hands a framework's namespace to the framework.
const PARKED: &[(&str, &str)] = &[
    ("active_model_serializers", "ActiveModel::Serializer"),
    ("active_model_serializers", "ActiveModel::ArraySerializer"),
];

fn gems_parking_constant<'a>(census: &'a GemCensus, constant_path: &str) -> Vec<&'a str> {
    let path = constant_path.trim_start_matches("::");
    census
        .unknown()
        .filter(|g| {
            g.version.is_some()
                && PARKED.iter().any(|(gem, base)| {
                    *gem == g.name && path.strip_prefix(base).is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
                })
        })
        .map(|g| g.name.as_str())
        .collect()
}

// Not the namespace tiers: an app class nested in a gem's namespace (`Twilio::IncomingMessageService`) is the app's own.
pub fn gem_defining_constant<'a>(census: &'a GemCensus, constant_path: &str) -> Option<&'a str> {
    let path = constant_path.trim_start_matches("::");
    let owners: Vec<_> = census
        .unknown()
        .filter(|g| {
            g.version.is_some()
                && (namespace_of(&g.name) == path || PARKED.iter().any(|(gem, base)| *gem == g.name && *base == path))
        })
        .map(|g| g.name.as_str())
        .collect();
    (owners.len() == 1).then(|| owners[0])
}

pub(crate) fn gems_owning_constant_with<'a>(census: &'a GemCensus, constant_path: &str, declares: &dyn Fn(&str, &str) -> Option<bool>) -> Vec<&'a str> {
    let parked = gems_parking_constant(census, constant_path);
    if !parked.is_empty() {
        return parked;
    }
    let head = constant_path.trim_start_matches("::").split("::").next().unwrap_or(constant_path);
    let exact: Vec<_> = census
        .unknown()
        .filter(|g| g.version.is_some() && namespace_of(&g.name) == head)
        .map(|g| g.name.as_str())
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    if census.resolved.iter().any(|s| namespace_of(s) == head) {
        return Vec::new();
    }
    census.unknown()
        .filter(|g| declares(&g.name, constant_path) != Some(false))
        .filter(|g| g.version.is_some() && namespace_candidates(&g.name).iter().any(|c| c == head))
        .map(|g| g.name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rotp's façade only raises, so the gem is compiled from source; WebPush's
    /// also raises in gem_facades.rb, but the Spinel tree ports it, and a
    /// sourced gem would drop the port's stub slots with the façade.
    #[test]
    fn a_ported_namespace_is_not_a_stub_facade() {
        let stubs = stub_facade_namespaces();
        assert!(stubs.contains("ROTP"), "{stubs:?}");
        assert!(!stubs.contains("WebPush"), "{stubs:?}");
    }

    #[test]
    fn gemfile_requires_reads_each_spelling() {
        let found = gemfile_requires(
            "gem \"svg-graph\", require: \"SVG/Graph/TimeSeries\"\n\
             gem \"plain\"\n\
             group :development do\n  gem \"quiet\", require: false\nend\n\
             gem \"both\", \"~> 1.0\", require: [\"a\", \"b/c\"]\n",
        );
        assert_eq!(found.get("svg-graph"), Some(&vec!["SVG/Graph/TimeSeries".to_string()]));
        assert_eq!(found.get("quiet"), Some(&Vec::new()));
        assert_eq!(found.get("both"), Some(&vec!["a".to_string(), "b/c".to_string()]));
        assert!(!found.contains_key("plain"));
    }

    #[test]
    fn external_root_index_matches_the_linear_scan() {
        let roots: std::collections::BTreeSet<String> =
            ["Rouge", "Outer::Root", "Root", "A", "Foo::Bar::Baz", "Zeitwerk"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let details = [
            "Rouge", "::Rouge", "::::Rouge", "Rouge::X", "::Rouge::X", "NotRouge::X",
            "App::Types::Rouge::X", "App::Rouge", "Outer::Root::X", "::Outer::Root::X",
            "X::Outer::Root::Y", "XRoot::Y", "Root::Y::Z", "Q::Root::Y", "_Root::Y", "9Root::Y",
            "Foo::Bar::Baz::Q", "Foo::Bar::Baz", "X::Foo::Bar::Baz::Q", "Bar::Baz::Q", "A::B",
            "A", "::A", "", "::", ":::", "::::", "Rouge:Rouge::X", " Rouge::X", "(Rouge::X)",
            "Rouge::Rouge", "Zeitwerk::Loader::Foo", "Zeit::Foo", "Rouge::", "::Rouge::",
            "É::Rouge::X", "ÉRouge::X", "Foo::Bar::Qux::Baz::Q", "Outer::Outer::Root::X",
            "Outer:: Root::X", "Root::", "X Root::Y",
        ];
        for set in [roots, std::collections::BTreeSet::new(), ["".to_string()].into()] {
            let mut index = ExternalRootIndex::new(&set);
            for detail in details {
                let linear = set.iter().any(|root| detail_names_external_namespace(detail, root));
                assert_eq!(index.names_any(detail), linear, "{detail:?} against {set:?}");
                assert_eq!(index.names_any(detail), linear, "memoized {detail:?}");
            }
        }
    }

    #[test]
    fn a_release_version_drops_only_a_listed_platform() {
        let lock = Lockfile::parse("GEM\n  specs:\n    nokogiri (1.18.3-arm64-darwin)\n    pre (1.2.3-alpha)\n    rails (8.0.2)\n\nPLATFORMS\n  arm64-darwin\n  ruby\n");
        assert_eq!(lock.release_version_of("nokogiri"), Some("1.18.3"));
        assert_eq!(lock.release_version_of("pre"), Some("1.2.3-alpha"));
        assert_eq!(lock.release_version_of("rails"), Some("8.0.2"));
        assert_eq!(lock.release_version_of("absent"), None);
    }

    #[test]
    fn git_activation_orders_dependencies_and_terminates_cycles() {
        let lock = Lockfile::parse("GIT\n  remote: https://example.invalid/parent.git\n  revision: 1111111111111111111111111111111111111111\n  specs:\n    parent (1.0)\n      child\n\nGIT\n  remote: https://example.invalid/child.git\n  revision: 2222222222222222222222222222222222222222\n  specs:\n    child (2.0)\n      parent\n\nGEM\n  specs:\n    client (3.0)\n      parent\n\nDEPENDENCIES\n  client\n");
        assert_eq!(lock.git_specs_in_closure(&["client", "parent"]), ["child", "parent"]);
    }

    #[test]
    fn a_constant_belongs_to_the_gem_nested_deepest_over_it() {
        let lock = Lockfile::parse("GEM\n  specs:\n    omniauth (2.1.2)\n    rack (3.1.8)\n    rack-attack (6.7.0)\n    rack-cors (2.0.2)\n    sentry-rails (5.0)\n    sentry-ruby (5.0)\n\nDEPENDENCIES\n  omniauth\n  rack-attack\n  rack-cors\n  sentry-rails\n  sentry-ruby\n");
        assert_eq!(lock_gems_owning_constant(&lock, "Rack::Attack"), ["rack-attack"]);
        assert_eq!(lock_gems_owning_constant(&lock, "Rack::Utils"), ["rack"]);
        assert_eq!(lock_gems_owning_constant(&lock, "OmniAuth::Strategies"), ["omniauth"]);
        assert_eq!(lock_gems_owning_constant(&lock, "Sentry"), ["sentry-rails", "sentry-ruby"]);
        assert!(lock_gems_owning_constant(&lock, "Devise").is_empty());
    }

    #[test]
    fn lock_namespace_index_matches_the_ordered_scan() {
        let mut names = vec!["caxlsx", "rubyzip", "sorbet-runtime", "net-http-persistent",
            "polars-df", "ruby_llm", "svg-graph", "twilio-ruby", "sqlite3", "jwt", "zip_kit"];
        names.extend(["rack", "rack-attack", "rack-cors", "rack_attack", "sentry-rails", "sentry-ruby",
            "aws-sdk-s3", "a", "a-b", "a-b-c", "a_b", "A-b", "é", "é-b", "", "-", "--"]);
        let mut paths = vec!["NotAGem".to_string(), "Rackette::Attack".into(), "::".into(), ":".into()];
        for name in &names {
            let nested = name.split('-').map(camelize).collect::<Vec<_>>().join("::");
            for ns in std::iter::once(nested).chain(namespace_candidates(name)) {
                paths.extend([ns.clone(), format!("::{ns}::Child"), ns.to_ascii_lowercase(),
                    format!("{ns}ish"), format!("{ns}:Child"), format!("{ns}::")]);
            }
        }
        let mut specs: Vec<_> = names.iter().map(|n| (n.to_string(), "1.0".to_string())).collect();
        specs.push(("rack-attack".into(), "1.0-platform".into()));
        for specs in [specs.clone(), specs.into_iter().rev().collect()] {
            let lock = Lockfile { specs, ..Default::default() };
            let index = LockGemIndex::new(&lock);
            for path in &paths {
                assert_eq!(index.owners(path), lock_gems_owning_constant(&lock, path), "{path:?}");
            }
        }
    }

    #[test]
    fn graphql_owns_the_graphql_namespace() {
        // Name-derived, it would be `Graphql`; graphql-ruby's is `GraphQL`.
        assert_eq!(namespace_of("graphql"), "GraphQL");
    }

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
        assert_eq!(fate_of("inertia_rails"), GemFate::Modeled);
        assert_eq!(namespace_of("front_matter_parser"), "FrontMatterParser");
        assert_eq!(namespace_of("aws-sdk-s3"), "Aws");
        assert_eq!(namespace_of("rubyzip"), "Zip");
        assert_eq!(namespace_of("polars-df"), "Polars");
        assert_eq!(namespace_of("ruby_llm"), "RubyLLM");
        assert_eq!(namespace_of("caxlsx"), "Axlsx");
        assert_eq!(namespace_of("duckdb"), "DuckDB");
        assert_eq!(namespace_of("mcp"), "MCP");
        for gem in ["polars-df", "ruby_llm", "caxlsx", "duckdb", "mcp", "rubyzip"] {
            assert_eq!(fate_of(gem), GemFate::Unknown, "{gem} stays unmodeled");
        }
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
        assert_eq!(gem_owning_constant(&census, "Acme::Client"), None, "two prefix candidates are ambiguous");
        assert_eq!(gems_owning_constant(&census, "Acme::Client"), vec!["acme-core", "acme-telemetry"]);
        assert_eq!(
            gem_owning_constant(&census, "AcmeTelemetry::Span"),
            Some("acme-telemetry"),
            "the whole name still wins where a gem spells it out"
        );
        assert_eq!(gem_owning_constant(&census, "Rails"), None, "framework gems don't claim");
        let lock = Lockfile::parse(LOCK);
        assert_eq!(gem_claiming_method(&lock, "policy_scope"), Some("pundit"));
        assert_eq!(gem_claiming_method(&lock, "acts_as_list"), None, "acts-as-list is not in this lock");
    }

    #[test]
    fn irregular_namespaces_pass_through_ruby_emit_for_any_lockfile() {
        let lock = "\
GEM
  remote: https://rubygems.org/
  specs:
    caxlsx (4.2.0)
    duckdb (1.3.1)
    mcp (0.4.0)
    polars-df (0.20.0)
    ruby_llm (1.8.2)
    rubyzip (2.3.2)

DEPENDENCIES
  caxlsx
  duckdb
  mcp
  polars-df
  ruby_llm
  rubyzip
";
        let source = "\
class Export
  def run
    Zip::File.open(\"a.zip\")
    Polars::DataFrame.new
    RubyLLM.chat
    Axlsx::Package.new
    DuckDB::Database.open
    MCP::Server.new
  end
end
";
        let tree = [
            ("Gemfile.lock", lock),
            ("lib/export.rb", source),
        ]
        .into_iter()
        .map(|(path, text)| (std::path::PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
        let app = crate::ingest::ingest_app_from_tree(tree).expect("ingest locked irregular gems");
        let namespaces = locked_external_gem_namespaces(&app);
        for name in ["Zip", "Polars", "RubyLLM", "Axlsx", "DuckDB", "MCP"] {
            assert!(namespaces.contains(name), "{name} missing from {namespaces:?}");
        }
        for miss in ["Rubyzip", "PolarsDf", "RubyLlm", "Caxlsx", "Duckdb", "Mcp"] {
            assert!(
                !namespaces.contains(miss),
                "camelized miss {miss} must not be the Ruby-emit pass-through key"
            );
        }
        assert!(
            !detail_names_external_namespace("Mcp::Tool", "MCP"),
            "an app module Mcp is a different constant from the mcp gem"
        );
        let census = GemCensus::of(app.gem_lock.as_ref().expect("lock"));
        for gem in ["caxlsx", "duckdb", "mcp", "polars-df", "ruby_llm", "rubyzip"] {
            assert_eq!(
                census.gems.iter().find(|g| g.name == gem).map(|g| g.fate),
                Some(GemFate::Unknown),
                "{gem} must stay unknown"
            );
        }
        assert_eq!(gem_owning_constant(&census, "Polars::DataFrame"), Some("polars-df"));
        assert_eq!(gem_owning_constant(&census, "RubyLLM::Chat"), Some("ruby_llm"));
        assert_eq!(gem_owning_constant(&census, "Axlsx::Package"), Some("caxlsx"));
        assert_eq!(gem_owning_constant(&census, "DuckDB::Database"), Some("duckdb"));
        assert_eq!(gem_owning_constant(&census, "MCP::Server"), Some("mcp"));
        assert_eq!(gem_owning_constant(&census, "Zip::File"), Some("rubyzip"));

        let files = crate::project::target_files(&app, std::path::Path::new("."), crate::project::BuildTarget::Ruby)
            .expect("emit Ruby project");
        let export = files
            .iter()
            .find(|(_, content)| content.contains("class Export"))
            .map(|(_, content)| content.as_str())
            .expect("emitted Export");
        for konst in ["Zip::File", "Polars::DataFrame", "RubyLLM.chat", "Axlsx::Package", "DuckDB::Database", "MCP::Server"] {
            assert!(
                export.contains(konst),
                "Ruby emit must keep locked gem constant {konst}:\n{export}"
            );
        }
        assert!(
            !export.contains("PolarsDf") && !export.contains("RubyLlm") && !export.contains("Caxlsx"),
            "emit must not rewrite the gem's real constant:\n{export}"
        );
        let gemfile = files.iter().find(|(path, _)| path == "Gemfile").expect("Gemfile").1.as_str();
        for pin in [
            "gem \"caxlsx\", \"= 4.2.0\"",
            "gem \"duckdb\", \"= 1.3.1\"",
            "gem \"mcp\", \"= 0.4.0\"",
            "gem \"polars-df\", \"= 0.20.0\"",
            "gem \"ruby_llm\", \"= 1.8.2\"",
            "gem \"rubyzip\", \"= 2.3.2\"",
        ] {
            assert!(gemfile.contains(pin), "missing {pin} in\n{gemfile}");
        }
        let boot = files.iter().find(|(path, _)| path == "boot.rb").expect("boot.rb").1.as_str();
        for feature in ["caxlsx", "duckdb", "mcp", "polars-df", "ruby_llm"] {
            assert!(
                boot.contains(&format!("require {feature:?}")),
                "boot must load {feature} by gem name:\n{boot}"
            );
        }
        assert!(
            !boot.contains("require \"rubyzip\""),
            "rubyzip has no rubyzip.rb entry; boot must not require that name:\n{boot}"
        );
        assert!(loads_by_gem_name("duckdb") && loads_by_gem_name("mcp"));
        assert!(!loads_by_gem_name("rubyzip"));
    }

    #[test]
    fn a_gem_parked_under_a_framework_namespace_owns_its_base() {
        let lock = "\
GEM
  remote: https://rubygems.org/
  specs:
    active_model_serializers (0.10.14)
    rails (8.0.2)
    sanitize (7.0.0)

DEPENDENCIES
  active_model_serializers
  rails
  sanitize
";
        let census = GemCensus::of(&Lockfile::parse(lock));
        assert_eq!(gem_owning_constant(&census, "ActiveModel::Serializer"), Some("active_model_serializers"));
        assert_eq!(gem_owning_constant(&census, "ActiveModel::Serializer::CollectionSerializer"), Some("active_model_serializers"));
        assert_eq!(gem_owning_constant(&census, "ActiveModel::Errors"), None, "the rest of the namespace stays the framework's");
        assert_eq!(gem_defining_constant(&census, "ActiveModel::Serializer"), Some("active_model_serializers"));
        assert_eq!(gem_defining_constant(&census, "ActiveModel::Serializer::Mine"), None);
        assert_eq!(gem_defining_constant(&census, "Sanitize"), Some("sanitize"));
        assert_eq!(gem_defining_constant(&census, "Sanitize::AppConfig"), None, "an app class nested in a gem's namespace is the app's");
    }

    #[test]
    fn lock_path_remotes_reads_only_path_sources() {
        let lock = "\
GIT
  remote: https://github.com/acme/widgets.git
  revision: abc123
  specs:
    widgets (1.0.0)

PATH
  remote: lib/billing
  specs:
    billing (0.1.0)
      rails

PATH
  remote: ../shared
  specs:
    shared (0.1.0)

GEM
  remote: https://rubygems.org/
  specs:
    rails (8.0.2)
";
        assert_eq!(lock_path_remotes(lock), vec!["lib/billing", "../shared"]);
        assert!(lock_path_remotes(LOCK).is_empty());
        let parsed = Lockfile::parse(lock);
        assert!(parsed.is_from_git("widgets"));
        assert!(!parsed.is_from_git("billing"));
        assert!(parsed.is_in_repo("billing"));
    }
}

/// Gems that put stylesheets on Propshaft's load path, and their stems:
/// `stylesheet_link_tag :all` links each on every page of an app that
/// bundles the gem. Read by ingest (the `<link>` list) and by the
/// Makefile generator (the copy-from-gem rules), so the page and the
/// build agree by construction.
pub const GEM_STYLESHEETS: &[(&str, &[&str])] = &[
    ("action_text-trix", &["trix"]),
    ("lexxy", &["lexxy-content", "lexxy-editor", "lexxy-variables", "lexxy"]),
];

/// Every class and module `runtime/spinel/*.rb` declares, by full name: a gem's
/// constant ported there (`Net::HTTP::Persistent`) runs on the Spinel tree.
pub(crate) fn spinel_runtime_constants() -> &'static crate::det::HashSet<String> {
    static NAMES: std::sync::OnceLock<crate::det::HashSet<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| declared_runtime_constants(&["runtime/spinel"]))
}

/// The same over the shared ports too (`runtime/ruby/typeid.rb`'s `TypeID`),
/// which every Ruby-family tree and the Spinel one carry. Not a façade: its
/// raising stand-in is what a gem compiled from source replaces.
pub(crate) fn ported_runtime_constants() -> &'static crate::det::HashSet<String> {
    static NAMES: std::sync::OnceLock<crate::det::HashSet<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| declared_runtime_constants(&["runtime/spinel", "runtime/ruby"]))
}

/// Top-level namespaces whose façade in `runtime/ruby/gem_facades.rb`
/// answers a call by `GemFacade.fail!` (rotp's `ROTP`): the Spinel tree
/// has no working body for them.
pub(crate) fn stub_facade_namespaces() -> &'static crate::det::HashSet<String> {
    static NAMES: std::sync::OnceLock<crate::det::HashSet<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut out = crate::det::HashSet::default();
        let Ok(text) = crate::runtime_files::read_to_string("runtime/ruby/gem_facades.rb") else { return out };
        let mut current: Option<String> = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("module ").or_else(|| line.strip_prefix("class ")) {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                current = (!name.is_empty() && name != "GemFacade").then_some(name);
            } else if line.contains("GemFacade.fail!")
                && let Some(name) = &current
            {
                out.insert(name.clone());
            }
        }
        // Not one the Spinel tree ports (`runtime/spinel/web_push.rb`): the sourced gem would drop the whole façade, and the port's stub slots with it.
        out.retain(|name| !spinel_runtime_constants().contains(name));
        out
    })
}

fn declared_runtime_constants(dirs: &[&str]) -> crate::det::HashSet<String> {
    struct Declared {
        nesting: Vec<String>,
        out: crate::det::HashSet<String>,
    }
    impl Declared {
        fn enter(&mut self, path: &ruby_prism::Node<'_>) {
            let name = String::from_utf8_lossy(path.location().as_slice()).trim_start_matches("::").to_string();
            self.nesting.push(name);
            self.out.insert(self.nesting.join("::"));
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Declared {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.enter(&node.constant_path());
            ruby_prism::visit_module_node(self, node);
            self.nesting.pop();
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            self.enter(&node.constant_path());
            ruby_prism::visit_class_node(self, node);
            self.nesting.pop();
        }
    }
    let mut files = Vec::new();
    for dir in dirs {
        let _ = crate::runtime_files::walk_flat(dir, &["rb"], "runtime/", &mut files);
    }
    let mut declared = Declared { nesting: Vec::new(), out: crate::det::HashSet::default() };
    for (path, source) in &files {
        if path.ends_with("facade.rb") || path.ends_with("facades.rb") {
            continue;
        }
        let parsed = ruby_prism::parse(source.as_bytes());
        ruby_prism::Visit::visit(&mut declared, &parsed.node());
    }
    declared.out
}
