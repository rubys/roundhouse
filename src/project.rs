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
//! The emit dispatch is host-only because the scaffold/runtime walks
//! read from disk (`runtime/spinel/scaffold/`, `runtime/ruby/`); WASM
//! builds use a different entry point and don't pull this module.

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
             - Node.js 18+ — for the End-to-end suite\n\n\
             ## Build\n\
             ```sh\n\
             spin build\n\
             ```\n\n\
             ## Run\n\
             ```sh\n\
             ./build/bin/blog\n\
             ```\n\n\
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
             - Node.js 18+\n\n\
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
             - Node.js 18+ (for bundling)\n\n\
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
             Browser smoke tests (Playwright). Needs Node.js 18+ and the \
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
pub fn target_files(
    app: &App,
    fixture: &Path,
    target: BuildTarget,
) -> Result<Vec<(String, String)>, String> {
    let files = match target {
        BuildTarget::Blog => blog_files(fixture),
        BuildTarget::Spinel => spinel_files(app, fixture).and_then(spin_shape),
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
    }?;

    // Ruby-family trees ship the framework runtime as verbatim text, so
    // their tree-shake runs here, on the finished file set (after
    // overlays — an overlay-only caller must count as a root), as a
    // text-level pass: see emit::ruby::shake. Other targets shake in IR
    // during emit.
    let files = if matches!(
        target,
        BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby
    ) {
        let synth_shakeable: std::collections::HashSet<String> = app
            .models
            .iter()
            .filter_map(|m| app.schema.tables.get(&m.table.0))
            .flat_map(crate::lower::model_to_library::shakeable_synthesized_names)
            .map(|s| s.as_str().to_string())
            .collect();
        let mut files = files;
        emit::ruby::shake::shake_tree(&mut files, &synth_shakeable, target.as_str());
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
    // "type": "module" matters: global-setup.js is written as ESM, and
    // without it Node loads the file as CommonJS ("exports is not
    // defined in ES module scope").
    files.push((
        "e2e/package.json".to_string(),
        "{\n  \"name\": \"app-e2e\",\n  \"private\": true,\n  \"type\": \"module\",\n  \
         \"description\": \"Playwright end-to-end smoke tests for this archive — see ../README.md\",\n  \
         \"scripts\": {\n    \"test\": \"playwright test\"\n  },\n  \
         \"devDependencies\": {\n    \"@playwright/test\": \"^1.49.0\"\n  }\n}\n"
            .to_string(),
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
/// mode of the `roundhouse` binary.
pub fn write_to_dir(files: &[(String, String)], dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    for (path, content) in files {
        let full = dest.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(&full, content)
            .map_err(|e| format!("write {}: {e}", full.display()))?;
    }
    Ok(())
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
/// Returns the number of files copied, so the caller can report a
/// truthful total.
pub fn write_binary_assets(
    assets: &[(String, Vec<u8>)],
    emitted: &[(String, String)],
    dest: &Path,
) -> Result<usize, String> {
    let mut written = 0usize;
    for (rel, bytes) in assets {
        if emitted.iter().any(|(p, _)| p == rel) {
            continue;
        }
        let full = dest.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(&full, bytes).map_err(|e| format!("write {}: {e}", full.display()))?;
        written += 1;
    }
    Ok(written)
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
fn ruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app, fixture)?;

    files.retain(|(p, _)| p != "runtime/db.rb");

    // Same swap as db.rb below: the flat walk picked up BOTH halves of
    // the keyed-digest split, and the CRuby/JRuby trees want the OpenSSL
    // one at the shared path. The spinel half reaches sp_crypto through
    // FFI declarations these trees can't compile.
    files.retain(|(p, _)| p != "runtime/message_digest.rb");

    // The scaffold's tailwind seed only belongs in trees whose SOURCE
    // app has stylesheets to build (real-blog's Propshaft setup). A
    // no-stylesheet app (the Roda + Sequel exemplar renders
    // inline-styled HTML) drops it, and the emitted Rakefile's
    // existence-conditional `assets` task then skips the whole
    // npm/tailwind pipeline — `rake dev` boots with no Node at all.
    if app.stylesheets.is_empty() {
        files.retain(|(p, _)| p != "app/assets/tailwind.css");
    }
    for (path, content) in files.iter_mut() {
        if path == "runtime/message_digest_cruby.rb" {
            *path = "runtime/message_digest.rb".to_string();
        }
        if path == "runtime/db_cruby.rb" {
            *path = "runtime/db.rb".to_string();
            // The CRuby/JRuby trees resolve the temporal intrinsics
            // (`ActiveSupport.db_now` in fill_timestamps,
            // `parse_db_time` in temporal readers) via the overlay's
            // ActiveSupport module. The server boot requires it from
            // main.rb, but the emitted test bootstrap
            // (test/test_helper.rb, shared verbatim with the spinel
            // tree, which lacks the file — spinel#1661) does not.
            // Chain it off db.rb — the one CRuby-only require every
            // persistence-touching bootstrap already loads — at
            // materialization time, since the source-tree relative
            // path differs from the emitted-tree one.
            content.insert_str(
                0,
                "require_relative \"active_support_time_parsing\"\n",
            );
        }
    }

    // Tep is a spinel-only transport (FFI HTTP server). The CRuby
    // target uses Puma + Rack via the ruby_overlay; nothing in its
    // boot path requires Tep, and the unsubstituted @TEP_SPHTTP_O@
    // placeholder in net.rb would confuse anyone exploring the tree.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    // Gem façades are SPINEL-ONLY: spinel AOT can't link the native
    // gems, so it ships loudly-raising stubs. On the CRuby/JRuby path the
    // real gems (markly / nokogiri / mail) ARE available — main.rb
    // guarded-requires them — and app code renders through them at
    // request time (Markdowner.to_html behind User#linkified_about is
    // read-path: `/u/:username` markdown-renders the profile bio live).
    // The inherited façade reopens those modules and REDEFINES their
    // methods to raise, shadowing the real gems. Neutralize it here to a
    // no-op so the `require_relative "runtime/gem_facades"` anchor still
    // resolves without clobbering the real implementations.
    for (path, content) in files.iter_mut() {
        if path == "runtime/gem_facades.rb" {
            // Not a bare no-op: the anchor has to MEAN something. main.rb
            // guarded-requires these gems too, but a test run never loads
            // main.rb — `test/test_helper.rb` builds its own require
            // chain — so a body reached only from the test harness saw an
            // anchor that resolved to an empty file and a constant that
            // was never defined. campfire's `users.yml` is the first such
            // body: `password_digest: <%= BCrypt::Password.create(…) %>`
            // lands in `UsersFixtures._fixtures_load!`, whose only caller
            // is the harness. Requires are idempotent, so doing them here
            // makes every consumer of the anchor self-sufficient without
            // changing main.rb's behavior.
            *content = "# Gem façades are spinel-only (no native gems there). On the CRuby\n\
                        # path the real gems ARE available, so this file guarded-requires\n\
                        # them rather than shadowing them with raising stubs. Guarded because\n\
                        # an app that uses none of them (the blog) must boot without them\n\
                        # installed. Keep this list in step with main.rb's. (JRuby swaps in\n\
                        # the commonmark-java Markly shim instead — see jruby_runtime_files.)\n\
                        [\"bcrypt\", \"htmlentities\", \"rotp\", \"markly\", \"nokogiri\", \"parslet\",\n\
                        \x20\"typeid\", \"rqrcode\", \"SVG/Graph/TimeSeries\"].each do |gem_name|\n\
                        \x20\x20begin\n\
                        \x20\x20\x20\x20require gem_name\n\
                        \x20\x20rescue LoadError\n\
                        \x20\x20\x20\x20nil\n\
                        \x20\x20end\n\
                        end\n"
                .to_string();
        }
    }

    // Same reasoning for the extras façades (Sponge): CRuby has the real
    // net/https / resolv / ipaddr stdlib and the vendored source runs as
    // written, so put the verbatim source-shape emit back over the
    // scaffold base's raising façade.
    emit::ruby::restore_extras_facades(&mut files, app);

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
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
    apply_cable_strip(&mut files, app)?;
    apply_makefile_test_list(&mut files, app);
    Ok(files)
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
/// hand-written one did.
fn apply_views_aggregator(files: &mut [(String, String)]) {
    use std::fmt::Write;

    let mut views: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| p.starts_with("app/views/") && p.ends_with(".rb"))
        .collect();
    if views.is_empty() {
        return;
    }
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
    const HARDCODED: &str = "  def self.instantiate_controller(sym)\n    case sym\n    when :articles then ArticlesController.new\n    when :comments then CommentsController.new\n    end\n  end";

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
            let stem = crate::naming::underscore(class);
            writeln!(
                arms,
                "    when :{sym} then require_relative \"app/controllers/{stem}\"; {class}.new"
            )
            .unwrap();
        } else {
            writeln!(arms, "    when :{sym} then {class}.new").unwrap();
        }
    }
    if arms.is_empty() {
        return;
    }
    let generated =
        format!("  def self.instantiate_controller(sym)\n    case sym\n{arms}    end\n  end");

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
        "    controller = case matched.controller\n{test_arms}                 end"
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

/// "jruby" archive: byte-identical to the "ruby" tree except the SQLite
/// backend. Same layering as `ruby_runtime_files` — spinel files +
/// ruby_overlay (Puma + Rack `config.ru`, all of which run unchanged on
/// the JVM) — but the Db shim swap installs the JDBC-backed
/// `runtime/db_jruby.rb` as `runtime/db.rb` instead of the CRuby
/// gem-backed `db_cruby.rb`. The `sqlite3` gem is a C extension with no
/// JRuby build, so JRuby reaches SQLite over JDBC. The emitted app/,
/// config/, and framework runtime are identical to the CRuby target —
/// JRuby is a deployment (VM) variant, not a source variant.
fn jruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app, fixture)?;

    // Keyed digests: JRuby has OpenSSL, so it takes the same swap the
    // CRuby target does — drop the spinel stub and promote the OpenSSL
    // backend. Without this the JVM tree got a `raise`-only
    // MessageDigest and the boot died where the aggregator requires it.
    files.retain(|(p, _)| p != "runtime/message_digest.rb");
    for (path, _) in files.iter_mut() {
        if path == "runtime/message_digest_cruby.rb" {
            *path = "runtime/message_digest.rb".to_string();
        }
    }

    // Db shim swap: drop the FFI `runtime/db.rb` and the CRuby gem
    // backend, then promote the JDBC backend into `runtime/db.rb`.
    // `db_jruby.rb` is excluded from `spinel_files`' base set, so read
    // it from disk and inject it here (mirrors the gem swap the CRuby
    // target does to `db_cruby.rb`).
    files.retain(|(p, _)| p != "runtime/db.rb" && p != "runtime/db_cruby.rb");
    let db_jruby = fs::read_to_string("runtime/spinel/db_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/db_jruby.rb: {e}"))?;
    // Chain the temporal-intrinsics module off db.rb, same as the
    // CRuby swap above (the emitted test bootstrap doesn't require it;
    // see ruby_runtime_files).
    files.push((
        "runtime/db.rb".to_string(),
        format!("require_relative \"active_support_time_parsing\"\n{db_jruby}"),
    ));

    // Gemfile gem swap: the committed scaffold Gemfile is MRI-only
    // (`gem "sqlite3"`, a C extension with no JRuby build), so its frozen
    // lock stays valid for the CRuby/Spinel toolchain jobs. The JRuby
    // tree reaches SQLite over JDBC, so rewrite that one line to the
    // Xerial driver here — the emitted tree's `bundle install` then
    // resolves a fresh JRuby lock (mirrors the `db_cruby.rb` swap above).
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

    // Tep is a spinel-only transport (FFI HTTP server); JRuby uses Puma
    // + Rack via the ruby_overlay, same as the CRuby target.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    // Markly shim: markly is cmark-gfm C bindings with no JRuby build,
    // so this tree implements the markly contract over commonmark-java
    // (reference-conformant: scripts/markly-conformance, vectors
    // generated from the real gem under CRuby). The shim rides the
    // gem_facades require anchor: swap the scaffold base's raising
    // façade for a loader that provides Markly via the shim, while
    // Nokogiri (java platform gem) and Mail (pure Ruby) resolve to the
    // real gems — the JRuby analogue of the CRuby neutralization in
    // `ruby_runtime_files`. Apps that never require the anchor (blog)
    // never load the shim, so the jars stay optional.
    let markly_shim = fs::read_to_string("runtime/spinel/markly_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/markly_jruby.rb: {e}"))?;
    files.push(("runtime/markly_jruby.rb".to_string(), markly_shim));
    for (path, content) in files.iter_mut() {
        if path == "runtime/gem_facades.rb" {
            *content = "# On the JRuby tree Markly is provided by the commonmark-java shim\n\
                        # (markly_jruby.rb); nokogiri (java platform gem) and mail (pure Ruby)\n\
                        # are the real gems, loaded by main.rb's guarded requires. This file\n\
                        # keeps the `require_relative \"runtime/gem_facades\"` anchor resolving.\n\
                        require_relative \"markly_jruby\"\n"
                .to_string();
        }
    }

    // The shim's jars (commonmark-java + org.nibor.autolink) can't ship
    // through the text-only emit; the tree fetches them from Maven
    // Central on demand.
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

    // Extras façades: same reasoning as the CRuby tree — Sponge's
    // vendored source is pure stdlib (net/https, resolv, ipaddr), all
    // of which run on the JVM — so restore the verbatim emit over the
    // scaffold base's raising façade.
    emit::ruby::restore_extras_facades(&mut files, app);

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

    // Controllers with the layout wrap — same pairing as the CRuby
    // tree: this main.rb ships `controller.body` verbatim (see
    // ruby_runtime_files).
    files.extend(sort_files(emit::ruby::emit_lowered_controllers_with_layout(app)));

    // The scaffold README → SPECIMEN rename already happened in `spinel_files`.
    let mut files = dedupe_last_wins(files);
    // Same lazy-dispatch rewrite as the CRuby tree — the ruby_overlay
    // main.rb superseded the base's eagerly-rewritten one at dedupe.
    apply_controller_dispatch(&mut files, app, true);
    // Same cable strip as the CRuby tree (this walk re-added cable.rb +
    // the config.ru seams; the Gemfile trim already ran in spinel_files).
    apply_cable_strip(&mut files, app)?;
    apply_makefile_test_list(&mut files, app);
    Ok(files)
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
    let mut files = spinel_files(app, fixture)?;
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
fn spinel_files(app: &App, fixture: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();

    walk_dir_into(Path::new("runtime/spinel/scaffold"), "", &mut files)?;

    walk_dir_partitioned(
        Path::new("runtime/spinel/test"),
        "test/",
        "sig/test/",
        &mut files,
    )?;

    walk_dir_flat(Path::new("runtime/spinel"), &["rb"], "runtime/", &mut files)?;

    // Temporal-intrinsics sidecar — the flat walk above picks only .rb,
    // and spinel's strict unresolved-call gate needs `parse_db_time`'s
    // `String?` param typed to compile the nil-guard narrow.
    {
        let rbs = fs::read_to_string("runtime/spinel/active_support_time_parsing.rbs")
            .map_err(|e| format!("read runtime/spinel/active_support_time_parsing.rbs: {e}"))?;
        files.push((
            "sig/runtime/active_support_time_parsing.rbs".to_string(),
            rbs,
        ));
    }

    // Duration sidecar — pins @seconds Integer so ago/from_now stay
    // Time-typed under AOT inference (an untyped @seconds widens the
    // temporal arithmetic to poly against the Time-typed C return).
    {
        let rbs = fs::read_to_string("runtime/spinel/active_support_duration.rbs")
            .map_err(|e| format!("read runtime/spinel/active_support_duration.rbs: {e}"))?;
        files.push(("sig/runtime/active_support_duration.rbs".to_string(), rbs));
    }

    // CGI shim sidecar — spinel has no stdlib CGI; the flat walk emits
    // runtime/cgi_spinel.rb but only .rb, so pair its typing contract (escape ->
    // String, parse -> Hash[String, Array[String]]) here.
    {
        let rbs = fs::read_to_string("runtime/spinel/cgi_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/cgi_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/cgi_spinel.rbs".to_string(), rbs));
    }

    // ERB::Util shim sidecar — same story as CGI: spinel has no stdlib
    // ERB, the flat walk emits runtime/erb_spinel.rb, and the .rbs pins
    // html_escape's String return so callers concatenating it stay typed.
    {
        let rbs = fs::read_to_string("runtime/spinel/erb_spinel.rbs")
            .map_err(|e| format!("read runtime/spinel/erb_spinel.rbs: {e}"))?;
        files.push(("sig/runtime/erb_spinel.rbs".to_string(), rbs));
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

    // Vendored Tep transport (FFI HTTP server). Both .rb files and
    // sphttp.c (precompiled to sphttp.o at transpile-post time).
    // Recursive walk picks the whole subtree.
    walk_dir_into(Path::new("runtime/spinel/tep"), "runtime/tep/", &mut files)?;

    for sub in [
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
    ] {
        walk_dir_partitioned(
            &Path::new("runtime/ruby").join(sub),
            &format!("runtime/{sub}/"),
            &format!("sig/runtime/{sub}/"),
            &mut files,
        )?;
    }
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
        "inflector",
        "inflector_ext",
        "json_builder",
        "active_support_ext",
        "params",
        "action_text",
        "active_storage",
    ] {
        let rb = Path::new("runtime/ruby").join(format!("{stem}.rb"));
        let content = fs::read_to_string(&rb)
            .map_err(|e| format!("read {}: {e}", rb.display()))?;
        files.push((format!("runtime/{stem}.rb"), content));
        let rbs = Path::new("runtime/ruby").join(format!("{stem}.rbs"));
        if rbs.exists() {
            let rbs_content = fs::read_to_string(&rbs)
                .map_err(|e| format!("read {}: {e}", rbs.display()))?;
            files.push((format!("sig/runtime/{stem}.rbs"), rbs_content));
        }
    }

    files.extend(sort_files(emit::ruby::emit_spinel(app)));

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
    emit::ruby::apply_extras_facades(&mut files);

    let js = fixture.join("app/javascript");
    if js.exists() {
        walk_dir_into(&js, "app/javascript/", &mut files)?;
    }
    let public = fixture.join("public");
    if public.exists() {
        walk_dir_into(&public, "public/", &mut files)?;
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
    apply_views_aggregator(&mut files);
    apply_models_aggregator(&mut files);
    // All three scaffold targets (spinel + the ruby/jruby trees derived
    // from this set) ship the comprehensive scaffold README as SPECIMEN.md,
    // freeing README.md for the generated quick-start `ensure_readme`
    // injects. ruby_overlay carries no README, so this is the only place
    // the rename needs to happen for any of them.
    scaffold_readme_to_specimen(&mut files);
    apply_gemfile_trim(&mut files, app, fixture);
    apply_test_gem_wiring(&mut files);
    Ok(files)
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
fn patch_mocha_lifecycle(helper: &mut String) {
    const SETUP: &str = "  def setup\n    SchemaSetup.reset! if defined?(SchemaSetup)";
    const TEARDOWN: &str = "  def teardown\n  end";

    if helper.contains("Mocha::API") {
        return;
    }
    if let Some(at) = helper.find(SETUP) {
        let with_include =
            format!("  include Mocha::API\n\n  def setup\n    mocha_setup\n    SchemaSetup.reset! if defined?(SchemaSetup)");
        helper.replace_range(at..at + SETUP.len(), &with_include);
    }
    if let Some(at) = helper.find(TEARDOWN) {
        let verified = "  def teardown\n    mocha_verify\n  ensure\n    mocha_teardown\n  end";
        helper.replace_range(at..at + TEARDOWN.len(), verified);
    }
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
    const TEST_GEMS: [(Marker, &str, &str); 2] = [
        (Marker::Constant("WebMock"), "webmock", "webmock/minitest"),
        (Marker::AnyText(&[".stubs(", ".expects(", ".any_instance"]), "mocha", "mocha/api"),
    ];

    let mut needed: Vec<(&str, &str)> = Vec::new();
    for (marker, gem, entry) in TEST_GEMS {
        let demanded = files.iter().any(|(p, c)| {
            p.starts_with("test/")
                && p.ends_with(".rb")
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
        for (_, entry) in &needed {
            let line = format!("require {entry:?}");
            if !helper.contains(&line) {
                helper.insert_str(0, &format!("{line}\n"));
            }
        }
        if needed.iter().any(|(gem, _)| *gem == "mocha") {
            patch_mocha_lifecycle(helper);
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

/// De-blog the scaffold Makefile's `SPINEL_TESTS` list for the CRuby /
/// JRuby trees, where it drives `make cruby-test` over the app's own
/// emitted tests. The scaffold hard-codes the blog's four stems, so
/// every other app shipped a target naming files it does not have —
/// campfire emits 52 and named none of them.
///
/// NOT applied in `spinel_files`, even though that is where the
/// Makefile arrives: the SPINEL target rewrites the same block from its
/// own `lane` (see `spin_shape`), which is a different selection of
/// tests, and it anchors on the blog list with a hard error if the
/// anchor is missing. Running this first consumed that anchor and took
/// `build-site` down. Two lanes, two owners, and the split is by TARGET
/// — so this has to sit on the CRuby side of the fork, not upstream of
/// it.
///
/// Derived from what the EMITTER produced rather than from
/// `app.test_modules`: re-deriving the stems from the source
/// declarations would be a second copy of `test_file_stem`'s naming
/// rules — including the namespace flatten
/// `Rooms::ClosedsControllerTest` → `rooms_closeds_controller` — and a
/// stale one the first time those rules change. It also cannot be a
/// scan of the FINAL file set: the scaffold drops the framework
/// runtime's own `test/models/*_test.rb` at the same paths (they
/// `require "models/article"` and are not runnable standalone), and
/// `article_broadcasts_test` rode along into the blog's list that way.
fn apply_makefile_test_list(files: &mut [(String, String)], app: &App) {
    let mut stems: Vec<String> = emit::ruby::emit_spinel(app)
        .iter()
        .filter_map(|f| {
            let p = f.path.to_str()?;
            let stem = p.strip_suffix(".rb")?;
            (stem.ends_with("_test")
                && (stem.starts_with("test/models/") || stem.starts_with("test/controllers/")))
            .then(|| stem.to_string())
        })
        .collect();
    stems.sort();
    apply_makefile_test_list_stems(files, &stems);
}

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
///    recursion.
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
    src.lines().any(|line| {
        if line.trim_start().starts_with('#') {
            return false;
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
                || (b.get(tail) == Some(&b':') && b.get(tail + 1) == Some(&b':'));
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

/// `write_bundled_requires` in the value-passing shape `target_files`'
/// match arms want.
fn with_bundled_requires(mut files: Vec<(String, String)>) -> Vec<(String, String)> {
    write_bundled_requires(&mut files);
    files
}

/// Constant → bundled library that provides it. One table, read by
/// both the pass that writes the requires and the gate that checks a
/// tree for missing ones — a second copy is how the rule drifts.
const BUNDLED: [(&str, &str); 10] = [
    ("Base64", "base64"),
    ("CSV", "csv"),
    ("Digest", "digest"),
    ("ERB", "erb"),
    ("JSON", "json"),
    ("OptionParser", "optparse"),
    ("Pathname", "pathname"),
    ("Set", "set"),
    ("StringIO", "stringio"),
    ("StringScanner", "strscan"),
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
        // The program defines the constant itself, so the bundled
        // library is not what the name refers to.
        if files
            .iter()
            .any(|(p, c)| p.ends_with(".rb") && defines_constant(c, konst))
        {
            continue;
        }
        let require_line = format!("require {feature:?}");
        for (i, (path, content)) in files.iter().enumerate() {
            if path.ends_with(".rb")
                && names_constant(content, konst)
                && !content.contains(&require_line)
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
        files[i].1.insert_str(0, &format!("{require_line}\n"));
    }
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
    for entry in files.iter_mut() {
        if !entry.0.starts_with("test/") || !entry.0.ends_with("_test.rb") {
            continue;
        }
        let base = entry.0.rsplit('/').next().unwrap().to_string();
        let top_level = entry.0 == format!("test/{base}");
        // A Minitest-shaped file is CRuby-only whatever else it holds;
        // otherwise the lane is decided by the same structural check
        // `test_class_and_count` applies below, so the two cannot
        // disagree. See `lane_test_class` for what this replaced.
        let in_lane = !entry.1.lines().any(declares_minitest_class)
            && entry.1.lines().any(|l| lane_test_class(l).is_some());
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
        if in_lane {
            let (class, n) = test_class_and_count(&entry.1, &new_path)?;
            lane.push((new_path, class, n));
        }
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

    write_bundled_requires(&mut files);
    // 6. Package manifest + compile root.
    let mut manifest = String::from(
        "# spin manifest — generated by Roundhouse (spinel target).\n\
         # An application needs no [package] identity. Dependencies go here:\n\
         #   [dependencies]\n\
         #   name = { path = \"../spinel-name\" }\n",
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
            "\t$(SPINEL) --rbs sig $< -o $@",
            "\t$(SPINEL) $(RBS_FLAG) $< -o $@",
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
    // With package dependencies, the raw `$(SPINEL) main.rb` lane can't
    // resolve `require "bcrypt"` (no -I / carried-C link) — delegate the
    // binary build to `spin build`, which owns dependency resolution and
    // the package-C cache, and copy the result to the path the
    // bench/e2e scripts expect. (The per-test compile recipe keeps the
    // raw lane: dep-carrying apps run tests via `spin test`.)
    if needs_bcrypt {
        let dep_patches: [(&str, &str); 2] = [
            ("SPINEL ?= spinel", "SPINEL ?= spinel\nSPIN   ?= spin"),
            (
                "\t$(SPINEL) main.rb $(RBS_FLAG) -o $@",
                "\t$(SPIN) build\n\tcp build/bin/blog $@",
            ),
        ];
        for (from, to) in dep_patches {
            if !makefile.1.contains(from) {
                return Err(format!(
                    "spin_shape: Makefile dep-patch pattern not found (scaffold \
                     Makefile changed?): {:?}",
                    &from[..from.len().min(60)]
                ));
            }
            makefile.1 = makefile.1.replacen(from, to, 1);
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
    if !rest.contains("< TestBase") && !rest.contains("< ActionDispatch::IntegrationTest") {
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
    let n = content.matches("def test_").count();
    if n == 0 {
        return Err(format!(
            "spin_shape: {path}: no test methods — would be a vacuous test program"
        ));
    }
    Ok((class, n))
}

/// Rewrite the requires of a file moving `old_dir` → `new_dir` inside
/// the virtual file set. `require_relative` targets are re-based;
/// bare `require "x"` (which the Makefile lanes resolved with `-I`
/// flags spin does not pass) becomes `require_relative` when `x.rb`
/// exists under test/, runtime/, app/, or the root. Anything else
/// (stdlib) is left bare for the require gate to judge. Lines with
/// trailing comments or non-literal arguments pass through untouched.
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

/// Walk `src` recursively, routing `.rb` files under `rb_prefix` and
/// `.rbs` files under `rbs_prefix`. Other extensions and dotfiles are
/// skipped. Splits `runtime/ruby/<sub>/` between the load-path tree
/// (`runtime/`) and the typed sidecar tree (`sig/runtime/`) in one pass.
fn walk_dir_partitioned(
    src: &Path,
    rb_prefix: &str,
    rbs_prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("missing {}/", src.display()));
    }
    let mut stack: Vec<(PathBuf, String)> = vec![(src.to_path_buf(), String::new())];
    while let Some((dir, sub)) = stack.pop() {
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
            let nested = format!("{sub}{name_str}");
            if ty.is_dir() {
                stack.push((path, format!("{nested}/")));
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let prefix = match ext {
                "rb" => rb_prefix,
                "rbs" => rbs_prefix,
                _ => continue,
            };
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            out.push((format!("{prefix}{nested}"), content));
        }
    }
    Ok(())
}

/// Walk `src` non-recursively, collecting only files whose extension
/// is in `exts`. Used to gather `runtime/spinel/*.rb` without
/// recursing into `runtime/spinel/{scaffold,test}` (those are walked
/// separately into different output prefixes).
fn walk_dir_flat(
    src: &Path,
    exts: &[&str],
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext_match = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| exts.contains(&e))
            .unwrap_or(false);
        if !ext_match {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("non-utf8 filename: {}", path.display()))?;
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        out.push((format!("{prefix}{name}"), content));
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

    copy_site_assets(out)?;
    copy_create_blog(out)?;

    let mut app =
        ingest_app(fixture).map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    // Analyze + the same post-analyze shared lowerings as the
    // single-target driver; the site build has no diagnostic surface,
    // so the residue is dropped.
    let _ = crate::session::analyze_and_lower(&mut app);

    for target in BuildTarget::ALL {
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
    fn trim_gemfile_drops_assets_and_websocket_blocks() {
        let gemfile = fs::read_to_string("runtime/spinel/scaffold/Gemfile").unwrap();
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
        });

        let mut files = vec![
            (
                "main.rb".to_string(),
                fs::read_to_string("runtime/spinel/scaffold/ruby_overlay/main.rb").unwrap(),
            ),
            (
                "test/test_helper.rb".to_string(),
                fs::read_to_string("runtime/spinel/test/test_helper.rb").unwrap(),
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
            fs::read_to_string("runtime/spinel/scaffold/ruby_overlay/main.rb").unwrap(),
        )];
        apply_controller_dispatch(&mut plain, &app, false);
        assert!(!plain[0].1.contains("Current.reset"));
    }

    #[test]
    fn strip_cable_from_config_ru_removes_all_three_seams() {
        let config_ru =
            fs::read_to_string("runtime/spinel/scaffold/ruby_overlay/config.ru").unwrap();
        let out = strip_cable_from_config_ru(&config_ru).unwrap();
        assert!(!out.contains("require_relative \"cable\""));
        assert!(!out.contains("Cable"), "no Cable constant may survive");
        assert!(!out.contains("/cable"));
        assert!(!out.contains("rack.hijack"));
        // The serving core survives intact.
        assert!(out.contains("require_relative \"main\""));
        assert!(out.contains("Db.with_connection { Main.run_rack(env) }"));
        assert!(out.contains("run app"));
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
    fn a_stale_seed_file_is_REPLACED_not_preserved() {
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
             \t$(SPINEL) --rbs sig $< -o $@\n\
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
        assert!(mk.contains("$(SPINEL) $(RBS_FLAG) $< -o $@"));
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
             \t$(SPINEL) --rbs sig $< -o $@\n\
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
             \t$(SPINEL) --rbs sig $< -o $@\n\
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
            // not what `ERB.new` refers to.
            ("runtime/erb.rb".to_string(), "module ERB\nend\n".to_string()),
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
}
