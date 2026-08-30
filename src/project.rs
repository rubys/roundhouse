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

    // `IPAddr`: the CRuby/JRuby trees have Ruby's own, and it is a
    // superset of the port. Same shape as the db.rb swap above — one
    // require path, target-appropriate implementation — but written as
    // a one-line file rather than a rename, because the require the
    // emitted models carry is `require_relative ".../runtime/ipaddr"`
    // and that path has to keep resolving.
    //
    // THIS IS NOT TIDINESS. Something on the CRuby side already loads
    // the stdlib's ipaddr (net/http reaches it through resolv), and two
    // definitions of `IPAddr::InvalidAddressError` with different
    // superclasses is a `TypeError: superclass mismatch` at REQUIRE
    // time — campfire's suite went 219/240 to 0/240, every file dying
    // on the same line.
    for (path, content) in files.iter_mut() {
        if path == "runtime/ipaddr.rb" {
            *content = "# Ruby's own ipaddr — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/ipaddr.rb exists for the targets\n\
                        # that have no stdlib to reach for; this tree has one, and\n\
                        # defining a second IPAddr beside it is a superclass mismatch\n\
                        # at require time.\n\
                        require \"ipaddr\"\n"
                .to_string();
        }
        // `Zlib`: same swap, and here it is a correctness one as much
        // as a speed one. The port computes the checksum in Ruby a bit
        // at a time; CRuby's is zlib's own C. Both answer the same
        // number by construction (the port IS CRC-32/ISO-HDLC), so the
        // tree that has the real one should use it.
        if path == "runtime/zlib.rb" {
            *content = "# Ruby's own zlib — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/zlib.rb exists for the targets\n\
                        # that have no zlib to bind to.\n\
                        require \"zlib\"\n"
                .to_string();
        }
        // `Resolv`: the same swap as ipaddr, and for both of ipaddr's
        // reasons at once. net/http loads the stdlib's resolver over
        // here, so a second `Resolv` beside it is a superclass mismatch
        // at require time; and the app's tests stub
        // `Resolv.getaddresses`, which only reaches the guard if the
        // guard dispatches to the class mocha patched.
        if path == "runtime/resolv.rb" {
            *content = "# Ruby's own resolv — see `project::ruby_runtime_files`.\n\
                        # The port at runtime/ruby/resolv.rb exists for the targets\n\
                        # that have no resolver to bind to.\n\
                        require \"resolv\"\n"
                .to_string();
        }
    }

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

    // `Module#delegate` for the GEMS in the emitted Gemfile — see the
    // file's own header. Pushed at the fork, never named in the
    // `spinel_files` stems list: a reopen of `Module` that defines
    // methods from a computed name is exactly what the strict targets
    // cannot compile, and nothing in this tree's own emitted code needs
    // it (an app's `delegate` is lowered at ingest).
    files.push((
        "runtime/module_delegate.rb".to_string(),
        fs::read_to_string("runtime/spinel/module_delegate.rb")
            .map_err(|e| format!("read runtime/spinel/module_delegate.rb: {e}"))?,
    ));

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
            *content = format!(
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
                 {}",
                gem_require_block(&[])
            );
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
    apply_runtime_gem_wiring(&mut files);
    // AGAIN, on purpose, and this time PERFORMING them. `spinel_files`
    // appended a commented-out block to the spinel tree's boot.rb (that
    // target cannot perform a mixin — see `apply_module_mixins`), and the
    // overlay's own boot.rb then replaced that file wholesale at
    // `dedupe_last_wins` above, taking the block with it. Each final tree
    // gets the block exactly once; this is the ruby family's turn, and
    // the ruby family is the one that can run the lines.
    apply_module_mixins(&mut files, app, MixinForm::ExplicitReceiver);
    Ok(files)
}

/// Rewrite `runtime/cable.rb`'s `Cable.build_connection` factory from
/// the ingested app, so a `/cable` handshake is identified by running
/// the app's OWN `ApplicationCable::Connection#connect`.
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
/// default arm in `cable.rb` connects anonymously, which is what an app
/// that never asked for identity needs. `ApplicationCable::Connection`
/// is Rails' fixed convention name, the same convention
/// `runtime/action_cable.rb` already encodes by giving
/// `ActionCable::Connection::Base` a body.
///
/// Written as a re-appliable SPAN replace between two markers, for the
/// reason `patch_harness_dispatch` gives: a match on the default body's
/// text would freeze this at today's spelling while `cable.rb` moved on.
fn apply_cable_connection(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "  # >>> generated: cable-connection\n";
    const TAIL: &str = "  # <<< generated: cable-connection\n";
    const CONNECTION_CLASS: &str = "ApplicationCable::Connection";

    if !app
        .library_classes
        .iter()
        .any(|lc| lc.name.0.as_str() == CONNECTION_CLASS)
    {
        return;
    }
    let generated = format!(
        "{HEAD}  def self.build_connection(cookies)\n    {CONNECTION_CLASS}.new(cookies)\n  end\n{TAIL}"
    );
    for (path, content) in files.iter_mut() {
        if !path.ends_with("cable.rb") || path.ends_with("action_cable.rb") {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
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

    if app.module_mixins.is_empty() {
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
    for (path, content) in files.iter_mut() {
        if path == "runtime/message_digest_cruby.rb" {
            *path = "runtime/message_digest.rb".to_string();
        }
        // JRuby has Ruby's own ipaddr too — same swap, same reason
        // (a second `IPAddr::InvalidAddressError` is a superclass
        // mismatch at require time). See `ruby_runtime_files`.
        if path == "runtime/ipaddr.rb" {
            *content = "require \"ipaddr\"\n".to_string();
        }
        // Same for zlib — the JVM tree has Ruby's own.
        if path == "runtime/zlib.rb" {
            *content = "require \"zlib\"\n".to_string();
        }
        // …and for resolv, which the JVM tree also has. See
        // `ruby_runtime_files` for why this one is not optional: the
        // app's tests stub `Resolv.getaddresses`.
        if path == "runtime/resolv.rb" {
            *content = "require \"resolv\"\n".to_string();
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
    // `Module#delegate` for the GEMS in the emitted Gemfile — see the
    // file's own header. Pushed at the fork, never named in the
    // `spinel_files` stems list: a reopen of `Module` that defines
    // methods from a computed name is exactly what the strict targets
    // cannot compile, and nothing in this tree's own emitted code needs
    // it (an app's `delegate` is lowered at ingest).
    files.push((
        "runtime/module_delegate.rb".to_string(),
        fs::read_to_string("runtime/spinel/module_delegate.rb")
            .map_err(|e| format!("read runtime/spinel/module_delegate.rb: {e}"))?,
    ));

    let markly_shim = fs::read_to_string("runtime/spinel/markly_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/markly_jruby.rb: {e}"))?;
    files.push(("runtime/markly_jruby.rb".to_string(), markly_shim));

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

    // The shim's jars are FETCHED, not shipped (bin/fetch-jars), so
    // requiring it in a tree that never renders markdown is a LoadError
    // on a dependency that app does not have. The anchor below is
    // required from boot.rb now — it used to be reached only when some
    // model named a gem constant, which is what kept the blog's JRuby
    // tree booting with no jars — so the shim require has to be
    // conditional on the app instead of on the require graph's shape.
    let needs_markly = files
        .iter()
        .any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && names_constant(c, "Markly"));
    let markly_require = if needs_markly {
        "require_relative \"markly_jruby\"\n"
    } else {
        "# (this app never names Markly, so the commonmark-java shim — and\n\
         # the jars bin/fetch-jars pulls — stay out of its require graph)\n"
    };
    for (path, content) in files.iter_mut() {
        if path == "runtime/gem_facades.rb" {
            *content = format!(
                "# On the JRuby tree Markly is provided by the commonmark-java shim\n\
                 # (markly_jruby.rb); every other gem below is the real one (nokogiri\n\
                 # ships a java platform build, the rest are pure Ruby). This file is\n\
                 # also the `require_relative \"runtime/gem_facades\"` anchor and the\n\
                 # one guarded-require list for this tree — boot.rb requires it rather\n\
                 # than carrying a second copy.\n\
                 {}\
                 require_relative \"module_delegate\"\n\
                 {}",
                markly_require,
                gem_require_block(&["markly"])
            );
        }
    }

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
    apply_runtime_gem_wiring(&mut files);
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

    // And for `module_delegate.rb` — ActiveSupport's `Module#delegate`,
    // supplied for the GEMS in the emitted Gemfile (see the file's own
    // header). A reopen of `Module` that defines methods from a computed
    // name is exactly what the strict targets cannot compile, and
    // nothing in an emitted tree's own code needs it: an app's
    // `delegate` is lowered at ingest. Injected only by
    // `ruby_runtime_files` / `jruby_runtime_files`.
    files.retain(|(p, _)| p != "runtime/module_delegate.rb");

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
        // actionpack's MIME registry, ported (see runtime/ruby/mime.rb).
        // Every target: nothing else in a tree defines `Mime`.
        "mime",
        // The stdlib class the strict targets have no stdlib for. The
        // ruby family reaches Ruby's own through a bare `require`, so
        // this one only has to exist where that does not.
        "ipaddr",
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
        // `Zlib.crc32`, same arrangement as ipaddr: ported for the
        // targets with no zlib to bind to, swapped for Ruby's own on
        // the CRuby/JRuby trees below.
        "zlib",
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

    let mut files = dedupe_last_wins(files);
    // De-blog the scaffold for whatever app was ingested: regenerate the
    // `app/views.rb` aggregator from the emitted view set and rewrite
    // `main.rb`'s `instantiate_controller` from the app's route table
    // (eager arms — AOT has no lazy-require escape hatch). Runs here, in
    // the shared base, so all three scaffold targets inherit it; the
    // CRuby/JRuby trees re-apply the dispatch in lazy flavor to the
    // ruby_overlay main.rb that supersedes this one.
    apply_controller_dispatch(&mut files, app, false);
    // Identity for the `/cable` handshake, in the shared base like the
    // dispatch above. Only the SPINEL tree runs the result: the
    // CRuby/JRuby trees carry this `runtime/cable.rb` too but never
    // require it — their config.ru requires the overlay's own top-level
    // `cable.rb`, which has its own identity path and its own test.
    apply_cable_connection(&mut files, app);
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
    const TEST_GEMS: [(Marker, &str, &str); 3] = [
        (Marker::Constant("WebMock"), "webmock", "webmock/minitest"),
        (Marker::AnyText(&[".stubs(", ".expects(", ".any_instance"]), "mocha", "mocha/api"),
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
        for (_, entry) in &needed {
            let line = format!("require {entry:?}");
            if !helper.contains(&line) {
                helper.insert_str(0, &format!("{line}\n"));
            }
        }
        if needed.iter().any(|(gem, _)| *gem == "mocha") {
            patch_mocha_lifecycle(helper);
        }
        if needed.iter().any(|(gem, _)| *gem == "webmock") {
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
/// `spinel_files`, beside `apply_makefile_test_list` for the reason that
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
    "nokogiri",
    "parslet",
    "typeid",
    "rqrcode",
    "SVG/Graph/TimeSeries",
    "sentry-ruby",
    "rails-html-sanitizer",
];

/// The guarded-require block, minus any gem this tree provides another
/// way. Guarded because an app that uses none of them — the blog — must
/// boot with none installed.
///
/// `exclude` takes gem NAMES, not a substring to cut out of rendered
/// source: JRuby provides Markly through the commonmark-java shim, and
/// a string surgery that silently missed would put the real gem (which
/// has no JRuby build) back in the list.
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
    const RUNTIME_GEMS: [(Marker, &str); 10] = [
        (Marker::Constant("BCrypt"), "bcrypt"),
        (Marker::Constant("HTMLEntities"), "htmlentities"),
        (Marker::Constant("ROTP"), "rotp"),
        (Marker::Constant("Markly"), "markly"),
        (Marker::Constant("Nokogiri"), "nokogiri"),
        (Marker::Constant("Parslet"), "parslet"),
        (Marker::Constant("TypeID"), "typeid"),
        (Marker::Constant("RQRCode"), "rqrcode"),
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
/// The prebuilt JS bundles that arrive from a gem rather than from the
/// app's own tree, keyed by the filename an import map pins them as.
/// Each is `<gem_dir>/app/assets/javascripts/<file>`, which is why the
/// generated rule can be a one-liner naming only the gem.
///
/// Filename-keyed rather than gem-keyed because that is the direction
/// the lookup runs: a pin gives a served path, and the question is which
/// gem — if any — ships it. Both the minified and unminified spellings
/// are listed for the same reason the `to:` kwarg exists at all: the
/// blog pins `turbo.min.js` and campfire pins `turbo.js`, and neither
/// spelling is more canonical than the other.
const GEM_JS_BUNDLES: &[(&str, &str)] = &[
    ("turbo.js", "turbo-rails"),
    ("turbo.min.js", "turbo-rails"),
    ("stimulus.js", "stimulus-rails"),
    ("stimulus.min.js", "stimulus-rails"),
    ("stimulus-loading.js", "stimulus-rails"),
    ("stimulus-autoloader.js", "stimulus-rails"),
    ("stimulus-importmap-autoloader.js", "stimulus-rails"),
    ("actioncable.esm.js", "actioncable"),
    ("actioncable.js", "actioncable"),
    ("action_cable.js", "actioncable"),
    ("actiontext.js", "actiontext"),
    ("actiontext.esm.js", "actiontext"),
];

/// De-blog the scaffold Makefile's `ASSET_JS` list and its gem-bundle
/// rules, the way `apply_makefile_test_list` does for `SPINEL_TESTS`.
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
    let mut gems: Vec<(String, String)> = Vec::new();
    let mut unsourced: Vec<String> = Vec::new();
    for rel in &rels {
        if has(&format!("app/javascript/{rel}")) || has(&format!("vendor/javascript/{rel}")) {
            targets.push(rel.clone());
        } else if let Some((_, gem)) = GEM_JS_BUNDLES.iter().find(|(file, _)| file == rel) {
            targets.push(rel.clone());
            gems.push((rel.clone(), (*gem).to_string()));
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

    let rules = gems
        .iter()
        .map(|(file, gem)| {
            format!(
                "$(ASSETS)/{file}:\n\
                 \t@mkdir -p $(dir $@)\n\
                 \tcp \"$$(bundle exec ruby -e 'puts Gem::Specification.find_by_name(%q({gem})).gem_dir')/app/assets/javascripts/{file}\" $@"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    apply_makefile_asset_blocks(files, BLOG_LIST, &list, &rules);

    // The rules above shell out to `bundle exec ruby -e ...find_by_name`,
    // which answers only for a gem IN THE BUNDLE. The scaffold's
    // `group :assets` names turbo-rails and stimulus-rails because those
    // are the blog's two; campfire also pins `actioncable.esm.js` and
    // `actiontext.js`, and without this the rule for each fails at
    // `find_by_name` rather than at the copy — an error naming Bundler,
    // three steps from the import map that actually asked for the file.
    let mut names: Vec<&str> = Vec::new();
    for (_, gem) in &gems {
        if !names.contains(&gem.as_str()) {
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
const BUNDLED: [(&str, &str); 12] = [
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
    let mut orphaned: Vec<String> = Vec::new(); // requires a file this app has no
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
            orphaned.push(entry.0.clone());
            continue;
        }
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

    // An orphaned framework test leaves with its sidecar and its
    // `.expected` snapshot — a snapshot for a program that is not in
    // the tree is what the next reader would have to explain away.
    if !orphaned.is_empty() {
        files.retain(|(p, _)| {
            !orphaned.iter().any(|o| {
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
        let makefile = fs::read_to_string("runtime/spinel/scaffold/Makefile").unwrap();
        let gemfile = fs::read_to_string("runtime/spinel/scaffold/Gemfile").unwrap();

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
        let makefile = fs::read_to_string("runtime/spinel/scaffold/Makefile").unwrap();

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

    /// `Cable.build_connection` is rewritten from the app, and the text
    /// it is rewritten to is the text `tests/spinel_cable_identity.rb`
    /// exercises.
    ///
    /// Reads the REAL `runtime/spinel/cable.rb`, so a rename of either
    /// marker fails here rather than silently shipping a tree whose
    /// `/cable` handshake identifies nobody — a failure mode with no
    /// symptom short of an unauthenticated socket, since the default arm
    /// connects anonymously ON PURPOSE and cannot be told apart from a
    /// generator that did not run.
    #[test]
    fn cable_connection_factory_is_generated_from_the_app() {
        use crate::dialect::LibraryClass;
        use crate::ident::{ClassId, Symbol};

        let cable = fs::read_to_string("runtime/spinel/cable.rb").unwrap();
        let connection_class = |name: &str| LibraryClass {
            name: ClassId(Symbol::from(name)),
            is_module: false,
            parent: Some(ClassId(Symbol::from("ActionCable::Connection::Base"))),
            includes: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        };

        // The app declares one: the arm names it.
        let mut app = App::new();
        app.library_classes.push(connection_class("ApplicationCable::Connection"));
        let mut files = vec![("runtime/cable.rb".to_string(), cable.clone())];
        apply_cable_connection(&mut files, &app);
        let out = &files[0].1;
        // VERBATIM the body `tests/spinel_cable_identity.rb` installs
        // before its generated-arm probes. Asserted as one string rather
        // than by `contains("ApplicationCable::Connection")` — the
        // default arm's comment block names that class too, in prose.
        assert!(
            out.contains(
                "  def self.build_connection(cookies)\n    ApplicationCable::Connection.new(cookies)\n  end\n"
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
        let mut files = vec![("runtime/cable.rb".to_string(), cable.clone())];
        apply_cable_connection(&mut files, &App::new());
        assert_eq!(files[0].1, cable, "a channel-less app had its cable.rb rewritten");

        // `action_cable.rb` also ends in `cable.rb` and carries no
        // markers; the suffix test must not claim it.
        let mut app2 = App::new();
        app2.library_classes.push(connection_class("ApplicationCable::Connection"));
        let action_cable = fs::read_to_string("runtime/spinel/action_cable.rb").unwrap();
        let mut files = vec![("runtime/action_cable.rb".to_string(), action_cable.clone())];
        apply_cable_connection(&mut files, &app2);
        assert_eq!(files[0].1, action_cable, "action_cable.rb was rewritten");
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
            includes: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
        });
        let mut files = vec![("boot.rb".to_string(), "# boot\n".to_string())];
        apply_module_mixins(&mut files, &modapp, MixinForm::Reopen);
        assert!(
            files[0].1.contains("module Greetable\n  include Loud\nend"),
            "a module target was reopened as a class:\n{}",
            files[0].1
        );
    }
}
