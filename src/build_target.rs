//! Which target a build is for.
//!
//! Host project assembly (`crate::project`) walks the filesystem and is
//! excluded from the wasm compiler. Lowering and Ruby emit still name a
//! target there, so the enum and the policies those passes read live here.

/// Targets the `roundhouse` binary can produce, plus the `Blog`
/// pseudo-target (verbatim source archive).
///
/// The transpile targets (`Spinel` through `TypescriptWorker`) are
/// valid for both `--target LANG` and `--site` modes. `Blog` is only
/// valid for `--site` — it's the source fixture, not a transpile
/// output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildTarget {
    /// Source fixture, walked verbatim. `--site` only.
    Blog,
    /// Spinel-target emit: scaffold + runtime + lowered app, FFI db.rb.
    Spinel,
    /// CRuby-target emit: spinel files + ruby_overlay + gem db.rb +
    /// fixture's app/javascript + public assets.
    Ruby,
    /// JRuby-target emit: byte-identical to the Ruby target except the
    /// SQLite backend — ships the JDBC `db_jruby.rb` (the `sqlite3` gem
    /// is a C extension with no JRuby build) so the same emitted source
    /// runs on the JVM.
    Jruby,
    /// Rails → Roda + Sequel source conversion (issue #67 spike). Runs
    /// on the real roda/sequel gems, not the roundhouse runtime, and
    /// emits from the INGEST-shape App (`bin/roundhouse` skips
    /// `analyze_and_lower` for it) — see `emit::roda`.
    Roda,
    /// Rails → Ruby as a partial evaluator: specialize what the analysis
    /// can resolve, leave the rest as residual Ruby running on the real
    /// gems (Rails included). Stage 0 is the identity — the residue is
    /// the whole app, walked verbatim — so the gate (`tests/futamura.rs`:
    /// the app's own Rails suite, run against the output) exists before
    /// the first specialization does. Like `Roda`, skips
    /// `analyze_and_lower` in `bin/roundhouse`.
    Futamura,
    Crystal,
    Elixir,
    Go,
    /// Kotlin/JVM emit (backend-only). In the `ALL` `--site` archive
    /// matrix as of the e2e-kotlin gate (the emitted archive builds via
    /// `gradle installDist` and boots — see `scripts/e2e kotlin`).
    /// Still incomplete (partial e2e/compare coverage), like several
    /// published targets — see `docs/archive/kotlin-migration-plan.md`.
    Kotlin,
    Python,
    Rust,
    /// Swift emit (backend-only). In the `ALL` `--site` archive matrix
    /// as of the compare/bench/CI gates closing (the emitted archive
    /// builds via `swift build` and boots; Server.swift serves
    /// `/assets/*`). Still incomplete (no frameworks/e2e gates, like
    /// several published targets) — see `docs/archive/swift-migration-plan.md`
    /// and issue #34.
    Swift,
    /// C# / .NET emit (backend-only). Scaffold stage — `emit` produces the
    /// .NET project scaffold (`roundhouse-app.csproj`, `Program.cs`) and the
    /// `ty`/`naming` mappings; models/controllers/views/runtime land in later
    /// phases. See `docs/archive/csharp-migration-plan.md`.
    CSharp,
    Typescript,
    /// TypeScript emit under the `worker` deployment profile
    /// (SharedWorker browser deployment).
    TypescriptWorker,
}

/// Driver support and the measured rollout policy are separate decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamBindCapability {
    /// Use binds unless the compiler receives the explicit `=0` override.
    DefaultOn,
    /// The driver is supported, but binds require the explicit `=1` override.
    OptIn,
    /// The selected driver/profile cannot execute the generated bind contract.
    Unsupported,
}

impl ParamBindCapability {
    fn enabled(self, override_value: Option<&str>) -> bool {
        match self {
            Self::DefaultOn => override_value != Some("0"),
            Self::OptIn => override_value == Some("1"),
            Self::Unsupported => false,
        }
    }
}

impl BuildTarget {
    /// All targets that participate in `--site` archive generation,
    /// in site-archive order.
    pub const ALL: &'static [BuildTarget] = &[
        BuildTarget::Blog,
        BuildTarget::Spinel,
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Kotlin,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Swift,
        BuildTarget::CSharp,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ];

    /// Targets the `--site` build archives beyond [`Self::ALL`]: they
    /// get a browse archive, but sit outside the per-target test
    /// matrices that iterate `ALL` (futamura's gate is the app's own
    /// Rails suite, `tests/futamura.rs`).
    pub const SITE_ONLY: &'static [BuildTarget] = &[BuildTarget::Futamura];

    /// Targets valid for `--target LANG` (transpile to directory).
    /// Excludes `Blog` (source-only) — `--target blog` would mean
    /// "copy the input to the output," which is a `cp -r`, not a
    /// transpile.
    pub const TRANSPILE: &'static [BuildTarget] = &[
        BuildTarget::Spinel,
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Roda,
        BuildTarget::Futamura,
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Kotlin,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Swift,
        BuildTarget::CSharp,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ];

    /// CLI name. Stable — used in `--target X` and in
    /// `_site/browse/<name>.{json,tgz,zip}` archive filenames.
    pub fn as_str(self) -> &'static str {
        match self {
            BuildTarget::Blog => "blog",
            BuildTarget::Spinel => "spinel",
            BuildTarget::Ruby => "ruby",
            BuildTarget::Jruby => "jruby",
            BuildTarget::Roda => "roda",
            BuildTarget::Futamura => "futamura",
            BuildTarget::Crystal => "crystal",
            BuildTarget::Elixir => "elixir",
            BuildTarget::Go => "go",
            BuildTarget::Kotlin => "kotlin",
            BuildTarget::Python => "python",
            BuildTarget::Rust => "rust",
            BuildTarget::Swift => "swift",
            BuildTarget::CSharp => "csharp",
            BuildTarget::Typescript => "typescript",
            BuildTarget::TypescriptWorker => "typescript-worker",
        }
    }

    /// Typed positional read binds: support plus the runtime's rollout policy.
    /// TypeScript's node-sync, libsql and worker shims are all disabled until
    /// each deployment profile implements that execution contract.
    pub fn param_bind_capability(self) -> ParamBindCapability {
        match self {
            BuildTarget::Spinel => ParamBindCapability::DefaultOn,
            // CRuby loses SQL-only result replay on repeated bound reads;
            // JRuby has correctness coverage but no matched performance run.
            BuildTarget::Ruby | BuildTarget::Jruby => ParamBindCapability::OptIn,
            BuildTarget::Blog
            | BuildTarget::Roda
            | BuildTarget::Futamura
            | BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Go
            | BuildTarget::Kotlin
            | BuildTarget::Python
            | BuildTarget::Rust
            | BuildTarget::Swift
            | BuildTarget::CSharp
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker => ParamBindCapability::Unsupported,
        }
    }

    /// Unknown values use the target default; `=1` cannot enable an unsupported shim.
    pub(crate) fn param_binds_enabled(self, override_value: Option<&str>) -> bool {
        self.param_bind_capability().enabled(override_value)
    }

    /// Does this target ship a runtime `ActiveRecord::Relation` for a
    /// query chain the Arel builder could not fold to SQL?
    ///
    /// The ruby-family targets (`ruby`, `jruby`, `spinel`) emit
    /// `runtime/ruby/active_record/relation.rb`, so an unfolded chain
    /// executes there — a missed specialization, not a defect. Nothing
    /// under `runtime/{rust,go,crystal,python,typescript,…}` defines a
    /// Relation or any of its methods, so for those targets the same
    /// chain is a call into a type that does not exist: uncompilable
    /// output. That is what makes the `relation_residue` ledger's
    /// "unsupported at strict-target emit" a WARNING on one side of
    /// this predicate and an ERROR on the other (issue #76).
    ///
    /// `Roda` converts to real Sequel datasets and `Futamura` leaves an
    /// unfolded chain on real ActiveRecord; both skip the lowerings
    /// entirely (`bin/roundhouse`), so no residue is ever raised
    /// against them; `Blog` is the source fixture walked verbatim.
    /// Matched exhaustively on purpose — a new target has to answer
    /// this question rather than inherit an answer.
    pub fn has_runtime_relation(self) -> bool {
        match self {
            BuildTarget::Blog
            | BuildTarget::Spinel
            | BuildTarget::Ruby
            | BuildTarget::Jruby
            | BuildTarget::Roda
            | BuildTarget::Futamura => true,
            BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Go
            | BuildTarget::Kotlin
            | BuildTarget::Python
            | BuildTarget::Rust
            | BuildTarget::Swift
            | BuildTarget::CSharp
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker => false,
        }
    }

    /// Does the target store a PostgreSQL array column (`array: true`) as
    /// the Array Rails reads (JSON text in SQLite), typing it
    /// `Array[<element>]`? The Ruby-family runtimes do
    /// (`lower::array_columns`). Every other target that stores rows
    /// holds one element and reports the column
    /// (`project::report_array_columns`).
    /// Does the target's `numericality:` check parse a String the way Rails'
    /// validator does (`ActiveSupport.numericality_number`)? The
    /// Ruby-family runtimes do; on the others an untyped attribute's
    /// `numericality:` is the reported gap
    /// (`ingest::app::refuse_numericality_allow_on_untyped`).
    pub fn parses_numericality_input(self) -> bool {
        matches!(self, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel)
    }

    pub fn stores_array_columns(self) -> bool {
        matches!(self, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel)
    }

    /// Does the emitted persistence runtime provide the single-statement
    /// `Db.exec_returning` operation needed to hydrate generated columns
    /// from the INSERT that created them? The Ruby-family runtimes each
    /// implement it (SQLite adapters require SQLite 3.35+); the SDK-backed
    /// targets do not ship a matching database runtime yet. `Blog` is only
    /// the source fixture, so it is not a runtime support claim.
    pub(crate) fn supports_generated_column_insert_returning(self) -> bool {
        match self {
            BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel => true,
            // Real Rails persists generated columns itself; `target_files`
            // returns the verbatim tree before this question is asked.
            BuildTarget::Futamura => true,
            BuildTarget::Blog
            | BuildTarget::Roda
            | BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Go
            | BuildTarget::Kotlin
            | BuildTarget::Python
            | BuildTarget::Rust
            | BuildTarget::Swift
            | BuildTarget::CSharp
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker => false,
        }
    }

    /// Parse a CLI string. Returns `None` for unknown names. Chains
    /// `TRANSPILE` after `ALL` so a transpile-only name (`roda`,
    /// `futamura`) still parses for `--target`.
    pub fn from_str(s: &str) -> Option<BuildTarget> {
        for t in BuildTarget::ALL.iter().chain(BuildTarget::TRANSPILE.iter()) {
            if t.as_str() == s {
                return Some(*t);
            }
        }
        None
    }
}
