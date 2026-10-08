//! Project-shape assembly: given an ingested + analyzed [`App`] plus
//! a [`BuildTarget`], return the canonical file set for that target as
//! a `Vec<(path, content)>`. Shared by the `roundhouse` binary's
//! `--target LANG` (single target → directory) and `--site` (all
//! targets → archives) modes.
//!
//! The per-target dispatch matches `src/emit/`: most targets are a
//! thin wrapper over `emit::<lang>::emit(&app)`, while `spinel` and
//! `ruby` compose a scaffold + runtime overlay on top of the lowered
//! emit (mirroring the Makefile's `ruby-transpile` / `spinel-transpile`
//! rules). `Blog` is a special target — the source fixture walked
//! verbatim, only used by the `--site` archive matrix.
//!
//! The scaffold/runtime trees (`runtime/spinel/scaffold/`,
//! `runtime/ruby/`) come from [`crate::runtime_files`], embedded at
//! build time, so the binary emits every target from any working
//! directory; the app's own directories are still walked on disk. The
//! emit dispatch is host-only; WASM builds use a different entry point
//! and don't pull this module.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use zip::write::SimpleFileOptions;

use crate::App;
use crate::emit::{self, EmittedFile};
use crate::ingest::ingest_app;

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

    /// Targets valid for `--target LANG` (transpile to directory).
    /// Excludes `Blog` (source-only) — `--target blog` would mean
    /// "copy the input to the output," which is a `cp -r`, not a
    /// transpile.
    pub const TRANSPILE: &'static [BuildTarget] = &[
        BuildTarget::Spinel,
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Roda,
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
    /// `Roda` converts to real Sequel datasets and skips the lowerings
    /// entirely (`bin/roundhouse`), so no residue is ever raised
    /// against it; `Blog` is the source fixture walked verbatim.
    /// Matched exhaustively on purpose — a new target has to answer
    /// this question rather than inherit an answer.
    pub fn has_runtime_relation(self) -> bool {
        match self {
            BuildTarget::Blog
            | BuildTarget::Spinel
            | BuildTarget::Ruby
            | BuildTarget::Jruby
            | BuildTarget::Roda => true,
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

    /// Does the emitted persistence runtime provide the single-statement
    /// `Db.exec_returning` operation needed to hydrate generated columns
    /// from the INSERT that created them? The Ruby-family runtimes each
    /// implement it (SQLite adapters require SQLite 3.35+); the SDK-backed
    /// targets do not ship a matching database runtime yet. `Blog` is only
    /// the source fixture, so it is not a runtime support claim.
    fn supports_generated_column_insert_returning(self) -> bool {
        match self {
            BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel => true,
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
    /// `TRANSPILE` after `ALL` so transpile-only targets not in the
    /// `--site` matrix (e.g. `kotlin`) still parse for `--target`.
    pub fn from_str(s: &str) -> Option<BuildTarget> {
        for t in BuildTarget::ALL.iter().chain(BuildTarget::TRANSPILE.iter()) {
            if t.as_str() == s {
                return Some(*t);
            }
        }
        None
    }
}

/// Quick-start README for a transpile target. Injected into every
/// file set by `target_files` (so both `--target` output and the
/// `--site` archives carry it), unless the set already contains a
/// `README.md` — the Blog fixture ships its own, which must not be
/// overwritten. (The scaffold targets spinel/ruby/jruby rename theirs to
/// `SPECIMEN.md` — see `scaffold_readme_to_specimen` — so they take this
/// quick-start.)
///
/// Content is intentionally short: prerequisites, build, run, test,
/// and the regenerate command. For `ships_e2e` targets the `## <name>`
/// sections are a CI contract — `scripts/smoke` executes their ```sh
/// blocks verbatim against the published archive.
/// MRI prerequisites describe the minimum in `.ruby-version`, not a CI
/// patch pin. Keep the human-facing minimum aligned when that line changes.
pub fn target_readme(target: BuildTarget) -> String {
    let name = target.as_str();
    let body = match target {
        BuildTarget::Blog => {
            "Source fixture, walked verbatim. Not a transpile output — no \
             build commands apply. This archive exists so consumers can \
             download the input that Roundhouse transpiles. (The Regenerate \
             command below re-walks the fixture into this archive.)\n"
        }
        // Spinel AOT: the tree is a spin package (spin.toml + bin/ +
        // test/*.rb with .expected snapshots — see `spin_shape`), so
        // Build/Test use spinel's own project tool. Assets ship prebuilt
        // (like jruby — `make assets` needs MRI). `spin test` feeds the
        // .rbs sidecars to the compiler itself (matz/spinel#1788), so no
        // explicit seed step is needed. The comprehensive scaffold doc
        // ships as SPECIMEN.md (scaffold_readme_to_specimen).
        BuildTarget::Spinel => {
            "This tree is the Rails-shape-without-metaprogramming \
             specimen, packaged as a [spin](https://github.com/matz/spinel/blob/master/docs/spin.md) \
             project and compiled ahead-of-time to a native binary by the \
             [Spinel](https://github.com/matz/spinel) Ruby VM — see \
             `SPECIMEN.md` for the full architecture document (layout, \
             runtime, ruleset, limitations). Static assets ship prebuilt \
             in `static/assets/` (the Makefile's `make assets` step needs \
             the MRI toolchain; the binary sendfiles them at `/assets/*`).\n\n\
             ## Prerequisites\n\
             - [spinel](https://github.com/matz/spinel) — `spinel` and \
             `spin` on PATH (the repo's `bin/` after `make`)\n\
             - A C toolchain + SQLite headers (`libsqlite3-dev`) — the binary links `-lsqlite3`\n\
             - jemalloc headers (`libjemalloc-dev`) — `spin.toml` asks for it as \
             this program's allocator, and a server is the shape where that is \
             worth having. Note the DEV package: the `libjemalloc2` runtime alone \
             is what `LD_PRELOAD` uses and is not enough to link against\n\
             - libvips (`libvips-dev` to build, `libvips42` to run; `brew install vips`) — \
             only when `spin.toml` lists `ruby-vips`, which it does when the app \
             declares image variants (thumbnails, avatars)\n\
             - Node.js 24+ — for the End-to-end suite\n\n\
             ## Build\n\
             ```sh\n\
             spin build\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./build/bin/blog\n\
             ```\n\
             The server is a green thread per connection, run on the runtime's OS \
             workers (one per core, autodetected); `SPINEL_WORKERS=N` in the \
             environment sets the count.\n\n\
             ## Test\n\
             Each `test/*.rb` compiles to its own binary and diffs against \
             its `.expected` snapshot. `spin test` feeds the `.rbs` \
             sidecars to the compiler itself, so no seeding is needed:\n\
             ```sh\n\
             spin test\n\
             ```\n"
        }
        BuildTarget::Roda => {
            "A Rails → Roda + Sequel source conversion (issue #67 spike). \
             Runs on the real `roda`/`sequel` gems — no roundhouse \
             runtime. Convertible constructs are emitted as idiomatic \
             Roda/Sequel; everything else is a `ROUNDHOUSE-TODO` comment \
             carrying the original Rails source for manual conversion.\n\n\
             ## Prerequisites\n\
             - Ruby 3.4+ (with bundler)\n\
             - SQLite (system library)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             bundle install\n\
             ```\n\n\
             ## Run\n\
             Migrations run when the app loads (so the migrate step is \
             just loading `db.rb` once), then seed the demo rows:\n\
             ```sh\n\
             bundle exec ruby -r ./db -e \"\"\n\
             sqlite3 db/blog.db < db/seed.sql\n\
             bundle exec rackup\n\
             ```\n"
        }
        // ruby/jruby Test sections run the same five driver files as
        // `tests/ruby_toolchain.rs` — NOT `rake test`: the archive's
        // emitted `test/test_helper.rb` is deliberately Minitest-free
        // (TestBase, for spinel AOT), while the scaffold's runtime
        // tests subclass Minitest::Test, so one rake_test_loader
        // process can't host both populations.
        BuildTarget::Ruby => {
            "This tree is the Rails-shape-without-metaprogramming \
             specimen — see `SPECIMEN.md` for the full architecture \
             document (layout, runtime, ruleset, limitations).\n\n\
             ## Prerequisites\n\
             - Ruby 3.4+ (with bundler)\n\
             - Node.js + npm — Tailwind/Turbo asset build (only when the \
             source app ships stylesheets/JS; a JS-less app skips this)\n\
             - SQLite (system library)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             bundle install\n\
             ```\n\n\
             ## Build\n\
             ```sh\n\
             make assets\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             bundle exec puma -C config/puma.rb\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             bundle exec ruby -Itest -I. test/models/article_test.rb\n\
             bundle exec ruby -Itest -I. test/models/comment_test.rb\n\
             bundle exec ruby -Itest -I. test/controllers/articles_controller_test.rb\n\
             bundle exec ruby -Itest -I. test/controllers/comments_controller_test.rb\n\
             bundle exec ruby -Itest -I. test/query_count_test.rb\n\
             ```\n"
        }
        BuildTarget::Jruby => {
            // `jruby -S bundle exec jruby …` (not `… exec ruby …`):
            // bundle exec resolves plain `ruby` via PATH/shebang, which
            // lands on MRI when both interpreters are installed.
            // Static assets ship prebuilt (ensure_static_assets): the
            // Makefile's turbo.min.js copy shells `bundle exec ruby`,
            // colliding the MRI and JRuby bundlers — so no Build step.
            "This tree is the Rails-shape-without-metaprogramming \
             specimen running on the JVM — see `SPECIMEN.md` for the \
             full architecture document. Static assets ship prebuilt \
             in `static/assets/` (the Makefile's `make assets` step is \
             MRI-only).\n\n\
             ## Prerequisites\n\
             - JRuby 10+ (JDK 21+)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             jruby -S bundle install\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             WEB_CONCURRENCY=0 jruby -S bundle exec puma -C config/puma.rb\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             jruby -S bundle exec jruby -Itest -I. test/models/article_test.rb\n\
             jruby -S bundle exec jruby -Itest -I. test/models/comment_test.rb\n\
             jruby -S bundle exec jruby -Itest -I. test/controllers/articles_controller_test.rb\n\
             jruby -S bundle exec jruby -Itest -I. test/controllers/comments_controller_test.rb\n\
             jruby -S bundle exec jruby -Itest -I. test/query_count_test.rb\n\
             ```\n"
        }
        BuildTarget::Crystal => {
            "## Prerequisites\n\
             - Crystal 1.10+\n\
             - SQLite (system library)\n\n\
             ## Build\n\
             ```sh\n\
             shards install\n\
             crystal build src/main.cr -o server\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./server\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             crystal spec\n\
             ```\n"
        }
        BuildTarget::Elixir => {
            "## Prerequisites\n\
             - Elixir 1.15+ (Mix)\n\n\
             ## Install dependencies\n\
             ```sh\n\
             mix deps.get\n\
             mix compile\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             mix run --no-halt -e \"Main.run\"\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             mix test\n\
             ```\n"
        }
        BuildTarget::Go => {
            // `go mod tidy` is mandatory: the emitted go.sum is an
            // empty placeholder, so nothing resolves without it.
            // `-o server` is too: the module is named `app` and the
            // tree has an `app/` source dir, so a bare `go build .`
            // fails with "build output already exists" (caught by
            // scripts/smoke the first time the README was executed).
            "## Prerequisites\n\
             - Go 1.24+\n\n\
             ## Build\n\
             ```sh\n\
             go mod tidy\n\
             go build -o server .\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./server\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             go test ./...\n\
             ```\n"
        }
        BuildTarget::Kotlin => {
            "## Prerequisites\n\
             - JDK 17+\n\
             - Gradle 8+\n\n\
             ## Build\n\
             ```sh\n\
             gradle installDist\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./build/install/roundhouse-app/bin/roundhouse-app\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             gradle test\n\
             ```\n"
        }
        BuildTarget::Swift => {
            "## Prerequisites\n\
             - Swift 6+ (swift.org toolchain or Xcode CLT)\n\
             - Linux: `libsqlite3-dev`\n\n\
             ## Build\n\
             ```sh\n\
             swift build\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             swift run\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             swift test\n\
             ```\n"
        }
        BuildTarget::Python => {
            // --extra test pulls pytest (an optional dependency group
            // in pyproject.toml) so the Test step below resolves.
            "## Prerequisites\n\
             - Python 3.11+\n\
             - `uv`\n\n\
             ## Install dependencies\n\
             ```sh\n\
             uv sync --extra test\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             uv run python -m app\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             uv run pytest\n\
             ```\n"
        }
        BuildTarget::Rust => {
            "## Prerequisites\n\
             - Rust 1.85+ (`cargo`)\n\
             - SQLite (system library)\n\n\
             ## Build\n\
             ```sh\n\
             cargo build --release\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./target/release/app\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             cargo test\n\
             ```\n"
        }
        BuildTarget::CSharp => {
            "## Prerequisites\n\
             - .NET SDK 10+ (`dotnet`)\n\n\
             ## Build\n\
             ```sh\n\
             dotnet build\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             dotnet run\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             dotnet test tests\n\
             ```\n"
        }
        BuildTarget::Typescript => {
            "## Prerequisites\n\
             - Node.js 24+\n\n\
             ## Install dependencies\n\
             ```sh\n\
             npm install\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             npm start\n\
             ```\n\n\
             ## Test\n\
             ```sh\n\
             npm test\n\
             ```\n"
        }
        BuildTarget::TypescriptWorker => {
            "Browser deployment as a SharedWorker. The emitted bundle \
             is loaded by a host HTML page — there's no standalone \
             server.\n\n\
             ## Prerequisites\n\
             - Node.js 24+ (for bundling)\n\n\
             ## Install + build\n\
             ```sh\n\
             npm install\n\
             npm run build\n\
             ```\n\n\
             ## Run\n\
             Open the host HTML page in a browser. The worker bundle \
             runs in a `SharedWorker` context.\n"
        }
    };
    // Inject a uniform `## Setup` step before `## Run`, for every target
    // that ships a DB-backed server (`ships_e2e`). It seeds the Rails-
    // traditional `storage/development.sqlite3` from the bundled
    // `db/seed.sql`, which is self-contained (CREATE TABLE IF NOT EXISTS
    // + INSERT) so it runs standalone — no build/boot first — and
    // `storage/.keep` (shipped by `ensure_storage_keep`) guarantees the
    // directory exists. scripts/smoke executes this block (it skips only
    // Run / Regenerate), so the previously-untested human setup path is
    // now CI-covered.
    let body = if ships_e2e(target) {
        body.replacen(
            "## Run\n",
            "## Setup\n\
             Seed the database — the Rails-traditional \
             `storage/development.sqlite3`, from the bundled `db/seed.sql` \
             (needs the `sqlite3` CLI):\n\
             ```sh\n\
             sqlite3 storage/development.sqlite3 < db/seed.sql\n\
             ```\n\n\
             ## Run\n",
            1,
        )
    } else {
        body.to_string()
    };
    // Every server target serves the same blog with the same env
    // conventions (PORT, default 3000; Action Cable at /cable), so
    // the "what you get" sentence lives here, not per target. Blog
    // (source fixture) and TypescriptWorker (no standalone server)
    // are the two non-server archives.
    let serves = match target {
        BuildTarget::Blog | BuildTarget::TypescriptWorker => "",
        // The Roda conversion serves through rackup (default :9292) and
        // has no Action Cable surface — its README body already carries
        // the run instructions.
        BuildTarget::Roda => "",
        _ => {
            "Running it serves the blog on http://localhost:3000 \
             (set `PORT` to override), with live Turbo Stream \
             updates over the `/cable` WebSocket.\n\n"
        }
    };
    let attribution = match target {
        BuildTarget::Blog => {
            "The Rails source app that [Roundhouse]\
             (https://rubys.github.io/roundhouse/) transpiles."
        }
        _ => {
            "Transpiled from a Rails source app by [Roundhouse]\
             (https://rubys.github.io/roundhouse/)."
        }
    };
    // Archives that ship the Playwright suite (see `ships_e2e`)
    // document its run here. CI's smoke job executes these blocks
    // verbatim, so the section must stay runnable as written.
    let e2e = if ships_e2e(target) {
        // Every e2e target now has per-session (cookie) flash, so the full
        // Playwright suite — including `flash.spec.js` — runs everywhere.
        // (Each server is a storage adapter over the shared Flash class's
        // show-once sweep: ruby/jruby via Rails; go/kotlin/swift/elixir/
        // python a central dispatch that loads `Flash(incoming)` + persists
        // `to_persisted`; rust a FLASH_OUT thread-local; crystal/typescript
        // raw Cookie/Set-Cookie headers — was a global in-memory slot that
        // raced the comment specs under `fullyParallel`.) The E2E_SKIP knob
        // stays in playwright.config.js for ad-hoc `scripts/e2e --skip`, but
        // no target needs it now. (See the flash-wiring punch list memory.)
        format!(
            "## End-to-end\n\
             Browser smoke tests (Playwright). Needs Node.js 24+ and the \
             `sqlite3` CLI; run after the Build steps above — the test \
             config boots the server and seeds `db/seed.sql` itself:\n\
             ```sh\n\
             cd e2e\n\
             npm install\n\
             npx playwright install chromium\n\
             npx playwright test\n\
             ```\n\n"
        )
    } else {
        String::new()
    };
    format!(
        "# Roundhouse → {name}\n\n\
         {attribution}\n\n\
         {serves}\
         {body}\n\
         {e2e}\
         ## Regenerate\n\
         ```sh\n\
         roundhouse --target {name} -o <output-dir> <input-app>\n\
         ```\n"
    )
}

/// Produce the file set for `target`. `app` must already be ingested
/// and analyzed. `fixture` is the source-app path on disk — needed
/// by `Blog` (walks the fixture) and `Ruby` (copies `app/javascript`
/// and `public`).
///
/// Returned entries are `(relative_path, file_content)`, sorted by
/// path. Binary files (anything containing a NUL byte, or files that
/// don't decode as UTF-8) are silently skipped — the archive payload
/// is text-only by construction.
/// The base's key contract, as wide as the app's keys. `runtime/ruby/
/// active_record/base.rbs` declares `id`, `id=` and `_adapter_insert`
/// over `Integer` — Rails' default, and the only key the transpiled
/// targets support. An app with a string or uuid key ships the sidecar
/// with those three widened to `(Integer | String)`: Spinel types a
/// Base-typed receiver's `id` from this declaration, and a String-keyed
/// model's own `attr_reader id: String` has to be a member of it
/// (roundhouse#90). Integer-only apps ship the sidecar byte-for-byte.
/// The three lines are matched exactly so a drift in the sidecar is a
/// build error here, not a silent narrowing.
fn widen_key_contract(app: &App, files: &mut [(String, String)]) -> Result<(), String> {
    let has_string_key = app.schema.tables.values().any(|t| {
        t.columns.iter().any(|c| {
            c.primary_key
                && !matches!(
                    c.col_type,
                    crate::schema::ColumnType::Integer | crate::schema::ColumnType::BigInt
                )
        })
    });
    if !has_string_key {
        return Ok(());
    }
    let Some((_, text)) = files.iter_mut().find(|(p, _)| p == "sig/runtime/active_record/base.rbs")
    else {
        return Err("widen_key_contract: sig/runtime/active_record/base.rbs not in the tree".into());
    };
    for (narrow, wide) in [
        ("    def id: () -> Integer\n", "    def id: () -> (Integer | String)\n"),
        ("    def id=: (Integer) -> Integer\n", "    def id=: (Integer | String) -> (Integer | String)\n"),
        ("    def _adapter_insert: () -> Integer\n", "    def _adapter_insert: () -> (Integer | String)\n"),
    ] {
        if !text.contains(narrow) {
            return Err(format!("widen_key_contract: base.rbs no longer declares {narrow:?}"));
        }
        *text = text.replace(narrow, wide);
    }
    // Relation#ids reads the primary key; string/uuid apps need the
    // same widened element type Spinel sees on Base#id (#310).
    let Some((_, relation)) =
        files.iter_mut().find(|(p, _)| p == "sig/runtime/active_record/relation.rbs")
    else {
        return Err(
            "widen_key_contract: sig/runtime/active_record/relation.rbs not in the tree".into(),
        );
    };
    let ids_narrow = "    def ids: () -> Array[Integer]\n";
    let ids_wide = "    def ids: () -> Array[Integer | String]\n";
    if !relation.contains(ids_narrow) {
        return Err(format!(
            "widen_key_contract: relation.rbs no longer declares {ids_narrow:?}"
        ));
    }
    *relation = relation.replace(ids_narrow, ids_wide);
    Ok(())
}

/// Spinel treats RBS `Base` as an *instance* (`sp_ActiveRecord__Base *`).
/// Shared `relation.rbs` uses `Base` in two places Roundhouse needs for
/// Bar A/B; both are rewritten on the Spinel tree only:
///
/// 1. `initialize: (Base model)` — callers pass a **class** (`User`,
///    `self` in a class method). Spinel rejects that as a Base instance.
/// 2. Terminals (`first` / `find_by` / …) returning `Base` / `Base?` —
///    callers expect a concrete model (`User*`); Spinel will not convert
///    Base → User.
/// Widen `Base` on def / ivar lines only. Comments stay. `Base?` before
/// bare `Base` so `-> Base?` becomes `untyped`, not `untyped?`.
fn widen_spinel_base_on_sigs(src: &str) -> String {
    let mut widened = String::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("def ") || trimmed.starts_with('@') {
            widened.push_str(
                &line
                    .replace("Array[Base]", "Array[untyped]")
                    .replace("Set[Base]", "Set[untyped]")
                    .replace("Base?", "untyped")
                    .replace("Base", "untyped"),
            );
        } else {
            widened.push_str(line);
        }
        widened.push('\n');
    }
    widened
}

pub fn spinel_relation_model_handle(files: &mut [(String, String)]) -> Result<(), String> {
    let idx = files
        .iter()
        .position(|(p, _)| {
            p == "sig/runtime/active_record/relation.rbs"
                || p == "runtime/active_record/relation.rbs"
        })
        .ok_or_else(|| {
            "spinel_relation_model_handle: active_record/relation.rbs not in the tree".to_string()
        })?;
    let relation = &mut files[idx].1;
    // Exact pairs: find_by conditions widen past `Base`. Spawn no longer
    // takes a records param (`take_query_lists` + clone).
    let replacements = [
        (
            "    def find_by: (Hash[Symbol, untyped] | String | nil conditions) -> Base?\n",
            "    def find_by: (untyped conditions) -> untyped\n",
        ),
        (
            "    def find_by!: (Hash[Symbol, untyped] | String | nil conditions) -> Base\n",
            "    def find_by!: (untyped conditions) -> untyped\n",
        ),
    ];
    for (narrow, wide) in replacements {
        if !relation.contains(narrow) {
            return Err(format!(
                "spinel_relation_model_handle: relation.rbs no longer declares {narrow:?}"
            ));
        }
        *relation = relation.replace(narrow, wide);
    }
    *relation = widen_spinel_base_on_sigs(relation);

    let conn_idx = files
        .iter()
        .position(|(p, _)| {
            p == "sig/runtime/active_record/connection.rbs"
                || p == "runtime/active_record/connection.rbs"
        })
        .ok_or_else(|| {
            "spinel_relation_model_handle: active_record/connection.rbs not in the tree"
                .to_string()
        })?;
    let conn = &mut files[conn_idx].1;
    // Dynamic `Model.first` / `Model.take` live here, not on Relation.
    let conn_pairs = [
        (
            "    def self.first: () -> Base?\n",
            "    def self.first: () -> untyped\n",
        ),
        (
            "    def self.take: () -> Base?\n",
            "    def self.take: () -> untyped\n",
        ),
    ];
    for (narrow, wide) in conn_pairs {
        if !conn.contains(narrow) {
            return Err(format!(
                "spinel_relation_model_handle: connection.rbs no longer declares {narrow:?}"
            ));
        }
        *conn = conn.replace(narrow, wide);
    }
    *conn = widen_spinel_base_on_sigs(conn);

    // The public find/exists? nil guards live in the connection reopen.
    // Spinel seeds their inherited adapter calls from Base's declaration,
    // even when a String-keyed model overrides the adapter with a String
    // argument. Follow the app-wide id contract established by
    // widen_key_contract: Integer-only apps keep their original seeds,
    // while apps with String keys need both kinds at shared dispatch.
    // Each generated model still declares its schema's scalar.
    let base = files
        .iter_mut()
        .find(|(p, _)| {
            p == "sig/runtime/active_record/base.rbs" || p == "runtime/active_record/base.rbs"
        })
        .ok_or_else(|| {
            "spinel_relation_model_handle: active_record/base.rbs not in the tree".to_string()
        })?;
    if base.1.contains("    def id: () -> (Integer | String)\n") {
        for method in ["self._adapter_find_by_id", "self._adapter_exists_by_id?"] {
            let narrow = format!("    def {method}: (Integer id)");
            let wide = format!("    def {method}: (Integer | String id)");
            if !base.1.contains(&narrow) {
                return Err(format!(
                    "spinel_relation_model_handle: base.rbs no longer declares {narrow:?}"
                ));
            }
            base.1 = base.1.replace(&narrow, &wide);
        }
    }
    resolve_runtime_sig_conflicts(files)
}

/// Two runtime RBS files can declare one method with different
/// signatures: the shared runtime declares a class's methods for every
/// target, and a ruby-family reopen or a Spinel-specific file re-declares
/// the ones it overrides (`ActiveRecord::Base.where` as a `Relation` in
/// connection.rbs, not an `Array[Base]`; `ActiveStorage::Service#upload`
/// returning `nil` in Spinel's active_storage_disk.rbs). Spinel used to let
/// whichever seed `readdir` visited later replace the other, which is
/// filesystem order; since spinel e6b845d1f ("Two declarations of one
/// method that disagree are an error") it refuses the build.
///
/// So the Spinel tree keeps one declaration per method, chosen the way the
/// Ruby definitions override each other: a file from runtime/spinel beats
/// one from runtime/ruby, and within one runtime a reopen beats the
/// class's primary file (active_record/base.rbs for ActiveRecord::Base).
/// A conflict the rule cannot decide fails the emit, rather than leaving
/// it to readdir.
fn resolve_runtime_sig_conflicts(files: &mut [(String, String)]) -> Result<(), String> {
    use std::collections::HashMap;
    struct Decl {
        file: usize,
        line: usize,
        sig: String,
    }
    // (qualified class, method) -> its declarations, from every runtime .rbs.
    let mut decls: HashMap<(String, String), Vec<Decl>> = HashMap::new();
    for (fi, (path, text)) in files.iter().enumerate() {
        if !path.ends_with(".rbs") || !(path.starts_with("runtime/") || path.starts_with("sig/runtime/")) {
            continue;
        }
        let mut stack: Vec<(usize, String)> = Vec::new();
        for (li, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let opener = ["class ", "module ", "interface "].iter().find_map(|k| trimmed.strip_prefix(k));
            if let Some(rest) = opener {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                stack.push((indent, name));
                continue;
            }
            if trimmed == "end" || trimmed.starts_with("end ") {
                while stack.last().is_some_and(|(i, _)| *i >= indent) {
                    stack.pop();
                }
                continue;
            }
            let Some(rest) = trimmed.strip_prefix("def ") else { continue };
            let Some(colon) = rest.find(':') else { continue };
            if stack.is_empty() {
                continue;
            }
            let class = stack.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join("::");
            let method = rest[..colon].trim().to_string();
            let sig = rest[colon + 1..].split_whitespace().collect::<Vec<_>>().join(" ");
            decls.entry((class, method)).or_default().push(Decl { file: fi, line: li, sig });
        }
    }
    // Rank of the file a declaration sits in: higher wins.
    let rank = |fi: usize, class: &str| -> u8 {
        let path = files[fi].0.trim_start_matches("sig/");
        let rest = path.trim_start_matches("runtime/");
        let spinel = crate::runtime_files::exists(&format!("runtime/spinel/{rest}"))
            && !crate::runtime_files::exists(&format!("runtime/ruby/{rest}"));
        let primary = format!("{}.rbs", crate::naming::underscore(class).replace("::", "/"));
        let reopen = rest != primary;
        (spinel as u8) * 2 + reopen as u8
    };
    let mut drop: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut undecided = Vec::new();
    for ((class, method), ds) in &decls {
        let first = &ds[0].sig;
        if ds.iter().all(|d| &d.sig == first) {
            continue;
        }
        let best = ds.iter().map(|d| rank(d.file, class)).max().unwrap_or(0);
        let winners: Vec<&Decl> = ds.iter().filter(|d| rank(d.file, class) == best).collect();
        if winners.iter().any(|d| d.sig != winners[0].sig) {
            undecided.push(format!(
                "{class}#{method}: {}",
                ds.iter().map(|d| format!("{} `{}`", files[d.file].0, d.sig)).collect::<Vec<_>>().join(" vs ")
            ));
            continue;
        }
        for d in ds.iter().filter(|d| rank(d.file, class) != best) {
            drop.entry(d.file).or_default().push(d.line);
        }
    }
    if !undecided.is_empty() {
        undecided.sort();
        return Err(format!(
            "resolve_runtime_sig_conflicts: runtime RBS declares a method twice and no file overrides the other:\n  {}",
            undecided.join("\n  ")
        ));
    }
    for (fi, lines) in drop {
        let text = &mut files[fi].1;
        let mut kept = String::with_capacity(text.len());
        for (i, line) in text.lines().enumerate() {
            if !lines.contains(&i) {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        *text = kept;
    }
    Ok(())
}

/// A plain class the app declares outside the Rails base classes
/// (`app/models/probe.rb` holding `class Probe`, or one under
/// `app/lib`/`lib`) is an `app.library_classes` entry. The ruby family
/// and TypeScript emit those; the other emitters only emit the library
/// classes they lower themselves (views, fixtures, tests), so the
/// class is silently missing while the tests and controllers that
/// call it are emitted, and the build fails later on an unknown name.
/// Report each one per target until those emitters carry them.
///
/// The gate is a denylist of targets that already emit plain library
/// classes (fail-closed for new `BuildTarget`s), matching sibling
/// `report_*` polarity. Roda is intentionally not on that list: its
/// spike emit never walks `app.library_classes`, so a PORO would stay
/// silently dropped under an allowlist of today's non-emitters.
///
/// Mixin modules are not reported: their bodies are spliced into the
/// including models by the shared lowering. Synthesized classes
/// (`origin` set) belong to the lowerer that made them. Classes whose
/// ancestry reaches a framework base (`ApplicationJob < ActiveJob::Base`,
/// `ApplicationMailer < ActionMailer::Base` and their subclasses) are
/// not plain Ruby and are left to the job and mailer handling.
fn target_emits_app_library_classes(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Blog
            | BuildTarget::Ruby
            | BuildTarget::Jruby
            | BuildTarget::Spinel
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker
        // Roda omitted: spike emit does not walk `app.library_classes`.
    )
}

fn report_unemitted_library_classes(app: &App, target: BuildTarget) {
    if target_emits_app_library_classes(target) {
        return;
    }
    for lc in &app.library_classes {
        if lc.is_module || lc.origin.is_some() || !is_plain_ruby_class(app, lc) {
            continue;
        }
        let span = lc
            .methods
            .first()
            .map(|m| m.name_span)
            .unwrap_or_else(crate::span::Span::synthetic);
        emit::diagnostics::report_unsupported(
            span,
            target.as_str(),
            "plain Ruby class",
            format!(
                "class `{}` is not emitted for this target (the ruby and typescript emits carry it)",
                lc.name.0.as_str()
            ),
        );
    }
}

/// True when every ancestor of `lc` is another app library class (or
/// explicit `Object`), so the chain ends at `Object` rather than a
/// framework / gem / stdlib base. Explicit `Object` is treated as the
/// same terminal as an omitted superclass; other unknown parents reject.
fn is_plain_ruby_class(app: &App, lc: &crate::dialect::LibraryClass) -> bool {
    let mut parent = lc.parent.as_ref();
    let mut hops = 0;
    while let Some(p) = parent {
        hops += 1;
        if hops > app.library_classes.len() {
            return false;
        }
        match app.library_classes.iter().find(|c| c.name == *p) {
            Some(c) => parent = c.parent.as_ref(),
            None if p.0.as_str() == "Object" => parent = None,
            None => return false,
        }
    }
    true
}

/// A non-integer primary key (`create_table …, id: :uuid`,
/// `primary_key: "identifier", id: :string`) is carried end to end by
/// the ruby-shape emit — CRuby, JRuby and Spinel: the analyzer types
/// `id`/`ids`/the finders from the key column, the synthesized
/// `_adapter_*` primitives write, compare and answer it, the base
/// holds no `@id` slot, and the shipped sidecar's key contract is
/// widened to the app's keys (`widen_key_contract`) (#90). The
/// compiled targets' runtimes still pin `id` as a 64-bit integer in
/// their model structs — so for those the key is an unsupported
/// construct, reported here per target rather than as an ingest gap
/// that would be false of the ruby family. `Severity::Error` fails
/// the transpile on the CLI unless `--allow-unsupported`; the
/// diagnostic names the table and the key so the inventory reads as
/// a ledger.
fn report_unsupported_keys(app: &App, target: BuildTarget) {
    if matches!(
        target,
        BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
    ) {
        return;
    }
    for table in app.schema.tables.values() {
        let Some(key) = table.columns.iter().find(|c| c.primary_key) else { continue };
        if matches!(
            key.col_type,
            crate::schema::ColumnType::Integer | crate::schema::ColumnType::BigInt
        ) {
            continue;
        }
        emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            crate::span::Span::synthetic(),
            Some(crate::ident::Symbol::from(target.as_str())),
            "non_integer_primary_key",
            format!(
                "table {}: key `{}` is {:?}; this target's model layer pins an integer id (the ruby and spinel emits carry it)",
                table.name.as_str(),
                key.name.as_str(),
                key.col_type
            ),
        ));
    }
}

/// True when the app schema or emitted roots mention a date-only value.
/// Shared by the unsupported-target gate and Spinel's conditional Date
/// runtime load (matz/spinel#7334: defining `Date#strftime` currently
/// breaks poly `Time | Date` dispatch for `Time#strftime`).
fn app_uses_date(app: &App) -> bool {
    fn expr_has_date(e: &crate::expr::Expr) -> bool {
        if e.ty.as_ref().is_some_and(crate::ty::Ty::contains_date)
            || matches!(&*e.node, crate::expr::ExprNode::Cast { target_ty, .. } if target_ty.contains_date())
            || matches!(&*e.node, crate::expr::ExprNode::Const { path }
                if (path.len() == 1 || (path.len() == 2 && path[0].as_str().is_empty()))
                    && path.last().is_some_and(|name| name.as_str() == "Date"))
        {
            return true;
        }
        let mut found = false;
        e.node.for_each_child(&mut |child| found |= expr_has_date(child));
        found
    }
    fn method_has_date(m: &crate::dialect::MethodDef) -> bool {
        m.signature.as_ref().is_some_and(crate::ty::Ty::contains_date)
            || expr_has_date(&m.body)
            || m.params.iter().filter_map(|p| p.default.as_ref()).any(expr_has_date)
    }
    fn class_has_date(lc: &crate::dialect::LibraryClass) -> bool {
        lc.methods.iter().any(method_has_date)
            || lc.constants.iter().any(|(_, e)| expr_has_date(e))
            || lc.unknown_calls.iter().any(expr_has_date)
    }
    let mut has_date = app.schema.tables.values().any(|table|
        table.columns.iter().any(|c| c.col_type == crate::schema::ColumnType::Date));
    crate::lower::for_each_hook_body_ref(app, &mut |e| has_date |= expr_has_date(e));
    for view in &app.views {
        has_date |= expr_has_date(&view.body);
        has_date |= view.strict_locals.iter().flatten()
            .filter_map(|p| p.default.as_ref()).any(expr_has_date);
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                has_date |= method_has_date(method);
            }
        }
    }
    for lc in app.library_classes.iter().chain(app.rails_application.iter()) {
        has_date |= class_has_date(lc);
    }
    // Hook bodies already include controller bodies/positional defaults
    // and seeds. Keyword defaults, tests and fixtures are emitted roots
    // too, even when they have not been analyzed at this boundary.
    for controller in &app.controllers {
        for item in &controller.body {
            if let crate::dialect::ControllerBodyItem::Action { action, .. } = item {
                has_date |= action.params.fields.values().any(crate::ty::Ty::contains_date);
                has_date |= action.kw_params.iter().filter_map(|(_, e)| e.as_ref()).any(expr_has_date);
            }
        }
    }
    for tm in &app.test_modules {
        has_date |= tm.setup.as_ref().is_some_and(expr_has_date)
            || tm.tests.iter().any(|t| expr_has_date(&t.body))
            || tm.helpers.iter().any(method_has_date)
            || tm.inner_classes.iter().any(class_has_date)
            || tm.constants.iter().any(|(_, e)| expr_has_date(e));
    }
    for fixture in &app.fixtures {
        has_date |= fixture.preamble.iter().any(expr_has_date)
            || fixture.records.values().flat_map(|r| r.values()).any(|value|
                matches!(value, crate::dialect::FixtureValue::Ruby(e) if expr_has_date(e)));
    }
    has_date |= app.routes.direct_helpers.iter().any(|h| expr_has_date(&h.body));
    for function in &app.sql_functions {
        has_date |= match &function.kind {
            crate::app::SqlFunctionKind::Scalar { method } => method_has_date(method),
            crate::app::SqlFunctionKind::Aggregate { step, finalize } =>
                method_has_date(step) || method_has_date(finalize),
        };
    }
    has_date |= app.rbs_signatures.values().flat_map(|methods| methods.values())
        .any(crate::ty::Ty::contains_date);
    has_date
}

/// Insert Date-package requires after `active_record_serialization` in
/// `boot.rb`. Spinel only — the program-defined `Date` class and the
/// date-column JSON reopen belong on that target (matz/spinel#7334).
fn inject_date_package_into_boot(boot: &mut String) -> Result<(), String> {
    let anchor = "require_relative \"runtime/active_record_serialization\"\n";
    // Prefer concat! over one escaped multiline string: the CI
    // planner's project.rs body-scope regex backtracks for minutes
    // on `\"` + `\` continuations in a single literal this large.
    let inject = concat!(
        "# Date package — only when the app uses date-only values (matz/spinel#7334).\n",
        "require_relative \"runtime/date\"\n",
        "require_relative \"runtime/active_support_date_parsing\"\n",
        "require_relative \"runtime/active_record_date_serialization\"\n",
    );
    if boot.contains("require_relative \"runtime/date\"") {
        return Ok(());
    }
    if let Some(at) = boot.find(anchor) {
        boot.insert_str(at + anchor.len(), inject);
        Ok(())
    } else {
        Err(
            "boot.rb missing active_record_serialization require \
             (Date package inject anchor)"
                .into(),
        )
    }
}

/// CRuby/JRuby overlay boot replaces Spinel's boot wholesale, wiping
/// the Date-package inject. Re-inject **only** the shared `date_*`
/// calendar helpers (invariant 2). Do not load Spinel's `date.rb`
/// polyfill (clobbers stdlib Date → `@year` nil on `>>`) or
/// `active_record_date_serialization` (overlay already wraps
/// `_as_json_only`; a second alias infinite-recurses).
fn inject_cruby_date_calendar_helpers(boot: &mut String) -> Result<(), String> {
    let anchor = "require_relative \"runtime/active_record_serialization\"\n";
    let inject = concat!(
        "# Date calendar helpers — shared with Spinel (invariant 2).\n",
        "require_relative \"runtime/active_support_date_parsing\"\n",
    );
    if boot.contains("require_relative \"runtime/active_support_date_parsing\"") {
        return Ok(());
    }
    if let Some(at) = boot.find(anchor) {
        boot.insert_str(at + anchor.len(), inject);
        Ok(())
    } else {
        Err(
            "boot.rb missing active_record_serialization require \
             (CRuby date calendar inject anchor)"
                .into(),
        )
    }
}

fn inject_cruby_date_calendar_boot_requires(
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let boot = files
        .iter_mut()
        .find(|(p, _)| p == "boot.rb")
        .ok_or("ruby family: boot.rb missing for date calendar re-inject")?;
    inject_cruby_date_calendar_helpers(&mut boot.1)
}

/// The executed Date-only runtime is native Ruby (and now Spinel), not
/// the timestamp seam shared by the other targets (including the
/// unverified JRuby adapter). Reject before entering their emitters:
/// dynamic backends may never render a type, so a type-position check
/// alone would silently emit a String/Time or call an absent intrinsic.
fn reject_unsupported_dates(app: &App, target: BuildTarget) -> Result<(), String> {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Spinel) {
        return Ok(());
    }
    if app_uses_date(app) {
        emit::diagnostics::unsupported_date_ty(target.as_str());
        return Err(format!("{}: Date-only values are not supported; use the native Ruby target", target.as_str()));
    }
    Ok(())
}

/// An `ActiveStorage::FixtureSet.blob` row loads by reading the file
/// under `test/fixtures/files` and uploading it to a storage service.
/// Only the ruby family has one (`runtime/spinel/active_storage_disk.rb`);
/// elsewhere the shared runtime's service raises, and the native fixture
/// emitters have no loader for the row. Refuse rather than load a set
/// without it.
fn reject_file_blob_fixtures(app: &App, target: BuildTarget) -> Result<(), String> {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
        return Ok(());
    }
    if let Some((fixture, label)) = app.fixtures.iter()
        .find_map(|f| f.file_blobs.keys().next().map(|label| (f, label)))
    {
        return Err(format!(
            "{}: fixture `test/fixtures/{}.yml` record `{}` is an `ActiveStorage::FixtureSet.blob` row; file-fixture blobs load only on the Ruby targets",
            target.as_str(),
            fixture.path.as_str(),
            label.as_str(),
        ));
    }
    Ok(())
}

/// Keep admitted Data factories outside unverified target emitters.
fn reject_unsupported_data_factories(app: &App, target: BuildTarget) -> Result<(), String> {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Spinel) {
        return Ok(());
    }
    fn visit(expr: &crate::expr::Expr, target: &str, found: &mut bool) {
        if expr.decisions & crate::expr::RESOLVED_DATA_FACTORY != 0 {
            *found = true;
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "Data.define",
                "this target has no supported Data factory representation; use Ruby or Spinel",
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target, found));
    }
    let mut found = false;
    crate::lower::for_each_forwarding_body_ref(app, &mut |expr| {
        visit(expr, target.as_str(), &mut found);
    });
    if found {
        return Err(format!("{}: Data.define is not supported; use Ruby or Spinel", target.as_str()));
    }
    Ok(())
}

/// Arbitrary `&expr` operands need a real forwarding convention, not
/// a lambda that returns the operand (or a dropped block). Keep the
/// unsupported native paths out of emit, even in survey mode.
fn reject_unsupported_forwarded_procs(app: &App, target: BuildTarget) -> Result<(), String> {
    if !matches!(target, BuildTarget::Rust | BuildTarget::Crystal | BuildTarget::Go
        | BuildTarget::Python | BuildTarget::Kotlin | BuildTarget::Swift | BuildTarget::Elixir) {
        return Ok(());
    }
    fn visit(e: &crate::expr::Expr, target: &str, found: &mut bool) {
        use crate::expr::ExprNode;
        if let ExprNode::Send { block: Some(block), .. }
            | ExprNode::Apply { block: Some(block), .. } = &*e.node
        {
            // These shapes predate the arbitrary-expression fallback;
            // their existing target-specific paths remain unchanged.
            // The bytes primitive implements literal &nil as no block. Use
            // its shared classifier, without accepting other &expr calls.
            if !matches!(&*block.node, ExprNode::Lambda { .. } | ExprNode::Var { .. }
                | ExprNode::MethodRef { .. })
                && !crate::emit::shared::string_bytes::materializes_array(e) {
                *found = true;
                crate::emit::diagnostics::report_unsupported(
                    block.span, target, "forwarded_proc",
                    "arbitrary &expr forwarding is not implemented on this target; use Ruby instead",
                );
            }
        }
        e.node.for_each_child(&mut |child| visit(child, target, found));
    }
    let mut found = false;
    let mut f = |e: &crate::expr::Expr| visit(e, target.as_str(), &mut found);
    crate::lower::for_each_emit_body_ref(app, &mut f);
    if found {
        return Err(format!("{}: arbitrary &expr Proc forwarding is not supported; use Ruby instead", target.as_str()));
    }
    Ok(())
}

/// Targets whose emitters name, file and dispatch a module-qualified
/// controller (`Admin::StatsController`) correctly. The others write
/// the `::` into identifiers and paths (TypeScript, Crystal, C#,
/// Kotlin, Swift), or derive a module/constructor/dispatch key that
/// disagrees with the route table's (Rust, Go, Python) — true of an
/// app's own namespaced controllers as well. Elixir and the Roda
/// conversion have no run proving one either way.
fn emits_namespaced_controllers(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel)
}

/// The app without `Rails::HealthController`, for a target that cannot
/// emit a namespaced controller: ingest synthesizes it for the `/up`
/// route, and emitting it there breaks the build or misroutes it,
/// which the route alone did not (it left `/up` unserved). Matched by name,
/// so an app's own `Rails::HealthController` is dropped there too; it
/// hit the same gap. `None` when there is nothing to drop. The warning
/// keeps the 404 on the ledger.
fn without_rails_health_controller(app: &App, target: BuildTarget) -> Option<App> {
    let health = crate::ingest::routes::RAILS_HEALTH_CONTROLLER;
    if emits_namespaced_controllers(target) || !app.controllers.iter().any(|c| c.name.0.as_str() == health) {
        return None;
    }
    let mut d = crate::diagnostic::Diagnostic::unsupported(
        crate::span::Span::synthetic(),
        Some(crate::ident::Symbol::from(target.as_str())),
        "namespaced_controller",
        format!("{health} is not emitted: this target does not emit namespaced controllers yet, so `rails/health#show` is not served"),
    );
    d.severity = crate::diagnostic::Severity::Warning;
    emit::diagnostics::push(d);
    let mut app = app.clone();
    app.controllers.retain(|c| c.name.0.as_str() != health);
    Some(app)
}

/// A unique index whose `where:` SQLite can't be trusted to run as
/// written — a Postgres dump's `((kind)::text = 'initial'::text)` or
/// `= ANY (ARRAY[…])` — is unique over every row in the SQLite DDL, as
/// it was before predicates were kept (`Dialect::index_predicate`). It
/// rejects rows Rails accepts, so each one is named. A warning, not an
/// error: the tree still runs, with a stricter index than the app's.
fn report_sqlite_index_predicates(app: &App, target: BuildTarget) {
    // The Roda conversion writes Sequel migrations, which carry no
    // predicate at all.
    if target == BuildTarget::Roda {
        return;
    }
    for table in app.schema.tables.values() {
        for index in &table.indexes {
            let Some(predicate) = index.predicate.as_deref() else { continue };
            if !index.unique
                || crate::emit::shared::schema_sql::Dialect::Sqlite
                    .index_predicate(table, index)
                    .is_some()
            {
                continue;
            }
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                crate::span::Span::synthetic(),
                None,
                "partial_unique_index",
                format!(
                    "table {}: `{predicate}`, the `where:` of unique index `{}`, is not \
                     one SQLite reads alike, so its SQLite index is unique over every row",
                    table.name.as_str(),
                    index.name.as_str()
                ),
            );
            d.severity = crate::diagnostic::Severity::Warning;
            emit::diagnostics::push(d);
        }
    }
}

/// Report syntax whose runtime contract is currently native Ruby only.
fn report_native_ruby_syntax(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget) {
        use crate::expr::{ExprNode, LValue};
        let construct = match &*expr.node {
            ExprNode::ForwardKeywords if target == BuildTarget::Spinel => None,
            ExprNode::ForwardKeywords => Some("anonymous keyword forwarding"),
            ExprNode::ForwardKeywordsWithPairs { .. } if target == BuildTarget::Spinel => None,
            ExprNode::ForwardKeywordsWithPairs { .. } => Some("anonymous keyword forwarding"),
            ExprNode::Defined { .. } => Some("runtime defined? query"),
            // The Ruby emitter (Spinel's and Roda's too) renders the whole
            // signature; the other emitters read only the required names
            // and would drop the rest, which the body still reads.
            ExprNode::Lambda { extra_params, .. }
                if !extra_params.is_empty() && !matches!(target, BuildTarget::Spinel | BuildTarget::Roda) =>
            {
                Some("lambda optional, keyword or keyword-rest parameters")
            }
            ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. }
                if name.as_str().starts_with("@@") => Some("class variable write"),
            ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. }
                if name.as_str().starts_with('$') => Some("global variable write"),
            ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => Some("class variable read"),
            _ => None,
        };
        if let Some(construct) = construct {
            crate::emit::diagnostics::report_unsupported(
                expr.span, target.as_str(), construct,
                "native Ruby semantics have no verified implementation on this target",
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target));
}

/// `case/in` is not equivalent to the targets' existing `case/when`
/// renderers, even for nil or a plain binding. Refuse before file emission
/// rather than lose bindings, skip evaluation, or turn a test into a wildcard.
fn reject_unsupported_pattern_matches(app: &App, target: BuildTarget) -> Result<(), String> {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda) {
        return Ok(());
    }
    fn visit(e: &crate::expr::Expr, target: &str, found: &mut bool) {
        use crate::expr::ExprNode;
        if matches!(&*e.node, ExprNode::CaseMatch { .. } | ExprNode::MatchPredicate { .. }
            | ExprNode::MatchRequired { .. }) {
            *found = true;
            emit::diagnostics::report_unsupported(e.span, target, e.node.kind_str(),
                "structural pattern matching requires a native Ruby target");
        }
        e.node.for_each_child(&mut |child| visit(child, target, found));
    }
    let mut found = false;
    let mut f = |e: &crate::expr::Expr| visit(e, target.as_str(), &mut found);
    crate::lower::for_each_emit_body_ref(app, &mut f);
    if found {
        return Err(format!("{}: structural pattern matching requires a native Ruby target", target.as_str()));
    }
    Ok(())
}

/// Generated fixture attributes cannot be reproduced by the current
/// model-based fixture loaders: their in-memory setter would expose the
/// supplied YAML value even though persistence correctly omits that
/// database-owned column. Fail at the source fixture and record instead
/// of silently loading a different value. The Blog target ships the
/// Rails fixture source verbatim and does not use these loaders.
fn reject_generated_fixture_assignments(app: &App) -> Result<(), String> {
    let lowered = crate::lower::lower_fixtures(app);
    for fixture in &lowered.fixtures {
        let Some(source_fixture) = app.fixtures.iter().find(|source| source.name == fixture.name) else {
            continue;
        };
        let Some(model) = app.models.iter().find(|model| model.name == fixture.class) else {
            continue;
        };
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        for record in &fixture.records {
            for field in &record.fields {
                if table.columns.iter().any(|column| {
                    column.name == field.column && column.generated.is_some()
                }) {
                    return Err(format!(
                        "fixture `test/fixtures/{}.yml` record `{}` assigns generated column `{}.{}`; generated fixture values are not supported",
                        source_fixture.path.as_str(),
                        record.label.as_str(),
                        table.name.as_str(),
                        field.column.as_str(),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn target_files(
    app: &App,
    fixture: &Path,
    target: BuildTarget,
) -> Result<Vec<(String, String)>, String> {
    let without_health;
    let app = match without_rails_health_controller(app, target) {
        Some(trimmed) => {
            without_health = trimmed;
            &without_health
        }
        None => app,
    };
    if target == BuildTarget::Roda {
        if let Some((table, column)) = app.schema.tables.values().find_map(|table| {
            table
                .columns
                .iter()
                .find(|column| column.generated.is_some())
                .map(|column| (table.name.as_str(), column.name.as_str()))
        }) {
            return Err(format!(
                "Roda target does not support generated column `{table}.{column}`"
            ));
        }
    } else if target != BuildTarget::Blog {
        if let Some((table, column)) = app.schema.tables.values().find_map(|table| {
            table
                .columns
                .iter()
                .find(|column| column.generated.is_some())
                .map(|column| (table.name.as_str(), column.name.as_str()))
        }) && !target.supports_generated_column_insert_returning()
        {
            return Err(format!(
                "{} target does not support generated-column model persistence: its runtime does not implement Db.exec_returning for `{table}.{column}`",
                target.as_str()
            ));
        }
        crate::emit::shared::schema_sql::validate_schema_for_dialect(
            &app.schema,
            crate::emit::shared::schema_sql::Dialect::Sqlite,
        )?;
        reject_generated_fixture_assignments(app)?;
    }
    // Before the refusals below: a refusal returns early, and a
    // reference that it hides would leave the transpile with fewer
    // errors than the app has.
    report_unsupported_bundled_constants(app, target);
    reject_unsupported_pattern_matches(app, target)?;
    reject_file_blob_fixtures(app, target)?;
    reject_unsupported_data_factories(app, target)?;
    reject_unsupported_dates(app, target)?;
    reject_unsupported_forwarded_procs(app, target)?;
    report_unsupported_keys(app, target);
    report_unemitted_library_classes(app, target);
    report_sqlite_index_predicates(app, target);
    report_native_ruby_syntax(app, target);
    // Full forwarding currently has a native Ruby contract only. A
    // declaration must be gated even when its body never forwards.
    if !matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
        for (span, policy) in crate::analyze::forwarding::keyword_calls(app) {
            if policy != crate::analyze::forwarding::KeywordPolicy::Legacy {
                let (construct, detail) = if policy == crate::analyze::forwarding::KeywordPolicy::RefuseOrdinarySuper {
                    ("keyword splat in ordinary super",
                     "super destination's native or lowered argument ABI cannot be verified")
                } else {
                    ("keyword splat into full argument forwarding",
                     "native Ruby keyword provenance has no verified carrier on this target")
                };
                crate::emit::diagnostics::report_unsupported(span, target.as_str(),
                    construct, detail);
            }
        }
    }
    // Controller actions too: their `*rest`, `**` and `...` reach the
    // emitted `def` through the same `Param`s (`Action::formal_params`).
    let actions = crate::analyze::forwarding::controller_defs(app);
    for method in crate::analyze::forwarding::methods(app)
        .map(|(_, m)| m)
        .chain(actions.iter().map(|(_, m)| m))
    {
        if target != BuildTarget::Blog && let Some(formal) = method.unsupported_formals {
            crate::emit::diagnostics::report_unsupported(method.name_span, target.as_str(), "parameter declaration", formal.description());
        }
        report_positional_rest_param(method, target);
        if !matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
            let keyword_rest = method
                .params
                .iter()
                .any(|p| p.keyword && p.rest && !p.forwarding);
            let full_forwarding = method.params.iter().any(|p| p.forwarding);
            // Spinel carries both named `**details` and anonymous `**`
            // keyword-rest parameters. Full `...` forwarding stays refused.
            if matches!(target, BuildTarget::Spinel) && keyword_rest && !full_forwarding {
                continue;
            }
            let construct = if full_forwarding {
                "full argument forwarding"
            } else if method.params.iter().any(|p| p.keyword && p.rest) {
                "keyword rest declaration"
            } else {
                continue;
            };
            crate::emit::diagnostics::report_unsupported(
                method.name_span,
                target.as_str(),
                construct,
                "native Ruby forwarding is preserved; this target's argument/block carrier is not verified",
            );
        }
    }
    // A keyword parameter is carried by the ruby family and by nothing
    // else yet. No other emitter reads `Param::keyword`, so a `def`
    // that declares one renders POSITIONALLY while its call site
    // renders a hash or an object literal — a mismatch that shows up
    // only when the emitted code runs. Say so instead.
    //
    // Not converted to positionals on the way out: that would lose the
    // two things that make a keyword a keyword, any order and skipping
    // an optional one, and a target that cannot express the construct
    // should report it rather than receive a lossy rewrite.
    if !matches!(
        target,
        BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda
    ) {
        report_keyword_params(app, target.as_str());
    }
    if !matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
        for controller in &app.controllers {
            if controller.class_methods().next().is_some() {
                return Err(format!("finite class-side configuration is not supported ({})", target.as_str()));
            }
        }
        if app.library_classes.iter().any(|lc| !lc.class_ivar_initializers.is_empty()) {
            return Err(format!("class-instance-variable initialization is not supported ({})", target.as_str()));
        }
    }
    // Spinel binds a repeated method name to its final implementation even
    // in an earlier class initializer. CRuby/JRuby execute each definition
    // in order; refuse this shape before producing a silently different app.
    if target == BuildTarget::Spinel {
        // Ingest keeps reopened declarations separate, but they share the
        // same class object and method table, including its initializers.
        let mut classes = std::collections::BTreeMap::new();
        for lc in &app.library_classes {
            classes.entry(&lc.name).or_insert_with(Vec::new).push(lc);
        }
        for (name, declarations) in classes {
            // Synthetic mattr/cattr `@@` seeds are not the Spinel ordering
            // hazard — only direct source `@ivar` initialization is.
            if declarations.iter().all(|lc| !lc.has_source_ivar_initializers()) {
                continue;
            }
            let mut methods = std::collections::HashSet::new();
            for m in declarations.iter().flat_map(|lc| &lc.methods) {
                if !methods.insert((m.receiver == crate::dialect::MethodReceiver::Class, &m.name)) {
                    return Err(format!(
                        "class-instance-variable initialization with method redefinition is not supported (spinel): {}.{}",
                        name.0, m.name,
                    ));
                }
            }
        }
    }
    let files = crate::timings::phase(format_args!("emit {}: assemble", target.as_str()), || match target {
        BuildTarget::Blog => blog_files(fixture),
        BuildTarget::Spinel => spinel_files_with_source_markers(app, fixture).and_then(|(mut files, _)| {
            spinel_relation_model_handle(&mut files)?;
            spin_shape(files)
        }),
        // The ruby family gets the bundled-library requires too: the
        // table used to live inside `spin_shape` and so reached only
        // the spinel tree, which cost campfire two test files on a
        // Ruby 3.4 runner (`Pathname()`).
        BuildTarget::Ruby => ruby_runtime_files(app, fixture).map(with_bundled_requires),
        BuildTarget::Jruby => jruby_runtime_files(app, fixture).map(with_bundled_requires),
        BuildTarget::Roda => Ok(sort_files(emit::roda::emit(app))),
        BuildTarget::Crystal => Ok(sort_files(emit::crystal::emit(app))),
        BuildTarget::Elixir => Ok(sort_files(emit::elixir::emit(app))),
        BuildTarget::Go => Ok(sort_files(emit::go::emit(app))),
        BuildTarget::Kotlin => Ok(sort_files(emit::kotlin::emit(app))),
        BuildTarget::Python => Ok(sort_files(emit::python::emit(app))),
        BuildTarget::Rust => Ok(sort_files(emit::rust::emit(app))),
        BuildTarget::Swift => Ok(sort_files(emit::swift::emit(app))),
        BuildTarget::CSharp => Ok(sort_files(emit::csharp::emit(app))),
        BuildTarget::Typescript => Ok(sort_files(emit::typescript::emit(app))),
        BuildTarget::TypescriptWorker => Ok(sort_files(emit::typescript::emit_with_profile(
            app,
            &crate::profile::DeploymentProfile::worker(),
        ))),
    })?;

    // Ruby-family trees ship the framework runtime as verbatim text, so
    // their tree-shake runs here, on the finished file set (after
    // overlays — an overlay-only caller must count as a root), as a
    // text-level pass: see emit::ruby::shake. Other targets shake in IR
    // during emit.
    let files = if matches!(
        target,
        BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby
    ) {
        let mut synth_shakeable: std::collections::HashSet<String> = app
            .models
            .iter()
            .flat_map(|m| app.schema.tables.get(&m.table.0).into_iter()
                .flat_map(|t| crate::lower::model_to_library::shakeable_synthesized_names(t, m)))
            .map(|s| s.as_str().to_string())
            .collect();
        // Ruby's text-level shake uses a global name set. An optional
        // predicate on another model must not expose a user's enum override
        // to shaking; conservatively keep that name on every model.
        for model in &app.models {
            for column in model.enums.keys() {
                let predicate = crate::ident::Symbol::from(format!("{}?", column.as_str()));
                if crate::lower::model_to_library::model_defines_instance_method(model, &predicate) {
                    synth_shakeable.remove(predicate.as_str());
                }
            }
        }
        let mut files = files;
        crate::timings::phase(format_args!("emit {}: tree shake", target.as_str()), || {
            emit::ruby::shake::shake_tree(&mut files, &synth_shakeable, target.as_str());
        });
        files
    } else {
        files
    };

    // Blog is the verbatim Rails source — it ships `db/seeds.rb` and is
    // seeded by Rails, so it needs no SQL seed. Every transpile target
    // gets a language-agnostic `db/seed.sql` so the published archive is
    // self-contained-seedable (`sqlite3 <db> < db/seed.sql`) with no Ruby
    // — see e2e harness (scripts/e2e). spinel/ruby/jruby already carry it
    // via the scaffold walk; inject-if-absent is a no-op there.
    let files = if target == BuildTarget::Blog {
        files
    } else {
        let files = ensure_seed_sql(files, app)?;
        let files = ensure_storage_keep(files, target);
        let files = ensure_static_assets(files, target);
        ensure_e2e(files, target)
    };
    Ok(ensure_readme(files, target))
}

/// Targets whose archives ship the Playwright e2e suite under `e2e/`
/// (and the matching `## End-to-end` README section). The archive is
/// the complete test artifact — `scripts/smoke` just runs the README's
/// steps against the unpacked tgz, subsuming the per-target
/// `toolchain-<t>`/`e2e-<t>` CI jobs.
///
/// Excluded: TypescriptWorker (no standalone server) and Blog (source
/// fixture). All three scaffold targets participate, including Spinel:
/// their scaffold README ships as SPECIMEN.md (matz's extraction surface
/// is preserved there) and a generated quick-start takes README.md (see
/// `scaffold_readme_to_specimen`). Spinel's e2e boots the AOT binary.
fn ships_e2e(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Go
            | BuildTarget::Typescript
            | BuildTarget::Rust
            | BuildTarget::Python
            | BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Kotlin
            | BuildTarget::Swift
            | BuildTarget::CSharp
            | BuildTarget::Ruby
            | BuildTarget::Jruby
            | BuildTarget::Spinel
    )
}

/// The Playwright specs, verbatim from the repo's `e2e/` harness — the
/// single source for both the legacy `scripts/e2e` path and the
/// in-archive suite. Compiled in via `include_str!` so `--site`
/// needs no disk layout beyond the crate itself.
const E2E_SPECS: &[(&str, &str)] = &[
    ("e2e/index.spec.js", include_str!("../e2e/index.spec.js")),
    ("e2e/validation.spec.js", include_str!("../e2e/validation.spec.js")),
    ("e2e/tailwind.spec.js", include_str!("../e2e/tailwind.spec.js")),
    ("e2e/turbo_comment.spec.js", include_str!("../e2e/turbo_comment.spec.js")),
    ("e2e/action_cable.spec.js", include_str!("../e2e/action_cable.spec.js")),
    // Ships to — and runs on — every archive: all targets now back flash
    // with a per-session cookie, so none E2E_SKIP it (see the 428 comment
    // in `target_readme`). Must be shipped here regardless: `npx playwright
    // test` only discovers specs present in the archive's e2e/ dir.
    ("e2e/flash.spec.js", include_str!("../e2e/flash.spec.js")),
];

/// Inject the self-contained Playwright e2e suite into an archive:
/// the specs (shared, target-agnostic) plus a generated
/// `playwright.config.js` whose `webServer` block seeds the target's
/// DB from the archive's `db/seed.sql` (`e2e/seed.js`, sqlite3 CLI,
/// idempotent) and then boots the target's own binary (built per the
/// README). Seeding rides the webServer command — NOT globalSetup —
/// because Playwright starts the webServer before globalSetup runs,
/// and servers that self-seed demo data on an empty DB (typescript)
/// or need the DB's parent dir created (elixir) must see the seeded
/// state at boot. The README's `## End-to-end` section documents the
/// run: `cd e2e && npm install && npx playwright install chromium &&
/// npx playwright test`.
fn ensure_e2e(
    mut files: Vec<(String, String)>,
    target: BuildTarget,
) -> Vec<(String, String)> {
    if !ships_e2e(target) {
        return files;
    }
    // Per-target boot command (relative to the archive root, after the
    // README's Build steps) and DB path (the server's unset-env default
    // — global-setup seeds the same file the server opens). The boot
    // command must NOT rebuild: scripts/smoke runs the README's Build
    // section first, and Playwright's webServer timeout (120s) is for
    // boot, not compilation.
    let (boot, db_rel) = match target {
        BuildTarget::Go => ("./server", "storage/development.sqlite3"),
        BuildTarget::Typescript => ("npm start", "storage/development.sqlite3"),
        BuildTarget::Rust => ("./target/release/app", "storage/development.sqlite3"),
        BuildTarget::Python => ("uv run python -m app", "storage/development.sqlite3"),
        BuildTarget::Crystal => ("./server", "storage/development.sqlite3"),
        // mix.exs declares no `mod:` (the app doesn't auto-start), so
        // the entry point must be explicit — bare `mix run --no-halt`
        // starts the BEAM and nothing else.
        BuildTarget::Elixir => (
            "mix run --no-halt -e \"Main.run\"",
            "storage/development.sqlite3",
        ),
        BuildTarget::Kotlin => (
            "./build/install/roundhouse-app/bin/roundhouse-app",
            "storage/development.sqlite3",
        ),
        BuildTarget::Swift => ("./.build/debug/App", "storage/development.sqlite3"),
        // The README's `## Build` runs `dotnet build` (Debug); boot the
        // produced DLL via the runtime host. Defaults to storage/
        // development.sqlite3 when BLOG_DB is unset (seed.js seeds it) and
        // reads PORT. /cable rides the same Kestrel listener (Action Cable).
        BuildTarget::CSharp => (
            "dotnet bin/Debug/net10.0/roundhouse-app.dll",
            "storage/development.sqlite3",
        ),
        // The server now defaults to storage/development.sqlite3 (Rails-
        // traditional) when BLOG_DB is unset, so the boot command is bare —
        // seed.js seeds that same path before this runs.
        BuildTarget::Ruby => (
            "bundle exec puma -C config/puma.rb",
            "storage/development.sqlite3",
        ),
        BuildTarget::Jruby => (
            "WEB_CONCURRENCY=0 jruby -S bundle exec puma -C config/puma.rb",
            "storage/development.sqlite3",
        ),
        // The AOT binary (built by the README's `spin build`) defaults to
        // storage/development.sqlite3 when BLOG_DB is unset, and reads PORT
        // (default 3000, which the playwright config expects). Serves
        // /assets/* from the prebuilt static/assets/.
        BuildTarget::Spinel => ("./build/bin/blog", "storage/development.sqlite3"),
        _ => unreachable!("ships_e2e gates the match"),
    };

    for (path, content) in E2E_SPECS {
        files.push((path.to_string(), content.to_string()));
    }
    // CI prewarms Chromium from this lockfile. Pin archives to the same
    // Playwright version so their standalone install reuses that browser.
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../e2e/package-lock.json")).expect("parse e2e lockfile");
    let playwright_version = lock["packages"]["node_modules/@playwright/test"]["version"]
        .as_str()
        .expect("locked Playwright version");
    // "type": "module" matters: global-setup.js is written as ESM, and
    // without it Node loads the file as CommonJS ("exports is not
    // defined in ES module scope").
    files.push((
        "e2e/package.json".to_string(),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "app-e2e",
            "private": true,
            "type": "module",
            "description": "Playwright end-to-end smoke tests for this archive — see ../README.md",
            "scripts": { "test": "playwright test" },
            "devDependencies": { "@playwright/test": playwright_version }
        }))
        .expect("serialize e2e manifest"),
    ));
    files.push((
        "e2e/playwright.config.js".to_string(),
        format!(
            "import {{ defineConfig, devices }} from '@playwright/test'\n\
             \n\
             // Generated by Roundhouse. Self-contained: `webServer` boots the app\n\
             // (built per ../README.md) and global-setup.js seeds ../db/seed.sql.\n\
             // E2E_SKIP is a space/comma list of spec basenames to skip.\n\
             const SKIP = (process.env.E2E_SKIP || '').split(/[\\s,]+/).filter(Boolean)\n\
             \n\
             export default defineConfig({{\n\
             \x20\x20testDir: '.',\n\
             \x20\x20testIgnore: SKIP.map(name => `**/${{name}}*.spec.js`),\n\
             \x20\x20fullyParallel: true,\n\
             \x20\x20forbidOnly: !!process.env.CI,\n\
             \x20\x20retries: process.env.CI ? 2 : 0,\n\
             \x20\x20reporter: process.env.CI ? [['github'], ['list']] : 'list',\n\
             \x20\x20use: {{\n\
             \x20\x20\x20\x20baseURL: 'http://localhost:3000',\n\
             \x20\x20\x20\x20trace: 'on-first-retry',\n\
             \x20\x20}},\n\
             \x20\x20// seed.js runs INSIDE the webServer command (not globalSetup —\n\
             \x20\x20// Playwright boots the webServer first) so the server opens an\n\
             \x20\x20// already-seeded DB.\n\
             \x20\x20webServer: {{\n\
             \x20\x20\x20\x20command: 'node e2e/seed.js && {boot}',\n\
             \x20\x20\x20\x20cwd: '..',\n\
             \x20\x20\x20\x20url: 'http://localhost:3000/articles',\n\
             \x20\x20\x20\x20reuseExistingServer: !process.env.CI,\n\
             \x20\x20\x20\x20timeout: 120_000,\n\
             \x20\x20}},\n\
             \x20\x20projects: [{{ name: 'chromium', use: {{ ...devices['Desktop Chrome'] }} }}],\n\
             }})\n"
        ),
    ));
    files.push((
        "e2e/seed.js".to_string(),
        format!(
            "// Generated by Roundhouse. Seeds the server's DB from ../db/seed.sql\n\
             // (sqlite3 CLI). Runs as the first half of playwright.config.js's\n\
             // webServer command, so the server boots against an already-seeded\n\
             // DB (some targets self-seed demo data on an empty one, with\n\
             // different row timestamps than the canonical seed). Idempotent:\n\
             // skips when articles already exist, so re-runs don't double-seed.\n\
             // For a truly fresh run, delete {db_rel} (or re-extract the archive).\n\
             import {{ execFileSync }} from 'node:child_process'\n\
             import {{ mkdirSync, readFileSync }} from 'node:fs'\n\
             import path from 'node:path'\n\
             import {{ fileURLToPath }} from 'node:url'\n\
             \n\
             const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..')\n\
             const db = path.join(root, '{db_rel}')\n\
             const seed = path.join(root, 'db', 'seed.sql')\n\
             \n\
             mkdirSync(path.dirname(db), {{ recursive: true }})\n\
             let count = 0\n\
             try {{\n\
             \x20\x20count = Number(execFileSync('sqlite3', [db, 'SELECT COUNT(*) FROM articles'],\n\
             \x20\x20\x20\x20{{ encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }}).trim())\n\
             }} catch {{ /* missing file or table — seed below */ }}\n\
             if (count === 0) {{\n\
             \x20\x20execFileSync('sqlite3', [db], {{ input: readFileSync(seed, 'utf8') }})\n\
             \x20\x20console.log(`seed.js: seeded ${{db}} from db/seed.sql`)\n\
             }} else {{\n\
             \x20\x20console.log(`seed.js: db already seeded (${{count}} articles)`)\n\
             }}\n"
        ),
    ));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Inject the quick-start README (`target_readme`) when the file set
/// doesn't already carry one. No-ops for Blog when the fixture has its
/// own README (the archive is the verbatim source); the scaffold targets
/// spinel/ruby/jruby rename the scaffold README to `SPECIMEN.md` first
/// (in `spinel_files`), so they get the quick-start. Lives here rather
/// than in the CLI so the `--site` archives and `--target` output carry
/// the same README.
fn ensure_readme(
    mut files: Vec<(String, String)>,
    target: BuildTarget,
) -> Vec<(String, String)> {
    if !files.iter().any(|(p, _)| p == "README.md") {
        files.push(("README.md".to_string(), target_readme(target)));
        files.sort_by(|a, b| a.0.cmp(&b.0));
    }
    files
}

/// Inject prebuilt static assets (the compiled `tailwind.css`, and later
/// `turbo.min.js` etc.) into an emit target's `static/assets/` so the
/// published archive is self-contained-styled — no build step required by a
/// downloader. The assets are read from the directory named by
/// `ROUNDHOUSE_ASSETS_DIR`; the build-site CI job compiles them once (the
/// Tailwind class set is identical across targets, so one build serves all)
/// and points the env at the output. When the env is unset or the directory
/// is missing, this is a no-op — `roundhouse --site` keeps working with no
/// Node/Tailwind toolchain, and the e2e harness builds the CSS as a fallback.
///
/// The emit targets get assets injected. The CRuby `ruby` target is the
/// sole exclusion: it builds + serves its own via the Makefile's `make
/// assets` (injecting would just be overwritten). The two other scaffold
/// targets bake them here because neither runs `make assets` at smoke
/// time: JRUBY's turbo.min.js step shells `bundle exec ruby`, which
/// collides the MRI-vs-JRuby bundler (same reason compare-jruby skips
/// assets); SPINEL's smoke Build is the bare AOT compile (no MRI toolchain
/// in that job), and its binary sendfiles `static/assets/` the same way
/// jruby's `Rack::Static` does. Without baked assets either serves no
/// tailwind.css / turbo.min.js and Turbo never boots.
fn ensure_static_assets(
    mut files: Vec<(String, String)>,
    target: BuildTarget,
) -> Vec<(String, String)> {
    let bakes_assets = matches!(
        target,
        BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Go
            | BuildTarget::Jruby
            | BuildTarget::Kotlin
            | BuildTarget::Python
            | BuildTarget::Rust
            | BuildTarget::Spinel
            | BuildTarget::Swift
            | BuildTarget::CSharp
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker
    );
    if !bakes_assets {
        return files;
    }
    let Ok(dir) = std::env::var("ROUNDHOUSE_ASSETS_DIR") else {
        return files;
    };
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return files;
    }
    let mut injected: Vec<(String, String)> = Vec::new();
    collect_asset_files(&dir, &dir, &mut injected);
    for (rel, content) in injected {
        let path = format!("static/assets/{rel}");
        if !files.iter().any(|(p, _)| p == &path) {
            files.push((path, content));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Recursively gather UTF-8 files under `dir` as `(relpath_from_root, content)`.
/// Binary/unreadable files are skipped (the archive is text-only, same as the
/// emit walk). `root` is the base the relative path is computed against.
fn collect_asset_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_asset_files(root, &path, out);
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue; // skip binary / non-UTF-8
        };
        if let Ok(rel) = path.strip_prefix(root) {
            out.push((rel.to_string_lossy().replace('\\', "/"), content));
        }
    }
}

/// Write `files` to `dest` — each entry's path is taken relative to
/// `dest`, parent dirs created as needed. Used by the `--target LANG`
/// mode of the `roundhouse` binary. Identical files are left untouched
/// so re-emitting does not invalidate mtime-based native builds.
pub fn write_to_dir(files: &[(String, String)], dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    for (path, content) in files {
        write_if_changed(&dest.join(path), content.as_bytes())?;
    }
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    // Comparison is an optimization, not a new read-permission requirement:
    // if reading fails, retain the existing write attempt and its I/O errors.
    fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Copy the app's binary assets into an emitted tree.
///
/// Separate from [`write_to_dir`] because these never became
/// `EmittedFile`s: that type's `content` is a `String`, so an image or a
/// binary test fixture could not be represented at all and was dropped
/// silently. They are copied VERBATIM — there is nothing in a JPEG to
/// transpile — which is also why they need no target dispatch.
///
/// A path an emitter already wrote WINS. The emit is the authority on
/// any file it knows how to produce; this only fills the holes the
/// text-only pipeline leaves.
///
/// Returns the number of non-conflicting assets materialized, including
/// identical files left untouched, so the caller can report a truthful total.
pub fn write_binary_assets(
    assets: &[(String, Vec<u8>)],
    emitted: &[(String, String)],
    dest: &Path,
) -> Result<usize, String> {
    let mut materialized = 0usize;
    for (rel, bytes) in assets {
        if emitted.iter().any(|(p, _)| p == rel) {
            continue;
        }
        write_if_changed(&dest.join(rel), bytes)?;
        materialized += 1;
    }
    Ok(materialized)
}

/// Sort the emit output (`Vec<EmittedFile>`) into the `(path, content)`
/// shape this module uses. Stable by path so the archive matrix is
/// deterministic.
pub fn sort_files(files: Vec<EmittedFile>) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// "blog" archive: the original Rails source fixture, walked
/// verbatim. The archive structure mirrors the fixture directory.
fn blog_files(fixture: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    walk_ruby(fixture, fixture, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// "ruby" archive: emitted CRuby-runnable tree. Starts from the
/// spinel-target file set and applies three CRuby-specific overlays
/// — same layering as the outer Makefile's `ruby-transpile` rule:
///
///   1. Db shim swap: drop the FFI `runtime/db.rb`, rename
///      `runtime/db_cruby.rb` into its place.
///   2. ruby_overlay: CGI-shaped main.rb, Rakefile, config.ru,
///      config/puma.rb, cable.rb at root.
///   3. Source-app static assets: `app/javascript/` and `public/`
///      from the fixture verbatim. Binary files are silently
///      skipped (text-only archive).
///
/// The seeded `storage/development.sqlite3` that `bin/rh` stages in is
/// NOT included — `Schema.load!` is idempotent so a fresh DB still boots
/// (the archive ships `storage/.keep` so the directory exists).
/// The stub slot the ruby-family trees get bolted onto Ruby's own
/// `Resolv`.
///
/// `lower::mocha` rewrites `Resolv.stubs(:getaddresses)` into
/// `Resolv.stub_getaddresses(...)` on EVERY target, because a lowering
/// may not branch on the target. The strict targets answer that from
/// the port (`runtime/ruby/resolv.rb`); CRuby and JRuby reach for the
/// stdlib resolver instead, so the slot has to be added on top of it.
///
/// REOPENED, not replaced: everything else about `Resolv` here is the
/// real one, and `getaddresses_without_stub` keeps the genuine lookup
/// reachable for any host no test stubbed. That is also why this tree
/// does NOT fall back to `GemFacade.fail!` the way the port does — here
/// there IS a resolver to fall through to.
/// The ruby-family `MochaBridge` — see `runtime/spinel/mocha_bridge.rb`
/// for the strict half and `lower::mocha` for what arrives here.
///
/// A REPLAY, not a reimplementation: the chain comes as data and goes
/// back out as the exact mocha calls the test wrote, block included, so
/// these lanes keep exercising the gem for every shape the typed slots
/// do not serve. A constant travels by name (`raises("Net::OpenTimeout")`)
/// and is resolved here; a bare matcher (`has_entry`) is one of
/// `Mocha::ParameterMatchers`, which the module extends for the purpose.
const MOCHA_BRIDGE_REPLAY: &str = r##"# `lower::mocha` bridge, ruby-family half — see `project::MOCHA_BRIDGE_REPLAY`.
begin
  require "mocha/api"
rescue LoadError
  nil
end

module MochaBridge
  CONST_NAME = /\A[A-Z]\w*(::[A-Z]\w*)*\z/

  def self.chain(konst, kind, meth, ops, &blk)
    # A head spelled `Const.reader` (`ActionCable.server`) is the object
    # that reader answers, not a constant of that name.
    target =
      if konst.include?(".")
        name, reader = konst.split(".", 2)
        Object.const_get(name).public_send(reader)
      else
        Object.const_get(konst)
      end
    expectation =
      case kind.to_s
      when "stubs" then target.stubs(meth)
      when "expects" then target.expects(meth)
      when "any_instance_stubs" then target.any_instance.stubs(meth)
      when "any_instance_expects" then target.any_instance.expects(meth)
      else raise ArgumentError, "mocha bridge: unknown kind #{kind}"
      end
    ops.each do |op, args|
      args = args.map { |a| a.is_a?(String) && a.match?(CONST_NAME) && Object.const_defined?(a) ? Object.const_get(a) : a } if op == :raises
      expectation =
        if op == :with && blk
          expectation.with(&blk)
        else
          expectation.public_send(op, *args)
        end
    end
    nil
  end

  # Extended lazily: this file is required from the helper's preamble,
  # ahead of the gem in some trees, and the matchers are only needed
  # once a test reaches one.
  def self.matcher(name, arg = nil)
    # `Mocha::ParameterMatchers::Methods` is where mocha 3 keeps the
    # matcher methods; `Mocha::API` includes it into a test class.
    extend Mocha::ParameterMatchers::Methods unless singleton_class.include?(Mocha::ParameterMatchers::Methods)
    arg.nil? ? public_send(name) : public_send(name, arg)
  end
end
"##;

/// The `WebPush.payload_send` stub slot the ruby-family trees get bolted
/// onto the real `web-push` gem — the same shape as `RESOLV_STUB_REOPEN`,
/// for the same reason: `lower::mocha` rewrites `WebPush.stubs(:payload_send)`
/// and `WebPush.expects(:payload_send).times(n)` on every target, and
/// on this one there IS a gem to fall through to. The strict trees carry
/// the slot inside the façade (`runtime/ruby/gem_facades.rb`).
const WEB_PUSH_STUB_REOPEN: &str = r##"
# Stub slot for `lower::mocha` — see `project::WEB_PUSH_STUB_REOPEN`.
module WebPush
  STUB_ON = [ false ]
  STUB_VALUE = [ "" ]
  CALLS = [ 0 ]
  EXPECTED = [ -1 ]
  EXPECT_KEY = [ "" ]
  EXPECT_VALUE = [ "" ]
  EXPECT_RAISE_NAME = [ "" ]

  class << self
    # `method_defined?`, not `respond_to?`: inside `class << self` the
    # receiver is the singleton class, which never responds to the
    # gem's `payload_send` — so the alias never ran and every real
    # delivery raised "not installed" with the gem loaded.
    alias_method :payload_send_without_stub, :payload_send if method_defined?(:payload_send)

    def stub_payload_send
      STUB_ON[0] = true
      STUB_VALUE[0] = ""
      nil
    end

    def stub_payload_send_any(value)
      STUB_ON[0] = true
      STUB_VALUE[0] = value.to_s
      nil
    end

    # A second expectation closes the first — see the façade's copy.
    def expect_payload_send(count)
      verify_payload_send_expectations
      CALLS[0] = 0
      STUB_ON[0] = true
      EXPECTED[0] = count
      nil
    end

    def expect_payload_send_with_entry(count, key, value)
      STUB_ON[0] = true
      EXPECTED[0] = count
      EXPECT_KEY[0] = key.to_s
      EXPECT_VALUE[0] = value.to_s
      nil
    end

    # By class NAME, like the façade's copy; the gem's classes are the
    # ones underneath on this lane, resolved when the raise is made.
    def expect_payload_send_raising(count, error_name)
      expect_payload_send(count)
      EXPECT_RAISE_NAME[0] = error_name.to_s
      nil
    end

    def clear_payload_send_stubs
      STUB_ON[0] = false
      STUB_VALUE[0] = ""
      CALLS[0] = 0
      EXPECTED[0] = -1
      EXPECT_KEY[0] = ""
      EXPECT_VALUE[0] = ""
      EXPECT_RAISE_NAME[0] = ""
      nil
    end

    def verify_payload_send_expectations
      expected = EXPECTED[0]
      return nil if expected < 0
      got = CALLS[0]
      EXPECTED[0] = -1
      raise "WebPush.payload_send was expected #{expected} time(s), got #{got}" if got != expected
      nil
    end

    def payload_send(**options)
      if STUB_ON[0]
        key = EXPECT_KEY[0]
        unless key.empty?
          got = options[key.to_sym].to_s
          raise "WebPush.payload_send was called with #{key}: #{got.inspect}, expected #{EXPECT_VALUE[0].inspect}" if got != EXPECT_VALUE[0]
        end
        CALLS[0] += 1
        unless EXPECT_RAISE_NAME[0].empty?
          klass = Object.const_get(EXPECT_RAISE_NAME[0])
          # The gem's response errors read `response.body` for their
          # message; the slot has no response, so a bodiless one.
          raise(klass <= WebPush::ResponseError ? klass.new(Struct.new(:body).new(""), "") : klass)
        end
        return STUB_VALUE[0]
      end
      raise NotImplementedError, "web-push is not installed" unless respond_to?(:payload_send_without_stub)
      payload_send_without_stub(**options)
    end
  end
end
"##;

/// The ruby family's `runtime/ipaddr.rb`: Ruby's own, plus the ONE
/// method the port has and the stdlib does not. `runtime/ruby/ipaddr.rb`
/// keeps the address as an `Array[Integer]` of octets because the strict
/// targets have no 128-bit `to_i`, and the runtime code written over it
/// reads that array — `Surfguard.embedded_ipv4` takes the last four for
/// a NAT64 / SIIT target. On this tree the class under the same require
/// path is the stdlib's, so without this reopen that call was a
/// `NoMethodError` for every `64:ff9b::/96` and `::ffff:0:0:0/96`
/// address: the guard failed CLOSED, but an unfurl of one 500'd where
/// Rails refuses it (three of campfire's 24 guard tests). `hton` is the
/// masked address in network order, which is what the port's `@octets`
/// holds too.
const IPADDR_STDLIB: &str = r##"# Ruby's own ipaddr — see `project::IPADDR_STDLIB`.
# The port at runtime/ruby/ipaddr.rb exists for the targets
# that have no stdlib to reach for; this tree has one, and
# defining a second IPAddr beside it is a superclass mismatch
# at require time.
require "ipaddr"

class IPAddr
  # The port's representation, answered by the stdlib's: 4 octets for
  # IPv4, 16 for IPv6, masked to the prefix.
  def octets
    hton.bytes
  end unless method_defined?(:octets)
end
"##;

const RESOLV_STUB_REOPEN: &str = r##"require "resolv"

# Stub slot for `lower::mocha` — see `project::RESOLV_STUB_REOPEN`.
class Resolv
  STUB_HOSTS = [ "" ]
  STUB_ADDRS = [ [ "" ] ]
  STUB_ANY = [ [ "" ] ]
  STUB_ANY_ON = [ false ]
  STUB_RAISE = [ StandardError ]
  STUB_RAISE_ON = [ false ]
  STUB_WHERE = []
  STUB_WHERE_ADDRS = [ [ "" ] ]
  STUB_SEQ_HOSTS = [ "" ]
  STUB_SEQ_ADDRS = [ [ [ "" ] ] ]

  class << self
    alias_method :getaddresses_without_stub, :getaddresses

    def stub_getaddresses(host, addrs)
      i = STUB_HOSTS.index(host)
      if i.nil?
        STUB_HOSTS << host
        STUB_ADDRS << addrs
      else
        STUB_ADDRS[i] = addrs
      end
      nil
    end

    # `.returns(a, b)`: consecutive calls answer consecutive values, the
    # last repeating — mocha's sequence.
    def stub_getaddresses_seq(host, answers)
      i = STUB_SEQ_HOSTS.index(host)
      if i.nil?
        STUB_SEQ_HOSTS << host
        STUB_SEQ_ADDRS << answers
      else
        STUB_SEQ_ADDRS[i] = answers
      end
      nil
    end

    def stub_getaddresses_any(addrs)
      STUB_ANY[0] = addrs
      STUB_ANY_ON[0] = true
      STUB_RAISE_ON[0] = false
      nil
    end

    def stub_getaddresses_raises(error)
      STUB_RAISE[0] = error
      STUB_RAISE_ON[0] = true
      STUB_ANY_ON[0] = false
      nil
    end

    def stub_getaddresses_where(addrs, pred)
      STUB_WHERE.replace([ pred ])
      STUB_WHERE_ADDRS[0] = addrs
      nil
    end

    def clear_getaddresses_stubs
      STUB_HOSTS.replace([ "" ])
      STUB_ADDRS.replace([ [ "" ] ])
      STUB_SEQ_HOSTS.replace([ "" ])
      STUB_SEQ_ADDRS.replace([ [ [ "" ] ] ])
      STUB_ANY_ON[0] = false
      STUB_RAISE_ON[0] = false
      STUB_WHERE.clear
      nil
    end

    def getaddresses(host)
      return STUB_WHERE_ADDRS[0] if !STUB_WHERE.empty? && STUB_WHERE[0].call(host)
      raise STUB_RAISE[0] if STUB_RAISE_ON[0]
      i = STUB_SEQ_HOSTS.index(host)
      unless i.nil?
        q = STUB_SEQ_ADDRS[i]
        return q.length > 1 ? q.shift : q[0]
      end
      i = STUB_HOSTS.index(host)
      return STUB_ADDRS[i] unless i.nil?
      return STUB_ANY[0] if STUB_ANY_ON[0]
      getaddresses_without_stub(host)
    end
  end
end
"##;

/// The ruby family's socket seam: `TcpSocketStub` (the strict file, kept
/// as written — it is our own module, no stdlib class to collide with)
/// with `TCPSocket.open` reopened to ask it first. That is the method
/// CRuby's `Net::HTTP#connect` opens its socket with, and the one mocha
/// replaced when the test was written; the strict lane asks from the
/// reopened client instead (`runtime/spinel/net_http.rb`).
const TCP_SOCKET_OPEN_REOPEN: &str = r##"
# The ruby family's seam — see `project::TCP_SOCKET_OPEN_REOPEN`.
require "socket"

class TCPSocket
  class << self
    alias_method :open_without_stub, :open

    def open(*args, **kw, &blk)
      TcpSocketStub.check(args[0].to_s, args[1].to_i)
      open_without_stub(*args, **kw, &blk)
    end
  end
end
"##;

/// The ruby family's `SecureRandom` stub slot — `lower::mocha` rewrites
/// `SecureRandom.stubs(:alphanumeric).returns(v)` (and `Random.stubs
/// (:uuid)`, whose app-side call is grounded to `SecureRandom.uuid`) to
/// `SecureRandom.stub_<m>(v)` on every target. Same shape as
/// `RESOLV_STUB_REOPEN`: here the stdlib's own rendering is one
/// `alias_method` away, so the unstubbed arm calls it; the spinel file
/// (`runtime/spinel/secure_random_stub.rb`) has no alias to reach the
/// package's and re-renders instead.
const SECURE_RANDOM_STUB_REOPEN: &str = r##"require "securerandom"

# Stub slot for `lower::mocha` — see `project::SECURE_RANDOM_STUB_REOPEN`.
module SecureRandom
  STUB_ALPHANUMERIC_ON = [ false ]
  STUB_ALPHANUMERIC = [ "" ]
  STUB_UUID_ON = [ false ]
  STUB_UUID = [ "" ]

  class << self
    alias_method :alphanumeric_without_stub, :alphanumeric
    alias_method :uuid_without_stub, :uuid

    def stub_alphanumeric(value)
      STUB_ALPHANUMERIC_ON[0] = true
      STUB_ALPHANUMERIC[0] = value
      nil
    end

    def stub_uuid(value)
      STUB_UUID_ON[0] = true
      STUB_UUID[0] = value
      nil
    end

    def clear_secure_random_stubs
      STUB_ALPHANUMERIC_ON[0] = false
      STUB_ALPHANUMERIC[0] = ""
      STUB_UUID_ON[0] = false
      STUB_UUID[0] = ""
      nil
    end

    def alphanumeric(n = 16, chars: nil)
      return STUB_ALPHANUMERIC[0] if STUB_ALPHANUMERIC_ON[0]
      chars.nil? ? alphanumeric_without_stub(n) : alphanumeric_without_stub(n, chars: chars)
    end

    def uuid
      return STUB_UUID[0] if STUB_UUID_ON[0]
      uuid_without_stub
    end
  end
end
"##;

/// `HttpStub` for the ruby family — the same three calls
/// `lower::webmock` rewrites every test to, delegated onto the real
/// WebMock gem. The strict targets answer them from a table
/// (`runtime/spinel/http_stub.rb`) and a reopened `Net::HTTP`; over
/// here the gem already intercepts CRuby's own client, so the delegate
/// is all that is needed, and today's CRuby lane keeps running the gem
/// it always did rather than betting its number on new code.
///
/// `require "webmock"` INSIDE `stub`, not at the top of the file. Every
/// emitted helper requires this file unconditionally (the clear call
/// has to exist in every test, stubbing or not — see
/// `lower::mocha::stub_requires`), and an app whose Gemfile never named
/// webmock would otherwise fail to load its entire suite on this line.
/// A test that stubs is the demand; `apply_test_gem_wiring` has already
/// declared the gem for it.
const HTTP_STUB_WEBMOCK_DELEGATE: &str = r##"# `HttpStub` over the real WebMock gem — see
# `project::HTTP_STUB_WEBMOCK_DELEGATE`. The table at
# runtime/spinel/http_stub.rb exists for the targets that have no gem
# to intercept their HTTP client.
module HttpStub
  def self.stub(verb, url, status, body, headers)
    require "webmock"
    WebMock.stub_request(verb.downcase.to_sym, url)
      .to_return(status: status, body: body, headers: headers)
    nil
  end

  # `.with(body: hash_including(expected))`, handed back to the gem's
  # own matcher.
  def self.stub_matching(verb, url, status, body, headers, expected_json)
    require "webmock"
    require "json"
    extend WebMock::API unless singleton_class.include?(WebMock::API)
    WebMock.stub_request(verb.downcase.to_sym, url)
      .with(body: hash_including(JSON.parse(expected_json)))
      .to_return(status: status, body: body, headers: headers)
    nil
  end

  def self.allow_net_connect(hosts)
    require "webmock"
    WebMock.disable_net_connect!(allow: hosts)
    nil
  end

  def self.clear
    WebMock.reset! if defined?(WebMock)
    nil
  end

  def self.disable
    require "webmock"
    WebMock.disable!
    nil
  end

  def self.enable
    require "webmock"
    WebMock.enable!
    nil
  end
end
"##;

/// The ruby family's `runtime/net_http.rb`: Ruby's own client. The
/// reopen at runtime/spinel/net_http.rb exists for spinel, whose
/// bundled client lacks the block form of `#request` and where the stub
/// table has to sit above the transport; over here WebMock sits there.
const NET_HTTP_STDLIB: &str = "# Ruby's own net/http — see `project::NET_HTTP_STDLIB`.\n\
                               require \"net/http\"\n";

/// Which VM a ruby-family tree is for. The two trees are the SAME
/// tree — same app/, same config/, same framework runtime, same Puma +
/// Rack overlay — and differ only in what the VM can link: the SQLite
/// backend (a C extension, or JDBC), and the markdown renderer (cmark
/// bindings, or a commonmark-java shim). Everything else runs unchanged
/// on the JVM, which is why there is one builder below and not two.
///
/// THERE USED TO BE TWO, and they drifted: the JRuby copy of the
/// stdlib-swap list was written once and missed every swap added
/// afterwards (`concurrent` — a superclass mismatch at boot under
/// campfire), skipped the image-processor wiring, and never ran
/// `apply_module_mixins`, so the JVM tree booted without the app's
/// initializer mixins. A VM variant is a flavor of one function, not a
/// second function.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RubyFlavor {
    CRuby,
    JRuby,
}

fn ruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    ruby_family_runtime_files(app, fixture, RubyFlavor::CRuby)
}

/// "jruby" archive: byte-identical to the "ruby" tree except the SQLite
/// backend and the markdown shim. The Db swap installs the JDBC-backed
/// `runtime/db_jruby.rb` as `runtime/db.rb` instead of the CRuby
/// gem-backed `db_cruby.rb`: the `sqlite3` gem is a C extension with
/// no JRuby build, so JRuby reaches SQLite over JDBC. JRuby is a
/// deployment (VM) variant, not a source variant — see `RubyFlavor`.
fn jruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    ruby_family_runtime_files(app, fixture, RubyFlavor::JRuby)
}

fn ruby_family_runtime_files(
    app: &App,
    fixture: &Path,
    flavor: RubyFlavor,
) -> Result<Vec<(String, String)>, String> {
    let (mut files, test_stems) = spinel_files(app, fixture)?;

    files.retain(|(p, _)| p != "runtime/db.rb");
    // The spinel SQL-functions file is FFI; CRuby writes its own below
    // (the sqlite3 gem's `create_function`), JRuby has none yet.
    files.retain(|(p, _)| p != "runtime/sql_functions.rb");
    // `IPAddr`: the CRuby/JRuby trees have Ruby's own. Same shape as
    // the db.rb swap above — one require path, target-appropriate
    // implementation — but written as a small file rather than a
    // rename, because the require the emitted models carry is
    // `require_relative ".../runtime/ipaddr"` and that path has to
    // keep resolving. See `IPADDR_STDLIB` for the one method the
    // stdlib's is NOT a superset in.
    //
    // THIS IS NOT TIDINESS. Something on the CRuby side already loads
    // the stdlib's ipaddr (net/http reaches it through resolv), and two
    // definitions of `IPAddr::InvalidAddressError` with different
    // superclasses is a `TypeError: superclass mismatch` at REQUIRE
    // time — campfire's suite went 219/240 to 0/240, every file dying
    // on the same line.
    for (path, content) in files.iter_mut() {
        if path == "runtime/ipaddr.rb" {
            *content = IPADDR_STDLIB.to_string();
        }
        // `Zlib`: same swap, and here it is a correctness one as much
        // as a speed one. The port computes the checksum in Ruby a bit
        // at a time; CRuby's is zlib's own C. Both answer the same
        // number by construction (the port IS CRC-32/ISO-HDLC), so the
        // tree that has the real one should use it.
        // `TypeID`: the gem itself (`project::GEM_REQUIRES` and the
        // Gemfile carry it), so the file under the port's require path
        // just loads it.
        if path == "runtime/typeid.rb" {
            *content = "# The typeid gem — see `project::ruby_family_runtime_files`.\n\
                        # The port at runtime/ruby/typeid.rb exists for the targets\n\
                        # that have no gem to load.\n\
                        require \"typeid\"\n"
                .to_string();
        }
        // `SQLite3::Database`: the sqlite3 gem the CRuby tree's Db shim
        // already loads. JRuby has no sqlite3 gem (its Db shim is JDBC),
        // so there the constant is refused at emit
        // (`unavailable_class_module_construct`) and the file is empty.
        if path == "runtime/sqlite3_database.rb" {
            *content = if flavor == RubyFlavor::JRuby {
                "# SQLite3::Database is not available on JRuby: the sqlite3 gem is\n\
                 # CRuby's, and this tree's Db shim is JDBC. A program naming it is\n\
                 # refused at emit — see `project::unavailable_class_module_construct`.\n"
                    .to_string()
            } else {
                "# The sqlite3 gem's own SQLite3::Database — see\n\
                 # `project::ruby_runtime_files`. The port at\n\
                 # runtime/spinel/sqlite3_database.rb is spinel's, over the FFI.\n\
                 require \"sqlite3\"\n"
                    .to_string()
            };
        }
        // `Rack::Utils`: the rack gem the overlay's Puma already loads,
        // under the port's require path.
        if path == "runtime/rack_utils.rb" {
            *content = "# The rack gem's own Rack::Utils — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/rack_utils.rb exists for the targets\n\
                        # that have no rack to load.\n\
                        require \"rack/utils\"\n"
                .to_string();
        }
        if path == "runtime/zlib.rb" {
            *content = "# Ruby's own zlib — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/zlib.rb exists for the targets\n\
                        # that have no zlib to bind to.\n\
                        require \"zlib\"\n"
                .to_string();
        }
        // `Tempfile`: the same swap again, and the one place the port
        // is not merely slower. Ruby's `create` opens with `O_EXCL` and
        // retries on a collision, so it cannot be made to clobber a
        // file an attacker pre-created; the port opens by name (see its
        // header). Every corpus caller is a test writing to its own
        // TMPDIR, but the tree that HAS the exclusive open should use
        // it.
        if path == "runtime/tempfile.rb" {
            *content = "# Ruby's own tempfile — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/tempfile.rb exists for the targets\n\
                        # that have no stdlib to bind to, and opens by name where\n\
                        # this one opens O_EXCL.\n\
                        require \"tempfile\"\n"
                .to_string();
        }
        // `Timeout`: default gem on CRuby/JRuby; the port is for Spinel.
        if path == "runtime/timeout.rb" {
            *content = "# Ruby's own timeout — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/timeout.rb exists for Spinel,\n\
                        # which has no stdlib timeout package.\n\
                        require \"timeout\"\n"
                .to_string();
        }
        // `Resolv`: the same swap as ipaddr, and for both of ipaddr's
        // reasons at once. net/http loads the stdlib's resolver over
        // here, so a second `Resolv` beside it is a superclass mismatch
        // at require time; and the app's tests stub
        // `Resolv.getaddresses`, which only reaches the guard if the
        // guard dispatches to the class mocha patched.
        if path == "runtime/resolv.rb" {
            *content = format!(
                "# Ruby's own resolv — see `project::ruby_runtime_files`.\n\
                 # The port at runtime/ruby/resolv.rb exists for the targets\n\
                 # that have no resolver to bind to.\n{RESOLV_STUB_REOPEN}"
            );
        }
        // `HttpStub` / `Net::HTTP`: the WebMock seam. The strict table
        // and the reopened client are spinel's; this tree has the gem.
        if path == "runtime/http_stub.rb" {
            *content = HTTP_STUB_WEBMOCK_DELEGATE.to_string();
        }
        // `MochaBridge`: the chains `lower::mocha`'s table cannot serve,
        // replayed through the real gem here; the strict file raises.
        if path == "runtime/mocha_bridge.rb" {
            *content = MOCHA_BRIDGE_REPLAY.to_string();
        }
        // `SecureRandom`: the stdlib's, with the stub slot aliased over
        // it. The spinel file re-renders because it has no alias.
        if path == "runtime/secure_random_stub.rb" {
            *content = SECURE_RANDOM_STUB_REOPEN.to_string();
        }
        // `TcpSocketStub`: the table as written, and `TCPSocket.open`
        // reopened to consult it — the seam the strict client asks from
        // inside `connect_with_timeout`.
        if path == "runtime/tcp_socket_stub.rb" {
            content.push_str(TCP_SOCKET_OPEN_REOPEN);
        }
        if path == "runtime/net_http.rb" {
            *content = NET_HTTP_STDLIB.to_string();
        }
        // `WebPush`: the gem itself. The spinel file ports the gem's
        // delivery over spinel's openssl package, whose byte-level key
        // API CRuby's openssl does not spell; here the façade file
        // guarded-requires the real gem and aliases its `payload_send`
        // under the stub slot, so the anchor needs only that.
        if path == "runtime/web_push.rb" {
            *content = "# The web-push gem's own — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/spinel/web_push.rb exists for spinel;\n\
                        # gem_facades.rb requires the real gem on this tree.\n\
                        require_relative \"gem_facades\"\n"
                .to_string();
        }
        if path == "runtime/web_push_crypto.rb" {
            *content = "# Spinel only — the gem's own cryptography runs on this tree.\n".to_string();
        }
        // `Concurrent`: the same swap as ipaddr, for the same two
        // reasons. concurrent-ruby is already in this bundle (sentry-ruby
        // depends on it) and loads part of itself under any app that
        // reaches Sentry, so a second `ThreadPoolExecutor` beside the
        // gem's would reopen it with a different `initialize`; and the
        // port at runtime/spinel/concurrent.rb exists for the lane that
        // has no gem to reach for. `RUNTIME_GEMS` declares the gem for
        // any app whose code names the constant.
        if path == "runtime/concurrent.rb" {
            *content = "# concurrent-ruby's own — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/spinel/concurrent.rb exists for spinel,\n\
                        # which has threads but no gem; this tree has the gem.\n\
                        require \"concurrent\"\n"
                .to_string();
        }
    }

    // The image processor over the ruby-vips GEM when the app declares
    // variants (`apply_image_processor_wiring`); the Gemfile line is
    // `apply_runtime_gem_wiring`'s, off the same marker.
    apply_image_processor_wiring(&mut files)?;

    // Same swap as db.rb below: the flat walk picked up BOTH halves of
    // the keyed-digest split, and the CRuby/JRuby trees want the OpenSSL
    // one at the shared path. The spinel half reaches sp_crypto through
    // FFI declarations these trees can't compile.
    files.retain(|(p, _)| p != "runtime/message_digest.rb");

    // The scaffold's tailwind seed only belongs in a tree that actually
    // BUILDS Tailwind, and the tell is `tailwind` among the app's
    // stylesheet stems — tailwindcss-rails writes its output to
    // `app/assets/builds/tailwind.css`, which is one of the two roots
    // `app.stylesheets` is ingested from.
    //
    // Two apps drop it, for different reasons. A no-stylesheet app (the
    // Roda + Sequel exemplar renders inline-styled HTML) has nothing to
    // build; campfire has twenty-six stylesheets and writes plain CSS,
    // and used to get a seed, an npm install and a Tailwind build for a
    // stylesheet its layout never links. Either way the emitted
    // Rakefile's existence-conditional `assets` task then skips the
    // npm/tailwind pipeline — `rake dev` boots with no Node at all.
    if !app.stylesheets.iter().any(|s| s == "tailwind") {
        files.retain(|(p, _)| p != "app/assets/tailwind.css");
    }
    for (path, _) in files.iter_mut() {
        if path == "runtime/message_digest_cruby.rb" {
            *path = "runtime/message_digest.rb".to_string();
        }
    }
    // The Db backend, at the shared `runtime/db.rb` path. Both trees
    // resolve the temporal intrinsics (`ActiveSupport.db_now` in
    // fill_timestamps, `parse_db_time` in temporal readers) via the
    // overlay's ActiveSupport module. The server boot requires it from
    // main.rb, but the emitted test bootstrap (test/test_helper.rb,
    // shared verbatim with the spinel tree, which lacks the file —
    // spinel#1661) does not. Chain it off db.rb — the one ruby-family
    // require every persistence-touching bootstrap already loads — at
    // materialization time, since the source-tree relative path differs
    // from the emitted-tree one.
    const TIME_PARSING_REQUIRE: &str = "require_relative \"active_support_time_parsing\"\n";
    match flavor {
        RubyFlavor::CRuby => {
            let sql_functions = cruby_sql_functions_file(app);
            for (path, content) in files.iter_mut() {
                if path == "runtime/db_cruby.rb" {
                    *path = "runtime/db.rb".to_string();
                    content.insert_str(0, TIME_PARSING_REQUIRE);
                    if sql_functions.is_some() {
                        content.insert_str(0, "require_relative \"sql_functions\"\n");
                    }
                }
            }
            if let Some(src) = sql_functions {
                files.push(("runtime/sql_functions.rb".to_string(), src));
            }
        }
        RubyFlavor::JRuby => {
            // Drop the CRuby gem backend and promote the JDBC one.
            // `db_jruby.rb` is excluded from `spinel_files`' base set,
            // so read it from disk and inject it here.
            files.retain(|(p, _)| p != "runtime/db_cruby.rb");
            let db_jruby = crate::runtime_files::read_to_string("runtime/spinel/db_jruby.rb")
                .map_err(|e| format!("read runtime/spinel/db_jruby.rb: {e}"))?;
            files.push((
                "runtime/db.rb".to_string(),
                format!("{TIME_PARSING_REQUIRE}{db_jruby}"),
            ));
            apply_jruby_gemfile(&mut files)?;
        }
    }

    // Tep is a spinel-only transport (FFI HTTP server). The CRuby
    // target uses Puma + Rack via the ruby_overlay; nothing in its
    // boot path requires Tep, and the unsubstituted @TEP_SPHTTP_O@
    // placeholder in net.rb would confuse anyone exploring the tree.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    // `Module#delegate` for the GEMS in the emitted Gemfile — see the
    // file's own header. Pushed at the fork, never named in the
    // `spinel_files` stems list: a reopen of `Module` that defines
    // methods from a computed name is exactly what the strict targets
    // cannot compile, and nothing in this tree's own emitted code needs
    // it (an app's `delegate` is lowered at ingest).
    files.push((
        "runtime/module_delegate.rb".to_string(),
        crate::runtime_files::read_to_string("runtime/spinel/module_delegate.rb")
            .map_err(|e| format!("read runtime/spinel/module_delegate.rb: {e}"))?,
    ));

    // Gem façades are SPINEL-ONLY: spinel AOT can't link the native
    // gems, so it ships loudly-raising stubs. On the ruby family the
    // real gems (markly / nokogiri / mail) ARE available — main.rb
    // guarded-requires them — and app code renders through them at
    // request time (Markdowner.to_html behind User#linkified_about is
    // read-path: `/u/:username` markdown-renders the profile bio live).
    // The inherited façade reopens those modules and REDEFINES their
    // methods to raise, shadowing the real gems. Neutralize it here so
    // the `require_relative "runtime/gem_facades"` anchor still
    // resolves without clobbering the real implementations.
    //
    // Not a bare no-op: the anchor has to MEAN something. main.rb
    // guarded-requires these gems too, but a test run never loads
    // main.rb — `test/test_helper.rb` builds its own require chain — so
    // a body reached only from the test harness saw an anchor that
    // resolved to an empty file and a constant that was never defined.
    // campfire's `users.yml` is the first such body: `password_digest:
    // <%= BCrypt::Password.create(…) %>` lands in
    // `UsersFixtures._fixtures_load!`, whose only caller is the harness.
    // Requires are idempotent, so doing them here makes every consumer
    // of the anchor self-sufficient without changing main.rb's behavior.
    //
    // THE ONLY LIST. boot.rb used to carry a second copy and the two had
    // drifted (it was missing rqrcode and sentry-ruby); it requires this
    // file now.
    //
    // JRuby: markly is cmark-gfm C bindings with no JRuby build, so that
    // tree provides Markly over commonmark-java (runtime/markly_jruby.rb,
    // reference-conformant: scripts/markly-conformance, vectors generated
    // from the real gem under CRuby) and drops the gem from the list.
    // The shim's jars are FETCHED, not shipped (bin/fetch-jars), so
    // requiring it in a tree that never renders markdown is a LoadError
    // on a dependency that app does not have — the require is
    // conditional on the app naming the constant, not on the require
    // graph's shape (which is what kept the blog's JRuby tree booting
    // with no jars).
    let gem_facades = match flavor {
        RubyFlavor::CRuby => format!(
            "# Gem façades are spinel-only (no native gems there). On the CRuby\n\
             # path the real gems ARE available, so this file guarded-requires\n\
             # them rather than shadowing them with raising stubs. Guarded because\n\
             # an app that uses none of them (the blog) must boot without them\n\
             # installed.\n\
             #\n\
             # THE ONLY LIST. boot.rb used to carry a second copy and the two had\n\
             # drifted (it was missing rqrcode and sentry-ruby); it requires this\n\
             # file now. (JRuby writes its own copy of this file — it swaps in the\n\
             # commonmark-java Markly shim — from the same list, minus that\n\
             # one name.)\n\
             require_relative \"module_delegate\"\n\
             require_relative \"open_telemetry_facade\"\n\
             {}{}",
            gem_require_block(&[]),
            WEB_PUSH_STUB_REOPEN
        ),
        RubyFlavor::JRuby => {
            let needs_markly = files.iter().any(|(p, c)| {
                p.starts_with("app/") && p.ends_with(".rb") && names_constant(c, "Markly")
            });
            let markly_require = if needs_markly {
                "require_relative \"markly_jruby\"\n"
            } else {
                "# (this app never names Markly, so the commonmark-java shim — and\n\
                 # the jars bin/fetch-jars pulls — stay out of its require graph)\n"
            };
            format!(
                "# On the JRuby tree Markly is provided by the commonmark-java shim\n\
                 # (markly_jruby.rb); every other gem below is the real one (nokogiri\n\
                 # ships a java platform build, the rest are pure Ruby). This file is\n\
                 # also the `require_relative \"runtime/gem_facades\"` anchor and the\n\
                 # one guarded-require list for this tree — boot.rb requires it rather\n\
                 # than carrying a second copy.\n\
                 {}\
                 require_relative \"module_delegate\"\n\
                 require_relative \"open_telemetry_facade\"\n\
                 {}{}",
                markly_require,
                gem_require_block(&["markly"]),
                WEB_PUSH_STUB_REOPEN
            )
        }
    };
    for (path, content) in files.iter_mut() {
        if path == "runtime/gem_facades.rb" {
            *content = gem_facades.clone();
        }
    }
    if flavor == RubyFlavor::JRuby {
        push_jruby_markly_shim(&mut files)?;
    }

    // Same reasoning for the extras façades (Sponge): CRuby has the real
    // net/https / resolv / ipaddr stdlib and the vendored source runs as
    // written, so put the verbatim source-shape emit back over the
    // scaffold base's raising façade.
    emit::ruby::restore_extras_facades(&mut files, app);

    crate::runtime_files::walk_into("runtime/spinel/scaffold/ruby_overlay",
        "",
        &mut files,
    )?;

    // Library classes (app/models/<stem>.rb for the ingested support
    // classes) and the regenerated app/views.rb aggregator are inherited
    // from `spinel_files` — the shared base emits both for all three
    // scaffold targets. Nothing after this point adds view files, so the
    // base's aggregator content stays correct.

    // config/application.rb (the per-app Rails::Application reopen) is
    // inherited from `spinel_files` — emit_spinel emits it (or a stub)
    // unconditionally for all three scaffold targets.

    // Controllers re-emitted WITH the layout wrap (dedupe last-wins
    // supersedes the spinel-shape versions): this tree's main.rb ships
    // `controller.body` verbatim, so the render call sites must apply
    // the layout — the seam where the @ivars a layout reads are in
    // scope. The plain spinel target keeps unwrapped controllers (its
    // dispatch wraps body-only).
    files.extend(sort_files(emit::ruby::emit_lowered_controllers_with_layout(app)));

    // The source app's `app/javascript/` + `public/` static assets are
    // already folded in by `spinel_files` (both targets need them — the
    // spinel binary now serves `/assets/*` too). Nothing CRuby-specific
    // to add here beyond the overlay above. The scaffold README → SPECIMEN
    // rename already happened in `spinel_files`.
    let mut files = dedupe_last_wins(files);
    // Lazy flavor: rewrites the ruby_overlay main.rb (which superseded the
    // base's eagerly-rewritten one at dedupe) and strips routes.rb's eager
    // controller-require header.
    apply_controller_dispatch(&mut files, app, true);
    apply_cruby_test_support(&mut files);
    apply_route_table_root(&mut files, app);
    apply_cable_strip(&mut files, app)?;
    apply_makefile_test_list_stems(&mut files, &test_stems);
    apply_runtime_gem_wiring(&mut files);
    // AGAIN, on purpose, and this time PERFORMING them. `spinel_files`
    // appended a commented-out block to the spinel tree's boot.rb (that
    // target cannot perform a mixin — see `apply_module_mixins`), and the
    // overlay's own boot.rb then replaced that file wholesale at
    // `dedupe_last_wins` above, taking the block with it. Each final tree
    // gets the block exactly once; this is the ruby family's turn, and
    // the ruby family is the one that can run the lines.
    apply_module_mixins(&mut files, app, MixinForm::ExplicitReceiver);
    // Overlay boot replaces the spinel boot wholesale (see comment above
    // on apply_module_mixins), wiping the Date-package inject from
    // `spinel_files`. Re-inject only the shared `date_*` helpers — not
    // Spinel's Date polyfill or date-JSON reopen (see
    // `inject_cruby_date_calendar_helpers`).
    if app_uses_date(app) {
        inject_cruby_date_calendar_boot_requires(&mut files)?;
    }
    Ok(files)
}

/// The JRuby tree's Gemfile: the committed scaffold Gemfile is MRI-only
/// (`gem "sqlite3"`, a C extension with no JRuby build), so its frozen
/// lock stays valid for the CRuby/Spinel toolchain jobs. The JRuby tree
/// reaches SQLite over JDBC, so rewrite that one line to the Xerial
/// driver here — the emitted tree's `bundle install` then resolves a
/// fresh JRuby lock.
fn apply_jruby_gemfile(files: &mut Vec<(String, String)>) -> Result<(), String> {
    let gemfile = files
        .iter_mut()
        .find(|(p, _)| p == "Gemfile")
        .ok_or("jruby_runtime_files: scaffold Gemfile not found")?;
    if !gemfile.1.contains("gem \"sqlite3\"") {
        return Err(
            "jruby_runtime_files: expected `gem \"sqlite3\"` in scaffold Gemfile to swap for \
             jdbc-sqlite3"
                .to_string(),
        );
    }
    gemfile.1 = gemfile
        .1
        .replace("gem \"sqlite3\"", "gem \"jdbc-sqlite3\"");

    // Pin `rdoc` below 8 for the JRuby tree. rdoc 8.0.0 (2026-06-26) added
    // a runtime dependency on `rbs (>= 4.0.0)`, whose 4.x line ships a C
    // parser extension with no JRuby build — `jruby -S bundle install`
    // dies in extconf ("The compiler failed to generate an executable
    // file"). rdoc only enters this tree transitively (stimulus-rails →
    // railties → irb → rdoc), and the CRuby/Spinel targets dodge it via
    // their frozen `Gemfile.lock` (which pins rdoc 7.2.0 — no rbs). The
    // JRuby tree resolves a fresh lock (it drops the MRI lock below), so
    // hold rdoc at the pre-8 line here to keep rbs out of the graph.
    gemfile.1.push_str("\n# rdoc 8 pulls rbs (C ext, no JRuby build); see jruby_runtime_files.\ngem \"rdoc\", \"< 8\"\n");

    // Drop the committed MRI `Gemfile.lock` from the JRuby tree: it pins
    // the C-ext `sqlite3` and omits `jdbc-sqlite3`, so shipping it would
    // make the tree's `jruby -S bundle install` a frozen-mode mismatch.
    // The JRuby bundle resolves its own platform-correct lock fresh.
    files.retain(|(p, _)| p != "Gemfile.lock");
    Ok(())
}

/// The commonmark-java Markly shim and the script that fetches its jars
/// (they can't ship through the text-only emit; the tree pulls them from
/// Maven Central on demand).
fn push_jruby_markly_shim(files: &mut Vec<(String, String)>) -> Result<(), String> {
    let markly_shim = crate::runtime_files::read_to_string("runtime/spinel/markly_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/markly_jruby.rb: {e}"))?;
    files.push(("runtime/markly_jruby.rb".to_string(), markly_shim));
    files.push((
        "bin/fetch-jars".to_string(),
        "#!/bin/sh\n\
         # Fetch the commonmark-java jars the Markly shim needs (see\n\
         # runtime/markly_jruby.rb). Run once: sh bin/fetch-jars\n\
         set -e\n\
         dir=\"$(dirname \"$0\")/../vendor/jars\"\n\
         mkdir -p \"$dir\"\n\
         for spec in \\\n\
           org/commonmark/commonmark/0.29.0/commonmark-0.29.0.jar \\\n\
           org/commonmark/commonmark-ext-gfm-strikethrough/0.29.0/commonmark-ext-gfm-strikethrough-0.29.0.jar \\\n\
           org/commonmark/commonmark-ext-autolink/0.29.0/commonmark-ext-autolink-0.29.0.jar \\\n\
           org/nibor/autolink/autolink/0.12.0/autolink-0.12.0.jar \\\n\
         ; do\n\
           f=\"$dir/$(basename \"$spec\")\"\n\
           [ -f \"$f\" ] || curl -sf -o \"$f\" \"https://repo1.maven.org/maven2/$spec\"\n\
         done\n\
         echo \"jars ready in $dir\"\n"
            .to_string(),
    ));
    Ok(())
}

/// Rewrite `runtime/action_cable.rb`'s `ActionCable::Connection.build`
/// factory from the ingested app, so a `/cable` handshake — and a
/// `Connection::TestCase`'s `connect` — is identified by running the
/// app's OWN `ApplicationCable::Connection#connect`.
///
/// THE SHAPE, and why it is a generated factory rather than a lookup:
/// `Cable.upgrade` has to reach a class the ingested app named, on a
/// target with no `const_get`. `apply_controller_dispatch` already
/// answers exactly that question for controllers with eager arms, and
/// this is the same answer for the one class Rails' convention fixes
/// the name of. It also preserves the property the CRuby overlay chose
/// a `REGISTRY` for: only a class the generator emitted an arm for is
/// reachable, so nothing that arrives on the wire can widen the set.
///
/// NO-OP when the app declares no `ApplicationCable::Connection` — the
/// default arm connects anonymously, which is what an app
/// that never asked for identity needs. `ApplicationCable::Connection`
/// is Rails' fixed convention name, the same convention
/// `runtime/action_cable.rb` already encodes by giving
/// `ActionCable::Connection::Base` a body.
///
/// Written as a re-appliable SPAN replace between two markers, for the
/// reason `patch_harness_dispatch` gives: a match on the default body's
/// text would freeze this at today's spelling while `cable.rb` moved on.
fn apply_cable_connection(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: cable-connection\n";
    const TAIL: &str = "    # <<< generated: cable-connection\n";
    const CONNECTION_CLASS: &str = "ApplicationCable::Connection";

    if !app
        .library_classes
        .iter()
        .any(|lc| lc.name.0.as_str() == CONNECTION_CLASS)
    {
        return;
    }
    let generated = format!(
        "{HEAD}    def self.build(cookies)\n      {CONNECTION_CLASS}.new(cookies)\n    end\n{TAIL}"
    );
    for (path, content) in files.iter_mut() {
        if !path.ends_with("/action_cable.rb") {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
    }
}

/// Route `ActionText::Content#to_s` through the app's own
/// `layouts/action_text/contents/_content` layout — Rails' contract,
/// and how a rich text arrives wrapped (campfire's layout is
/// `<div class="trix-content">`, and its stylesheet keys on the
/// class). The layout is per-APP and the strict targets resolve every
/// call statically, so the dispatch is generated exactly when the
/// emitted tree carries the lowered view module; a tree without one
/// keeps the bare fragment, which is Rails' own fallback. Found by
/// `scripts/campfire-compare`: the CRuby overlay had already grown
/// this (guarded on `defined?`, which a static target has no lane
/// for), so the binary rendered every message body one div short of
/// both Rails and the ruby lane.
///
/// And, in the same file, `Content.render_attachment` — Rails'
/// `render_action_text_attachments`, per app: one arm per attachable
/// model whose `to_attachable_partial_path` names a partial the tree
/// carries (`lower::attachable::attachable_partials`), dispatched on
/// the node's resolved model NAME with the finder and the view module
/// spelled as literals — the shape `apply_attachable_locate` writes,
/// for the same reason. The partial receives the RECORD: Rails hands it
/// the Attachment, which delegates to the record, and campfire's
/// `users/_mention` reads only record methods. A model whose partial
/// the tree does not have gets no arm.
/// `ActionText::ContentHelper.app_allowed_attributes` — the attributes the
/// app's boot adds to Action Text's sanitizer allow-list, read at ingest
/// (`App::content_helper_allowed_attributes`) and spelled here as the
/// literal the runtime's `allowed_attributes` appends. An app that adds
/// none keeps the default body's `[]`.
fn apply_content_helper_attributes(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: content-helper-attributes\n";
    const TAIL: &str = "    # <<< generated: content-helper-attributes\n";
    if app.content_helper_allowed_attributes.is_empty() {
        return;
    }
    let list = app
        .content_helper_allowed_attributes
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!("{HEAD}    def self.app_allowed_attributes\n      [{list}]\n    end\n{TAIL}");
    for (path, content) in files.iter_mut() {
        if !path.ends_with("runtime/action_text.rb") {
            continue;
        }
        if let Some(start) = content.find(HEAD) {
            if let Some(rel_end) = content[start..].find(TAIL) {
                let end = start + rel_end + TAIL.len();
                content.replace_range(start..end, &body);
            }
        }
    }
}

fn apply_content_layout(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: content-layout\n";
    const TAIL: &str = "    # <<< generated: content-layout\n";
    const VIEW: &str = "app/views/layouts/action_text/contents/_content.rb";
    const RENDER_HEAD: &str = "    # >>> generated: attachment-render\n";
    const RENDER_TAIL: &str = "    # <<< generated: attachment-render\n";
    const PLAIN_HEAD: &str = "    # >>> generated: attachment-plain-text\n";
    const PLAIN_TAIL: &str = "    # <<< generated: attachment-plain-text\n";
    const BUILT_HEAD: &str = "    # >>> generated: attachable-by-content-type\n";
    const BUILT_TAIL: &str = "    # <<< generated: attachable-by-content-type\n";

    let has_layout = files.iter().any(|(path, _)| path.ends_with(VIEW));
    // What the layout YIELDS is Action Text's own partial,
    // `action_text/contents/_content.html.erb`, rendered: `<%=
    // render_action_text_content(content) %>` and the newline that
    // ends that file. The app's layout then writes `<%= yield -%>`,
    // whose `-` drops ITS newline, so the one before `</div>` is the
    // partial's — `…boxes.\n</div>` on campfire's room page, where the
    // emit wrote `…boxes.</div>` without it.
    let layout = format!(
        "{HEAD}    def rendered_html\n      \
         Views::Layouts::ActionText::Contents.content(render_attachments + \"\\n\")\n    end\n{TAIL}"
    );

    // Two kinds of arm, in the order campfire's own `from_node` reopen
    // dispatches them: a class the node names by CONTENT TYPE builds
    // itself from the node (`OpengraphEmbed.from_node`, which applies
    // its own `web_url` filter and answers nil for any other node), and
    // is asked first; then the sgid's model, by name.
    //
    // And beside it `Content.attachment_plain_text` — `Attachment#
    // to_plain_text`'s dispatch, the same arms in the same order, for
    // the classes that define `attachable_plain_text_representation`:
    // a content-type class answers through the instance its `from_node`
    // built, a model through the record, and a node neither claims
    // falls through to the framework's own attachables
    // (`default_attachment_plain_text`). A class without the hook gets
    // no arm here — Rails would answer its caption, which is what the
    // fall-through does.
    let mut by_content_type = String::new();
    let mut by_model = String::new();
    let mut plain_by_content_type = String::new();
    let mut plain_by_model = String::new();
    // And `Content.content_type_attachable` — the same content-type arms
    // answering the INSTANCE, for `Attachment#attachable`.
    let mut built_by_content_type = String::new();
    for binding in crate::lower::attachable::attachable_partial_bindings(app) {
        let (dir, stem) = crate::lower::view_to_library::split_view_name(&binding.partial);
        let view_path = format!("app/views/{dir}/_{stem}.rb");
        if !files.iter().any(|(path, _)| path.ends_with(&view_path)) {
            continue;
        }
        let module = crate::lower::view_to_library::view_module_id(dir);
        let name = binding.class.0.as_str();
        let local = &binding.local;
        if binding.content_type.is_some() {
            by_content_type.push_str(&format!(
                "      {local} = ::{name}.from_node(attachment)\n      \
                 unless {local}.nil?\n        {local}.attachment = attachment\n        \
                 return {module}.{stem}({local})\n      end\n",
                module = module.0.as_str(),
            ));
            built_by_content_type.push_str(&format!(
                "      {local} = ::{name}.from_node(attachment)\n      \
                 unless {local}.nil?\n        {local}.attachment = attachment\n        \
                 return {local}\n      end\n",
            ));
            if binding.plain_text {
                plain_by_content_type.push_str(&format!(
                    "      {local} = ::{name}.from_node(attachment)\n      \
                     return {local}.attachable_plain_text_representation(attachment.caption) unless {local}.nil?\n",
                ));
            }
        } else {
            by_model.push_str(&format!(
                "      when \"{name}\"\n        {local} = {name}.find_by({{ id: attachment.resolved_id }})\n        \
                 {local}.nil? ? \"\" : {module}.{stem}({local})\n",
                module = module.0.as_str(),
            ));
            if binding.plain_text {
                plain_by_model.push_str(&format!(
                    "      when \"{name}\"\n        {local} = {name}.find_by({{ id: attachment.resolved_id }})\n        \
                     return {local}.attachable_plain_text_representation(attachment.caption) unless {local}.nil?\n",
                ));
            }
        }
    }
    // Action Text's own attachable, after the app's: a blob's node
    // renders `active_storage/blobs/_blob` with the ATTACHMENT as `blob`
    // (it delegates to the blob, as Rails' does), when the tree carries
    // the app's copy of that partial. campfire overrides it to show an
    // embedded file by name rather than making a preview on view.
    if files.iter().any(|(path, _)| path.ends_with("app/views/active_storage/blobs/_blob.rb")) {
        by_model.push_str(
            "      when \"ActiveStorage::Blob\"\n        \
             attachment.blob.nil? ? \"\" : Views::ActiveStorage::Blobs.blob(attachment)\n",
        );
    }
    let built = if built_by_content_type.is_empty() {
        None
    } else {
        Some(format!(
            "{BUILT_HEAD}    def self.content_type_attachable(attachment)\n{built_by_content_type}      nil\n    end\n{BUILT_TAIL}"
        ))
    };
    let plain = if plain_by_content_type.is_empty() && plain_by_model.is_empty() {
        None
    } else {
        let sgid_dispatch = if plain_by_model.is_empty() {
            String::new()
        } else {
            format!("      case attachment.resolved_model_name\n{plain_by_model}      end\n")
        };
        Some(format!(
            "{PLAIN_HEAD}    def self.attachment_plain_text(attachment)\n{plain_by_content_type}{sgid_dispatch}      \
             Content.default_attachment_plain_text(attachment)\n    end\n{PLAIN_TAIL}"
        ))
    };
    let render = if by_content_type.is_empty() && by_model.is_empty() {
        None
    } else {
        let sgid_dispatch = if by_model.is_empty() {
            "      \"\"\n".to_string()
        } else {
            format!("      case attachment.resolved_model_name\n{by_model}      else\n        \"\"\n      end\n")
        };
        Some(format!(
            "{RENDER_HEAD}    def self.render_attachment(attachment)\n{by_content_type}{sgid_dispatch}    end\n{RENDER_TAIL}"
        ))
    };

    for (path, content) in files.iter_mut() {
        if !path.ends_with("runtime/action_text.rb") {
            continue;
        }
        if has_layout {
            if let Some(start) = content.find(HEAD) {
                if let Some(rel_end) = content[start..].find(TAIL) {
                    let end = start + rel_end + TAIL.len();
                    content.replace_range(start..end, &layout);
                }
            }
        }
        if let Some(render) = &render {
            if let Some(start) = content.find(RENDER_HEAD) {
                if let Some(rel_end) = content[start..].find(RENDER_TAIL) {
                    let end = start + rel_end + RENDER_TAIL.len();
                    content.replace_range(start..end, render);
                }
            }
        }
        if let Some(built) = &built {
            if let Some(start) = content.find(BUILT_HEAD) {
                if let Some(rel_end) = content[start..].find(BUILT_TAIL) {
                    let end = start + rel_end + BUILT_TAIL.len();
                    content.replace_range(start..end, built);
                }
            }
        }
        if let Some(plain) = &plain {
            if let Some(start) = content.find(PLAIN_HEAD) {
                if let Some(rel_end) = content[start..].find(PLAIN_TAIL) {
                    let end = start + rel_end + PLAIN_TAIL.len();
                    content.replace_range(start..end, plain);
                }
            }
        }
    }
}

/// Write one arm per app channel into `runtime/action_cable.rb`'s
/// `ActionCable::Channel.build`, so a subscribe frame — and a
/// `Channel::TestCase`'s `subscribe` — reaches the class it NAMES and
/// the app's own `subscribed` runs.
///
/// THE THIRD EAGER-ARM FACTORY, after `apply_controller_dispatch` and
/// `apply_cable_connection`, and for the identical reason: the name
/// arrives as a STRING off the wire and this target resolves every call
/// statically, so there is no `const_get` to reach the class with. The
/// property that makes an eager arm the right answer rather than a
/// workaround is that the set is CLOSED — only a class the generator
/// wrote an arm for is reachable, so a crafted name on the wire cannot
/// widen it.
///
/// TRANSITIVE DESCENT, not a one-level check. campfire's channels are
/// two and three deep (`RoomChannel < ApplicationCable::Channel <
/// ActionCable::Channel::Base`, and `RoomMessagesChannel` below that),
/// so asking which classes name `ActionCable::Channel::Base` as their
/// parent finds NONE of them.
///
/// ABSTRACT MIDDLE CLASSES GET ARMS TOO, deliberately. Rails would
/// happily build an `ApplicationCable::Channel` for a client that named
/// it; it defines no `subscribed`, so the base's no-op runs, no stream
/// is registered and the subscription is confirmed with nothing in it.
/// Skipping it would be a policy this pipeline invented — and telling
/// the two apart needs `abstract_class`-style intent that a channel
/// hierarchy does not carry.
///
/// `Turbo::StreamsChannel` is spelled in the DEFAULT body in
/// `runtime/action_cable.rb` rather than generated here: it is a runtime
/// class, not an app one, so no descent over `library_classes` finds it,
/// and every tree carries it.
fn apply_cable_channels(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: cable-channels\n";
    const TAIL: &str = "    # <<< generated: cable-channels\n";
    const ROOT: &str = "ActionCable::Channel::Base";

    let mut channels: Vec<&str> = Vec::new();
    let mut known: Vec<&str> = vec![ROOT];
    // Fixpoint rather than one pass: `library_classes` is in ingest
    // order, and a subclass may be listed before the parent that puts
    // it in the set.
    loop {
        let before = known.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str();
            if lc.is_module || known.contains(&name) {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            if known.contains(&parent.0.as_str()) {
                known.push(name);
                channels.push(name);
            }
        }
        if known.len() == before {
            break;
        }
    }
    channels.sort_unstable();

    let mut generated = String::from(HEAD);
    generated.push_str("    def self.build(name, connection, identifier)\n");
    for name in channels.iter().chain(["Turbo::StreamsChannel"].iter()) {
        generated.push_str(&format!(
            "      if name == \"{name}\"\n\
             \x20       return {name}.new(\n\
             \x20         connection, identifier, ActionCable::Channel::Parameters.new(identifier))\n\
             \x20     end\n",
        ));
    }
    generated.push_str("      nil\n    end\n");
    generated.push_str(TAIL);

    for (path, content) in files.iter_mut() {
        if !path.ends_with("/action_cable.rb") {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
    }
}

/// Write one `GlobalID::Locator.locate_<model>` entry point into
/// `runtime/global_id_locator.rb` for every model class an `only:`
/// names, with the finder spelled as a LITERAL constant.
///
/// The pair to [`crate::lower::global_id_locate`], which rewrote each
/// call site to the name generated here. Both halves exist because
/// `only:` is the finder and it arrives as a class object: CRuby
/// dispatches through the singleton, a strict target has none, and
/// spinel emits a call to a class method `ActiveRecord::Base` never
/// defines (matz/spinel#4217). The set of models is CLOSED at ingest —
/// one per literal call site — so specializing removes the dispatch
/// rather than working around it.
///
/// NO-OP with an empty span when the app has no `locate` call site,
/// which is every corpus app but campfire. The generic `locate` stays
/// in the file either way: it is what a computed `only:` still reaches,
/// and it runs correctly on the Ruby lanes.
///
/// A SPAN REPLACE between two markers, not a match on today's text —
/// same reason `apply_cable_connection` gives.
/// `ActiveRecord::Base.instantiate_named` (runtime/spinel/
/// active_record_db_config.rb): one arm per model, by the name
/// `record.class.name` answers for it, and an STI subclass name the
/// base hydrates. See `lower::record_snapshot`.
fn apply_instantiate_named(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "      # >>> generated: instantiate-named\n";
    const TAIL: &str = "      # <<< generated: instantiate-named\n";
    let mut arms: Vec<(String, String)> = Vec::new();
    for model in &app.models {
        // Only a class with a table has rows to snapshot, and only it has
        // `instantiate`: an ActiveModel class (campfire's
        // `Opengraph::Location`) or the abstract `ApplicationRecord` in
        // the list was a call to a method nothing defines — spinel
        // refused the whole build over it.
        if !app.schema.tables.contains_key(&model.table.0) {
            continue;
        }
        let name = model.name.0.as_str().to_string();
        arms.push((name.clone(), name.clone()));
        for sub in &model.sti_subclass_names {
            if !app.models.iter().any(|m| m.name == *sub) {
                arms.push((sub.0.as_str().to_string(), name.clone()));
            }
        }
    }
    // No models, no arms: the default body (a NameError) stays.
    if arms.is_empty() {
        return;
    }
    let mut generated = String::from(HEAD);
    generated.push_str("      case name\n");
    for (name, class) in &arms {
        generated.push_str(&format!(
            "      when {name:?} then {class}.instantiate(attributes)\n"
        ));
    }
    generated.push_str("      else raise NameError, \"uninitialized constant #{name}\"\n      end\n");
    generated.push_str(TAIL);
    for (path, content) in files.iter_mut() {
        if !path.ends_with("active_record_db_config.rb") {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
    }
}

fn apply_global_id_locate(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: global-id-locate\n";
    const TAIL: &str = "    # <<< generated: global-id-locate\n";

    let mut generated = String::from(HEAD);
    for model in &app.global_id_locate_models {
        let name = model.as_str();
        let suffix = crate::lower::global_id_locate::entry_point_suffix(name);
        let find = global_id_find(app, name);
        generated.push_str(&format!(
            "    def self.locate_{suffix}(gid_param)\n\
             \x20     parts = parts_from(gid_param)\n\
             \x20     return nil if parts.nil?\n\
             {find}\
             \x20   end\n",
        ));
    }
    for model in &app.global_id_locate_signed_models {
        let name = model.as_str();
        let suffix = crate::lower::global_id_locate::entry_point_suffix(name);
        let find = global_id_find(app, name);
        generated.push_str(&format!(
            "    def self.locate_signed_{suffix}(sgid, purpose)\n\
             \x20     parts = parts_from_signed(sgid, purpose)\n\
             \x20     return nil if parts.nil?\n\
             {find}\
             \x20   end\n",
        ));
    }
    generated.push_str(TAIL);

    for (path, content) in files.iter_mut() {
        if !path.ends_with("global_id_locator.rb") {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
    }
}

/// The name check and finder shared by both `locate_*` entry points.
///
/// An STI base also accepts its known subclasses' names: Rails mints an
/// STI row's GlobalID with the row's own class, and `only: Room` admits
/// every descendant. Nothing is constantized from the wire — the names
/// are the closed set `sti_scope` stamped on the base model. The finder
/// stays the base's (hydration is base-classed), so a subclass name is
/// confirmed against the row: the record must mint that same name, or
/// `Rooms::Open/<id of a plain Room>` would answer the plain room where
/// Rails' scoped `Rooms::Open.find` finds nothing.
fn global_id_find(app: &App, base: &str) -> String {
    let subclasses = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == base)
        .map(|model| model.sti_subclass_names.as_slice())
        .unwrap_or_default();
    let allowed = std::iter::once(base)
        .chain(subclasses.iter().map(|name| name.0.as_str()))
        .map(|name| format!("parts[1] == \"{name}\""))
        .collect::<Vec<_>>()
        .join(" || ");
    if subclasses.is_empty() {
        return format!(
            "      return nil unless {allowed}\n\n      {base}.find(cast_id(parts[2]))\n"
        );
    }
    format!(
        "      return nil unless {allowed}\n\n\
         \x20     record = {base}.find(cast_id(parts[2]))\n\
         \x20     return nil unless record.to_gid_param == GlobalID.param(parts[1], record.id)\n\n\
         \x20     record\n"
    )
}

/// Write `ActionText::Attachable.locate(model_name, id)` into
/// `runtime/global_id_locator.rb`: one `when "<Model>" then
/// <Model>.find_by(id: id)` per model that mixes `ActionText::Attachable`
/// in (directly or through a concern — `lower::attachable_models`
/// resolves the include), with the finder spelled as a LITERAL constant.
///
/// The read half of the sgid round trip that `Attachment#attachable`
/// needs and a shared runtime cannot write: the sgid verifies and
/// splits into a name and an id there, and turning the name into a
/// class is the one step that is either reflection (`constantize`,
/// which this pipeline does not emit and a wire string should not
/// steer) or this switch. The set is closed at ingest, so the switch IS
/// the registry, and an app with no attachable model keeps the
/// scaffold's nil-answering body — every sgid then reads as missing,
/// Rails' answer for a gid naming no class.
///
/// A SPAN REPLACE between two markers, like `apply_global_id_locate`
/// above it, and on the same file — both trees require it once.
///
/// And, in the same file, `ActionText::Attachment.permitted_without_signature`
/// — the models whose sgid the app resolves with a FAILED signature
/// (campfire's rotated-secret tolerance, read by
/// `ingest::on_load_reopen` from its `from_node` reopen), as a literal
/// array. Only written when the app has the list; the scaffold's empty
/// default stands otherwise, and a tampered sgid is missing.
fn apply_attachable_locate(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: attachable-locate\n";
    const TAIL: &str = "    # <<< generated: attachable-locate\n";
    const UNSIGNED_HEAD: &str = "    # >>> generated: attachable-unsigned\n";
    const UNSIGNED_TAIL: &str = "    # <<< generated: attachable-unsigned\n";

    let models = crate::lower::attachable::attachable_models(app);
    let mut generated = String::from(HEAD);
    if !models.is_empty() {
        generated.push_str("    def self.locate(model_name, id)\n      case model_name\n");
        for model in &models {
            let name = model.0.as_str();
            generated.push_str(&format!(
                "      when \"{name}\"\n        {name}.find_by({{ id: id }})\n"
            ));
        }
        generated.push_str("      end\n    end\n");
    }
    generated.push_str(TAIL);

    let mut unsigned = String::from(UNSIGNED_HEAD);
    if !app.attachable_unsigned_models.is_empty() {
        let names: Vec<String> = app
            .attachable_unsigned_models
            .iter()
            .map(|m| format!("\"{}\"", m.as_str()))
            .collect();
        unsigned.push_str(&format!(
            "    def self.permitted_without_signature\n      [{}]\n    end\n",
            names.join(", ")
        ));
    }
    unsigned.push_str(UNSIGNED_TAIL);

    for (path, content) in files.iter_mut() {
        if !path.ends_with("global_id_locator.rb") {
            continue;
        }
        if !models.is_empty() {
            if let Some(start) = content.find(HEAD) {
                if let Some(rel_end) = content[start..].find(TAIL) {
                    let end = start + rel_end + TAIL.len();
                    content.replace_range(start..end, &generated);
                }
            }
        }
        if !app.attachable_unsigned_models.is_empty() {
            if let Some(start) = content.find(UNSIGNED_HEAD) {
                if let Some(rel_end) = content[start..].find(UNSIGNED_TAIL) {
                    let end = start + rel_end + UNSIGNED_TAIL.len();
                    content.replace_range(start..end, &unsigned);
                }
            }
        }
    }
}

/// De-cable the CRuby/JRuby overlay for a broadcast-less app: drop
/// `cable.rb` and the three /cable seams in `config.ru` (the require,
/// the `Cable::Registry` transport registration, and the WebSocket-
/// upgrade hijack branch). Pairs with `apply_gemfile_trim` dropping
/// `gem "websocket-driver"` from the same tree — keeping the wiring
/// would force a gem install for a surface the app can't reach
/// (issue #67: the Roda exemplar's tree loaded websocket-driver at
/// boot despite the app having no websockets).
fn apply_cable_strip(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    if crate::lower::app_broadcasts_live(app) {
        return Ok(());
    }
    files.retain(|(p, _)| p != "cable.rb");
    for (p, content) in files.iter_mut() {
        if p == "config.ru" {
            *content = strip_cable_from_config_ru(content)?;
        }
    }
    Ok(())
}

/// String half of `apply_cable_strip` (separated for unit testing).
/// Marker-based rather than exact-match so comment rewording in
/// config.ru doesn't silently defeat it; errors loudly when a marker
/// is missing so a config.ru restructure updates this function too.
fn strip_cable_from_config_ru(content: &str) -> Result<String, String> {
    let mut out: Vec<&str> = Vec::new();
    let mut found_require = false;
    let mut found_registry = false;
    let mut found_branch = false;
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if line == "require_relative \"cable\"" {
            found_require = true;
            continue;
        }
        // Comment paragraph + `Broadcasts.set_transport(Cable::Registry)`.
        if line.starts_with("# Register the Cable registry") {
            found_registry = true;
            for l in lines.by_ref() {
                if l.contains("Broadcasts.set_transport") {
                    break;
                }
            }
            if lines.peek() == Some(&"") {
                lines.next();
            }
            continue;
        }
        // The hijack branch inside the Rack lambda: comment block through
        // its closing two-space `end`.
        if line.trim_start().starts_with("# WebSocket upgrade:") {
            found_branch = true;
            for l in lines.by_ref() {
                if l == "  end" {
                    break;
                }
            }
            if lines.peek() == Some(&"") {
                lines.next();
            }
            continue;
        }
        out.push(line);
    }
    if !(found_require && found_registry && found_branch) {
        return Err(format!(
            "strip_cable_from_config_ru: cable markers not all found in ruby_overlay \
             config.ru (require={found_require} registry={found_registry} \
             branch={found_branch}) — config.ru restructured? Update the markers here."
        ));
    }
    let mut s = out.join("\n");
    s.push('\n');
    Ok(s)
}

/// Regenerate `app/views.rb` (the per-app `Views::*` aggregator) from the
/// view files actually emitted into the tree, replacing the scaffold's
/// blog-hardcoded require list. Without this, every non-blog app's emitted
/// views are orphaned — `app/views.rb` ships the blog's `views/articles/*`
/// requires (most of which don't exist for another app), and the real
/// emitted views are never loaded, so any `Views::X.method` call fails at
/// dispatch.
///
/// View `.rb` files only reference each other at *request* time (via
/// `render partial:`), never at load time, so the require order is free; we
/// list partials (`_name`) before templates and otherwise sort, purely for
/// a stable, legible file. For the blog the emitted view set matches the
/// scaffold's, so the generated aggregator loads the same modules the
/// hand-written one did. An app with no views at all (an API app that
/// only renders JSON) gets an aggregator that requires nothing: keeping
/// the scaffold's copy would require blog views the tree does not have.
fn apply_views_aggregator(files: &mut [(String, String)]) {
    use std::fmt::Write;

    let mut views: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| p.starts_with("app/views/") && p.ends_with(".rb"))
        .collect();
    // Partials (`_foo.rb`) first, then alphabetical — deterministic.
    views.sort_by_key(|p| {
        let is_partial = Path::new(p)
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| f.starts_with('_'));
        (!is_partial, *p)
    });

    let mut body = String::from(
        "# Loads every view module into the Views::* namespace. Generated\n\
         # from the emitted app/views/ tree (see apply_views_aggregator).\n\
         # Each file pulls its own dependencies, so the order here is only\n\
         # for legibility (partials first).\n",
    );
    for path in views {
        // `app/views/x/y.rb` -> require_relative "views/x/y" (relative to
        // `app/views.rb`, whose directory is `app/`).
        let anchor = path
            .strip_prefix("app/")
            .unwrap_or(path)
            .strip_suffix(".rb")
            .unwrap_or(path);
        writeln!(body, "require_relative {anchor:?}").unwrap();
    }

    for (path, content) in files.iter_mut() {
        if path == "app/views.rb" {
            *content = body.clone();
        }
    }
}

/// Generate `app/models.rb` — the aggregator that loads every emitted
/// `app/models/*.rb` (AR models, ingested support classes, synthesized
/// `*Row`/`*Params` siblings). Counterpart of `apply_views_aggregator`,
/// with one semantic difference: views never need each other at load
/// time, while model files DO carry load-time edges (superclass,
/// `include`s, class-body constant refs) — those stay as per-file
/// `require_relative` headers, so the order here is still free. What
/// model files no longer carry is requires for method-body-only refs
/// (see the edge classification in `emit::ruby::library`); this file is
/// what guarantees those targets are loaded before any dispatch. The
/// scaffold main.rb and test_helper.rb require it up front.
///
/// Always emitted (even empty) because main.rb's require is
/// unconditional.
/// Append the app's initializer-registered `prepend`/`include` lines to
/// `boot.rb`.
///
/// AT THE END OF BOOT, and that placement is the whole design: a mixin
/// names two constants and both must already be defined, which is only
/// guaranteed after `app/models` and `app/views` have loaded. Rails gets
/// the same ordering from `to_prepare`, which runs after eager load.
///
/// Only mixins `lower::module_mixins` kept reach here — it has already
/// dropped and reported the ones naming a constant no tree defines, so
/// every line written here resolves.
///
/// Appended rather than spliced at a marker: it is the last thing in the
/// file, so there is nothing below it to anchor against and nothing a
/// scaffold edit could shift it away from.
///
/// Which spelling of the mixin line a tree gets. Not a "do it / skip
/// it" flag: both forms perform the mixin, they just say so differently.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MixinForm {
    /// `X.prepend Y` — the ruby family, and what Rails itself writes.
    ExplicitReceiver,
    /// `class X\n  prepend Y\nend` — spinel, which refuses an explicit
    /// receiver.
    Reopen,
}

/// `class` or `module` for a reopen of `target`.
///
/// A mixin can name a module (`X.include Y` where X is a module), and
/// reopening one with the wrong keyword is a TypeError at boot. Ingested
/// constants answer from `library_classes`; a runtime-provided target is
/// credited the way `lower::module_mixins::RUNTIME_MIXIN_TARGETS`
/// credits it, and the one entry there — `Turbo::StreamsChannel` — is a
/// class (`runtime/spinel/turbo_streams.rb`, and the test that pins the
/// name pins that too). Anything unrecognised falls to `class`, which is
/// what every performed mixin in the corpus is.
fn mixin_target_keyword(app: &App, target: &str) -> &'static str {
    let is_module = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == target)
        .map(|lc| lc.is_module);
    match is_module {
        Some(true) => "module",
        _ => "class",
    }
}

/// THE FORM DIFFERS BY TARGET, and the reason is spinel's, not ours.
/// `X.prepend Y` through an explicit receiver is refused outright there
/// — "the class graph, ancestor chain, and method/ivar layout are baked
/// at compile time, so a class cannot be restructured through an
/// explicit receiver" — which stopped the campfire binary building the
/// moment `Turbo::StreamsChannel` existed for the line to name. So the
/// spinel tree gets the REOPEN form, which says the same thing in the
/// spelling that target accepts.
///
/// THAT FORM USED TO BE A SILENT NO-OP, WHICH IS WHY THIS EMITTED A
/// COMMENT INSTEAD. `class X; prepend Y; end` compiled and did nothing:
/// a `Guard#hello` calling `super` printed `guarded hi` under CRuby and
/// `hi` from the spinel binary, with no diagnostic — campfire's
/// authorization module back in the tree, tested, and out of the lookup
/// chain, the exact failure `lower::module_mixins` exists to prevent,
/// minus the report. **FIXED UPSTREAM in matz/spinel `a7b6f726`**
/// (`register_prepends` walked the class body alone where
/// `register_includes` had always had a second pass over every
/// ClassNode; the two can no longer disagree about what a reopen means).
/// Verified by RUNNING it, not by the issue being closed: the repro
/// prints `guarded:base` from a spinel binary.
///
/// The ruby family keeps the explicit-receiver form it has always
/// emitted — it is landed, tested by `overlay_cable_dispatch`, and
/// CRuby has no quarrel with it. One form each, each the one its target
/// accepts.
fn apply_module_mixins(files: &mut Vec<(String, String)>, app: &App, form: MixinForm) {
    use std::fmt::Write;

    if app.module_mixins.is_empty() && app.initializer_filters.is_empty() {
        return;
    }
    let mut block = String::from(match form {
        MixinForm::ExplicitReceiver => {
            "\n# Module mixins the app registers in config/initializers/ \
(generated —\n\
         # see apply_module_mixins). At the END of boot because a mixin names\n\
         # two constants and both have to be defined: Rails gets the same\n\
         # ordering from `to_prepare`, which runs after eager load.\n\
         #\n\
         # `prepend` inserts AHEAD of the target in the lookup chain, which is\n\
         # what lets the module's method call `super` — an `include` would\n\
         # never run where the class defines its own.\n"
        }
        MixinForm::Reopen => {
            "\n# Module mixins the app registers in config/initializers/ \
(generated —\n\
         # see apply_module_mixins). At the END of boot because a mixin names\n\
         # two constants and both have to be defined: Rails gets the same\n\
         # ordering from `to_prepare`, which runs after eager load.\n\
         #\n\
         # `prepend` inserts AHEAD of the target in the lookup chain, which is\n\
         # what lets the module's method call `super` — an `include` would\n\
         # never run where the class defines its own.\n\
         #\n\
         # WRITTEN AS A REOPEN, not `X.prepend Y`: spinel bakes the ancestor\n\
         # chain at compile time and refuses an explicit receiver. This form\n\
         # was a silent no-op until matz/spinel a7b6f726 — if a guard below\n\
         # stops running, check that fix is in the spinel you built with.\n"
        }
    });
    for mixin in &app.module_mixins {
        let _ = match form {
            MixinForm::ExplicitReceiver => writeln!(
                block,
                "{}.{} {}",
                mixin.target.as_str(),
                mixin.kind.as_str(),
                mixin.module.as_str()
            ),
            // `class X` on a module is a TypeError at boot, so the
            // keyword is chosen from what the tree says the target is —
            // loud either way, but this way it does not happen.
            MixinForm::Reopen => writeln!(
                block,
                "{} {}\n  {} {}\nend",
                mixin_target_keyword(app, mixin.target.as_str()),
                mixin.target.as_str(),
                mixin.kind.as_str(),
                mixin.module.as_str()
            ),
        };
    }
    // The filters, one reopen per target. The runtime controller's
    // `process_action` calls `initializer_filters(action_name)` before
    // the action and stops when a filter has answered (`performed?`),
    // the same shape the lowered app controllers spell inline. A
    // reopen on both lanes: CRuby redefines the seam's default, and
    // spinel takes the later definition in a reopened class.
    let mut targets: Vec<&str> = app.initializer_filters.iter().map(|f| f.target.as_str()).collect();
    targets.dedup();
    for target in targets {
        let _ = writeln!(
            block,
            "\n# `before_action` the app registers on a framework controller in\n\
             # config/initializers/ (generated — see apply_module_mixins). The\n\
             # controller asks this before its action and stops if it answered.\n\
             class {target}\n  def initializer_filters(action_name)"
        );
        for f in app.initializer_filters.iter().filter(|f| f.target.as_str() == target) {
            if f.only.is_empty() {
                let _ = writeln!(block, "    {}", f.method.as_str());
            } else {
                let only: Vec<String> = f.only.iter().map(|a| format!(":{}", a.as_str())).collect();
                let _ = writeln!(block, "    {} if [{}].include?(action_name)", f.method.as_str(), only.join(", "));
            }
            let _ = writeln!(block, "    return nil if performed?");
        }
        let _ = writeln!(block, "    nil\n  end\nend");
    }
    for (path, content) in files.iter_mut() {
        if path == "boot.rb" {
            content.push_str(&block);
            return;
        }
    }
}

fn apply_models_aggregator(files: &mut Vec<(String, String)>) {
    use std::fmt::Write;

    let mut models: Vec<String> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| p.starts_with("app/models/") && p.ends_with(".rb"))
        .map(str::to_string)
        .collect();
    models.sort();

    let mut body = String::from(
        "# Loads every model/support class under app/models/. Generated\n\
         # from the emitted tree (see apply_models_aggregator). Each file\n\
         # requires its own LOAD-time deps (superclass, includes, class-body\n\
         # constants), so the order here is only for legibility; method-body\n\
         # references between these files rely on this aggregator having\n\
         # run by dispatch time.\n",
    );
    for path in &models {
        // `app/models/x.rb` -> require_relative "models/x" (relative to
        // `app/models.rb`, whose directory is `app/`).
        let anchor = path
            .strip_prefix("app/")
            .unwrap_or(path)
            .strip_suffix(".rb")
            .unwrap_or(path);
        writeln!(body, "require_relative {anchor:?}").unwrap();
    }

    if let Some((_, content)) = files.iter_mut().find(|(p, _)| p == "app/models.rb") {
        *content = body;
    } else {
        files.push(("app/models.rb".to_string(), body));
    }
}

/// Replace the scaffold `main.rb`'s blog-hardcoded
/// `instantiate_controller` (`:articles`/`:comments` only) with a
/// dispatch generated from the app's own route table — one
/// `when :<sym> then <Controller>.new` arm per controller the router can
/// reach. Without this, every non-blog app's routes resolve to a `nil`
/// controller and crash at dispatch (`controller.params=` on nil) — and
/// under spinel AOT the stale `ArticlesController` constant is already a
/// compile error. The symbol derivation (`controller_symbol`) is the same
/// one the emitted route table uses, so the arms match the router's
/// `:sym` exactly. For the blog the generated arms equal the hardcoded
/// stub, so its output is byte-identical.
///
/// `lazy_requires` selects the require strategy. The CRuby/JRuby trees
/// pass true: each arm `require_relative`s its controller at first
/// dispatch (Rails-faithful autoload; a never-dispatched controller with
/// unsatisfiable deps doesn't abort boot) and the eager controller-require
/// header is stripped from `config/routes.rb`. The spinel tree passes
/// false: AOT resolves the whole require graph statically, so arms are
/// bare `<Class>.new` and routes.rb keeps its eager header — there is no
/// lazy escape hatch to preserve.
/// Rewrite the test harness's controller-dispatch table in
/// `RequestDispatch#dispatch_request`.
///
/// The harness carries a blog-shaped copy of main.rb's table — two
/// eager `require_relative "../app/controllers/…"` lines plus a
/// two-arm case. campfire's spliced `SessionTestHelper#sign_in` POSTs
/// to `session_url`, so EVERY controller test reached it and died at
/// `cannot load such file -- app/controllers/articles_controller`.
///
/// Written as a re-appliable SPAN replace, not a match on the blog
/// text, because `apply_controller_dispatch` runs twice on a CRuby
/// tree: once in `spinel_files` (eager) and again in
/// `ruby_runtime_files` (lazy, over the overlay main.rb that supersedes
/// it). A one-shot text match would freeze the harness at the first
/// pass's shape while main.rb moved on — and on a lazy tree, where
/// routes.rb's eager requires have been stripped, that means nothing
/// requires controllers at all.
/// Load the overlay's `test/test_support_cruby.rb` (helpers only CRuby
/// can run — `stub_const`) at the END of the ruby tree's test helper,
/// after `TestBase` exists. The spinel tree never gets the file, so a
/// test calling one fails there as the gap it is.
fn apply_cruby_test_support(files: &mut [(String, String)]) {
    if !files.iter().any(|(p, _)| p == "test/test_support_cruby.rb") {
        return;
    }
    let line = "require_relative \"test_support_cruby\"";
    if let Some((_, helper)) = files.iter_mut().find(|(p, _)| p == "test/test_helper.rb") {
        if !helper.contains(line) {
            if !helper.ends_with('\n') {
                helper.push('\n');
            }
            helper.push_str(line);
            helper.push('\n');
        }
    }
}

fn patch_harness_dispatch(content: &mut String, generated: &str) {
    const HEAD: &str = "    controller = case matched.controller\n";
    const TAIL: &str = "                 end";

    // Eager pair at the top of the method: the arms carry their own
    // requires on a lazy tree, and on an eager one routes.rb already
    // did it. Line-filtered so re-running is a no-op.
    if content.contains("require_relative \"../app/controllers/") {
        let filtered: Vec<&str> = content
            .lines()
            .filter(|l| !l.trim_start().starts_with("require_relative \"../app/controllers/"))
            .collect();
        *content = filtered.join("\n");
        content.push('\n');
    }

    let Some(start) = content.find(HEAD) else { return };
    let Some(rel_end) = content[start..].find(TAIL) else { return };
    let end = start + rel_end + TAIL.len();
    content.replace_range(start..end, generated);
}

fn apply_controller_dispatch(files: &mut [(String, String)], app: &App, lazy_requires: bool) {
    use std::fmt::Write;
    // The `else` arm is the Active Storage engine's three controllers
    // (`runtime/active_storage_disk.rb`), mounted beside the app's
    // table in `Main.route_table`; a symbol no app arm names is one
    // of theirs.
    const HARDCODED: &str = "  def self.instantiate_controller(sym)\n    case sym\n    when :articles then ArticlesController.new\n    when :comments then CommentsController.new\n    else ActiveStorage::Routes.instantiate_controller(sym)\n    end\n  end";

    let flat = crate::lower::flatten_routes(app);
    let mut seen = std::collections::HashSet::new();
    let current_classes = &app.current_attribute_classes;
    let mut arms = String::new();
    // Same rule as the routes.rb require header: a route may name a
    // controller the app never defines (campfire's `resource :settings`
    // under `scope module: "rooms"`). Rails resolves lazily and fails
    // only on request; an eager `require_relative` for a missing file
    // takes the whole process down at boot.
    let defined: std::collections::HashSet<&str> =
        app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    for r in &flat {
        let class = r.controller.0.as_str();
        if lazy_requires && !defined.contains(class) {
            continue;
        }
        let sym = crate::lower::routes_to_library::controller_symbol(class);
        if !seen.insert(sym.clone()) {
            continue;
        }
        if lazy_requires {
            // `underscore`, not `snake_case`: the latter passes `::`
            // straight through, so a namespaced controller required
            // `app/controllers/accounts::users_controller` while
            // `emit_lowered_controllers` had written it to
            // `app/controllers/accounts/users_controller.rb`. Every
            // namespaced route raised LoadError at dispatch — ~20 of
            // campfire's — and only at dispatch, so a boot probe that
            // touched top-level controllers saw nothing wrong. The
            // spinel lane, which resolves requires at BUILD time,
            // is what surfaced it.
            //
            // Memoized per arm: `require_relative` of an already-loaded
            // file still resolves its path and searches $LOADED_FEATURES,
            // on every request (1% of a small route). The ivar lives on
            // Main (the method is `def self.`), so each controller's file
            // is required once and laziness is kept.
            let stem = crate::naming::underscore(class);
            writeln!(
                arms,
                "    when :{sym} then @__ctl_{sym} ||= require_relative(\"app/controllers/{stem}\") || true; {class}.new"
            )
            .unwrap();
        } else {
            writeln!(arms, "    when :{sym} then {class}.new").unwrap();
        }
    }
    if lazy_requires {
        apply_controller_paths(files, &flat);
    }
    if arms.is_empty() {
        return;
    }
    let generated = format!(
        "  def self.instantiate_controller(sym)\n    case sym\n{arms}    else ActiveStorage::Routes.instantiate_controller(sym)\n    end\n  end"
    );

    // The test harness dispatches controller tests through its own copy
    // of the same table, hard-coded to the blog. campfire's spliced
    // `SessionTestHelper#sign_in` POSTs to `session_url`, so every
    // controller test reached it and died at
    // `cannot load such file -- app/controllers/articles_controller`.
    // Generated from the same route data as main.rb's, one line below —
    // the two were never meant to be different tables.
    // Same arms, re-indented for the harness's method body and with the
    // require path walked up one level (`test/` → tree root).
    let test_arms = arms
        .replace("app/controllers/", "../app/controllers/")
        .replace("    when ", "                 when ");
    let generated_test_case = format!(
        "    controller = case matched.controller\n{test_arms}                 else ActiveStorage::Routes.instantiate_controller(matched.controller)\n                 end"
    );

    for (path, content) in files.iter_mut() {
        if path.ends_with("main.rb") && content.contains(HARDCODED) {
            *content = content.replace(HARDCODED, &generated);
        }
        if path.ends_with("test/test_helper.rb") {
            patch_harness_dispatch(content, &generated_test_case);
        }
        // Per-request reset for the app's `ActiveSupport::CurrentAttributes`
        // subclass. `Current.instance` memoizes on the CLASS, so without
        // this a long-running process carries one request's `Current.user`
        // into the next — and an UNAUTHENTICATED request would read the
        // previous visitor's user, which is an auth bypass rather than a
        // staleness bug. Rails resets it per request through an executor
        // hook the emitted trees have no equivalent of.
        //
        // BOTH DISPATCHERS, because there are two. The test harness parks
        // the same `ActionController::Current` pair as main.rb and then
        // calls `process_action` the same way, so a controller test
        // inherits the PREVIOUS request's `Current.user` — which is how
        // campfire's `users_controller_test` saw `get join_url(code)`
        // answer 302 to root: an earlier `sign_in` in the same file left
        // a user parked, `redirect_signed_in_user_to_root` believed it,
        // and the unauthenticated join page was never reachable. Rails'
        // integration tests get the reset from the executor in the
        // middleware stack every `get`/`post` runs through.
        if path.ends_with("main.rb") || path.ends_with("test/test_helper.rb") {
            for class in current_classes {
                let marker = "    ActionController::Current.controller = controller";
                let with_reset =
                    format!("{marker}\n    {}.reset", class.0.as_str());
                if content.contains(marker) && !content.contains(&with_reset) {
                    *content = content.replace(marker, &with_reset);
                }
            }
        }
        // Drop the eager `require_relative "../app/controllers/<x>"` header
        // from routes.rb — controllers now load lazily via
        // instantiate_controller. Routing itself only needs the route
        // table (data), not the controller classes. Lazy trees only: the
        // spinel tree's whole-graph compile reaches controllers through
        // this header.
        if lazy_requires && path.ends_with("config/routes.rb") {
            *content = content
                .lines()
                .filter(|l| !l.trim_start().starts_with("require_relative \"../app/controllers/"))
                .collect::<Vec<_>>()
                .join("\n");
            content.push('\n');
        }
    }
}

/// Add `RouteTable::CONTROLLER_PATHS` to the routes.rb of a lazy tree.
/// It maps the router symbol of a namespaced controller to the Rails
/// controller path (`admin_posts: "admin/posts"`). The ruby overlay's
/// `recognize_path` reads it. A top-level controller has no row, and
/// `recognize_path` gives its router symbol. The path comes from the
/// class name, so it differs from Rails for an acronym inflection
/// (`admin/apikeys`, not `admin/api_keys`) or a digit after an
/// underscore. Two controllers with the same router symbol share a row.
fn apply_controller_paths(files: &mut [(String, String)], flat: &[crate::lower::FlatRoute]) {
    let mut rows = std::collections::BTreeMap::new();
    for r in flat {
        let class = r.controller.0.as_str();
        if class.contains("::") {
            let base = class.strip_suffix("Controller").unwrap_or(class);
            let sym = crate::lower::routes_to_library::controller_symbol(class);
            rows.insert(sym, crate::naming::underscore(base));
        }
    }
    let rows: Vec<String> = rows.iter().map(|(sym, path)| format!("{sym}: {path:?}")).collect();
    let header = format!("module RouteTable\n  CONTROLLER_PATHS = {{ {} }}.freeze\n\n", rows.join(", "));
    for (path, content) in files.iter_mut() {
        if path == "config/routes.rb" && !content.contains("CONTROLLER_PATHS") {
            *content = content.replacen("module RouteTable\n", &header, 1);
        }
    }
}

/// Leave `RouteTable.root` out of the dispatch table when the app
/// declares no root.
///
/// `main.rb` and the test harness compose the table as
/// `[RouteTable.root] + RouteTable.table + …`, but the routes emit
/// defines `RouteTable.root` only for a route at `/` (`root "c#a"`).
/// An API app often has none, and then the spinel build refused the
/// call (`unsupported call … CallNode root`) and the ruby tree raised
/// NoMethodError on its first request (#165). Without a root the table
/// starts at `RouteTable.table`, which is what the Crystal, TypeScript
/// and Python mains already do on the same test.
///
/// Runs wherever `apply_controller_dispatch` does, for the same reason:
/// the CRuby/JRuby trees replace the base's main.rb with the
/// ruby_overlay one at dedupe, so the second pass rewrites that one.
/// The harness it already rewrote has nothing left to match. Exact
/// paths, not `ends_with`: an app's own `app/models/domain.rb` ends in
/// `main.rb` too.
fn apply_route_table_root(files: &mut [(String, String)], app: &App) {
    const WITH_ROOT: &str = "[RouteTable.root] + RouteTable.table";
    let has_root = crate::lower::flatten_routes(app).iter().any(|r| r.path == "/");
    if has_root {
        return;
    }
    for (path, content) in files.iter_mut() {
        if path == "main.rb" || path == "test/test_helper.rb" {
            *content = content.replace(WITH_ROOT, "RouteTable.table");
        }
    }
}

/// The scaffold targets (spinel/ruby/jruby) ship the scaffold's
/// comprehensive README as `SPECIMEN.md`, freeing `README.md` for the
/// generated machine-runnable quick-start (`target_readme` via
/// `ensure_readme`) that the smoke contract executes. matz's primary
/// extraction surface is preserved as `SPECIMEN.md` in the spinel archive.
/// Called once, from `spinel_files` (the shared base for all three).
fn scaffold_readme_to_specimen(files: &mut [(String, String)]) {
    for (path, _) in files.iter_mut() {
        if path == "README.md" {
            *path = "SPECIMEN.md".to_string();
        }
    }
}


/// The spinel file set BEFORE `spin_shape` re-points it at the spin
/// package layout — i.e. the tree `make spinel-test` drives, with the
/// scaffold Makefile's own `--rbs sig` rules intact.
///
/// Exists for `tests/spinel_toolchain.rs`, which used to assemble this
/// tree by hand-copying an enumerated list of runtime files. That list
/// was the FIFTH copy of "which runtime files does a spinel tree need"
/// and its own comment said so; it drifted, and the miss surfaced as
/// `spinel: main.rb: cannot load such file` rather than as anything a
/// unit test could see. A toolchain test should drive what ships.
pub fn spinel_base_files(app: &App, fixture: &Path) -> Result<Vec<(String, String)>, String> {
    // The bundled-library requires belong HERE, not only in
    // `spin_shape`. This is the tree `tests/spinel_toolchain.rs`
    // compiles, and without them it compiles something the CLI never
    // ships: a `Pathname.new` in the test harness built clean through
    // `--target spinel` and failed the toolchain lane with "Pathname is
    // provided by the bundled pathname library, which this program does
    // not require".
    //
    // Exactly the bug `write_bundled_requires`' own doc describes — the
    // table lived inside `spin_shape` and the ruby family silently never
    // got it — one layer down. A lane is evidence only if it runs the
    // same code. Idempotent: the gap scan skips a file that already
    // requires the library, so `spin_shape` running it again is inert.
    let (mut files, _) = spinel_files_with_source_markers(app, fixture)?;
    spinel_relation_model_handle(&mut files)?;

    write_bundled_requires(&mut files);
    Ok(files)
}

/// Spinel-target files: lowered emit (app/, config/, test/) plus
/// scaffold + runtime overlays. Order matches `make spinel-transpile`
/// — scaffold first, runtime test/lib next, lowered emit on top.
/// `dedupe_last_wins` resolves overlap (e.g. emit_spinel's
/// `test/test_helper.rb` supersedes the scaffold's canonical version).
///
/// The source app's `app/javascript/` (the importmap JS entry +
/// Stimulus controllers) and `public/` icons are walked in verbatim:
/// `make assets` copies them under `static/assets/`, and the spinel
/// binary's `Main.dispatch` serves them at `/assets/*`. Binary files
/// (e.g. `icon.png`) are silently skipped — the archive is text-only.
/// Ledger every keyword parameter reaching a target that cannot
/// express one.
///
/// The ruby family renders `def f(code:, upcase: false)` verbatim, and
/// the call site beside it passes them by name, so the two agree.
/// Everywhere else the def renders positionally while the call renders
/// a hash, and nothing in the emitted tree says so.
/// A positional rest (`def f(*args)`, and the anonymous `def f(*)`)
/// is carried by the ruby family alone. Every other emitter renders it
/// as ONE plain parameter (`Param::rest`'s doc), so `f()` and
/// `f(a, b)` stop matching the declaration and `f(a)` hands the body
/// `a` where Ruby hands it `[a]` — an arity and value change that
/// `check` cannot see. Until a target maps it onto its own variadics,
/// say so instead of emitting the narrowed signature.
///
/// Full `...` forwarding and keyword rests are reported by the
/// forwarding gate beside the call; a `**rest` that ingest flattened
/// (`from_kwrest`) is a keyword bundle, not a positional rest.
fn report_positional_rest_param(method: &crate::dialect::MethodDef, target: BuildTarget) {
    if matches!(
        target,
        BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda
    ) {
        return;
    }
    let Some(param) = method
        .params
        .iter()
        .find(|p| p.rest && !p.keyword && !p.forwarding && !p.from_kwrest)
    else {
        return;
    };
    let name = if param.name.as_str().is_empty() || param.name.as_str().starts_with("__anon_rest") {
        "*".to_string()
    } else {
        format!("*{}", param.name.as_str())
    };
    crate::emit::diagnostics::report_unsupported(
        method.name_span,
        target.as_str(),
        "rest parameter",
        format!(
            "`{name}` on `{}` — this target renders a rest parameter as one positional, \
             which changes the method's arity",
            method.name.as_str()
        ),
    );
}

fn report_keyword_params(app: &App, target: &str) {
    // Read from the controllers rather than from `library_classes`:
    // the lowered helper is built inside each target's emit and never
    // stored on the App, so a walk over the stored classes finds
    // nothing. `kw_params` is the ingest's own record and is here.
    for controller in &app.controllers {
        for action in controller.actions() {
            for (name, _) in &action.kw_params {
                crate::emit::diagnostics::report_unsupported(
                    action.name_span,
                    target,
                    "keyword parameter",
                    format!(
                        "`{}` on `{}#{}` — this target renders parameters positionally, \
                         and the call site renders the keywords as a hash",
                        name.as_str(),
                        controller.name.0.as_str(),
                        action.name.as_str()
                    ),
                );
            }
        }
    }
}

/// Exception classes defined in `runtime/ruby/`
/// (`action_controller/parameter_missing.rb`,
/// `action_view/missing_template.rb`) that only the ruby family and
/// Spinel ship. Strict targets must ledger a typed constant read; keep
/// this list as the single name source for the gate and its tests.
pub const RUBY_FAMILY_RUNTIME_CONSTANTS: &[&str] = &[
    "ActionController::ParameterMissing",
    "ActionController::UnpermittedParameters",
    "ActionController::UnknownFormat",
    "ActionController::BadRequest",
    "ActionController::InvalidAuthenticityToken",
    "ActionController::RoutingError",
    "AbstractController::ActionNotFound",
    "ActionView::MissingTemplate",
];

/// Availability policy for class/module *values* that inference
/// resolves but the target does not ship. Returns the diagnostic
/// construct when the name is unavailable on `target`.
///
/// Two policies share one visitor walk and the app-defined exemptions:
/// Ruby/Spinel bundled library objects (`bundled_constant`), and
/// ruby-family runtime exception stubs (`ruby_family_runtime_constant`).
fn unavailable_class_module_construct(name: &str, target: &str) -> Option<&'static str> {
    // Spinel supplies the other bundled and ruby-family class values,
    // but recognizes GeneratorError only as a literal rescue name.
    if target == "spinel" {
        return if name == "JSON::GeneratorError" {
            Some("bundled_constant")
        } else {
            None
        };
    }
    if RUBY_FAMILY_RUNTIME_CONSTANTS.iter().any(|n| *n == name) {
        // JRuby ships the same runtime files as CRuby.
        return if target == "jruby" {
            None
        } else {
            Some("ruby_family_runtime_constant")
        };
    }
    let bundled = matches!(
        name,
        "URI::HTTP" | "URI::HTTPS" | "URI::Error" | "URI::InvalidURIError" | "Net::OpenTimeout" | "Net::ReadTimeout"
        | "SocketError"
        | "Net::HTTPRedirection" | "Net::HTTPOK" | "StringIO" | "OpenSSL::OpenSSLError"
        | "Rails::HTML5::SafeListSanitizer" | "JSON" | "JSON::ParserError"
        | "Struct" | "Mutex" | "Queue" | "SizedQueue"
        | "Thread::Queue" | "Thread::SizedQueue" | "Thread::Mutex"
        | "ThreadError" | "ClosedQueueError" | "Comparable" | "Enumerable"
        | "JSON::GeneratorError"
        // Ported for spinel (`runtime/ruby/rack_utils.rb`), the gem's own on
        // the ruby family; no strict target loads either.
        | "Rack::Utils"
        // `runtime/ruby/active_support_ext.rb` ships to the ruby family
        // and spinel only.
        | "ActiveSupport::JSON"
        // The sqlite3 gem on the ruby family, `runtime/spinel/
        // sqlite3_database.rb` over the FFI on spinel; `FileUtils` is a
        // default gem / `packages/fileutils`; the Rails classes come
        // with `runtime/spinel/active_record_db_config.rb`.
        | "SQLite3::Database" | "SQLite3::Exception" | "SQLite3::CantOpenException"
        | "SQLite3::BusyException" | "SQLite3::SQLException"
        | "FileUtils" | "ActiveRecord::ConnectionAdapters::SQLite3Adapter"
        // runtime/spinel/active_support_cache.rb.
        | "ActiveSupport::Cache" | "ActiveSupport::Cache::MemoryStore"
        // rqrcode_core's / spinel-rqrcode's.
        | "RQRCodeCore::QRCodeRunTimeError" | "RQRCodeCore::QRCodeArgumentError");
    if !bundled {
        return None;
    }
    // Nokogiri does not supply HTML5 on JRuby, and the sqlite3 gem does
    // not run there. The other bundled values remain available.
    if target == "jruby" && name != "Rails::HTML5::SafeListSanitizer" && !name.starts_with("SQLite3::") {
        return None;
    }
    Some("bundled_constant")
}

/// Class methods of a stdlib class whose shared port (`runtime/ruby/`)
/// stops short of them: Ruby's own library serves them on the ruby
/// family and spinel's package on spinel, while a strict target's port
/// has no body to call. `Zlib.gzip` needs a deflate the CRC-32 port
/// (`runtime/ruby/zlib.rb`) does not have. A Rails class method served
/// only by a ruby-family/spinel runtime file is gated the same way.
const RUBY_SPINEL_ONLY_METHODS: &[(&str, &str)] = &[
    ("Zlib", "gzip"),
    // Answered from the Db shim (`runtime/spinel/
    // active_record_db_config.rb`); a strict target's Db glue has none.
    ("ActiveRecord::Base", "connection_db_config"),
    ("ActiveRecord::Base", "connection_pool"),
    // `lower::record_snapshot`'s closed-world `constantize` — the
    // generated `case` lives in the same ruby-family/spinel file.
    ("ActiveRecord::Base", "instantiate_named"),
];

/// Instance methods only the ruby family and spinel can answer: a model's
/// `attributes_before_type_cast` is a Hash of every column's raw stored
/// value, one value type per column, which a strict target's Hash cannot
/// hold.
const RUBY_SPINEL_ONLY_INSTANCE_METHODS: &[&str] = &["attributes_before_type_cast"];

/// True only for an application model instance (including nullable model
/// unions) that does not define its own method with this spelling.
fn is_unported_model_instance_method(
    recv: &crate::expr::Expr,
    method: &str,
    app: &App,
) -> bool {
    fn model_ids(ty: &crate::ty::Ty, out: &mut Vec<crate::ident::ClassId>) {
        match ty {
            crate::ty::Ty::Class { id, .. } => out.push(id.clone()),
            crate::ty::Ty::Union { variants } => {
                for variant in variants {
                    model_ids(variant, out);
                }
            }
            _ => {}
        }
    }
    if matches!(&*recv.node, crate::expr::ExprNode::Const { .. }) {
        return false;
    }
    let Some(ty) = &recv.ty else { return false };
    let mut ids = Vec::new();
    model_ids(ty, &mut ids);
    ids.iter().any(|id| {
        app.models.iter().any(|model| model.name == *id)
            && !crate::analyze::source_instance_method(app, id, method)
        })
}

/// True when the app already defines `id` as a class/module value, so
/// the availability gate must not ledger it as a missing runtime stub.
fn app_defines_class(app: &App, id: &crate::ident::ClassId) -> bool {
    app.library_classes.iter().any(|class| class.name == *id)
        || app.models.iter().any(|model| model.name == *id)
        || app.controllers.iter().any(|controller| controller.name == *id)
        || app.rails_application.as_ref().is_some_and(|class| class.name == *id)
        || app.test_modules.iter().any(|module| {
            module.inner_classes.iter().any(|class| class.name == *id)
        })
}

/// Ledger a Const / superclass name that is unavailable on `target`
/// (bundled library object or ruby-family runtime exception stub).
fn report_unavailable_class_value(
    app: &App,
    target: &str,
    name: &str,
    span: crate::span::Span,
) {
    if span.is_synthetic() {
        return;
    }
    let id = crate::ident::ClassId(crate::ident::Symbol::from(name));
    if let Some(construct) = unavailable_class_module_construct(name, target)
        && !app_defines_class(app, &id)
    {
        emit::diagnostics::report_unsupported(
            span,
            target,
            construct,
            format!("{name} is not available as a class/module value on {target}"),
        );
    }
}

/// Most of these class objects are supplied by Ruby/Spinel's bundled
/// libraries, not by the transpiled runtimes. The others are exception
/// classes that only the ruby-family runtime defines. Recognizing them
/// during inference must not turn a missing target implementation into
/// a clean emit. Also ledgers superclass Consts (via `parent_span`) and
/// lowers-added exception Consts surveyed from a throwaway controller
/// lower (emit still lowers controllers after this gate today).
fn report_unsupported_bundled_constants(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Roda) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, app: &App, target: &str) {
        // Spinel matches this exception by its rescue name, but does not
        // expose its class as a constant. Keep the value guard below while
        // permitting the supported literal rescue clause, including ::JSON.
        if target == "spinel" {
            if let crate::expr::ExprNode::BeginRescue { rescues, .. } = &*expr.node {
                expr.node.for_each_child(&mut |child| {
                    let named_rescue = rescues.iter().any(|rescue| rescue.classes.iter().any(|class| {
                        std::ptr::eq(child, class)
                            && matches!(&*class.node, crate::expr::ExprNode::Const { path }
                                if path.iter().map(|part| part.as_str()).filter(|part| !part.is_empty())
                                    .eq(["JSON", "GeneratorError"]))
                    }));
                    if !named_rescue {
                        visit(child, app, target);
                    }
                });
                return;
            }
        }
        if matches!(&*expr.node, crate::expr::ExprNode::Const { .. }) {
            if let Some(crate::ty::Ty::Class { id, .. }) = &expr.ty {
                report_unavailable_class_value(app, target, id.0.as_str(), expr.span);
            }
        }
        if let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && target != "spinel"
            && target != "jruby"
            && !expr.span.is_synthetic()
            && RUBY_SPINEL_ONLY_INSTANCE_METHODS.contains(&method.as_str())
            && is_unported_model_instance_method(recv, method.as_str(), app)
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "bundled_method",
                format!("#{method} is only available on the ruby family and spinel"),
            );
        }
        if let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && target != "spinel"
            && target != "jruby"
            && !expr.span.is_synthetic()
            && let crate::expr::ExprNode::Const { path } = &*recv.node
        {
            let owner = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
            if RUBY_SPINEL_ONLY_METHODS.contains(&(owner.as_str(), method.as_str())) {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "bundled_method",
                    format!("{owner}.{method} is only available on the ruby family and spinel"),
                );
            }
        }
        // A mapped JSON call does not emit a Ruby module object. Skip
        // only that exact receiver, not its arguments (which can still
        // contain unsupported class values) or unmapped method calls.
        if let crate::expr::ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node {
            if matches!(&*recv.node, crate::expr::ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "JSON")
                && args.len() == 1
                && (method.as_str() == "generate"
                    || method.as_str() == "parse" && matches!(target, "typescript" | "typescript-worker" | "python" | "crystal")
                    || method.as_str() == "dump" && matches!(target, "rust" | "elixir")
                    || matches!(method.as_str(), "fast_generate" | "pretty_generate") && target == "rust")
            {
                expr.node.for_each_child(&mut |child| {
                    if !std::ptr::eq(child, recv) {
                        visit(child, app, target);
                    }
                });
                return;
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, app, target));
    }
    let mut visit = |expr: &crate::expr::Expr| visit(expr, app, target.as_str());
    crate::lower::for_each_hook_body_ref(app, &mut visit);
    // Controller `render` → `MissingTemplate` rewriting lives in
    // `controller_to_library`, which runs at emit time after this gate.
    // Survey a throwaway lower so lowers-added exception Consts (now
    // typed) are still ledgered on strict targets (Thomas 2B).
    let lowered_controllers =
        crate::lower::controller_to_library::lower_controllers_with_arel_and_views(
            &app.controllers,
            Vec::new(),
            Some(&app.schema),
            &app.views,
        );
    for class in &lowered_controllers {
        for method in &class.methods {
            visit(&method.body);
            for param in &method.params {
                if let Some(default) = &param.default {
                    visit(default);
                }
            }
        }
    }
    // Like the Date gate, include roots outside the app-body survey.
    for controller in &app.controllers {
        for action in controller.actions() {
            for (_, default) in &action.kw_params {
                if let Some(default) = default {
                    visit(default);
                }
            }
        }
    }
    for fixture in &app.fixtures {
        for expr in &fixture.preamble {
            visit(expr);
        }
        for value in fixture.records.values().flat_map(|record| record.values()) {
            if let crate::dialect::FixtureValue::Ruby(expr) = value {
                visit(expr);
            }
        }
    }
    for helper in &app.routes.direct_helpers {
        visit(&helper.body);
    }
    for function in &app.sql_functions {
        let methods = match &function.kind {
            crate::app::SqlFunctionKind::Scalar { method } => [Some(method), None],
            crate::app::SqlFunctionKind::Aggregate { step, finalize } => [Some(step), Some(finalize)],
        };
        for method in methods.into_iter().flatten() {
            visit(&method.body);
            for default in method.params.iter().filter_map(|param| param.default.as_ref()) {
                visit(default);
            }
        }
    }
    for view in &app.views {
        visit(&view.body);
        for param in view.strict_locals.iter().flatten() {
            if let Some(default) = &param.default {
                visit(default);
            }
        }
    }
    for module in &app.test_modules {
        if let Some(setup) = &module.setup {
            visit(setup);
        }
        for test in &module.tests {
            visit(&test.body);
        }
        for helper in &module.helpers {
            visit(&helper.body);
            for param in &helper.params {
                if let Some(default) = &param.default {
                    visit(default);
                }
            }
        }
        for (_, value) in &module.constants {
            visit(value);
        }
        for class in &module.inner_classes {
            for method in &class.methods {
                visit(&method.body);
                for param in &method.params {
                    if let Some(default) = &param.default {
                        visit(default);
                    }
                }
            }
            for (_, value) in &class.constants {
                visit(value);
            }
            for call in &class.unknown_calls {
                visit(call);
            }
        }
    }
    // The immutable app-body survey also excludes association extensions.
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Association {
                assoc: crate::dialect::Association::HasMany { extension, .. }, ..
            } = item {
                for method in extension {
                    visit(&method.body);
                    for param in &method.params {
                        if let Some(default) = &param.default {
                            visit(default);
                        }
                    }
                }
            }
        }
    }

    // Superclass Consts are ClassIds with a captured parent_span, not
    // Exprs in a method body — visit them explicitly so
    // `class X < ActionController::RoutingError` is ledgered.
    let target = target.as_str();
    for model in &app.models {
        if let Some(parent) = &model.parent {
            report_unavailable_class_value(app, target, parent.0.as_str(), model.parent_span);
        }
    }
    for controller in &app.controllers {
        if let Some(parent) = &controller.parent {
            report_unavailable_class_value(
                app,
                target,
                parent.0.as_str(),
                controller.parent_span,
            );
        }
        for sibling in &controller.sibling_classes {
            report_unavailable_class_value(
                app,
                target,
                sibling.parent.as_str(),
                sibling.parent_span,
            );
        }
    }
    for class in app.library_classes.iter().chain(app.rails_application.iter()) {
        if let Some(parent) = &class.parent {
            report_unavailable_class_value(app, target, parent.0.as_str(), class.parent_span);
        }
    }
    for module in &app.test_modules {
        for class in &module.inner_classes {
            if let Some(parent) = &class.parent {
                report_unavailable_class_value(
                    app,
                    target,
                    parent.0.as_str(),
                    class.parent_span,
                );
            }
        }
    }
}

// Return app-emitted test stems separately: merging the scaffold loses
// their provenance, and the Ruby-family Makefile must exclude runtime tests.
/// The spinel tree with `#<SPINEL_SOURCE>` markers naming the app
/// `.rb`/`.erb` each emitted line came from (spinel#7630), so `#line`,
/// debug stepping and perf report app positions rather than lowered
/// Ruby. Spinel-only: the ruby family shares `spinel_files` and keeps
/// its output unmarked. See `emit::ruby::source_markers`.
fn spinel_files_with_source_markers(
    app: &App,
    fixture: &Path,
) -> Result<(Vec<(String, String)>, Vec<String>), String> {
    let (mut files, stems) =
        emit::ruby::source_markers::with_source_markers(app, || spinel_files(app, fixture))?;
    for (path, content) in files.iter_mut() {
        if path.ends_with(".rb") {
            *content = emit::ruby::source_markers::finish(content);
        }
    }
    Ok((files, stems))
}

fn spinel_files(app: &App, fixture: &Path) -> Result<(Vec<(String, String)>, Vec<String>), String> {
    let mut files: Vec<(String, String)> = Vec::new();

    crate::runtime_files::walk_into("runtime/spinel/scaffold", "", &mut files)?;

    crate::runtime_files::walk_partitioned("runtime/spinel/test",
        "test/",
        "sig/test/",
        &mut files,
    )?;

    crate::runtime_files::walk_flat("runtime/spinel", &["rb"], "runtime/", &mut files)?;

    let needs_date = app_uses_date(app);

    // Temporal-intrinsics sidecar — the flat walk above picks only .rb,
    // and spinel's strict unresolved-call gate needs `parse_db_time`'s
    // `String?` param typed to compile the nil-guard narrow.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/active_support_time_parsing.rbs")
            .map_err(|e| format!("read runtime/spinel/active_support_time_parsing.rbs: {e}"))?;
        files.push((
            "sig/runtime/active_support_time_parsing.rbs".to_string(),
            rbs,
        ));
    }

    // Program-defined Date package for apps that use date-only values.
    // Loading Date#strftime into every Spinel tree breaks poly
    // Time|Date receivers (matz/spinel#7334). Default boot has no Date
    // requires; when needed we inject the package (class + date parse/
    // format + date JSON rewrite) after always-on AR serialization.
    const DATE_PACKAGE_FILES: &[&str] = &[
        "runtime/date.rb",
        "runtime/active_support_date_parsing.rb",
        "runtime/active_record_date_serialization.rb",
        "sig/runtime/date.rbs",
        "sig/runtime/active_support_date_parsing.rbs",
        "sig/runtime/active_record_date_serialization.rbs",
    ];
    if needs_date {
        for (src, dest) in [
            ("runtime/spinel/date.rbs", "sig/runtime/date.rbs"),
            (
                "runtime/spinel/active_support_date_parsing.rbs",
                "sig/runtime/active_support_date_parsing.rbs",
            ),
            (
                "runtime/spinel/active_record_date_serialization.rbs",
                "sig/runtime/active_record_date_serialization.rbs",
            ),
        ] {
            let rbs = crate::runtime_files::read_to_string(src)
                .map_err(|e| format!("read {src}: {e}"))?;
            files.push((dest.to_string(), rbs));
        }
        if let Some((_, boot)) = files.iter_mut().find(|(p, _)| p == "boot.rb") {
            inject_date_package_into_boot(boot)?;
        }
    } else {
        files.retain(|(p, _)| !DATE_PACKAGE_FILES.contains(&p.as_str()));
    }

    // Always-on AR JSON entrypoint signatures (date rewrite RBS is gated above).
    {
        let rbs = crate::runtime_files::read_to_string(
            "runtime/spinel/active_record_serialization.rbs",
        )
        .map_err(|e| format!("read runtime/spinel/active_record_serialization.rbs: {e}"))?;
        files.push(("sig/runtime/active_record_serialization.rbs".to_string(), rbs));
    }

    // Schema-less json/jsonb column seam. The flat walk emits the Ruby
    // implementation; this sidecar preserves its gradual logical value
    // while keeping the database slot String-typed.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/json_column.rbs")
            .map_err(|e| format!("read runtime/spinel/json_column.rbs: {e}"))?;
        files.push(("sig/runtime/json_column.rbs".to_string(), rbs));
    }

    // Duration sidecar — pins @seconds Integer so ago/from_now stay
    // Time-typed under AOT inference (an untyped @seconds widens the
    // temporal arithmetic to poly against the Time-typed C return).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/active_support_duration.rbs")
            .map_err(|e| format!("read runtime/spinel/active_support_duration.rbs: {e}"))?;
        files.push(("sig/runtime/active_support_duration.rbs".to_string(), rbs));
    }

    // CGI sidecar — the flat walk emits runtime/cgi_spinel.rb but only
    // .rb, so pair its typing contract here. Since spinel's bundled
    // `cgi` package landed (matz/spinel#4812) the file is a REOPEN and
    // the contract is one method: `parse -> Hash[String, Array[String]]`.
    // The escapes are the package's own Ruby and are inferred from it.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/cgi_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/cgi_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/cgi_spinel.rbs".to_string(), rbs));
    }

    // `HttpStub` sidecar — the WebMock double's contract; see the file
    // for why `headers` has to be DECLARED.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/http_stub.rbs")
            .map_err(|e| format!("read runtime/spinel/http_stub.rbs: {e}"))?;
        files.push(("sig/runtime/http_stub.rbs".to_string(), rbs));
    }

    // `MochaStub` sidecar — the app-method stub slot `lower::mocha` writes
    // into lowered test bodies and model guards; the flat walk emits the
    // .rb, and the .rbs is what types the slot on the model
    // (`@__mocha_<m>: MochaStub?`).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/mocha_stub.rbs")
            .map_err(|e| format!("read runtime/spinel/mocha_stub.rbs: {e}"))?;
        files.push(("sig/runtime/mocha_stub.rbs".to_string(), rbs));
    }

    // `TcpSocketStub` sidecar — the `TCPSocket.open` expectation table
    // the lowered DNS-rebinding tests fill and the reopened client asks;
    // the .rbs types the predicate array from its contract, not from a
    // test's first push.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/tcp_socket_stub.rbs")
            .map_err(|e| format!("read runtime/spinel/tcp_socket_stub.rbs: {e}"))?;
        files.push(("sig/runtime/tcp_socket_stub.rbs".to_string(), rbs));
    }

    // ERB::Util shim sidecar — same story as CGI: spinel has no stdlib
    // ERB, the flat walk emits runtime/erb_spinel.rb, and the .rbs pins
    // html_escape's String return so callers concatenating it stay typed.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/erb_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/erb_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/erb_spinel.rbs".to_string(), rbs));
    }

    // Record-equality sidecar — the reopen in
    // runtime/active_record_equality_spinel.rb gives ActiveRecord::Base
    // Rails' `==`; the .rbs declares it so a typed comparison binds.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/active_record_equality_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/active_record_equality_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/active_record_equality_spinel.rbs".to_string(), rbs));
    }

    // RecordIdentifier sidecar — runtime/record_identifier_spinel.rb
    // names `ActionView::RecordIdentifier.dom_id`; the .rbs carries the
    // shared `dom_id`'s contract so the record argument stays typed.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/record_identifier_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/record_identifier_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/record_identifier_spinel.rbs".to_string(), rbs));
    }

    // `redirect_back_or_to` sidecar — the ActionController::Base reopen
    // in runtime/redirect_back.rb (ruby family only).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/redirect_back.rbs")
            .map_err(|e| format!("read runtime/spinel/redirect_back.rbs: {e}"))?;
        files.push(("sig/runtime/redirect_back.rbs".to_string(), rbs));
    }

    // `Rails::HTML5::SafeListSanitizer` sidecar — runtime/
    // rails_html_sanitizer_spinel.rb, which spinel's boot.rb alone loads.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/rails_html_sanitizer_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/rails_html_sanitizer_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/rails_html_sanitizer_spinel.rbs".to_string(), rbs));
    }

    // Forgery-check sidecar — the ActionController::Base reopen in
    // runtime/request_forgery_protection.rb (ruby family only).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/request_forgery_protection.rbs")
            .map_err(|e| format!("read runtime/spinel/request_forgery_protection.rbs: {e}"))?;
        files.push(("sig/runtime/request_forgery_protection.rbs".to_string(), rbs));
    }

    // Rails 8.1.4 ActionController::Head's options-hash implementation
    // is only emitted for the Ruby family and Spinel, not strict targets.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/action_controller_head.rbs")
            .map_err(|e| format!("read runtime/spinel/action_controller_head.rbs: {e}"))?;
        files.push(("sig/runtime/action_controller_head.rbs".to_string(), rbs));
    }

    // HTTP Token/Basic auth sidecar — the ActionController::Base reopen in
    // runtime/http_authentication.rb (ruby family only). It types the
    // block parameters the helpers yield, which the app's blocks compare.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/http_authentication.rbs")
            .map_err(|e| format!("read runtime/spinel/http_authentication.rbs: {e}"))?;
        files.push(("sig/runtime/http_authentication.rbs".to_string(), rbs));
    }

    // Secret sidecar — `LocalSecret.resolve` in runtime/local_secret.rb,
    // the generated-and-kept key both boots park when SECRET_KEY_BASE is
    // unset (ruby family only).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/local_secret.rbs")
            .map_err(|e| format!("read runtime/spinel/local_secret.rbs: {e}"))?;
        files.push(("sig/runtime/local_secret.rbs".to_string(), rbs));
    }

    // Signed-cookie sidecar — ActionDispatch::SignedCookie and the
    // Session reopen in runtime/signed_cookies.rb (ruby family only).
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/signed_cookies.rbs")
            .map_err(|e| format!("read runtime/spinel/signed_cookies.rbs: {e}"))?;
        files.push(("sig/runtime/signed_cookies.rbs".to_string(), rbs));
    }

    // Hash#to_query nesting sidecar — the reopen in
    // runtime/hash_to_query.rb renders a nested Hash/Array value as
    // Rails' bracket grammar; the .rbs keeps the reopened method's
    // signature the shared one's.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/hash_to_query.rbs")
            .map_err(|e| format!("read runtime/spinel/hash_to_query.rbs: {e}"))?;
        files.push(("sig/runtime/hash_to_query.rbs".to_string(), rbs));
    }

    // Array attribute-value sidecar — the reopen in
    // runtime/attr_value_text.rb renders an Array value as Rails does
    // (space-joined; the `class:` conditional list); the .rbs keeps the
    // reopened method's signature the shared one's.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/attr_value_text.rbs")
            .map_err(|e| format!("read runtime/spinel/attr_value_text.rbs: {e}"))?;
        files.push(("sig/runtime/attr_value_text.rbs".to_string(), rbs));
    }

    // SecureRandom stub-slot sidecar — the reopen in
    // runtime/secure_random_stub.rb replaces the package's
    // `alphanumeric`/`uuid` with stub-first arms; the .rbs pins the
    // slot setters the helper and the lowered tests call.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/secure_random_stub.rbs")
            .map_err(|e| format!("read runtime/spinel/secure_random_stub.rbs: {e}"))?;
        files.push(("sig/runtime/secure_random_stub.rbs".to_string(), rbs));
    }

    // Nokogiri read-path sidecar — the reopen in runtime/nokogiri_spinel.rb
    // adds `xpath`/`key?`/`load` over the façade's classes; the .rbs pins
    // `xpath -> Array[Element]` so the app's `.map do |tag| … end` over
    // it is typed rather than poly.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/nokogiri_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/nokogiri_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/nokogiri_spinel.rbs".to_string(), rbs));
    }

    // Multipart + Active Storage disk sidecars — the ruby family's
    // `ActionDispatch::Http::UploadedFile` / `Multipart` and the
    // reopen of the shared Active Storage contract with a disk service
    // and the engine's routes; the .rbs pin the file's bytes and the
    // controllers' shapes.
    for stem in ["multipart", "active_storage_disk"] {
        let path = format!("runtime/spinel/{stem}.rbs");
        let rbs = crate::runtime_files::read_to_string(&path)?;
        files.push((format!("sig/runtime/{stem}.rbs"), rbs));
    }

    // View buffer capacity memo — returning wrappers call
    // ViewBufferCap.alloc/store (lower::view_buffer_passing). Spinel
    // stub + CRuby overlay share this contract.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/view_buffer_cap.rbs")
            .map_err(|e| format!("read runtime/spinel/view_buffer_cap.rbs: {e}"))?;
        files.push(("sig/runtime/view_buffer_cap.rbs".to_string(), rbs));
    }

    // `db_jruby.rb` is the JRuby/JDBC Db backend — it uses Java interop
    // (`java_import`, `Java::`) that the CRuby and Spinel toolchains (and
    // the spinel-subset compliance gate) must never see. It is injected
    // only by `jruby_runtime_files`, so keep it out of the shared base.
    files.retain(|(p, _)| p != "runtime/db_jruby.rb");

    // Same story for `markly_jruby.rb` — the JRuby implementation of the
    // markly contract over commonmark-java (Java interop; conformance
    // vectors at bench/gem-shims/markly/). Injected only by
    // `jruby_runtime_files`.
    files.retain(|(p, _)| p != "runtime/markly_jruby.rb");

    // And for `module_delegate.rb` — ActiveSupport's `Module#delegate`,
    // supplied for the GEMS in the emitted Gemfile (see the file's own
    // header). A reopen of `Module` that defines methods from a computed
    // name is exactly what the strict targets cannot compile, and
    // nothing in an emitted tree's own code needs it: an app's
    // `delegate` is lowered at ingest. Injected only by
    // `ruby_runtime_files` / `jruby_runtime_files`.
    files.retain(|(p, _)| p != "runtime/module_delegate.rb");

    // The HTTP/WebSocket server (runtime/spinel/tep, over spinel's
    // sp_net; no C of its own) plus its NOTICE. Recursive walk picks
    // the whole subtree.
    crate::runtime_files::walk_into("runtime/spinel/tep", "runtime/tep/", &mut files)?;

    for sub in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
    ] {
        crate::runtime_files::walk_partitioned(
            &format!("runtime/ruby/{sub}"),
            &format!("runtime/{sub}/"),
            &format!("sig/runtime/{sub}/"),
            &mut files,
        )?;
    }
    widen_key_contract(app, &mut files)?;
    // Relation Base→untyped rewrite is Spinel-only (see
    // `spinel_relation_model_handle` call sites). `ruby_family_runtime_files`
    // shares this tree and must keep Base for CRuby/JRuby Bar A/B.
    for stem in [
        "rails",
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
        "action_mailer",
        "active_job",
        "gem_facades",
        "bcrypt_facade",
        "rqrcode_facade",
        // Nokogiri's raising stand-in, its own file for the same swap:
        // spin_shape replaces it with `require "nokogiri"` (spinel-nokogiri)
        // when the app names Nokogiri.
        "nokogiri_facade",
        // OpenTelemetry's non-recording-span stub — never swapped for a
        // real gem (see the file's own header); still its own file, same
        // whole-file-grain reasoning. `gem_facades.rb`'s own
        // `require_relative` anchors it for Spinel; the CRuby/JRuby
        // rewrite below (which replaces that file's content outright)
        // carries its own matching `require_relative` so it is never
        // dropped there.
        "open_telemetry_facade",
        "inflector",
        "inflector_ext",
        "json_builder",
        // Pure string ActiveSupport helpers for the Ruby/Spinel
        // scaffold (strict targets literalize controller_name/path).
        // Listed before active_support_ext so require_relative resolves.
        "active_support_inflections",
        "active_support_number_helper",
        "active_support_number_helper_mixin",
        "action_view_number_helper",
        "action_view_number_helper_mixin",
        "active_support_ext",
        "hash_deep_merge",
        "security_utils",
        "params",
        "action_text",
        "active_storage",
        // actionpack's MIME registry, ported (see runtime/ruby/mime.rb).
        // Every target: nothing else in a tree defines `Mime`.
        "mime",
        // The stdlib class the strict targets have no stdlib for. The
        // ruby family reaches Ruby's own through a bare `require`, so
        // this one only has to exist where that does not.
        "ipaddr",
        // rack's `Utils.q_values` / `select_best_encoding`, ported for
        // spinel (no rack gem to load); the ruby family swaps it for
        // `require "rack/utils"` below, as ipaddr and zlib are swapped.
        "rack_utils",
        // Ruby's `Logger` plus the two ActiveSupport wrappers in front
        // of it. Same arrangement as ipaddr, with one difference the
        // file's header gives: NOT swapped for Ruby's own on the
        // CRuby/JRuby trees, because an app SUBCLASSES
        // `Logger::Formatter` and two definitions of that constant is a
        // superclass mismatch at load.
        "logger",
        // Ruby's `Tempfile.create` block form. Same arrangement as
        // ipaddr and zlib, swap included: the CRuby/JRuby trees take
        // Ruby's own (below), and the targets with no stdlib take
        // this. campfire's vips policy test writes each probe image to
        // one, because `vips_foreign_find_load` takes a path.
        "tempfile",
        // The one method of Ruby's resolver `surfguard` calls. Swapped
        // for `require "resolv"` on the CRuby/JRuby trees below, same
        // as ipaddr — and it has to be, or the app's own
        // `Resolv.stubs(:getaddresses)` lands on the wrong class.
        "resolv",
        // basecamp/surfguard's SSRF address policy, ported over the
        // IPAddr above. Not swapped for the real gem on the CRuby/JRuby
        // trees the way ipaddr and zlib are: surfguard is a GIT gem, so
        // there is no `gem install` to swap TO. One implementation,
        // every target, ours.
        "surfguard",
        // `useragent` 0.16.11 + `platform_agent` 1.0.1, ported. NOT
        // swapped for the real gems on the CRuby/JRuby trees the way
        // ipaddr and zlib are: `ApplicationPlatform` subclasses
        // `PlatformAgent`, so two definitions of the constant is a
        // superclass mismatch at load, not a fallback. One
        // implementation, every target, ours — and the suite beside the
        // port compares it against the real gems.
        "user_agent",
        // `typeid` 0.2.2, ported for `TypeID.new(prefix)` — lobsters'
        // Token concern mints every record's token with it. Swapped for
        // the gem on the CRuby/JRuby trees below: the gem's `TypeID` is
        // a String subclass the port cannot be, and the tree that has
        // the gem should run it.
        "typeid",
        // `Zlib.crc32`, same arrangement as ipaddr: ported for the
        // targets with no zlib to bind to, swapped for Ruby's own on
        // the CRuby/JRuby trees below.
        "zlib",
        // `Timeout.timeout` / `Timeout::Error` — Campfire unfurl + video
        // previewer capture. Port for Spinel; CRuby/JRuby swap to the
        // default gem below. BUNDLED also lists Timeout → "timeout".
        "timeout",
        // `I18n.locale` / `default_locale` — the locale campfire folds
        // into its cache keys. Rails' default, `:en`; setting one is
        // not modeled (see the file's header).
        "i18n_locale",
    ] {
        let rb = format!("runtime/ruby/{stem}.rb");
        let content = crate::runtime_files::read_to_string(&rb)?;
        files.push((format!("runtime/{stem}.rb"), content));
        let rbs = format!("runtime/ruby/{stem}.rbs");
        if crate::runtime_files::exists(&rbs) {
            files.push((
                format!("sig/runtime/{stem}.rbs"),
                crate::runtime_files::read_to_string(&rbs)?,
            ));
        }
    }

    // `Tempfile`: spinel has the library now (matz/spinel#4811, merged
    // 2026-09-22 as be8d7db1), so every tree built from here takes it —
    // the spinel lanes reach `packages/tempfile` and the ruby family's
    // own swap below restates the same require against Ruby's stdlib.
    // The port at `runtime/ruby/tempfile.rb` stays, and keeps its typing
    // gate, for the strict targets that still have no stdlib to bind to;
    // this is the three-branch shape `ipaddr` would have if there were a
    // `packages/ipaddr`.
    //
    // THE REASON IS THE ONE THE PORT'S OWN HEADER NAMES. Ruby's `create`
    // opens `O_EXCL` and retries, so it cannot be made to clobber a file
    // an attacker pre-created; the port opens by name.
    // `packages/tempfile` opens `"wx+"`, which IS that exclusive create,
    // so the lane that ships as a binary is no longer the one running
    // the weaker of the two.
    //
    // THE .rbs GOES WITH IT, and that is not tidiness: the port is a
    // `module Tempfile` and both libraries are a `class`, so a signature
    // left behind reopens a class as a module — the collision
    // `runtime/spinel/erb_spinel.rb`'s header records, which cost the
    // lobsters AOT lane ten days.
    //
    // `Zlib`: same three-branch swap, different reason. The CRC-32 port
    // stays for targets with no zlib; spinel's `packages/zlib` is the
    // real codec (gzip/deflate), which tep uses to honour
    // Accept-Encoding the way campfire's Rack::Deflater does on CRuby.
    // Without the swap, `Zlib.gzip` is a NameError and every HTML page
    // ships uncompressed. Its .rbs goes too: the port's surface is not
    // the library's.
    //
    // HERE rather than in `spin_shape`, because `spin_shape` is not the
    // only tree that ships: `spinel_base_files` is what
    // `tests/spinel_toolchain.rs` compiles, and the bundled-require
    // table's own comment records what it cost to have the two disagree.
    // A lane is evidence only if it runs the same code.
    files.retain(|(p, _)| p != "sig/runtime/tempfile.rbs" && p != "sig/runtime/zlib.rbs");
    for (path, content) in files.iter_mut() {
        if path == "runtime/tempfile.rb" {
            *content = "# The bundled tempfile library — see `project::spinel_files`.\n\
                        # The port at runtime/ruby/tempfile.rb exists for the strict\n\
                        # targets that have no stdlib to bind to, and opens by name\n\
                        # where this one opens O_EXCL.\n\
                        require \"tempfile\"\n"
                .to_string();
        }
        if path == "runtime/zlib.rb" {
            *content = "# The bundled zlib library — see `project::spinel_files`.\n\
                        # The port at runtime/ruby/zlib.rb is CRC-32 only; this one is\n\
                        # packages/zlib (gzip/deflate) so tep can honour Accept-Encoding.\n\
                        require \"zlib\"\n"
                .to_string();
        }
    }

    let app_files = emit::ruby::emit_spinel(app);
    let test_stems = app_test_stems(&app_files);
    files.extend(sort_files(app_files));

    // Emit the ingested support classes (extras/, lib/, app/helpers/,
    // app/mailers/, and non-AR classes under app/models/ — Markdowner,
    // TrafficHelper, StoriesPaginator, …) as `app/models/<stem>.rb`.
    // `emit_spinel` (the shared Rails-shape path) emits only the lowered
    // models/controllers/views; these `app.library_classes` are ingested
    // for analysis and *referenced* by emitted code (so a require is
    // generated) but were never produced, leaving the require graph
    // dangling. The bodies are source-shape (un-lowered) Ruby — faithful
    // under the Ruby→Ruby round-trip; under spinel AOT they are priced by
    // the strict whole-graph check, which is the point (spinel as
    // completeness oracle).
    files.extend(sort_files(emit::ruby::emit_library(app)));
    // Vendored extras whose bodies drive un-modeled stdlib (Sponge →
    // Net::HTTP/Resolv/IPAddr/OpenSSL) get raising façades at the same
    // require path — spinel AOT prices every reachable body and the
    // verbatim ones can't compile until the stdlib spin packages land.
    // The CRuby tree restores the verbatim emit (real stdlib there).
    emit::ruby::apply_extras_facades(&mut files, app);

    let js = fixture.join("app/javascript");
    if js.exists() {
        walk_dir_into(&js, "app/javascript/", &mut files)?;
    }
    // importmap-rails' OTHER source root, and one the blog fixture does
    // not have — which is why it went missing until an app that uses it
    // was emitted. campfire vendors trix, highlight.js, `@rails/request.js`
    // and twelve language grammars here and pins all sixteen; without this
    // walk `make assets` had nothing to copy them from and the composer's
    // imports never resolved.
    let vendor_js = fixture.join("vendor/javascript");
    if vendor_js.exists() {
        walk_dir_into(&vendor_js, "vendor/javascript/", &mut files)?;
    }
    // The app's own stylesheets — the two Propshaft roots `app.stylesheets`
    // is ingested from, so the files and the names the layout links are
    // read from the same place.
    //
    // These are TEXT, which is why they were missing: `collect_binary_assets`
    // carries `app/assets` but only what is not valid UTF-8, and its doc
    // comment names this exact gap ("a TEXT asset under these roots that no
    // emitter produces would still be dropped"). campfire is the app that
    // needed it — twenty-six hand-written stylesheets, none of which reached
    // the tree.
    // ...and `app/assets/images/`, for the same reason and with the same
    // blind spot: `collect_binary_assets` carries the PNGs and skips the
    // SVGs, because an SVG is text. campfire draws its entire interface
    // in SVG — eighty icons, every one of them a `/assets/*.svg` the page
    // requests and the tree did not have. `walk_dir_into` skips whatever
    // is not valid UTF-8, so the two mechanisms partition cleanly rather
    // than emitting a file twice.
    for root in [
        "app/assets/stylesheets",
        "app/assets/builds",
        "app/assets/images",
    ] {
        let dir = fixture.join(root);
        if dir.exists() {
            walk_dir_into(&dir, &format!("{root}/"), &mut files)?;
        }
    }
    let public = fixture.join("public");
    if public.exists() {
        walk_dir_into(&public, "public/", &mut files)?;
    }
    // `test/fixtures/files` — `file_fixture_path`, which `file_fixture`,
    // `fixture_file_upload` and an `ActiveStorage::FixtureSet.blob`
    // fixture row read. The same blind spot again: the binary files
    // arrive through `collect_binary_assets`, a `.txt` or `.csv` did not,
    // and a blob row naming one loaded nothing.
    let file_fixtures = fixture.join("test/fixtures/files");
    if file_fixtures.exists() {
        walk_dir_into(&file_fixtures, "test/fixtures/files/", &mut files)?;
    }

    let mut files = dedupe_last_wins(files);
    // De-blog the scaffold for whatever app was ingested: regenerate the
    // `app/views.rb` aggregator from the emitted view set and rewrite
    // `main.rb`'s `instantiate_controller` from the app's route table
    // (eager arms — AOT has no lazy-require escape hatch). Runs here, in
    // the shared base, so all three scaffold targets inherit it; the
    // CRuby/JRuby trees re-apply the dispatch in lazy flavor to the
    // ruby_overlay main.rb that supersedes this one.
    apply_controller_dispatch(&mut files, app, false);
    apply_route_table_root(&mut files, app);
    // Identity for the `/cable` handshake, in the shared base like the
    // dispatch above. Only the SPINEL tree runs the result: the
    // CRuby/JRuby trees carry this `runtime/cable.rb` too but never
    // require it — their config.ru requires the overlay's own top-level
    // `cable.rb`, which has its own identity path and its own test.
    apply_cable_connection(&mut files, app);
    apply_cable_channels(&mut files, app);
    apply_content_layout(&mut files, app);
    apply_content_helper_attributes(&mut files, app);
    // The `only:`-as-finder specializations, also in the shared base:
    // `runtime/global_id_locator.rb` is ONE file that both the spinel
    // tree and the ruby overlay require, and the rewrite that names
    // these entry points ran at lowering — before any target was
    // chosen. Generating on only one lane would leave the other calling
    // a method nothing defined.
    apply_global_id_locate(&mut files, app);
    apply_instantiate_named(&mut files, app);
    apply_attachable_locate(&mut files, app);
    apply_views_aggregator(&mut files);
    apply_models_aggregator(&mut files);
    apply_module_mixins(&mut files, app, MixinForm::Reopen);
    // All three scaffold targets (spinel + the ruby/jruby trees derived
    // from this set) ship the comprehensive scaffold README as SPECIMEN.md,
    // freeing README.md for the generated quick-start `ensure_readme`
    // injects. ruby_overlay carries no README, so this is the only place
    // the rename needs to happen for any of them.
    scaffold_readme_to_specimen(&mut files);
    apply_gemfile_trim(&mut files, app, fixture);
    // After the two JS walks above and after `dedupe_last_wins`: the
    // generator sorts each pin by which root actually holds its file, so
    // it has to read the FINAL file set, not the fixture.
    apply_makefile_asset_list(&mut files, app);
    apply_makefile_stylesheet_list(&mut files, app);
    apply_test_gem_wiring(&mut files);
    apply_spinel_sql_functions(&mut files, app)?;
    apply_pagination_demand(&mut files)?;
    Ok((files, test_stems))
}

/// geared_pagination's `set_page_and_extract_portion_from` sets `@page` on
/// the controller (runtime/ruby/action_controller/pagination.rb reopens
/// `ActionController::Base` to do it). An app that never calls it gets
/// neither the file nor its require: a controller that uses `@page` for
/// something else — lobsters' page NUMBER — would otherwise share the
/// ivar with the base's `Page`, which spinel refuses as a class-layout
/// conflict (and which a Rails app without the gem never has). campfire,
/// which paginates through it, keeps both.
fn apply_pagination_demand(files: &mut Vec<(String, String)>) -> Result<(), String> {
    let wanted = files.iter().any(|(p, c)| {
        (p.starts_with("app/") || p.starts_with("test/")) && p.ends_with(".rb") && c.contains("set_page_and_extract_portion_from")
    });
    if wanted {
        return Ok(());
    }
    const REQUIRE: &str = "require_relative \"action_controller/pagination\"\n";
    let ac = files
        .iter_mut()
        .find(|(p, _)| p == "runtime/action_controller.rb")
        .ok_or("spinel tree has no runtime/action_controller.rb")?;
    if !ac.1.contains(REQUIRE) {
        return Err("runtime/action_controller.rb: the pagination require moved".into());
    }
    ac.1 = ac.1.replacen(REQUIRE, "", 1);
    files.retain(|(p, _)| {
        p != "runtime/action_controller/pagination.rb"
            && p != "runtime/action_controller/pagination.rbs"
            && p != "sig/runtime/action_controller/pagination.rbs"
    });
    Ok(())
}

/// The app's initializer-registered SQL functions on the spinel tree:
/// `runtime/sql_functions.rb` (see `spinel_sql_functions_file`), required
/// by `runtime/db.rb` and installed on every connection the pool opens,
/// right after its pragmas — as Rails' adapter patch installs them on
/// every connection. The ruby family replaces the file with its own
/// (`ruby_family_runtime_files`). A no-op when the app registers none,
/// so the blog's db.rb is untouched.
fn apply_spinel_sql_functions(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    let Some(src) = spinel_sql_functions_file(app) else { return Ok(()) };
    const ANCHOR: &str = "      PRAGMAS.each { |p| SQL.sqlite3_exec(dbh, p, nil, nil, nil) }\n";
    let db = files
        .iter_mut()
        .find(|(p, _)| p == "runtime/db.rb")
        .ok_or("spinel tree has no runtime/db.rb to install SQL functions from")?;
    if !db.1.contains(ANCHOR) {
        return Err("runtime/db.rb: the connection-open anchor for SqlFunctions.install moved".into());
    }
    db.1 = format!(
        "require_relative \"sql_functions\"\n{}",
        db.1.replacen(ANCHOR, &format!("{ANCHOR}      SqlFunctions.install(dbh)\n"), 1)
    );
    files.push(("runtime/sql_functions.rb".to_string(), src));
    Ok(())
}

/// How a test body asks for a gem: by naming a constant, or by writing
/// one of its method spellings.
enum Marker {
    Constant(&'static str),
    AnyText(&'static [&'static str]),
}

/// `Mocha::API` mixed into TestBase, with the three lifecycle calls it
/// requires. `mocha/minitest` would do this itself; it also refuses to
/// load without Minitest, which the emitted helper deliberately does not
/// have.
///
/// `mocha_verify` in teardown is the half that makes an `expects`
/// assertion mean anything — without it an unmet expectation passes
/// silently, which is worse than not having the gem. `mocha_teardown`
/// runs in an `ensure` so a failed verify still unstubs; leaving a stub
/// installed would leak into the next test in the file.
///
/// Anchored on the two shapes the shared helper has always had, and
/// silent if either moves — the demand for mocha comes from the app, so
/// a tree that misses this patch fails loudly on the first `stubs` call
/// rather than passing with stubs that never took.
///
/// GUARDED ON `defined?(Mocha)`, because a STRICT target has no mocha at
/// all. Unguarded, the three lifecycle calls are the first thing every
/// test runs, so `undefined local variable or method 'mocha_setup'`
/// failed all six of campfire's `room_test` — none of which stubs
/// anything. The gem ceiling is real (mocha is a Ruby metaprogramming
/// library; there is nothing to compile against), but it belongs to the
/// tests that actually stub, not to every test in the tree.
///
/// The guard does NOT weaken the paragraph above: `mocha_verify` is
/// skipped only where mocha could not have installed an expectation in
/// the first place. A test that writes `stubs` on a target without the
/// gem still fails loudly, on that call, which is the honest place.
///
/// `with_mocha` is whether the tree demanded the gem: the `Mocha::API`
/// include and its three lifecycle calls go in only then (an app that
/// stubs HTTP but never mocks would otherwise `NameError` on the
/// include). The stub-slot clears — mocha's and webmock's — go in
/// whichever of the two was demanded, because the slots they clear are
/// defined by runtime files every helper requires, not by either gem.
fn patch_stub_lifecycle(helper: &mut String, with_mocha: bool) {
    const SETUP: &str = "  def setup\n    SchemaSetup.reset! if defined?(SchemaSetup)";
    const TEARDOWN: &str = "  def teardown\n  end";

    if helper.contains("Mocha::API") || helper.contains("HttpStub.clear") {
        return;
    }
    let clears = format!(
        "{}{}",
        crate::lower::mocha::stub_clear_lines("    "),
        crate::lower::webmock::stub_clear_lines("    "),
    );
    if let Some(at) = helper.find(SETUP) {
        let with_include = if with_mocha {
            format!(
                "  include Mocha::API\n\n  def setup\n    mocha_setup if defined?(Mocha)\n{clears}    SchemaSetup.reset! if defined?(SchemaSetup)",
            )
        } else {
            format!("  def setup\n{clears}    SchemaSetup.reset! if defined?(SchemaSetup)")
        };
        helper.replace_range(at..at + SETUP.len(), &with_include);
    }
    // The slots' own verifies run in teardown whether or not the gem is
    // there: a count filed by `expect_<m>(n)` and not met raises here,
    // which the autorun shim charges to the test that filed it — the
    // same moment, and the same failure, as `mocha_verify`.
    let verifies = crate::lower::mocha::stub_verify_lines("    ");
    if let Some(at) = helper.find(TEARDOWN) {
        let verified = if with_mocha {
            format!(
                "  def teardown\n{verifies}    mocha_verify if defined?(Mocha)\n  ensure\n    mocha_teardown if defined?(Mocha)\n  end"
            )
        } else {
            format!("  def teardown\n{verifies}  end")
        };
        helper.replace_range(at..at + TEARDOWN.len(), &verified);
    }

    // LAST, and that ordering is load-bearing. This inserts near the TOP
    // of the file, which shifts every offset after it — doing it between
    // `find(SETUP)` and the matching `replace_range` spliced the
    // replacement at a stale index and produced
    // `SchemaSetup.reset! if defined?(SchemaSetup)Setup.reset! ...`,
    // a syntax error that took every campfire test file down at once.
    //
    // The slots the clear calls reach have to be DEFINED in every test
    // file, not just the ones that stub — see `lower::mocha::stub_requires`
    // for what guarding on `defined?` cost instead.
    let requires = format!(
        "{}{}",
        crate::lower::mocha::stub_requires(),
        crate::lower::webmock::stub_requires(),
    );
    if !requires.is_empty() {
        const BOOT: &str = "require_relative \"../boot\"\n";
        if let Some(b) = helper.find(BOOT) {
            helper.insert_str(b + BOOT.len(), &requires);
        }
    }
}

/// `WebMock::API` mixed into TestBase.
///
/// `webmock/minitest` includes it into `Minitest::Test`, which this
/// helper deliberately is not — the same reason mocha's lifecycle is
/// wired by hand right above. The module's constant-rooted calls
/// (`WebMock.stub_request`) resolve without it; its BARE matchers do
/// not, and campfire's `webhook_test` writes one:
///
/// ```text
/// WebMock.stub_request(:post, url).with(body: hash_including(...))
/// ```
///
/// Anchored on the class line rather than on setup/teardown, because
/// this adds no lifecycle — and silent if that line moves, for the same
/// reason the mocha patch is: the demand comes from the app, so a tree
/// that misses this fails loudly on the first bare matcher.
fn patch_webmock_api_include(helper: &mut String) {
    const ANCHOR: &str = "  include Mocha::API\n";
    const ALT: &str = "  include ActionDispatch::TestProcess\n";

    if helper.contains("include WebMock::API") {
        return;
    }
    let at = match helper.find(ANCHOR) {
        Some(i) => i + ANCHOR.len(),
        None => match helper.find(ALT) {
            Some(i) => i + ALT.len(),
            None => return,
        },
    };
    helper.insert_str(at, "  include WebMock::API\n");
}

/// Test-only gems the APP's own suite reaches for, which our emitted
/// `test/test_helper.rb` drops by construction: that file is our shim,
/// not the app's, so every `require` the app's helper made is gone.
/// campfire's opens with `require "mocha/minitest"` and
/// `require "webmock/minitest"`, and declares both in its Gemfile's
/// `group :test`.
///
/// Detected from the emitted TEST TREE rather than from the app's
/// Gemfile: a gem listed there but never reached is a dependency we
/// would be inventing a need for, and the constant in a test body is
/// the demand itself. The require path is the gem's own documented
/// Minitest entry point — a two-column table, not a derivation.
///
/// Ruby-family only, and honestly so: this hands the emitted tree a
/// real CRuby gem that intercepts `Net::HTTP`. A strict target needs
/// the same behaviour built at its own transport seam; nothing here
/// pretends otherwise. Same seam the tree already uses for sqlite3 and
/// bcrypt.
fn apply_test_gem_wiring(files: &mut Vec<(String, String)>) {
    // (marker a test body writes, gem, the require that gives it)
    //
    // WebMock is named as a CONSTANT (`WebMock.stub_request`), so the
    // same scan the bundled-library table uses finds it. Mocha never
    // appears by name at all — a test writes `Resolv.stubs(:getaddress)`
    // or `Webhook.any_instance` — so its demand is a METHOD, and the
    // marker is the call spelling.
    //
    // `mocha/api`, NOT `mocha/minitest`: the latter raises "Minitest must
    // be loaded *before* `require 'mocha/minitest'`" and our emitted
    // test/test_helper.rb is deliberately Minitest-free. `mocha/api` is
    // the documented entry point for a foreign test framework, and it
    // needs the lifecycle wiring below.
    //
    // `HttpStub.` beside `WebMock`: `lower::webmock` has already rewritten
    // every understood `stub_request` chain to the slot by the time this
    // scan runs, so on the ruby family the slot's delegate is what reaches
    // the gem — and that is the demand.
    const TEST_GEMS: [(Marker, &str, &str); 3] = [
        (Marker::AnyText(&["WebMock.", "WebMock::", "HttpStub."]), "webmock", "webmock/minitest"),
        // `MochaBridge.` is the lowering's own spelling of a chain it hands
        // back to the gem (`lower::mocha`), and the paren-less forms are
        // campfire's `@membership.user.expects :reset_remote_connections`.
        (
            Marker::AnyText(&[".stubs(", ".expects(", ".stubs ", ".expects ", ".any_instance", "MochaBridge."]),
            "mocha",
            "mocha/api",
        ),
        // ruby-vips, named as `::Vips::Image` — campfire's logo and
        // avatar tests decode the response body to assert its PIXEL
        // dimensions, which is the only honest way to check that an
        // image endpoint served an image of the right size.
        (Marker::Constant("Vips"), "ruby-vips", "vips"),
    ];

    let mut needed: Vec<(&str, &str)> = Vec::new();
    for (marker, gem, entry) in TEST_GEMS {
        let demanded = files.iter().any(|(p, c)| {
            p.starts_with("test/")
                && p.ends_with(".rb")
                // …but NOT our own shim. `test/test_helper.rb` is the
                // emitted helper, not a test body, so a gem name in it is
                // never the app asking for the gem — it is us reaching
                // for one we have already decided to wire. Scanning it
                // makes the wiring self-fulfilling: the helper's
                // `WebMock.reset! if defined?(WebMock)` demanded webmock
                // for every app, and the blog fixture (which has no such
                // gem) stopped loading at all.
                && p != "test/test_helper.rb"
                && match marker {
                    Marker::Constant(konst) => names_constant(c, konst),
                    Marker::AnyText(needles) => needles.iter().any(|n| c.contains(n)),
                }
        });
        if demanded {
            needed.push((gem, entry));
        }
    }
    if needed.is_empty() {
        return;
    }
    // One require, in the helper every test file loads — the place the
    // app put it.
    if let Some((_, helper)) = files.iter_mut().find(|(p, _)| p == "test/test_helper.rb") {
        for (gem, entry) in &needed {
            let line = format!("require {entry:?}");
            if helper.contains(&line) {
                continue;
            }
            // WebMock AFTER the app has loaded, as Rails' test_helper
            // orders it (`config/environment`, then `webmock/minitest`):
            // the gem swaps `Net::HTTP` for its own subclass while it is
            // enabled, so an app class that subclasses `Net::HTTP` at
            // load (campfire's `WebPush::Connections::HTTP`) would
            // otherwise inherit WebMock's, and once a test disables
            // WebMock to reach a real server its `start` breaks.
            const BOOT: &str = "require_relative \"../boot\"\n";
            match helper.find(BOOT) {
                Some(at) if *gem == "webmock" => helper.insert_str(at + BOOT.len(), &format!("{line}\n")),
                _ => helper.insert_str(0, &format!("{line}\n")),
            }
        }
        let with_mocha = needed.iter().any(|(gem, _)| *gem == "mocha");
        let with_webmock = needed.iter().any(|(gem, _)| *gem == "webmock");
        if with_mocha || with_webmock {
            patch_stub_lifecycle(helper, with_mocha);
        }
        if with_webmock {
            patch_webmock_api_include(helper);
        }
    }
    // Declared as well as required: a tree whose tests load a gem its
    // Gemfile does not name is a tree that only runs where the gem
    // happens to be installed, which is exactly how three ambient
    // dependencies hid until campfire's suite met a clean runner.
    if let Some((_, gemfile)) = files.iter_mut().find(|(p, _)| p == "Gemfile") {
        let mut block = String::from(
            "\n# Test-only gems the app's own suite reaches for. The emitted\n\
             # test/test_helper.rb is our shim rather than the app's, so the\n\
             # requires its helper made are re-added by\n\
             # `project.rs::apply_test_gem_wiring` — declared here so the\n\
             # tree runs where the gems are NOT already installed.\n\
             group :test do\n",
        );
        for (gem, _) in &needed {
            if gemfile.contains(&format!("gem {gem:?}")) {
                continue;
            }
            block.push_str(&format!("  gem {gem:?}\n"));
        }
        block.push_str("end\n");
        if block.contains("  gem ") {
            if !gemfile.ends_with('\n') {
                gemfile.push('\n');
            }
            gemfile.push_str(&block);
        }
    }
}

/// The RUNTIME twin of `apply_test_gem_wiring`, and it exists because
/// `runtime/gem_facades.rb` swallows the failure.
///
/// That file guarded-requires the CRuby-path gems — `require gem_name`
/// inside `rescue LoadError; nil` — so an app whose Gemfile does not name
/// one does not fail at boot. It fails at REQUEST TIME, as an undefined
/// constant, in whatever code path first reaches the gem. campfire's is
/// sign-in: `POST /session` answered 500 with `uninitialized constant
/// User::BCrypt`, every authenticated page redirected, and nothing in the
/// tree said a gem was missing.
///
/// The rescue is right — the blog uses none of these and must boot without
/// them installed — so the fix is to DECLARE what this app reaches, which
/// is the same argument `apply_test_gem_wiring` already makes one function
/// up: a tree that only runs where a gem happens to be installed is a tree
/// whose dependencies are ambient. CI installing bcrypt is what kept this
/// hidden; a clean `bundle install && puma` is what found it.
///
/// Detected from `app/`, not from the app's own Gemfile, for the reason
/// the test table gives: a gem listed there but never reached would be a
/// dependency we invented. The constant in an emitted body IS the demand.
///
/// Ruby-family only, and wired at the CRuby/JRuby forks rather than in
/// `spinel_files`, beside `apply_makefile_test_list_stems` for the reason that
/// function's own note gives: the shared scaffold set feeds spinel too,
/// and a spinel tree that declares nokogiri in a Gemfile its toolchain
/// lane has to `bundle install` is a build break in a target that never
/// wanted the gem. The spinel target answers the same demand at its own
/// seam — `spin_shape` swaps `runtime/bcrypt_facade.rb` for the real spin
/// package and adds it to the manifest. This is the CRuby half of that
/// same decision, which had no half until now.
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
const GEM_REQUIRES: &[&str] = &[
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
];

/// The guarded-require block, minus any gem this tree provides another
/// way. Guarded because an app that uses none of them — the blog — must
/// boot with none installed.
///
/// `exclude` takes gem NAMES, not a substring to cut out of rendered
/// source: JRuby provides Markly through the commonmark-java shim, and
/// a string surgery that silently missed would put the real gem (which
/// has no JRuby build) back in the list.
/// `runtime/sql_functions.rb` for the CRuby tree: the app's SQL
/// functions (`App::sql_functions`) as `SqlFunctions` class methods,
/// plus the `install(db)` the gem-backed `Db` calls on every pooled
/// connection. `None` when the app registers none.
///
/// The bodies are the app's, emitted from IR like any other method; the
/// `install` glue is the sqlite3 gem's registration API, which is why
/// this file is CRuby's alone. The `fn` each body takes is the gem's
/// function proxy (`result=`, and `[]`/`[]=` for an aggregate's state),
/// the object the app's own blocks were handed. JRuby's JDBC driver
/// and spinel's FFI binding register functions differently and do not
/// install these yet (docs/pipeline/runtime.md).
fn cruby_sql_functions_file(app: &App) -> Option<String> {
    use crate::app::SqlFunctionKind;
    if app.sql_functions.is_empty() {
        return None;
    }
    let indent = |s: String| {
        s.lines()
            .map(|l| if l.is_empty() { String::new() } else { format!("  {l}") })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut out = String::from(
        "# SQL functions the app's initializers register on every SQLite\n\
         # connection (generated from App::sql_functions — see\n\
         # src/ingest/sql_functions.rs). Installed by Db.open_pool.\n\
         module SqlFunctions\n",
    );
    let mut install = String::from("  def self.install(db)\n");
    for f in &app.sql_functions {
        let args: Vec<String> = (0..f.arity).map(|i| format!("a{i}")).collect();
        let bargs = std::iter::once("fn".to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(", ");
        match &f.kind {
            SqlFunctionKind::Scalar { method } => {
                out.push_str(&indent(crate::emit::ruby::emit_method(method)));
                out.push_str("\n\n");
                install.push_str(&format!(
                    "    db.create_function({:?}, {}) {{ |{bargs}| SqlFunctions.{}({bargs}) }}\n",
                    f.name, f.arity, method.name.as_str()
                ));
            }
            SqlFunctionKind::Aggregate { step, finalize } => {
                out.push_str(&indent(crate::emit::ruby::emit_method(step)));
                out.push_str("\n\n");
                out.push_str(&indent(crate::emit::ruby::emit_method(finalize)));
                out.push_str("\n\n");
                install.push_str(&format!(
                    "    db.create_aggregate({:?}, {}) do\n      step {{ |{bargs}| SqlFunctions.{}({bargs}) }}\n      finalize {{ |fn| SqlFunctions.{}(fn) }}\n    end\n",
                    f.name, f.arity, step.name.as_str(), finalize.name.as_str()
                ));
            }
        }
    }
    install.push_str("  end\n");
    out.push_str(&install);
    out.push_str("end\n");
    Some(out)
}

/// The FFI half of `runtime/sql_functions.rb` on the spinel tree, which
/// has no sqlite3 gem to hand the app's blocks to: registration through
/// `sqlite3_create_function_v2`, reading the `sqlite3_value` arguments,
/// and the `fn` context object the app's bodies were written against
/// (`fn.result = …`, an aggregate's `fn[:n]`).
///
/// Two spinel shapes, both measured on 234125c5 / fe8ebb81: an
/// `ffi_callback` argument cannot be `nil`, and a callback's trampoline
/// is typed over `const void *`. So registration goes through a small
/// `ffi_source` adapter that fills the unused slots and casts. An
/// aggregate's state lives in a Ruby Hash keyed by the address
/// `sqlite3_aggregate_context` returns — one per GROUP, freed when the
/// group finalizes — behind a Mutex, because spinel's OS workers share
/// the process.
const SPINEL_SQL_FUNCTIONS_FFI: &str = r##"module SqlFunctionsFFI
  ffi_lib "sqlite3"
  ffi_callback :rh_sqlfn, [:ptr, :int, :ptr], :void
  ffi_callback :rh_sqlfin, [:ptr], :void
  ffi_source <<~C
    #include <sqlite3.h>
    #include <stdint.h>
    int rh_sql_scalar(void *db, const char *n, int a, void (*f)(const void *, int, const void *)) {
      return sqlite3_create_function_v2((sqlite3 *)db, n, a, SQLITE_UTF8, 0,
        (void (*)(sqlite3_context *, int, sqlite3_value **))f, 0, 0, 0);
    }
    int rh_sql_agg(void *db, const char *n, int a, void (*s)(const void *, int, const void *), void (*fin)(const void *)) {
      return sqlite3_create_function_v2((sqlite3 *)db, n, a, SQLITE_UTF8, 0, 0,
        (void (*)(sqlite3_context *, int, sqlite3_value **))s, (void (*)(sqlite3_context *))fin, 0);
    }
    intptr_t rh_sql_agg_key(void *ctx) { return (intptr_t)sqlite3_aggregate_context((sqlite3_context *)ctx, 8); }
    void *rh_sql_argv(void *argv, int i) { return ((sqlite3_value **)argv)[i]; }
    void rh_sql_result_text(void *ctx, const char *s) { sqlite3_result_text((sqlite3_context *)ctx, s, -1, SQLITE_TRANSIENT); }
  C
  ffi_func :rh_sql_scalar, [:ptr, :str, :int, :rh_sqlfn], :int
  ffi_func :rh_sql_agg, [:ptr, :str, :int, :rh_sqlfn, :rh_sqlfin], :int
  ffi_func :rh_sql_agg_key, [:ptr], :long
  ffi_func :rh_sql_argv, [:ptr, :int], :ptr
  ffi_func :rh_sql_result_text, [:ptr, :str], :void
  ffi_func :sqlite3_value_type, [:ptr], :int
  ffi_func :sqlite3_value_int64, [:ptr], :long
  ffi_func :sqlite3_value_double, [:ptr], :double
  ffi_func :sqlite3_value_text, [:ptr], :str
  ffi_func :sqlite3_result_int64, [:ptr, :long], :void
  ffi_func :sqlite3_result_double, [:ptr, :double], :void
  ffi_func :sqlite3_result_null, [:ptr], :void
end

# The context object SQLite hands a function block (the sqlite3 gem's
# `fn`): `result=` answers the call, `[]` / `[]=` hold an aggregate's
# per-group state.
class SqlFnContext
  STATE = {}
  LOCK = Mutex.new

  def initialize(ctx, key)
    @ctx = ctx
    @key = key
  end

  def result=(v)
    if v.nil?
      SqlFunctionsFFI.sqlite3_result_null(@ctx)
    elsif v == true
      SqlFunctionsFFI.sqlite3_result_int64(@ctx, 1)
    elsif v == false
      SqlFunctionsFFI.sqlite3_result_int64(@ctx, 0)
    elsif v.is_a?(Integer)
      SqlFunctionsFFI.sqlite3_result_int64(@ctx, v)
    elsif v.is_a?(Float)
      SqlFunctionsFFI.sqlite3_result_double(@ctx, v)
    else
      SqlFunctionsFFI.rh_sql_result_text(@ctx, v.to_s)
    end
    v
  end

  def [](k)
    v = nil
    LOCK.synchronize do
      h = STATE[@key]
      v = h[k] unless h.nil?
    end
    v
  end

  def []=(k, v)
    LOCK.synchronize do
      h = STATE[@key]
      if h.nil?
        h = {}
        STATE[@key] = h
      end
      h[k] = v
    end
    v
  end

  # An aggregate group is done: drop its state.
  def release
    LOCK.synchronize { STATE.delete(@key) }
    nil
  end

  # Argument `i` as the Ruby value the sqlite3 gem would hand the block.
  def self.arg(argv, i)
    v = SqlFunctionsFFI.rh_sql_argv(argv, i)
    t = SqlFunctionsFFI.sqlite3_value_type(v)
    return SqlFunctionsFFI.sqlite3_value_int64(v) if t == 1
    return SqlFunctionsFFI.sqlite3_value_double(v) if t == 2
    return nil if t == 5
    SqlFunctionsFFI.sqlite3_value_text(v)
  end
end
"##;

/// `runtime/sql_functions.rb` for the spinel tree — the counterpart of
/// `cruby_sql_functions_file`. The app's function bodies are the same
/// `SqlFunctions` methods (see src/ingest/sql_functions.rs); what differs
/// is the install: an `ffi_callback` trampoline per function, each
/// building the `fn` context and reading the arguments, registered on
/// every pooled connection by `Db` (the spinel db.rb calls `install`
/// after its pragmas). `None` when the app registers none.
fn spinel_sql_functions_file(app: &App) -> Option<String> {
    use crate::app::SqlFunctionKind;
    if app.sql_functions.is_empty() {
        return None;
    }
    let indent = |s: String| {
        s.lines()
            .map(|l| if l.is_empty() { String::new() } else { format!("  {l}") })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let emit = crate::emit::ruby::emit_method;
    let mut out = String::from(
        "# SQL functions the app's initializers register on every SQLite\n\
         # connection (generated from App::sql_functions — see\n\
         # src/ingest/sql_functions.rs and project::spinel_sql_functions_file).\n\
         # Installed by Db on each pooled connection.\n",
    );
    out.push_str(SPINEL_SQL_FUNCTIONS_FFI);
    out.push_str("\nmodule SqlFunctions\n");
    let mut install = String::from("  def self.install(db)\n");
    let mut trampolines = String::new();
    for f in &app.sql_functions {
        let args = (0..f.arity).map(|i| format!("SqlFnContext.arg(argv, {i})")).collect::<Vec<_>>();
        let tramp_args = |fn_expr: &str| {
            std::iter::once(fn_expr.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(", ")
        };
        let stem = f.name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
        match &f.kind {
            SqlFunctionKind::Scalar { method } => {
                out.push_str(&indent(emit(method)));
                out.push_str("\n\n");
                install.push_str(&format!(
                    "    SqlFunctionsFFI.rh_sql_scalar(db, {:?}, {}, method(:rh_sqlfn_{stem}))\n",
                    f.name, f.arity
                ));
                trampolines.push_str(&format!(
                    "\ndef rh_sqlfn_{stem}(ctx, argc, argv)\n  SqlFunctions.{}({})\n  nil\nend\n",
                    method.name.as_str(),
                    tramp_args("SqlFnContext.new(ctx, 0)")
                ));
            }
            SqlFunctionKind::Aggregate { step, finalize } => {
                out.push_str(&indent(emit(step)));
                out.push_str("\n\n");
                out.push_str(&indent(emit(finalize)));
                out.push_str("\n\n");
                install.push_str(&format!(
                    "    SqlFunctionsFFI.rh_sql_agg(db, {:?}, {}, method(:rh_sqlfn_{stem}_step), method(:rh_sqlfn_{stem}_final))\n",
                    f.name, f.arity
                ));
                trampolines.push_str(&format!(
                    "\ndef rh_sqlfn_{stem}_step(ctx, argc, argv)\n  SqlFunctions.{}({})\n  nil\nend\n",
                    step.name.as_str(),
                    tramp_args("SqlFnContext.new(ctx, SqlFunctionsFFI.rh_sql_agg_key(ctx))")
                ));
                trampolines.push_str(&format!(
                    "\ndef rh_sqlfn_{stem}_final(ctx)\n  fn = SqlFnContext.new(ctx, SqlFunctionsFFI.rh_sql_agg_key(ctx))\n  SqlFunctions.{}(fn)\n  fn.release\n  nil\nend\n",
                    finalize.name.as_str()
                ));
            }
        }
    }
    install.push_str("    nil\n  end\n");
    out.push_str(&install);
    out.push_str("end\n");
    out.push_str(&trampolines);
    Some(out)
}

fn gem_require_block(exclude: &[&str]) -> String {
    let names: Vec<String> = GEM_REQUIRES
        .iter()
        .filter(|g| !exclude.contains(g))
        .map(|g| format!("{g:?}"))
        .collect();
    let mut out = format!("[{}].each do |gem_name|\n", names.join(", "));
    out.push_str("  begin\n    require gem_name\n  rescue LoadError\n    nil\n  end\nend\n");
    out
}

fn apply_runtime_gem_wiring(files: &mut Vec<(String, String)>) {
    // Kept in step with the guarded list in `runtime/gem_facades.rb`:
    // (constant an emitted body names, gem that defines it). Only gems
    // whose absence is a RUNTIME error belong here — the list is the
    // façade's, not a survey of what an app might like.
    const RUNTIME_GEMS: [(Marker, &str); 16] = [
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
    ];

    let mut needed: Vec<&str> = Vec::new();
    for (marker, gem) in RUNTIME_GEMS {
        // `app/` only. The runtime tree names several of these itself (the
        // façade lists all nine), and a scan that saw those would declare
        // every gem for every app — the same self-fulfilling wiring the
        // test table had to exclude `test_helper.rb` to avoid.
        let demanded = files.iter().any(|(p, c)| {
            p.starts_with("app/")
                && p.ends_with(".rb")
                && match marker {
                    Marker::Constant(konst) => names_constant(c, konst),
                    Marker::AnyText(needles) => needles.iter().any(|n| c.contains(n)),
                }
        });
        if demanded {
            needed.push(gem);
        }
    }
    if needed.is_empty() {
        return;
    }
    if let Some((_, gemfile)) = files.iter_mut().find(|(p, _)| p == "Gemfile") {
        let mut block = String::from(
            "\n# Gems the emitted app reaches at RUNTIME. runtime/gem_facades.rb\n\
             # guarded-requires these, so a missing one is not a boot failure —\n\
             # it is an undefined constant on the first request that needs it.\n\
             # Declared by `project.rs::apply_runtime_gem_wiring` from the\n\
             # constants app/ actually names.\n",
        );
        for gem in &needed {
            if gemfile.contains(&format!("gem {gem:?}")) {
                continue;
            }
            block.push_str(&format!("gem {gem:?}\n"));
        }
        if block.contains("gem \"") {
            if !gemfile.ends_with('\n') {
                gemfile.push('\n');
            }
            gemfile.push_str(&block);
        }
    }
}

/// The prebuilt JS bundles that arrive from a gem rather than from the
/// app's own tree, keyed by the filename an import map pins them as.
/// Each is `<gem_dir>/<dir>/<file>` — `app/assets/javascripts` for the
/// Rails gems; `app/assets/javascript`, singular, for `lexxy`, which is
/// where that gem keeps its editor bundle.
///
/// Filename-keyed rather than gem-keyed because that is the direction
/// the lookup runs: a pin gives a served path, and the question is which
/// gem — if any — ships it. Both the minified and unminified spellings
/// are listed for the same reason the `to:` kwarg exists at all: the
/// blog pins `turbo.min.js` and campfire pins `turbo.js`, and neither
/// spelling is more canonical than the other.
const GEM_JS_BUNDLES: &[(&str, &str, &str)] = &[
    ("turbo.js", "turbo-rails", RAILS_JS),
    ("turbo.min.js", "turbo-rails", RAILS_JS),
    ("stimulus.js", "stimulus-rails", RAILS_JS),
    ("stimulus.min.js", "stimulus-rails", RAILS_JS),
    ("stimulus-loading.js", "stimulus-rails", RAILS_JS),
    ("stimulus-autoloader.js", "stimulus-rails", RAILS_JS),
    ("stimulus-importmap-autoloader.js", "stimulus-rails", RAILS_JS),
    ("actioncable.esm.js", "actioncable", RAILS_JS),
    ("actioncable.js", "actioncable", RAILS_JS),
    ("action_cable.js", "actioncable", RAILS_JS),
    ("actiontext.js", "actiontext", RAILS_JS),
    ("actiontext.esm.js", "actiontext", RAILS_JS),
    ("lexxy.js", "lexxy", "app/assets/javascript"),
    ("lexxy.min.js", "lexxy", "app/assets/javascript"),
];
const RAILS_JS: &str = "app/assets/javascripts";

/// De-blog the scaffold Makefile's `ASSET_JS` list and its gem-bundle
/// rules, the way `apply_makefile_test_list_stems` does for `SPINEL_TESTS`.
///
/// The scaffold hard-codes the blog's seven pins, ending in
/// `controllers/hello_controller.js` — a file no other app has, so
/// `make assets` in any other tree died on a missing prerequisite
/// before copying anything. campfire pins ninety-five modules and the
/// scaffold named none of them; its `static/assets/` came out empty and
/// its pages served ninety-five 404s, which is a chat application with
/// no Turbo and no composer.
///
/// Derived from `app.importmap` — the same pins `javascript_importmap_tags`
/// renders into the page — so the list of what the Makefile BUILDS and the
/// list of what the page ASKS FOR cannot drift apart. Anything else (a
/// glob of `app/javascript/`, say) would be a second, independently wrong
/// answer to the same question.
///
/// A pin is sorted by where its bytes live, checked against the file set
/// this emit actually produced rather than against the source app:
///
///   * `app/javascript/<rel>` or `vendor/javascript/<rel>` — covered by
///     the scaffold's two pattern rules, so it needs no rule of its own.
///   * a gem bundle (`GEM_JS_BUNDLES`) — gets an explicit rule.
///   * neither — OMITTED from the list, and named in a comment in its
///     place. Omitting keeps `make assets` runnable (the alternative is a
///     tree that cannot build its assets at all because one pin is
///     unresolvable); naming it keeps the gap readable to whoever unpacks
///     the archive, which a silent drop would not.
fn apply_makefile_asset_list(files: &mut [(String, String)], app: &App) {
    const BLOG_LIST: &str = "ASSET_JS := $(ASSETS)/turbo.min.js \\\n\
                             \x20           $(ASSETS)/stimulus.min.js \\\n\
                             \x20           $(ASSETS)/stimulus-loading.js \\\n\
                             \x20           $(ASSETS)/application.js \\\n\
                             \x20           $(ASSETS)/controllers/application.js \\\n\
                             \x20           $(ASSETS)/controllers/index.js \\\n\
                             \x20           $(ASSETS)/controllers/hello_controller.js";

    // Pin order is Rails' order (it drives modulepreload emission), and
    // duplicates are possible — two names can pin the same file. An app
    // with no import map at all (the Roda + Sequel exemplar) falls
    // through with an empty list, which is the point: it used to keep
    // the blog's seven targets and could not run `make assets` either.
    let mut rels: Vec<String> = Vec::new();
    for pin in app.importmap.iter().flat_map(|m| &m.pins) {
        let Some(rel) = pin.path.strip_prefix("/assets/") else {
            continue;
        };
        if !rel.ends_with(".js") {
            continue;
        }
        if !rels.iter().any(|r| r == rel) {
            rels.push(rel.to_string());
        }
    }

    let has = |p: &str| files.iter().any(|(path, _)| path == p);

    let mut targets: Vec<String> = Vec::new();
    let mut gems: Vec<(String, String, String)> = Vec::new();
    let mut unsourced: Vec<String> = Vec::new();
    for rel in &rels {
        if has(&format!("app/javascript/{rel}")) || has(&format!("vendor/javascript/{rel}")) {
            targets.push(rel.clone());
        } else if let Some((_, gem, gem_dir)) = GEM_JS_BUNDLES.iter().find(|(file, _, _)| file == rel) {
            targets.push(rel.clone());
            gems.push((rel.clone(), (*gem).to_string(), (*gem_dir).to_string()));
        } else {
            unsourced.push(rel.clone());
        }
    }

    let mut list = String::new();
    for note in &unsourced {
        list.push_str(&format!(
            "# NOT BUILT — `{note}` is pinned by config/importmap.rb and is in\n\
             # neither app/javascript/, vendor/javascript/, nor a gem this\n\
             # scaffold knows how to copy from. The page will request it.\n"
        ));
    }
    if targets.is_empty() {
        // Same posture as an empty SPINEL_TESTS: define the variable so
        // `assets` is trivially satisfiable rather than leaving a
        // dangling reference.
        list.push_str("ASSET_JS :=");
    } else {
        list.push_str("ASSET_JS := ");
        let joined = targets
            .iter()
            .map(|t| format!("$(ASSETS)/{t}"))
            .collect::<Vec<_>>()
            .join(" \\\n            ");
        list.push_str(&joined);
    }

    let mut rules = gems
        .iter()
        .map(|(file, gem, gem_dir)| {
            format!(
                "$(ASSETS)/{file}:\n\
                 \t@mkdir -p $(dir $@)\n\
                 \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q({gem})).gem_dir')/{gem_dir}/{file}\" $@"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    // CSS a GEM ships (`crate::gems::GEM_STYLESHEETS`: `trix.css` from
    // `action_text-trix`, Lexxy's four from `lexxy`). Ingest puts each
    // stem in `app.stylesheets` when the bundle has the gem (the page
    // links it because Rails' `:all` expansion walks gem asset paths), so
    // `ASSET_CSS` names `$(ASSETS)/<stem>.css` — and with no
    // `app/assets/**/<stem>.css` for the pattern rules to copy, each needs
    // the same explicit copy-from-gem rule the JS bundles get.
    let mut css_gems: Vec<&str> = Vec::new();
    for (gem, stems) in crate::gems::GEM_STYLESHEETS {
        for stem in *stems {
            let needed = app.stylesheets.iter().any(|s| s == stem)
                && !has(&format!("app/assets/stylesheets/{stem}.css"))
                && !has(&format!("app/assets/builds/{stem}.css"));
            if !needed {
                continue;
            }
            if !rules.is_empty() {
                rules.push_str("\n\n");
            }
            rules.push_str(&format!(
                "$(ASSETS)/{stem}.css:\n\
                 \t@mkdir -p $(dir $@)\n\
                 \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q({gem})).gem_dir')/app/assets/stylesheets/{stem}.css\" $@"
            ));
            if !css_gems.contains(gem) {
                css_gems.push(gem);
            }
        }
    }

    apply_makefile_asset_blocks(files, BLOG_LIST, &list, &rules);

    // The rules above shell out to `bundle exec ruby -e ...find_by_name`,
    // which answers only for a gem IN THE BUNDLE. The scaffold's
    // `group :assets` names turbo-rails and stimulus-rails because those
    // are the blog's two; campfire also pins `actioncable.esm.js` and
    // `actiontext.js`, and without this the rule for each fails at
    // `find_by_name` rather than at the copy — an error naming Bundler,
    // three steps from the import map that actually asked for the file.
    let mut names: Vec<&str> = Vec::new();
    for (_, gem, _) in &gems {
        if !names.contains(&gem.as_str()) {
            names.push(gem);
        }
    }
    // The stylesheet rules shell out to the same `find_by_name`, so
    // their gems ride the same group. (`actiontext` already depends on
    // `action_text-trix`, but naming it keeps rule and bundle agreeing
    // by construction rather than by transitivity.)
    for gem in css_gems {
        if !names.contains(&gem) {
            names.push(gem);
        }
    }
    apply_gemfile_asset_group(files, &names);
}

/// De-blog the scaffold Makefile's `ASSET_CSS` list, and drop the
/// Tailwind rule for an app that does not use Tailwind.
///
/// Same source as the `<link>` tags themselves — `app.stylesheets`, the
/// stems under `app/assets/stylesheets/` + `app/assets/builds/` that
/// `stylesheet_link_tag`'s group expansion renders one tag each for. One
/// list, two consumers, so a page cannot link a stylesheet the build
/// does not produce.
///
/// The Tailwind half is a separate question from the list. The scaffold
/// builds `tailwind.css` unconditionally, which for campfire meant
/// running npm and Tailwind to produce a stylesheet its layout never
/// links — and, worse, meant `make assets` needed Node at all. An app
/// uses Tailwind iff `tailwind` is one of its stylesheet stems, which is
/// exactly what `app/assets/builds/tailwind.css` (tailwindcss-rails'
/// output path) puts there.
fn apply_makefile_stylesheet_list(files: &mut [(String, String)], app: &App) {
    const BLOG_LIST: &str = "ASSET_CSS := $(ASSETS)/application.css \\\n\
                             \x20            $(ASSETS)/tailwind.css";

    let uses_tailwind = app.stylesheets.iter().any(|s| s == "tailwind");

    // An app with no stylesheets gets an empty variable rather than the
    // blog's two, for the same reason `SPINEL_TESTS` does: `assets` is
    // then trivially satisfiable, which is honest (there is nothing to
    // build) where naming files it does not have was not.
    let list = if app.stylesheets.is_empty() {
        "ASSET_CSS :=".to_string()
    } else {
        format!(
            "ASSET_CSS := {}",
            app.stylesheets
                .iter()
                .map(|s| format!("$(ASSETS)/{s}.css"))
                .collect::<Vec<_>>()
                .join(" \\\n             ")
        )
    };

    for (path, content) in files.iter_mut() {
        if path != "Makefile" {
            continue;
        }
        if content.contains(BLOG_LIST) {
            *content = content.replace(BLOG_LIST, &list);
        }
        if !uses_tailwind {
            strip_tailwind_rule(content);
        }
    }
}

/// Remove the Tailwind build rule and its npm sentinel from the scaffold
/// Makefile, leaving the surrounding comment (which explains why an app
/// might not have one). Anchored on both recipes; silent if either has
/// moved, matching the other Makefile rewrites here.
fn strip_tailwind_rule(makefile: &mut String) {
    // Bracketed by the two recipe HEADERS rather than matched as one
    // long literal: the body in between is a Tailwind command line that
    // will be re-flagged as the CLI changes, and an exact anchor over it
    // would go stale silently — leaving the rule in place for an app
    // that has no Tailwind, which is the bug this exists to prevent.
    const START: &str = "$(ASSETS)/tailwind.css: app/assets/tailwind.css";
    const LAST: &str = "\t@touch node_modules/.installed\n";

    let Some(at) = makefile.find(START) else { return };
    let Some(rel_end) = makefile[at..].find(LAST) else { return };
    let end = at + rel_end + LAST.len();
    makefile.replace_range(
        at..end,
        "# (This app writes plain CSS — no Tailwind rule, and no npm.)\n",
    );
}

/// Rewrite the scaffold Gemfile's `group :assets` body to exactly the
/// gems whose bundles this app's import map pins.
///
/// Derived from the same partition that generated the Makefile rules, so
/// the two cannot disagree — a rule that copies out of a gem dir and a
/// bundle that has no such gem is the failure this prevents.
///
/// Silent when the group is absent: `apply_gemfile_trim` drops it whole
/// for an app with no importmap JS, and that app reaches here with an
/// empty gem set anyway.
fn apply_gemfile_asset_group(files: &mut [(String, String)], gems: &[&str]) {
    const OPEN: &str = "group :assets do\n";
    const CLOSE: &str = "end\n";

    let Some((_, gemfile)) = files.iter_mut().find(|(p, _)| p == "Gemfile") else {
        return;
    };
    let Some(open_at) = gemfile.find(OPEN) else {
        return;
    };
    let body_at = open_at + OPEN.len();
    let Some(rel_close) = gemfile[body_at..].find(CLOSE) else {
        return;
    };
    let body: String = gems.iter().map(|g| format!("  gem \"{g}\"\n")).collect();
    gemfile.replace_range(body_at..body_at + rel_close, &body);
}

/// String half of `apply_makefile_asset_list` (separated for unit
/// testing). Silent when an anchor is missing, matching
/// `apply_makefile_test_list_stems` — the scaffold Makefile is ours, so
/// a moved anchor is a repo-side change caught by the emit tests, not
/// something an ingested app can provoke.
fn apply_makefile_asset_blocks(
    files: &mut [(String, String)],
    list_anchor: &str,
    list: &str,
    rules: &str,
) {
    const GEM_RULES: &str = "$(ASSETS)/turbo.min.js:\n\
        \t@mkdir -p $(dir $@)\n\
        \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(turbo-rails)).gem_dir')/app/assets/javascripts/turbo.min.js\" $@\n\
        \n\
        $(ASSETS)/stimulus.min.js:\n\
        \t@mkdir -p $(dir $@)\n\
        \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus.min.js\" $@\n\
        \n\
        $(ASSETS)/stimulus-loading.js:\n\
        \t@mkdir -p $(dir $@)\n\
        \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q(stimulus-rails)).gem_dir')/app/assets/javascripts/stimulus-loading.js\" $@";

    for (path, content) in files.iter_mut() {
        if path != "Makefile" {
            continue;
        }
        if content.contains(list_anchor) {
            *content = content.replace(list_anchor, list);
        }
        if content.contains(GEM_RULES) {
            // An app pinning no gem bundle leaves the block empty; the
            // surrounding comment stays, which is the honest thing —
            // it explains why there are no rules under it.
            *content = content.replace(GEM_RULES, rules);
        }
    }
}

/// Read the actual app emission before scaffold/runtime merging. The emitter
/// owns filename rules, including `Rooms::ClosedsControllerTest` becoming
/// `rooms_closeds_controller_test`. Scanning the merged tree would also pick
/// up scaffold-only tests such as `test/models/article_broadcasts_test.rb`.
fn app_test_stems(app_files: &[EmittedFile]) -> Vec<String> {
    let mut stems: Vec<String> = app_files
        .iter()
        .filter_map(|f| {
            let p = f.path.to_str()?;
            let stem = p.strip_suffix(".rb")?;
            // Any `test/<dir>/` the ruby emit writes — models,
            // controllers, channels, helpers (`emit::ruby::test_subdir`).
            (stem.ends_with("_test")
                && stem.starts_with("test/")
                && stem.matches('/').count() == 2)
            .then(|| stem.to_string())
        })
        .collect();
    stems.sort();
    stems
}

/// Replace the scaffold's blog test list only for CRuby/JRuby. Spinel's
/// `spin_shape` selects its own lane and still needs the original anchor.
fn apply_makefile_test_list_stems(files: &mut [(String, String)], stems: &[String]) {
    const BLOG_LIST: &str = "SPINEL_TESTS := \\\n\
                             \ttest/models/article_test \\\n\
                             \ttest/models/comment_test \\\n\
                             \ttest/controllers/articles_controller_test \\\n\
                             \ttest/controllers/comments_controller_test";

    let list = if stems.is_empty() {
        // An app with no tests still needs the variable defined —
        // `$(addprefix …)` over an undefined var is empty, and
        // `spinel-test` then trivially succeeds, which is honest here
        // (there is nothing to run) in a way it would not be if the
        // list were merely wrong.
        "SPINEL_TESTS :=".to_string()
    } else {
        format!(
            "SPINEL_TESTS := \\\n{}",
            stems
                .iter()
                .map(|s| format!("\t{s}"))
                .collect::<Vec<_>>()
                .join(" \\\n")
        )
    };

    for (path, content) in files.iter_mut() {
        if path == "Makefile" && content.contains(BLOG_LIST) {
            *content = content.replace(BLOG_LIST, &list);
        }
    }
}

/// De-blog the scaffold Gemfile: drop gem blocks whose backing app
/// surface the ingested app doesn't have. Two blocks are conditional:
///
///   * `group :assets` (turbo-rails + stimulus-rails) — only wanted when
///     the app ships importmap JS (`app/javascript/application.js` in
///     the fixture; the emitted Rakefile's `assets` task keys on the
///     same file). The dependency closure of these two gems is all of
///     Rails, so a JS-less tree that keeps them `bundle install`s Rails
///     it never loads — the first thing a reviewer of the Roda exemplar
///     tree noticed (issue #67).
///   * `gem "websocket-driver"` — backs the CRuby /cable endpoint;
///     dead weight when no model declares broadcasts (the paired
///     overlay wiring is dropped by `apply_cable_strip`).
///
/// When either block is dropped, the committed `Gemfile.lock` (which
/// pins the full closure) is dropped with it — the emitted tree's
/// `bundle install` resolves the reduced Gemfile fresh, the same way
/// the JRuby tree already ships lock-free after its sqlite3 gem swap.
fn apply_gemfile_trim(files: &mut Vec<(String, String)>, app: &App, fixture: &Path) {
    let has_js = fixture.join("app/javascript/application.js").exists();
    let has_cable = crate::lower::app_broadcasts_live(app);
    if has_js && has_cable {
        return;
    }
    let Some((_, gemfile)) = files.iter_mut().find(|(p, _)| p == "Gemfile") else {
        return;
    };
    *gemfile = trim_gemfile(gemfile, has_js, has_cable);
    files.retain(|(p, _)| p != "Gemfile.lock");
}

/// String half of `apply_gemfile_trim` (separated for unit testing):
/// drops whole blank-line-separated Gemfile paragraphs by marker, so
/// each gem's leading comment block travels with its `gem` line.
fn trim_gemfile(content: &str, has_js: bool, has_cable: bool) -> String {
    let kept: Vec<&str> = content
        .split("\n\n")
        .filter(|para| {
            if !has_js && para.contains("group :assets") {
                return false;
            }
            if !has_cable && para.contains("gem \"websocket-driver\"") {
                return false;
            }
            true
        })
        .collect();
    let mut out = kept.join("\n\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Reshape the assembled spinel tree into a spin package — spinel's
/// project tool (upstream `docs/spin.md`, landed 2026-07-05). Only
/// `BuildTarget::Spinel` routes through here; ruby/jruby consume the
/// raw `spinel_files` shape (subdir tests, `sig/` tree) unchanged.
///
/// Moves, in order:
///
/// 1. `sig/<p>.rbs` → `<p>.rbs` — spin's convention is file-adjacent
///    sidecars ("everything participates by extension"), not a sig/
///    tree. The Makefile's bare-spinel recipes switch to `--rbs .`.
/// 2. Spinel-lane tests — subclasses of `TestBase` or
///    `ActionDispatch::IntegrationTest` — flatten from
///    `test/{models,controllers}/` to `test/<name>.rb`: spin treats
///    exactly the top-level `test/*.rb` files as test programs, no
///    recursion. A lane test with no `def test_` leaves the tree, and
///    a warning names it.
/// 3. Top-level tests *outside* the lane (Minitest::Test shapes the
///    archive's TestBase helper never autoruns — compiled, they are
///    do-nothing binaries whose empty output vacuously matches an
///    empty snapshot) move to `test/cruby/`, which spin ignores. The
///    ruby/jruby archives remain their live lane.
/// 4. Relocated files get their requires rewritten for the new
///    location: bare `require "x"` (Makefile `-I` style) resolves
///    against test/, runtime/, app/, and the root to a
///    `require_relative`; unresolved names (stdlib) stay bare. spin
///    compiles with the require gate on, so a bare name that is not a
///    package root or dependency is a hard compile error.
/// 5. Every lane test gets a `.expected` snapshot — the emitted
///    runner's `<Class>: <N> tests passed` footer (src/emit/ruby.rs),
///    re-derived from the class line and `def test_` count. A snapshot
///    skips spin's no-snapshot CRuby diff lane, which cannot load this
///    tree (runtime/db.rb is spinel-FFI) — the same pattern the
///    published spinel-redis/spinel-pg packages use.
/// 6. `spin.toml` and `bin/blog.rb` (compile root; main.rb boots
///    unconditionally on require) land, and the Makefile's spinel
///    recipes are re-pointed at the sidecar layout with the
///    SPINEL_TESTS list regenerated from the actual lane. Patches are
///    exact-match — a scaffold edit that invalidates one fails the
///    emit loudly instead of desyncing silently.
///
/// `spin test` feeds the `.rbs` sidecars to the compiler itself
/// (matz/spinel#1788), so the emitted tree compiles as a plain spin
/// package with no explicit analyzer seeding.
///
/// `query_count_test.rb` rides the normal lane (it subclasses
/// `ActionDispatch::IntegrationTest`). It was previously carved to
/// `test/cruby/` for the civ-array codegen gap #1819/#1827 — its
/// `Db.capture_sql -> Array[String]` pin (the class-ivar-backed
/// `@query_log` array) either inferred `poly` (`=~` on poly raised) or,
/// once pinned, was rejected/segfaulted. matz's #1827 fix (spinel
/// 70581d31) honors the typed-array return pin by unboxing at the
/// boundary, so the test now compiles + runs green in the lane.
/// True where `konst` is named in code position: `Set.new`, `JSON[`,
/// `ERB(`, `Digest::SHA256`. The sigil is what separates a constant from
/// the prose the emitted runtime is full of — "Set-Cookie", "Set New
/// Password", "Set by tick()" all fail it — and a preceding identifier
/// character or colon rules out `HashSet` and `Foo::Set`. Whole-line
/// comments are skipped: the cookie jar explains a `Set.new` rewrite in
/// one, and that is not a use.
fn names_constant(src: &str, konst: &str) -> bool {
    // Most files never mention this name. Reject those with the optimized
    // substring search before walking/decoding every line of the runtime.
    if !src.contains(konst) {
        return false;
    }
    src.lines().any(|line| {
        if line.trim_start().starts_with('#') {
            return false;
        }
        // SUPERCLASS position — `class ApplicationPlatform <
        // PlatformAgent`. The only place a used constant stands at end
        // of line with no trailing `.`/`(`/`::` to prove it is a use
        // rather than a definition, so the punctuation rule below
        // cannot see it. campfire reaches the `platform_agent` gem
        // exactly once, exactly here, and the gem went undeclared.
        if let Some(rest) = line.trim().strip_prefix("class ") {
            if let Some((_, parent)) = rest.split_once('<') {
                if parent.trim() == konst {
                    return true;
                }
            }
        }
        let b = line.as_bytes();
        let mut i = 0;
        while let Some(off) = line[i..].find(konst) {
            let at = i + off;
            let head_ok = at == 0
                || !matches!(b[at - 1],
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b':');
            let tail = at + konst.len();
            let tail_ok = matches!(b.get(tail), Some(b'.' | b'[' | b'('))
                || (b.get(tail) == Some(&b':') && b.get(tail + 1) == Some(&b':'))
                // Only right after the keyword or a rescue-list comma: `raise "Timeout waiting"` names no constant.
                || (matches!(b.get(tail), None | Some(b',' | b' ')) && {
                    let before = line[..at].trim_start();
                    before == "rescue " || before == "raise "
                        || (before.starts_with("rescue ") && before.trim_end().ends_with(','))
                });
            if head_ok && tail_ok {
                return true;
            }
            i = tail;
        }
        false
    })
}

/// True where the emitted program defines the constant itself, in which
/// case the bundled library is not what the name refers to.
fn defines_constant(src: &str, konst: &str) -> bool {
    if !src.contains(konst) {
        return false;
    }
    src.lines().any(|line| {
        let trimmed = line.trim_start();
        ["class ", "module "].iter().any(|kw| {
            trimmed.strip_prefix(kw).is_some_and(|rest| {
                let name = rest
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("");
                name == konst
            })
        })
    })
}

/// True where the file has the require as a STATEMENT. A substring
/// match is not that: `runtime/spinel/erb_spinel.rb` explains in a
/// comment why it is not named `erb.rb` — quoting `require "erb"` —
/// and that quote made the shim look like a reopen over the bundled
/// library rather than the definer of `ERB`, so `require "erb"` was
/// written into every file naming `ERB::Util` and spinel's
/// `packages/erb` (a `class`) collided with the shim (a `module`). The
/// lobsters AOT lane was red for ten days on that comment.
fn requires_feature(src: &str, require_line: &str) -> bool {
    if !src.contains(require_line) {
        return false;
    }
    src.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed
            .strip_prefix(require_line)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

/// `write_bundled_requires` in the value-passing shape `target_files`'
/// match arms want.
fn with_bundled_requires(mut files: Vec<(String, String)>) -> Vec<(String, String)> {
    write_bundled_requires(&mut files);
    files
}

/// The libraries in `BUNDLED` that are bundled gems, not default gems,
/// as of Ruby 3.4. Under bundler a bundled gem is not on the load path
/// unless the Gemfile names it.
const BUNDLED_GEMS: [&str; 3] = ["base64", "bigdecimal", "csv"];

/// Names in the Gemfile each bundled gem that the tree requires.
/// Without the line, `bundle exec` stops at the require with "cannot
/// load such file -- csv". The scaffold Gemfile names the gems that the
/// runtime requires, so in practice only an app that names `CSV` gets
/// a line.
///
/// The last step of `write_bundled_requires`: the emit drops the app's
/// own requires, and that pass writes them back from `BUNDLED`, so the
/// requires are final only there. Every file, not only app/: a test
/// that names `CSV` runs under the same bundle.
fn apply_bundled_gem_wiring(files: &mut [(String, String)]) {
    let Some(gemfile_at) = files.iter().position(|(p, _)| p == "Gemfile") else {
        return;
    };
    let missing: Vec<&str> = BUNDLED_GEMS
        .into_iter()
        .filter(|gem| !files[gemfile_at].1.contains(&format!("gem {gem:?}")))
        .filter(|gem| {
            let require_line = format!("require {gem:?}");
            files.iter().any(|(p, c)| p.ends_with(".rb") && requires_feature(c, &require_line))
        })
        .collect();
    if missing.is_empty() {
        return;
    }
    let gemfile = &mut files[gemfile_at].1;
    gemfile.push_str(
        "\n# Bundled gems that the tree requires. Declared by\n\
         # `project.rs::apply_bundled_gem_wiring`.\n",
    );
    for gem in missing {
        gemfile.push_str(&format!("gem {gem:?}\n"));
    }
}

/// Constant → bundled library that provides it. One table, read by
/// both the pass that writes the requires and the gate that checks a
/// tree for missing ones — a second copy is how the rule drifts.
const BUNDLED: [(&str, &str); 19] = [
    // INERT in our trees, and deliberately: `runtime/spinel/base64.rb`
    // defines `Base64` without requiring the library, which the second
    // condition below reads as "the program defines it" and drops the
    // row for the whole tree. So the port answers every lane and
    // spinel's C `packages/base64` is never reached. The row stays as
    // the statement of what WOULD happen without the port — re-arming
    // it means giving base64 the three-branch ipaddr treatment, which
    // nothing has asked for. Same for `ERB` and `runtime/spinel/
    // erb_spinel.rb`.
    ("Base64", "base64"),
    // `BigDecimal(...)` — CRuby's bundled gem, spinel's
    // `packages/bigdecimal` (352f67d5, answering matz/spinel#4881).
    // lobsters' Comment#calculated_confidence moved from Float to it on
    // purpose, so no Float stand-in; Rails loads it for the app, so the
    // app never writes this require itself.
    ("BigDecimal", "bigdecimal"),
    ("CSV", "csv"),
    ("Digest", "digest"),
    ("ERB", "erb"),
    ("JSON", "json"),
    ("OptionParser", "optparse"),
    ("Pathname", "pathname"),
    ("Set", "set"),
    ("StringIO", "stringio"),
    ("StringScanner", "strscan"),
    ("URI", "uri"),
    // `Net::HTTP` — a REAL client on both lanes, so unlike IPAddr there
    // is nothing for roundhouse to port: CRuby resolves this to its own
    // stdlib and spinel to `packages/net`, which speaks the same
    // spelling (and, since 58b7c592/6a7107d6, HTTPS over the system
    // libssl). `names_constant` needs the name followed by `::`/`.`/`(`,
    // so `Net::HTTP` and `rescue Net::OpenTimeout` both anchor it and a
    // constant like `NetworkGuard` does not.
    ("Net", "net/http"),
    // `SecureRandom` — a Ruby DEFAULT GEM, so the CRuby/JRuby trees
    // resolve it from their own stdlib, and spinel from
    // `packages/securerandom`. Nothing for roundhouse to port: the
    // entropy is a primitive, not something Ruby can compute, and
    // spinel's package binds its runtime CSPRNG directly.
    //
    // A Rails app never writes this require — ActiveSupport loads
    // securerandom as an implementation detail — so without the entry
    // here the constant reached spinel undefined and campfire's very
    // first write (`Account::Joinable#generate_join_code`) raised
    // `undefined method 'join' for unknown`.
    ("SecureRandom", "securerandom"),
    // `Timeout.timeout` / `Timeout::Error` — Campfire unfurl deadline and
    // TimeLimitedVideoPreviewer#capture. Default gem on CRuby/JRuby;
    // Spinel takes `runtime/ruby/timeout.rb` via spinel_files.
    ("Timeout", "timeout"),
    // `rescue SocketError` — the socket extension's class, loaded by Rails
    // before the app; spinel's runtime defines it without a package.
    ("SocketError", "socket"),
    // `Shellwords.escape`: a default gem that a booted Rails 8.1 app has
    // already loaded, so apps call it without a require. INERT on our
    // trees: `runtime/spinel/shellwords.rb` defines the module (no
    // String/Array reopen — packages/shellwords' reopen makes
    // String#split a PolyArray and the Rails tree fails C compile), so
    // the program-defined-constant clause below drops the row.
    ("Shellwords", "shellwords"),
    // `PTY.spawn`: the app writes `require "pty"` (Rails does not load
    // it), but an app file reaches the tree without its requires.
    // Spinel takes `packages/pty`.
    ("PTY", "pty"),
    // `FileUtils.mkdir_p` / `remove_entry` — campfire's WAL checkpointer
    // makes its lock directory. A default gem on CRuby/JRuby (Rails has
    // loaded it); spinel takes `packages/fileutils`.
    ("FileUtils", "fileutils"),
];

/// Every gap in a tree, as `(file index, require line)`. One walk,
/// read by both the pass that writes the requires and the gate that
/// checks a tree for missing ones — a second copy is how the rule
/// drifts.
///
/// Per FILE, not per tree: spin compiles bin/blog.rb and each entry of
/// SPINEL_TESTS as separate programs, and `test/cruby/` is compiled by
/// neither, so "something in the tree requires it" does not mean the
/// program being built does. `require` is idempotent, so the file that
/// names the constant carries the require, the way a Ruby author would
/// write it. (A tree-wide check let `test/cruby/cgi_io_test.rb`'s
/// `require "stringio"` mask a missing one in `app/models/story.rb`.)
fn bundled_require_gaps(files: &[(String, String)]) -> Vec<(usize, String)> {
    let mut gaps = Vec::new();
    for (konst, feature) in BUNDLED {
        let require_line = format!("require {feature:?}");
        // The program defines the constant itself, so the bundled
        // library is not what the name refers to. A file that REQUIRES
        // the library and then opens the constant is not that: it is a
        // reopen over the bundled one (`runtime/net_http.rb` puts the
        // stub table and the socket seam in front of spinel's
        // `packages/net`, keeping the package's classes), and every
        // other file naming the constant still needs the require.
        if files.iter().any(|(p, c)| {
            p.ends_with(".rb") && defines_constant(c, konst) && !requires_feature(c, &require_line)
        }) {
            continue;
        }
        for (i, (path, content)) in files.iter().enumerate() {
            if path.ends_with(".rb")
                && names_constant(content, konst)
                && !requires_feature(content, &require_line)
            {
                gaps.push((i, require_line.clone()));
            }
        }
    }
    gaps
}

/// Files in an emitted tree that name a bundled-library constant with
/// no require for it — `"path: require \"x\""` per gap. The emitted
/// tree is the thing that runs, so the gate reads IT rather than
/// re-deriving which targets `write_bundled_requires` was wired into:
/// the table lived inside `spin_shape` for months and the ruby family
/// silently never got it.
pub fn missing_bundled_requires(files: &[(String, String)]) -> Vec<String> {
    bundled_require_gaps(files)
        .into_iter()
        .map(|(i, line)| format!("{}: {line}", files[i].0))
        .collect()
}

/// Bundled-library requires. spinel resolves `Set`, `StringIO` and
/// their siblings only when the program requires the library by name.
/// CRuby autoloads two of them outright — `Set.new` and `Pathname.new`
/// work in a bare script, the other eight raise NameError — and Rails
/// loads several more as a side effect of booting, so an app carried
/// over from Rails names them with no require anywhere and spinel
/// refuses the build: "X is provided by the bundled Y library, which
/// this program does not require" (matz/spinel 83658c1e). Write the
/// require the app never had to.
///
/// **Not spinel-only, though it lived inside `spin_shape` until now.**
/// The two CRuby autoloads are the ones a dev box hides: `Pathname()`
/// (campfire's `app/helpers/cable_helper.rb` calls the Kernel
/// conversion method) resolves bare on Ruby 4.0 and raises on 3.4, and
/// 3.4 is what the scaffold README claims and what
/// `campfire-conformance` pins — two test files died on a clean runner
/// that passed on a laptop. The other eight raise on every CRuby, so
/// the ruby family needs this table at least as much as spinel does.
///
/// The three conditions mirror spinel's own check: the constant is
/// named in code, the program does not define it itself (runtime/erb.rb
/// and runtime/base64.rb define theirs), and nothing requires it yet.
/// Sorted by constant so a tree that needs two requires gets them in a
/// stable order.
fn write_bundled_requires(files: &mut [(String, String)]) {
    for (i, require_line) in bundled_require_gaps(files) {
        insert_require_line(&mut files[i].1, &require_line);
    }
    apply_bundled_gem_wiring(files);
}

/// Write `line` (a `require`) at the top of a Ruby file, after its
/// leading magic comments: Ruby reads `# frozen_string_literal: true`
/// (and `encoding`, `warn_indent`, `shareable_constant_value`) only
/// before the first token, so a require written above one switches it
/// off and warns "'frozen_string_literal' is ignored after any tokens".
/// A shebang stays the first line.
fn insert_require_line(content: &mut String, line: &str) {
    // Ruby reads magic comments anywhere in the leading run of comments
    // and blank lines, so scan that whole run (a license header may come
    // first, a blank line may separate two magic comments) and write the
    // require after the last magic line, or the shebang.
    let mut at = 0;
    let mut end = 0;
    for (n, text) in content.split_inclusive('\n').enumerate() {
        end += text.len();
        let trimmed = text.trim_end();
        if n == 0 && trimmed.starts_with("#!") {
            at = end;
        } else if ruby_magic_comment(trimmed) {
            at = end;
        } else if !(trimmed.is_empty() || trimmed.starts_with('#')) {
            break;
        }
    }
    if at > 0 && !content[..at].ends_with('\n') {
        content.push('\n');
        at = content.len();
    }
    content.insert_str(at, &format!("{line}\n"));
}
fn ruby_magic_comment(line: &str) -> bool {
    let Some(body) = line.strip_prefix('#') else { return false };
    let body = body.trim();
    if body.starts_with("-*-") {
        return true;
    }
    let Some((key, _)) = body.split_once(':') else { return false };
    let key = key.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "frozen_string_literal" | "encoding" | "coding" | "warn_indent" | "shareable_constant_value" | "warn_past_scope"
    )
}

/// The emitted call every declared variant lowers to (`lower::attached
/// ::variations_expr`): the reader's constructor argument on the app
/// side, and the one line the wiring below looks for. An app with no
/// `attachable.variant …` block emits an empty list and never names it.
const VARIATION_MARKER: &str = "ActiveStorage::Variation.new(";

/// Whether the emitted app declares Active Storage variants — the
/// demand for an image processor.
fn app_declares_variants(files: &[(String, String)]) -> bool {
    files
        .iter()
        .any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && c.contains(VARIATION_MARKER))
}

/// The image-processor swap, shared by the spinel and CRuby trees:
/// when the app declares variants, `runtime/active_storage_processor
/// .rb` — a comment-only stub in the scaffold, so a tree without
/// variants links no image library — becomes the ruby-vips reopen of
/// `ActiveStorage::Processor` (runtime/spinel/facades/
/// active_storage_processor_vips.rb). `require "vips"` in that file
/// is the spinel-ruby-vips spin package on one tree and the gem on
/// the other; the manifest / Gemfile half is each caller's, since the
/// dependency is spelled differently in each. Same grain as the
/// bcrypt façade: whole-file, one require anchor either way.
fn apply_image_processor_wiring(files: &mut [(String, String)]) -> Result<bool, String> {
    if !app_declares_variants(files) {
        return Ok(false);
    }
    let real = crate::runtime_files::read_to_string("runtime/spinel/facades/active_storage_processor_vips.rb")
        .map_err(|e| format!("read runtime/spinel/facades/active_storage_processor_vips.rb: {e}"))?;
    let stub = files
        .iter_mut()
        .find(|(p, _)| p == "runtime/active_storage_processor.rb")
        .ok_or("image processor: app declares variants but runtime/active_storage_processor.rb \
                is not in the file set")?;
    stub.1 = real;
    Ok(true)
}

fn spin_shape(files: Vec<(String, String)>) -> Result<Vec<(String, String)>, String> {
    use std::collections::HashSet;

    // 1. sig/ tree → file-adjacent sidecars.
    let mut files: Vec<(String, String)> = files
        .into_iter()
        .map(|(p, c)| match p.strip_prefix("sig/") {
            Some(rest) => (rest.to_string(), c),
            None => (p, c),
        })
        .collect();

    // Require-resolution universe (post-sidecar move; .rb only).
    let rb_paths: HashSet<String> = files
        .iter()
        .filter(|(p, _)| p.ends_with(".rb"))
        .map(|(p, _)| p.clone())
        .collect();

    // 2./3./4. Relocate test programs; rewrite requires of anything moved.
    let mut lane: Vec<(String, String, usize)> = Vec::new(); // (path, class, n)
    let mut renames: Vec<(String, String)> = Vec::new(); // old .rb → new .rb
    let mut dropped: Vec<String> = Vec::new();
    for entry in files.iter_mut() {
        if !entry.0.starts_with("test/") || !entry.0.ends_with("_test.rb") {
            continue;
        }
        let base = entry.0.rsplit('/').next().unwrap().to_string();
        let top_level = entry.0 == format!("test/{base}");
        // A framework test written against the BLOG fixture names its
        // fixtures by hand (`require_relative "fixtures/articles"`), and
        // an app without them cannot compile it. `lane_test_class` is
        // structural and cannot see that: `query_count_test.rb` declares
        // `< ActionDispatch::IntegrationTest`, so it entered campfire's
        // lane and then failed on `cannot load such file --
        // test/fixtures/articles.rb`. Its Minitest-shaped sibling
        // `relation_find_test.rb`, which requires the same two, was
        // already safe only by accident — it takes the `test/cruby/`
        // fork for an unrelated reason.
        //
        // Asked as "do this file's own requires resolve in THIS tree?"
        // rather than by naming the file: `rb_paths` is the same
        // resolution universe the move rewriter uses, so the next such
        // test needs no second edit. An APP's emitted test cannot trip
        // this — `emit::ruby` writes its fixture requires from the
        // fixtures it just emitted — so what this drops is always a
        // framework test that does not belong in this app's tree.
        if unresolvable_require(&entry.1, &entry.0, &rb_paths).is_some() {
            dropped.push(entry.0.clone());
            continue;
        }
        // A Minitest-shaped file is CRuby-only whatever else it holds;
        // otherwise the lane is decided by the same structural check
        // `test_class_and_count` applies below, so the two cannot
        // disagree. See `lane_test_class` for what this replaced.
        let in_lane = !entry.1.lines().any(declares_minitest_class)
            && entry.1.lines().any(|l| lane_test_class(l).is_some());
        // The class and the count come from one call, so the drop and
        // the snapshot cannot disagree. The require rewrite below
        // changes neither.
        let counted = if in_lane {
            match test_class_and_count(&entry.1, &entry.0) {
                Ok(counted) => Some(counted),
                // Preserve recovery for app tests outside the snapshot
                // runner's supported shape, with their sidecars removed too.
                Err(e) => {
                    eprintln!("roundhouse: {e}; dropped");
                    dropped.push(entry.0.clone());
                    continue;
                }
            }
        } else {
            None
        };
        // An emitted lane test with no `def test_` compiles to a program
        // that tests nothing, so the spin tree omits it. `rails generate
        // model` writes such a class. The same shape also comes from
        // tests that the emit did not model (an `include`d module, a
        // `define_method` loop), so the warning names what the emit
        // wrote, not what the source holds.
        if counted.as_ref().is_some_and(|(_, n)| *n == 0) {
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                crate::span::Span::synthetic(),
                Some(crate::ident::Symbol::from("spinel")),
                "test_class_without_emitted_tests",
                format!(
                    "{}: the emitted test class has no `def test_` methods, so the spin \
                     tree omits it; tests that the emit did not model are lost too",
                    entry.0
                ),
            );
            d.severity = crate::diagnostic::Severity::Warning;
            emit::diagnostics::push(d);
            dropped.push(entry.0.clone());
            continue;
        }
        let new_path = if in_lane {
            format!("test/{base}")
        } else if top_level {
            format!("test/cruby/{base}")
        } else {
            continue; // subdir non-lane tests stay put (invisible to spin)
        };
        if new_path != entry.0 {
            let old_dir = entry.0.rsplit_once('/').unwrap().0.to_string();
            let new_dir = new_path.rsplit_once('/').unwrap().0.to_string();
            let rewritten = rewrite_requires_for_move(&entry.1, &old_dir, &new_dir, &rb_paths);
            renames.push((entry.0.clone(), new_path.clone()));
            entry.0 = new_path.clone();
            entry.1 = rewritten;
        }
        if let Some((class, n)) = counted {
            lane.push((new_path, class, n));
        }
    }

    // An orphaned framework test or a test class without emitted tests
    // (see above) leaves with its sidecar and its `.expected` snapshot —
    // a snapshot for a program that is not in the tree is what the next
    // reader would have to explain away.
    if !dropped.is_empty() {
        files.retain(|(p, _)| {
            !dropped.iter().any(|o| {
                p == o
                    || *p == format!("{}.rbs", o.trim_end_matches(".rb"))
                    || *p == format!("{o}.expected")
            })
        });
    }

    // A moved test's own `.rbs` sidecar travels with it: spin's
    // convention is file-adjacent, and `spin test` feeds it via `--rbs`
    // from beside the file (matz/spinel#1788).
    for entry in files.iter_mut() {
        if let Some(stem) = entry.0.strip_suffix(".rbs") {
            let old_rb = format!("{stem}.rb");
            if let Some((_, new_rb)) = renames.iter().find(|(o, _)| *o == old_rb) {
                entry.0 = format!("{}.rbs", new_rb.trim_end_matches(".rb"));
            }
        }
    }

    // Relocations can only collide by construction error — fail loudly.
    {
        let mut seen = HashSet::new();
        for (p, _) in &files {
            if !seen.insert(p.as_str()) {
                return Err(format!("spin_shape: path collision after reshaping: {p}"));
            }
        }
    }

    // 5. Snapshots.
    lane.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, class, n) in &lane {
        files.push((format!("{path}.expected"), format!("{class}: {n} tests passed\n")));
    }
    // test_helper.rb sits at test/ top level (shared layout with the
    // ruby/jruby trees), so spin runs it as a test program too. An
    // empty snapshot records the truth — it loads everything and
    // prints nothing — and turns it into a standalone compile gate
    // rather than a CRuby-lane failure (the no-snapshot lane can't
    // load the spinel-FFI runtime/db.rb).
    if files.iter().any(|(p, _)| p == "test/test_helper.rb") {
        files.push(("test/test_helper.rb.expected".to_string(), String::new()));
    }

    // bcrypt: when the app consumes BCrypt (has_secure_password /
    // login), swap the raising façade file for the real spin package —
    // `require "bcrypt"` resolves to spinel-bcrypt (crypt_blowfish in
    // carried C; spin compiles and links it), and the manifest gains
    // the dependency. Apps without BCrypt keep the façade: it compiles,
    // is dead code, and adds no dependency. The façade's .rbs sidecar
    // is dropped with it — the package's surface is inferred from its
    // source.
    let needs_bcrypt = files
        .iter()
        .any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && c.contains("BCrypt::"));
    if needs_bcrypt {
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/bcrypt_facade.rb")
            .ok_or("spin_shape: app references BCrypt but runtime/bcrypt_facade.rb \
                    is not in the spinel file set")?;
        facade.1 = "# Real bcrypt — the spinel-bcrypt spin package (crypt_blowfish in\n\
                    # carried C; see spin.toml [dependencies]). This file is the swap\n\
                    # point: the scaffold base ships a raising façade here for targets\n\
                    # without the package. Same require anchor either way.\n\
                    require \"bcrypt\"\n"
            .to_string();
        files.retain(|(p, _)| p != "runtime/bcrypt_facade.rbs");
    }

    // rqrcode: the same swap, for the QR code an app renders as SVG
    // (campfire's `QrCodeController`, lobsters' 2FA enrollment). The
    // package is spinel-rqrcode — the gem's matrix and SVG exporter over
    // Nayuki's qrcodegen in carried C, byte-identical to the gem — and it
    // carries its encoder, so unlike ruby-vips it asks nothing of the
    // machine. Manifest below, git form (matz/spin-index#8).
    let needs_rqrcode = files
        .iter()
        .any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && c.contains("RQRCode::"));
    if needs_rqrcode {
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/rqrcode_facade.rb")
            .ok_or("spin_shape: app references RQRCode but runtime/rqrcode_facade.rb \
                    is not in the spinel file set")?;
        facade.1 = "# Real rqrcode — the spinel-rqrcode spin package (Nayuki's qrcodegen\n\
                    # in carried C, the gem's version and mask choice; see spin.toml\n\
                    # [dependencies]). This file is the swap point: the scaffold base\n\
                    # ships a raising façade here for targets without the package.\n\
                    # Same require anchor either way.\n\
                    require \"rqrcode\"\n"
            .to_string();
        files.retain(|(p, _)| p != "runtime/rqrcode_facade.rbs");
    }

    // nokogiri: the same swap, for HTML parsing and DOM surgery (lobsters'
    // Markdowner post-processing and og: meta reads, campfire's Opengraph
    // and its turbo-stream test helper). The package is spinel-nokogiri —
    // libxml2 2.13.9 with Nokogiri's own patches, carried and compiled
    // with it, so the parse and the serialization are the gem's bytes.
    // The spinel read-path reopen (nokogiri_spinel.rb) goes with the
    // façade: it reopens the façade's classes, and the real ones are the
    // package's. Manifest below, git form (matz/spin-index#10/#11).
    let needs_nokogiri = files
        .iter()
        .any(|(p, c)| {
            p.starts_with("app/") && p.ends_with(".rb") && (c.contains("Nokogiri::") || c.contains("Nokogiri."))
        });
    if needs_nokogiri {
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/nokogiri_facade.rb")
            .ok_or("spin_shape: app references Nokogiri but runtime/nokogiri_facade.rb \
                    is not in the spinel file set")?;
        facade.1 = "# Real nokogiri — the spinel-nokogiri spin package (libxml2 2.13.9\n\
                    # with Nokogiri's patches, carried; see spin.toml [dependencies]).\n\
                    # This file is the swap point: the scaffold base ships a raising\n\
                    # façade here for targets without the package.\n\
                    require \"nokogiri\"\n"
            .to_string();
        if let Some(sidecar) = files.iter_mut().find(|(p, _)| p == "runtime/nokogiri_spinel.rb") {
            sidecar.1 = "# The read-path reopen of the Nokogiri façade is not loaded here: this\n\
                         # tree has the real Nokogiri (spinel-nokogiri), and the façade's\n\
                         # classes it reopened are not in it. Kept so boot.rb's require\n\
                         # still resolves.\n\
                         require \"nokogiri\"\n"
                .to_string();
        }
        files.retain(|(p, _)| p != "runtime/nokogiri_facade.rbs" && p != "runtime/nokogiri_spinel.rbs");
    }

    // commonmarker: no façade to swap (Markdowner's own façade stands aside
    // when the app's Markdowner names Commonmarker and Nokogiri — see
    // `Facade::lifted_by_constants`), only the package to declare: the
    // commonmarker gem's surface over cmark-gfm 0.29.0.gfm.13, carried.
    let needs_commonmarker = files
        .iter()
        .any(|(p, c)| {
            p.starts_with("app/") && p.ends_with(".rb") && (c.contains("Commonmarker.") || c.contains("Commonmarker::"))
        });
    // The app's own `require "commonmarker"` does not survive the emit (a
    // bare gem require is the ruby family's GEM_REQUIRES job), so the
    // package needs an anchor the way the swapped façades are one: the
    // require rides on gem_facades.rb, which the app's models already
    // require.
    if needs_commonmarker {
        let gf = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/gem_facades.rb")
            .ok_or("spin_shape: app references Commonmarker but runtime/gem_facades.rb \
                    is not in the spinel file set")?;
        gf.1.push_str("\n# The commonmarker spin package (spinel-commonmarker) — see spin.toml.\nrequire \"commonmarker\"\n");
    }

    // ruby-vips: the same swap for the image processor, when the app
    // declares variants — see `apply_image_processor_wiring`. The
    // package is the spinel-ruby-vips spin package; the manifest gains
    // it below, in the same git form bcrypt uses and for the same
    // reason (matz/spin-index#7 is the registration).
    let needs_vips = apply_image_processor_wiring(&mut files)?;

    write_bundled_requires(&mut files);
    // 6. Package manifest + compile root.
    let mut manifest = String::from(
        "# spin manifest — generated by Roundhouse (spinel target).\n\
         # Dependencies go here:\n\
         #   [dependencies]\n\
         #   name = { path = \"../spinel-name\" }\n\
         \n\
         [package]\n\
         # A program that runs for a second and exits spends no meaningful\n\
         # time in malloc. This one is a server: it allocates for every\n\
         # request for as long as it is up, and glibc's malloc/free is ~35%\n\
         # of its CPU under load. jemalloc is worth +58% on one OS worker\n\
         # and +25% on twelve, measured on this application\n\
         # (matz/spinel#4344). Rails ships jemalloc in its production image\n\
         # for the same reason.\n\
         #\n\
         # This makes libjemalloc-dev a BUILD requirement, not an optional\n\
         # speedup: spin fails the build rather than quietly producing a\n\
         # slower binary, since a build that varies silently between\n\
         # machines is a benchmark that silently compares two different\n\
         # binaries. It is the same contract the tree already has with\n\
         # libsqlite3-dev, and the README lists it beside that one. An\n\
         # older spin that predates the key ignores it and builds as\n\
         # before.\n\
         allocator = \"jemalloc\"\n",
    );
    if needs_bcrypt {
        manifest.push_str(
            "\n[dependencies]\n\
             # Real password hashing for has_secure_password / login —\n\
             # crypt_blowfish in carried C.\n\
             #\n\
             # The GIT form, not `bcrypt = \"~> 0.1\"`: that is the index\n\
             # form, and bcrypt is not in the published index\n\
             # (github.com/matz/spin-index carries pg, redis, spinel_kit).\n\
             # `spin build` on this tree failed out of the box with\n\
             # `not in the index: bcrypt` for anyone without a local\n\
             # checkout to `spin add --path`. Registration is filed as\n\
             # matz/spin-index#5 — switch this back to the version\n\
             # constraint once that merges.\n\
             #\n\
             # `ref` is a clone `--branch`, so it takes a branch or tag,\n\
             # never a commit SHA (`git clone --branch <sha>` fails with\n\
             # `Remote branch ... not found`). spinel-bcrypt publishes no\n\
             # tags today, so this tracks `main`.\n\
             bcrypt = { git = \"https://github.com/rubys/spinel-bcrypt\", ref = \"main\" }\n",
        );
    }
    if needs_vips {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# Image variants (thumbnails, avatars, logos): a subset of the\n\
             # ruby-vips gem over the SYSTEM libvips, in carried C. The\n\
             # package needs libvips linkable at build time (`libvips-dev`\n\
             # on Debian/Ubuntu, `brew install vips` on macOS) and loadable\n\
             # at run time (`libvips42`). Git form for the reason bcrypt\n\
             # gives above: matz/spin-index#7 is the registration.\n\
             ruby-vips = { git = \"https://github.com/rubys/spinel-ruby-vips\", ref = \"main\" }\n",
        );
    }
    if needs_rqrcode {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# QR codes as SVG (a join link, 2FA enrollment): the rqrcode gem's\n\
             # matrix and exporter over Nayuki's qrcodegen, carried in the\n\
             # package — no system library. Git form for the reason bcrypt\n\
             # gives above: matz/spin-index#8 is the registration.\n\
             rqrcode = { git = \"https://github.com/rubys/spinel-rqrcode\", ref = \"main\" }\n",
        );
    }
    if needs_nokogiri {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# HTML parsing, CSS/XPath and serialization: the nokogiri gem's\n\
             # surface over libxml2 2.13.9 with Nokogiri's patches, carried in\n\
             # the package. Git form until matz/spin-index#10/#11 land.\n\
             nokogiri = { git = \"https://github.com/rubys/spinel-nokogiri\", ref = \"main\" }\n",
        );
    }
    if needs_commonmarker {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# Markdown: the commonmarker gem's surface over cmark-gfm\n\
             # 0.29.0.gfm.13, carried in the package. Git form until\n\
             # matz/spin-index#9 lands.\n\
             commonmarker = { git = \"https://github.com/rubys/spinel-commonmarker\", ref = \"main\" }\n",
        );
    }
    files.push(("spin.toml".to_string(), manifest));
    files.push((
        "bin/blog.rb".to_string(),
        "# spin compile root: `spin build` → build/bin/blog; `spin run`.\n\
         # The application lives in main.rb (see SPECIMEN.md) — it boots\n\
         # the server unconditionally on require; this file exists because\n\
         # spin's unit of build is bin/<name>.rb.\n\
         require_relative \"../main\"\n"
            .to_string(),
    ));

    // Makefile re-pointing (exact-match patches; see doc comment).
    let spinel_tests = if lane.is_empty() {
        "SPINEL_TESTS :=".to_string()
    } else {
        let stems: Vec<String> = lane
            .iter()
            .map(|(p, _, _)| p.trim_end_matches(".rb").to_string())
            .collect();
        format!("SPINEL_TESTS := \\\n\t{}", stems.join(" \\\n\t"))
    };
    let patches: [(&str, &str); 4] = [
        (
            "RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)",
            "RBS_SRC  := $(shell find . -type f -name '*.rbs' 2>/dev/null)",
        ),
        (
            "RBS_FLAG := $(if $(wildcard sig),--rbs sig)",
            "RBS_FLAG := --rbs .",
        ),
        (
            "\t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@",
            "\t$(SPINEL) $(RBS_FLAG) $(SPINEL_TEST_FLAGS) $< -o $@",
        ),
        (
            "SPINEL_TESTS := \\\n\ttest/models/article_test \\\n\ttest/models/comment_test \\\n\ttest/controllers/articles_controller_test \\\n\ttest/controllers/comments_controller_test",
            "", // placeholder — replaced below with the computed list
        ),
    ];
    let makefile = files
        .iter_mut()
        .find(|(p, _)| p == "Makefile")
        .ok_or("spin_shape: no Makefile in the spinel file set")?;
    for (i, (from, to)) in patches.iter().enumerate() {
        let to: &str = if i == patches.len() - 1 {
            &spinel_tests
        } else {
            to
        };
        if !makefile.1.contains(from) {
            return Err(format!(
                "spin_shape: Makefile patch pattern not found (scaffold Makefile \
                 changed?): {:?}",
                &from[..from.len().min(60)]
            ));
        }
        makefile.1 = makefile.1.replacen(from, to, 1);
    }
    // The binary build goes through `spin build`, always. With package
    // dependencies the raw `$(SPINEL) main.rb` lane cannot resolve
    // `require "bcrypt"` (no -I, no carried-C link), which is why this
    // started as a bcrypt-only patch — but `spin build` is also the only
    // lane that READS spin.toml, and since matz/spinel#4344 the manifest
    // is where this program names its allocator. A dependency-free tree
    // built the raw way would carry an `allocator = "jemalloc"` the build
    // silently ignored, which is worse than not asking for it.
    //
    // It is also what the emitted README has always told the reader to
    // run, and what the published archive's three-line script runs. The
    // Makefile was the odd one out.
    //
    // THE TEST RECIPE NEEDS THE PACKAGES TOO, and used to say so in a
    // parenthesis here ("dep-carrying apps run tests via `spin test`")
    // that nothing implemented. The raw lane cannot resolve
    // `require "bcrypt"`, so every campfire test binary was built with
    // BCrypt missing — spinel warns and carries on, the constant resolves
    // to nothing, and the first fixture to hash a password dies with
    // `undefined method 'create' for unknown` (BCrypt::Password.create,
    // test/fixtures/users.rb). That is in `_fixtures_load!`, which every
    // test's `setup` runs, so it took out all six of room_test's tests
    // including the ones that touch no password at all. The lane could not
    // have passed whatever the app code said.
    //
    // `spin flags` is the supported seam for exactly this (spinel #4105):
    // it resolves the dependencies, warms the native cache, and prints the
    // `-I` / `--rbs` / `--link` line for a build spin does not itself
    // drive. It also supplies the `--rbs` root, so the test recipe drops
    // `$(RBS_FLAG)` rather than passing two.
    //
    // MINUS `--require-gate`, which `spin flags` includes and the app
    // binary should keep: it turns an unresolvable require into a
    // compile-time refusal. A TEST tree requires gems that have no spinel
    // surface at all — campfire's suite pulls vips, mocha and webmock —
    // and gating those fails the compile before the type errors this lane
    // exists to count. The gate is right for `spin build` and wrong here.
    //
    // Lazy `=`, not `:=`: this shells out, and `:=` would run it on every
    // `make assets` (cloning and building packages on a cold cache) rather
    // than only when a test is compiled. Warm cost is 0.75 s against a
    // ~23 s compile.
    {
        let dep_patches: [(&str, &str); 3] = [
            (
                "SPINEL ?= spinel",
                "SPINEL ?= spinel\nSPIN   ?= spin\n\n# Package deps for the per-test compiles, from spin itself.\n# `--require-gate` is stripped: the app binary wants an\n# unresolvable require to be fatal, a test tree requires\n# gems with no spinel surface (vips, mocha, webmock) and\n# must still compile far enough to report its type errors.\nSPIN_TEST_FLAGS = $(shell $(SPIN) flags 2>/dev/null | sed 's/--require-gate //')",
            ),
            (
                "\t$(SPINEL) main.rb $(RBS_FLAG) -o $@",
                "\t$(SPIN) build\n\tcp build/bin/blog $@",
            ),
            (
                "\t$(SPINEL) $(RBS_FLAG) $(SPINEL_TEST_FLAGS) $< -o $@",
                "\t$(SPINEL) $(SPIN_TEST_FLAGS) $(SPINEL_TEST_FLAGS) $< -o $@",
            ),
        ];
        // Both anchors or neither: half a patch would define $(SPIN) and
        // still run the raw recipe. A tree that carries a package MUST get
        // the spin lane — `require "bcrypt"` cannot resolve without it — so
        // there a missing anchor is an error. Everywhere else it just means
        // this input has no scaffold Makefile to patch (some unit inputs do
        // not), and the tree is left alone.
        if dep_patches.iter().all(|(from, _)| makefile.1.contains(from)) {
            for (from, to) in dep_patches {
                makefile.1 = makefile.1.replacen(from, to, 1);
            }
        } else if needs_bcrypt || needs_vips || needs_rqrcode || needs_nokogiri || needs_commonmarker {
            return Err(
                "spin_shape: Makefile dep-patch anchors not found (scaffold \
                 Makefile changed?) and this tree carries a package"
                    .to_string(),
            );
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// The class a line DECLARES as a lane test, if it declares one:
/// `class <Name> < TestBase` / `< ActionDispatch::IntegrationTest`.
///
/// STRUCTURAL, not a substring over the whole file — and that
/// distinction has drawn blood twice. Lane assignment used to ask
/// `content.contains("< TestBase")`, so ANY mention promoted a file
/// into the spinel lane: first a Minitest-only helper class in
/// `broadcasts_test.rb` (which then dragged the ActiveRecord graph into
/// its AOT compile and reddened `smoke-spinel`), then the COMMENT
/// written to warn about it — quoting the trigger string was enough to
/// spring the trap, and this time `test_class_and_count` disagreed with
/// the lane check and errored with "no test class found".
///
/// The two questions — "is this a lane test?" and "which class is it?"
/// — must be answered by the same code, or they can disagree. They now
/// are, with one addition: a file declaring a `Minitest::Test` subclass
/// is CRuby-only regardless (`declares_minitest_class`), which is what
/// makes a `< TestBase` HELPER class beside a Minitest one harmless.
fn lane_test_class(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("class ")?;
    // The parents `emit::ruby` leaves on a test class: `TestBase` is
    // what `ActiveSupport::TestCase` becomes; the rest are the
    // harness's own subclasses of it (runtime/spinel/test/test_helper.rb).
    const LANE_PARENTS: [&str; 5] = [
        "< TestBase",
        "< ActionDispatch::IntegrationTest",
        "< ActionView::TestCase",
        "< ActionCable::Channel::TestCase",
        "< ActionCable::Connection::TestCase",
    ];
    if !LANE_PARENTS.iter().any(|p| rest.contains(p)) {
        return None;
    }
    Some(rest.split_whitespace().next().unwrap_or_default().to_string())
}

/// Does this line declare a `Minitest::Test` subclass?
///
/// A file that does is CRuby-only whatever else it contains: it needs
/// `minitest/autorun`, which spin does not have (its own build says so —
/// "'minitest/autorun' is not available in Spinel"). This is the marker
/// that separates a framework unit test from a spin test program, and it
/// is why a `< TestBase` HELPER class beside a Minitest one does not
/// drag the file into the AOT lane.
fn declares_minitest_class(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("class ")
        .is_some_and(|rest| rest.contains("< Minitest::Test"))
}

/// The `class <Name> < TestBase` / `< ActionDispatch::IntegrationTest`
/// line (exactly one per lane test) plus the `def test_*` count —
/// enough to synthesize the runner's `<Class>: <N> tests passed`
/// footer (src/emit/ruby.rs prints it with no singular special-case).
/// A zero count is not an error here: `spin_shape` drops that test
/// with a warning.
fn test_class_and_count(content: &str, path: &str) -> Result<(String, usize), String> {
    let mut class: Option<String> = None;
    for line in content.lines() {
        if let Some(name) = lane_test_class(line) {
            if class.replace(name).is_some() {
                return Err(format!(
                    "spin_shape: {path}: multiple test classes in one file — \
                     snapshot synthesis assumes one"
                ));
            }
        }
    }
    let class = class.ok_or_else(|| format!("spin_shape: {path}: no test class found"))?;
    Ok((class, content.matches("def test_").count()))
}

/// Rewrite the requires of a file moving `old_dir` → `new_dir` inside
/// the virtual file set. `require_relative` targets are re-based;
/// bare `require "x"` (which the Makefile lanes resolved with `-I`
/// flags spin does not pass) becomes `require_relative` when `x.rb`
/// exists under test/, runtime/, app/, or the root. Anything else
/// (stdlib) is left bare for the require gate to judge. Lines with
/// trailing comments or non-literal arguments pass through untouched.
/// The first `require_relative` target in `content` that names no file
/// in `rb_paths`, or None when every one of them resolves.
///
/// `path` is the requiring file's own path — a `require_relative` is
/// relative to its directory, which is what makes `"fixtures/articles"`
/// inside `test/query_count_test.rb` mean `test/fixtures/articles.rb`.
fn unresolvable_require(
    content: &str,
    path: &str,
    rb_paths: &std::collections::HashSet<String>,
) -> Option<String> {
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    content.lines().find_map(|line| {
        let target = line
            .trim_start()
            .strip_prefix("require_relative \"")?
            .strip_suffix('"')?;
        let canon = vpath_normalize(&format!("{dir}/{target}"));
        (!rb_paths.contains(&format!("{canon}.rb"))).then(|| canon)
    })
}

fn rewrite_requires_for_move(
    content: &str,
    old_dir: &str,
    new_dir: &str,
    rb_paths: &std::collections::HashSet<String>,
) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        let rewritten = if let Some(rest) = t.strip_prefix("require_relative \"") {
            rest.strip_suffix('"').map(|target| {
                let canon = vpath_normalize(&format!("{old_dir}/{target}"));
                format!("{indent}require_relative \"{}\"", vpath_rel(new_dir, &canon))
            })
        } else if let Some(rest) = t.strip_prefix("require \"") {
            rest.strip_suffix('"').and_then(|target| {
                ["test", "runtime", "app", ""].iter().find_map(|root| {
                    let cand = if root.is_empty() {
                        format!("{target}.rb")
                    } else {
                        format!("{root}/{target}.rb")
                    };
                    rb_paths.contains(&cand).then(|| {
                        let canon = cand.trim_end_matches(".rb");
                        format!("{indent}require_relative \"{}\"", vpath_rel(new_dir, canon))
                    })
                })
            })
        } else {
            None
        };
        out.push_str(rewritten.as_deref().unwrap_or(line));
        out.push('\n');
    }
    out
}

/// Normalize a set-relative path: fold `.` and `..` components.
fn vpath_normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Relative path from directory `from_dir` ("" = set root) to `to` —
/// the string `require_relative` needs from a file in `from_dir`.
fn vpath_rel(from_dir: &str, to: &str) -> String {
    let from: Vec<&str> = if from_dir.is_empty() {
        Vec::new()
    } else {
        from_dir.split('/').collect()
    };
    let to_parts: Vec<&str> = to.split('/').collect();
    let mut i = 0;
    while i < from.len() && i + 1 < to_parts.len() && from[i] == to_parts[i] {
        i += 1;
    }
    let mut rel: Vec<String> = vec!["..".to_string(); from.len() - i];
    rel.extend(to_parts[i..].iter().map(|s| s.to_string()));
    rel.join("/")
}

/// Ensure the file set carries `db/seed.sql` (the self-contained,
/// Ruby-free seed applied with `sqlite3 <db> < db/seed.sql`).
///
/// THE APP'S OWN DATA, ALWAYS. `db/seeds.rb` renders to SQL
/// (`emit::shared::seed_sql`); an app without one gets its SCHEMA and no
/// rows. Either way this replaces whatever is in the set, including the
/// scaffold's copy that spinel/ruby/jruby pick up by directory walk.
///
/// What this retired: a hand-maintained transcription of the blog's rows
/// living in the compiler, injected into every archive whose emit
/// produced no seed — which was all of them. The blog shipped the same
/// data twice, derived and transcribed, with nothing keeping them in
/// sync; and every other app shipped the BLOG's rows. tiny-blog and
/// roda-blog have `posts`, campfire has fifteen chat tables, and all
/// three got `INSERT INTO articles`, which created a stray table and
/// left the real ones empty — so their Setup step appeared to succeed
/// against an empty database.
fn ensure_seed_sql(
    files: Vec<(String, String)>,
    app: &App,
) -> Result<Vec<(String, String)>, String> {
    let Some(content) = emit::shared::seed_sql::render_seed_sql(app)
        .or_else(|| emit::shared::seed_sql::render_schema_only_sql(app))
    else {
        return Ok(files);
    };
    let mut files: Vec<(String, String)> =
        files.into_iter().filter(|(p, _)| p != "db/seed.sql").collect();
    files.push(("db/seed.sql".to_string(), content));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Ship an empty `storage/.keep` so the Rails-traditional
/// `storage/development.sqlite3` always has a parent directory in the
/// extracted archive. The server's default DB path (BLOG_DB unset) and
/// the README's `## Setup` step (`sqlite3 storage/development.sqlite3 <
/// db/seed.sql`) both open that path, and sqlite creates the *file* but
/// not the *directory* — without `.keep` the first open fails with
/// `SQLITE_CANTOPEN`. Only DB-backed server archives (`ships_e2e`) need
/// it; TypescriptWorker/Blog have no server. No-op if already present.
fn ensure_storage_keep(
    files: Vec<(String, String)>,
    target: BuildTarget,
) -> Vec<(String, String)> {
    if !ships_e2e(target) || files.iter().any(|(p, _)| p == "storage/.keep") {
        return files;
    }
    let mut files = files;
    files.push(("storage/.keep".to_string(), String::new()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Resolve duplicate paths by keeping the last-inserted entry, then
/// sort alphabetically. Matches the Makefile's sequential-cp
/// semantics where later copies overwrite earlier ones.
fn dedupe_last_wins(files: Vec<(String, String)>) -> Vec<(String, String)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (path, content) in files {
        map.insert(path, content);
    }
    map.into_iter().collect()
}

/// Directory names that are dev/build-only and must not appear in
/// the emitted output. Matches the scaffold's `.gitignore`-shape
/// plus `vendor/`/`coverage/` (CI's bundler-cache populates them
/// with read-only gem trees that EACCES the walk).
///
/// `ruby_overlay` is the CRuby-target-specific scaffold overlay; the
/// build walker must NOT include the subdir verbatim or the manifest
/// re-creates it inside the emit on every transpile.
const SKIP_DIRS: &[&str] = &[
    "vendor", "node_modules", "build", "static", "tmp", "coverage", "log", ".bundle",
    "ruby_overlay",
];

/// Walk `src` recursively, collecting every readable text file as
/// `(prefix + relative_path, content)`. Skips dotfiles, unreadable
/// (binary) files, and well-known dev/build directories.
fn walk_dir_into(
    src: &Path,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing {}/", src.display()));
    }
    let mut stack = vec![(src.to_path_buf(), String::from(prefix))];
    while let Some((dir, sub_prefix)) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("read entry: {e}"))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
            if ty.is_dir() && SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            let nested = format!("{sub_prefix}{name_str}");
            if ty.is_dir() {
                stack.push((path, format!("{nested}/")));
            } else {
                let content = match fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                out.push((nested, content));
            }
        }
    }
    Ok(())
}

/// Orchestrates the `--site` mode of the `roundhouse` binary: for
/// every `BuildTarget`, produce `_site/browse/<lang>.{json,tgz,zip}`,
/// and copy the static landing-page assets (`site/`) plus the
/// `scripts/create-blog` standalone download to the output root.
///
/// `fixture` is the source-app path; `out` is the site output dir
/// (typically `_site/`). The output dir is removed and recreated if
/// it exists, so callers should pick a dedicated path.
pub fn build_site(fixture: &Path, out: &Path) -> Result<(), String> {
    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| format!("clean {}: {e}", out.display()))?;
    }
    fs::create_dir_all(out.join("browse"))
        .map_err(|e| format!("mkdir {}: {e}", out.display()))?;

    // Archive generation cleans browse/, so copy its viewer assets afterwards.
    build_archives(fixture, out, BuildTarget::ALL)?;
    copy_site_assets(out)?;
    copy_create_blog(out)?;
    crate::guide::render_site(out)?;

    Ok(())
}

/// Build selected browse archives without website assets or WASM demos.
/// The file sets and archive writers are shared with `build_site`.
pub fn build_archives(fixture: &Path, out: &Path, targets: &[BuildTarget]) -> Result<(), String> {
    let browse = out.join("browse");
    if browse.exists() {
        fs::remove_dir_all(&browse).map_err(|e| format!("clean {}: {e}", browse.display()))?;
    }
    fs::create_dir_all(&browse).map_err(|e| format!("mkdir {}: {e}", browse.display()))?;
    let mut app =
        ingest_app(fixture).map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    // Analyze + the same post-analyze shared lowerings as the
    // single-target driver; the site build has no diagnostic surface,
    // so the residue is dropped.
    let _ = crate::session::analyze_and_lower(&mut app);

    for target in targets {
        let files = target_files(&app, fixture, *target)?;
        let name = target.as_str();

        let json_path = out.join("browse").join(format!("{name}.json"));
        fs::write(&json_path, write_manifest_json(name, &files))
            .map_err(|e| format!("write {}: {e}", json_path.display()))?;
        eprintln!("wrote {}", json_path.display());

        let tgz_path = out.join("browse").join(format!("{name}.tgz"));
        write_tgz(&tgz_path, name, &files)?;
        eprintln!("wrote {}", tgz_path.display());

        let zip_path = out.join("browse").join(format!("{name}.zip"));
        write_zip(&zip_path, name, &files)?;
        eprintln!("wrote {}", zip_path.display());
    }

    Ok(())
}

fn copy_site_assets(out: &Path) -> Result<(), String> {
    let site = PathBuf::from("site");
    if !site.exists() {
        return Err(format!("missing {}/ (static assets)", site.display()));
    }
    copy_tree(&site, out)
}

/// Copy `scripts/create-blog` to `_site/create-blog`. fs::copy
/// preserves the executable bit on Unix.
fn copy_create_blog(out: &Path) -> Result<(), String> {
    let src = Path::new("scripts/create-blog");
    if !src.exists() {
        return Err(format!("missing {}", src.display()));
    }
    let dst = out.join("create-blog");
    fs::copy(src, &dst).map_err(|e| format!("copy {} → {}: {e}", src.display(), dst.display()))?;
    eprintln!("wrote {}", dst.display());
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            fs::create_dir_all(&dst_path)
                .map_err(|e| format!("mkdir {}: {e}", dst_path.display()))?;
            copy_tree(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} → {}: {e}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn write_manifest_json(language: &str, files: &[(String, String)]) -> String {
    #[derive(serde::Serialize)]
    struct File<'a> {
        path: &'a str,
        content: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Manifest<'a> {
        language: &'a str,
        files: Vec<File<'a>>,
    }
    let manifest = Manifest {
        language,
        files: files
            .iter()
            .map(|(p, c)| File { path: p, content: c })
            .collect(),
    };
    serde_json::to_string(&manifest).expect("serialize manifest")
}

/// Write a gzipped tar with each emitted file at `<language>/<path>`.
/// The leading `<language>/` means `tar -xzf rust.tgz` extracts into
/// `rust/` rather than scattering files into cwd. Mode 0644, mtime 0
/// for reproducible builds.
fn write_tgz(out: &Path, language: &str, files: &[(String, String)]) -> Result<(), String> {
    let f = fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let gz = GzEncoder::new(f, Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        let bytes = content.as_bytes();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        let archive_path = format!("{language}/{path}");
        tar.append_data(&mut header, &archive_path, bytes)
            .map_err(|e| format!("append {archive_path}: {e}"))?;
    }
    tar.into_inner()
        .and_then(|gz| gz.finish())
        .map_err(|e| format!("finalize {}: {e}", out.display()))?;
    Ok(())
}

fn write_zip(out: &Path, language: &str, files: &[(String, String)]) -> Result<(), String> {
    let f = fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(f);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (path, content) in files {
        let archive_path = format!("{language}/{path}");
        zip.start_file(&archive_path, opts)
            .map_err(|e| format!("zip start {archive_path}: {e}"))?;
        zip.write_all(content.as_bytes())
            .map_err(|e| format!("zip write {archive_path}: {e}"))?;
    }
    zip.finish()
        .map_err(|e| format!("zip finalize {}: {e}", out.display()))?;
    Ok(())
}

fn walk_ruby(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            walk_ruby(root, &path, files)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip prefix: {e}"))?;
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if content.contains('\0') {
                continue;
            }
            files.push((rel.to_string_lossy().into_owned(), content));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_require_stays_below_the_files_magic_comments() {
        let cases = [
            ("# frozen_string_literal: true\n\n# doc\nx = 1\n", "# frozen_string_literal: true\nrequire \"bigdecimal\"\n\n# doc\nx = 1\n"),
            ("#!/usr/bin/env ruby\n# encoding: utf-8\n# frozen_string_literal: true\nx\n", "#!/usr/bin/env ruby\n# encoding: utf-8\n# frozen_string_literal: true\nrequire \"bigdecimal\"\nx\n"),
            ("# -*- coding: utf-8 -*-\nx\n", "# -*- coding: utf-8 -*-\nrequire \"bigdecimal\"\nx\n"),
            ("# A doc comment: not magic\nx\n", "require \"bigdecimal\"\n# A doc comment: not magic\nx\n"),
            ("x = 1\n", "require \"bigdecimal\"\nx = 1\n"),
            ("# frozen_string_literal: true", "# frozen_string_literal: true\nrequire \"bigdecimal\"\n"),
            // A license or doc header above the magic comment.
            ("# Copyright 2020\n# License: MIT\n\n# frozen_string_literal: true\n\nx = 1\n", "# Copyright 2020\n# License: MIT\n\n# frozen_string_literal: true\nrequire \"bigdecimal\"\n\nx = 1\n"),
            // A blank line between two magic comments.
            ("# encoding: utf-8\n\n# frozen_string_literal: true\nx = 1\n", "# encoding: utf-8\n\n# frozen_string_literal: true\nrequire \"bigdecimal\"\nx = 1\n"),
            // A shebang with no magic comment.
            ("#!/usr/bin/env ruby\n# doc\nx = 1\n", "#!/usr/bin/env ruby\nrequire \"bigdecimal\"\n# doc\nx = 1\n"),
            // A magic-looking comment after the first token is no magic comment.
            ("x = 1\n# frozen_string_literal: true\n", "require \"bigdecimal\"\nx = 1\n# frozen_string_literal: true\n"),
        ];
        for (before, after) in cases {
            let mut content = before.to_string();
            super::insert_require_line(&mut content, "require \"bigdecimal\"");
            assert_eq!(content, after, "{before:?}");
        }
    }

    #[test]
    fn a_bare_exception_class_is_named_only_in_rescue_or_raise_position() {
        assert!(names_constant("  rescue SocketError => e\n", "SocketError"));
        assert!(names_constant("  rescue Timeout::Error, SocketError\n", "SocketError"));
        assert!(names_constant("  rescue Timeout::Error,SocketError\n", "SocketError"));
        assert!(names_constant("  raise SocketError, \"down\"\n", "SocketError"));
        assert!(names_constant("  raise SocketError\n", "SocketError"));
        assert!(!names_constant("  raise \"Timeout waiting for pool tasks\"\n", "Timeout"));
        assert!(!names_constant("  rescue => e # SocketError here\n", "SocketError"));
        assert!(!names_constant("  raise Error, \"SocketError down\"\n", "SocketError"));
    }

    #[test]
    fn bundled_instance_method_gate_is_scoped_to_model_receivers() {
        use crate::expr::{Expr, ExprNode};
        use crate::ident::{ClassId, Symbol};
        use crate::span::Span;
        use crate::ty::Ty;

        let tree = [
            ("app/models/article.rb", "class Article < ApplicationRecord; end\n"),
            ("app/models/custom.rb", "class Custom < ApplicationRecord\n  def attributes_before_type_cast; {}; end\nend\n"),
            ("app/models/parent.rb", "class Parent < ApplicationRecord\n  def attributes_before_type_cast; {}; end\nend\n"),
            ("app/models/child.rb", "class Child < Parent; end\n"),
            ("app/models/concerns/raw_values.rb", "module RawValues\n  def attributes_before_type_cast; {}; end\nend\n"),
            ("app/models/included.rb", "class Included < ApplicationRecord\n  include RawValues\nend\n"),
            ("app/services/probe.rb", "class Probe; end\n"),
        ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
        let mut app = crate::ingest::ingest_app_from_tree(tree).unwrap();
        let mut empty_reopen = app.library_classes.iter()
            .find(|class| class.name.0.as_str() == "RawValues")
            .unwrap().clone();
        empty_reopen.methods.clear();
        app.library_classes.insert(0, empty_reopen);
        let receiver = |node, ty| {
            let mut expr = Expr::new(Span::synthetic(), node);
            expr.ty = Some(ty);
            expr
        };
        let receiver_for = |name: &str| {
            receiver(ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::new("receiver"),
            }, Ty::Class { id: ClassId(Symbol::new(name)), args: vec![] })
        };

        assert!(is_unported_model_instance_method(&receiver_for("Article"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Probe"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Custom"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Child"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Included"), "attributes_before_type_cast", &app));
        assert!(is_unported_model_instance_method(
            &receiver(ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::new("receiver") },
                Ty::Union { variants: vec![Ty::Class { id: ClassId(Symbol::new("Article")), args: vec![] }, Ty::Nil] }),
            "attributes_before_type_cast",
            &app,
        ));
        assert!(!is_unported_model_instance_method(
            &receiver(ExprNode::Const { path: vec![Symbol::new("Article")] },
                Ty::Class { id: ClassId(Symbol::new("Article")), args: vec![] }),
            "attributes_before_type_cast",
            &app,
        ));
    }

    #[test]
    fn bundled_constant_gate_covers_auxiliary_emitted_roots() {
        use crate::app::{SqlFunction, SqlFunctionKind};
        use crate::dialect::{ControllerBodyItem, DirectHelper, Fixture, FixtureValue, Param};
        use crate::expr::{Expr, ExprNode};
        use crate::ident::{ClassId, Symbol};
        use crate::span::{FileId, Span};
        use crate::ty::Ty;

        let tree = [
            ("app/controllers/probes_controller.rb", "class ProbesController < ActionController::Base\n  def index(value: nil); nil; end\nend\n"),
            ("app/services/probe.rb", "class Probe\n  def value; nil; end\nend\n"),
        ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
        let mut base = crate::ingest::ingest_app_from_tree(tree).unwrap();
        let method = base.library_classes.iter().find(|class| class.name.0.as_str() == "Probe")
            .unwrap().methods[0].clone();
        base.library_classes.clear();

        for root in 0..8 {
            let mut app = base.clone();
            let span = Span { file: FileId(1), start: root, end: root + 1 };
            let mut constant = Expr::new(span, ExprNode::Const {
                path: vec![Symbol::new("Net"), Symbol::new("HTTPOK")],
            });
            constant.ty = Some(Ty::Class { id: ClassId(Symbol::new("Net::HTTPOK")), args: vec![] });
            match root {
                0 | 1 => app.fixtures.push(Fixture {
                    name: Symbol::new("probes"), path: Symbol::new("probes"), model_class: None,
                    file_blobs: Default::default(),
                    preamble: if root == 0 { vec![constant.clone()] } else { vec![] },
                    records: if root == 1 {
                        [(Symbol::new("one"), [(Symbol::new("value"), FixtureValue::Ruby(constant))].into_iter().collect())].into_iter().collect()
                    } else { Default::default() },
                }),
                2 => {
                    let ControllerBodyItem::Action { action, .. } = &mut app.controllers[0].body[0] else {
                        panic!("expected the controller action");
                    };
                    action.kw_params[0].1 = Some(constant);
                }
                3 => app.routes.direct_helpers.push(DirectHelper {
                    name: Symbol::new("probe"), params: vec![], body: constant,
                }),
                _ => {
                    let mut changed = method.clone();
                    if root == 4 || root == 6 {
                        changed.body = constant;
                    } else {
                        changed.params = vec![Param::with_default(Symbol::new("value"), constant)];
                    }
                    let kind = if root < 6 {
                        SqlFunctionKind::Scalar { method: changed }
                    } else if root == 6 {
                        SqlFunctionKind::Aggregate { step: changed, finalize: method.clone() }
                    } else {
                        SqlFunctionKind::Aggregate { step: method.clone(), finalize: changed }
                    };
                    app.sql_functions.push(SqlFunction { name: "probe".into(), arity: 1, kind });
                }
            }
            for target in [BuildTarget::Kotlin, BuildTarget::Ruby, BuildTarget::Jruby] {
                let (_, diags) = emit::diagnostics::scope(|| report_unsupported_bundled_constants(&app, target));
                assert_eq!(diags.len(), usize::from(target == BuildTarget::Kotlin), "root {root}, {target:?}: {diags:?}");
                if let Some(diag) = diags.first() {
                    assert_eq!(diag.span, span);
                    assert_eq!(diag.severity, crate::diagnostic::Severity::Error);
                    assert!(matches!(&diag.kind, crate::diagnostic::DiagnosticKind::Unsupported { construct, target: Some(name), .. }
                        if construct.as_str() == "bundled_constant" && name.as_str() == "kotlin"));
                }
            }
        }
    }

    #[test]
    fn forwarded_proc_boundary_preserves_existing_block_shapes() {
        for source in ["[1, 2].map { |x| x + 1 }", "callback = ->(x) { x + 1 }; [1, 2].map(&callback)",
            "[1, 2].map(&method(:normalize))"] {
            let tree = [("db/seeds.rb", source)].into_iter()
                .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
            let app = crate::ingest::ingest_app_from_tree(tree).unwrap();
            for &target in BuildTarget::ALL {
                let (result, diagnostics) = crate::emit::diagnostics::scope(||
                    reject_unsupported_forwarded_procs(&app, target));
                assert!(result.is_ok(), "{target:?}: {source}: {result:?}");
                assert!(diagnostics.is_empty(), "{target:?}: {source}: {diagnostics:?}");
            }
        }
    }

    #[test]
    fn archive_playwright_matches_the_prewarmed_version() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../e2e/package-lock.json")).unwrap();
        let version = lock["packages"]["node_modules/@playwright/test"]["version"]
            .as_str()
            .unwrap();
        for &target in BuildTarget::ALL {
            let files = ensure_e2e(Vec::new(), target);
            if matches!(target, BuildTarget::Blog | BuildTarget::TypescriptWorker) {
                assert!(files.is_empty(), "{} has no server e2e suite", target.as_str());
                continue;
            }
            let manifest = &files.iter().find(|(p, _)| p == "e2e/package.json").unwrap().1;
            let manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
            assert_eq!(
                manifest["devDependencies"]["@playwright/test"].as_str(),
                Some(version),
                "{} must reuse the prewarmed browser, not resolve a newer version",
                target.as_str()
            );
        }
    }

    /// The three test-case parents campfire's `test/channels` and
    /// `test/helpers` write against are lane tests, the way an
    /// integration test is; a Minitest one still is not.
    #[test]
    fn lane_test_class_knows_every_harness_parent() {
        for line in [
            "class PresenceChannelTest < ActionCable::Channel::TestCase",
            "class ApplicationCable::ConnectionTest < ActionCable::Connection::TestCase",
            "class MessagesHelperTest < ActionView::TestCase",
            "class RoomTest < TestBase",
            "class RoomsControllerTest < ActionDispatch::IntegrationTest",
        ] {
            assert!(lane_test_class(line).is_some(), "{line}");
        }
        assert_eq!(
            lane_test_class("class ApplicationCable::ConnectionTest < ActionCable::Connection::TestCase"),
            Some("ApplicationCable::ConnectionTest".to_string())
        );
        assert_eq!(lane_test_class("class InflectorTest < Minitest::Test"), None);
    }

    /// The gem-demand scan reads the APP's test bodies, never our own
    /// shim. `test/test_helper.rb` mentions `WebMock` on purpose (it
    /// resets the stub registry between tests when the gem is there),
    /// and counting that as demand wired webmock into every emitted
    /// tree — including the blog fixture, which then failed to LOAD.
    #[test]
    fn the_shim_helper_never_demands_a_test_gem_for_itself() {
        let mut files = vec![
            (
                "test/test_helper.rb".to_string(),
                "class TestBase\n  def setup\n    WebMock.reset! if defined?(WebMock)\n  end\nend\n"
                    .to_string(),
            ),
            (
                "test/models/article_test.rb".to_string(),
                "class ArticleTest < TestBase\n  def test_x\n  end\nend\n".to_string(),
            ),
            ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
        ];
        apply_test_gem_wiring(&mut files);
        let gemfile = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;
        assert!(
            !gemfile.contains("webmock"),
            "the helper's own mention is not demand:\n{gemfile}"
        );

        // …and a real test body still is.
        files[1].1 =
            "class ArticleTest < TestBase\n  def test_x\n    WebMock.stub_request(:get, \"/\")\n  end\nend\n"
                .to_string();
        files[2].1 = "source \"https://rubygems.org\"\n".to_string();
        apply_test_gem_wiring(&mut files);
        let gemfile = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;
        assert!(gemfile.contains("webmock"), "a test body's demand stands:\n{gemfile}");
    }

    #[test]
    fn trim_gemfile_drops_assets_and_websocket_blocks() {
        let gemfile = crate::runtime_files::read_to_string("runtime/spinel/scaffold/Gemfile").unwrap();
        let out = trim_gemfile(&gemfile, false, false);
        assert!(!out.contains("turbo-rails"), "assets group should be gone");
        assert!(!out.contains("stimulus-rails"));
        assert!(!out.contains("group :assets"));
        assert!(!out.contains("websocket-driver"));
        // The unconditional core survives.
        assert!(out.contains("gem \"sqlite3\""));
        assert!(out.contains("gem \"puma\""));
        assert!(out.contains("rubocop_spinel"));
        assert!(out.ends_with('\n'));
        // An app with both surfaces keeps the committed file verbatim.
        assert_eq!(trim_gemfile(&gemfile, true, true), gemfile);
        // JS-only trim keeps websocket-driver, and vice versa.
        assert!(trim_gemfile(&gemfile, false, true).contains("websocket-driver"));
        assert!(trim_gemfile(&gemfile, true, false).contains("turbo-rails"));
    }

    /// The asset list, the gem rules and the `:assets` group are three
    /// views of ONE partition, and the failure this guards is them
    /// disagreeing: a rule that copies out of a gem dir the bundle does
    /// not have fails at `find_by_name`, three steps from the import map
    /// that asked for the file.
    ///
    /// Reads the REAL scaffold Makefile and Gemfile, so a moved anchor
    /// fails here rather than silently shipping the blog's seven targets
    /// to an app that has none of them.
    #[test]
    fn asset_list_gem_rules_and_bundle_agree() {
        let makefile = crate::runtime_files::read_to_string("runtime/spinel/scaffold/Makefile").unwrap();
        let gemfile = crate::runtime_files::read_to_string("runtime/spinel/scaffold/Gemfile").unwrap();

        // campfire's shape: an app-side entry, a vendored module three
        // levels down, and two gem bundles the blog never pins.
        let mut app = App::new();
        app.importmap = Some(crate::app::Importmap {
            pins: ["/assets/application.js",
                   "/assets/lib/autocomplete/custom_elements/suggestion_option.js",
                   "/assets/trix.esm.min.js",
                   "/assets/actioncable.esm.js",
                   "/assets/actiontext.js",
                   "/assets/nowhere.js"]
                .iter()
                .map(|p| crate::app::ImportmapPin {
                    name: p.to_string(),
                    path: p.to_string(),
                })
                .collect(),
        });
        let mut files = vec![
            ("Makefile".to_string(), makefile),
            ("Gemfile".to_string(), gemfile),
            ("app/javascript/application.js".to_string(), String::new()),
            (
                "app/javascript/lib/autocomplete/custom_elements/suggestion_option.js".to_string(),
                String::new(),
            ),
            ("vendor/javascript/trix.esm.min.js".to_string(), String::new()),
        ];
        apply_makefile_asset_list(&mut files, &app);

        let mk = &files.iter().find(|(p, _)| p == "Makefile").unwrap().1;
        let gf = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;

        // The blog's list is gone, including the file no other app has.
        assert!(!mk.contains("hello_controller.js"), "blog list survived:\n{mk}");
        // Both source roots reach the list; neither needs a rule.
        assert!(mk.contains("$(ASSETS)/lib/autocomplete/custom_elements/suggestion_option.js"));
        assert!(mk.contains("$(ASSETS)/trix.esm.min.js"));
        // A gem bundle is listed AND ruled AND bundled — all three.
        for (file, gem) in [("actioncable.esm.js", "actioncable"), ("actiontext.js", "actiontext")] {
            assert!(mk.contains(&format!("$(ASSETS)/{file}")), "{file} not listed");
            assert!(mk.contains(&format!("$(ASSETS)/{file}:\n")), "{file} has no rule");
            assert!(mk.contains(&format!("%q({gem})")), "{gem} rule names the wrong gem");
            assert!(gf.contains(&format!("gem \"{gem}\"")), "{gem} not in the bundle");
        }
        // A gem this app does not pin loses its rule with its listing.
        // Matched on the RECIPE and the `gem` line, not on the word:
        // both files carry prose above these naming the blog's two.
        assert!(!mk.contains("$(ASSETS)/stimulus-loading.js:"), "unpinned gem rule survived");
        assert!(!gf.contains("\n  gem \"stimulus-rails\""), "unpinned gem stayed in the bundle");
        // An unsourceable pin is omitted and SAID SO, rather than listed
        // (make would stop on it) or dropped silently.
        assert!(!mk.contains("$(ASSETS)/nowhere.js"), "unsourceable pin was listed");
        assert!(mk.contains("NOT BUILT — `nowhere.js`"), "unsourceable pin went unnamed:\n{mk}");
    }

    /// An app with no Tailwind gets neither the build rule nor the npm
    /// install behind it — `make assets` there needs no Node at all.
    /// campfire writes twenty-six plain stylesheets and used to build a
    /// Tailwind file its layout never links.
    #[test]
    fn stylesheet_list_tracks_the_app_and_tailwind_is_conditional() {
        let makefile = crate::runtime_files::read_to_string("runtime/spinel/scaffold/Makefile").unwrap();

        let mut plain = App::new();
        plain.stylesheets = vec!["base".into(), "messages".into()];
        let mut files = vec![("Makefile".to_string(), makefile.clone())];
        apply_makefile_stylesheet_list(&mut files, &plain);
        let mk = &files[0].1;
        assert!(mk.contains("ASSET_CSS := $(ASSETS)/base.css \\\n             $(ASSETS)/messages.css"));
        // The RECIPE, not the word — the file's header comment and the
        // block's own explanation both still name @tailwindcss/cli.
        assert!(
            !mk.contains("$(ASSETS)/tailwind.css: app/assets/tailwind.css"),
            "tailwind rule survived:\n{mk}"
        );
        assert!(!mk.contains("node_modules/.installed:\n"), "npm sentinel survived");
        assert!(mk.contains("This app writes plain CSS"), "no note left in its place");
        // The copy patterns are what build them, and they stay.
        assert!(mk.contains("$(ASSETS)/%.css: app/assets/stylesheets/%.css"));

        // The blog does build Tailwind, and keeps the rule.
        let mut tw = App::new();
        tw.stylesheets = vec!["application".into(), "tailwind".into()];
        let mut files = vec![("Makefile".to_string(), makefile)];
        apply_makefile_stylesheet_list(&mut files, &tw);
        assert!(files[0].1.contains("$(ASSETS)/tailwind.css: app/assets/tailwind.css"));
        assert!(files[0].1.contains("node_modules/.installed:\n"));
    }

    /// Both dispatchers reset the app's `CurrentAttributes` subclass.
    ///
    /// There are two — `main.rb` serves and `test/test_helper.rb`
    /// drives controller tests — and they park the same
    /// `ActionController::Current` pair before calling
    /// `process_action`. Only main.rb used to clear `Current`, so a
    /// controller test inherited the previous request's user: campfire's
    /// `get join_url(code)` answered 302 to root because an earlier
    /// `sign_in` in the same file was still parked. Reads the REAL
    /// scaffold files, so a rename of the marker line fails here rather
    /// than silently dropping the reset from an emitted tree.
    #[test]
    fn current_attributes_reset_lands_in_both_dispatchers() {
        let mut app = App::new();
        app.current_attribute_classes =
            vec![crate::ident::ClassId(crate::ident::Symbol::from("Current"))];
        app.routes.entries.push(crate::dialect::RouteSpec::Root {
            target: "articles#index".to_string(),
            as_name: None,
        });

        let mut files = vec![
            (
                "main.rb".to_string(),
                crate::runtime_files::read_to_string("runtime/spinel/scaffold/ruby_overlay/main.rb").unwrap(),
            ),
            (
                "test/test_helper.rb".to_string(),
                crate::runtime_files::read_to_string("runtime/spinel/test/test_helper.rb").unwrap(),
            ),
        ];
        apply_controller_dispatch(&mut files, &app, false);

        for (path, content) in &files {
            assert!(
                content.contains(
                    "    ActionController::Current.controller = controller\n    Current.reset"
                ),
                "{path} parks the request without resetting Current"
            );
            // Once, not once per re-run: the pass runs twice on a CRuby tree.
            assert_eq!(content.matches("Current.reset").count(), 1, "{path}");
        }

        // An app with no CurrentAttributes subclass gets no reset call.
        app.current_attribute_classes.clear();
        let mut plain = vec![(
            "main.rb".to_string(),
            crate::runtime_files::read_to_string("runtime/spinel/scaffold/ruby_overlay/main.rb").unwrap(),
        )];
        apply_controller_dispatch(&mut plain, &app, false);
        assert!(!plain[0].1.contains("Current.reset"));
    }

    /// Reads the REAL scaffold files, so a reworded `route_table` in
    /// any of the three fails here rather than leaving a `RouteTable.root`
    /// call in a root-less app's tree.
    #[test]
    fn a_route_table_without_a_root_route_leaves_route_table_root_out() {
        let scaffold = || {
            [
                "runtime/spinel/scaffold/main.rb",
                "runtime/spinel/scaffold/ruby_overlay/main.rb",
                "runtime/spinel/test/test_helper.rb",
            ]
            .iter()
            .map(|p| {
                let path = if p.ends_with("test_helper.rb") { "test/test_helper.rb" } else { "main.rb" };
                (path.to_string(), crate::runtime_files::read_to_string(p).unwrap())
            })
            .collect::<Vec<_>>()
        };
        let mut app = App::new();
        app.routes.entries.push(crate::dialect::RouteSpec::Explicit {
            method: crate::dialect::HttpMethod::Get,
            path: "/widgets".to_string(),
            controller: crate::ident::ClassId(crate::ident::Symbol::from("WidgetsController")),
            action: crate::ident::Symbol::from("index"),
            as_name: None,
            constraints: Default::default(),
            scope: Default::default(),
        });

        // An app file whose name merely ends in `main.rb` is the app's.
        let domain = ("app/models/domain.rb".to_string(), "ROUTES = \"[RouteTable.root] + RouteTable.table\"\n".to_string());
        let mut files = scaffold();
        files.push(domain.clone());
        apply_route_table_root(&mut files, &app);
        let domain_after = files.pop().unwrap();
        assert_eq!(domain_after, domain, "an app file is not the dispatcher");
        for (path, content) in &files {
            assert!(!content.contains("RouteTable.root"), "{path} still calls RouteTable.root");
            assert!(content.contains("RouteTable.table + ActiveStorage::Routes.table"), "{path}");
        }

        // With a root the table keeps it, first.
        app.routes.entries.push(crate::dialect::RouteSpec::Root {
            target: "widgets#index".to_string(),
            as_name: None,
        });
        let mut files = scaffold();
        apply_route_table_root(&mut files, &app);
        for (path, content) in &files {
            assert!(content.contains("[RouteTable.root] + RouteTable.table"), "{path}");
        }
    }

    /// The emitted `sig/config/routes.rbs` has the same `module RouteTable`
    /// line as routes.rb. A Ruby constant there is not RBS.
    #[test]
    fn controller_paths_land_only_in_the_routes_file() {
        let mut app = App::new();
        app.routes.entries.push(crate::dialect::RouteSpec::Explicit {
            method: crate::dialect::HttpMethod::Get,
            path: "/admin/posts".to_string(),
            controller: crate::ident::ClassId(crate::ident::Symbol::from("Admin::PostsController")),
            action: crate::ident::Symbol::from("index"),
            as_name: None,
            constraints: Default::default(),
            scope: Default::default(),
        });
        let module = "module RouteTable\nend\n".to_string();
        let mut files = vec![
            ("config/routes.rb".to_string(), module.clone()),
            ("sig/config/routes.rbs".to_string(), module.clone()),
        ];
        apply_controller_paths(&mut files, &crate::lower::flatten_routes(&app));
        assert_eq!(
            files[0].1,
            "module RouteTable\n  CONTROLLER_PATHS = { admin_posts: \"admin/posts\" }.freeze\n\nend\n"
        );
        assert_eq!(files[1].1, module);
    }

    #[test]
    fn spinel_tree_keeps_one_declaration_per_runtime_method() {
        // `hash_to_query.rbs` exists only under runtime/spinel; the others
        // only under runtime/ruby (the embedded table decides the origin).
        let mut files = vec![
            ("runtime/active_record/base.rbs".to_string(),
             "module ActiveRecord\n  class Base\n    def self.where: (Hash[Symbol, untyped] c) -> Array[Base]\n    def self.count: () -> Integer\n  end\nend\n".to_string()),
            ("runtime/active_record/connection.rbs".to_string(),
             "module ActiveRecord\n  class Base\n    def self.where: (Hash[Symbol, untyped] c) -> Relation\n    def self.count: () -> Integer\n  end\nend\n".to_string()),
            ("runtime/action_view/view_helpers.rbs".to_string(),
             "module ActionView\n  module ViewHelpers\n    def self.to_query_pairs: (Hash[Symbol, untyped] p, String n) -> String\n  end\nend\n".to_string()),
            ("runtime/hash_to_query.rbs".to_string(),
             "module ActionView\n  module ViewHelpers\n    def self.to_query_pairs: (Hash[untyped, untyped] p, String n) -> String\n  end\nend\n".to_string()),
        ];
        resolve_runtime_sig_conflicts(&mut files).unwrap();
        // The reopen beats the primary file; identical duplicates stay.
        assert!(!files[0].1.contains("self.where"), "{}", files[0].1);
        assert!(files[0].1.contains("self.count"));
        assert!(files[1].1.contains("-> Relation"));
        // A runtime/spinel file beats the shared runtime's.
        assert!(!files[2].1.contains("to_query_pairs"), "{}", files[2].1);
        assert!(files[3].1.contains("Hash[untyped, untyped]"));

        // Two primary-rank files that disagree: no file overrides the
        // other, so the emit fails instead of leaving it to readdir.
        let mut undecided = vec![
            ("runtime/active_record/base.rbs".to_string(),
             "module ActiveRecord\n  class Base\n    def self.x: () -> Integer\n  end\nend\n".to_string()),
            ("sig/runtime/active_record/base.rbs".to_string(),
             "module ActiveRecord\n  class Base\n    def self.x: () -> String\n  end\nend\n".to_string()),
        ];
        let err = resolve_runtime_sig_conflicts(&mut undecided).unwrap_err();
        assert!(err.contains("ActiveRecord::Base#self.x"), "{err}");
    }

    #[test]
    fn strip_cable_from_config_ru_removes_all_three_seams() {
        let config_ru =
            crate::runtime_files::read_to_string("runtime/spinel/scaffold/ruby_overlay/config.ru").unwrap();
        let out = strip_cable_from_config_ru(&config_ru).unwrap();
        assert!(!out.contains("require_relative \"cable\""));
        assert!(!out.contains("Cable"), "no Cable constant may survive");
        assert!(!out.contains("/cable"));
        assert!(!out.contains("rack.hijack"));
        // The serving core survives intact.
        assert!(out.contains("require_relative \"main\""));
        assert!(out.contains("Db.with_connection { Main.run_rack(env) }"));
        assert!(out.contains("run app"));
        assert!(
            out.contains("GzipCache"),
            "campfire's config.ru gzips; the overlay must too"
        );
        // A config.ru missing the markers errors loudly instead of
        // silently shipping a tree whose require graph dangles.
        assert!(strip_cable_from_config_ru("run app\n").is_err());
    }

    /// A fixture through the SAME pipeline the emitter uses — ingest
    /// then analyze+lower. Raw ingest is not enough: `rewrite_assoc_create`
    /// (which turns `article.comments.create!` into the explicit
    /// `Comment.create!(article_id: …)` the seed renderer reads) needs the
    /// types analyze attaches.
    fn lowered_app(fixture: &str) -> App {
        let mut app = ingest_app(std::path::Path::new(fixture)).expect("ingest");
        let _ = crate::session::analyze_and_lower(&mut app);
        app
    }

    #[test]
    fn seed_sql_is_generated_from_the_apps_own_seeds() {
        let files = vec![("app/main.go".to_string(), "package main".to_string())];
        let out = ensure_seed_sql(files, &lowered_app("fixtures/real-blog")).unwrap();
        let seed = &out.iter().find(|(p, _)| p == "db/seed.sql").expect("seed shipped").1;
        assert!(seed.contains("generated from the app's own db/seeds.rb"), "{seed}");
        // The blog's three articles and three comments, from db/seeds.rb.
        assert_eq!(seed.matches("INSERT INTO articles").count(), 3, "{seed}");
        assert_eq!(seed.matches("INSERT INTO comments").count(), 3, "{seed}");
        // Schema first, so the file is self-sufficient against a fresh DB.
        assert!(
            seed.find("CREATE TABLE").unwrap() < seed.find("INSERT INTO").unwrap(),
            "DDL must precede the rows:\n{seed}"
        );
        assert!(out.windows(2).all(|w| w[0].0 <= w[1].0), "stays sorted");
    }

    #[test]
    fn a_stale_seed_file_is_replaced_not_preserved() {
        // The inverse of the old contract, and the point of the change:
        // spinel/ruby/jruby pick up the scaffold's copy by directory
        // walk, and that copy held the BLOG's rows for every app.
        let files = vec![
            ("db/seed.sql".to_string(), "-- stale scaffold copy".to_string()),
            ("app/main.go".to_string(), "package main".to_string()),
        ];
        let out = ensure_seed_sql(files, &lowered_app("fixtures/real-blog")).unwrap();
        let seeds: Vec<_> = out.iter().filter(|(p, _)| p == "db/seed.sql").collect();
        assert_eq!(seeds.len(), 1, "no duplicate db/seed.sql");
        assert!(
            !seeds[0].1.contains("stale scaffold copy"),
            "the app's own data must win:\n{}",
            seeds[0].1
        );
    }

    #[test]
    fn an_app_without_seeds_ships_its_schema_and_no_rows() {
        // tiny-blog has no `db/seeds.rb`. It must NOT inherit another
        // app's rows — its tables are `posts`/`comments`, and it was
        // being handed `INSERT INTO articles`.
        let out = ensure_seed_sql(Vec::new(), &lowered_app("fixtures/tiny-blog")).unwrap();
        let seed = &out.iter().find(|(p, _)| p == "db/seed.sql").expect("seed shipped").1;
        assert!(seed.contains("CREATE TABLE IF NOT EXISTS posts"), "{seed}");
        assert!(!seed.contains("INSERT INTO"), "no rows to invent:\n{seed}");
        assert!(!seed.contains("articles"), "no other app's tables:\n{seed}");
    }

    #[test]
    fn vpath_relative_paths() {
        assert_eq!(vpath_normalize("test/models/../test_helper"), "test/test_helper");
        assert_eq!(vpath_rel("test", "test/test_helper"), "test_helper");
        assert_eq!(vpath_rel("test", "app/models/article"), "../app/models/article");
        assert_eq!(vpath_rel("test/cruby", "runtime/broadcasts"), "../../runtime/broadcasts");
        assert_eq!(vpath_rel("", "main"), "main");
    }

    #[test]
    fn move_rewrites_bare_and_relative_requires() {
        let rb_paths: std::collections::HashSet<String> = [
            "app/models/article.rb",
            "test/fixtures/articles.rb",
            "test/test_helper.rb",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let src = "require_relative \"../test_helper\"\n\
                   require \"models/article\"\n\
                   require \"fixtures/articles\"\n\
                   require \"stringio\"\n";
        let out = rewrite_requires_for_move(src, "test/models", "test", &rb_paths);
        assert_eq!(
            out,
            "require_relative \"test_helper\"\n\
             require_relative \"../app/models/article\"\n\
             require_relative \"fixtures/articles\"\n\
             require \"stringio\"\n"
        );
    }

    /// The full reshaping on a synthetic miniature of the spinel set:
    /// sidecar move, lane flattening + snapshot, Minitest quarantine,
    /// package files, Makefile patches.
    #[test]
    fn spin_shape_reshapes_the_tree() {
        let makefile = "RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)\n\
             RBS_FLAG := $(if $(wildcard sig),--rbs sig)\n\
             $(BUILD)/test/%: test/%.rb $(RUBY_SRC)\n\
             \t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@\n\
             SPINEL_TESTS := \\\n\
             \ttest/models/article_test \\\n\
             \ttest/models/comment_test \\\n\
             \ttest/controllers/articles_controller_test \\\n\
             \ttest/controllers/comments_controller_test\n";
        let files = vec![
            ("Makefile".to_string(), makefile.to_string()),
            ("app/models/article.rb".to_string(), "class Article\nend\n".to_string()),
            ("main.rb".to_string(), "Main.run\n".to_string()),
            ("sig/app/models/article.rbs".to_string(), "class Article\nend\n".to_string()),
            (
                "sig/test/models/article_test.rbs".to_string(),
                "class ArticleTest\nend\n".to_string(),
            ),
            ("test/test_helper.rb".to_string(), "class TestBase\nend\n".to_string()),
            (
                "test/models/article_test.rb".to_string(),
                "require_relative \"../test_helper\"\n\
                 require \"models/article\"\n\
                 class ArticleTest < TestBase\n  def test_a\n  end\n  def test_b\n  end\nend\n"
                    .to_string(),
            ),
            (
                "test/broadcasts_test.rb".to_string(),
                // The COMMENT quotes the lane's trigger string, and a
                // helper class subclasses TestBase — neither makes this
                // a spin test program, and both used to promote it into
                // the lane when the check was a whole-file substring.
                "require_relative \"test_helper\"\n\
                 # Quarantined because no `class X < TestBase` declares a\n\
                 # test here — see `lane_test_class`.\n\
                 class Probe < TestBase\n  def helper\n  end\nend\n\
                 class BroadcastsTest < Minitest::Test\n  def test_x\n  end\nend\n"
                    .to_string(),
            ),
            (
                "test/query_count_test.rb".to_string(),
                "class QueryCountTest < ActionDispatch::IntegrationTest\n  def test_q\n  end\nend\n"
                    .to_string(),
            ),
        ];
        let out = spin_shape(files).unwrap();
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        let get = |p: &str| &out.iter().find(|(q, _)| q == p).unwrap().1;

        // Sidecar moved out of sig/; a moved test's sidecar follows it.
        assert!(paths.contains(&"app/models/article.rbs"));
        assert!(paths.contains(&"test/article_test.rbs"));
        assert!(!paths.iter().any(|p| p.starts_with("sig/")));

        // Lane test flattened, requires re-based, snapshot synthesized.
        assert!(paths.contains(&"test/article_test.rb"));
        let flat = get("test/article_test.rb");
        assert!(flat.contains("require_relative \"test_helper\""));
        assert!(flat.contains("require_relative \"../app/models/article\""));
        assert_eq!(get("test/article_test.rb.expected"), "ArticleTest: 2 tests passed\n");

        // Minitest shapes are quarantined to test/cruby/, with no
        // snapshots (they are not spin test programs) — even when the
        // file MENTIONS the lane's trigger string in a comment and
        // defines a `< TestBase` helper class beside its Minitest one.
        // Both of those reddened `smoke-spinel` when lane assignment
        // was a whole-file substring; the check is now the same
        // structural one snapshot synthesis applies (`lane_test_class`).
        assert!(paths.contains(&"test/cruby/broadcasts_test.rb"));
        assert!(!paths.contains(&"test/broadcasts_test.rb"));
        assert!(!paths.iter().any(|p| p.contains("cruby") && p.ends_with(".expected")));
        assert!(get("test/cruby/broadcasts_test.rb").contains("require_relative \"../test_helper\""));

        // query_count rides the normal lane (ActionDispatch::IntegrationTest):
        // flattened, snapshotted, not quarantined. #1819/#1827 fixed upstream.
        assert!(paths.contains(&"test/query_count_test.rb"));
        assert!(!paths.contains(&"test/cruby/query_count_test.rb"));
        assert_eq!(get("test/query_count_test.rb.expected"), "QueryCountTest: 1 tests passed\n");

        // Package files present. No BCrypt consumer in this tree, so no
        // bcrypt dependency (the manifest's [dependencies] mention is
        // the how-to comment only).
        assert!(get("spin.toml").contains("[dependencies]"));
        assert!(!get("spin.toml").contains("bcrypt"));
        assert!(get("bin/blog.rb").contains("require_relative \"../main\""));

        // Makefile re-pointed at the sidecar layout + actual lane list.
        let mk = get("Makefile");
        assert!(mk.contains("RBS_FLAG := --rbs ."));
        assert!(mk.contains("$(SPINEL) $(RBS_FLAG) $(SPINEL_TEST_FLAGS) $< -o $@"));
        assert!(mk.contains("SPINEL_TESTS := \\\n\ttest/article_test \\\n\ttest/query_count_test\n"));
        assert!(!mk.contains("test/models/article_test"));
    }

    /// An app that consumes BCrypt (has_secure_password / login) gets
    /// the real spin package: the raising façade file swaps to
    /// `require "bcrypt"`, its sidecar drops, the manifest declares the
    /// dependency, and the Makefile's binary build delegates to
    /// `spin build` (the raw $(SPINEL) lane can't resolve the package).
    #[test]
    fn spin_shape_swaps_bcrypt_facade_for_the_package() {
        let makefile = "SPINEL ?= spinel\n\
             RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)\n\
             RBS_FLAG := $(if $(wildcard sig),--rbs sig)\n\
             $(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC)\n\
             \t@mkdir -p $(BUILD)\n\
             \t$(SPINEL) main.rb $(RBS_FLAG) -o $@\n\
             $(BUILD)/test/%: test/%.rb $(RUBY_SRC)\n\
             \t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@\n\
             SPINEL_TESTS := \\\n\
             \ttest/models/article_test \\\n\
             \ttest/models/comment_test \\\n\
             \ttest/controllers/articles_controller_test \\\n\
             \ttest/controllers/comments_controller_test\n";
        let files = vec![
            ("Makefile".to_string(), makefile.to_string()),
            ("main.rb".to_string(), "Main.run\n".to_string()),
            (
                "app/models/user.rb".to_string(),
                "class User\n  def authenticate(pw)\n    BCrypt::Password.new(@digest) == pw\n  end\nend\n"
                    .to_string(),
            ),
            (
                "runtime/bcrypt_facade.rb".to_string(),
                "module BCrypt\nend\n".to_string(),
            ),
            (
                "sig/runtime/bcrypt_facade.rbs".to_string(),
                "module BCrypt\nend\n".to_string(),
            ),
        ];
        let out = spin_shape(files).unwrap();
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        let get = |p: &str| &out.iter().find(|(q, _)| q == p).unwrap().1;

        let facade = get("runtime/bcrypt_facade.rb");
        assert!(facade.contains("require \"bcrypt\""), "{facade}");
        assert!(!facade.contains("module BCrypt"), "{facade}");
        assert!(!paths.contains(&"runtime/bcrypt_facade.rbs"), "sidecar must drop");

        let manifest = get("spin.toml");
        assert!(manifest.contains("[dependencies]\n"), "{manifest}");
        assert!(manifest.contains("bcrypt = \"~> 0.1\""), "{manifest}");

        let mk = get("Makefile");
        assert!(mk.contains("SPIN   ?= spin"), "{mk}");
        assert!(mk.contains("\t$(SPIN) build\n\tcp build/bin/blog $@"), "{mk}");
        assert!(!mk.contains("$(SPINEL) main.rb"), "{mk}");
    }

    /// An app that names RQRCode (a QR code rendered as SVG) gets the
    /// spinel-rqrcode package the same way bcrypt does: façade file
    /// swapped to `require "rqrcode"`, sidecar dropped, manifest
    /// dependency declared. An app that does not keeps the façade.
    #[test]
    fn spin_shape_swaps_rqrcode_facade_for_the_package() {
        let makefile = "SPINEL ?= spinel\n\
             RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)\n\
             RBS_FLAG := $(if $(wildcard sig),--rbs sig)\n\
             $(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC)\n\
             \t@mkdir -p $(BUILD)\n\
             \t$(SPINEL) main.rb $(RBS_FLAG) -o $@\n\
             $(BUILD)/test/%: test/%.rb $(RUBY_SRC)\n\
             \t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@\n\
             SPINEL_TESTS := \\\n\
             \ttest/models/article_test \\\n\
             \ttest/models/comment_test \\\n\
             \ttest/controllers/articles_controller_test \\\n\
             \ttest/controllers/comments_controller_test\n";
        let tree = |controller: &str| {
            vec![
                ("Makefile".to_string(), makefile.to_string()),
                ("main.rb".to_string(), "Main.run\n".to_string()),
                ("app/controllers/qr_code_controller.rb".to_string(), controller.to_string()),
                ("runtime/rqrcode_facade.rb".to_string(), "module RQRCode\nend\n".to_string()),
                ("sig/runtime/rqrcode_facade.rbs".to_string(), "module RQRCode\nend\n".to_string()),
            ]
        };
        let with = spin_shape(tree(
            "class QrCodeController\n  def show\n    RQRCode::QRCode.new(url).as_svg(viewbox: true)\n  end\nend\n",
        ))
        .unwrap();
        let get = |out: &Vec<(String, String)>, p: &str| out.iter().find(|(q, _)| q == p).unwrap().1.clone();
        let facade = get(&with, "runtime/rqrcode_facade.rb");
        assert!(facade.contains("require \"rqrcode\""), "{facade}");
        assert!(!facade.contains("module RQRCode"), "{facade}");
        assert!(!with.iter().any(|(p, _)| p == "sig/runtime/rqrcode_facade.rbs"), "sidecar must drop");
        let manifest = get(&with, "spin.toml");
        assert!(
            manifest.contains("rqrcode = { git = \"https://github.com/rubys/spinel-rqrcode\""),
            "{manifest}"
        );

        let without = spin_shape(tree("class QrCodeController\nend\n")).unwrap();
        assert!(get(&without, "runtime/rqrcode_facade.rb").contains("module RQRCode"));
        assert!(!get(&without, "spin.toml").contains("rqrcode"));
    }

    /// A declared variant (`ActiveStorage::Variation.new(` in an emitted
    /// model) swaps the image-processor stub for the ruby-vips reopen and
    /// names the package in the manifest; an app without one keeps the
    /// stub and links no image library.
    #[test]
    fn spin_shape_swaps_the_image_processor_for_ruby_vips_when_variants_are_declared() {
        let makefile = "SPINEL ?= spinel\n\
             RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)\n\
             RBS_FLAG := $(if $(wildcard sig),--rbs sig)\n\
             $(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC)\n\
             \t@mkdir -p $(BUILD)\n\
             \t$(SPINEL) main.rb $(RBS_FLAG) -o $@\n\
             $(BUILD)/test/%: test/%.rb $(RUBY_SRC)\n\
             \t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@\n\
             SPINEL_TESTS := \\\n\
             \ttest/models/article_test \\\n\
             \ttest/models/comment_test \\\n\
             \ttest/controllers/articles_controller_test \\\n\
             \ttest/controllers/comments_controller_test\n";
        let base = |model: &str| {
            vec![
                ("Makefile".to_string(), makefile.to_string()),
                ("main.rb".to_string(), "Main.run\n".to_string()),
                ("app/models/user.rb".to_string(), model.to_string()),
                (
                    "runtime/active_storage_processor.rb".to_string(),
                    "# stub\n".to_string(),
                ),
            ]
        };
        let with = spin_shape(base(
            "class User\n  def avatar\n    ActiveStorage::Attached.new(\"User\", @id, \"avatar\", \
             [ActiveStorage::Variation.new(\"square\", 512, 512, \"webp\")])\n  end\nend\n",
        ))
        .unwrap();
        let get = |out: &Vec<(String, String)>, p: &str| {
            out.iter().find(|(q, _)| q == p).unwrap().1.clone()
        };
        let processor = get(&with, "runtime/active_storage_processor.rb");
        assert!(processor.contains("require \"vips\""), "{processor}");
        assert!(processor.contains("Vips::Image.thumbnail_buffer"), "{processor}");
        assert!(
            processor.contains("VipsExt.sp_vips_find_load"),
            "find_load wrap must reach C through VipsExt:\n{processor}"
        );
        assert!(
            !processor.contains("alias_method :"),
            "wrapping the Ruby finder in place re-enters the wrapper on the spinel package:\n{processor}"
        );
        let manifest = get(&with, "spin.toml");
        assert!(manifest.contains("[dependencies]\n"), "{manifest}");
        assert!(
            manifest.contains("ruby-vips = { git = \"https://github.com/rubys/spinel-ruby-vips\""),
            "{manifest}"
        );

        let without = spin_shape(base(
            "class User\n  def avatar\n    ActiveStorage::Attached.new(\"User\", @id, \"avatar\", [])\n  end\nend\n",
        ))
        .unwrap();
        assert_eq!(get(&without, "runtime/active_storage_processor.rb"), "# stub\n");
        assert!(!get(&without, "spin.toml").contains("ruby-vips"));
    }

    /// spinel will not resolve `Set` or `StringIO` without the require,
    /// and a Rails app never writes one. Add it — but only where the
    /// constant is really named, not where a header name or a page title
    /// happens to start with those letters, and not where the emitted
    /// runtime defines the constant itself.
    #[test]
    fn spin_shape_requires_the_bundled_libraries_the_app_names() {
        let makefile = "SPINEL ?= spinel\n\
             RBS_SRC  := $(shell find sig -type f -name '*.rbs' 2>/dev/null)\n\
             RBS_FLAG := $(if $(wildcard sig),--rbs sig)\n\
             $(BUILD)/blog: $(RUBY_SRC) $(RBS_SRC)\n\
             \t@mkdir -p $(BUILD)\n\
             \t$(SPINEL) main.rb $(RBS_FLAG) -o $@\n\
             $(BUILD)/test/%: test/%.rb $(RUBY_SRC)\n\
             \t$(SPINEL) --rbs sig $(SPINEL_TEST_FLAGS) $< -o $@\n\
             SPINEL_TESTS := \\\n\
             \ttest/models/article_test \\\n\
             \ttest/models/comment_test \\\n\
             \ttest/controllers/articles_controller_test \\\n\
             \ttest/controllers/comments_controller_test\n";
        let files = vec![
            ("Makefile".to_string(), makefile.to_string()),
            ("main.rb".to_string(), "Main.run\n".to_string()),
            (
                "app/models/comment.rb".to_string(),
                "require_relative \"application_record\"\n\
                 class Comment\n  def followers\n    Set.new\n  end\nend\n"
                    .to_string(),
            ),
            (
                "app/controllers/login_controller.rb".to_string(),
                "class LoginController\n  def edit\n    @title = \"Set New Password\"\n  end\nend\n"
                    .to_string(),
            ),
            (
                "runtime/tep/response.rb".to_string(),
                "# Set-Cookie can repeat.\nclass Response\nend\n".to_string(),
            ),
            (
                "app/models/story.rb".to_string(),
                "class Story\n  def pdf(body)\n    StringIO.new(body)\n  end\n\
                 \n  def digest(s)\n    Digest::SHA256.hexdigest(s)\n  end\nend\n"
                    .to_string(),
            ),
            // Already carries its own — one require, not two.
            (
                "test/cruby/cgi_io_test.rb".to_string(),
                "require \"stringio\"\nStringIO.new(\"x\")\n".to_string(),
            ),
            // The emitted runtime defines this one, so the bundled erb is
            // not what `ERB.new` refers to — and the comment QUOTING the
            // require it stands in for is not a require (the real shim,
            // runtime/spinel/erb_spinel.rb, carries that sentence; the
            // substring match it once tripped put `require "erb"` into
            // every file naming `ERB::Util`, and spinel's `class ERB`
            // package collided with the `module ERB` shim).
            (
                "runtime/erb.rb".to_string(),
                "# Not named erb.rb: a bare `require \"erb\"` must reach the stdlib.\n\
                 module ERB\nend\n"
                    .to_string(),
            ),
            (
                "app/views/show.rb".to_string(),
                "ERB.new(src).result(b)\n".to_string(),
            ),
        ];
        let out = spin_shape(files).unwrap();
        let get = |p: &str| &out.iter().find(|(q, _)| q == p).unwrap().1;

        let comment = get("app/models/comment.rb");
        assert!(comment.starts_with("require \"set\"\n"), "{comment}");
        // Still loads its parent — the require goes above, not instead.
        assert!(comment.contains("require_relative \"application_record\""), "{comment}");

        // `Digest::SHA256` is a use even though no `.`/`(` follows the name.
        let story = get("app/models/story.rb");
        assert!(story.contains("require \"stringio\""), "{story}");
        assert!(story.contains("require \"digest\""), "{story}");

        // A require in test/cruby/ — compiled by no spin program — must not
        // stand in for the one app/models/story.rb needs.
        let carved = get("test/cruby/cgi_io_test.rb");
        assert_eq!(carved.matches("require \"stringio\"").count(), 1, "{carved}");

        assert!(!get("app/views/show.rb").contains("require \"erb\""), "program defines ERB");

        for prose in ["app/controllers/login_controller.rb", "runtime/tep/response.rb"] {
            assert!(!get(prose).contains("require \"set\""), "{prose}");
        }
    }

    /// `ActionCable::Connection.build` is rewritten from the app, and the
    /// text it is rewritten to is the text `tests/spinel_cable_identity.rb`
    /// exercises.
    ///
    /// Reads the REAL `runtime/spinel/action_cable.rb`, so a rename of either
    /// marker fails here rather than silently shipping a tree whose
    /// `/cable` handshake identifies nobody — a failure mode with no
    /// symptom short of an unauthenticated socket, since the default arm
    /// connects anonymously ON PURPOSE and cannot be told apart from a
    /// generator that did not run.
    #[test]
    fn cable_connection_factory_is_generated_from_the_app() {
        use crate::dialect::LibraryClass;
        use crate::ident::{ClassId, Symbol};

        let cable = crate::runtime_files::read_to_string("runtime/spinel/action_cable.rb").unwrap();
        let connection_class = |name: &str| LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: false,
            parent: Some(ClassId(Symbol::from("ActionCable::Connection::Base"))),
            parent_span: Default::default(),
            includes: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
            class_ivar_initializers: Vec::new(),
        };

        // The app declares one: the arm names it.
        let mut app = App::new();
        app.library_classes.push(connection_class("ApplicationCable::Connection"));
        let mut files = vec![("runtime/action_cable.rb".to_string(), cable.clone())];
        apply_cable_connection(&mut files, &app);
        let out = &files[0].1;
        // VERBATIM the body `tests/spinel_cable_identity.rb` installs
        // before its generated-arm probes. Asserted as one string rather
        // than by `contains("ApplicationCable::Connection")` — the
        // default arm's comment block names that class too, in prose.
        assert!(
            out.contains(
                "    def self.build(cookies)\n      ApplicationCable::Connection.new(cookies)\n    end\n"
            ),
            "generated arm not written:\n{out}"
        );
        assert!(
            !out.contains("ActionCable::Connection::Base.new(cookies)"),
            "default arm survived alongside the generated one:\n{out}"
        );
        // Re-appliable: running twice is running once. The spinel tree
        // takes this pass in the shared base, and a second pass over an
        // already-rewritten file must not nest or duplicate the arm.
        let once = out.clone();
        apply_cable_connection(&mut files, &app);
        assert_eq!(files[0].1, once, "second application changed the file");

        // No connection class: the file ships its default arm untouched,
        // and the blog fixture keeps connecting anonymously.
        let mut files = vec![("runtime/action_cable.rb".to_string(), cable.clone())];
        apply_cable_connection(&mut files, &App::new());
        assert_eq!(files[0].1, cable, "a channel-less app had its action_cable.rb rewritten");

        // The transport file is a DELEGATION to the factory and carries
        // no markers of its own; a second copy of the arm there would be
        // two spellings of one class per tree.
        let mut app2 = App::new();
        app2.library_classes.push(connection_class("ApplicationCable::Connection"));
        let transport = crate::runtime_files::read_to_string("runtime/spinel/cable.rb").unwrap();
        assert!(
            transport.contains("    ActionCable::Connection.build(cookies)\n"),
            "cable.rb no longer delegates to the generated factory:\n{transport}"
        );
        let mut files = vec![("runtime/cable.rb".to_string(), transport.clone())];
        apply_cable_connection(&mut files, &app2);
        assert_eq!(files[0].1, transport, "cable.rb was rewritten");
    }

    /// `Content#to_s` renders through the app's own content layout —
    /// exactly when the tree carries one. Found by
    /// `scripts/campfire-compare`: the binary rendered every message
    /// body one `<div class="trix-content">` short of Rails, while the
    /// CRuby overlay had already grown the wrapper behind a `defined?`
    /// guard no static target can spell.
    #[test]
    fn content_layout_dispatch_is_generated_when_the_app_ships_one() {
        let runtime = crate::runtime_files::read_to_string("runtime/ruby/action_text.rb").unwrap();
        let layout_path =
            "app/views/layouts/action_text/contents/_content.rb".to_string();

        // The tree carries the layout: the marked span calls it.
        let mut files = vec![
            ("runtime/action_text.rb".to_string(), runtime.clone()),
            (layout_path.clone(), "module Views; end\n".to_string()),
        ];
        apply_content_layout(&mut files, &App::new());
        let out = &files[0].1;
        // Over `render_attachments`, not `@html`: the attachment
        // nodes are rendered before the layout wraps them, as Rails'
        // `render_action_text_attachments` runs first. Plus the newline
        // that ends Action Text's own `_content` partial, which is what
        // the layout yields.
        assert!(
            out.contains(
                "    def rendered_html\n      Views::Layouts::ActionText::Contents.content(render_attachments + \"\\n\")\n    end\n"
            ),
            "generated dispatch not written:\n{out}"
        );
        assert!(
            !out.contains("    def rendered_html\n      \"<div class=\\\"trix-content\\\">"),
            "default body survived alongside the generated one:\n{out}"
        );
        // No attachable model in this App: the render seam keeps its
        // empty default rather than an empty `case`.
        assert!(
            out.contains("    def self.render_attachment(attachment)\n      \"\"\n    end\n"),
            "the render default was rewritten with no attachable:\n{out}"
        );
        // Re-appliable: a second pass must not nest or duplicate.
        let once = files[0].1.clone();
        apply_content_layout(&mut files, &App::new());
        assert_eq!(files[0].1, once, "second application changed the file");

        // No layout in the tree: the default (Action Text's own layout)
        // ships untouched — the blog fixture has no Action Text layout.
        let mut files = vec![("runtime/action_text.rb".to_string(), runtime.clone())];
        apply_content_layout(&mut files, &App::new());
        assert_eq!(files[0].1, runtime, "a layout-less app had its runtime rewritten");
    }

    /// The spinel tree PERFORMS its mixins, as a class reopen.
    ///
    /// It used to emit a commented-out line instead, because spinel
    /// refuses `X.prepend Y` through an explicit receiver AND the reopen
    /// its diagnostic recommended was a silent no-op — campfire's
    /// `RoomStreamsAreAuthorized` present in the tree and absent from the
    /// lookup chain, the exact failure `lower::module_mixins` exists to
    /// prevent. Fixed upstream in matz/spinel `a7b6f726`.
    ///
    /// ASSERTS THE COMMENT IS GONE, not just that the line is there: the
    /// old form was a `#   `-prefixed copy of the same text, so a check
    /// for the target's name alone passes against either.
    #[test]
    fn the_spinel_tree_performs_its_module_mixins_as_a_reopen() {
        use crate::app::{MixinKind, ModuleMixin};
        use crate::ident::Symbol;

        let mut app = App::new();
        app.module_mixins.push(ModuleMixin {
            target: Symbol::from("Turbo::StreamsChannel"),
            module: Symbol::from("RoomStreamsAreAuthorized"),
            kind: MixinKind::Prepend,
        });

        let mut files = vec![("boot.rb".to_string(), "# boot\n".to_string())];
        apply_module_mixins(&mut files, &app, MixinForm::Reopen);
        let boot = &files[0].1;
        assert!(
            boot.contains("class Turbo::StreamsChannel\n  prepend RoomStreamsAreAuthorized\nend"),
            "reopen form not emitted:\n{boot}"
        );
        assert!(
            !boot.contains("#   Turbo::StreamsChannel"),
            "the commented-out form came back:\n{boot}"
        );
        assert!(
            !boot.contains("NOT\n         # PERFORMED"),
            "still claims the mixin is unperformed:\n{boot}"
        );

        // The ruby family keeps the explicit-receiver spelling it has
        // always emitted — CRuby has no quarrel with it, and
        // `overlay_cable_dispatch` drives a subscribe through it.
        let mut files = vec![("boot.rb".to_string(), "# boot\n".to_string())];
        apply_module_mixins(&mut files, &app, MixinForm::ExplicitReceiver);
        assert!(
            files[0].1.contains("Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized"),
            "explicit-receiver form changed:\n{}",
            files[0].1
        );

        // A mixin onto a MODULE reopens with `module`; `class` on one is
        // a TypeError at boot.
        let mut modapp = App::new();
        modapp.module_mixins.push(ModuleMixin {
            target: Symbol::from("Greetable"),
            module: Symbol::from("Loud"),
            kind: MixinKind::Include,
        });
        modapp.library_classes.push(crate::dialect::LibraryClass {
            name: crate::ident::ClassId(Symbol::from("Greetable")),
            is_module: true,
            parent: None,
            parent_span: Default::default(),
            includes: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
            class_ivar_initializers: Vec::new(),
        });
        let mut files = vec![("boot.rb".to_string(), "# boot\n".to_string())];
        apply_module_mixins(&mut files, &modapp, MixinForm::Reopen);
        assert!(
            files[0].1.contains("module Greetable\n  include Loud\nend"),
            "a module target was reopened as a class:\n{}",
            files[0].1
        );
    }

    /// With the gem loaded underneath, an UNSTUBBED `payload_send`
    /// must reach the gem's own. The guard once asked `respond_to?`
    /// from inside `class << self` — the singleton class, which never
    /// responds — so the alias was never taken and every real campfire
    /// notification on the CRuby tree raised "web-push is not
    /// installed". A stand-in module plays the gem so the test needs
    /// neither the gem nor the network.
    #[test]
    fn web_push_stub_reopen_delegates_to_the_loaded_gem() {
        let script = format!(
            "module WebPush\n  def self.payload_send(**o) = \"delivered:#{{o[:endpoint]}}\"\nend\n{WEB_PUSH_STUB_REOPEN}\n\
             print WebPush.payload_send(endpoint: \"e\")\n\
             WebPush.stub_payload_send\n\
             print \" \", WebPush.payload_send(endpoint: \"e\").inspect\n"
        );
        let out = std::process::Command::new("ruby")
            .arg("-e")
            .arg(&script)
            .output()
            .expect("ruby");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "delivered:e \"\"",
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The spinel tree names the app source each emitted line came from
    /// (`#<SPINEL_SOURCE>`, spinel#7630): a view's `<%= %>` tag reports
    /// its ERB line, at column 0 where Spinel matches the marker. The
    /// ruby family shares `spinel_files` and stays unmarked.
    #[test]
    fn spinel_tree_marks_app_source_lines() {
        let fixture = Path::new("fixtures/real-blog");
        if !fixture.is_dir() {
            eprintln!("skip: fixtures/real-blog absent");
            return;
        }
        let app = crate::ingest::ingest_app(fixture).expect("ingest real-blog");
        let files = spinel_base_files(&app, fixture).expect("spinel base files");
        let view = files
            .iter()
            .find(|(p, _)| p == "app/views/articles/_article.rb")
            .map(|(_, c)| c.as_str())
            .expect("_article.rb in spinel tree");
        let lines: Vec<&str> = view.lines().collect();
        let title = lines
            .iter()
            .position(|l| l.contains("article.title"))
            .expect("the link_to article.title line");
        assert_eq!(
            lines[title - 1],
            "#<SPINEL_SOURCE>real-blog/app/views/articles/_article.html.erb:4",
            "{view}"
        );
        // The def line itself is marked: Spinel names a --debug
        // backtrace frame by it (spinel#7658), and unmarked it would
        // report the previous method's held position.
        let def = lines
            .iter()
            .position(|l| l.trim_start().starts_with("def self.article_into"))
            .expect("the article_into def");
        assert_eq!(
            lines[def - 1],
            "#<SPINEL_SOURCE>real-blog/app/views/articles/_article.html.erb:1",
            "{view}"
        );
        let ruby = ruby_runtime_files(&app, fixture).expect("ruby tree");
        let marked: Vec<&str> = ruby
            .iter()
            .filter(|(_, c)| c.contains("#<SPINEL_SOURCE>"))
            .map(|(p, _)| p.as_str())
            .collect();
        assert!(marked.is_empty(), "ruby tree carries spinel markers: {marked:?}");
    }

    /// Shared relation.rbs types `initialize` / terminals as `Base` for
    /// Roundhouse Bar A/B; Spinel emit must rewrite those to `untyped`
    /// or campfire AOT rejects class handles and User* slots.
    #[test]
    fn spinel_emit_rewrites_relation_base_handle_to_untyped() {
        let fixture = Path::new("fixtures/tiny-blog");
        if !fixture.is_dir() {
            eprintln!("skip: fixtures/tiny-blog absent");
            return;
        }
        let app = crate::ingest::ingest_app(fixture).expect("ingest tiny-blog");
        let files = spinel_base_files(&app, fixture).expect("spinel base files");
        let relation = files
            .iter()
            .find(|(p, _)| p.ends_with("active_record/relation.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("relation.rbs in spinel tree");
        assert!(
            relation.contains("def initialize: (untyped model) -> void"),
            "Spinel must not keep initialize:(Base): {relation}"
        );
        assert!(
            relation.contains("def first: () -> untyped"),
            "Spinel must not keep first:()->Base?: {relation}"
        );
        assert!(
            relation.contains("def to_a: () -> Array[untyped]"),
            "Spinel must not keep to_a:()->Array[Base]: {relation}"
        );
        assert!(
            relation.contains("def detect: () { (untyped) -> bool } -> untyped"),
            "Spinel must not keep detect:()->Base?: {relation}"
        );
        assert!(
            relation.contains("def to_set: () -> Set[untyped]"),
            "Spinel must not keep to_set:()->Set[Base]: {relation}"
        );
        assert!(
            relation.contains("def each_with_object: (untyped memo) { (untyped, untyped) -> untyped }"),
            "Spinel must not keep each_with_object Base block: {relation}"
        );
        assert!(
            relation.contains("def take_query_lists:"),
            "Spinel must keep #462 take_query_lists: {relation}"
        );
        let leftover: Vec<&str> = relation
            .lines()
            .filter(|l| l.trim_start().starts_with("def ") && l.contains("Base"))
            .collect();
        assert!(
            leftover.is_empty(),
            "Spinel def lines still spell Base: {leftover:?}"
        );
        assert!(
            relation.contains("def find_by: (untyped conditions) -> untyped"),
            "Spinel must not keep find_by:()->Base?: {relation}"
        );
        let connection = files
            .iter()
            .find(|(p, _)| p.ends_with("active_record/connection.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("connection.rbs in spinel tree");
        assert!(
            connection.contains("def self.first: () -> untyped"),
            "Spinel must not keep Base.first:()->Base?: {connection}"
        );
        assert!(
            connection.contains("def self.take: () -> untyped"),
            "Spinel must not keep Base.take:()->Base?: {connection}"
        );
        let conn_leftover: Vec<&str> = connection
            .lines()
            .filter(|l| l.trim_start().starts_with("def ") && l.contains("Base"))
            .collect();
        assert!(
            conn_leftover.is_empty(),
            "Spinel connection def lines still spell Base: {conn_leftover:?}"
        );
        // Shared source still spells Base for Roundhouse Bar B.
        let shared = crate::runtime_files::read_to_string("runtime/ruby/active_record/relation.rbs")
            .expect("shared relation.rbs");
        assert!(
            shared.contains("def initialize: (Base model) -> void"),
            "shared relation.rbs must keep Base for Bar A/B"
        );
        // CRuby/JRuby share `spinel_files` but must NOT get the rewrite.
        let ruby = ruby_runtime_files(&app, fixture).expect("ruby runtime files");
        let ruby_relation = ruby
            .iter()
            .find(|(p, _)| p.ends_with("active_record/relation.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("relation.rbs in ruby tree");
        assert!(
            ruby_relation.contains("def initialize: (Base model) -> void"),
            "CRuby relation.rbs must keep Base: {ruby_relation}"
        );
        assert!(
            ruby_relation.contains("def first: () -> Base?"),
            "CRuby relation.rbs must keep first:()->Base?: {ruby_relation}"
        );
        let ruby_connection = ruby
            .iter()
            .find(|(p, _)| p.ends_with("active_record/connection.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("connection.rbs in ruby tree");
        assert!(
            ruby_connection.contains("def self.first: () -> Base?"),
            "CRuby connection.rbs must keep Base.first:()->Base?: {ruby_connection}"
        );
    }
}
