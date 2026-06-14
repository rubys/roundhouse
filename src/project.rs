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
use crate::analyze::Analyzer;
use crate::emit::{self, EmittedFile};
use crate::ingest::ingest_app;

/// Targets the `roundhouse` binary can produce. Matches the
/// `TARGETS` list in the legacy `build-site` binary plus the `Blog`
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
    Crystal,
    Elixir,
    Go,
    /// Kotlin/JVM emit (backend-only). In the `ALL` `--site` archive
    /// matrix as of the e2e-kotlin gate (the emitted archive builds via
    /// `gradle installDist` and boots — see `scripts/e2e kotlin`).
    /// Still incomplete (partial e2e/compare coverage), like several
    /// published targets — see `docs/kotlin-migration-plan.md`.
    Kotlin,
    Python,
    Rust,
    /// Swift emit (backend-only). In the `ALL` `--site` archive matrix
    /// as of the compare/bench/CI gates closing (the emitted archive
    /// builds via `swift build` and boots; Server.swift serves
    /// `/assets/*`). Still incomplete (no frameworks/e2e gates, like
    /// several published targets) — see `docs/swift-migration-plan.md`
    /// and issue #34.
    Swift,
    Typescript,
    /// TypeScript emit under the `worker` deployment profile
    /// (SharedWorker browser deployment).
    TypescriptWorker,
}

impl BuildTarget {
    /// All targets that participate in `--site` archive generation,
    /// in the same order the legacy `build-site` binary iterated them.
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
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Kotlin,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Swift,
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
            BuildTarget::Crystal => "crystal",
            BuildTarget::Elixir => "elixir",
            BuildTarget::Go => "go",
            BuildTarget::Kotlin => "kotlin",
            BuildTarget::Python => "python",
            BuildTarget::Rust => "rust",
            BuildTarget::Swift => "swift",
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
/// `README.md` — the spinel target ships the comprehensive scaffold
/// README, and the Blog fixture its own, that must not be overwritten.
/// (ruby/jruby rename theirs to `SPECIMEN.md` — see
/// `scaffold_readme_to_specimen` — so they take this quick-start.)
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
        BuildTarget::Spinel => {
            // Should not reach: the spinel archive keeps the scaffold
            // README and `ensure_readme` skips when one is present.
            "See the scaffold-provided README.md for build/run/test \
             instructions.\n"
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
             - Node.js + npm — Tailwind/Turbo asset build\n\
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
             BLOG_DB=tmp/blog.sqlite3 bundle exec puma -C config/puma.rb\n\
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
             BLOG_DB=tmp/blog.sqlite3 WEB_CONCURRENCY=0 jruby -S bundle exec puma -C config/puma.rb\n\
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
    // Every server target serves the same blog with the same env
    // conventions (PORT, default 3000; Action Cable at /cable), so
    // the "what you get" sentence lives here, not per target. Blog
    // (source fixture) and TypescriptWorker (no standalone server)
    // are the two non-server archives.
    let serves = match target {
        BuildTarget::Blog | BuildTarget::TypescriptWorker => "",
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
        // The flash spec (`flash.spec.js`) needs per-session (cookie)
        // flash. ruby + jruby have it via Rails; go wires it through a
        // cookie-backed store (server.go ReadFlashCookie/WriteFlashCookie
        // + Flash#to_persisted). Crystal/TS share one global in-memory
        // flash slot, which races with the comment specs'
        // `redirect_to … notice:` under `fullyParallel`; the rest don't
        // wire flash yet. Skip it everywhere but the per-session targets,
        // and drop a target from this skip as it gains per-session flash.
        // (See the flash-wiring punch list memory.)
        let run = if matches!(
            target,
            BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Go
        ) {
            "npx playwright test"
        } else {
            "E2E_SKIP=\"flash\" npx playwright test"
        };
        format!(
            "## End-to-end\n\
             Browser smoke tests (Playwright). Needs Node.js 18+ and the \
             `sqlite3` CLI; run after the Build steps above — the test \
             config boots the server and seeds `db/seed.sql` itself:\n\
             ```sh\n\
             cd e2e\n\
             npm install\n\
             npx playwright install chromium\n\
             {run}\n\
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
        BuildTarget::Spinel => spinel_files(app, fixture),
        BuildTarget::Ruby => ruby_runtime_files(app, fixture),
        BuildTarget::Jruby => jruby_runtime_files(app, fixture),
        BuildTarget::Crystal => Ok(sort_files(emit::crystal::emit(app))),
        BuildTarget::Elixir => Ok(sort_files(emit::elixir::emit(app))),
        BuildTarget::Go => Ok(sort_files(emit::go::emit(app))),
        BuildTarget::Kotlin => Ok(sort_files(emit::kotlin::emit(app))),
        BuildTarget::Python => Ok(sort_files(emit::python::emit(app))),
        BuildTarget::Rust => Ok(sort_files(emit::rust::emit(app))),
        BuildTarget::Swift => Ok(sort_files(emit::swift::emit(app))),
        BuildTarget::Typescript => Ok(sort_files(emit::typescript::emit(app))),
        BuildTarget::TypescriptWorker => Ok(sort_files(emit::typescript::emit_with_profile(
            app,
            &crate::profile::DeploymentProfile::worker(),
        ))),
    }?;

    // Blog is the verbatim Rails source — it ships `db/seeds.rb` and is
    // seeded by Rails, so it needs no SQL seed. Every transpile target
    // gets a language-agnostic `db/seed.sql` so the published archive is
    // self-contained-seedable (`sqlite3 <db> < db/seed.sql`) with no Ruby
    // — see e2e harness (scripts/e2e). spinel/ruby/jruby already carry it
    // via the scaffold walk; inject-if-absent is a no-op there.
    let files = if target == BuildTarget::Blog {
        files
    } else {
        let files = ensure_seed_sql(files)?;
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
/// Excluded: Spinel (keeps the scaffold README as its top-level doc —
/// the specimen document and matz's extraction surface; not in the
/// smoke matrix), TypescriptWorker (no standalone server), and Blog
/// (source fixture). ruby/jruby participate: their scaffold README
/// ships as SPECIMEN.md and a generated quick-start takes README.md
/// (see `scaffold_readme_to_specimen`).
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
            | BuildTarget::Ruby
            | BuildTarget::Jruby
    )
}

/// The Playwright specs, verbatim from the repo's `e2e/` harness — the
/// single source for both the legacy `scripts/e2e` path and the
/// in-archive suite. Compiled in via `include_str!` so build-site
/// needs no disk layout beyond the crate itself.
const E2E_SPECS: &[(&str, &str)] = &[
    ("e2e/index.spec.js", include_str!("../e2e/index.spec.js")),
    ("e2e/validation.spec.js", include_str!("../e2e/validation.spec.js")),
    ("e2e/tailwind.spec.js", include_str!("../e2e/tailwind.spec.js")),
    ("e2e/turbo_comment.spec.js", include_str!("../e2e/turbo_comment.spec.js")),
    ("e2e/action_cable.spec.js", include_str!("../e2e/action_cable.spec.js")),
    // Ships to every archive, but only runs on per-session (cookie) flash
    // targets — the others E2E_SKIP it via the README `## End-to-end`
    // block (see `target_readme`). Without shipping it here the skip list
    // is inert: `npx playwright test` only discovers specs present in the
    // archive's e2e/ dir.
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
        BuildTarget::Typescript => ("npm start", "db/development.sqlite3"),
        BuildTarget::Rust => ("./target/release/app", "storage/development.sqlite3"),
        BuildTarget::Python => ("uv run python -m app", "storage/development.sqlite3"),
        BuildTarget::Crystal => ("./server", "db/development.sqlite3"),
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
        // BLOG_DB must be explicit: bare puma (without the rake dev
        // task's env defaulting) falls back to :memory: and the seed
        // file is never opened.
        BuildTarget::Ruby => (
            "BLOG_DB=tmp/blog.sqlite3 bundle exec puma -C config/puma.rb",
            "tmp/blog.sqlite3",
        ),
        BuildTarget::Jruby => (
            "BLOG_DB=tmp/blog.sqlite3 WEB_CONCURRENCY=0 jruby -S bundle exec puma -C config/puma.rb",
            "tmp/blog.sqlite3",
        ),
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
/// doesn't already carry one. No-ops for spinel (ships the
/// comprehensive scaffold README) and for Blog when the fixture has
/// its own README (the archive is the verbatim source); ruby/jruby
/// rename the scaffold README to `SPECIMEN.md` first, so they get the
/// quick-start. Lives here rather than in the CLI so the `--site`
/// archives and `--target` output carry the same README.
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
/// The emit targets get assets injected. The scaffold targets ruby/spinel
/// build + serve their own via the Makefile's `make assets` (so injecting
/// would just be overwritten), and are excluded. JRUBY IS THE EXCEPTION: it's
/// a scaffold target (Puma + Rack, ruby_overlay's `Rack::Static` serves
/// `static/assets/`), but it CANNOT run `make assets` — the turbo.min.js step
/// shells `bundle exec ruby`, which collides MRI-vs-JRuby bundler (same reason
/// compare-jruby / e2e-jruby skip assets). So jruby is the one scaffold target
/// that needs the baked assets injected here; without them it serves no
/// tailwind.css / turbo.min.js and Turbo never boots.
fn ensure_static_assets(
    mut files: Vec<(String, String)>,
    target: BuildTarget,
) -> Vec<(String, String)> {
    let emit_target = matches!(
        target,
        BuildTarget::Crystal
            | BuildTarget::Elixir
            | BuildTarget::Go
            | BuildTarget::Jruby
            | BuildTarget::Kotlin
            | BuildTarget::Python
            | BuildTarget::Rust
            | BuildTarget::Swift
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker
    );
    if !emit_target {
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
/// The seeded `tmp/blog.sqlite3` that the Makefile copies in is NOT
/// included — `Schema.load!` is idempotent so a fresh DB still boots.
fn ruby_runtime_files(
    app: &App,
    fixture: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut files = spinel_files(app, fixture)?;

    files.retain(|(p, _)| p != "runtime/db.rb");
    for (path, _) in files.iter_mut() {
        if path == "runtime/db_cruby.rb" {
            *path = "runtime/db.rb".to_string();
        }
    }

    // Tep is a spinel-only transport (FFI HTTP server). The CRuby
    // target uses Puma + Rack via the ruby_overlay; nothing in its
    // boot path requires Tep, and the unsubstituted @TEP_SPHTTP_O@
    // placeholder in net.rb would confuse anyone exploring the tree.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

    // The source app's `app/javascript/` + `public/` static assets are
    // already folded in by `spinel_files` (both targets need them — the
    // spinel binary now serves `/assets/*` too). Nothing CRuby-specific
    // to add here beyond the overlay above.
    let mut files = dedupe_last_wins(files);
    scaffold_readme_to_specimen(&mut files);
    Ok(files)
}

/// The ruby/jruby archives ship the scaffold's comprehensive README as
/// `SPECIMEN.md`, freeing `README.md` for the generated machine-runnable
/// quick-start (`target_readme` via `ensure_readme`) that the smoke
/// contract executes. The spinel archive is untouched — its scaffold
/// README stays top-level (it's the specimen document for that target
/// and matz's primary extraction surface).
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

    // Db shim swap: drop the FFI `runtime/db.rb` and the CRuby gem
    // backend, then promote the JDBC backend into `runtime/db.rb`.
    // `db_jruby.rb` is excluded from `spinel_files`' base set, so read
    // it from disk and inject it here (mirrors the gem swap the CRuby
    // target does to `db_cruby.rb`).
    files.retain(|(p, _)| p != "runtime/db.rb" && p != "runtime/db_cruby.rb");
    let db_jruby = fs::read_to_string("runtime/spinel/db_jruby.rb")
        .map_err(|e| format!("read runtime/spinel/db_jruby.rb: {e}"))?;
    files.push(("runtime/db.rb".to_string(), db_jruby));

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

    // Drop the committed MRI `Gemfile.lock` from the JRuby tree: it pins
    // the C-ext `sqlite3` and omits `jdbc-sqlite3`, so shipping it would
    // make the tree's `jruby -S bundle install` a frozen-mode mismatch.
    // The JRuby bundle resolves its own platform-correct lock fresh.
    files.retain(|(p, _)| p != "Gemfile.lock");

    // Tep is a spinel-only transport (FFI HTTP server); JRuby uses Puma
    // + Rack via the ruby_overlay, same as the CRuby target.
    files.retain(|(p, _)| !p.starts_with("runtime/tep/"));

    walk_dir_into(
        Path::new("runtime/spinel/scaffold/ruby_overlay"),
        "",
        &mut files,
    )?;

    let mut files = dedupe_last_wins(files);
    scaffold_readme_to_specimen(&mut files);
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

    // `db_jruby.rb` is the JRuby/JDBC Db backend — it uses Java interop
    // (`java_import`, `Java::`) that the CRuby and Spinel toolchains (and
    // the spinel-subset compliance gate) must never see. It is injected
    // only by `jruby_runtime_files`, so keep it out of the shared base.
    files.retain(|(p, _)| p != "runtime/db_jruby.rb");

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
        "active_record",
        "action_view",
        "action_controller",
        "action_dispatch",
        "inflector",
        "json_builder",
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

    let js = fixture.join("app/javascript");
    if js.exists() {
        walk_dir_into(&js, "app/javascript/", &mut files)?;
    }
    let public = fixture.join("public");
    if public.exists() {
        walk_dir_into(&public, "public/", &mut files)?;
    }

    Ok(dedupe_last_wins(files))
}

/// Canonical path of the language-agnostic SQL seed. Single source of
/// truth — spinel/ruby/jruby ship it via the scaffold walk, every other
/// target gets it injected by `ensure_seed_sql`.
const SEED_SQL_SRC: &str = "runtime/spinel/scaffold/db/seed.sql";

/// Ensure the file set carries `db/seed.sql` (the self-contained,
/// Ruby-free seed applied with `sqlite3 <db> < db/seed.sql`). No-op when
/// the set already includes it (spinel/ruby/jruby, via the scaffold);
/// otherwise reads the canonical file and inserts it in sorted position.
fn ensure_seed_sql(files: Vec<(String, String)>) -> Result<Vec<(String, String)>, String> {
    if files.iter().any(|(p, _)| p == "db/seed.sql") {
        return Ok(files);
    }
    let content = fs::read_to_string(SEED_SQL_SRC)
        .map_err(|e| format!("read {SEED_SQL_SRC}: {e}"))?;
    let mut files = files;
    files.push(("db/seed.sql".to_string(), content));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
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
    Analyzer::new(&app).analyze(&mut app);

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
    fn ensure_seed_sql_injects_when_absent() {
        let files = vec![("app/main.go".to_string(), "package main".to_string())];
        let out = ensure_seed_sql(files).unwrap();
        let seed = out.iter().find(|(p, _)| p == "db/seed.sql");
        assert!(seed.is_some(), "db/seed.sql should be injected");
        // Content is the canonical file — sanity-check it carries the seed rows.
        assert!(seed.unwrap().1.contains("INSERT INTO articles"));
        // Result stays sorted by path.
        assert!(out.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn ensure_seed_sql_is_idempotent_when_present() {
        let files = vec![
            ("db/seed.sql".to_string(), "-- already here".to_string()),
            ("app/main.go".to_string(), "package main".to_string()),
        ];
        let out = ensure_seed_sql(files).unwrap();
        let seeds: Vec<_> = out.iter().filter(|(p, _)| p == "db/seed.sql").collect();
        assert_eq!(seeds.len(), 1, "no duplicate db/seed.sql");
        assert_eq!(seeds[0].1, "-- already here", "existing content preserved");
    }
}
