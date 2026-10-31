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
//! and don't pull this module. The target enum lives in [`crate::build_target`]
//! so lowering and Ruby emit can name a target on wasm.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use zip::write::SimpleFileOptions;

use crate::App;
use crate::emit::{self, EmittedFile};
use crate::gems::{DEMAND_REQUIRES, GEM_REQUIRES, Marker, VARIATION_MARKER, compiler_runtime_gems};
use crate::ingest::ingest_app;

pub use crate::build_target::{BuildTarget, ParamBindCapability};

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
        BuildTarget::Futamura => {
            "Rails → Ruby as a partial evaluator: what Roundhouse can \
             resolve statically is specialized, and everything else stays \
             residual Ruby running on the app's real gems, Rails included. \
             The tree is the Rails app with its static query chains \
             specialized (config/initializers/futamura.rb) and, when the \
             analysis clears any GET routes, whole-route replacement: those \
             routes are served by the app's `--target ruby` emit \
             (vendor/futamura/lowered) inside the Rails process. That part \
             needs Ruby 4.1 started with RUBY_BOX=1; \
             FUTAMURA_WHOLE_ROUTE=off runs the tree as plain Rails. Build, \
             run and test it as the Rails app it is:\n\n\
             ```sh\n\
             bundle install\n\
             bin/rails db:prepare\n\
             RUBY_BOX=1 bin/rails test\n\
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
        // ruby/jruby `## Test` is `rake test`. The Rakefile's
        // `FileList["test/**/*_test.rb"]` is the suite: emitted
        // TestBase files run their autorun shim when loaded, and the
        // scaffold's Minitest::Test files run at exit. A hand-written
        // list of five drivers (the old README, and
        // `tests/ruby_toolchain.rs`) left every other shipped file
        // unrun — views, broadcasts, route helpers — so they drifted
        // for months while smoke stayed green.
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
             bundle exec rake test\n\
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
             jruby -S bundle exec jruby -S rake test\n\
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
        BuildTarget::Roda | BuildTarget::Futamura => "",
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

/// The controller's response state, in `@_`-prefixed ivars on the Ruby
/// family and Spinel trees. The shared `action_controller` runtime keeps
/// it in `@status`, `@body`, `@content_type` and the rest, but in Ruby
/// those are the app controller's own ivars too, so a render overwrote
/// an app's `@status`, and an app's `@headers` broke `response.headers`.
/// Rails keeps its state in `@_response`, `@_request` and the like. The
/// strict targets transpile the same source into fields and accessors,
/// where no app ivar shares them, so they keep it as written.
///
/// Every use in these files is renamed, including `Current`'s own
/// `@request` (a module ivar behind its own `self.request` accessors,
/// so the rename is consistent in the file). An `attr_*` can't read an
/// ivar of another name, so the two that name these are written out;
/// they're matched exactly, and any other `attr_*` naming one is an
/// error, so a drift in the runtime is a build error here.
fn controller_state_ivars(files: &mut [(String, String)]) -> Result<(), String> {
    const NAMES: &[&str] = &[
        "status", "body", "content_type", "content_type_explicit", "performed", "location", "head_response",
        "headers", "request",
    ];
    const STEMS: &[&str] = &["base", "cache_control", "conditional_get", "current", "default_headers", "inertia"];
    const ATTRS: &[(&str, &str, &str)] = &[
        (
            "base",
            "    attr_reader   :status, :body, :location, :content_type\n",
            "    def status\n      @_status\n    end\n\n    def body\n      @_body\n    end\n\n    def location\n      @_location\n    end\n\n    def content_type\n      @_content_type\n    end\n",
        ),
        (
            "current",
            "    attr_accessor :request\n",
            "    def request\n      @_request\n    end\n\n    def request=(value)\n      @_request = value\n    end\n",
        ),
    ];
    let ivar = regex::Regex::new(&format!(r"(^|[^@\w])@({})\b", NAMES.join("|"))).expect("ivar pattern");
    let attr = regex::Regex::new(&format!(r"attr_(reader|writer|accessor)\b[^\n]*:({})\b", NAMES.join("|"))).expect("attr pattern");
    for stem in STEMS {
        for path in [format!("runtime/action_controller/{stem}.rb"), format!("sig/runtime/action_controller/{stem}.rbs")] {
            let Some((_, text)) = files.iter_mut().find(|(p, _)| *p == path) else {
                return Err(format!("controller_state_ivars: {path} not in the tree"));
            };
            if path.ends_with(".rb") {
                for (owner, narrow, wide) in ATTRS.iter().filter(|(owner, _, _)| owner == stem) {
                    if !text.contains(narrow) {
                        return Err(format!("controller_state_ivars: {owner}.rb no longer declares {narrow:?}"));
                    }
                    *text = text.replacen(narrow, wide, 1);
                }
                if let Some(m) = attr.find(text) {
                    return Err(format!("controller_state_ivars: {path} has `{}`, which reads an unrenamed ivar", m.as_str()));
                }
            }
            *text = ivar.replace_all(text, "${1}@_${2}").into_owned();
        }
    }
    Ok(())
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
    let string_key = |col: &crate::schema::Column| {
        !matches!(
            col.col_type,
            crate::schema::ColumnType::Integer | crate::schema::ColumnType::BigInt
        )
    };
    let has_string_key = app.schema.tables.values().any(|t| {
        t.columns.iter().any(|c| c.primary_key && string_key(c))
    }) || app.models.iter().any(|model| {
        let Some(name) = &model.primary_key else { return false };
        app.schema
            .tables
            .get(&model.table.0)
            .is_some_and(|table| table.columns.iter().any(|c| &c.name == name && string_key(c)))
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
        ("    def _primary_key_value: () -> Integer\n", "    def _primary_key_value: () -> (Integer | String)\n"),
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

/// Spinel has no unboxed `Array[HashConfig]`. An RBS that claims one is
/// misread at call sites as a String, so `with_temporary_connection`
/// refuses the element (`sp_HashConfig *`). Roundhouse keeps the
/// precise array type for Bar A/B; only the Spinel tree widens it.
pub fn widen_spinel_hash_config_arrays(files: &mut [(String, String)]) {
    const NARROW: &str = "Array[ActiveRecord::DatabaseConfigurations::HashConfig]";
    const WIDE: &str = "Array[untyped]";
    for (path, content) in files.iter_mut() {
        if !path.ends_with(".rbs") || !content.contains(NARROW) {
            continue;
        }
        *content = content.replace(NARROW, WIDE);
    }
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
    use crate::det::HashMap;
    struct Decl {
        file: usize,
        span: std::ops::Range<usize>,
        sig: String,
    }
    // (qualified class, receiver, method) -> declarations from every runtime .rbs.
    let mut decls: HashMap<(String, &'static str, String), Vec<Decl>> = HashMap::default();
    for (fi, (path, text)) in files.iter().enumerate() {
        if !path.ends_with(".rbs")
            || !(path.starts_with("runtime/") || path.starts_with("sig/runtime/"))
        {
            continue;
        }
        let found = rbs_method_declarations(text)
            .map_err(|e| format!("resolve_runtime_sig_conflicts: {path}: {e}"))?;
        for d in found {
            decls
                .entry((d.owner, d.receiver, d.name))
                .or_default()
                .push(Decl { file: fi, span: d.span, sig: d.sig });
        }
    }
    // Rank of the file a declaration sits in: higher wins.
    let rank = |fi: usize, class: &str| -> u8 {
        let path = files[fi].0.trim_start_matches("sig/");
        let rest = path.trim_start_matches("runtime/");
        let spinel = crate::runtime_files::exists(&format!("runtime/spinel/{rest}"))
            && !crate::runtime_files::exists(&format!("runtime/ruby/{rest}"));
        let primary = format!(
            "{}.rbs",
            crate::naming::underscore(class).replace("::", "/")
        );
        let reopen = rest != primary;
        (spinel as u8) * 2 + reopen as u8
    };
    let mut drop: HashMap<usize, Vec<std::ops::Range<usize>>> = HashMap::default();
    let mut undecided = Vec::new();
    for ((class, receiver, method), ds) in &decls {
        let first = &ds[0].sig;
        if ds.iter().all(|d| &d.sig == first) {
            continue;
        }
        let best = ds.iter().map(|d| rank(d.file, class)).max().unwrap_or(0);
        let winners: Vec<&Decl> = ds.iter().filter(|d| rank(d.file, class) == best).collect();
        if winners.iter().any(|d| d.sig != winners[0].sig) {
            undecided.push(format!(
                "{class}#{receiver}{method}: {}",
                ds.iter()
                    .map(|d| format!("{} `{}`", files[d.file].0, d.sig))
                    .collect::<Vec<_>>()
                    .join(" vs ")
            ));
            continue;
        }
        for d in ds.iter().filter(|d| rank(d.file, class) != best) {
            drop.entry(d.file).or_default().push(d.span.clone());
        }
    }
    if !undecided.is_empty() {
        undecided.sort();
        return Err(format!(
            "resolve_runtime_sig_conflicts: runtime RBS declares a method twice and no file overrides the other:\n  {}",
            undecided.join("\n  ")
        ));
    }
    for (fi, spans) in drop {
        files[fi].1 = remove_rbs_spans(&files[fi].1, spans);
    }
    Ok(())
}

/// One method declaration in an RBS file, as the passes that drop one
/// see it: `resolve_runtime_sig_conflicts`, and `emit::ruby::reopen` for
/// a method an app's reopen redefines.
pub(crate) struct RbsMethodDecl {
    /// The qualified class, module or interface that declares it.
    pub(crate) owner: String,
    /// `""`, `"self."` or `"self?."`.
    pub(crate) receiver: &'static str,
    pub(crate) name: String,
    /// What removing it takes: every overload and annotation, plus the
    /// indentation and line end when no neighbor shares that line.
    pub(crate) span: std::ops::Range<usize>,
    /// The signature, compared without layout or comments.
    pub(crate) sig: String,
}

/// Every method declaration in `text`, nested owners included.
pub(crate) fn rbs_method_declarations(text: &str) -> Result<Vec<RbsMethodDecl>, String> {
    let signature = ruby_rbs::node::parse(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    collect_rbs_methods(signature.declarations().iter(), "", text, &mut out)?;
    Ok(out)
}

// Compare the complete signature, ignoring layout and comments but keeping
// token boundaries and the contents of quoted literal types intact.
fn signature_key(source: &str) -> String {
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '#' {
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            continue;
        }
        let mut token = c.to_string();
        if c == '\'' || c == '"' {
            while let Some(next) = chars.next() {
                token.push(next);
                if next == '\\' {
                    if let Some(escaped) = chars.next() {
                        token.push(escaped);
                    }
                } else if next == c {
                    break;
                }
            }
        } else if c.is_alphanumeric() || c == '_' {
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                token.push(chars.next().unwrap());
            }
        }
        tokens.push(token);
    }
    tokens.join(" ")
}

fn collect_rbs_methods<'a>(
    nodes: impl Iterator<Item = ruby_rbs::node::Node<'a>>,
    parent: &str,
    text: &str,
    out: &mut Vec<RbsMethodDecl>,
) -> Result<(), String> {
    use ruby_rbs::node::{MethodDefinitionKind, Node};
    for node in nodes {
        let (name, members) = match node {
            Node::Class(c) => (c.name(), c.members()),
            Node::Module(m) => (m.name(), m.members()),
            Node::Interface(i) => (i.name(), i.members()),
            Node::MethodDefinition(method) => {
                let prefix = match method.kind() {
                    MethodDefinitionKind::Instance => "",
                    MethodDefinitionKind::Singleton => "self.",
                    MethodDefinitionKind::SingletonInstance => "self?.",
                };
                let loc = method.location();
                let mut start = loc.start() as usize;
                let mut end = loc.end() as usize;
                // ruby-rbs 0.3 leaves name_location unset for a backtick
                // identifier. The name symbol's own location is populated
                // for every spelling, including quoted names and operators.
                let sig_start = method.name().location().end() as usize;
                let invalid_location = || {
                    format!(
                        "invalid source location for {parent}#{prefix}{}",
                        method.name()
                    )
                };
                let source = text
                    .get(sig_start..end)
                    .filter(|_| text.get(start..sig_start).is_some())
                    .ok_or_else(invalid_location)?;
                let sig = signature_key(source);
                for annotation in method.annotations().iter() {
                    start = start.min(annotation.location().start() as usize);
                }
                // Remove indentation/newline only when the declaration owns
                // that part of the line; same-line neighbors must survive.
                let before = text.get(..start).ok_or_else(invalid_location)?;
                let line_start = before.rfind('\n').map_or(0, |n| n + 1);
                if text[line_start..start].trim().is_empty() {
                    start = line_start;
                }
                let line_end = text[end..].find('\n').map_or(text.len(), |n| end + n + 1);
                let suffix = text[end..line_end].trim();
                if suffix.is_empty() || suffix.starts_with('#') {
                    end = line_end;
                }
                // A quoted instance name can itself start with `self.`.
                // Keep the receiver separate from the decoded name.
                out.push(RbsMethodDecl {
                    owner: parent.to_string(),
                    receiver: prefix,
                    name: method.name().as_str().to_string(),
                    span: start..end,
                    sig,
                });
                continue;
            }
            _ => continue,
        };
        let namespace = name.namespace();
        let mut parts = Vec::new();
        if !namespace.absolute() && !parent.is_empty() {
            parts.push(parent.to_string());
        }
        parts.extend(namespace.path().iter().filter_map(|part| match part {
            Node::Symbol(s) => Some(s.as_str().to_string()),
            _ => None,
        }));
        parts.push(name.name().as_str().to_string());
        collect_rbs_methods(members.iter(), &parts.join("::"), text, out)?;
    }
    Ok(())
}

/// `text` without `spans`, overlapping ones included.
pub(crate) fn remove_rbs_spans(text: &str, mut spans: Vec<std::ops::Range<usize>>) -> String {
    let mut kept = String::with_capacity(text.len());
    spans.sort_by_key(|span| span.start);
    let mut cursor = 0;
    for span in spans {
        if cursor < span.start {
            kept.push_str(&text[cursor..span.start]);
        }
        cursor = cursor.max(span.end);
    }
    kept.push_str(&text[cursor..]);
    kept
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
            // Carries every class verbatim (it returns before this check).
            | BuildTarget::Futamura
        // Roda omitted: spike emit does not walk `app.library_classes`.
    )
}

/// A gem's class method called where the receiver may be a relation:
/// only the Ruby family hands a relation on (`lower::relation_class_calls`).
fn report_relation_class_calls(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Futamura) {
        return;
    }
    for (span, method) in crate::lower::relation_class_calls::untyped_calls(app) {
        emit::diagnostics::report_unsupported(
            span,
            target.as_str(),
            "class method on a relation",
            format!(
                "`{}` is a class method the model takes from a gem module, called on a receiver whose type is unknown; \
                 Rails' relation answers it, and this target has no relation to hand it",
                method.as_str()
            ),
        );
    }
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

/// Every Ruby exception class is JavaScript's `Error` on the TypeScript
/// targets, so an app class named `Error` emits `class Error extends
/// Error`, which reads the class it is declaring.
fn report_typescript_error_class(app: &App, target: BuildTarget) {
    if !matches!(target, BuildTarget::Typescript | BuildTarget::TypescriptWorker) {
        return;
    }
    for lc in app.library_classes.iter().filter(|lc| lc.origin.is_none() && !lc.is_module && lc.name.0.as_str() == "Error") {
        emit::diagnostics::report_unsupported(
            lc.parent_span,
            target.as_str(),
            "class named Error",
            format!("an app class named `Error` shadows the `Error` the {} emit maps every Ruby exception to", target.as_str()),
        );
    }
}

/// A model's class-body `extend AppModule` and the calls after it to the
/// module's methods (`ingest::app_extension_calls`) are replayed by the
/// Ruby family's emit; no other target carries a class body's own code.
fn report_replayed_class_bodies(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item
                && expr.decisions & crate::expr::REPLAYED_CLASS_BODY != 0
            {
                let name = match &*expr.node {
                    crate::expr::ExprNode::Send { method, .. } => method.as_str().to_string(),
                    _ => "call".to_string(),
                };
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target.as_str(),
                    "class body",
                    format!("`{name}` in the body of `{}` runs the app's own module code as the class loads, which the {} emit does not carry", model.name.0.as_str(), target.as_str()),
                );
            }
        }
    }
    for lc in &app.library_classes {
        for expr in lc.unknown_calls.iter().filter(|e| e.decisions & crate::expr::REPLAYED_CLASS_BODY != 0) {
            let crate::expr::ExprNode::Send { method, .. } = &*expr.node else { continue };
            emit::diagnostics::report_unsupported(
                expr.span,
                target.as_str(),
                "class body",
                format!("`{}` in the body of `{}` runs its own class method as it loads, which the {} emit does not carry", method.as_str(), lc.name.0.as_str(), target.as_str()),
            );
        }
    }
}

/// A mailer and the `ActionMailer` runtime it delivers through ship on
/// the Ruby family only; elsewhere `Mailer.action(…)` reached nothing,
/// or a view of the same name.
fn report_mailers(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
        return;
    }
    let mailers = crate::lower::mailer_class_side::mailer_names(app);
    // Not every mailer: real-blog's bare `ApplicationMailer` has no action to deliver.
    for lc in app.library_classes.iter().filter(|lc| mailers.contains(lc.name.0.as_str())) {
        let Some(span) = lc
            .methods
            .iter()
            .filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() != "initialize")
            .map(|m| m.name_span)
            .find(|s| !s.is_synthetic())
        else {
            continue;
        };
        emit::diagnostics::report_unsupported(
            span,
            target.as_str(),
            "mailer",
            format!("`{}` delivers through the Ruby-family ActionMailer runtime; the {} runtime ships none", lc.name.0.as_str(), target.as_str()),
        );
    }
}

/// `class LoginFailedError < StandardError; end` beside a controller in
/// its file: the Ruby emit re-declares it ahead of the controller, the
/// other targets declare nothing for it.
fn report_controller_sibling_classes(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
        return;
    }
    for controller in &app.controllers {
        for sibling in &controller.sibling_classes {
            emit::diagnostics::report_unsupported(
                sibling.parent_span,
                target.as_str(),
                "class",
                format!(
                    "`{}`, declared beside `{}` in its file, is not emitted for this target (the Ruby emit carries it)",
                    sibling.name.as_str(),
                    controller.name.0.as_str()
                ),
            );
        }
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

/// True when the app schema or emitted roots mention a date-only value
/// (or `DateTime`). Shared by the unsupported-target gate and Spinel's
/// conditional load of its `date` package.
fn app_uses_date(app: &App) -> bool {
    fn expr_has_date(e: &crate::expr::Expr) -> bool {
        if e.ty.as_ref().is_some_and(crate::ty::Ty::contains_date)
            || matches!(&*e.node, crate::expr::ExprNode::Cast { target_ty, .. } if target_ty.contains_date())
            || matches!(&*e.node, crate::expr::ExprNode::Const { path }
                if (path.len() == 1 || (path.len() == 2 && path[0].as_str().is_empty()))
                    && path.last().is_some_and(|name| matches!(name.as_str(), "Date" | "DateTime")))
            || matches!(&*e.node, crate::expr::ExprNode::Const { path }
                if (path.len() == 2 || (path.len() == 3 && path[0].as_str().is_empty()))
                    && path[path.len() - 2].as_str() == "Date"
                    && path[path.len() - 1].as_str() == "Error")
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
/// `boot.rb`. Spinel only — Spinel's `date` package and the date-column
/// JSON reopen belong on that target.
fn inject_date_package_into_boot(boot: &mut String) -> Result<(), String> {
    let anchor = "require_relative \"runtime/active_record_serialization\"\n";
    // Prefer concat! over one escaped multiline string: the CI
    // planner's project.rs body-scope regex backtracks for minutes
    // on `\"` + `\` continuations in a single literal this large.
    let inject = concat!(
        "# Date package — only when the app uses Date or DateTime.\n",
        "require \"date\"\n",
        "require_relative \"runtime/active_support_date_parsing\"\n",
        "require_relative \"runtime/active_record_date_serialization\"\n",
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
             (Date package inject anchor)"
                .into(),
        )
    }
}

/// CRuby/JRuby overlay boot replaces Spinel's boot wholesale, wiping
/// the Date-package inject. Re-inject **only** the shared `date_*`
/// calendar helpers (invariant 2). Do not load
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

/// `boot.rb` requires `runtime/array_column` after the JSON-column seam,
/// only for an app with a stored array column (`lower::array_columns`).
/// The ruby overlay's boot replaces Spinel's, so both trees call this.
fn inject_array_column_require(files: &mut [(String, String)], app: &App) -> Result<(), String> {
    if !app.schema.tables.values().any(|t| t.columns.iter().any(|c| c.array_stored)) {
        return Ok(());
    }
    let anchor = "require_relative \"runtime/json_column\"\n";
    let Some((_, boot)) = files.iter_mut().find(|(p, _)| p == "boot.rb") else {
        return Err("boot.rb missing for the array-column require".into());
    };
    if boot.contains("require_relative \"runtime/array_column\"") {
        return Ok(());
    }
    let at = boot.find(anchor).ok_or("boot.rb missing the json_column require (array-column anchor)")?;
    boot.insert_str(at + anchor.len(), "require_relative \"runtime/array_column\"\n");
    Ok(())
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
    fn report(span: crate::span::Span, target: &str, found: &mut bool) {
        *found = true;
        emit::diagnostics::report_unsupported(
            span,
            target,
            "Data.define",
            "this target has no supported Data factory representation; use Ruby or Spinel",
        );
    }
    fn visit(expr: &crate::expr::Expr, target: &str, found: &mut bool) {
        if expr.decisions & crate::expr::RESOLVED_DATA_FACTORY != 0 {
            report(expr.span, target, found);
        }
        expr.node.for_each_child(&mut |child| visit(child, target, found));
    }
    let mut found = false;
    crate::lower::for_each_forwarding_body_ref(app, &mut |expr| {
        visit(expr, target.as_str(), &mut found);
    });
    // A file-level `X = Data.define(…)` is a library class, not an
    // expression in a body.
    for (_, span) in app.library_classes.iter().filter_map(|lc| lc.data_define()) {
        report(span, target.as_str(), &mut found);
    }
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

/// A `t` that reads the translation table or the whole catalog when it
/// runs (`lower::i18n_translate`): a target whose runtime carries no
/// `I18n.translate_site` / `I18n.translate_runtime` reports each one
/// rather than emit a call to nothing.
// Not a BuildTarget match: the bundled-constant walk names its target as a string, and both read these
const I18N_RUNTIME_TARGETS: &[&str] = &["blog", "futamura", "ruby", "jruby", "spinel", "typescript", "crystal", "csharp", "kotlin", "swift", "python", "go", "elixir", "rust"];
const I18N_CATALOG_TARGETS: &[&str] = &["blog", "futamura", "ruby", "jruby", "spinel", "typescript", "crystal"];

fn ships_i18n_runtime(target: BuildTarget) -> bool {
    I18N_RUNTIME_TARGETS.contains(&target.as_str())
}

fn ships_i18n_catalog(target: BuildTarget) -> bool {
    I18N_CATALOG_TARGETS.contains(&target.as_str())
}

fn report_i18n_table_sites(app: &App, target: BuildTarget) {
    if ships_i18n_runtime(target) {
        if !ships_i18n_catalog(target) {
            for span in &app.i18n_runtime_sites {
                crate::emit::diagnostics::report_unsupported(
                    *span,
                    target.as_str(),
                    "I18n",
                    "a translation whose key or options are known only when it runs, which reads the app's whole catalog; this target's runtime carries the locale-chosen table, not the catalog",
                );
            }
        }
        return;
    }
    for span in &app.i18n_table_sites {
        crate::emit::diagnostics::report_unsupported(
            *span,
            target.as_str(),
            "I18n",
            "a translation chosen by the locale or read from the app's catalog when it runs, which only the Ruby-family runtime carries",
        );
    }
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
/// A PostgreSQL array column (`array: true`) on a target that stores rows
/// itself: its SQLite storage holds one element, not the `Array` Rails
/// reads, so the column is reported for that target. The Roda conversion
/// runs on Sequel and futamura on Rails, which read the database's own
/// type.
fn report_array_columns(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    for table in app.schema.tables.values() {
        // A stored array column holds the Array (`lower::array_columns`).
        for column in table.columns.iter().filter(|column| column.array && !column.array_stored) {
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                crate::span::Span::synthetic(),
                Some(crate::ident::Symbol::from(target.as_str())),
                "array_column",
                format!(
                    "table {}: `{}` is a PostgreSQL array column; this target stores and reads \
                     one element where Rails reads an Array",
                    table.name.as_str(),
                    column.name.as_str()
                ),
            );
            d.severity = crate::diagnostic::Severity::Warning;
            emit::diagnostics::push(d);
        }
    }
}

/// Rails' default forgery check heads every controller rooted at
/// ActionController::Base, and only the Ruby family's runtime issues and
/// verifies the token (`runtime/ruby/action_controller/authenticity_token.rb`).
/// Elsewhere the check passes every request, so the gap is named once.
fn report_forgery_protection(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
        return;
    }
    let parent_of = |name: &str| {
        app.controllers.iter().find(|c| c.name.0.as_str() == name).and_then(|c| c.parent.as_ref()).map(|p| p.0.as_str().to_string())
    };
    // Not the controllers ingest synthesizes: the health check and the redirect routes answer GET, which the check never reads
    let synthesized = [crate::ingest::routes::RAILS_HEALTH_CONTROLLER, crate::ingest::routes::REDIRECT_CONTROLLER];
    let rooted_at_base = app.controllers.iter().filter(|c| !synthesized.contains(&c.name.0.as_str())).any(|c| {
        let mut cur = c.parent.as_ref().map(|p| p.0.as_str().to_string());
        for _ in 0..32 {
            match cur {
                Some(ref p) if p == "ActionController::Base" => return true,
                Some(ref p) => cur = parent_of(p),
                None => return false,
            }
        }
        false
    });
    if !rooted_at_base {
        return;
    }
    let mut d = crate::diagnostic::Diagnostic::unsupported(
        crate::span::Span::synthetic(),
        Some(crate::ident::Symbol::from(target.as_str())),
        "forgery_protection",
        "Rails checks every non-GET request for an authenticity token; this target's runtime issues no token and passes every request",
    );
    d.severity = crate::diagnostic::Severity::Warning;
    emit::diagnostics::push(d);
}

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

/// The date and decimal coercions `ingest::dry_struct` and
/// `ingest::dry_initializer` generate call
/// stdlib the CRuby and JRuby trees load. Spinel's tree has `Date`
/// (Spinel's `date` package) and `BigDecimal()`. Its `DateTime` coercion
/// stays reported until a run pins it, and it has no `Time.parse` or
/// `BigDecimal.interpret_loosely`; the other targets have none of them.
fn report_dry_struct_stdlib(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget) {
        use crate::expr::ExprNode;
        let spinel = matches!(target, BuildTarget::Spinel);
        let named = |path: &[crate::ident::Symbol]| -> Vec<String> {
            path.iter().map(|p| p.as_str().to_string()).filter(|p| !p.is_empty()).collect()
        };
        let construct = match &*expr.node {
            ExprNode::Send { recv: Some(recv), method, .. } if method.as_str() == "parse" => match &*recv.node {
                ExprNode::Const { path } if named(path) == ["Time"] => Some(("Time.parse", "dry-types date coercion")),
                _ => None,
            },
            ExprNode::Send { recv: Some(recv), method, .. } if method.as_str() == "interpret_loosely" => match &*recv.node {
                ExprNode::Const { path } if named(path) == ["BigDecimal"] => {
                    Some(("BigDecimal.interpret_loosely", "dry-types decimal coercion"))
                }
                _ => None,
            },
            ExprNode::Send { recv: None, method, .. } if method.as_str() == "BigDecimal" && !spinel => {
                Some(("BigDecimal()", "dry-types decimal coercion"))
            }
            ExprNode::Const { path } if named(path) == ["DateTime"] => Some(("DateTime", "dry-types date coercion")),
            ExprNode::Const { path } if named(path) == ["Date"] && !spinel => {
                Some(("Date", "dry-types date coercion"))
            }
            _ => None,
        };
        if let Some((what, construct)) = construct {
            crate::emit::diagnostics::report_unsupported(
                expr.span,
                target.as_str(),
                construct,
                &format!("`{what}` has no runtime on this target"),
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target));
    }
    for lc in &app.library_classes {
        if matches!(
            lc.origin,
            Some(
                crate::dialect::LibraryClassOrigin::DryStruct
                    | crate::dialect::LibraryClassOrigin::DryInitializer
                    | crate::dialect::LibraryClassOrigin::DryValidation
            )
        ) {
            for method in &lc.methods {
                visit(&method.body, target);
            }
            // Shared defaults are class constants, evaluated at load.
            for (_, value) in &lc.constants {
                visit(value, target);
            }
        }
    }
}

/// Report syntax whose runtime contract is currently native Ruby only.
fn report_native_ruby_syntax(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget, load_time: bool) {
        use crate::expr::{ExprNode, LValue};
        let construct = match &*expr.node {
            ExprNode::ForwardKeywords if target == BuildTarget::Spinel => None,
            ExprNode::ForwardKeywords => Some("anonymous keyword forwarding"),
            ExprNode::ForwardKeywordsWithPairs { .. } if target == BuildTarget::Spinel => None,
            ExprNode::ForwardKeywordsWithPairs { .. } => Some("anonymous keyword forwarding"),
            // `defined?(A::B)` that `lower::defined_const_fold` could not
            // decide, where it runs after the program has loaded (a method
            // body, a lambda, a template): Spinel answers it from the whole
            // program, which is then what CRuby answers for the emitted
            // program ("constant" or nil; lexical, inherited, nested and
            // missing paths included). While the program loads, CRuby
            // answers nil for a constant whose file has not run yet and
            // Spinel does not, so that position stays refused.
            ExprNode::Defined { operand }
                if target == BuildTarget::Spinel && !load_time
                    && matches!(&*operand.node, ExprNode::Const { .. }) => None,
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
        expr.node.for_each_child(&mut |child| visit(child, target, load_time));
    }
    let load_time = crate::lower::defined_const_fold::load_time_roots(app);
    crate::lower::for_each_emit_body_ref(app, &mut |expr| {
        visit(expr, target, load_time.contains(&(expr as *const crate::expr::Expr as usize)))
    });
}

/// A self binding (`T.bind(self, T)`, `#: self as T`) retypes `self` for
/// the rest of a body, and the analyzer types the sends after it against
/// that type. The Ruby emitters drop the binding and leave those sends to
/// Ruby's dynamic `self`. Every other target emits them on the method's
/// own receiver, which the binding does not change, so it refuses it.
fn report_self_bindings(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget) {
        if crate::expr::is_self_binding(expr) {
            crate::emit::diagnostics::report_unsupported(
                expr.span, target.as_str(), "self binding",
                "the sends after it are typed against the bound type, which `self` does not take on in this target",
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target));
}

/// Sends the analyzer types for every target but whose runtime only the
/// Ruby family has: `Array#fetch`, `each_slice`, `each_cons`, an
/// enumerator's `with_object`, `Array[...]` and `Hash[...]`,
/// `Array.try_convert` (what a nested splat desugars to), `include?` on
/// a `Class`/`Module` value (Module's ancestry query, which the other
/// targets' `include?` is not), Kernel's `Rational(...)`, which a
/// rational literal ingests as, Kernel's `Integer(...)` and `Float(...)`,
/// the `Pathname(...)` and `URI(...)` conversions analysis admitted as
/// Kernel's (`expr::KERNEL_CONVERSION`, decided for the call's own class),
/// and the ActiveRecord methods only connection.rb defines.
/// The other emitters render them as the method name on
/// the receiver, which nothing there defines, so they refuse each by name.
/// `alias read_attribute_for_serialization send`: the alias delegates to a
/// method every Ruby object answers (`expr::BUILTIN_ALIAS`), which a strict
/// target has no counterpart for. Refused there where it is written, naming
/// the builtin.
fn report_builtin_aliases(app: &App, target: BuildTarget) {
    if matches!(
        target,
        BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura
    ) {
        return;
    }
    fn find(e: &crate::expr::Expr, out: &mut Vec<(crate::span::Span, crate::ident::Symbol)>) {
        if e.decisions & crate::expr::BUILTIN_ALIAS != 0
            && let crate::expr::ExprNode::Send { method, .. } = &*e.node
        {
            out.push((e.span, method.clone()));
        }
        e.node.for_each_child(&mut |c| find(c, out));
    }
    for class in &app.library_classes {
        for method in &class.methods {
            let mut found = Vec::new();
            find(&method.body, &mut found);
            for (span, builtin) in found {
                emit::diagnostics::report_unsupported(
                    span,
                    target.as_str(),
                    "builtin alias",
                    format!(
                        "`{}` aliases `{}`, a method every Ruby object answers, which the {} emit has no counterpart for",
                        method.name.as_str(),
                        builtin.as_str(),
                        target.as_str()
                    ),
                );
            }
        }
    }
}

/// `attribute :x, :type, default: v` on a model: the ruby family's lazy,
/// cast default is verified there (`tests/emit_and_run/
/// active_model_attributes_form.rs`); a strict target emits the same IR
/// without the casts its runtime lacks, so the declaration is reported.
fn report_model_attribute_defaults(app: &App, target: BuildTarget) {
    if matches!(
        target,
        BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura
    ) {
        return;
    }
    for model in &app.models {
        for (name, default) in crate::lower::model_to_library::markers::attribute_api_defaults(&model.body) {
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                default.span,
                Some(crate::ident::Symbol::from(target.as_str())),
                format!("attribute :{}, default:", name.as_str()),
                "the default is read on first access and cast on the ruby family; this target's lowering is not verified",
            );
            d.severity = crate::diagnostic::Severity::Warning;
            emit::diagnostics::push(d);
        }
    }
}


// These readers are the ruby family's uri library's and Spinel's
// packages/uri's, not methods supplied by strict runtimes.
fn report_uri_reader_sends(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda) {
        return;
    }
    fn has_unported_uri(ty: &crate::ty::Ty, app: &App, reader: &str) -> bool {
        use crate::ty::Ty;
        match ty {
            // Reopening a URI class supplies only the readers the app defines.
            Ty::Class { id, .. } => matches!(id.0.as_str(), "URI::Generic" | "URI::HTTP" | "URI::HTTPS")
                && !crate::analyze::source_instance_method(app, id, reader),
            Ty::Union { variants } => variants.iter().any(|ty| has_unported_uri(ty, app, reader)),
            _ => false,
        }
    }
    fn visit(expr: &crate::expr::Expr, app: &App, target: BuildTarget) {
        use crate::expr::ExprNode;
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && matches!(method.as_str(), "scheme" | "host" | "port" | "path" | "query" | "fragment"
                | "userinfo" | "user" | "password" | "to_s" | "request_uri")
            && recv.ty.as_ref().is_some_and(|ty| has_unported_uri(ty, app, method.as_str()))
        {
            let owner = if method.as_str() == "request_uri" { "URI::HTTP" } else { "URI::Generic" };
            emit::diagnostics::report_unsupported(
                expr.span, target.as_str(), format!("{owner}#{}", method.as_str()),
                "this target's URI runtime does not implement this reader",
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, app, target));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, app, target));
}

fn report_ruby_only_sends(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda) {
        return;
    }
    // Not per call site, except for the conversions analysis marked
    // (`expr::KERNEL_CONVERSION`): a receiverless send in a model or library
    // body carries no mark of the method it dispatched to, so an app method
    // of the same name anywhere keeps its calls out of the refusal.
    let defines = |name: &str| {
        app.models.iter().any(|model| model.methods().any(|method| method.name.as_str() == name))
            || app.library_classes.iter().any(|class| class.methods.iter().any(|method| method.name.as_str() == name))
            || app.controllers.iter().any(|controller| controller.actions().any(|action| action.name.as_str() == name))
    };
    let kernel: Vec<&str> = ["Rational", "Integer", "Float", "throw", "catch", "update_attribute", "attribute_in_database", "create_or_find_by", "create_or_find_by!"]
        .into_iter()
        .filter(|name| !defines(name))
        .collect();
    fn names_app_path(ty: &crate::ty::Ty) -> bool {
        match ty {
            crate::ty::Ty::Class { id, .. } => id.0.as_str() == "Rails::AppPath",
            crate::ty::Ty::Union { variants } => variants.iter().any(names_app_path),
            _ => false,
        }
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget, kernel: &[&str]) {
        use crate::expr::ExprNode;
        use crate::ty::Ty;
        if let ExprNode::Send { recv, method, .. } = &*expr.node {
            let recv_ty = recv.as_ref().and_then(|r| r.ty.as_ref());
            let admitted = expr.decisions & crate::expr::KERNEL_CONVERSION != 0;
            let construct = match (recv, method.as_str()) {
                (None, "Rational") if kernel.contains(&"Rational") || admitted => Some("Kernel#Rational"),
                (None, "Integer") if kernel.contains(&"Integer") => Some("Kernel#Integer"),
                (None, "Float") if kernel.contains(&"Float") => Some("Kernel#Float"),
                (None, "URI") if admitted => Some("Kernel#URI"),
                // Lowering's spelling of an admitted `Pathname(path)`.
                (Some(_), "new") if admitted => Some("Kernel#Pathname"),
                (None, "throw") if kernel.contains(&"throw") => Some("Kernel#throw"),
                (None, "catch") if kernel.contains(&"catch") => Some("Kernel#catch"),
                (_, "update_attribute") if kernel.contains(&"update_attribute") => Some("ActiveRecord::Base#update_attribute"),
                (_, "attribute_in_database") if kernel.contains(&"attribute_in_database") => Some("ActiveRecord::Base#attribute_in_database"),
                (_, "create_or_find_by") if kernel.contains(&"create_or_find_by") => Some("ActiveRecord::Relation#create_or_find_by"),
                (_, "create_or_find_by!") if kernel.contains(&"create_or_find_by!") => Some("ActiveRecord::Relation#create_or_find_by!"),
                // Not filtered by what the app defines: the model's own `_run_x_callbacks` is the generated chain, built on yield and catch.
                (_, name) if name.starts_with("_run_") && name.ends_with("_callbacks") => Some("ActiveRecord::Base#run_callbacks"),
                // Not only `= false`: the setter lives in connection.rb, which only the ruby family loads, so even `= true` calls nothing elsewhere
                (Some(r), "record_timestamps=") if r.decisions & crate::expr::CLASS_OBJECT_VALUE != 0 => {
                    Some("ActiveRecord::Base.record_timestamps=")
                }
                (Some(r), "[]") => match &*r.node {
                    ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Array" => Some("Array[]"),
                    ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Hash" => Some("Hash[]"),
                    _ => None,
                },
                (Some(r), "try_convert") => match &*r.node {
                    ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Array" => Some("Array.try_convert"),
                    _ => None,
                },
                (Some(_), "fetch") if matches!(recv_ty, Some(Ty::Array { .. })) => Some("Array#fetch"),
                (Some(_), "each_slice") if matches!(recv_ty, Some(Ty::Array { .. })) => Some("Array#each_slice"),
                (Some(_), "each_cons") if matches!(recv_ty, Some(Ty::Array { .. })) => Some("Array#each_cons"),
                (Some(_), "lazy") if matches!(recv_ty, Some(Ty::Array { .. })) => Some("Array#lazy"),
                (Some(_), "with_object") if matches!(recv_ty, Some(Ty::Array { .. })) => Some("Enumerator#with_object"),
                (Some(_), "include?") if matches!(recv_ty, Some(Ty::Class { id, .. })
                    if matches!(id.0.as_str(), "Class" | "Module")) => Some("Module#include?"),
                // The uri library is the ruby family's and spinel's `packages/uri`; no strict runtime carries it.
                (Some(_), "escape") if matches!(recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "URI::RFC2396_Parser") => Some("URI::RFC2396_Parser#escape"),
                (Some(_), "unescape") if matches!(recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "URI::RFC2396_Parser") => Some("URI::RFC2396_Parser#unescape"),
                (Some(_), "escape") if matches!(recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "URI::RFC3986_Parser") => Some("URI::RFC3986_Parser#escape"),
                (Some(_), "unescape") if matches!(recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "URI::RFC3986_Parser") => Some("URI::RFC3986_Parser#unescape"),
                // `Rails.root`'s stand-in is runtime/ruby/rails.rb's, which only the ruby family and spinel load,
                // whichever arm of the receiver it is (`Rails::AppPath?`, `Rails::AppPath | Pathname`).
                (Some(_), "expand_path") if recv_ty.is_some_and(names_app_path) => Some("Rails::AppPath#expand_path"),
                _ => None,
            };
            if let Some(construct) = construct {
                crate::emit::diagnostics::report_unsupported(
                    expr.span, target.as_str(), construct,
                    "only the Ruby targets implement it; this target would emit the call on a receiver that does not define it",
                );
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, target, kernel));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target, &kernel));
}

/// An ancestor controller's method reading an ivar only a descendant
/// writes: analysis types it from the descendants (Phase B″), which the
/// ruby family runs as written, but a strict target's ancestor has no
/// such field to read.
fn report_descendant_ivar_reads(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    use crate::expr::{ExprNode, LValue};
    fn writes(expr: &crate::expr::Expr, out: &mut crate::det::HashSet<String>) {
        match &*expr.node {
            ExprNode::Assign { target: LValue::Ivar { name }, .. } | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => {
                out.insert(name.as_str().to_string());
            }
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    if let LValue::Ivar { name } = t {
                        out.insert(name.as_str().to_string());
                    }
                }
            }
            _ => {}
        }
        expr.node.for_each_child(&mut |child| writes(child, out));
    }
    let own: crate::det::HashMap<&crate::ident::ClassId, crate::det::HashSet<String>> = app
        .controllers
        .iter()
        .map(|c| {
            let mut out = crate::det::HashSet::default();
            for action in c.actions() {
                writes(&action.body, &mut out);
            }
            (&c.name, out)
        })
        .collect();
    let parent_of: crate::det::HashMap<&crate::ident::ClassId, &crate::ident::ClassId> =
        app.controllers.iter().filter_map(|c| c.parent.as_ref().map(|p| (&c.name, p))).collect();
    let chain = |start: &crate::ident::ClassId| {
        let mut out = vec![start.clone()];
        let mut cursor = parent_of.get(start).copied();
        while let Some(p) = cursor {
            if out.contains(p) {
                break;
            }
            out.push(p.clone());
            cursor = parent_of.get(p).copied();
        }
        out
    };
    for controller in &app.controllers {
        let mut seen: crate::det::HashSet<String> = crate::det::HashSet::default();
        for id in chain(&controller.name) {
            if let Some(w) = own.get(&id) {
                seen.extend(w.iter().cloned());
            }
        }
        let below: crate::det::HashSet<&String> = app
            .controllers
            .iter()
            .filter(|d| d.name != controller.name && chain(&d.name).contains(&controller.name))
            .filter_map(|d| own.get(&d.name))
            .flatten()
            .filter(|name| !seen.contains(*name))
            .collect();
        if below.is_empty() {
            continue;
        }
        fn reads(expr: &crate::expr::Expr, below: &crate::det::HashSet<&String>, target: BuildTarget) {
            if let ExprNode::Ivar { name } = &*expr.node
                && below.contains(&name.as_str().to_string())
            {
                crate::emit::diagnostics::report_unsupported(
                    expr.span, target.as_str(), "ivar written only by a subclass",
                    format!("@{} is set by a subclass's action; this target gives the ancestor no field to read it from", name.as_str()),
                );
            }
            expr.node.for_each_child(&mut |child| reads(child, below, target));
        }
        let spliced = app.concern_spliced_actions.get(&controller.name);
        // Not a concern's copy: its ivars are seeded from the concern's includers, which the analysis types separately
        for action in controller.actions().filter(|a| !spliced.is_some_and(|m| m.contains_key(&a.name))) {
            reads(&action.body, &below, target);
        }
    }
}

/// `include Rails.application.routes.url_helpers` in an app class: the
/// ruby emit routes its helper calls through the generated RouteHelpers
/// module; no strict target's class reaches that module's helpers.
fn report_url_helpers_includes(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    for class in &app.library_classes {
        for (i, included) in class.includes.iter().enumerate() {
            if included.0.as_str() != "RouteHelpers" {
                continue;
            }
            crate::emit::diagnostics::report_unsupported(
                class.include_spans.get(i).copied().unwrap_or_default(), target.as_str(), "include of the route url_helpers",
                format!("`{}` includes Rails.application.routes.url_helpers, whose helpers only the ruby emit gives a class", class.name.0),
            );
        }
    }
}

/// An operator that only some arms of its operands take (`Integer | String > Integer`):
/// a warning in analysis, since the ruby family runs it as Ruby does and raises for
/// the arms that miss. A strict target emits a raise for the whole expression, so it
/// is reported there. Nil-only misses were already checked by `lower::nil_operand`.
fn report_partial_binops(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby
        | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget) {
        if let Some(crate::diagnostic::DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty }) = &expr.diagnostic()
            && crate::analyze::some_operands_take_it(op.as_str(), lhs_ty, rhs_ty)
        {
            crate::emit::diagnostics::report_unsupported(
                expr.span, target.as_str(), "operator on some of its operands' types",
                format!("`{}` takes only some of the types its operands may hold; the ruby family raises for the rest as Ruby does, this target for all", op.as_str()),
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target));
}

/// A constant of an unknown gem (pinned in the lockfile, no façade, no
/// spin package): the CRuby tree requires the real gem, but nothing in
/// the Spinel tree defines it, so a subclass or a reference raises when
/// it runs (lobsters' `SearchParser < Parslet::Parser` stopped the server
/// at boot with a NoMethodError while the build was clean).
fn report_spinel_unknown_gem_constants(app: &App, target: BuildTarget) {
    if !matches!(target, BuildTarget::Spinel) {
        return;
    }
    let Some(lock) = app.gem_lock.as_ref() else { return };
    let census = crate::gems::GemCensus::of(lock);
    let carried = crate::gems::native_extension_gem_namespaces(app);
    let unknown: crate::det::HashSet<&str> = census
        .unknown()
        .filter(|gem| !lock.is_from_git(&gem.name) && !app.extensions.is_rails_dependent(&gem.name))
        .filter(|gem| !carried.contains(&crate::gems::namespace_of(&gem.name)))
        .map(|gem| gem.name.as_str())
        .collect();
    if unknown.is_empty() {
        return;
    }
    // The longest-matching gem across the whole lockfile, so `Rack::Utils` stays rack's while `Rack::MiniProfiler` is rack-mini-profiler's
    let ported = crate::gems::spinel_runtime_constants();
    let ownership = crate::gems::LockGemIndex::new(lock);
    let owner = |path: &str| -> Option<String> {
        if ported.contains(path) {
            return None;
        }
        let owners = ownership.owners(path);
        (!owners.is_empty() && owners.iter().all(|g| unknown.contains(g))).then(|| owners.join(", "))
    };
    let report = |span: crate::span::Span, path: &str, ns: &str| {
        crate::emit::diagnostics::report_unsupported(
            span, "spinel", "constant of an unknown gem",
            format!("`{path}` belongs to the {ns} gem, which the Spinel tree does not carry (no façade, no spin package); it raises where it runs"),
        );
    };
    for_each_app_constant(app, &mut |span, path| {
        if let Some(ns) = owner(path) {
            report(span, path, &ns);
        }
    });
}

/// Every constant the app's classes name that the app does not define itself:
/// superclasses, and each `Const` in an emitted body.
fn for_each_app_constant(app: &App, f: &mut dyn FnMut(crate::span::Span, &str)) {
    let own: crate::det::HashSet<&str> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str()))
        .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
        .collect();
    let f = &mut |span: crate::span::Span, path: &str| {
        if !own.contains(path) {
            f(span, path);
        }
    };
    let parents = app
        .models
        .iter()
        .map(|m| (m.parent.as_ref(), m.parent_span))
        .chain(app.library_classes.iter().map(|c| (c.parent.as_ref(), c.parent_span)))
        .chain(app.controllers.iter().map(|c| (c.parent.as_ref(), c.parent_span)));
    for (parent, span) in parents {
        if let Some(parent) = parent {
            f(span, parent.0.as_str());
        }
    }
    fn visit(expr: &crate::expr::Expr, f: &mut dyn FnMut(crate::span::Span, &str)) {
        if let crate::expr::ExprNode::Const { path } = &*expr.node {
            let joined = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            f(expr.span, &joined);
            return;
        }
        expr.node.for_each_child(&mut |child| visit(child, f));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, f));
}


/// Not left to the runtime's raise: `scope_chain` needs the receiver's model to write the join's SQL.
fn report_dynamic_association_joins(app: &App, target: BuildTarget) {
    if matches!(target, BuildTarget::Futamura) {
        return;
    }
    fn names_association(arg: &crate::expr::Expr) -> bool {
        use crate::expr::{ExprNode, Literal};
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { .. } } => true,
            ExprNode::Array { elements, .. } => elements.iter().any(names_association),
            ExprNode::Hash { entries, .. } => entries.iter().any(|(k, v)| names_association(k) || names_association(v)),
            _ => false,
        }
    }
    fn runtime_class(expr: &crate::expr::Expr) -> bool {
        use crate::expr::ExprNode;
        let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return false };
        matches!(method.as_str(), "constantize" | "safe_constantize" | "const_get") || runtime_class(r)
    }
    fn visit(expr: &crate::expr::Expr, target: BuildTarget) {
        use crate::expr::ExprNode;
        if let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node
            && matches!(method.as_str(), "joins" | "left_outer_joins" | "left_joins")
            && runtime_class(r)
            && args.iter().any(names_association)
        {
            crate::emit::diagnostics::report_unsupported(
                expr.span, target.as_str(), "association join on an unknown model",
                format!("`{}` names an association, which is resolved from the receiver's model at transpile time; this receiver's model is only known at run time", method.as_str()),
            );
        }
        expr.node.for_each_child(&mut |child| visit(child, target));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target));
}

/// A send on a typed Array, number or String receiver that the TypeScript emit has
/// no translation for: it writes the Ruby name on the JS value
/// (`xs.zip(ys)`, `n.digits` as a property read), which throws or reads
/// `undefined`. `emit::typescript::array_method_handled`,
/// `numeric_method_handled` and `string_method_handled` name what it does translate.
fn report_typescript_array_sends(app: &App, target: BuildTarget) {
    if !matches!(target, BuildTarget::Typescript | BuildTarget::TypescriptWorker) {
        return;
    }
    // (model, reader) of each collection association: its reader answers a CollectionProxy.
    let proxies: crate::det::HashSet<(&str, &str)> = app
        .models
        .iter()
        .flat_map(|m| {
            m.associations().filter_map(move |a| match a {
                crate::dialect::Association::HasMany { name, .. }
                | crate::dialect::Association::HasAndBelongsToMany { name, .. } => {
                    Some((m.name.0.as_str(), name.as_str()))
                }
                _ => None,
            })
        })
        .collect();
    let models: crate::det::HashSet<&str> = app.models.iter().map(|m| m.name.0.as_str()).collect();
    struct Records<'a> {
        proxies: crate::det::HashSet<(&'a str, &'a str)>,
        models: crate::det::HashSet<&'a str>,
    }
    /// A query chain off a model class or a collection reader (`Post.where(…)`,
    /// `product.subscribers.where(…)`): a Relation or CollectionProxy that types as
    /// an Array of the model. `to_a` and `records` end it in a real Array.
    fn is_query(e: &crate::expr::Expr, records: &Records) -> bool {
        use crate::expr::ExprNode;
        use crate::ty::Ty;
        match &*e.node {
            ExprNode::Const { path } => records.models.contains(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str()),
            ExprNode::Send { recv: Some(owner), method, .. } => {
                if matches!(method.as_str(), "to_a" | "records" | "load" | "entries") {
                    return false;
                }
                let reader = matches!(owner.ty.as_ref(), Some(Ty::Class { id, .. })
                    if records.proxies.contains(&(id.0.as_str(), method.as_str())));
                reader || is_query(owner, records)
            }
            _ => false,
        }
    }
    let records = Records { proxies, models };
    fn visit(expr: &crate::expr::Expr, target: BuildTarget, records: &Records) {
        use crate::expr::ExprNode;
        use crate::ty::Ty;
        if let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*expr.node {
            let elem = match r.ty.as_ref() {
                Some(Ty::Array { elem }) => Some(elem),
                Some(Ty::Union { variants }) if variants.len() == 2 && variants.iter().any(|v| matches!(v, Ty::Nil)) => {
                    variants.iter().find_map(|v| match v {
                        Ty::Array { elem } => Some(elem),
                        _ => None,
                    })
                }
                _ => None,
            };
            // Already refused by name in `report_ruby_only_sends`.
            let ruby_only = matches!(method.as_str(), "fetch" | "each_slice" | "each_cons" | "lazy" | "with_object");
            // A has_many reader types as an Array of its model, but answers a CollectionProxy (`build`, `create!`).
            if let Some(elem) = elem.filter(|_| !is_query(r, records)) {
                let argc = args.len() + usize::from(block.is_some());
                if !ruby_only && !crate::emit::typescript::array_method_handled(method.as_str(), argc, block.as_ref(), elem) {
                    crate::emit::diagnostics::report_unsupported(
                        expr.span,
                        target.as_str(),
                        format!("Array#{}", method.as_str()).as_str(),
                        "the TypeScript emit has no translation for it and would call the Ruby name on a JS Array",
                    );
                }
            }
            let number = match r.ty.as_ref() {
                Some(Ty::Int) => Some("Integer"),
                Some(Ty::Float) => Some("Float"),
                _ => None,
            };
            if let Some(class) = number {
                let argc = args.len() + usize::from(block.is_some());
                if !crate::emit::typescript::numeric_method_handled(method.as_str(), argc, block.as_ref()) {
                    crate::emit::diagnostics::report_unsupported(
                        expr.span,
                        target.as_str(),
                        format!("{class}#{}", method.as_str()).as_str(),
                        "the TypeScript emit has no translation for it and would call the Ruby name on a JS number",
                    );
                }
            }
            let string = match r.ty.as_ref() {
                Some(Ty::Str) => true,
                Some(Ty::Union { variants }) => {
                    variants.len() == 2 && variants.iter().any(|v| matches!(v, Ty::Nil)) && variants.iter().any(|v| matches!(v, Ty::Str))
                }
                _ => false,
            };
            if string && !crate::emit::typescript::string_method_handled(method.as_str(), args, block.as_ref()) {
                crate::emit::diagnostics::report_unsupported(
                    expr.span,
                    target.as_str(),
                    format!("String#{}", method.as_str()).as_str(),
                    "the TypeScript emit has no translation for it and would call the Ruby name on a JS string",
                );
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, target, records));
    }
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, target, &records));
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

/// `--target futamura`'s tree: the app (`futamura_files`), its query
/// specializations, and, given the `ruby` target's side
/// (`futamura_whole_route`), whole-route replacement for the GET actions
/// the analysis clears.
pub fn futamura_target_files(
    app: &App,
    fixture: &Path,
    whole_route: Option<&emit::futamura::WholeRoute>,
) -> Result<Vec<(String, String)>, String> {
    let target = BuildTarget::Futamura;
    let ledger = std::env::var_os("ROUNDHOUSE_FUTAMURA_LEDGER").is_some();
    let mut files = futamura_files(fixture)?;
    let report = emit::futamura::specialize(app, &mut files)?;
    eprintln!(
        "roundhouse: futamura: specialized {} query site(s); {} left to Rails{}",
        report.specialized.len(),
        report.residue.len(),
        if ledger { "" } else { " (ROUNDHOUSE_FUTAMURA_LEDGER=1 lists them)" },
    );
    if ledger {
        for s in &report.specialized {
            eprintln!("  specialized {s}");
        }
        for r in &report.residue {
            eprintln!("  residue     {r}");
        }
    }
    if let Some(wr) = whole_route {
        let plan = emit::futamura::whole_route(&mut files, wr)?;
        eprintln!(
            "roundhouse: futamura: whole-route: {} GET action(s) served by the lowered app; {} left to Rails{}",
            plan.actions.len(),
            plan.residue.len(),
            if ledger { "" } else { " (ROUNDHOUSE_FUTAMURA_LEDGER=1 lists them)" },
        );
        if ledger {
            for a in &plan.actions {
                eprintln!("  lowered     {a}");
            }
            for r in &plan.residue {
                eprintln!("  rails       {r}");
            }
            for u in &plan.unowned {
                eprintln!("  unowned     {u}");
            }
        }
    }
    Ok(ensure_readme(files, target))
}

/// The `ruby` target's half of `--target futamura`'s whole-route
/// replacement, from the INGESTED app: lowered the way `--target ruby`
/// lowers it, with every diagnostic that emit would report. Errors do
/// not stop it: under this target an error keeps the routes that reach
/// it on Rails (`emit::futamura::routes`).
pub fn futamura_whole_route(
    ingested: &App,
    fixture: &Path,
    gaps: &[crate::ingest::IngestError],
) -> Result<emit::futamura::WholeRoute, String> {
    let mut lowered = ingested.clone();
    let mut diags = crate::session::analyze_and_lower_for(&mut lowered, BuildTarget::Ruby);
    diags.extend(crate::analyze::diagnose(&lowered));
    crate::lower::relation_residue::elevate_dynamic_relation_for_target(&mut diags, BuildTarget::Ruby);
    crate::lower::params_residue::elevate_nested_params_for_target(&mut diags, BuildTarget::Ruby);
    // Guarded so the box still loads if the planner leaves a route in
    // place that reaches a file with an unresolved include — see
    // `emit::ruby::expr::FUTAMURA_BOX`. Plain `--target ruby` never
    // wraps this and stays fail-loud.
    let (files, emit_diags) = emit::diagnostics::scope(|| {
        crate::emit::ruby::expr::with_futamura_box(|| target_files(&lowered, fixture, BuildTarget::Ruby))
    });
    diags.extend(emit_diags);
    // Ingest gaps aren't diagnostics (survey mode substitutes a nil
    // placeholder and keeps going), so they never reach `diags` above;
    // carried separately for `routes::plan` to attribute to the routes
    // whose controller or view they belong to.
    //
    // One recorded gap is excluded on purpose: `ingest::app::
    // CSP_DIVERGENCE_MESSAGE` names a `content_security_policy` block
    // this ingest can read well enough to know it does nothing but
    // build a policy object (INTERIM, rubys/roundhouse#58, CSP option
    // 2 — see `ingest::app::csp_block_is_recognized`'s doc comment).
    // It still belongs in the survey's punch list (recorded the same
    // way every other gap is, via `IngestError::Unsupported`, so
    // `--survey` and `check --continue` show it), but it must not
    // reach `routes::plan`'s gap-blocking: the directive builders
    // aren't modelled either way, so the lowered app sends no CSP
    // header, but that is a recorded DIVERGENCE, not a reason to keep
    // every route behind it on Rails.
    let gaps = gaps
        .iter()
        .filter_map(|g| match g {
            crate::ingest::IngestError::Unsupported { file, message } => {
                if message == crate::ingest::app::CSP_DIVERGENCE_MESSAGE {
                    return None;
                }
                Some((file.clone(), message.clone()))
            }
            crate::ingest::IngestError::Io(_) | crate::ingest::IngestError::Parse { .. } => None,
        })
        .collect();
    Ok(emit::futamura::WholeRoute { lowered, files: files?, diags, gaps })
}

pub fn target_files(
    app: &App,
    fixture: &Path,
    target: BuildTarget,
) -> Result<Vec<(String, String)>, String> {
    // Stage 0 of the specializer: the identity. Nothing is specialized
    // yet, so none of the trimming or refusals below apply — every construct is
    // residue, and the residue runs on Rails.
    if target == BuildTarget::Futamura {
        return futamura_target_files(app, fixture, None);
    }
    let refusals = crate::gems::extension_gem_refusals(app, Some(target.as_str()));
    if !refusals.is_empty() {
        let message = refusals.iter().map(|diagnostic| diagnostic.message.as_str()).collect::<Vec<_>>().join("\n");
        for diagnostic in refusals {
            emit::diagnostics::push(diagnostic);
        }
        return Err(message);
    }
    if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
        let (namespaces, includes) = if target == BuildTarget::Ruby {
            (crate::gems::locked_external_gem_namespaces(app), crate::gems::statically_resolved_external_includes(app))
        } else {
            (crate::gems::native_extension_gem_namespaces(app), Default::default())
        };
        return crate::emit::ruby::with_external_gem_namespaces(&namespaces, includes, || {
            crate::emit::ruby::expr::with_body_statements(|| target_files_inner(app, fixture, target))
        });
    }
    target_files_inner(app, fixture, target)
}

/// The ruby family runs an automatically sourced gem as the real gem,
/// through its Gemfile and require (`ingest::auto_source_gems`): its
/// compiled classes leave this tree, and the gem reads as the unknown
/// boundary the Gemfile wiring declares.
fn without_auto_source_gems(app: &App, target: BuildTarget) -> Option<App> {
    if app.auto_source_gems.is_empty() || !matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
        return None;
    }
    let lock = app.gem_lock.as_ref();
    let mounted = |file: crate::span::FileId| {
        app.source_path(file).is_some_and(|path| {
            app.auto_source_gems.iter().any(|gem| {
                lock.and_then(|l| l.version_of(gem)).is_some_and(|v| crate::ingest::source_gems::file_of(path, gem, v))
            })
        })
    };
    let mut trimmed = app.clone();
    // A class records no span of its own: the first real file among its parts names where it was written.
    let file_of_class = |c: &crate::dialect::LibraryClass| {
        // The class's own header first: a macro the gem defines (parslet's `rule`) leaves its expanded methods' spans in the gem.
        std::iter::once(c.parent_span.file)
            .chain(c.include_spans.iter().map(|span| span.file))
            .chain(c.constants.iter().map(|(_, value)| value.span.file))
            .chain(c.methods.iter().map(|m| m.body.span.file))
            .find(|file| file.0 != 0)
    };
    let dropped: crate::det::HashSet<String> = app
        .library_classes
        .iter()
        .filter(|c| file_of_class(c).is_some_and(mounted))
        .map(|c| c.name.0.to_string())
        .collect();
    // A partless class (a bare namespace reopening, a `Struct.new` constant) has no file to tell, so the gem's own namespace decides.
    let under_dropped = |name: &str| {
        let outer = name.rsplit_once("::").map_or(name, |(outer, _)| outer);
        dropped.contains(name) || dropped.contains(outer) || dropped.iter().any(|d| d.starts_with(&format!("{name}::")))
    };
    trimmed.library_classes.retain(|c| match file_of_class(c) {
        Some(file) => !mounted(file),
        None => !under_dropped(c.name.0.as_str()),
    });
    trimmed.models.retain(|m| !mounted(m.parent_span.file));
    // Its files' programs too: the real gem runs its own top level.
    let mounted_path = |path: &str| {
        app.auto_source_gems.iter().any(|gem| {
            lock.and_then(|l| l.version_of(gem)).is_some_and(|v| crate::ingest::source_gems::file_of(path, gem, v))
        })
    };
    trimmed.file_programs.retain(|p| !mounted_path(&p.source));
    trimmed.boot_gem_entries.retain(|e| !mounted_path(e));
    if let Some(lock) = trimmed.gem_lock.as_mut() {
        lock.source.retain(|gem| !app.auto_source_gems.contains(gem));
    }
    trimmed.auto_source_gems.clear();
    Some(trimmed)
}

fn target_files_inner(
    app: &App,
    fixture: &Path,
    target: BuildTarget,
) -> Result<Vec<(String, String)>, String> {
    if !matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
        refuse_conditional_definitions(app, target);
    }
    let without_auto_sources;
    let app = match without_auto_source_gems(app, target) {
        Some(trimmed) => {
            without_auto_sources = trimmed;
            &without_auto_sources
        }
        None => app,
    };
    // The Spinel tree carries only the part of a gem compiled from source the program reaches.
    let shaken_gems;
    let app = match (target == BuildTarget::Spinel).then(|| crate::source_gem_shake::shake(app)).flatten() {
        Some(trimmed) => {
            shaken_gems = trimmed;
            &shaken_gems
        }
        None => app,
    };
    let without_health;
    let app = match without_rails_health_controller(app, target) {
        Some(trimmed) => {
            without_health = trimmed;
            &without_health
        }
        None => app,
    };
    // One gate for the Oj stand-in: ruby, jruby and spinel keep the class
    // ingest installed. Every other transpile target drops it here, before
    // the bundled-constant and TypeScript walks, so a missing `JSON` or
    // `String#read` inside the class is not a second, incidental error.
    let without_oj;
    let app = match crate::lower::oj::without_unrun_stand_in(app, target) {
        Some(trimmed) => {
            without_oj = trimmed;
            &without_oj
        }
        None => app,
    };
    // The gems only the ruby family ships (Faraday, Retryable, JWT, ...) run
    // as the real gems on ruby and jruby; every other target refuses each
    // reference (`lower::ruby_family_gems`).
    let without_ruby_family_gems;
    let app = match crate::lower::ruby_family_gems::without_unshipped_gems(app, target) {
        Some(trimmed) => {
            without_ruby_family_gems = trimmed;
            &without_ruby_family_gems
        }
        None => app,
    };
    let with_closed_constantize;
    let app = match crate::lower::constantize::lower_for_target(app, target) {
        Some(lowered) => {
            with_closed_constantize = lowered;
            &with_closed_constantize
        }
        None => app,
    };
    let with_nil_operands;
    let app = match crate::lower::nil_operand::lower_for_target(app, target) {
        Some(lowered) => {
            with_nil_operands = lowered;
            &with_nil_operands
        }
        None => app,
    };
    let with_where_bounds;
    let app = match crate::lower::where_bounds::lower_for_target(app, target) {
        Some(lowered) => {
            with_where_bounds = lowered;
            &with_where_bounds
        }
        None => app,
    };
    let with_eager_loads;
    let app = match crate::lower::eager_load::lower_for_target(app, target) {
        Some(lowered) => {
            with_eager_loads = lowered;
            &with_eager_loads
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
    report_i18n_table_sites(app, target);
    // Snapshot the optional measurement override at the target boundary. The
    // lowerers receive a value; they never consult process-global environment.
    let param_binds = target.param_binds_enabled(
        std::env::var("ROUNDHOUSE_PARAM_BINDS").ok().as_deref(),
    );
    reject_unsupported_pattern_matches(app, target)?;
    reject_file_blob_fixtures(app, target)?;
    reject_unsupported_data_factories(app, target)?;
    reject_unsupported_dates(app, target)?;
    reject_unsupported_forwarded_procs(app, target)?;
    report_unsupported_keys(app, target);
    report_unemitted_library_classes(app, target);
    report_relation_class_calls(app, target);
    report_typescript_error_class(app, target);
    report_replayed_class_bodies(app, target);
    report_controller_sibling_classes(app, target);
    // `validates …, uniqueness:` left out of a target with no runtime
    // Relation: reported here, where the CLI and the MCP `wont_lower`
    // both collect a target's gaps, so neither reports a clean emit.
    for d in crate::lower::relation_residue::uniqueness_gaps_for_target(app, target) {
        emit::diagnostics::push(d);
    }
    // `acceptance:` on an `attribute :x, :boolean`, left out where the
    // writer cannot keep Rails' nil.
    for d in crate::lower::validations::acceptance_gaps_for_target(app, target) {
        emit::diagnostics::push(d);
    }
    for d in crate::lower::relation_residue::default_scope_gaps_for_target(app, target) {
        emit::diagnostics::push(d);
    }
    for d in crate::lower::model_to_library::select_aliases::select_alias_gaps_for_target(app, target) {
        emit::diagnostics::push(d);
    }
    report_mailers(app, target);
    report_sqlite_index_predicates(app, target);
    report_array_columns(app, target);
    report_forgery_protection(app, target);
    report_native_ruby_syntax(app, target);
    report_self_bindings(app, target);
    report_ruby_only_sends(app, target);
    report_uri_reader_sends(app, target);
    report_descendant_ivar_reads(app, target);
    report_url_helpers_includes(app, target);
    report_partial_binops(app, target);
    report_spinel_unknown_gem_constants(app, target);
    report_spinel_gem_package_settings(app, target);
    report_model_attribute_defaults(app, target);
    report_builtin_aliases(app, target);
    report_dynamic_association_joins(app, target);
    report_typescript_array_sends(app, target);
    report_dry_struct_stdlib(app, target);
    // Full forwarding currently has a native Ruby contract only. A
    // declaration must be gated even when its body never forwards.
    if !matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby) {
        for (span, policy) in crate::analyze::forwarding::keyword_calls(app) {
            if !matches!(policy, crate::analyze::forwarding::KeywordPolicy::Legacy | crate::analyze::forwarding::KeywordPolicy::NativeLibrary) {
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
        let model_class_ivars = app.models.iter().any(|m| {
            m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if crate::lower::model_to_library::writes_class_ivar(expr)))
        });
        if model_class_ivars || app.library_classes.iter().any(|lc| !lc.class_ivar_initializers.is_empty()) {
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
            let mut methods = crate::det::HashSet::default();
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
    let files = crate::timings::phase(format_args!("emit {}: assemble", target.as_str()), || crate::runtime_loader::with_app_locales(app, || match target {
        BuildTarget::Blog => blog_files(fixture),
        BuildTarget::Futamura => futamura_files(fixture),
        BuildTarget::Spinel => spinel_files_with_source_markers(app, fixture, param_binds).and_then(|(mut files, _)| {
            spinel_relation_model_handle(&mut files)?;
            widen_spinel_hash_config_arrays(&mut files);
            let mut files = spin_shape(files)?;
            apply_spinel_int_overflow(&mut files, app);
            Ok(files)
        }),
        // The ruby family gets the bundled-library requires too: the
        // table used to live inside `spin_shape` and so reached only
        // the spinel tree, which cost campfire two test files on a
        // Ruby 3.4 runner (`Pathname()`).
        // In the dialect of the app's own database (`Dialect::for_app`):
        // the Ruby target runs PostgreSQL on the pg gem
        // (`apply_postgresql_tree`), every other target SQLite.
        BuildTarget::Ruby => {
            let dialect = crate::emit::shared::schema_sql::Dialect::for_app(app);
            dialect.scope(|| ruby_runtime_files(app, fixture, param_binds && dialect == crate::emit::shared::schema_sql::Dialect::Sqlite))
                .map(with_bundled_requires)
        }
        BuildTarget::Jruby => jruby_runtime_files(app, fixture, param_binds).map(with_bundled_requires),
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
    }))?;
    let mut files = files;
    // The Ruby family writes its table with its runtime (`spinel_files`); a strict target reads it from the same path.
    if ships_i18n_runtime(target)
        && !matches!(target, BuildTarget::Blog | BuildTarget::Futamura | BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby)
        && (!app.i18n_table.is_empty() || !app.i18n_runtime_sites.is_empty())
    {
        files.push(("config/i18n_translations".to_string(), i18n_table_text(app)));
    }

    // Ruby-family trees ship the framework runtime as verbatim text, so
    // their tree-shake runs here, on the finished file set (after
    // overlays — an overlay-only caller must count as a root), as a
    // text-level pass: see emit::ruby::shake. Other targets shake in IR
    // during emit.
    let files = if matches!(
        target,
        BuildTarget::Spinel | BuildTarget::Ruby | BuildTarget::Jruby
    ) {
        let mut synth_shakeable: crate::det::HashSet<String> = app
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
        // The app's reopens of runtime classes first, on the same
        // finished set: an override retires the runtime's def, and the
        // shake then reads the tree that ships (see emit::ruby::reopen).
        let models = app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
        emit::ruby::reopen::apply_runtime_reopens(&mut files, &models);
        if target == BuildTarget::Spinel {
            emit::ruby::shake::drop_shadowed_base_defs(&mut files);
        }
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
        let files = ensure_static_assets(files, target, app);
        ensure_e2e(files, target)
    };
    Ok(with_sidekiq_readme(ensure_readme(files, target), target, app))
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

/// A `### Jobs on Sidekiq` part at the end of the README's `## Run`, for a
/// tree that [`needs_redis`]: how to start the worker role, what its
/// options default to, and where its broadcasts go. A `###` heading, so
/// `scripts/smoke` skips it with the rest of `## Run` (the worker runs in
/// the foreground).
fn with_sidekiq_readme(mut files: Vec<(String, String)>, target: BuildTarget, app: &App) -> Vec<(String, String)> {
    let worker = match target {
        BuildTarget::Spinel => "./build/bin/blog sidekiq",
        BuildTarget::Ruby => "bundle exec ruby sidekiq.rb",
        BuildTarget::Jruby => "jruby -S bundle exec jruby sidekiq.rb",
        _ => return files,
    };
    if !needs_redis(app) {
        return files;
    }
    let cable = match cable_redis(app) {
        Some(c) if c.channel_prefix.is_empty() => {
            "config/cable.yml's production adapter is `redis`, so every broadcast, \
             the worker's and the server's, is a `PUBLISH` on its stream name, and \
             each server process delivers what it receives to its own sockets."
                .to_string()
        }
        Some(c) => format!(
            "config/cable.yml's production adapter is `redis`, so every broadcast, \
             the worker's and the server's, is a `PUBLISH` on `{}:<stream>`, and \
             each server process delivers what it receives to its own sockets.",
            c.channel_prefix
        ),
        None => "config/cable.yml's production adapter isn't `redis`, so a broadcast a job \
                 makes in the worker reaches no browser (the transpile warns about this)."
            .to_string(),
    };
    let section = format!(
        "### Jobs on Sidekiq\n\
         The app is on `queue_adapter :sidekiq`, so `perform_later` pushes each job \
         to Redis (`REDIS_URL`, or `redis://localhost:6379`) in Sidekiq 8's format. \
         Run them with the same build in a second process (or with a Ruby Sidekiq \
         process on the Rails app):\n\
         ```sh\n\
         {worker}\n\
         ```\n\
         `-c N` sets the processor threads (default: config/sidekiq.yml's \
         `:concurrency:`, then `RAILS_MAX_THREADS`, then 5), `-q queue[,weight]` the \
         queues (repeatable; default: config/sidekiq.yml's `:queues:`, then \
         `default`), and `-t seconds` the shutdown timeout (default: \
         config/sidekiq.yml's `:timeout:`, then 25). Each thread has its own Redis \
         connection, and the database pool is sized to the threads plus headroom \
         unless `DATABASE_POOL_SIZE` is set. TSTP stops fetching; TERM or INT \
         waits for running jobs up to the timeout and pushes the rest back. A job \
         that raises is retried with Sidekiq's backoff (up to config/sidekiq.yml's \
         `:max_retries:`, then 25), then goes to the dead set, and the worker moves \
         due retries and scheduled jobs back onto their queues, as Sidekiq's \
         scheduler does.\n\n\
         {cable}\n\n"
    );
    if let Some((_, readme)) = files.iter_mut().find(|(p, _)| p == "README.md") {
        let Some(run) = readme.find("## Run\n") else { return files };
        let end = readme[run + 1..].find("\n## ").map(|i| run + 2 + i).unwrap_or(readme.len());
        readme.insert_str(end, &section);
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
/// The fingerprinted stylesheet Propshaft will request for a baked
/// logical file such as `tailwind.css`. Nested paths stay put: the
/// asset graph bakes those under the logical name the importmap uses.
fn propshaft_content_digest(content: &str) -> String {
    use sha1::Digest;
    let digest = format!("{:x}", sha1::Sha1::digest(content.as_bytes()));
    digest[..8].to_string()
}

/// Write `content` at the fingerprint of those bytes and point every
/// Propshaft lookup of the ingest-time name at that path. The old
/// fingerprinted path stays free so `write_binary_assets` can still
/// materialize the source-compiled bytes there.
fn place_baked_stylesheet(files: &mut Vec<(String, String)>, app: &App, rel: &str, content: &str) {
    let Some(old) = baked_propshaft_name(app, rel) else {
        return;
    };
    let Some(stem) = rel.strip_suffix(".css") else {
        return;
    };
    let name = format!("{stem}-{}", propshaft_content_digest(content));
    let digested = format!("{name}.css");
    let digested_path = format!("static/assets/{digested}");
    if !files.iter().any(|(path, _)| path == &digested_path) {
        files.push((digested_path, content.to_string()));
    }
    if digested != old {
        let old_url = format!("/assets/{old}");
        let new_url = format!("/assets/{digested}");
        for (_, file) in files.iter_mut() {
            if file.contains(&old_url) {
                *file = file.replace(&old_url, &new_url);
            }
        }
    }
}

fn baked_propshaft_name(app: &App, rel: &str) -> Option<String> {
    let stem = rel.strip_suffix(".css")?;
    if stem.contains('/') {
        return None;
    }
    let prefix = format!("static/assets/{stem}-");
    app.binary_assets.iter().find_map(|(path, _)| {
        (path.starts_with(&prefix) && path.ends_with(".css"))
            .then(|| path["static/assets/".len()..].to_string())
    })
}

fn ensure_static_assets(
    mut files: Vec<(String, String)>,
    target: BuildTarget,
    app: &App,
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
            files.push((path, content.clone()));
        }
        // The layout asks for the fingerprinted name. That name has to be
        // the digest of these bytes, not of the unbuilt source Propshaft
        // hashed at ingest, or the archive serves CSS that does not match
        // its fingerprint.
        place_baked_stylesheet(&mut files, app, &rel, &content);
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

/// Give each emitted file the executable bits its source counterpart
/// has. Only the Futamura target needs it: its tree IS the source app,
/// whose `bin/rails` and friends must still run. Every other target
/// documents its run step as `ruby main.rb`-shaped commands.
pub fn copy_exec_bits(src: &Path, files: &[(String, String)], dest: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for (rel, _) in files {
            let Ok(meta) = fs::metadata(src.join(rel)) else { continue };
            let exec = meta.permissions().mode() & 0o111;
            if exec == 0 {
                continue;
            }
            let path = dest.join(rel);
            let mut perms = fs::metadata(&path)
                .map_err(|e| format!("stat {}: {e}", path.display()))?
                .permissions();
            perms.set_mode(perms.mode() | exec);
            fs::set_permissions(&path, perms).map_err(|e| format!("chmod {}: {e}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (src, files, dest);
    Ok(())
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

/// Stage-0 Futamura tree: the app as source control holds it. Unlike
/// [`blog_files`] it keeps dotfiles (`.ruby-version` decides which Ruby
/// runs the residue) and the `.keep` files that hold Rails' empty
/// directories open. What it leaves out is run-time state, named by
/// the paths a generated Rails app's `.gitignore` excludes. Binary
/// files travel separately, as `App::binary_assets`.
fn futamura_files(app_root: &Path) -> Result<Vec<(String, String)>, String> {
    const UNWALKED: [&str; 2] = [".git", "node_modules"];
    const STATE: [&str; 5] = ["log/", "tmp/", "storage/", "app/assets/builds/", "public/assets/"];
    let generated_state = |rel: &str| {
        (STATE.iter().any(|dir| rel.starts_with(dir)) && !rel.ends_with("/.keep"))
            || rel.starts_with(".bundle")
            || rel == "config/master.key"
    };
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("read entry: {e}"))?;
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip prefix: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().map_err(|e| format!("stat: {e}"))?.is_dir() {
                if !UNWALKED.contains(&rel.as_str()) {
                    walk(root, &path, out)?;
                }
            } else {
                out.push((rel, path));
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    walk(app_root, app_root, &mut paths)?;
    let mut files: Vec<(String, String)> = paths
        .into_iter()
        .filter(|(rel, _)| !generated_state(rel))
        .filter_map(|(rel, path)| fs::read_to_string(&path).ok().map(|content| (rel, content)))
        .collect();
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
    # A model class method a relation reaches (`User.active.find_by_x(id)`)
    # takes the relation as a trailing `__rel`, the lowering's spelling of
    # Rails' implicit scoping, so the call carries one argument more than
    # the test's `with(...)` names. It is matched by an optional
    # `anything` — the stubbed method is the one the test wrote either way.
    # Read before the first stub replaces the method's parameter list, and
    # remembered, since a second stub of the same method sees mocha's.
    @rel_params ||= {}
    rel_param =
      %w[stubs expects].include?(kind.to_s) &&
      @rel_params.fetch([target, meth.to_sym]) do |key|
        @rel_params[key] = target.respond_to?(meth) && target.method(meth).parameters.last&.last == :__rel
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
      args = args + [matcher(:optionally, matcher(:anything))] if op == :with && rel_param && !blk
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
    param_binds: bool,
) -> Result<Vec<(String, String)>, String> {
    ruby_family_runtime_files(app, fixture, RubyFlavor::CRuby, param_binds)
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
    param_binds: bool,
) -> Result<Vec<(String, String)>, String> {
    ruby_family_runtime_files(app, fixture, RubyFlavor::JRuby, param_binds)
}

fn ruby_family_runtime_files(
    app: &App,
    fixture: &Path,
    flavor: RubyFlavor,
    param_binds: bool,
) -> Result<Vec<(String, String)>, String> {
    // Keep the original immutable App borrowed across base assembly and overlays.
    let _indexes = emit::ruby::install_lookup_indexes(app);
    let _body_names = crate::lower::model_to_library::schema::share_model_body_names(&app.models, &Default::default());
    let (mut files, test_stems) = spinel_files(app, fixture, param_binds)?;

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
    let sidekiq = needs_redis(app);
    // On these lanes the redis gem is required at the first push, not at
    // boot: a tree whose tests record pushes through `SidekiqPush`'s seam
    // then boots without the gem installed. Spinel resolves requires at
    // compile time, so its tree keeps the file's top-level require.
    let sidekiq_client = if sidekiq {
        let client = crate::runtime_files::read_to_string(SIDEKIQ_REDIS_CLIENT)?;
        const AT_LOAD: &str = "require \"redis\"\n";
        const OPEN: &str = "    conn = password == \"\" ? Redis.new(url: url, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT) : Redis.new(url: url, password: password, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT)\n";
        if !client.contains(AT_LOAD) || !client.contains(OPEN) {
            return Err(format!("{SIDEKIQ_REDIS_CLIENT}: the require or the connect line moved"));
        }
        client
            .replacen(AT_LOAD, "# `require \"redis\"` is at the first connect on this lane.\n", 1)
            .replacen(OPEN, &format!("    require \"redis\"\n{OPEN}"), 1)
    } else {
        String::new()
    };
    let sidekiq_worker = if sidekiq {
        crate::runtime_files::read_to_string(SIDEKIQ_WORKER)?
    } else {
        String::new()
    };
    // The Redis broadcast transport: redis-rb has no `Redis.pubsub`, and
    // a plain connection subscribes there; the gem is required at the
    // first connect, as for the push. Broadcasts' transport on this lane
    // takes the String or Hash `Cable::Registry` would, so it encodes
    // here what spinel's `Cable.publish_raw` is handed encoded.
    let redis_cable = if cable_redis(app).is_some() {
        let real = crate::runtime_files::read_to_string(REDIS_CABLE)?;
        const AT_LOAD: &str = "require \"redis\"\n";
        const SUBSCRIBER: &str = "    Redis.pubsub(url: url)\n";
        if !real.contains(AT_LOAD) || !real.contains(SUBSCRIBER) {
            return Err(format!("{REDIS_CABLE}: the require or the subscriber line moved"));
        }
        let mut real = real
            .replacen(AT_LOAD, "# `require \"redis\"` is at the first connect on this lane.\n", 1)
            .replacen(SUBSCRIBER, "    require \"redis\"\n    Redis.new(url: url)\n", 1);
        real.push_str(REDIS_CABLE_RUBY_TRANSPORT);
        real
    } else {
        String::new()
    };
    for (path, content) in files.iter_mut() {
        if path == "runtime/ipaddr.rb" {
            *content = IPADDR_STDLIB.to_string();
        }
        // `Zlib`: same swap, and here it is a correctness one as much
        // as a speed one. The port computes the checksum in Ruby a bit
        // at a time; CRuby's is zlib's own C. Both answer the same
        // number by construction (the port IS CRC-32/ISO-HDLC), so the
        // tree that has the real one should use it.
        // `TypeID`: the gem itself (`gems::GEM_REQUIRES` and the
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
        // `SidekiqRedis`: the real connection, over the redis-rb gem, for
        // a tree on queue_adapter :sidekiq (`needs_redis`).
        if path == "runtime/sidekiq_redis.rb" && sidekiq {
            *content = sidekiq_client.clone();
        }
        if path == "runtime/sidekiq_worker.rb" && sidekiq {
            *content = sidekiq_worker.clone();
        }
        if path == "runtime/redis_cable.rb" && !redis_cable.is_empty() {
            *content = redis_cable.clone();
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
                    // Below the file's magic comments, or the requires would
                    // switch its `frozen_string_literal` off.
                    insert_require_line(content, TIME_PARSING_REQUIRE.trim_end());
                    if sql_functions.is_some() {
                        insert_require_line(content, "require_relative \"sql_functions\"");
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
            apply_database_pragmas(&mut files, app);
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
    // A demand-gated require (`gems::DEMAND_REQUIRES`) whose constant no
    // app body names stays out of the list.
    let undemanded: Vec<&str> = DEMAND_REQUIRES
        .iter()
        .filter(|(_, konst)| !files.iter().any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && names_constant(c, konst)))
        .map(|(require, _)| *require)
        .collect();
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
            gem_require_block(&undemanded, &lock_gated_requires(app)),
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
                gem_require_block(&[&["markly"][..], &undemanded].concat(), &lock_gated_requires(app)),
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

    // Same reasoning for the extras façades (Markdowner and the rest of
    // `facades::EXTRAS_FACADES`): CRuby has the real gems and stdlib and
    // the vendored source runs as written, so put the verbatim
    // source-shape emit back over the scaffold base's raising façade.
    emit::ruby::restore_extras_facades(&mut files, app);

    crate::runtime_files::walk_into("runtime/spinel/scaffold/ruby_overlay",
        "",
        &mut files,
    )?;
    // `ruby sidekiq.rb`, the worker role's entry, only where there is a
    // worker to run.
    if !sidekiq {
        files.retain(|(p, _)| p != "sidekiq.rb");
    }

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
    files.extend(sort_files(emit::ruby::emit_lowered_controllers_with_layout_and_param_binds(app, param_binds)));

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
    apply_runtime_gem_wiring(&mut files, app)?;
    apply_lock_gated_gem_wiring(&mut files, app)?;
    // The general unknown-gem fallback remains CRuby-only. Extension facts
    // above explicitly select native gems for both Ruby-family targets.
    if flavor == RubyFlavor::CRuby {
        apply_locked_external_gem_wiring(&mut files, app);
    }
    // AGAIN, on purpose, and this time PERFORMING them. `spinel_files`
    // appended a commented-out block to the spinel tree's boot.rb (that
    // target cannot perform a mixin — see `apply_module_mixins`), and the
    // overlay's own boot.rb then replaced that file wholesale at
    // `dedupe_last_wins` above, taking the block with it. Each final tree
    // gets the block exactly once; this is the ruby family's turn, and
    // the ruby family is the one that can run the lines.
    apply_module_mixins(&mut files, app, MixinForm::ExplicitReceiver);
    // Overlay boot replaced the spinel boot, so the flags written into
    // that file in `spinel_files` are gone. Write them again here.
    apply_environment_flags(&mut files, app);
    apply_app_requires(&mut files, app, RequireTarget::RubyFamily);
    apply_rails_version(&mut files, app);
    // Overlay boot replaces the spinel boot wholesale (see comment above
    // on apply_module_mixins), wiping the Date-package inject from
    // `spinel_files`. Re-inject only the shared `date_*` helpers — not
    // Spinel's Date polyfill or date-JSON reopen (see
    // `inject_cruby_date_calendar_helpers`).
    if app_uses_date(app) {
        inject_cruby_date_calendar_boot_requires(&mut files)?;
    }
    inject_array_column_require(&mut files, app)?;
    // Re-apply after the gem_facades rewrite above: the CRuby/JRuby
    // content replaces the spinel-tree file that `apply_pay_demand`
    // already annotated in `spinel_files`.
    apply_pay_demand(&mut files, app)?;
    if flavor == RubyFlavor::CRuby && crate::emit::shared::schema_sql::Dialect::current() == crate::emit::shared::schema_sql::Dialect::Postgres {
        apply_postgresql_tree(&mut files, app)?;
    }
    apply_conditional_declarations(&mut files, app);
    apply_file_programs(&mut files, app);
    Ok(files)
}

/// A class or module a load-time conditional declares
/// (`ConditionalArm::declarations`) loads where its branch runs, from the
/// `require_relative` the conditional writes there; nothing else loads its
/// file: not app/models.rb, nor another file's dependency require at its
/// top. One whose file belongs to an enclosing class (a class declared in
/// a conditional inside a class) cannot load apart from it, and is an
/// error.
fn apply_conditional_declarations(files: &mut Vec<(String, String)>, app: &App) {
    let mut anchors: crate::det::HashSet<String> = Default::default();
    for lc in &app.library_classes {
        for conditional in &lc.conditional_definitions {
            for name in conditional.declared_names() {
                let anchor = crate::emit::ruby::library_file_anchor(name.0.as_str(), app);
                if anchor != format!("models/{}", crate::naming::underscore(name.0.as_str())) {
                    report_program_error(
                        conditional.span,
                        "conditional_declaration",
                        &format!(
                            "`{}` is declared in a load-time conditional inside a class, whose file it shares; it is not carried",
                            name.0.as_str()
                        ),
                    );
                    continue;
                }
                anchors.insert(anchor);
            }
        }
    }
    if anchors.is_empty() {
        return;
    }
    for (path, content) in files.iter_mut() {
        let Some(rel) = path.strip_prefix("app/").and_then(|p| p.strip_suffix(".rb")) else { continue };
        let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        let kept: Vec<&str> = content
            .lines()
            .filter(|line| {
                let target = line
                    .strip_prefix("require_relative \"")
                    .and_then(|l| l.strip_suffix('"'))
                    .map(|r| normalize_source(&if dir.is_empty() { r.to_string() } else { format!("{dir}/{r}") }));
                !target.is_some_and(|t| anchors.contains(&t))
            })
            .collect();
        if kept.len() != content.lines().count() {
            *content = kept.join("\n") + "\n";
        }
    }
}

/// Each gem-source file's file-level program (`App::file_programs`) as
/// `app/models/roundhouse_files/<source path>.rb`: what Ruby runs when it
/// loads that file, in source order. app/models.rb already loads every
/// declaration file, so a program runs its statements and the requires
/// that order them: a declaration loads its emitted file (a no-op once
/// loaded); a `require` of a file with a program loads that program, of
/// a file of declarations alone nothing more, of a framework feature
/// nothing (the runtime ports answer it), of anything else outside the
/// tree what it names; a `require_relative` of an app file the tree
/// does not carry, and a statement the ingest could not carry, are named
/// on stderr as the file loads, and reported as errors.
///
/// app/models.rb gains `RoundhouseAutoload` and, before the app's own
/// files as in Rails' boot, the programs of the files `Bundler.require`
/// loads (`App::boot_gem_entries`). A program nothing reaches from those is
/// listed in `UNREACHED_PROGRAMS` and not loaded, as Ruby loads no gem
/// file that nothing requires.
fn apply_file_programs(files: &mut Vec<(String, String)>, app: &App) {
    use std::fmt::Write;
    if app.file_programs.is_empty() {
        return;
    }
    let relative = |source: &str| -> String {
        let root = app.root.trim_end_matches('/');
        let rel = if root.is_empty() { source } else { source.strip_prefix(root).unwrap_or(source) };
        rel.trim_start_matches('/').to_string()
    };
    let anchor_of = |source: &str| -> String {
        let rel = relative(source);
        format!("models/roundhouse_files/{}", rel.strip_suffix(".rb").unwrap_or(&rel))
    };
    let programs: crate::det::HashMap<String, &crate::app::FileProgram> =
        app.file_programs.iter().map(|p| (normalize_source(&p.source), p)).collect();
    let known: crate::det::HashSet<String> = app.sources.iter().map(|f| normalize_source(&f.path)).collect();
    let emitted: crate::det::HashSet<String> = files
        .iter()
        .filter_map(|(p, _)| p.strip_prefix("app/").and_then(|p| p.strip_suffix(".rb")).map(str::to_string))
        .collect();
    // `require "x"` from a gem file: the file of that feature under the
    // requiring gem's lib/, then any gem's.
    let resolve = |from: &str, feature: &str, relative: bool| -> Option<String> {
        let file = if feature.ends_with(".rb") { feature.to_string() } else { format!("{feature}.rb") };
        if relative {
            let dir = std::path::Path::new(from).parent()?;
            return Some(normalize_source(&dir.join(&file).display().to_string()));
        }
        let lib_of = |path: &str| path.rfind("/lib/").map(|at| path[..at + 5].to_string());
        let own = lib_of(from).map(|lib| normalize_source(&format!("{lib}{file}")));
        if let Some(own) = own.filter(|p| known.contains(p)) {
            return Some(own);
        }
        let suffix = format!("/lib/{file}");
        let mut found: Vec<&String> = known.iter().filter(|p| p.ends_with(&suffix)).collect();
        found.sort();
        found.first().map(|p| p.to_string())
    };
    let site = |span: crate::span::Span| -> String {
        let Some(source) = (span.file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i)) else {
            return "(unknown)".to_string();
        };
        let (line, _) = source.line_col(span.start);
        format!("{}:{line}", relative(&source.path))
    };
    // The programs an `autoload` names: each, as it starts loading,
    // drops the pending autoloads of what it is about to define, as Ruby
    // does for the file it is loading (`RoundhouseAutoload.entering`).
    // Each autoloaded constant (`Owner::Name`, `Name` at the top level)
    // and the program it loads.
    let mut autoloaded: Vec<(String, String)> = Vec::new();
    for program in &app.file_programs {
        for item in all_program_items(&program.items) {
            let crate::app::ProgramItem::Autoload { owner, name, feature, .. } = item else { continue };
            let Some(target) = resolve(&normalize_source(&program.source), feature, false).filter(|t| programs.contains_key(t)) else {
                continue;
            };
            let constant = if owner == "Object" { name.clone() } else { format!("{owner}::{name}") };
            autoloaded.push((constant, target));
        }
    }
    // Which programs each program's requires load, for reachability.
    let mut edges: crate::det::HashMap<String, Vec<String>> = Default::default();
    for program in &app.file_programs {
        let from = normalize_source(&program.source);
        let mut out = String::new();
        writeln!(
            out,
            "# The file-level program of {}:\n\
             # what Ruby runs when it loads that file, in source order. Loaded\n\
             # where a carried file requires it, else at boot when Bundler\n\
             # requires its gem (app/models.rb).",
            relative(&program.source)
        )
        .unwrap();
        let names: Vec<&str> = autoloaded.iter().filter(|(_, target)| *target == from).map(|(c, _)| c.as_str()).collect();
        // A program an autoload names runs inside `entering`, which keeps
        // Ruby's autoload when the file raises.
        let header = std::mem::take(&mut out);
        let mut loaded: crate::det::HashSet<String> = Default::default();
        let mut steps = Vec::new();
        program_steps(&program.items, 0, &mut steps);
        for (depth, step) in steps {
            let item = match step {
                ProgramStep::Line(line) => {
                    writeln!(out, "{}{line}", "  ".repeat(depth)).unwrap();
                    continue;
                }
                ProgramStep::Item(item) => item,
            };
            // Inside a branch: indented to it, and what it loads is loaded
            // again in another branch.
            let start = out.len();
            let saved = (depth > 0).then(|| loaded.clone());
            match item {
                crate::app::ProgramItem::Branch { .. } => unreachable!("flattened by program_steps"),
                crate::app::ProgramItem::Declared(names) => {
                    for name in names {
                        let anchor = crate::emit::ruby::library_file_anchor(name.0.as_str(), app);
                        if emitted.contains(&anchor) && loaded.insert(anchor.clone()) {
                            writeln!(out, "require RoundhouseAutoload.path({anchor:?})").unwrap();
                        }
                    }
                }
                crate::app::ProgramItem::Statement(expr) => {
                    writeln!(out, "{}", crate::emit::ruby::emit_expr(expr)).unwrap();
                }
                crate::app::ProgramItem::Require { span, feature, relative: rel } => {
                    let keyword = if *rel { "require_relative" } else { "require" };
                    let target = resolve(&from, feature, *rel);
                    match target {
                        Some(target) if target == from => {
                            writeln!(
                                out,
                                "# {}: `{keyword} {feature:?}` names this gem file itself, which is loading: Ruby's circular require, a no-op",
                                site(*span)
                            )
                            .unwrap();
                        }
                        Some(target) if programs.contains_key(&target) => {
                            let anchor = anchor_of(&target);
                            edges.entry(from.clone()).or_default().push(target);
                            if loaded.insert(anchor.clone()) {
                                writeln!(out, "RoundhouseAutoload.require_gem_file({anchor:?})").unwrap();
                            }
                        }
                        Some(target) if known.contains(&target) => {
                            writeln!(out, "# {}: `{keyword} {feature:?}` loads {}, whose declarations app/models.rb loads", site(*span), relative(&target)).unwrap();
                        }
                        _ if *rel => {
                            let message = format!(
                                "roundhouse: error[file_require_not_carried]: {}: `{keyword} {feature:?}` names an app file the tree does not carry; it was not loaded",
                                site(*span)
                            );
                            report_program_error(*span, "file_require_not_carried", &message);
                            writeln!(out, "$stderr.puts {message:?}").unwrap();
                        }
                        _ if is_framework_feature(feature) => {
                            writeln!(out, "# {}: `require {feature:?}` is a Rails feature this tree's runtime ports answer; not required", site(*span)).unwrap();
                        }
                        _ => {
                            writeln!(out, "require {feature:?}").unwrap();
                        }
                    }
                }
                crate::app::ProgramItem::Opened(chains) => {
                    for chain in chains {
                        let open: Vec<String> =
                            chain.iter().map(|(segment, is_class)| format!("{} {segment}", if *is_class { "class" } else { "module" })).collect();
                        writeln!(out, "{}; {}", open.join("; "), vec!["end"; chain.len()].join("; ")).unwrap();
                    }
                }
                // Lazily, as Ruby does: the constant's first read loads
                // the program of the file it names.
                crate::app::ProgramItem::Autoload { span, owner, name, feature } => match resolve(&from, feature, false) {
                    Some(target) if programs.contains_key(&target) => {
                        edges.entry(from.clone()).or_default().push(target.clone());
                        writeln!(out, "{owner}.autoload(:{name}, RoundhouseAutoload.path({:?}))", anchor_of(&target)).unwrap();
                    }
                    _ if is_framework_feature(feature) => {
                        writeln!(out, "# {}: `autoload :{name}, {feature:?}` is a Rails feature this tree's runtime ports answer", site(*span)).unwrap();
                    }
                    _ => {
                        writeln!(out, "{owner}.autoload(:{name}, {feature:?})").unwrap();
                    }
                },
                crate::app::ProgramItem::NotCarried { span, message } => {
                    let text = format!("roundhouse: error[file_statement_unsupported]: {}: {message}", site(*span));
                    report_program_error(*span, "file_statement_unsupported", &text);
                    writeln!(out, "$stderr.puts {text:?}").unwrap();
                }
            }
            if depth > 0 {
                let written: String = out[start..]
                    .lines()
                    .map(|line| if line.is_empty() { "\n".to_string() } else { format!("{}{line}\n", "  ".repeat(depth)) })
                    .collect();
                out.truncate(start);
                out.push_str(&written);
            }
            if let Some(saved) = saved {
                loaded = saved;
            }
        }
        let body = std::mem::take(&mut out);
        out = header;
        if names.is_empty() {
            out.push_str(&body);
        } else {
            writeln!(out, "RoundhouseAutoload.entering({:?}, {names:?}) do", anchor_of(&program.source)).unwrap();
            for line in body.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    writeln!(out, "  {line}").unwrap();
                }
            }
            out.push_str("end\n");
        }
        files.push((format!("app/{}.rb", anchor_of(&program.source)), out));
    }
    // What boot reaches: the gem entries' programs and what they require.
    let mut reached: crate::det::HashSet<String> = Default::default();
    let mut pending: Vec<String> = app.boot_gem_entries.iter().map(|e| normalize_source(e)).filter(|e| programs.contains_key(e)).collect();
    while let Some(next) = pending.pop() {
        if reached.insert(next.clone()) {
            pending.extend(edges.get(&next).into_iter().flatten().cloned());
        }
    }
    let mut unreached: Vec<String> =
        programs.keys().filter(|p| !reached.contains(*p)).map(|p| anchor_of(p)).collect();
    unreached.sort();
    let mut preamble = String::from(
        "# The gem files' programs (app/models/roundhouse_files): each loads\n\
         # where a carried file requires it, else at boot below, when\n\
         # Bundler requires its gem, before the app's files (apply_file_programs).\n\
         module RoundhouseAutoload\n\
         \x20 ROOT = __dir__\n\
         \x20 LOADING = {}\n\
         \x20 # The programs nothing loads at boot: no gem entry Bundler requires\n\
         \x20 # reaches them, so Ruby would not load them either.\n\
         \x20 UNREACHED_PROGRAMS = [\n",
    );
    for anchor in &unreached {
        writeln!(preamble, "    {anchor:?},").unwrap();
    }
    preamble.push_str(
        "  ].freeze\n\n\
         \x20 def self.path(anchor)\n\
         \x20   File.join(ROOT, anchor)\n\
         \x20 end\n\n\
         \x20 # A program an autoload names, starting to load by any route (a\n\
         \x20 # plain require too): the constants whose autoloads point at it\n\
         \x20 # lose them, so defining or reopening them does not run it again,\n\
         \x20 # as Ruby treats the file it is loading.\n\
         \x20 # The program's body is the block. When it raises, each autoload\n\
         \x20 # it dropped comes back, as Ruby keeps a constant's autoload when\n\
         \x20 # its file raises: the next read loads the file again.\n\
         \x20 def self.entering(anchor, names)\n\
         \x20   removed = []\n\
         \x20   names.each do |cpath|\n\
         \x20     *outer, name = cpath.split(\"::\")\n\
         \x20     scope = outer.inject(Object) { |s, n| s&.const_defined?(n, false) ? s.const_get(n, false) : nil }\n\
         \x20     # An autoload whose file is loading: listed, but not defined\n\
         \x20     # (`autoload?` answers nil while its feature loads).\n\
         \x20     next unless scope.is_a?(Module) && scope.constants(false).include?(name.to_sym) && !scope.const_defined?(name, false)\n\
         \x20     scope.send(:remove_const, name)\n\
         \x20     removed << [scope, name]\n\
         \x20   end\n\
         \x20   yield\n\
         \x20 rescue Exception\n\
         \x20   removed.each do |scope, name|\n\
         \x20     scope.send(:remove_const, name) if scope.const_defined?(name, false)\n\
         \x20     scope.autoload(name, path(anchor))\n\
         \x20   end\n\
         \x20   raise\n\
         \x20 end\n\n\
         \x20 # A gem file's require of another. One still loading is Ruby's\n\
         \x20 # circular require: a no-op answering false (and a warning, which\n\
         \x20 # natively points into RubyGems and is not the app's).\n\
         \x20 def self.require_gem_file(anchor)\n\
         \x20   file = path(anchor)\n\
         \x20   return false if LOADING[file]\n\
         \x20   LOADING[file] = true\n\
         \x20   begin\n\
         \x20     require file\n\
         \x20   ensure\n\
         \x20     LOADING.delete(file)\n\
         \x20   end\n\
         \x20 end\n\
         end\n\n",
    );
    // Each gem's lib/ of programs on the load path: `require "globgem"`
    // finds the program its boot already loaded (and answers false), and
    // any `require "globgem/…"` loads that file's program.
    let mut libs: Vec<String> = programs
        .keys()
        .filter_map(|source| {
            let rel = relative(source);
            rel.rfind("/lib/").map(|at| format!("models/roundhouse_files/{}", &rel[..at + 4]))
        })
        .collect();
    libs.sort();
    libs.dedup();
    for lib in &libs {
        writeln!(preamble, "$LOAD_PATH.unshift(RoundhouseAutoload.path({lib:?}))").unwrap();
    }
    if !libs.is_empty() {
        preamble.push('\n');
    }
    let mut boot = String::new();
    for entry in &app.boot_gem_entries {
        let entry = normalize_source(entry);
        if programs.contains_key(&entry) {
            writeln!(boot, "RoundhouseAutoload.require_gem_file({:?})", anchor_of(&entry)).unwrap();
        }
    }
    // A declaration file's load-time require of a constant under an
    // autoloaded namespace loads that namespace's program, as reading the
    // constant does natively (`Track::None` from `Options` runs track.rb,
    // which loads base, none and status in its order), not the constant's
    // own declarations, whose `module Track` line would run the autoload
    // from inside a file it loads.
    if !autoloaded.is_empty() {
        let mut anchors: crate::det::HashMap<String, Vec<&str>> = Default::default();
        for lc in &app.library_classes {
            anchors.entry(crate::emit::ruby::library_file_anchor(lc.name.0.as_str(), app)).or_default().push(lc.name.0.as_str());
        }
        let through = |anchor: &str| -> Option<String> {
            let names = anchors.get(anchor)?;
            names
                .iter()
                .filter_map(|name| {
                    autoloaded
                        .iter()
                        .filter(|(constant, _)| *name == constant || name.starts_with(&format!("{constant}::")))
                        .max_by_key(|(constant, _)| constant.len())
                })
                .max_by_key(|(constant, _)| constant.len())
                .map(|(_, target)| anchor_of(target))
        };
        for (path, content) in files.iter_mut() {
            let Some(rel) = path.strip_prefix("app/").and_then(|p| p.strip_suffix(".rb")) else { continue };
            if !rel.starts_with("models/") || rel.starts_with("models/roundhouse_files/") {
                continue;
            }
            let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let mut changed = false;
            let lines: Vec<String> = content
                .lines()
                .map(|line| {
                    let target = line
                        .strip_prefix("require_relative \"")
                        .and_then(|l| l.strip_suffix('"'))
                        .map(|r| normalize_source(&format!("{dir}/{r}")));
                    match target.as_deref().and_then(through) {
                        Some(program) => {
                            changed = true;
                            format!("RoundhouseAutoload.require_gem_file({program:?})")
                        }
                        None => line.to_string(),
                    }
                })
                .collect();
            if changed {
                *content = lines.join("\n") + "\n";
            }
        }
    }
    // A file of the gem's declarations alone loads where the gem's own
    // requires and autoloads reach it, as natively, not with the app's
    // files: a gem file nothing requires defines nothing.
    let gem_names: crate::det::HashSet<&str> = app
        .file_programs
        .iter()
        .flat_map(|p| all_program_items(&p.items))
        .filter_map(|item| match item {
            crate::app::ProgramItem::Declared(names) => Some(names.iter().map(|n| n.0.as_str())),
            _ => None,
        })
        .flatten()
        .collect();
    // Only for a gem whose programs the boot reaches: one nothing loads
    // at boot keeps its declarations among the app's files, as before.
    let booted_libs: Vec<String> = reached
        .iter()
        .filter_map(|source| source.rfind("/lib/").map(|at| source[..at + 5].to_string()))
        .collect();
    let gem_names: crate::det::HashSet<&str> = app
        .file_programs
        .iter()
        .filter(|p| booted_libs.iter().any(|lib| normalize_source(&p.source).starts_with(lib.as_str())))
        .flat_map(|p| all_program_items(&p.items))
        .filter_map(|item| match item {
            crate::app::ProgramItem::Declared(names) => Some(names.iter().map(|n| n.0.as_str())),
            _ => None,
        })
        .flatten()
        .filter(|name| gem_names.contains(name))
        .collect();
    let mut by_anchor: crate::det::HashMap<String, bool> = Default::default();
    for lc in &app.library_classes {
        let gem = gem_names.contains(lc.name.0.as_str());
        let entry = by_anchor.entry(crate::emit::ruby::library_file_anchor(lc.name.0.as_str(), app)).or_insert(true);
        *entry &= gem;
    }
    if let Some((_, models)) = files.iter_mut().find(|(p, _)| p == "app/models.rb") {
        let kept: Vec<&str> = models
            .lines()
            .filter(|line| {
                let anchor = line.strip_prefix("require_relative \"").and_then(|l| l.strip_suffix('"'));
                !anchor.is_some_and(|a| by_anchor.get(a).copied().unwrap_or(false))
            })
            .collect();
        *models = kept.join("\n") + "\n";
        // Bundler.require runs in config/application.rb, before any app
        // file loads: the gems' entry programs come first.
        let heading = if boot.is_empty() { "" } else { "# Bundler.require: the gems' entry files.\n" };
        *models = format!("{preamble}{heading}{boot}{}{models}", if boot.is_empty() { "" } else { "\n" });
    }
}

/// A class-body load-time conditional (`dialect::ConditionalDefinitions`)
/// is written as the source has it only by the Ruby family; another
/// target would define every branch's methods, the last one winning, so
/// each such statement is an error there.
fn refuse_conditional_definitions(app: &App, target: BuildTarget) {
    for lc in &app.library_classes {
        for conditional in &lc.conditional_definitions {
            let diagnostic = crate::diagnostic::Diagnostic::unsupported(
                conditional.span,
                Some(crate::ident::Symbol::from(target.as_str())),
                "conditional_definition",
                format!(
                    "`{}` defines methods in a load-time `if`/`unless`, which only the Ruby and JRuby targets carry",
                    lc.name.0.as_str()
                ),
            );
            emit::diagnostics::push(diagnostic);
        }
    }
}

/// One line or item of a program as written, branches flattened
/// (`program_steps`).
enum ProgramStep<'a> {
    Line(String),
    Item(&'a crate::app::ProgramItem),
}

/// A program's items with each branch as its `if` / `else` / `end` lines
/// around its items, one level deeper, as `(depth, step)`.
fn program_steps<'a>(items: &'a [crate::app::ProgramItem], depth: usize, out: &mut Vec<(usize, ProgramStep<'a>)>) {
    for item in items {
        match item {
            crate::app::ProgramItem::Branch { condition, negated, then_items, else_items } => {
                let keyword = if *negated { "unless" } else { "if" };
                out.push((depth, ProgramStep::Line(format!("{keyword} {}", crate::emit::ruby::emit_expr(condition)))));
                program_steps(then_items, depth + 1, out);
                if !else_items.is_empty() {
                    out.push((depth, ProgramStep::Line("else".to_string())));
                    program_steps(else_items, depth + 1, out);
                }
                out.push((depth, ProgramStep::Line("end".to_string())));
            }
            other => out.push((depth, ProgramStep::Item(other))),
        }
    }
}

/// Every item of a program, those in its branches too.
fn all_program_items(items: &[crate::app::ProgramItem]) -> Vec<&crate::app::ProgramItem> {
    let mut out = Vec::new();
    for item in items {
        if let crate::app::ProgramItem::Branch { then_items, else_items, .. } = item {
            out.extend(all_program_items(then_items));
            out.extend(all_program_items(else_items));
        } else {
            out.push(item);
        }
    }
    out
}

/// A path with `.` and `..` segments folded, as a require resolves it.
fn normalize_source(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." if out.last().is_some_and(|s| !s.is_empty() && *s != "..") => {
                out.pop();
            }
            _ => out.push(segment),
        }
    }
    out.join("/")
}

/// A feature of the Rails frameworks (`active_support/core_ext/…`,
/// `rails`): the runtime ports are the framework in an emitted tree, and
/// no such file is on its load path.
fn is_framework_feature(feature: &str) -> bool {
    let root = feature.split('/').next().unwrap_or(feature);
    matches!(
        root,
        "rails" | "railties" | "active_support" | "activesupport" | "active_record" | "activerecord" | "arel"
            | "active_model" | "activemodel" | "active_job" | "activejob" | "action_pack" | "actionpack"
            | "action_controller" | "action_dispatch" | "abstract_controller" | "action_view" | "actionview"
            | "action_mailer" | "actionmailer" | "action_cable" | "actioncable" | "action_text" | "actiontext"
            | "active_storage" | "activestorage" | "action_mailbox" | "actionmailbox"
    )
}

/// An error diagnostic for a program item the tree does not carry.
fn report_program_error(span: crate::span::Span, construct: &str, message: &str) {
    let diagnostic = crate::diagnostic::Diagnostic::unsupported(
        span,
        Some(crate::ident::Symbol::from("ruby")),
        construct,
        message.to_string(),
    );
    emit::diagnostics::push(diagnostic);
}

/// The Ruby tree of an app whose primary database is PostgreSQL
/// (`Dialect::for_app`); the SQL in it was lowered for PostgreSQL by the
/// same emit. Its Db is runtime/spinel/db_pg.rb over the pg gem, its
/// driver differences pg_driver_cruby.rb, and its database the server
/// `DATABASE_URL` names (with libpq's `PG*` variables as fallbacks), at
/// boot and in the emitted tests. Each test file runs in a schema of its
/// own, created when it starts and dropped when it exits.
fn apply_postgresql_tree(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    if !app.sql_functions.is_empty() {
        // An error diagnostic as well as the refusal, as
        // `extension_gem_refusals` reports one, so `check` names it.
        let names = app.sql_functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
        let message = format!(
            "the primary database is postgresql, and the app registers SQLite functions ({names}), which PostgreSQL \
             has no equivalent of; not substituted"
        );
        let diagnostic = crate::diagnostic::Diagnostic::unsupported(
            crate::span::Span::synthetic(),
            Some(crate::ident::Symbol::from("ruby")),
            "sqlite_functions",
            message,
        );
        let refusal = diagnostic.message.clone();
        emit::diagnostics::push(diagnostic);
        return Err(refusal);
    }
    let take = |files: &mut Vec<(String, String)>, path: &str| -> Result<String, String> {
        let at = files.iter().position(|(p, _)| p == path).ok_or_else(|| format!("PostgreSQL tree: {path} not found"))?;
        Ok(files.remove(at).1)
    };
    let db_pg = take(files, "runtime/db_pg.rb")?;
    let driver = take(files, "runtime/pg_driver_cruby.rb")?;
    files.retain(|(p, _)| !matches!(p.as_str(), "runtime/db.rb" | "runtime/pg_driver.rb" | "Gemfile.lock"));
    files.push(("runtime/db.rb".to_string(), db_pg));
    files.push(("runtime/pg_driver.rb".to_string(), driver));

    // The gem, and a fresh lock: the committed one is SQLite's.
    let gemfile = files.iter_mut().find(|(p, _)| p == "Gemfile").ok_or("PostgreSQL tree: Gemfile not found")?;
    if !gemfile.1.contains("gem \"sqlite3\"") {
        return Err("PostgreSQL tree: expected `gem \"sqlite3\"` in the scaffold Gemfile".to_string());
    }
    gemfile.1 = gemfile.1.replace("gem \"sqlite3\"", "gem \"pg\"");

    const SERVER_SQLITE: &str = "    db_path = ENV[\"BLOG_DB\"]\n    path = (!db_path.nil? && !db_path.empty?) ? db_path : \"storage/development.sqlite3\"\n";
    const SERVER_POSTGRESQL: &str = "    # PostgreSQL: the server DATABASE_URL names, else libpq's PG*\n    # variables (apply_postgresql_tree).\n    path = ENV.fetch(\"DATABASE_URL\", \"\")\n";
    let main = files.iter_mut().find(|(p, _)| p == "main.rb").ok_or("PostgreSQL tree: main.rb not found")?;
    if !main.1.contains(SERVER_SQLITE) {
        return Err("PostgreSQL tree: main.rb's database path no longer matches".to_string());
    }
    main.1 = main.1.replace(SERVER_SQLITE, SERVER_POSTGRESQL);

    const TEST_SQLITE: &str = "Db.configure(_test_db_path)\n";
    const TEST_POSTGRESQL: &str = "# PostgreSQL (apply_postgresql_tree): the server DATABASE_URL names,\n\
# else libpq's PG* variables, in a schema of this test process's own,\n\
# created here and dropped when its tests are done: after Minitest's\n\
# run for a file that autoruns it (whose at_exit would otherwise come\n\
# after this one), else at exit. search_path rides PGOPTIONS, so\n\
# every connection of the pool resolves the tables to it. The pid comes\n\
# first, so cutting a long file name to PostgreSQL's 63 bytes keeps it.\n\
_test_schema = (\"rh_test_\" + Process.pid.to_s + \"_\" + File.basename($0, \".rb\").gsub(/[^a-z0-9_]/, \"_\"))[0, 63]\n\
ENV[\"PGOPTIONS\"] = \"-c search_path=\" + _test_schema\n\
Db.configure(ENV.fetch(\"DATABASE_URL\", \"\"))\n\
Db.exec(\"DROP SCHEMA IF EXISTS \" + _test_schema + \" CASCADE\")\n\
Db.exec(\"CREATE SCHEMA \" + _test_schema)\n\
_drop_test_schema = -> { Db.exec(\"DROP SCHEMA IF EXISTS \" + _test_schema + \" CASCADE\") }\n\
if defined?(Minitest) && Minitest.respond_to?(:after_run)\n  \
Minitest.after_run(&_drop_test_schema)\n\
else\n  \
at_exit(&_drop_test_schema)\n\
end\n";
    let helper = files.iter_mut().find(|(p, _)| p == "test/test_helper.rb").ok_or("PostgreSQL tree: test/test_helper.rb not found")?;
    if !helper.1.contains(TEST_SQLITE) {
        return Err("PostgreSQL tree: test_helper.rb's database setup no longer matches".to_string());
    }
    helper.1 = helper.1.replace(TEST_SQLITE, TEST_POSTGRESQL);
    Ok(())
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

/// `runtime/i18n_locale.rb`'s generated block: the app's
/// `config.i18n.default_locale` and `available_locales` (read at ingest,
/// `App::i18n`), as the literals `I18n.default_locale` and
/// `I18n.available_locales` answer and `I18n.locale =` checks against.
/// An app that sets neither keeps the file's `:en`.
/// `app.i18n_table` as `I18n.load_translation_table` reads it: each
/// distinct entry once (`E`), and per site, locale, environment and key
/// the index of its entry (`T`), so no entry is spelled per locale.
fn i18n_table_text(app: &App) -> String {
    let mut index: crate::det::HashMap<&Vec<String>, usize> = crate::det::HashMap::default();
    let mut entries = String::new();
    let mut lines = String::new();
    let table: Vec<(String, Vec<String>)> = app.i18n_table.iter().flat_map(|site| site.records()).collect();
    for (record, entry) in &table {
        let next = index.len();
        let at = *index.entry(entry).or_insert_with(|| {
            entries.push('E');
            for field in entry {
                entries.push('\u{1f}');
                entries.push_str(field);
            }
            entries.push_str("\u{1f}.\u{1e}");
            next
        });
        // Not packed per site and locale: unpacking a run of keys is a loop inside the line loop, which a functional target cannot write
        lines.push_str(&format!("T\u{1f}{}\u{1f}{at}\u{1f}.\u{1e}", record.splitn(4, '\u{1c}').collect::<Vec<_>>().join("\u{1f}")));
    }
    let mut out = entries;
    out.push_str(&lines);
    // The whole catalog and each locale's fallback chain, for `I18n.translate_runtime`: only when a call is left to it.
    if !app.i18n_runtime_sites.is_empty() {
        let catalog = app.i18n.or_rails_default();
        let mut locales = catalog.locales();
        for other in &catalog.others {
            if !locales.contains(&other.locale) {
                locales.push(other.locale.clone());
            }
        }
        for locale in &locales {
            let Some(c) = catalog.catalog_for(locale) else { continue };
            // In the files' order, not the catalog's sorted one: `translate_tree` builds its Hash from them as they come.
            for (key, text) in c.ordered_nodes() {
                match text {
                    Some(text) => out.push_str(&format!("L\u{1f}{locale}.{key}\u{1f}{text}\u{1f}.\u{1e}")),
                    None => out.push_str(&format!("M\u{1f}{locale}.{key}\u{1f}.\u{1e}")),
                }
            }
            for key in &c.opaque {
                let fields = c.values.get(key).map_or_else(|| vec!["?".to_string()], |v| v.fields());
                out.push_str(&format!("O\u{1f}{locale}.{key}\u{1f}{}\u{1f}.\u{1e}", fields.join("\u{1f}")));
            }
        }
        for variant in catalog.variants(&locales) {
            let chain = match &variant.fallbacks {
                Some(defaults) => crate::i18n::fallback_chain(&variant.locale, defaults),
                None => vec![variant.locale.clone()],
            };
            for env in variant.envs.clone().unwrap_or_else(|| vec![String::new()]) {
                out.push_str(&format!("F\u{1f}{}\u{1f}{env}\u{1f}{}\u{1f}.\u{1e}", variant.locale, chain.join(",")));
            }
        }
        for (locale, rule) in catalog.plurals.iter().filter(|(_, rule)| !rule.is_empty()) {
            out.push_str(&format!("R\u{1f}{locale}\u{1f}{rule}\u{1f}.\u{1e}"));
        }
    }
    out
}

fn apply_i18n_locales(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "  # >>> generated: i18n-locales\n";
    const TAIL: &str = "  # <<< generated: i18n-locales\n";
    let catalog = app.i18n.or_rails_default();
    let sym = |l: &str| match l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        true => format!(":{l}"),
        false => format!(":{}", crate::emit::ruby::expr::ruby_str_literal(l)),
    };
    let available = catalog.locales().iter().map(|l| sym(l)).collect::<Vec<_>>().join(", ");
    // `.freeze`: a literal Array of literals frozen in place is one shared
    // object on CRuby 3.4+, and `I18n.locale` searches this list on every
    // call (once per element of a cached collection's keys).
    let generated = format!(
        "{HEAD}  def self.app_default_locale\n    {}\n  end\n\n  def self.app_available_locales\n    [{available}].freeze\n  end\n{TAIL}",
        sym(&catalog.locale)
    );
    for (path, content) in files.iter_mut() {
        if !path.ends_with("runtime/i18n_locale.rb") {
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

/// `ActionText::ContentHelper.app_allowed_tags` — editor tags the app
/// unions into ActionText's sanitizer at boot (campfire's
/// `EDITOR_FORMATTING_TAGS`). Same generated-hook shape as attributes.
fn apply_content_helper_tags(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "    # >>> generated: content-helper-tags\n";
    const TAIL: &str = "    # <<< generated: content-helper-tags\n";
    if app.content_helper_allowed_tags.is_empty() {
        return;
    }
    let list = app
        .content_helper_allowed_tags
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!("{HEAD}    def self.app_allowed_tags\n      [{list}]\n    end\n{TAIL}");
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
    // The partials named here are called by the method they lowered to,
    // which a template of the same name in their directory moves
    // (`PartialStems`); the files keep their names.
    let stems = crate::lower::view_to_library::PartialStems::of(&app.views);
    let content = stems.method("layouts/action_text/contents", "content");
    // What the layout YIELDS is Action Text's own partial,
    // `action_text/contents/_content.html.erb`, rendered: `<%=
    // render_action_text_content(content) %>` and the newline that
    // ends that file. The app's layout then writes `<%= yield -%>`,
    // whose `-` drops ITS newline, so the one before `</div>` is the
    // partial's — `…boxes.\n</div>` on campfire's room page, where the
    // emit wrote `…boxes.</div>` without it.
    let layout = format!(
        "{HEAD}    def rendered_html\n      \
         Views::Layouts::ActionText::Contents.{content}(render_action_text_content + \"\\n\")\n    end\n{TAIL}"
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
        let method = stems.method(dir, stem);
        let name = binding.class.0.as_str();
        let local = &binding.local;
        if binding.content_type.is_some() {
            by_content_type.push_str(&format!(
                "      {local} = ::{name}.from_node(attachment)\n      \
                 unless {local}.nil?\n        {local}.attachment = attachment\n        \
                 return {module}.{method}({local})\n      end\n",
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
                 {local}.nil? ? \"\" : {module}.{method}({local})\n",
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
        let blob = stems.method("active_storage/blobs", "blob");
        by_model.push_str(&format!(
            "      when \"ActiveStorage::Blob\"\n        \
             attachment.blob.nil? ? \"\" : Views::ActiveStorage::Blobs.{blob}(attachment)\n",
        ));
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

/// `Channel::Base`'s own public instance methods (actioncable 8.1:
/// `Base.public_instance_methods(true) - Object.public_instance_methods`).
/// Rails takes them out of `action_methods`, so a client cannot
/// `perform("stream_from")`; an app ancestor's method of the same name
/// is taken out with them.
const CABLE_BASE_PUBLIC_METHODS: &[&str] = &[
    "__callbacks", "_run_subscribe_callbacks", "_run_subscribe_callbacks!",
    "_run_unsubscribe_callbacks", "_run_unsubscribe_callbacks!", "_subscribe_callbacks",
    "_unsubscribe_callbacks", "broadcast_to", "broadcasting_for", "channel_name",
    "connection", "handler_for_rescue", "identifier", "logger", "params", "perform_action",
    "periodic_timers=", "pubsub", "rescue_handlers", "rescue_handlers=", "rescue_handlers?",
    "rescue_with_handler", "run_callbacks", "stop_all_streams", "stop_stream_for",
    "stop_stream_from", "stream_for", "stream_from", "stream_or_reject_for",
    "subscribe_to_channel", "unsubscribe_from_channel", "unsubscribed?",
];

/// Methods this pipeline writes INTO every channel class
/// (`ingest::channel_callbacks`): the `on_subscribe`/`on_unsubscribe`
/// chains and the literal `channel_name`. In Rails they are class-level
/// machinery, never instance methods a client could name, so they are
/// never actions - even though, here, the channel itself defines them.
const CABLE_SYNTHESIZED_METHODS: &[&str] = &["after_subscribe", "after_unsubscribe", "channel_name"];

/// How one action is called, decided from its formals the way Rails'
/// `dispatch_action` decides it from `method(action).arity`.
enum CableActionCall {
    /// Arity exactly 1: one required positional, nothing else.
    WithData,
    /// Callable with no arguments (every formal optional, or none).
    Bare,
    /// Neither: Rails calls it with no arguments and Ruby raises.
    Raises(String),
}

fn cable_action_call(method: &crate::dialect::MethodDef) -> CableActionCall {
    let params = &method.params;
    let keywordish = |p: &crate::dialect::Param| p.keyword || p.from_keyword || p.from_kwrest;
    let required_positional = params
        .iter()
        .filter(|p| !keywordish(p) && !p.rest && !p.forwarding && p.default.is_none())
        .count();
    let required_keywords: Vec<&str> = params
        .iter()
        .filter(|p| p.keyword && p.default.is_none() && !p.rest)
        .map(|p| p.name.as_str())
        .collect();
    if params.len() == 1 && required_positional == 1 {
        return CableActionCall::WithData;
    }
    if required_positional > 0 {
        return CableActionCall::Raises(format!(
            "wrong number of arguments (given 0, expected {required_positional})"
        ));
    }
    if !required_keywords.is_empty() {
        let names: Vec<String> = required_keywords.iter().map(|k| format!(":{k}")).collect();
        let noun = if names.len() == 1 { "keyword" } else { "keywords" };
        return CableActionCall::Raises(format!("missing {noun}: {}", names.join(", ")));
    }
    CableActionCall::Bare
}

/// Write `ActionCable::Channel.perform(channel, action, data)` - the
/// dispatch for the client's `subscription.perform(action, data)` (#71
/// item 6) - into the Spinel lane's `runtime/action_cable.rb`.
///
/// Rails finds the method by reflection: `action_methods` is the
/// channel's public instance methods with its ancestors', less
/// `Channel::Base`'s, plus those the class defines itself; then
/// `public_send`. A strict target has neither, so the same set is
/// worked out here, per channel, from the ingested classes:
///
/// - the channel's own public methods, then its mixins and its app
///   ancestors' with theirs, in Ruby's lookup order - modules a
///   `config/initializers/` file prepends or includes (`app.module_mixins`)
///   take their place in it - and the first definition of a name decides,
///   so a subclass that makes an inherited method private takes it out;
/// - a method the channel class itself defines publicly stays an action
///   even when a prepended module makes it private, as in Rails; its arm
///   raises the NoMethodError `public_send` raises there;
/// - an inherited name `Channel::Base` also answers is not an action
///   (`CABLE_BASE_PUBLIC_METHODS`); the methods this pipeline writes
///   into channels never are (`CABLE_SYNTHESIZED_METHODS`).
///
/// ONE ARM PER CHANNEL CLASS, most derived first: the arm is an
/// `is_a?`, and a parent's arm would otherwise answer for its
/// subclasses with the parent's set. Each arm repeats the inherited
/// actions, so no arm needs another's.
///
/// The CRuby overlay does not read this: it has reflection, and runs
/// Rails' own algorithm in its `Channel::Base#perform_action`.
fn apply_cable_actions(files: &mut [(String, String)], app: &App) {
    use crate::app::MixinKind;
    use crate::dialect::{MethodReceiver, MethodVisibility};
    const HEAD: &str = "    # >>> generated: cable-actions\n";
    const TAIL: &str = "    # <<< generated: cable-actions\n";
    const ROOT: &str = "ActionCable::Channel::Base";

    let class_named = |name: &str| app.library_classes.iter().find(|lc| lc.name.0.as_str() == name);
    // `X.include M` / `X.prepend M` from config/initializers, in registration order.
    let initializer_mixins = |target: &str, kind: MixinKind| -> Vec<&crate::dialect::LibraryClass> {
        app.module_mixins
            .iter()
            .filter(|m| m.kind == kind && m.target.as_str().trim_start_matches("::") == target)
            .filter_map(|m| class_named(m.module.as_str().trim_start_matches("::")))
            .collect()
    };

    // (depth, name) for every class descending from Base: the same
    // transitive descent `apply_cable_channels` does, keeping the depth.
    let mut channels: Vec<(usize, &str)> = Vec::new();
    let mut known: Vec<(&str, usize)> = vec![(ROOT, 0)];
    loop {
        let before = known.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str();
            if lc.is_module || known.iter().any(|(k, _)| *k == name) {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            if let Some(&(_, depth)) = known.iter().find(|(k, _)| *k == parent.0.as_str()) {
                known.push((name, depth + 1));
                channels.push((depth + 1, name));
            }
        }
        if known.len() == before {
            break;
        }
    }
    channels.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));

    let mut generated = String::from(HEAD);
    generated.push_str("    def self.perform(channel, action, data)\n");
    for &(_, channel) in &channels {
        // Ruby's lookup order, up to Base: for each class, the modules an
        // initializer prepends onto it (newest first), the class, the
        // modules an initializer includes into it (newest first, so ahead of
        // the class body's), its own `include`s (last first); then the parent.
        let mut order: Vec<(&crate::dialect::LibraryClass, bool)> = Vec::new();
        let mut current = class_named(channel);
        let mut own = true;
        while let Some(lc) = current {
            if lc.name.0.as_str() == ROOT {
                break;
            }
            let class = lc.name.0.as_str();
            for module in initializer_mixins(class, MixinKind::Prepend).into_iter().rev() {
                order.push((module, false));
            }
            order.push((lc, own));
            for module in initializer_mixins(class, MixinKind::Include).into_iter().rev() {
                order.push((module, false));
            }
            for include in lc.includes.iter().rev() {
                if let Some(module) = class_named(include.0.as_str()) {
                    order.push((module, false));
                }
            }
            own = false;
            current = lc.parent.as_ref().and_then(|p| class_named(p.0.as_str()));
        }

        // The definition each name resolves to (the first in that order),
        // and the names the channel class itself defines publicly: Rails
        // adds those to `action_methods` even when a prepended module
        // shadows them, and `public_send` then meets the shadowing method.
        let mut effective: Vec<(&str, &crate::dialect::MethodDef, bool)> = Vec::new();
        let mut own_public: Vec<&str> = Vec::new();
        for (lc, own) in order {
            for method in &lc.methods {
                let name = method.name.as_str();
                if method.receiver != MethodReceiver::Instance {
                    continue;
                }
                if own && method.visibility == MethodVisibility::Public {
                    own_public.push(name);
                }
                if !effective.iter().any(|(n, _, _)| *n == name) {
                    effective.push((name, method, own));
                }
            }
        }

        let mut arms = String::new();
        for (name, method, own) in effective {
            if CABLE_SYNTHESIZED_METHODS.contains(&name) {
                continue;
            }
            let call = if method.visibility == MethodVisibility::Public {
                let base_name = !own && !own_public.contains(&name) && CABLE_BASE_PUBLIC_METHODS.contains(&name);
                if method.unsupported_formals.is_some() || base_name {
                    continue;
                }
                match cable_action_call(method) {
                    CableActionCall::WithData => format!("channel.{name}(data)"),
                    CableActionCall::Bare => format!("channel.{name}"),
                    // The message is built from a count and identifiers only,
                    // so it needs no escaping inside the double quotes.
                    CableActionCall::Raises(message) => format!("raise ArgumentError, \"{message}\""),
                }
            } else if own_public.contains(&name) {
                // Rails: an action (the class defines it publicly) whose call
                // `public_send` refuses, since a prepended module made it
                // private or protected.
                let kind = if method.visibility == MethodVisibility::Private { "private" } else { "protected" };
                format!("raise NoMethodError, \"{kind} method '{name}' called for an instance of {channel}\"")
            } else {
                continue;
            };
            arms.push_str(&format!(
                "        if action == \"{name}\"\n\
                 \x20         {call}\n\
                 \x20         return true\n\
                 \x20       end\n",
            ));
        }
        // An arm even with no actions in it: a channel that made every
        // inherited action private must not fall through to its parent's arm.
        generated.push_str(&format!("      if channel.is_a?({channel})\n{arms}        return false\n      end\n"));
    }
    generated.push_str("      false\n    end\n");
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
///
/// NO-OP when nothing in the tree calls it (every app without a
/// snapshot rebuild): the default body raises. Spinel compiles the
/// method whether or not it is called, and with no caller to type
/// `attributes` its arms handed every model's `instantiate` an untyped
/// row, widening each `<Model>Row.from_raw` to the boxed path.
fn apply_instantiate_named(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "      # >>> generated: instantiate-named\n";
    const TAIL: &str = "      # <<< generated: instantiate-named\n";
    // The runtime file that defines the method, by its path in the
    // emitted tree. A basename match would also skip (and splice into)
    // an app file of the same name, `app/models/active_record_db_config.rb`.
    const DEFINER: &str = "runtime/active_record_db_config.rb";
    let called = files
        .iter()
        .any(|(path, content)| path != DEFINER && content.contains("instantiate_named"));
    if !called {
        return;
    }
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
        if path != DEFINER {
            continue;
        }
        let Some(start) = content.find(HEAD) else { continue };
        let Some(rel_end) = content[start..].find(TAIL) else { continue };
        let end = start + rel_end + TAIL.len();
        content.replace_range(start..end, &generated);
    }
}

/// `config/database.yml`'s `pragmas:` (`App::database_pragmas`, see
/// `ingest::database_yml`) into each SQLite Db shim's connection open,
/// after the shim's own pins: one arm per environment that names any,
/// keyed on the parked `Rails.env_name` (unset reads as development,
/// as `Rails.env` does). A span replace between markers; an app with
/// no pragmas leaves the region empty. Each shim has its own way to
/// run a statement on the handle it just opened, so the call is picked
/// by which shim the file is.
fn apply_database_pragmas(files: &mut [(String, String)], app: &App) {
    const MARK: &str = "# >>> generated: database-pragmas\n";
    const TAIL_MARK: &str = "# <<< generated: database-pragmas\n";
    if app.database_pragmas.is_empty() {
        return;
    }
    for (path, content) in files.iter_mut() {
        if !path.starts_with("runtime/db") || !path.ends_with(".rb") {
            continue;
        }
        let Some(mark) = content.find(MARK) else { continue };
        let line_start = content[..mark].rfind('\n').map_or(0, |i| i + 1);
        let indent = content[line_start..mark].to_string();
        let Some(rel_end) = content[mark..].find(TAIL_MARK) else { continue };
        let end = mark + rel_end + TAIL_MARK.len();
        let exec = |stmt: &str| {
            let lit = crate::emit::ruby::expr::ruby_str_literal(stmt);
            if content.contains("SQL.sqlite3_exec(dbh") {
                format!("SQL.sqlite3_exec(dbh, {lit}, nil, nil, nil)")
            } else if content.contains("st = raw.create_statement") {
                format!("st.execute({lit})")
            } else {
                format!("db.execute({lit})")
            }
        };
        let mut generated = format!("{MARK}{indent}case Rails.env_name.to_s\n");
        for (env, statements) in &app.database_pragmas {
            let names = if env == "development" {
                format!("{}, \"\"", crate::emit::ruby::expr::ruby_str_literal(env))
            } else {
                crate::emit::ruby::expr::ruby_str_literal(env)
            };
            generated.push_str(&format!("{indent}when {names}\n"));
            for stmt in statements {
                generated.push_str(&format!("{indent}  {}\n", exec(stmt)));
            }
        }
        generated.push_str(&format!("{indent}end\n{indent}{TAIL_MARK}"));
        content.replace_range(mark..end, &generated);
    }
}

/// Write `JobRegistry` — each payload job's writer and reader, and the
/// record locators they use — into `runtime/job_registry.rb`, from
/// `App::job_plans`. A span replace between markers, like
/// `apply_global_id_locate`; an app with no payload jobs keeps a
/// registry whose `perform` answers false.
/// Whether a tree pushes jobs to Redis: the app is on `queue_adapter
/// :sidekiq` and has at least one payload job (`lower::job_payload`). An
/// app on `:sidekiq` whose jobs all keep their Proc, or that has no
/// ActiveJob jobs at all (Mastodon's are native Sidekiq workers), gets
/// no Redis client and no dependency.
pub(crate) fn needs_redis(app: &App) -> bool {
    app.queue_adapter.as_deref() == Some("sidekiq") && app.job_plans.iter().any(|p| p.is_payload())
}

/// The `JobRegistry.sidekiq?` body `apply_job_registry` writes for a tree
/// that [`needs_redis`]. `spin_shape` keys the Redis package on it, since
/// it sees files, not the `App`.
const SIDEKIQ_REGISTRY_FLAG: &str = "  def self.sidekiq?\n    true\n  end\n";

/// Broadcasts' transport for the Ruby lanes' tree on the Redis cable:
/// `Cable::Registry` takes a String or a Hash and encodes it at delivery,
/// so this encodes it the same way before it is published.
const REDIS_CABLE_RUBY_TRANSPORT: &str = "
module RedisCable
  class Transport
    def broadcast(stream, message)
      RedisCable.publish(stream, JsonBuilder.escape_html_entities(JSON.generate(message)))
    end
  end
end
";

/// The real Redis connection for a tree on `:sidekiq`, swapped in for the
/// raising façade at `runtime/sidekiq_redis.rb` on every Ruby-family lane.
const SIDEKIQ_REDIS_CLIENT: &str = "runtime/spinel/sidekiq/sidekiq_redis.rb";

/// The worker role for a tree on `:sidekiq`, swapped in for the façade at
/// `runtime/sidekiq_worker.rb` on every Ruby-family lane, as the client
/// above is.
const SIDEKIQ_WORKER: &str = "runtime/spinel/sidekiq/sidekiq_worker.rb";

/// The body of `JobRegistry.sidekiq_redis_url` or `_password` for one
/// `configure_client` setting: `""` when the app gives none, so the push
/// falls back to the environment. `SidekiqPush.setting` reads a variable
/// when the push first needs it, as Sidekiq reads it at boot.
fn redis_setting_source(setting: Option<&crate::app::RedisSetting>) -> String {
    use crate::app::RedisSetting;
    use crate::emit::ruby::expr::ruby_str_literal;
    match setting {
        None => "\"\"".to_string(),
        Some(RedisSetting::Literal(value)) => ruby_str_literal(value),
        Some(RedisSetting::Env(name)) => format!("SidekiqPush.setting({})", ruby_str_literal(name)),
        Some(RedisSetting::EnvFetch { name, default: None }) => {
            format!("SidekiqPush.required_setting({})", ruby_str_literal(name))
        }
        Some(RedisSetting::EnvFetch { name, default: Some(default) }) => {
            format!("SidekiqPush.setting_or({}, {})", ruby_str_literal(name), ruby_str_literal(default))
        }
    }
}

/// `JobRegistry`'s `config/sidekiq.yml` defaults for the worker:
/// concurrency 0 and timeout -1 when the file gives none, and the queues
/// as one String of `name` or `name,weight` words, `""` for none. The
/// retry limit, the dead set's trims and the poller's intervals are -1
/// (-1.0) when the file gives none, so the worker keeps Sidekiq's.
fn worker_defaults_source(defaults: &crate::app::SidekiqWorkerDefaults) -> String {
    use crate::emit::ruby::expr::ruby_str_literal;
    let queues: Vec<String> = defaults
        .queues
        .iter()
        .map(|(name, weight)| if *weight == 0 { name.clone() } else { format!("{name},{weight}") })
        .collect();
    let float = |v: Option<f64>| {
        let v = v.unwrap_or(-1.0);
        if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") }
    };
    let method = |name: &str, body: String| format!("\n  def self.{name}\n    {body}\n  end\n");
    [
        method("sidekiq_worker_concurrency", defaults.concurrency.unwrap_or(0).to_string()),
        method("sidekiq_worker_timeout", defaults.timeout.unwrap_or(-1).to_string()),
        method("sidekiq_worker_queues", ruby_str_literal(&queues.join(" "))),
        method("sidekiq_worker_max_retries", defaults.max_retries.unwrap_or(-1).to_string()),
        method("sidekiq_worker_dead_max_jobs", defaults.dead_max_jobs.unwrap_or(-1).to_string()),
        method("sidekiq_worker_dead_timeout", defaults.dead_timeout_in_seconds.unwrap_or(-1).to_string()),
        method("sidekiq_worker_poll_interval_average", float(defaults.poll_interval_average)),
        method("sidekiq_worker_scheduled_poll_interval", float(defaults.average_scheduled_poll_interval)),
    ]
    .concat()
}

/// Whether the tree sends broadcasts through Redis: on `:sidekiq`
/// (`needs_redis`), with config/cable.yml's production adapter `redis`.
fn cable_redis(app: &App) -> Option<&crate::app::CableRedisConfig> {
    if needs_redis(app) { app.cable_redis.as_ref() } else { None }
}

/// The `JobRegistry.cable_redis?` body for a tree that [`cable_redis`]
/// is true for. `spin_shape` keys the transport swap on it.
const CABLE_REDIS_REGISTRY_FLAG: &str = "  def self.cable_redis?\n    true\n  end\n";

/// The real Redis broadcast transport, swapped in for the façade at
/// `runtime/redis_cable.rb` on every Ruby-family lane for a tree that
/// [`cable_redis`] is true for.
const REDIS_CABLE: &str = "runtime/spinel/sidekiq/redis_cable.rb";

/// `JobRegistry`'s broadcast settings: whether they go through Redis,
/// where, and Action Cable's channel prefix.
fn cable_redis_source(app: &App) -> String {
    use crate::emit::ruby::expr::ruby_str_literal;
    match cable_redis(app) {
        None => "\n  def self.cable_redis?\n    false\n  end\n\n  def self.cable_redis_url\n    \"\"\n  end\n\n  def self.cable_channel_prefix\n    \"\"\n  end\n".to_string(),
        Some(config) => format!(
            "\n{CABLE_REDIS_REGISTRY_FLAG}\n  def self.cable_redis_url\n    {}\n  end\n\n  def self.cable_channel_prefix\n    {}\n  end\n",
            redis_setting_source(Some(&config.url)),
            ruby_str_literal(&config.channel_prefix),
        ),
    }
}

fn apply_job_registry(files: &mut [(String, String)], app: &App) {
    const HEAD: &str = "  # >>> generated: job-registry\n";
    const TAIL: &str = "  # <<< generated: job-registry\n";
    let flag = if needs_redis(app) {
        SIDEKIQ_REGISTRY_FLAG.to_string()
    } else {
        "  def self.sidekiq?\n    false\n  end\n".to_string()
    };
    let redis = format!(
        "\n  def self.sidekiq_redis_url\n    {}\n  end\n\n  def self.sidekiq_redis_password\n    {}\n  end\n{}",
        redis_setting_source(app.sidekiq_redis.url.as_ref()),
        redis_setting_source(app.sidekiq_redis.password.as_ref()),
        worker_defaults_source(&app.sidekiq_worker),
    );
    let redis = format!("{redis}{}", cable_redis_source(app));
    let generated = format!(
        "{HEAD}{}\n{flag}{redis}{TAIL}",
        crate::lower::job_payload::registry_source(&app.job_plans, needs_redis(app))
    );
    for (path, content) in files.iter_mut() {
        if !path.ends_with("job_registry.rb") {
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

/// Which tree [`apply_app_requires`] writes into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequireTarget {
    RubyFamily,
    Spinel,
}

/// Standard libraries in `ingest::app`'s requirable list that Spinel has
/// no package or native module for: `spinel` warns "is not available in
/// Spinel" for each, and an app binary's `--strict-require` (from `spin
/// flags`) makes that fatal. The spinel tree leaves them out, as it did
/// before app requires were carried; the ruby family loads them all.
const SPINEL_LACKS: &[&str] = &[
    "delegate", "English", "etc", "io/nonblock", "io/wait", "ipaddr", "observer", "pp",
    "weakref",
];

/// The app's own standard-library requires ([`App::app_requires`]) at
/// the head of `boot.rb`, ahead of everything that file loads. Twice on
/// the ruby family, like the mixins: the overlay's boot.rb replaces the
/// spinel one wholesale, so each final tree gets the block from its own
/// call. Idempotent, so a tree that already has it keeps one copy.
///
/// A library whose constant the tree defines itself is left out, by the
/// rule `bundled_require_gaps` reads for the same table: the runtime's
/// own `ERB` and `Shellwords` stand in for the libraries, and loading
/// the library as well would reopen them (spinel's erb package opens
/// `class ERB` over the runtime's module).
fn apply_app_requires(files: &mut [(String, String)], app: &App, target: RequireTarget) {
    let provided_by_tree = |lib: &str| {
        let require_line = format!("require {lib:?}");
        BUNDLED.iter().filter(|(_, feature)| *feature == lib).any(|(konst, _)| {
            files.iter().any(|(p, c)| {
                p.ends_with(".rb") && defines_constant(c, konst) && !requires_feature(c, &require_line)
            })
        })
    };
    let libs: Vec<&String> = app
        .app_requires
        .iter()
        .filter(|lib| !provided_by_tree(lib))
        .filter(|lib| target == RequireTarget::RubyFamily || !SPINEL_LACKS.contains(&lib.as_str()))
        .collect();
    if libs.is_empty() {
        return;
    }
    const MARK: &str = "# The app's own standard-library requires";
    let Some((_, boot)) = files.iter_mut().find(|(p, _)| p == "boot.rb") else { return };
    if boot.contains(MARK) {
        return;
    }
    let mut block = format!(
        "{MARK} (`require` lines in app/ and lib/,\n\
         # which the emitted files do not carry; see App::app_requires).\n"
    );
    for lib in libs {
        block.push_str(&format!("require {lib:?}\n"));
    }
    block.push('\n');
    boot.insert_str(0, &block);
}

/// `Rails::VERSION` as the app's own Rails answers it: the `rails` (or
/// `railties`) version its `Gemfile.lock` resolved, split the way
/// railties' `gem_version.rb` splits it (MAJOR, MINOR, TINY, PRE, and
/// STRING joined from them), for an app that prints
/// `Rails::VERSION::STRING`. Without a lockfile nothing is written and the
/// constant stays undefined, as before. The block goes right after
/// `runtime/rails` loads, ahead of the app's own files, as `require
/// "rails"` defines it in Rails before `config/application.rb` runs.
fn apply_rails_version(files: &mut [(String, String)], app: &App) {
    const MARK: &str = "# Rails::VERSION from the app's Gemfile.lock";
    let Some(lock) = &app.gem_lock else { return };
    let Some(version) = ["rails", "railties"].iter().find_map(|g| {
        lock.specs.iter().find(|(n, _)| n == g).map(|(_, v)| v.clone())
    }) else {
        return;
    };
    // A version is digits, letters and dots (`8.1.0.rc1`); anything else
    // would be spelled into a double-quoted Ruby string as written.
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        return;
    }
    let Some((_, boot)) = files.iter_mut().find(|(p, _)| p == "boot.rb") else { return };
    if boot.contains(MARK) {
        return;
    }
    let parts: Vec<&str> = version.split('.').collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let pre = if parts.len() > 3 { format!("{:?}", parts[3..].join(".")) } else { "nil".to_string() };
    let block = format!(
        "{MARK} (see project.rs::apply_rails_version).\n\
         module Rails\n  module VERSION\n    MAJOR = {}\n    MINOR = {}\n    TINY = {}\n    PRE = {pre}\n    STRING = {version:?}\n  end\nend\n",
        num(0), num(1), num(2)
    );
    const AFTER: &str = "require_relative \"runtime/rails\"\n";
    match boot.find(AFTER) {
        Some(at) => boot.insert_str(at + AFTER.len(), &block),
        None => {
            boot.push('\n');
            boot.push_str(&block);
        }
    }
}

/// `AppEnvironment` at the end of boot.rb. `assume_ssl?(env)` is
/// `config.assume_ssl = true` in that environment (Rails'
/// `ActionDispatch::AssumeSSL`). `serving_assume_ssl?` reads
/// `RAILS_ENV` (development when unset) so both dispatchers share one
/// check. Always defined, so a tree with the flag off still answers
/// false.
fn apply_environment_flags(files: &mut [(String, String)], app: &App) {
    const MARK: &str = "# Per-environment flags, from config/environments/*.rb";
    let Some((_, boot)) = files.iter_mut().find(|(p, _)| p == "boot.rb") else {
        return;
    };
    if boot.contains(MARK) {
        return;
    }
    let cond = if app.assume_ssl_envs.is_empty() {
        "false".to_string()
    } else {
        app.assume_ssl_envs.iter().map(|e| format!("env == {e:?}")).collect::<Vec<_>>().join(" || ")
    };
    boot.push_str(&format!(
        "\n{MARK} (see project.rs::apply_environment_flags).\n\
         module AppEnvironment\n  def self.assume_ssl?(env)\n    {cond}\n  end\n  \
         def self.serving_assume_ssl?\n    name = ENV[\"RAILS_ENV\"].to_s\n    \
         if name.empty?\n      name = \"development\"\n    end\n    \
         assume_ssl?(name)\n  end\nend\n"
    ));
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

    if app.module_mixins.is_empty()
        && app.initializer_filters.is_empty()
        && app.initializer_response_headers.is_empty()
        && app.collection_cache_store.is_none()
    {
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
    // The headers, one reopen per target, after the action as Rails runs
    // an `after_action`: the controller asks this once the action has
    // answered, and a `before_action` that answered skips it.
    let mut header_targets: Vec<&str> =
        app.initializer_response_headers.iter().map(|h| h.target.as_str()).collect();
    header_targets.dedup();
    for target in header_targets {
        let _ = writeln!(
            block,
            "\n# `after_action` the app registers on a framework controller in\n\
             # config/initializers/ (generated — see apply_module_mixins). The\n\
             # controller asks this after its action.\n\
             class {target}\n  def initializer_after_filters(action_name)"
        );
        for h in app.initializer_response_headers.iter().filter(|h| h.target.as_str() == target) {
            let write = format!(
                "headers[{}] = {}",
                crate::emit::ruby::expr::ruby_str_literal(&h.name),
                crate::emit::ruby::expr::ruby_str_literal(&h.value)
            );
            if h.only.is_empty() {
                let _ = writeln!(block, "    {write}");
            } else {
                let only: Vec<String> = h.only.iter().map(|a| format!(":{}", a.as_str())).collect();
                let _ = writeln!(block, "    {write} if [{}].include?(action_name)", only.join(", "));
            }
        }
        let _ = writeln!(block, "    nil\n  end\nend");
    }
    // The store cached collections read, as the app's initializer points
    // it (`lower::module_mixins` kept it only when the reader resolves).
    // Here for the same reason as a mixin: it names an app constant, so
    // it runs once every class has loaded, where `to_prepare` runs it.
    if let Some(store) = &app.collection_cache_store {
        let _ = writeln!(
            block,
            "\n# `ActionView::PartialRenderer.collection_cache` as config/initializers/\n\
             # assigns it (generated — see apply_module_mixins).\n\
             ActionView::PartialRenderer.collection_cache = {}.{}",
            store.receiver.as_str(),
            store.reader.as_str()
        );
    }
    for (path, content) in files.iter_mut() {
        if path == "boot.rb" {
            content.push_str(&block);
            return;
        }
    }
}

/// `app/route_helpers.rb` is generated from the app's named routes, so
/// a tree whose source carries no `config/routes.rb` (an engine, a
/// gem's test app) gets no file. Code that names a route helper still
/// requires `app/route_helpers` at LOAD time, so the tree did not boot
/// (`cannot load such file -- app/route_helpers`, from a controller).
/// The module is emitted empty instead: the tree loads, and a call to a
/// helper raises NoMethodError naming it, as Rails does for a route that
/// is not declared.
fn apply_route_helpers_demand(files: &mut Vec<(String, String)>) {
    const PATH: &str = "app/route_helpers.rb";
    if files.iter().any(|(p, _)| p == PATH) {
        return;
    }
    let named = files
        .iter()
        .any(|(p, c)| p.ends_with(".rb") && !p.starts_with("runtime/") && c.contains("RouteHelpers"));
    if !named {
        return;
    }
    files.push((
        PATH.to_string(),
        "# The source app declares no named routes (its config/routes.rb is\n\
         # not in this tree), so no route helper is generated. Code that\n\
         # names one loads; calling it raises NoMethodError, naming it.\n\
         module RouteHelpers\n\
         end\n"
            .to_string(),
    ));
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

/// Rails-dom-testing's `css_select` for a spinel tree that carries the
/// real Nokogiri (spinel-nokogiri): the elements of the last response
/// matching `selector`, as Nokogiri nodes, parsed the way the CRuby
/// overlay parses them (`test_support_cruby.rb`). Only when a test calls
/// it — a tree without the package has nothing to answer it with, and
/// there the call stays the compile error it is.
/// The helper itself. A string here rather than a file under
/// `runtime/spinel/test/`: every file there ships into the emitted
/// `test/`, where `spin test` builds it as a program of its own.
const SPINEL_CSS_SELECT: &str = r#"
# Rails-dom-testing's `css_select`: the elements of the last response
# matching `selector`, as Nokogiri nodes (`node["id"]`). Appended to a
# spinel tree's harness only when it carries spinel-nokogiri and a test
# calls it (`project::apply_spinel_css_select`); the CRuby tree has its
# own in test_support_cruby.rb. rails-dom-testing parses with HTML5.
require "nokogiri"

class TestBase
  def css_select(selector)
    Nokogiri::HTML5.fragment(@__response.body.to_s).css(selector).to_a
  end
end
"#;

fn apply_spinel_css_select(files: &mut [(String, String)]) {
    let called = files
        .iter()
        .any(|(p, c)| p.starts_with("test/") && p.ends_with(".rb") && c.contains("css_select("));
    if !called {
        return;
    }
    if let Some((_, helper)) = files.iter_mut().find(|(p, _)| p == "test/test_helper.rb") {
        if !helper.contains("def css_select(") {
            if !helper.ends_with('\n') {
                helper.push('\n');
            }
            helper.push_str(SPINEL_CSS_SELECT);
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
    let mut seen = crate::det::HashSet::default();
    let current_classes = &app.current_attribute_classes;
    let mut arms = String::new();
    // Same rule as the routes.rb require header: a route may name a
    // controller the app never defines (campfire's `resource :settings`
    // under `scope module: "rooms"`). Rails resolves lazily and fails
    // only on request; an eager `require_relative` for a missing file
    // takes the whole process down at boot.
    let defined: crate::det::HashSet<&str> =
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
    // Both trees: the test helper's `assert_recognizes` reads it too.
    apply_controller_paths(files, &flat);
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

/// Add `RouteTable::CONTROLLER_PATHS` to routes.rb. It maps the router
/// symbol of a namespaced controller to the Rails controller path
/// (`admin__posts: "admin/posts"`). The ruby overlay's `recognize_path`
/// and the test helper's `assert_recognizes` read it. A top-level controller has no row, and
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
    // The test helper's route generation puts the root back among the
    // declarations (`RoutingAssertions#generated_route`).
    const ROOT_INSERT: &str = "        routes.insert(RouteTable.root_position, RouteTable.root)\n";
    let has_root = crate::lower::flatten_routes(app).iter().any(|r| r.path == "/");
    if has_root {
        return;
    }
    for (path, content) in files.iter_mut() {
        if path == "main.rb" || path == "test/test_helper.rb" {
            *content = content.replace(WITH_ROOT, "RouteTable.table").replace(ROOT_INSERT, "");
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
    let param_binds = BuildTarget::Spinel.param_binds_enabled(None);
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
    let (mut files, _) = spinel_files_with_source_markers(app, fixture, param_binds)?;
    spinel_relation_model_handle(&mut files)?;
    widen_spinel_hash_config_arrays(&mut files);

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

/// Class/module values defined in `runtime/ruby/`
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
    "ActionDispatch::Http::Parameters::ParseError",
    "AbstractController::ActionNotFound",
    "ActionView::MissingTemplate",
    // `runtime/ruby/active_record/arel.rb` — node constructors the ruby
    // family and spinel load with that file. A strict target has no Arel
    // text nodes to construct.
    "Arel::Nodes",
    "Arel::Nodes::Node",
    "Arel::Nodes::SqlLiteral",
    "Arel::Nodes::Quoted",
    "Arel::Nodes::Not",
    "Arel::Nodes::Case",
    "Arel::Nodes::NamedFunction",
    "Arel::Nodes::Ascending",
    "Arel::Nodes::Descending",
    "Arel::Nodes::Predicate",
    "ActiveModel::UnknownAttributeError",
    // `runtime/ruby/active_record/connection_not_established.rb`,
    // `encryption_errors.rb` and `stale_object_error.rb`. A rescue of
    // ConnectionNotEstablished catches the two children. Strict targets
    // do not load the files.
    "ActiveRecord::ConnectionFailed",
    "ActiveRecord::ConnectionTimeoutError",
    "ActiveRecord::Encryption::Errors::Decryption",
    "ActiveRecord::Encryption::Errors::Configuration",
    "ActiveRecord::StaleObjectError",
    // `runtime/ruby/active_record/preloading.rb`. The two-keyword call
    // lowers away; the class value does not ship on a strict target.
    "ActiveRecord::Associations::Preloader",
    // `runtime/ruby/active_record/types.rb`. Boolean stays on the cast
    // lowerer; these two classes are the ruby-family casters.
    "ActiveRecord::Type::String",
    "ActiveRecord::Type::Integer",
    "ActiveRecord::Type::Value",
    // Thread-local state and the notification fan-out. ruby family and
    // spinel, required from rails.rb. ConfigurationFile parses YAML and is
    // kept out of every Spinel boot, so that class stays on the ruby family.
    "ActiveSupport::IsolatedExecutionState",
    "ActiveSupport::Notifications",
    "ActiveSupport::Notifications::Event",
    "ActiveSupport::ConfigurationFile",
    // Exception classes and their newly exposed owners in
    // `runtime/ruby/active_storage.rb`. Registering a nested error also
    // resolves its enclosing namespace as a value, which needs the same
    // runtime availability policy as the error itself.
    "ActiveStorage::Transformers",
    "ActiveStorage::Transformers::Transformer",
    "ActiveStorage::Transformers::ImageProcessingTransformer",
    "ActiveStorage::Service::GCSService",
    "ActiveStorage::Error",
    "ActiveStorage::InvariableError",
    "ActiveStorage::UnpreviewableError",
    "ActiveStorage::UnrepresentableError",
    "ActiveStorage::IntegrityError",
    "ActiveStorage::FileNotFoundError",
    "ActiveStorage::PreviewError",
    "ActiveStorage::InvalidKeyError",
    "ActiveStorage::Preview::UnprocessedError",
    "ActiveStorage::Service::GCSService::MetadataServerError",
    "ActiveStorage::Service::GCSService::MetadataServerNotFoundError",
    "ActiveStorage::Transformers::ImageProcessingTransformer::UnsupportedImageProcessingMethod",
    "ActiveStorage::Transformers::ImageProcessingTransformer::UnsupportedImageProcessingArgument",
    // `runtime/ruby/active_job.rb`.
    "ActiveJob::EnqueueError",
    "ActiveJob::Continuable",
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
    // but recognizes GeneratorError only as a literal rescue name, and
    // its `packages/net` stops short of net/protocol's `HTTPBadResponse`.
    if target == "spinel" {
        // The Arel ordering and predicate nodes live on the CRuby overlay (`arel_predicates.rb`).
        if matches!(name, "ActiveSupport::ConfigurationFile" | "Arel::Nodes::Ascending" | "Arel::Nodes::Descending" | "Arel::Nodes::Predicate") {
            return Some("ruby_family_runtime_constant");
        }
        return if matches!(name, "JSON::GeneratorError" | "Net::HTTPBadResponse") {
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
        "URI::Generic" | "URI::HTTP" | "URI::HTTPS" | "URI::Error" | "URI::InvalidURIError" | "Net::OpenTimeout" | "Net::ReadTimeout"
        | "SocketError"
        | "Net::HTTPRedirection" | "Net::HTTPOK" | "StringIO" | "OpenSSL::OpenSSLError"
        | "Net::HTTPSuccess" | "Net::HTTPBadResponse" | "Zlib::DataError" | "OpenSSL::Random"
        // `packages/strscan` and `packages/zlib` on spinel; no strict runtime carries either.
        | "StringScanner" | "Zlib::GzipWriter"
        // ruby-vips on the ruby family, spinel-ruby-vips on spinel.
        | "Vips::Image"
        | "Rails::HTML5::SafeListSanitizer" | "JSON" | "JSON::ParserError"
        | "Struct" | "Mutex" | "Queue" | "SizedQueue"
        | "Thread::Queue" | "Thread::SizedQueue" | "Thread::Mutex"
        | "ThreadError" | "ClosedQueueError" | "RegexpError" | "Comparable" | "Enumerable"
        | "LoadError" | "Proc" | "Range" | "Method" | "Gem::Version"
        // `runtime/ruby/ipaddr.rb` ships to spinel only; no strict runtime list carries it.
        | "IPAddr" | "IPAddr::InvalidAddressError"
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
        | "ActiveSupport::Cache::NullStore"
        // rqrcode_core's / spinel-rqrcode's.
        | "RQRCodeCore::QRCodeRunTimeError" | "RQRCodeCore::QRCodeArgumentError"
        | "GC" | "OpenSSL::HMAC" | "CSV::MalformedCSVError"
        | "Enumerator::Product" | "Enumerator::Lazy" | "Enumerator::Chain"
        | "Monitor" | "ConditionVariable" | "Thread::ConditionVariable"
        | "UNIXServer" | "UNIXSocket" | "Open3" | "Find")
        // The rexml gem on the ruby family, spinel's `packages/rexml`.
        || name == "REXML"
        || name.starts_with("REXML::");
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
    ("Zlib", "gunzip"),
    ("Zlib", "deflate"),
    ("Zlib", "inflate"),
    ("Zlib", "adler32"),
    ("Zlib", "zlib_version"),
    // Answered from the Db shim (`runtime/spinel/
    // active_record_db_config.rb`); a strict target's Db glue has none.
    ("ActiveRecord::Base", "connection_db_config"),
    ("ActiveRecord::Base", "connection_pool"),
    // `lower::record_snapshot`'s closed-world `constantize` — the
    // generated `case` lives in the same ruby-family/spinel file.
    ("ActiveRecord::Base", "instantiate_named"),
    // `lower::preloader` over several models' records: `runtime/ruby/active_record/preloading.rb`, which no strict target loads.
    ("ActiveRecord::Preloading", "records"),
    // `lower::errors_index`'s `errors.to_hash`, `messages`,
    // `attribute_names`, `delete`, `details`, `import` and `merge!`: the helpers are
    // `runtime/ruby/active_support_ext.rb`, which no strict target loads.
    // The ActiveModel::Attributes writers call the same module. Flattening
    // the include is not a claim that a strict target can run the cast.
    ("ActiveSupport", "cast_string"),
    ("ActiveSupport", "cast_integer"),
    ("ActiveSupport", "cast_float"),
    ("ActiveSupport", "cast_decimal"),
    ("ActiveSupport", "numericality_number"),
    ("ActiveSupport", "cast_boolean"),
    ("ActiveSupport", "cast_datetime"),
    ("ActiveSupport", "change_time_usec"),
    ("ActiveSupport", "errors_to_hash"),
    ("ActiveSupport", "errors_messages"),
    ("ActiveSupport", "errors_attribute_names"),
    ("ActiveSupport", "errors_delete"),
    ("ActiveSupport", "errors_details"),
    ("ActiveSupport", "error_detail"),
    ("ActiveSupport", "error_template"),
    ("ActiveSupport", "errors_import"),
    ("ActiveSupport", "errors_merge"),
];

/// Instance methods only the ruby family and spinel can answer: a model's
/// `attributes_before_type_cast` is a Hash of every column's raw stored
/// value, one value type per column, which a strict target's Hash cannot
/// hold.
const RUBY_SPINEL_ONLY_INSTANCE_METHODS: &[&str] = &["attributes_before_type_cast", "assign_attributes"];

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

/// Whether `recv`'s class carries a `method` the compiler wrote (its name span synthetic), as opposed to one the app declares.
fn calls_synthesized_method(recv: &crate::expr::Expr, method: &str, app: &App) -> bool {
    fn class_ids(ty: &crate::ty::Ty, out: &mut Vec<crate::ident::ClassId>) {
        match ty {
            crate::ty::Ty::Class { id, .. } => out.push(id.clone()),
            crate::ty::Ty::Union { variants } => variants.iter().for_each(|v| class_ids(v, out)),
            _ => {}
        }
    }
    let Some(ty) = &recv.ty else { return false };
    let mut ids = Vec::new();
    class_ids(ty, &mut ids);
    ids.iter().any(|id| {
        app.models.iter().filter(|m| m.name == *id).any(|m| {
            m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Method { method: def, .. }
                if def.name.as_str() == method && def.name_span.is_synthetic()))
        }) || app.library_classes.iter().filter(|c| c.name == *id).any(|c| {
            c.methods.iter().any(|def| def.name.as_str() == method && def.name_span.is_synthetic())
        })
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
    // Column readers are synthesized inside the emitter, after this walk,
    // so a Send match never sees them. The schema column is the fact.
    // Ruby, JRuby, and Spinel ship the interval intrinsics. Roda is Sequel:
    // it has no parser, so a text column typed as Duration is not a runtime.
    if !matches!(
        target,
        BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
    ) {
        for table in app.schema.tables.values() {
            for column in &table.columns {
                if column.col_type == crate::schema::ColumnType::Interval {
                    emit::diagnostics::report_unsupported(
                        crate::span::Span::synthetic(),
                        target.as_str(),
                        "interval",
                        format!(
                            "PostgreSQL interval column `{}.{}` lowers to ActiveSupport interval intrinsics; the {} runtime has no interval column runtime",
                            table.name.as_str(),
                            column.name.as_str(),
                            target.as_str()
                        ),
                    );
                }
            }
        }
    }
    // TypeScript emitted `class Enum extends Hash` and `this.is_has_key(...)`, neither of which exists there.
    if !matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Roda | BuildTarget::Jruby | BuildTarget::Spinel) {
        let classes = app.library_classes.iter().chain(app.test_modules.iter().flat_map(|tm| tm.inner_classes.iter()));
        for lc in classes.filter(|lc| !lc.is_module) {
            if let Some(parent) = lc.parent.as_ref().filter(|p| matches!(p.0.as_str(), "Hash" | "Array" | "String" | "Set")) {
                emit::diagnostics::report_unsupported(
                    lc.parent_span,
                    target.as_str(),
                    "builtin subclass",
                    format!("`{}` subclasses {}, which the {} emit has no class to extend", lc.name.0.as_str(), parent.0.as_str(), target.as_str()),
                );
            }
        }
    }
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Roda) {
        return;
    }
    if !matches!(target.as_str(), "jruby" | "spinel" | "futamura") {
        fn implicit_assignments(expr: &crate::expr::Expr, target: &str) {
            if let crate::expr::ExprNode::Send { recv: None, method, .. } = &*expr.node
                && method.as_str() == "assign_attributes"
            {
                emit::diagnostics::report_unsupported(expr.span, target, "assign_attributes",
                    "ordered attribute assignment is supplied by the Ruby-family runtime");
            }
            expr.node.for_each_child(&mut |child| implicit_assignments(child, target));
        }
        for model in &app.models {
            if crate::analyze::source_instance_method(app, &model.name, "assign_attributes") {
                continue;
            }
            for item in &model.body {
                if let crate::dialect::ModelBodyItem::Method { method, .. } = item
                    && method.receiver == crate::dialect::MethodReceiver::Instance
                {
                    implicit_assignments(&method.body, target.as_str());
                }
            }
        }
    }
    fn visit(expr: &crate::expr::Expr, app: &App, target: &str) {
        // Not the whole constant: spinel answers `Method` as a value, and refuses only a class test against it.
        if target == "spinel" {
            let names_method = |e: &crate::expr::Expr| matches!(&*e.node, crate::expr::ExprNode::Const { path }
                if path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).eq(["Method"]));
            let class_test = match &*expr.node {
                crate::expr::ExprNode::Send { method, args, .. } => {
                    matches!(method.as_str(), "is_a?" | "kind_of?" | "instance_of?") && args.first().is_some_and(names_method)
                }
                crate::expr::ExprNode::Case { arms, .. } => arms.iter().any(|arm| {
                    matches!(&arm.pattern, crate::expr::Pattern::Expr { expr } if names_method(expr))
                }),
                _ => false,
            };
            if class_test {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "Method",
                    "a class test against `Method`, which the spinel emit refuses".to_string(),
                );
            }
        }
        // Spinel reads `x::CONST` through one of several classes, but not every
        // explicit Module query on such a value (`const_get` raises there and
        // `const_defined?` answers false), so it refuses them all.
        if target == "spinel"
            && expr.decisions & crate::expr::MODULE_QUERY != 0
            && expr.decisions & crate::expr::CONST_PATH == 0
            && let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && matches!(recv.ty, Some(crate::ty::Ty::Union { .. }))
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "Module protocol",
                format!("`{}` on one of several classes, which the spinel emit does not answer", method.as_str()),
            );
        }
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
        // Only the Ruby-family trees ship the Duration value class
        // (`runtime/spinel/active_support_duration.rb`) that `30.minutes`
        // grounds to and a declared
        // `#: ActiveSupport::Duration` names.
        if !matches!(target, "jruby" | "spinel") {
            let names_duration = |ty: &crate::ty::Ty| {
                let found = std::cell::Cell::new(false);
                ty.map_class_ids(&|id| {
                    found.set(found.get() || id.0.as_str() == "ActiveSupport::Duration");
                    id.clone()
                });
                found.get()
            };
            let duration = match &*expr.node {
                crate::expr::ExprNode::Const { path } => {
                    path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).eq(["ActiveSupport", "Duration"])
                }
                crate::expr::ExprNode::Cast { target_ty, .. } => names_duration(target_ty),
                _ => false,
            };
            if duration && !app.library_classes.iter().any(|class| class.name.0.as_str() == "ActiveSupport::Duration") {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "duration",
                    format!("ActiveSupport::Duration is the Ruby-family runtime's value class; the {target} runtime ships none"),
                );
            }
            // Interval columns lower to these intrinsics. Ruby, JRuby, and
            // Spinel ship them; a strict target must not emit a call it
            // cannot run.
            if let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
                && matches!(method.as_str(), "parse_db_interval" | "format_db_interval")
                && matches!(&*recv.node, crate::expr::ExprNode::Const { path }
                    if path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).eq(["ActiveSupport"]))
            {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "interval",
                    format!("PostgreSQL interval columns use ActiveSupport.{}; the {target} runtime has no interval column runtime", method.as_str()),
                );
            }
        }
        // A regexp literal maps to each target's own; the `Regexp` class does not, so its methods were emitted as names nothing defines.
        if !matches!(target, "jruby" | "spinel")
            && let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && matches!(&*recv.node, crate::expr::ExprNode::Const { path } if path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).eq(["Regexp"]))
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "Regexp",
                format!("`Regexp.{}` is Ruby's Regexp class, which the {target} emit does not map (a regexp literal it does)", method.as_str()),
            );
        }
        if !matches!(target, "jruby" | "spinel")
            && let crate::expr::ExprNode::Const { path } = &*expr.node
            && let ["Regexp", flag @ ("IGNORECASE" | "EXTENDED" | "MULTILINE")] = path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).collect::<Vec<_>>()[..]
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "bundled_constant",
                format!("Regexp::{flag} is Ruby's option bit, which the {target} emit has no Regexp to read"),
            );
        }
        // Typed as IO for the Ruby family; a strict emit spelled them as names its runtime does not define.
        if !matches!(target, "jruby" | "spinel")
            && let Some(name) = match &*expr.node {
                crate::expr::ExprNode::Const { path } => match path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).collect::<Vec<_>>()[..] {
                    [n @ ("STDIN" | "STDOUT" | "STDERR")] => Some(n.to_string()),
                    _ => None,
                },
                crate::expr::ExprNode::Var { name, .. } if matches!(name.as_str(), "$stdin" | "$stdout" | "$stderr") => Some(name.as_str().to_string()),
                _ => None,
            }
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "bundled_constant",
                format!("`{name}` is Ruby's process stream, which the {target} emit does not map"),
            );
        }
        if !matches!(target, "jruby" | "spinel")
            && let crate::expr::ExprNode::Const { path } = &*expr.node
            && matches!(path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).collect::<Vec<_>>()[..], ["RUBY_VERSION"] | ["Object", "RUBY_VERSION"])
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "bundled_constant",
                format!("`RUBY_VERSION` is the running Ruby's, which the {target} emit has none of"),
            );
        }
        // A strict emit spelled these as methods its class objects do not have.
        // A constant path through a value (`CONST_PATH`) is one too, also
        // where a lowering builds it after the analysis (a validation's
        // callable `in:`).
        if !matches!(target, "jruby" | "spinel")
            && let crate::expr::ExprNode::Send { method, .. } = &*expr.node
            && expr.decisions & (crate::expr::MODULE_QUERY | crate::expr::CONST_PATH) != 0
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "Module protocol",
                format!("`{}` asks a class object what it holds when the program runs, which the {target} emit cannot answer", method.as_str()),
            );
        }
        // These newly admitted class objects are retained by Ruby/JRuby only.
        if target != "jruby"
            && expr.decisions & crate::expr::RUNTIME_CLASS_OBJECT != 0
            && let crate::expr::ExprNode::Send { method, .. } = &*expr.node
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "runtime class object",
                format!("`{}` requires a runtime class object; this form is supported on Ruby and JRuby only", method.as_str()),
            );
        }
        // A name computed when it runs: the Ruby family defines it then, which no compiled target can.
        if !matches!(target, "jruby")
            && let crate::expr::ExprNode::Send { recv, method, args, .. } = &*expr.node
            && matches!(method.as_str(), "define_method" | "define_singleton_method")
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, crate::expr::ExprNode::SelfRef))
            && !args.first().is_some_and(|a| matches!(&*a.node, crate::expr::ExprNode::Lit { value: crate::expr::Literal::Sym { .. } | crate::expr::Literal::Str { .. } }))
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "runtime-defined method",
                format!("`{}` with a name computed when it runs, which the {target} emit cannot define ahead of time", method.as_str()),
            );
        }
        if !matches!(target, "jruby" | "spinel" | "futamura")
            && let crate::expr::ExprNode::Send { method, .. } = &*expr.node
            && method.as_str() == "lock"
            && let Some(crate::ty::Ty::Relation { of }) = &expr.ty
            && !crate::analyze::source_class_method(app, of, "lock")
        {
            emit::diagnostics::report_unsupported(
                expr.span, target, "locking relation",
                format!("the SQLite locking relation is supplied by the Ruby-family runtime; the {target} runtime ships none"),
            );
        }
        // `runtime/ruby/active_record/connection.rb` carries `upsert_all` and the
        // `insert_all` forms the lowering does not inline; no strict runtime transpiles it.
        let answers_result = matches!(&expr.ty, Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "ActiveRecord::Result");
        let answers_rows = matches!(&expr.ty, Some(crate::ty::Ty::Array { elem }) if **elem == crate::ty::Ty::Untyped);
        if !matches!(target, "jruby" | "spinel")
            && let crate::expr::ExprNode::Send { recv, method, args, block, .. } = &*expr.node
            && ((matches!(method.as_str(), "upsert" | "upsert_all" | "insert" | "insert!" | "insert_all!") && answers_result)
                || (method.as_str() == "insert_all" && answers_rows))
            // This walk runs before lowering: a call `scope_chain` inlines compiles
            // everywhere. A receiverless call in a model's class method is never inlined.
            && !recv.as_ref().is_some_and(|recv| {
                crate::lower::scope_chain::inlines_insert_all(
                    recv,
                    method.as_str(),
                    args,
                    block.is_some(),
                    &crate::lower::scope_chain::model_set(&app.models),
                )
            })
        {
            let construct = if method.as_str().starts_with("upsert") { "upsert" } else { "insert_all" };
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                construct,
                format!("`{}` is the Ruby-family runtime's; the {target} runtime ships none", method.as_str()),
            );
        }
        // `translate_site` / `translate_runtime` are reported with their call (`report_i18n_table_sites`).
        if let crate::expr::ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && matches!(&*recv.node, crate::expr::ExprNode::Const { path } if path.iter().map(|s| s.as_str()).skip_while(|s| s.is_empty()).eq(["I18n"]))
        {
            let method = method.as_str();
            if !I18N_RUNTIME_TARGETS.contains(&target) && !matches!(method, "translate_site" | "translate_runtime") {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "I18n",
                    format!("`I18n.{method}` is the Ruby-family runtime's (the locale in effect, switching it, or a missing translation's answer); the {target} runtime ships none"),
                );
            } else if I18N_RUNTIME_TARGETS.contains(&target)
                && !I18N_CATALOG_TARGETS.contains(&target)
                && method != "translate_runtime"
                && crate::runtime_loader::I18N_CATALOG_METHODS.contains(&method)
            {
                emit::diagnostics::report_unsupported(
                    expr.span,
                    target,
                    "I18n",
                    format!("`I18n.{method}` reads the app's whole catalog when it runs; the {target} runtime carries the locale-chosen table, not the catalog"),
                );
            }
        }
        if matches!(&*expr.node, crate::expr::ExprNode::Const { .. }) {
            if let Some(crate::ty::Ty::Class { id, .. }) = &expr.ty {
                report_unavailable_class_value(app, target, id.0.as_str(), expr.span);
            }
        }
        // Not emitted on the other strict targets: the written-down JSON encoder (`lower::as_json_poro`) has been built and run only on the ruby family, spinel, Rust, TypeScript, Python, Crystal, C#, Kotlin, Swift, Elixir and Go.
        if let crate::expr::ExprNode::Send { recv: Some(_), method, .. } = &*expr.node
            && !matches!(target, "spinel" | "jruby" | "typescript" | "python" | "crystal" | "csharp" | "kotlin" | "swift" | "elixir" | "go" | "rust")
            && !expr.span.is_synthetic()
            && (method.as_str() == crate::lower::JSON_WRITER_METHOD
                || method.as_str().starts_with(&format!("{}__", crate::lower::JSON_WRITER_METHOD)))
            && let crate::expr::ExprNode::Send { recv: Some(recv), .. } = &*expr.node
            && calls_synthesized_method(recv, method.as_str(), app)
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "json_writer",
                format!("the compiled `render json:` encoder (`#{method}`) is verified on the ruby family, spinel, Rust, TypeScript, Python, Crystal, C#, Kotlin, Swift, Elixir and Go only"),
            );
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
        // The pagy gem's `pagy(scope)` is `runtime/ruby/action_controller/
        // pagy.rb`, a Base reopen only the ruby family and spinel load. On
        // a strict target the call would reach a controller with no such
        // method; ledger it rather than emit it.
        if let crate::expr::ExprNode::Send { recv, method, .. } = &*expr.node
            && method.as_str() == "pagy"
            && target != "spinel"
            && target != "jruby"
            && !expr.span.is_synthetic()
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, crate::expr::ExprNode::SelfRef))
            && app.gem_lock.as_ref().is_some_and(|lock| lock.has("pagy"))
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "pagy",
                "the pagy gem's `pagy` is only available on the ruby family and spinel".to_string(),
            );
        }
        // Kernel#` is the backtick / `%x` shell-out. The ruby family and
        // spinel emit it as a literal and run it; every other target would
        // spell a method named "`" and fail to compile. One gate, not a
        // per-emitter shell.
        if let crate::expr::ExprNode::Send { method, .. } = &*expr.node
            && method.as_str() == "`"
            && !matches!(target, "spinel" | "jruby" | "futamura")
            && !expr.span.is_synthetic()
        {
            emit::diagnostics::report_unsupported(
                expr.span,
                target,
                "shell_command",
                format!("Kernel#` (a backtick / %x shell-out) is only available on the ruby family and spinel"),
            );
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
    // The throwaway lower is the strict-target shape whatever `target` is; its own reports belong to the target's real lower, which runs at emit.
    let (lowered_controllers, _) = emit::diagnostics::scope(|| {
        crate::lower::controller_to_library::lower_controllers_with_arel_and_views(
            &app.controllers,
            Vec::new(),
            Some(&app.schema),
            &app.views,
        )
    });
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
    // A validation's callable `in:` is read on the record, after the
    // analysis (`ValidationRule::InclusionCall`).
    for model in &app.models {
        for validation in model.validations() {
            for rule in &validation.rules {
                if let crate::dialect::ValidationRule::InclusionCall { set } | crate::dialect::ValidationRule::ExclusionCall { set } = rule {
                    visit(set);
                }
            }
        }
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
    // The immutable app-body survey also excludes association extensions,
    // scope lambdas, and belongs_to defaults. Those expressions are
    // copied into synthesized methods only while emitting, after this gate.
    for model in &app.models {
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Association { assoc, .. } = item else { continue };
            let (scope, default, extension) = match assoc {
                crate::dialect::Association::BelongsTo { default, .. } => (None, default.as_ref(), &[][..]),
                crate::dialect::Association::HasMany { scope, extension, .. } => (scope.as_ref(), None, extension.as_slice()),
                crate::dialect::Association::HasOne { scope, .. } => (scope.as_ref(), None, &[][..]),
                crate::dialect::Association::HasAndBelongsToMany { .. } => (None, None, &[][..]),
            };
            if let Some(scope) = scope {
                visit(scope);
            }
            if let Some(default) = default {
                visit(default);
            }
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
    param_binds: bool,
) -> Result<(Vec<(String, String)>, Vec<String>), String> {
    let (mut files, stems) =
        emit::ruby::source_markers::with_source_markers(app, || spinel_files(app, fixture, param_binds))?;
    apply_gem_packages(&mut files, app)?;
    for (path, content) in files.iter_mut() {
        if path.ends_with(".rb") {
            *content = emit::ruby::source_markers::finish(content);
        }
    }
    // `spinel_base_files` (the toolchain tree) and `--target spinel`
    // both come through here. The ruby family calls `spinel_files`
    // directly and keeps `ConfigurationFile`.
    drop_yaml_configuration_file(&mut files);
    Ok((files, stems))
}

// Not `lock.has` alone: `require: false` keeps Bundler.require from loading the gem, so its Railtie never inserts the middleware.
fn package_loads(lock: &crate::gems::Lockfile, package: &crate::gems::GemPackage) -> bool {
    lock.has(package.gem) && lock.gemfile_requires.get(package.gem).is_none_or(|features| !features.is_empty())
}

// Not left out silently: a setting the package's Config has no writer for is dropped from boot.rb, so the initializer is not applied as written.
fn report_spinel_gem_package_settings(app: &App, target: BuildTarget) {
    if !matches!(target, BuildTarget::Spinel) {
        return;
    }
    let Some(lock) = app.gem_lock.as_ref() else { return };
    for package in crate::gems::GEM_PACKAGES.iter().filter(|p| package_loads(lock, p)) {
        for setting in app.gem_package_settings.iter().filter(|s| s.gem == package.gem) {
            if !setting.carried() {
                crate::emit::diagnostics::report_unsupported(
                    setting.span, "spinel", "gem package setting",
                    format!("`{}` is not a setting the Spinel tree's {} package carries; the initializer line is not applied", setting.statement, package.gem),
                );
            }
        }
    }
}

fn apply_gem_packages(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    const ANCHOR: &str = "require_relative \"runtime/request_hooks\"\n";
    let Some(lock) = app.gem_lock.as_ref() else { return Ok(()) };
    let mut boot_lines = String::new();
    for package in crate::gems::GEM_PACKAGES.iter().filter(|p| package_loads(lock, p)) {
        let dir = format!("runtime/gems/{}", package.dir);
        // Not `walk_partitioned`'s sig/ split: spin_shape moves sidecars beside their .rb anyway, and the assets must ship too.
        crate::runtime_files::walk_into(&dir, &format!("{dir}/"), files)?;
        boot_lines.push_str(&format!("require_relative \"{dir}/{}\"\n", package.entry));
        let settings: Vec<&str> = app
            .gem_package_settings
            .iter()
            .filter(|s| s.gem == package.gem && s.carried())
            .map(|s| s.statement.as_str())
            .collect();
        if !settings.is_empty() {
            boot_lines.push_str(&format!("# {}'s settings from config/initializers.\n", package.gem));
            for statement in settings {
                boot_lines.push_str(statement);
                boot_lines.push('\n');
            }
        }
    }
    if boot_lines.is_empty() {
        return Ok(());
    }
    let (_, boot) = files
        .iter_mut()
        .find(|(p, _)| p == "boot.rb")
        .ok_or("apply_gem_packages: no boot.rb")?;
    let at = boot.find(ANCHOR).ok_or("apply_gem_packages: boot.rb lost its runtime/request_hooks require")? + ANCHOR.len();
    boot.insert_str(at, &boot_lines);
    Ok(())
}

fn spinel_files(app: &App, fixture: &Path, param_binds: bool) -> Result<(Vec<(String, String)>, Vec<String>), String> {
    let mut files: Vec<(String, String)> = Vec::new();

    crate::runtime_files::walk_into("runtime/spinel/scaffold", "", &mut files)?;

    crate::runtime_files::walk_partitioned("runtime/spinel/test",
        "test/",
        "sig/test/",
        &mut files,
    )?;
    // The scaffold's test/models and test/views, and the root tests below, exercise the blog's Article (its model, fixtures and routes); an app without that model would stop `rake test` at the first of them.
    if !app.models.iter().any(|m| m.name.0.as_str() == "Article") {
        const BLOG_ONLY: [&str; 5] = ["models/", "views/", "query_count_test.", "relation_find_test.", "route_helpers_test."];
        files.retain(|(path, _)| {
            !BLOG_ONLY.iter().any(|blog| path.starts_with(&format!("test/{blog}")) || path.starts_with(&format!("sig/test/{blog}")))
        });
    }

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
        let interval_rbs = crate::runtime_files::read_to_string("runtime/spinel/active_support_interval.rbs")
            .map_err(|e| format!("read runtime/spinel/active_support_interval.rbs: {e}"))?;
        files.push((
            "sig/runtime/active_support_interval.rbs".to_string(),
            interval_rbs,
        ));
    }

    // Spinel's `date` package for apps that use Date or DateTime. Default
    // boot has no Date requires; when needed we inject the package (stdlib
    // date + date parse/format + date JSON rewrite) after always-on AR
    // serialization.
    const DATE_PACKAGE_FILES: &[&str] = &[
        "runtime/active_support_date_parsing.rb",
        "runtime/active_record_date_serialization.rb",
        "sig/runtime/active_support_date_parsing.rbs",
        "sig/runtime/active_record_date_serialization.rbs",
    ];
    if needs_date {
        for (src, dest) in [
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

    // PostgreSQL array-column seam: the JSON text the row holds and the
    // typed Array the record keeps (`lower::array_columns`). Only an app
    // with a stored array column ships it.
    let stores_arrays = app.schema.tables.values().any(|t| t.columns.iter().any(|c| c.array_stored));
    if stores_arrays {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/array_column.rbs")
            .map_err(|e| format!("read runtime/spinel/array_column.rbs: {e}"))?;
        files.push(("sig/runtime/array_column.rbs".to_string(), rbs));
    } else {
        files.retain(|(p, _)| p != "runtime/array_column.rb" && p != "runtime/array_column.rbs");
    }
    inject_array_column_require(&mut files, app)?;

    // ActiveJob payload codecs. Same reason as the JSON-column sidecar:
    // the readers take parsed JSON, which only the sidecar can type.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/active_job_serialization.rbs")
            .map_err(|e| format!("read runtime/spinel/active_job_serialization.rbs: {e}"))?;
        files.push(("sig/runtime/active_job_serialization.rbs".to_string(), rbs));
    }

    // Sidekiq pushes (queue_adapter :sidekiq): the envelope writer and
    // the Redis façade. `spin_shape` drops the façade's sidecar when it
    // swaps in the real client, whose surface is inferred.
    for name in ["sidekiq_push", "sidekiq_redis", "sidekiq_envelope", "sidekiq_worker", "redis_cable"] {
        let src = format!("runtime/spinel/{name}.rbs");
        let rbs = crate::runtime_files::read_to_string(&src).map_err(|e| format!("read {src}: {e}"))?;
        files.push((format!("sig/runtime/{name}.rbs"), rbs));
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

    // ERB::Util shim sidecar: the flat walk emits runtime/erb_spinel.rb,
    // which reopens spinel's `packages/erb` class, and the .rbs pins
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

    // Any flash key (Rails' FlashHash): a Ruby-family and Spinel reopen of the shared Flash.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/flash_any_key.rbs")
            .map_err(|e| format!("read runtime/spinel/flash_any_key.rbs: {e}"))?;
        files.push(("sig/runtime/flash_any_key.rbs".to_string(), rbs));
    }

    // Rails 8.1.4 `send_data(filename:)` and its Content-Disposition
    // escaping, which works over UTF-8 bytes: Ruby family and Spinel only.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/action_controller_send_data.rbs")
            .map_err(|e| format!("read runtime/spinel/action_controller_send_data.rbs: {e}"))?;
        files.push(("sig/runtime/action_controller_send_data.rbs".to_string(), rbs));
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
        // `Rake::Task`. `active_record.rb` requires `rake/task`.
        "rake",
    ] {
        crate::runtime_files::walk_partitioned(
            &format!("runtime/ruby/{sub}"),
            &format!("runtime/{sub}/"),
            &format!("sig/runtime/{sub}/"),
            &mut files,
        )?;
    }
    if let Some(source) = app.rake_tasks_source.as_deref() {
        if let Some((_, content)) = files
            .iter_mut()
            .find(|(path, _)| path == "runtime/active_record/app_rake_tasks.rb")
        {
            *content = source.to_string();
        }
    }
    widen_key_contract(app, &mut files)?;
    controller_state_ivars(&mut files)?;
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
        "active_model_validator",
        // `Rails.event` / `Rails.error`: the reporters, ParameterFilter and
        // ExecutionContext. rails.rb requires it, so it ships beside rails.
        "active_support_reporters",
        "active_support_isolated_execution_state",
        "active_support_notifications",
        "active_support_configuration_file",
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
        // `Pay::Error` / `Pay::ActionRequired` + Charge/Subscription/
        // Customer class constants — Jumpstart Pro / pay gem first slice.
        // Stripped when the lock has no `pay` (see `apply_pay_demand`).
        "pay",
        // `I18n.locale` / `default_locale` — the locale campfire folds
        // into its cache keys; per thread, the app's default until a
        // request sets one (see the file's header).
        "i18n_host",
        "i18n_locale",
        // The i18n gem's `I18n::Locale::Tag`, which `i18n_locale.rb` requires.
        "i18n_locale_tag",
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

    let app_files = emit::ruby::emit_spinel_with_param_binds(app, param_binds);
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
    // Vendored extras whose bodies drive surface no spinel package
    // carries (`facades::EXTRAS_FACADES`) get raising façades at the same
    // require path — spinel AOT prices every reachable body and the
    // verbatim ones can't compile. The CRuby tree restores the verbatim
    // emit (real gems and stdlib there).
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
        // Everything under public/, at any depth: Rails serves the directory as
        // it is (ActionDispatch::Static), and hindmost's terminal loads xterm
        // from public/vendor/xterm, which the dev-directory skip dropped.
        walk_dir_into_skipping(&public, "public/", &[], &mut files)?;
    }
    // A PATH gem's own data, beside its compiled code.
    carry_path_gem_data(fixture, &mut files)?;
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
    apply_cable_actions(&mut files, app);
    apply_content_layout(&mut files, app);
    apply_content_helper_attributes(&mut files, app);
    apply_content_helper_tags(&mut files, app);
    apply_i18n_locales(&mut files, app);
    if !app.i18n_table.is_empty() || !app.i18n_runtime_sites.is_empty() {
        files.push(("config/i18n_translations".to_string(), i18n_table_text(app)));
    }
    // The `only:`-as-finder specializations, also in the shared base:
    // `runtime/global_id_locator.rb` is ONE file that both the spinel
    // tree and the ruby overlay require, and the rewrite that names
    // these entry points ran at lowering — before any target was
    // chosen. Generating on only one lane would leave the other calling
    // a method nothing defined.
    apply_global_id_locate(&mut files, app);
    apply_instantiate_named(&mut files, app);
    // Every SQLite shim in the base set (db.rb, db_cruby.rb); the JDBC one
    // is read in later, by `ruby_family_runtime_files`.
    apply_database_pragmas(&mut files, app);
    apply_attachable_locate(&mut files, app);
    // Same reason: both lanes require `runtime/job_registry.rb`.
    apply_job_registry(&mut files, app);
    apply_views_aggregator(&mut files);
    apply_models_aggregator(&mut files);
    apply_route_helpers_demand(&mut files);
    apply_module_mixins(&mut files, app, MixinForm::Reopen);
    apply_environment_flags(&mut files, app);
    apply_app_requires(&mut files, app, RequireTarget::Spinel);
    apply_rails_version(&mut files, app);
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
    apply_pagy_demand(&mut files, app)?;
    apply_pay_demand(&mut files, app)?;
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
/// `ConfigurationFile.parse` loads YAML, and `rails.rb` would pull that
/// file, and with it spinel's whole psych package, into every Spinel
/// boot; an app that names YAML gets the require itself (`BUNDLED`).
/// The ruby family keeps both.
fn drop_yaml_configuration_file(files: &mut Vec<(String, String)>) {
    const REQUIRE: &str = "require_relative \"active_support_configuration_file\"\n";
    if let Some((_, rails)) = files.iter_mut().find(|(path, _)| path == "runtime/rails.rb") {
        if rails.contains(REQUIRE) {
            *rails = rails.replacen(REQUIRE, "", 1);
        }
    }
    files.retain(|(path, _)| {
        path != "runtime/active_support_configuration_file.rb"
            && path != "sig/runtime/active_support_configuration_file.rbs"
    });
}

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

/// `runtime/ruby/pay.rb` rides the tree only for an app whose
/// Gemfile.lock carries `pay`. Without the gem the façade would still
/// define `Pay::Error`, which is fine for name resolution but wrong for
/// an app that never asked for billing — strip it the same way pagy is.
/// With the gem, ensure `gem_facades.rb` loads it (the CRuby/JRuby
/// rewrite of that file drops the source-tree `require_relative`), and
/// drop any `require "pay"` that locked-external wiring added: the
/// gem stays **Unknown** in the census (so Pay::Currency dispatches
/// stay notes, not invented errors), and the façade is the runtime
/// half of the first-slice claim (`Pay::Error` / `is_a?(Pay::Charge)`).
fn apply_pay_demand(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    const REQUIRE: &str = "require_relative \"pay\"\n";
    let has_pay = app.gem_lock.as_ref().is_some_and(|l| l.has("pay"));
    if !has_pay {
        if let Some((_, facades)) = files.iter_mut().find(|(p, _)| p == "runtime/gem_facades.rb") {
            if facades.contains(REQUIRE) {
                *facades = facades.replacen(REQUIRE, "", 1);
            }
        }
        files.retain(|(p, _)| {
            p != "runtime/pay.rb"
                && p != "runtime/pay.rbs"
                && p != "sig/runtime/pay.rbs"
        });
        return Ok(());
    }
    if let Some((_, facades)) = files.iter_mut().find(|(p, _)| p == "runtime/gem_facades.rb") {
        if !facades.contains(REQUIRE) {
            facades.push_str(REQUIRE);
        }
    }
    // Prefer the façade over Bundler's `pay` gem for this slice.
    for (path, content) in files.iter_mut() {
        if path != "boot.rb" && path != "Gemfile" {
            continue;
        }
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| {
                let t = line.trim();
                t != "require \"pay\"" && !t.starts_with("gem \"pay\"")
            })
            .collect();
        *content = filtered.join("\n");
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
    }
    Ok(())
}

/// The pagy gem's `pagy` / `Pagy` (runtime/ruby/action_controller/pagy.rb)
/// ride the tree only for an app whose Gemfile.lock carries pagy: the file
/// reopens `ActionController::Base`, and an app without the gem has no
/// `pagy` method to model (a controller of its own named `pagy` would
/// otherwise meet the base's).
fn apply_pagy_demand(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    if app.gem_lock.as_ref().is_some_and(|l| l.has("pagy")) {
        return Ok(());
    }
    const REQUIRE: &str = "require_relative \"action_controller/pagy\"\n";
    let ac = files
        .iter_mut()
        .find(|(p, _)| p == "runtime/action_controller.rb")
        .ok_or("spinel tree has no runtime/action_controller.rb")?;
    if !ac.1.contains(REQUIRE) {
        return Err("runtime/action_controller.rb: the pagy require moved".into());
    }
    ac.1 = ac.1.replacen(REQUIRE, "", 1);
    files.retain(|(p, _)| {
        p != "runtime/action_controller/pagy.rb"
            && p != "runtime/action_controller/pagy.rbs"
            && p != "sig/runtime/action_controller/pagy.rbs"
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
    const TEST_GEMS: [(Marker, &str, &str); 6] = [
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
        // The `lower::ruby_family_gems` gems in a test body alone:
        // `gems::RUNTIME_GEMS` reads app/ only, so without these rows the
        // suite would reach a gem its Gemfile never named.
        (Marker::Constant("Retryable"), "retryable", "retryable"),
        (Marker::Constant("JWT"), "jwt", "jwt"),
        (Marker::Constant("RestClient"), "rest-client", "rest-client"),
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
            // ruby-vips loads the system libxml. Nokogiri's packaged
            // libxml (the one with the wildcard-namespace patch that
            // `*:a` and `css-class()` need) is loaded when boot
            // requires nokogiri. A `require "vips"` above that line
            // binds the process to libxml 2.9 and every HTML5 CSS
            // wildcard raises `Invalid expression`.
            let after_boot = *gem == "webmock" || *gem == "ruby-vips";
            match helper.find(BOOT) {
                Some(at) if after_boot => helper.insert_str(at + BOOT.len(), &format!("{line}\n")),
                _ => insert_require_line(helper, &line),
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
            // The scaffold lock no longer matches; `bundle install`
            // resolves the edited Gemfile fresh.
            files.retain(|(p, _)| p != "Gemfile.lock");
        }
    }
}

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

/// `lock_gated` appends a project's extension requires
/// ([`lock_gated_requires`]) after the compiler's own list, in extension
/// order, skipping one the list already has.
fn gem_require_block(exclude: &[&str], lock_gated: &[&crate::extensions::LockGatedRequire]) -> String {
    let mut requires: Vec<&str> = GEM_REQUIRES.iter().copied().filter(|g| !exclude.contains(g)).collect();
    for row in lock_gated {
        if !exclude.contains(&row.gem.as_str()) && !requires.contains(&row.require.as_str()) {
            requires.push(&row.require);
        }
    }
    let names: Vec<String> = requires.iter().map(|g| format!("{g:?}")).collect();
    let mut out = format!("[{}].each do |gem_name|\n", names.join(", "));
    out.push_str(
        "  begin\n    require gem_name\n  rescue LoadError => error\n    \
         $stderr.puts \"roundhouse: require #{gem_name.inspect} failed: #{error.message}\" unless error.path == gem_name\n    \
         nil\n  end\nend\n",
    );
    out
}

/// Extension boot requires whose gems the lock resolves. PATH gems remain
/// source inputs; registry and Git gems load natively on Ruby and JRuby.
fn lock_gated_requires(app: &App) -> Vec<&crate::extensions::LockGatedRequire> {
    let Some(lock) = app.gem_lock.as_ref() else { return Vec::new() };
    app.extensions
        .lock_gated_requires
        .iter()
        .filter(|row| lock.has(&row.gem) && !lock.is_in_repo(&row.gem))
        .collect()
}

/// Declare extension boot gems with the locked version or Git source.
fn apply_lock_gated_gem_wiring(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    let mut gems: Vec<&str> = Vec::new();
    for row in lock_gated_requires(app) {
        if !gems.contains(&row.gem.as_str()) {
            gems.push(&row.gem);
        }
    }
    if gems.is_empty() {
        return Ok(());
    }
    let Some((_, gemfile)) = files.iter_mut().find(|(path, _)| path == "Gemfile") else {
        return Err("Ruby-family project has no Gemfile for extension boot gems".into());
    };
    for gem in gems {
        let declaration = runtime_gem_declaration(app.gem_lock.as_ref(), gem, true)?;
        ensure_runtime_gem_declaration(gemfile, gem, &declaration);
    }
    files.retain(|(path, _)| path != "Gemfile.lock");
    Ok(())
}

fn runtime_gem_declaration(lock: Option<&crate::gems::Lockfile>, gem: &str, pin_version: bool) -> Result<String, String> {
    if let Some(source) = lock.and_then(|lock| lock.git_source_of(gem)) {
        if source.remote.is_empty() || source.revision.is_empty() {
            return Err(format!("Git source for gem {gem} requires both a remote and a locked revision"));
        }
        let mut declaration = format!("gem {gem:?}, git: {:?}, ref: {:?}", source.remote, source.revision);
        if let Some(glob) = &source.glob {
            declaration.push_str(&format!(", glob: {glob:?}"));
        }
        if source.submodules {
            declaration.push_str(", submodules: true");
        }
        return Ok(declaration);
    }
    match lock.and_then(|lock| lock.version_of(gem)).filter(|_| pin_version) {
        Some(version) => Ok(format!("gem {gem:?}, \"= {version}\"")),
        None => Ok(format!("gem {gem:?}")),
    }
}

fn ensure_runtime_gem_declaration(gemfile: &mut String, gem: &str, declaration: &str) {
    if gemfile.lines().any(|line| line.trim() == declaration)
        || rewrite_gemfile_declaration(gemfile, gem, declaration)
    {
        return;
    }
    if !gemfile.ends_with('\n') {
        gemfile.push('\n');
    }
    gemfile.push_str(declaration);
    gemfile.push('\n');
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
fn apply_runtime_gem_wiring(files: &mut Vec<(String, String)>, app: &App) -> Result<(), String> {
    // `app/` only. The runtime tree names several of these itself (the
    // façade lists all nine), and a scan that saw those would declare
    // every gem for every app — the same self-fulfilling wiring the
    // test table had to exclude `test_helper.rb` to avoid.
    let app_body_has = |found: &dyn Fn(&str) -> bool| {
        files.iter().any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && found(c))
    };
    let mut needed: Vec<&str> = Vec::new();
    for (marker, gem) in compiler_runtime_gems() {
        let demanded = app_body_has(&|c| match marker {
            Marker::Constant(konst) => names_constant(c, konst),
            Marker::AnyText(needles) => needles.iter().any(|n| c.contains(n)),
        });
        if demanded {
            needed.push(gem);
        }
    }
    // A project's extension rows, after the compiler's and in extension
    // order (`crate::extensions`).
    for row in &app.extensions.runtime_gems {
        let demanded = app_body_has(&|c| match &row.demand {
            crate::extensions::Demand::Constant(konst) => names_constant(c, konst),
            crate::extensions::Demand::AnyText(needles) => needles.iter().any(|n| c.contains(n.as_str())),
        });
        if demanded && !needed.contains(&row.gem.as_str())
            && !app.gem_lock.as_ref().is_some_and(|lock| lock.is_in_repo(&row.gem))
        {
            needed.push(&row.gem);
        }
    }
    // redis-rb, for a tree on queue_adapter :sidekiq. Its demand is the
    // generated registry's flag, not a constant in app/: the client that
    // names `Redis` is the runtime's own `sidekiq_redis.rb`.
    if files
        .iter()
        .any(|(p, c)| p == "runtime/job_registry.rb" && c.contains(SIDEKIQ_REGISTRY_FLAG))
        && !needed.contains(&"redis")
    {
        needed.push("redis");
    }
    let gated = lock_gated_requires(app);
    if needed.is_empty() && gated.is_empty() {
        return Ok(());
    }
    if let Some((_, gemfile)) = files.iter_mut().find(|(path, _)| path == "Gemfile") {
        for gem in &needed {
            let declaration = runtime_gem_declaration(app.gem_lock.as_ref(), gem, false)?;
            if app.gem_lock.as_ref().is_some_and(|lock| lock.is_from_git(gem)) {
                ensure_runtime_gem_declaration(gemfile, gem, &declaration);
            } else if !gemfile.contains(&format!("gem {gem:?}")) {
                ensure_runtime_gem_declaration(gemfile, gem, &declaration);
            }
        }
    }
    for row in gated {
        if !needed.contains(&row.gem.as_str()) {
            needed.push(&row.gem);
        }
    }
    apply_git_gem_load_paths(files, &needed, app.gem_lock.as_ref())
}

/// Activate the locked Git checkouts before any guarded native requires.
/// Never replace a missing checkout with a published gem of the same name.
fn apply_git_gem_load_paths(
    files: &mut Vec<(String, String)>,
    needed: &[&str],
    lock: Option<&crate::gems::Lockfile>,
) -> Result<(), String> {
    let Some(lock) = lock else { return Ok(()) };
    let git_gems = lock.git_specs_in_closure(needed);
    if git_gems.is_empty() {
        return Ok(());
    }
    let mut entries = String::new();
    for gem in &git_gems {
        let source = lock.git_source_of(gem).expect("Git closure member");
        let declaration = runtime_gem_declaration(Some(lock), gem, false)?;
        let Some((_, gemfile)) = files.iter_mut().find(|(path, _)| path == "Gemfile") else {
            return Err("Ruby-family project has no Gemfile for Git gems".into());
        };
        ensure_runtime_gem_declaration(gemfile, gem, &declaration);
        entries.push_str(&format!(
            "  [{gem:?}, {:?}, {:?}, {:?}, {:?}],\n",
            source.checkout_dir(), source.remote, source.revision,
            source.glob.as_deref().unwrap_or("{,*,*/*}.gemspec"),
        ));
    }
    let mut body = format!("# frozen_string_literal: true\n# Git sources pinned by the application's Gemfile.lock.\n[\n{entries}");
    body.push_str(r##"].each do |name, checkout, remote, revision, glob|
  directory = Gem.path.map { |root| File.join(root, "bundler", "gems", checkout) }.find { |path| File.directory?(path) }
  unless directory
    raise LoadError, "roundhouse: git gem #{name} (#{remote} at #{revision}) has no checkout bundler/gems/#{checkout} on Gem.path"
  end
  if (loaded = Gem.loaded_specs[name])
    root = File.expand_path(directory) + File::SEPARATOR
    unless (File.expand_path(loaded.full_gem_path) + File::SEPARATOR).start_with?(root)
      raise LoadError, "roundhouse: git gem #{name} (#{remote} at #{revision}) is already loaded from #{loaded.full_gem_path}, not #{directory}"
    end
    next
  end
  spec = Dir[File.join(directory, glob)].filter_map { |file| Gem::Specification.load(file) }.find { |candidate| candidate.name == name }
  unless spec
    raise LoadError, "roundhouse: git gem #{name} (#{remote} at #{revision}) has no gemspec in #{directory}"
  end
  spec.full_gem_path = File.dirname(spec.loaded_from)
  spec.extension_dir = File.join(File.dirname(directory), "extensions", Gem::Platform.local.to_s,
    Gem.extension_api_version, [checkout, File.basename(spec.full_gem_path)].uniq.join("-"))
  spec.activate
  # Framework specs can satisfy dependencies, but their Ruby files must not
  # replace the framework implementations in this emitted project.
  if %w[rails railties activesupport activemodel activerecord activejob activestorage actionpack actionview actioncable actionmailer actionmailbox actiontext].include?(name)
    paths = spec.full_require_paths
    $LOAD_PATH.reject! { |path| paths.include?(path) }
  end
end
"##);
    let Some((_, facades)) = files.iter_mut().find(|(path, _)| path == "runtime/gem_facades.rb") else {
        return Err("Ruby-family project has no gem facade loader for Git gems".into());
    };
    facades.insert_str(0, "require_relative \"git_gems\"\n");
    files.push(("runtime/git_gems.rb".into(), body));
    files.retain(|(path, _)| path != "Gemfile.lock");
    Ok(())
}

/// Carry direct, unmodeled gems that the emitted CRuby app actually names.
/// The source lockfile is the version authority; source constants are the
/// demand signal. Remove the scaffold lock so Bundler can resolve those pins.
fn apply_locked_external_gem_wiring(files: &mut Vec<(String, String)>, app: &App) {
    let Some(lock) = app.gem_lock.as_ref() else {
        return;
    };
    let census = crate::gems::GemCensus::of(lock);
    // Not only the gem's conventional namespace spelled exactly: svg-graph's is `SVG::Graph`.
    let mut owned = crate::det::HashSet::default();
    let ownership = crate::gems::LockGemIndex::new(lock);
    for_each_app_constant(app, &mut |_, path| {
        owned.extend(ownership.owners(path).into_iter().map(str::to_string));
    });
    let needed: Vec<_> = census
        .unknown()
        .filter(|gem| {
            // Fail-closed: a GIT-sourced unknown must not be rewritten as a
            // rubygems.org name/version pin.
            if lock.is_from_git(&gem.name) {
                return false;
            }
            // A gem a project extension says reaches Rails is never
            // required natively (`crate::extensions`).
            if app.extensions.is_rails_dependent(&gem.name) {
                return false;
            }
            let namespace = crate::gems::namespace_of(&gem.name);
            owned.contains(&gem.name)
                || files.iter().any(|(path, content)| {
                    (path.starts_with("app/") || path.starts_with("lib/"))
                        && path.ends_with(".rb")
                        && names_constant(content, &namespace)
                })
        })
        .cloned()
        .collect();
    if needed.is_empty() {
        return;
    }
    let Some((_, gemfile)) = files.iter_mut().find(|(path, _)| path == "Gemfile") else {
        return;
    };
    let mut eager_requires = Vec::new();
    for gem in needed {
        // Not only a gem that loads by its name: the app's Gemfile can say what Bundler requires for it.
        match lock.gemfile_requires.get(&gem.name) {
            Some(features) => eager_requires.extend(features.iter().cloned()),
            None if crate::gems::loads_by_gem_name(&gem.name) => eager_requires.push(gem.name.clone()),
            None => {}
        }
        let version = gem.version.as_deref();
        let pinned = match version {
            Some(v) => format!("gem {:?}, \"= {v}\"", gem.name),
            None => format!("gem {:?}", gem.name),
        };
        if gemfile_declares_exact(gemfile, &gem.name, version) {
            continue;
        }
        if rewrite_gemfile_declaration(gemfile, &gem.name, &pinned) {
            continue;
        }
        if !gemfile.ends_with('\n') {
            gemfile.push('\n');
        }
        gemfile.push_str(&format!(
            "\n# Required by emitted application constants.\n{pinned}\n"
        ));
    }
    // Not only the gems the app names: a real gem can load a bundled one the app's own Gemfile lists for it (lobsters' rexml for svg-graph).
    for dep in lock.dependencies.iter().filter(|dep| crate::gems::fate_of(dep) == crate::gems::GemFate::Stdlib) {
        if !gemfile.contains(&format!("gem {dep:?}")) {
            gemfile.push_str(&format!("gem {dep:?}\n"));
        }
    }
    if let Some((_, boot)) = files.iter_mut().find(|(path, _)| path == "boot.rb") {
        let requires = eager_requires
            .iter()
            .filter(|feature| !boot.contains(&format!("require {feature:?}")))
            .map(|feature| format!("require {feature:?}\n"))
            .collect::<String>();
        if !requires.is_empty() {
            insert_require_line(boot, &requires);
        }
    }
    files.retain(|(path, _)| path != "Gemfile.lock");
}

/// True when `Gemfile` already has `gem "name"` or `gem "name", "= ver"`.
fn gemfile_declares_exact(gemfile: &str, name: &str, version: Option<&str>) -> bool {
    let want = match version {
        Some(v) => format!("gem {name:?}, \"= {v}\""),
        None => format!("gem {name:?}"),
    };
    gemfile.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == want || trimmed.starts_with(&format!("{want} #")) || trimmed.starts_with(&format!("{want}\t#"))
    })
}

/// Rewrite an existing `gem "name"` / `gem "name", …` line to `pinned`.
/// Returns true when a declaration was found and updated.
fn rewrite_gemfile_declaration(gemfile: &mut String, name: &str, pinned: &str) -> bool {
    let marker = format!("gem {name:?}");
    let Some(line_start) = gemfile.find(&marker).map(|idx| {
        gemfile[..idx].rfind('\n').map(|n| n + 1).unwrap_or(0)
    }) else {
        return false;
    };
    let line_end = gemfile[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(gemfile.len());
    let line = &gemfile[line_start..line_end];
    let trimmed = line.trim_start();
    if !trimmed.starts_with(&marker) {
        return false;
    }
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];
    let comment = trimmed
        .find(" #")
        .or_else(|| trimmed.find("\t#"))
        .map(|i| &trimmed[i..])
        .unwrap_or("");
    let replacement = format!("{indent}{pinned}{comment}");
    gemfile.replace_range(line_start..line_end, &replacement);
    true
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
fn propshaft_stylesheet_asset_name(app: &App, stem: &str) -> Option<String> {
    let propshaft = app
        .gem_lock
        .as_ref()
        .is_some_and(|lock| lock.has("propshaft"))
        && app.rails_application.as_ref().is_some_and(|class| {
            class
                .methods
                .iter()
                .any(|method| method.name.as_str() == "propshaft_asset_path")
        });
    if !propshaft {
        return None;
    }
    let prefix = format!("static/assets/{stem}-");
    app.binary_assets
        .iter()
        .find(|(path, _)| path.starts_with(&prefix) && path.ends_with(".css"))
        .map(|(path, _)| path["static/assets/".len()..].to_string())
}

fn insert_propshaft_stylesheet_copy_rules(makefile: &mut String, app: &App) {
    const ANCHOR: &str = "$(ASSETS)/%.css: app/assets/stylesheets/%.css";
    let Some(at) = makefile.find(ANCHOR) else {
        return;
    };
    let mut rules = String::new();
    for stem in &app.stylesheets {
        if stem == "tailwind" {
            continue;
        }
        let Some(out) = propshaft_stylesheet_asset_name(app, stem) else {
            continue;
        };
        rules.push_str(&format!(
            "$(ASSETS)/{out}: app/assets/stylesheets/{stem}.css\n\
             \t@mkdir -p $(dir $@)\n\
             \tcp $< $@\n\n"
        ));
    }
    if !rules.is_empty() {
        makefile.insert_str(at, &rules);
    }
}

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
                .map(|s| {
                    propshaft_stylesheet_asset_name(app, s)
                        .map(|name| format!("$(ASSETS)/{name}"))
                        .unwrap_or_else(|| format!("$(ASSETS)/{s}.css"))
                })
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
        if uses_tailwind {
            if let Some(digested) = propshaft_stylesheet_asset_name(app, "tailwind") {
                *content = content.replace(
                    "$(ASSETS)/tailwind.css: app/assets/tailwind.css",
                    &format!("$(ASSETS)/{digested}: app/assets/tailwind.css"),
                );
            }
        } else {
            strip_tailwind_rule(content);
        }
        insert_propshaft_stylesheet_copy_rules(content, app);
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
///    compiles with strict require on, so a bare name that is not a
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
/// Whether the end of `head` lies inside a single- or double-quoted
/// literal on its line (a backslash escapes the next byte). Code inside a
/// double-quoted `#{…}` is code again.
fn inside_quotes(head: &[u8]) -> bool {
    // Open literals, innermost last, each with the `{` depth of the
    // interpolation inside it (0 while in the literal's text).
    let mut open: Vec<(u8, usize)> = Vec::new();
    let mut escaped = false;
    let mut i = 0;
    while i < head.len() {
        let c = head[i];
        match open.last_mut() {
            Some((q, 0)) => {
                if escaped {
                    escaped = false;
                } else if c == b'\\' {
                    escaped = true;
                } else if c == *q {
                    open.pop();
                } else if *q == b'"' && c == b'#' && head.get(i + 1) == Some(&b'{') {
                    open.last_mut().unwrap().1 = 1;
                    i += 1;
                }
            }
            Some((_, depth)) if c == b'{' => *depth += 1,
            Some((_, depth)) if c == b'}' => *depth -= 1,
            _ if c == b'"' || c == b'\'' => open.push((c, 0)),
            _ => {}
        }
        i += 1;
    }
    matches!(open.last(), Some((_, 0)))
}

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
            let ident = |c: u8| matches!(c, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_');
            // `::RestClient` is the top-level name too; `Foo::RestClient`
            // is another one.
            let rooted = at >= 2 && &b[at - 2..at] == b"::" && (at == 2 || !ident(b[at - 3]));
            let head_ok = (at == 0 || rooted || !(ident(b[at - 1]) || b[at - 1] == b':'))
                && !inside_quotes(&b[..at]);
            let tail = at + konst.len();
            // A value: `client = RestClient`, `use(::RestClient)`, an
            // argument or array element, `"#{RestClient}"`. A `%w[...]`
            // word is a string.
            let value = {
                let start = if rooted { at - 2 } else { at };
                let before = line[..start].trim_end();
                let words = ["%w[", "%W[", "%i[", "%I["].iter().any(|w| before.ends_with(w));
                // `[ RestClient ]` — the BracketsSpaced array style.
                let after = line[tail..].trim_start().as_bytes().first();
                ((before.ends_with('=') || before.ends_with('(') || before.ends_with(',') || before.ends_with('['))
                    && !words
                    && matches!(after, None | Some(b')' | b',' | b']')))
                    || (before.ends_with("#{") && after == Some(&b'}'))
            };
            let tail_ok = value
                || matches!(b.get(tail), Some(b'.' | b'[' | b'('))
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
/// unless the Gemfile names it. rexml has been one since Ruby 3.0, and
/// its feature is `rexml/document`, so a require of the gem name or of a
/// path under it counts.
const BUNDLED_GEMS: [&str; 4] = ["base64", "bigdecimal", "csv", "rexml"];

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
            let under_gem = format!("require \"{gem}/");
            files.iter().any(|(p, c)| {
                p.ends_with(".rb")
                    && (requires_feature(c, &require_line)
                        || c.lines().any(|line| line.trim_start().starts_with(&under_gem)))
            })
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

/// Take out of `runtime/gem_facades.rb` (and its `.rbs`) each top-level
/// `module X` / `class X` block whose namespace a gem compiled from source
/// (`ingest::auto_source_gems`) defines: that is the real class, and the
/// façade's raising one would be a second definition of it (spinel
/// refuses two signatures for one method).
fn drop_facades_the_program_defines(files: &mut [(String, String)]) {
    let top_name = |line: &str| -> Option<String> {
        let rest = line.strip_prefix("module ").or_else(|| line.strip_prefix("class "))?;
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        (!name.is_empty()).then_some(name)
    };
    // Not any program file: campfire's own `module WebPush` reopens the namespace its façade fills.
    // Only a file compiled from a gem's source (its `#<SPINEL_SOURCE>` line names the mount) is the gem's.
    let defined: std::collections::BTreeSet<String> = files
        .iter()
        .filter(|(p, c)| {
            !p.starts_with("runtime/")
                && p.ends_with(".rb")
                && c.lines().any(|l| l.starts_with("#<SPINEL_SOURCE>") && l.contains(crate::ingest::source_gems::MOUNT))
        })
        .flat_map(|(_, c)| c.lines().filter_map(top_name).collect::<Vec<_>>())
        .collect();
    for (path, content) in files.iter_mut() {
        if path != "runtime/gem_facades.rb" && path != "runtime/gem_facades.rbs" {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        let mut keep = vec![true; lines.len()];
        let mut i = 0;
        while i < lines.len() {
            if top_name(lines[i]).is_some_and(|n| defined.contains(&n)) {
                let mut start = i;
                // The block's own comment header goes with it.
                while start > 0 && lines[start - 1].starts_with('#') {
                    start -= 1;
                }
                let end = (i + 1..lines.len()).find(|&j| lines[j] == "end").unwrap_or(lines.len() - 1);
                keep[start..=end].iter_mut().for_each(|k| *k = false);
                i = end + 1;
            } else {
                i += 1;
            }
        }
        if keep.iter().any(|k| !k) {
            let mut out: String = lines.iter().zip(&keep).filter(|(_, k)| **k).map(|(l, _)| format!("{l}\n")).collect();
            if !content.ends_with('\n') {
                out.pop();
            }
            *content = out;
        }
    }
}

/// Constant → bundled library that provides it. One table, read by
/// both the pass that writes the requires and the gate that checks a
/// tree for missing ones — a second copy is how the rule drifts.
const BUNDLED: [(&str, &str); 30] = [
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
    // `Vips::Image.new_from_buffer`: the ruby-vips gem on CRuby/JRuby, the
    // spinel-ruby-vips package on spinel; an app requires it by hand.
    ("Vips", "vips"),
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
    // already loaded, so apps call it without a require. The Spinel
    // port is `runtime/spinel/shellwords_spinel.rb` (module only — no
    // String/Array reopen, because packages/shellwords makes
    // String#split a PolyArray and the Rails tree fails C compile).
    // Defining the module drops this row. The file must not be named
    // `shellwords.rb`: walk_flat copies it into every target and
    // `runtime/` is on `$LOAD_PATH`, so that name would answer the
    // require below and hide stdlib `String#shellescape`. CRuby/JRuby
    // boot requires the stdlib feature by hand.
    ("Shellwords", "shellwords"),
    // `PTY.spawn`: the app writes `require "pty"` (Rails does not load
    // it), but an app file reaches the tree without its requires.
    // Spinel takes `packages/pty`.
    ("PTY", "pty"),
    // `FileUtils.mkdir_p` / `remove_entry` — campfire's WAL checkpointer
    // makes its lock directory. A default gem on CRuby/JRuby (Rails has
    // loaded it); spinel takes `packages/fileutils`.
    ("FileUtils", "fileutils"),
    // `Dir.mktmpdir` — campfire's WAL checkpointer test makes its lock
    // directory under one. Not a constant (Dir is core), so the row names
    // the call: `Dir` has `mktmpdir` only after `require "tmpdir"`, which
    // a booted Rails app has done and spinel's `packages/tmpdir` answers.
    ("Dir.mktmpdir", "tmpdir"),
    // `OpenSSL::HMAC.digest` - a default gem on CRuby/JRuby, spinel's
    // `packages/openssl`; an app that signs a payload without HTTP never
    // loads it through net/http.
    ("OpenSSL", "openssl"),
    // `Monitor.new`, `UNIXServer.new` / `UNIXSocket.new`, `Open3.capture3`
    // and `Find.find`: the standard library on CRuby/JRuby; spinel
    // defines Monitor and the socket classes only after the require, and
    // takes `packages/open3` and `packages/find`.
    ("Monitor", "monitor"),
    ("UNIXServer", "socket"),
    ("UNIXSocket", "socket"),
    ("Open3", "open3"),
    ("Find", "find"),
    // `REXML::Document.new(xml)` - a bundled gem an app requires by hand
    // (Rails does not load it); spinel takes `packages/rexml`. Without the
    // require the constant is not REXML's at all on spinel: another
    // `Document` in the tree (the Nokogiri facade's) answers the name.
    ("REXML", "rexml/document"),
    // `YAML.safe_load_file` / `rescue Psych::Exception`: Rails has loaded
    // yaml before the app, so an app names both with no require. yaml.rb
    // is `require "psych"` then `YAML = Psych`; spinel takes
    // `packages/yaml` and `packages/psych`.
    ("YAML", "yaml"),
    ("Psych", "psych"),
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
                && !gaps.contains(&(i, require_line.clone()))
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

/// `roundhouse.yml`'s `spinel.int_overflow` on every compile the Makefile
/// runs: the app binary through `spin build` (which passes what follows
/// `--` to the compiler) or the raw compiler, and each test binary.
fn apply_spinel_int_overflow(files: &mut [(String, String)], app: &App) {
    let Some(mode) = app.spinel_int_overflow.as_deref() else { return };
    let flag = format!("--int-overflow={mode}");
    let Some((_, makefile)) = files.iter_mut().find(|(p, _)| p == "Makefile") else { return };
    for (from, to) in [
        ("\t$(SPIN) build\n".to_string(), format!("\t$(SPIN) build -- {flag}\n")),
        ("\t$(SPINEL) main.rb $(RBS_FLAG) -o $@".to_string(), format!("\t$(SPINEL) main.rb $(RBS_FLAG) {flag} -o $@")),
        ("$(SPINEL_TEST_FLAGS) $< -o $@".to_string(), format!("$(SPINEL_TEST_FLAGS) {flag} $< -o $@")),
    ] {
        *makefile = makefile.replace(&from, &to);
    }
}

fn spin_shape(files: Vec<(String, String)>) -> Result<Vec<(String, String)>, String> {
    use crate::det::HashSet;

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
        let mut seen = HashSet::default();
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
        apply_spinel_css_select(&mut files);
    }

    // A gem compiled from source (svg-graph's `SVG`) defines what its raising façade stands in for.
    drop_facades_the_program_defines(&mut files);

    // redis: the same swap, for a tree on queue_adapter :sidekiq with a
    // payload job (`needs_redis`, recorded as the registry's `sidekiq?`).
    // `runtime/sidekiq_redis.rb` becomes the real connection over
    // spinel-redis, and the manifest gains the package. Every other tree
    // keeps the raising façade and no dependency.
    let needs_redis = files
        .iter()
        .any(|(p, c)| p == "runtime/job_registry.rb" && c.contains(SIDEKIQ_REGISTRY_FLAG));
    if needs_redis {
        let client = crate::runtime_files::read_to_string(SIDEKIQ_REDIS_CLIENT)?;
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/sidekiq_redis.rb")
            .ok_or("spin_shape: the app is on queue_adapter :sidekiq but runtime/sidekiq_redis.rb \
                    is not in the spinel file set")?;
        facade.1 = client;
        let worker = crate::runtime_files::read_to_string(SIDEKIQ_WORKER)?;
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/sidekiq_worker.rb")
            .ok_or("spin_shape: the app is on queue_adapter :sidekiq but runtime/sidekiq_worker.rb \
                    is not in the spinel file set")?;
        facade.1 = worker;
        files.retain(|(p, _)| p != "runtime/sidekiq_redis.rbs" && p != "runtime/sidekiq_worker.rbs");
    }
    // The Redis broadcast transport, for a tree on :sidekiq whose
    // config/cable.yml names `adapter: redis` (the registry's
    // `cable_redis?`).
    let cable_redis = files
        .iter()
        .any(|(p, c)| p == "runtime/job_registry.rb" && c.contains(CABLE_REDIS_REGISTRY_FLAG));
    if cable_redis {
        let real = crate::runtime_files::read_to_string(REDIS_CABLE)?;
        let facade = files
            .iter_mut()
            .find(|(p, _)| p == "runtime/redis_cable.rb")
            .ok_or("spin_shape: the tree sends broadcasts through Redis but runtime/redis_cable.rb \
                    is not in the spinel file set")?;
        facade.1 = real;
        files.retain(|(p, _)| p != "runtime/redis_cable.rbs");
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
    // An app that draws with vips itself (lobsters' StoryImage) needs the package too.
    let needs_vips = apply_image_processor_wiring(&mut files)?
        || files.iter().any(|(p, c)| p.starts_with("app/") && p.ends_with(".rb") && names_constant(c, "Vips"));

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
             # `Remote branch ... not found`). It names the release tag, so\n\
             # a build doesn't pick up whatever lands on `main` next.\n\
             bcrypt = { git = \"https://github.com/roundhouse-rb/spinel-bcrypt\", ref = \"v0.1.1\" }\n",
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
             ruby-vips = { git = \"https://github.com/roundhouse-rb/spinel-ruby-vips\", ref = \"v0.3.0\" }\n",
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
             rqrcode = { git = \"https://github.com/roundhouse-rb/spinel-rqrcode\", ref = \"v0.1.0\" }\n",
        );
    }
    if needs_nokogiri {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# HTML parsing, CSS/XPath and serialization: the nokogiri gem's\n\
             # surface over libxml2 2.13.9 with Nokogiri's patches, carried in\n\
             # the package. Git form until matz/spin-index#14 lands.\n\
             nokogiri = { git = \"https://github.com/roundhouse-rb/spinel-nokogiri\", ref = \"v0.3.1\" }\n",
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
             commonmarker = { git = \"https://github.com/roundhouse-rb/spinel-commonmarker\", ref = \"v0.1.0\" }\n",
        );
    }
    if needs_redis {
        if !manifest.lines().any(|l| l.trim() == "[dependencies]") {
            manifest.push_str("\n[dependencies]\n");
        }
        manifest.push_str(
            "# Redis, for queue_adapter :sidekiq: jobs are pushed in Sidekiq's\n\
             # format for a Sidekiq process to run. spinel-redis, pure Ruby over\n\
             # sp_net. Git form, pinned to a tag because `ref` takes a branch\n\
             # or tag, never a SHA. 0.3.0 adds the connect_timeout: and\n\
             # read_timeout: this push sets to 0.2.0's url:, AUTH, zadd and\n\
             # pipelined; 0.3.2 adds the script and evalsha the worker's\n\
             # scheduler runs.\n\
             redis = { git = \"https://github.com/roundhouse-rb/spinel-redis\", ref = \"v0.3.2\" }\n",
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
    // MINUS `--strict-require` (`--require-gate` before Spinel 6aaec4f41), which `spin flags` includes and the app
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
                "SPINEL ?= spinel\nSPIN   ?= spin\n\n# Package deps for the per-test compiles, from spin itself.\n# `--strict-require` is stripped: the app binary wants an\n# unresolvable require to be fatal, a test tree requires\n# gems with no spinel surface (vips, mocha, webmock) and\n# must still compile far enough to report its type errors.\nSPIN_TEST_FLAGS = $(shell $(SPIN) flags 2>/dev/null | sed -e 's/--strict-require //' -e 's/--require-gate //')",
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
        } else if needs_bcrypt || needs_vips || needs_rqrcode || needs_nokogiri || needs_commonmarker || needs_redis {
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
    const LANE_PARENTS: [&str; 6] = [
        "< TestBase",
        "< ActionDispatch::IntegrationTest",
        "< ActionController::TestCase",
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
/// (stdlib) is left bare for strict require to judge. Lines with
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
    rb_paths: &crate::det::HashSet<String>,
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
    rb_paths: &crate::det::HashSet<String>,
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
    walk_dir_into_skipping(src, prefix, SKIP_DIRS, out)
}

/// The text files of each PATH gem in the app's tree (`Gemfile.lock`'s
/// `PATH` remotes inside the app), other than the Ruby under its `lib/`,
/// which is compiled: a gem reads them by a path from its own file
/// (`File.join(File.dirname(__FILE__), "../VERSION")`), and the emitted
/// `__FILE__` names the source path (`emit::ruby::expr::source_file_path`).
/// Each `lib/` directory the source has is kept (with a `.keep` where no
/// file is carried under it), so a path that goes up through it resolves.
/// Hidden entries are skipped, as are files named for secrets and the
/// directories tools leave in a checkout (`node_modules`, `tmp`, `log`,
/// `coverage`).
fn carry_path_gem_data(fixture: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let Ok(lock) = fs::read_to_string(fixture.join("Gemfile.lock")) else { return Ok(()) };
    let secret = |name: &str| {
        let lower = name.to_ascii_lowercase();
        ["secret", "credential", "password", "token"].iter().any(|w| lower.contains(w))
            || [".key", ".pem", ".p12", ".env"].iter().any(|ext| lower.ends_with(ext))
    };
    for remote in crate::gems::lock_path_remotes(&lock) {
        let gem = Path::new(&remote);
        if gem.is_absolute() || gem.components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            continue;
        }
        let root = fixture.join(gem);
        if !root.is_dir() {
            continue;
        }
        let prefix = format!("{}/", remote.trim_end_matches('/').trim_start_matches("./"));
        let mut found = Vec::new();
        // Its own vendor/ and build/ may hold what it reads; what tools
        // leave beside a checkout does not.
        walk_dir_into_skipping(&root, &prefix, &["node_modules", "tmp", "log", "coverage", ".bundle"], &mut found)?;
        let lib = format!("{prefix}lib/");
        found.retain(|(rel, _)| {
            let name = rel.rsplit('/').next().unwrap_or(rel);
            !(rel.starts_with(&lib) && rel.ends_with(".rb")) && !secret(name)
        });
        let mut dirs = vec![root.join("lib")];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let rel = format!("{prefix}{}/", dir.strip_prefix(&root).unwrap_or(&dir).display());
            if !found.iter().any(|(path, _)| path.starts_with(&rel)) {
                found.push((format!("{rel}.keep"), String::new()));
            }
            for entry in entries.flatten() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if !hidden && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    dirs.push(entry.path());
                }
            }
        }
        out.extend(found);
    }
    Ok(())
}

/// [`walk_dir_into`] with the directory names to skip given explicitly.
fn walk_dir_into_skipping(
    src: &Path,
    prefix: &str,
    skip_dirs: &[&str],
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
            if ty.is_dir() && skip_dirs.contains(&name_str.as_ref()) {
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
    let targets: Vec<BuildTarget> =
        BuildTarget::ALL.iter().chain(BuildTarget::SITE_ONLY).copied().collect();
    build_archives(fixture, out, &targets)?;
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
    let app =
        ingest_app(fixture).map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    // Futamura needs the types but none of the lowerings, same split as
    // the single-target driver (`bin/roundhouse`).
    let analyzed = targets.contains(&BuildTarget::Futamura).then(|| {
        let mut app = app.clone();
        crate::analyze::Analyzer::new(&app).analyze(&mut app);
        app
    });
    for target in targets {
        // Analyze + the same post-analyze shared lowerings as the
        // single-target driver, with the target's own pre-analysis
        // choices (`session::analyze_and_lower_for`). What it refused or
        // left behind is printed per target, as the CLI prints it.
        let files = match (target, &analyzed) {
            (BuildTarget::Futamura, Some(analyzed)) => target_files(analyzed, fixture, *target)?,
            _ => {
                let mut target_app = app.clone();
                for d in crate::session::analyze_and_lower_for(&mut target_app, *target) {
                    eprintln!("roundhouse: {}: {}", target.as_str(), d.render(&target_app.sources));
                }
                target_files(&target_app, fixture, *target)?
            }
        };
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
            }, Ty::Class { id: ClassId(Symbol::new(name)), args: vec![].into() })
        };

        assert!(is_unported_model_instance_method(&receiver_for("Article"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Probe"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Custom"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Child"), "attributes_before_type_cast", &app));
        assert!(!is_unported_model_instance_method(&receiver_for("Included"), "attributes_before_type_cast", &app));
        assert!(is_unported_model_instance_method(
            &receiver(ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::new("receiver") },
                Ty::Union { variants: vec![Ty::Class { id: ClassId(Symbol::new("Article")), args: vec![].into() }, Ty::Nil].into() }),
            "attributes_before_type_cast",
            &app,
        ));
        assert!(!is_unported_model_instance_method(
            &receiver(ExprNode::Const { path: vec![Symbol::new("Article")] },
                Ty::Class { id: ClassId(Symbol::new("Article")), args: vec![].into() }),
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
            constant.ty = Some(Ty::Class { id: ClassId(Symbol::new("Net::HTTPOK")), args: vec![].into() });
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
            "class ArticlesControllerTest < ActionController::TestCase",
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

    /// A test gem added to the Gemfile drops the scaffold lock, which
    /// does not pin it; a Gemfile left alone keeps its lock.
    #[test]
    fn a_declared_test_gem_drops_the_stale_lock() {
        let test = |body: &str| {
            let mut files = vec![
                ("test/test_helper.rb".to_string(), String::new()),
                ("test/models/article_test.rb".to_string(), body.to_string()),
                ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
                ("Gemfile.lock".to_string(), "GEM\n".to_string()),
            ];
            apply_test_gem_wiring(&mut files);
            files.iter().any(|(p, _)| p == "Gemfile.lock")
        };
        assert!(!test("WebMock.stub_request(:get, \"/\")\n"));
        assert!(test("assert true\n"));
    }

    /// A rooted `::Name` names the top-level constant, and so does a value
    /// use; `Outer::Name` does not.
    #[test]
    fn names_constant_reads_a_rooted_name_and_a_value() {
        assert!(names_constant("x = ::RestClient.get(u)\n", "RestClient"));
        assert!(names_constant("::RestClient::Request.execute(a)\n", "RestClient"));
        assert!(names_constant("rescue ::RestClient::NotFound\n", "RestClient"));
        assert!(!names_constant("x = Foo::RestClient.get(u)\n", "RestClient"));
        assert!(!names_constant("x = MyRestClient.get(u)\n", "RestClient"));
        // As a value, bare or rooted.
        assert!(names_constant("CLIENT = RestClient\n", "RestClient"));
        assert!(names_constant("client = ::RestClient\n", "RestClient"));
        assert!(names_constant("register(RestClient)\n", "RestClient"));
        assert!(names_constant("pick(RestClient, 1)\n", "RestClient"));
        assert!(!names_constant("CLIENT = Outer::RestClient\n", "RestClient"));
        // Not prose in a literal.
        assert!(!names_constant("msg = \"Saved, RestClient down\"\n", "RestClient"));
        assert!(!names_constant("msg = \"Saved, RestClient\"\n", "RestClient"));
        assert!(!names_constant("msg = 'a \\' (RestClient)'\n", "RestClient"));
        assert!(!names_constant("doc = \"::RestClient::Request.execute\"\n", "RestClient"));
        assert!(!names_constant("doc = 'see RestClient.get(u)'\n", "RestClient"));
        // Interpolated code is code.
        assert!(names_constant("msg = \"#{::RestClient::Request.new(a).url}\"\n", "RestClient"));
        assert!(names_constant("msg = \"a #{h.fetch(:x) { RestClient.get(u) }} b\"\n", "RestClient"));
        assert!(!names_constant("msg = \"#{x} RestClient.get\"\n", "RestClient"));
        assert!(names_constant("CLIENTS = [RestClient]\n", "RestClient"));
        assert!(names_constant("CLIENTS = [ RestClient ]\n", "RestClient"));
        assert!(names_constant("CLIENTS = [ RestClient , Net::HTTP ]\n", "RestClient"));
        assert!(names_constant("CLIENTS = [Net::HTTP, ::RestClient]\n", "RestClient"));
        assert!(names_constant("msg = \"via #{RestClient}\"\n", "RestClient"));
        assert!(!names_constant("NAMES = %w[RestClient]\n", "RestClient"));
        assert!(!names_constant("NAMES = %i[RestClient]\n", "RestClient"));
        assert!(!names_constant("msg = \"[RestClient]\"\n", "RestClient"));
    }

    /// An app body that names ElasticAPM only as a value still declares
    /// elastic-apm: its marker is a constant, read by `names_constant`.
    #[test]
    fn elastic_apm_as_a_value_declares_the_gem() {
        assert!(compiler_runtime_gems().iter().any(|(m, gem)| {
            *gem == "elastic-apm" && matches!(m, Marker::Constant("ElasticAPM"))
        }));
        assert!(names_constant("APM = ElasticAPM\n", "ElasticAPM"));
        assert!(names_constant("instrument(ElasticAPM)\n", "ElasticAPM"));
    }

    /// A test body that alone names a `lower::ruby_family_gems` gem declares
    /// and requires it.
    #[test]
    fn a_test_body_naming_a_ruby_family_gem_declares_it() {
        for (body, gem) in [
            ("Retryable.retryable(tries: 2) { 1 }", "retryable"),
            ("JWT.encode({ a: 1 }, \"k\", \"HS256\")", "jwt"),
            ("RestClient.get(\"http://example.test\")", "rest-client"),
            ("client = ::RestClient", "rest-client"),
        ] {
            let mut files = vec![
                ("test/test_helper.rb".to_string(), "require_relative \"../boot\"\n".to_string()),
                (
                    "test/models/article_test.rb".to_string(),
                    format!("class ArticleTest < TestBase\n  def test_x\n    {body}\n  end\nend\n"),
                ),
                ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
            ];
            apply_test_gem_wiring(&mut files);
            let gemfile = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;
            assert!(gemfile.contains(&format!("{gem:?}")), "{gem}: {gemfile}");
            let helper = &files.iter().find(|(p, _)| p == "test/test_helper.rb").unwrap().1;
            assert!(helper.contains(&format!("require {gem:?}")), "{gem}: {helper}");
        }
    }

    /// `require "vips"` after boot. ruby-vips pulls in the system
    /// libxml; Nokogiri has to load its packaged one first or `*:a`
    /// and `css-class()` raise `Invalid expression`.
    #[test]
    fn vips_require_follows_boot() {
        let mut files = vec![
            (
                "test/test_helper.rb".to_string(),
                "require_relative \"../boot\"\n".to_string(),
            ),
            (
                "test/models/avatar_test.rb".to_string(),
                "class AvatarTest < TestBase\n  def test_pixels\n    Vips::Image.new_from_buffer(body, \"\")\n  end\nend\n"
                    .to_string(),
            ),
            ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
        ];
        apply_test_gem_wiring(&mut files);
        let helper = &files.iter().find(|(p, _)| p == "test/test_helper.rb").unwrap().1;
        let boot = helper.find("require_relative \"../boot\"\n").expect("boot");
        let vips = helper.find("require \"vips\"\n").expect("vips require");
        assert!(
            boot < vips,
            "vips must load after boot so Nokogiri's libxml wins:\n{helper}"
        );
        let gemfile = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;
        assert!(gemfile.contains("ruby-vips"), "{gemfile}");
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

    #[test]
    fn baked_css_also_lands_at_the_propshaft_name() {
        let mut app = App::new();
        app.binary_assets
            .push(("static/assets/tailwind-be38fee9.css".into(), Vec::new()));
        assert_eq!(
            baked_propshaft_name(&app, "tailwind.css").as_deref(),
            Some("tailwind-be38fee9.css")
        );
        assert_eq!(baked_propshaft_name(&app, "controllers/index.js"), None);
    }

    #[test]
    fn baked_css_fingerprint_is_the_injected_bytes() {
        let mut app = App::new();
        app.binary_assets
            .push(("static/assets/tailwind-be38fee9.css".into(), b"source".to_vec()));
        let css = "compiled-tailwind {}\n";
        let mut files = vec![(
            "config/application.rb".to_string(),
            "def propshaft_asset_path(source)\nwhen \"tailwind.css\"\n\"/assets/tailwind-be38fee9.css\"\nend\nend\n"
                .to_string(),
        )];
        place_baked_stylesheet(&mut files, &app, "tailwind.css", css);
        let digested = format!("tailwind-{}.css", propshaft_content_digest(css));
        assert!(files.iter().any(|(path, body)| {
            path == &format!("static/assets/{digested}") && body == css
        }));
        assert!(!files.iter().any(|(path, _)| path == "static/assets/tailwind-be38fee9.css"));
        let ruby = &files.iter().find(|(path, _)| path == "config/application.rb").unwrap().1;
        assert!(ruby.contains(&format!("/assets/{digested}")));
        assert!(!ruby.contains("be38fee9"));
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
            action_symbol: false,
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
            action_symbol: false,
        });
        let module = "module RouteTable\nend\n".to_string();
        let mut files = vec![
            ("config/routes.rb".to_string(), module.clone()),
            ("sig/config/routes.rbs".to_string(), module.clone()),
        ];
        apply_controller_paths(&mut files, &crate::lower::flatten_routes(&app));
        assert_eq!(
            files[0].1,
            "module RouteTable\n  CONTROLLER_PATHS = { admin__posts: \"admin/posts\" }.freeze\n\nend\n"
        );
        assert_eq!(files[1].1, module);
    }

    #[test]
    fn spinel_runtime_sig_conflicts_accept_quoted_method_names() {
        for name in [
            "probe",
            "[]",
            "[]=",
            "+",
            "ready?",
            "probe!",
            "value=",
            "class",
            "two words",
            "a:b",
            "a#b",
            "méthode",
            r"a\\b",
            "self.probe",
            "self?.probe",
            "two\nlines",
            "a\"b",
            "a'b",
        ] {
            for prefix in ["runtime/", "sig/runtime/"] {
                for receiver in ["", "self.", "self?."] {
                    let primary = format!(
                        "# multibyte prefix: é\nmodule ActiveRecord\n  class Base\n\
                         \x20   type payload = Array[Hash[String, Integer?]]\n\
                         \x20   %a{{deprecated}}\n    def {receiver}`{name}`: (\n\
                         \x20     payload body\n    ) -> String\n\
                         \x20   def untouched: () -> Integer\n\
                         \x20   alias `retained alias` untouched\n  end\nend\n"
                    );
                    let mut files = vec![(format!("{prefix}active_record/base.rbs"), primary)];
                    // A declaration needs no competitor to trigger the original panic.
                    let original = files.clone();
                    resolve_runtime_sig_conflicts(&mut files).unwrap();
                    assert_eq!(files, original);

                    let reopen = format!(
                        "class ::ActiveRecord::Base\n  def {receiver}`{name}`: (untyped) -> untyped\nend\n"
                    );
                    files.push((
                        format!("{prefix}active_record/connection.rbs"),
                        reopen.clone(),
                    ));
                    resolve_runtime_sig_conflicts(&mut files).unwrap();
                    assert!(!files[0].1.contains("%a{"));
                    assert!(!files[0].1.contains("-> String"));
                    assert!(
                        files[0]
                            .1
                            .contains("type payload = Array[Hash[String, Integer?]]")
                    );
                    assert!(files[0].1.contains("def untouched: () -> Integer"));
                    assert!(files[0].1.contains("alias `retained alias` untouched"));
                    assert_eq!(files[1].1, reopen);
                    for (_, text) in &files {
                        ruby_rbs::node::parse(text).expect("quoted declarations and aliases parse");
                    }
                    let resolved = files.clone();
                    resolve_runtime_sig_conflicts(&mut files).unwrap();
                    assert_eq!(files, resolved);
                }
            }
        }
    }

    #[test]
    fn spinel_runtime_sig_conflicts_match_quoted_and_unquoted_names() {
        for name in [
            "probe", "[]", "[]=", "+", "ready?", "probe!", "value=", "class",
        ] {
            for receiver in ["", "self.", "self?."] {
                for quoted_primary in [false, true] {
                    let names = if quoted_primary {
                        [format!("`{name}`"), name.to_string()]
                    } else {
                        [name.to_string(), format!("`{name}`")]
                    };
                    let mut files = vec![
                        (
                            "runtime/active_record/base.rbs".into(),
                            format!(
                                "class ActiveRecord::Base\n  def {receiver}{}: () -> String\nend\n",
                                names[0]
                            ),
                        ),
                        (
                            "sig/runtime/active_record/base.rbs".into(),
                            format!(
                                "class ActiveRecord::Base\n  def {receiver}{}: (\n  ) -> String\nend\n",
                                names[1]
                            ),
                        ),
                    ];
                    let original = files.clone();
                    resolve_runtime_sig_conflicts(&mut files).unwrap();
                    assert_eq!(
                        files, original,
                        "equal decoded names and signatures survive"
                    );
                    files[1].1 = files[1].1.replace("String", "Integer");
                    let conflicting = files.clone();
                    assert!(
                        resolve_runtime_sig_conflicts(&mut files)
                            .unwrap_err()
                            .contains(&format!("ActiveRecord::Base#{receiver}{name}"))
                    );
                    assert_eq!(
                        files, conflicting,
                        "undecidable conflicts leave inputs intact"
                    );
                    files[1].0 = "runtime/active_record/connection.rbs".into();
                    resolve_runtime_sig_conflicts(&mut files).unwrap();
                    assert_eq!(files[0].1, "class ActiveRecord::Base\nend\n");
                    assert!(files[1].1.contains("Integer"));
                }
            }
        }
    }

    #[test]
    fn spinel_runtime_sig_conflicts_keep_quoted_names_and_receivers_distinct() {
        // Receiver-like quoted names and distinct receiver kinds must stay independent.
        for (left_receiver, left_name, right_receiver, right_name) in [
            ("", "self.probe", "self.", "probe"),
            ("", "self?.probe", "self?.", "probe"),
            ("self.", "self?.probe", "self?.", "self.probe"),
        ] {
            for scope in ["class", "module"] {
                let text = format!(
                    "{scope} Example\n  def {left_receiver}`{left_name}`: () -> String\n\
                     \x20 def {right_receiver}`{right_name}`: () -> Integer\nend\n"
                );
                let mut files = vec![("runtime/example.rbs".into(), text)];
                let original = files.clone();
                resolve_runtime_sig_conflicts(&mut files).unwrap();
                assert_eq!(files, original);
            }
        }
        // Quoted punctuation and qualified reopens work in nested module/interface scopes too.
        for scope in ["module Helpers", "interface _Helpers"] {
            let name = scope.split_whitespace().nth(1).unwrap();
            let mut files = vec![
                (
                    "runtime/action_view/view_helpers.rbs".into(),
                    format!("module ActionView {scope}\n  def `a:b#c`: () -> String\nend end\n"),
                ),
                (
                    "runtime/hash_to_query.rbs".into(),
                    format!(
                        "{} ::ActionView::{name}\n  def `a:b#c`: () -> Integer\nend\n",
                        scope.split_whitespace().next().unwrap()
                    ),
                ),
            ];
            resolve_runtime_sig_conflicts(&mut files).unwrap();
            assert!(!files[0].1.contains("def "));
            for (_, text) in &files {
                ruby_rbs::node::parse(text).expect("quoted nested declarations parse");
            }
        }
    }

    #[test]
    fn spinel_runtime_sig_conflicts_remove_complete_declarations() {
        let shapes = [
            "(\n      String body,\n      ?status: Symbol\n    ) -> void",
            "() ->\n      (String |\n       Integer)",
            "(Array[Hash[Symbol, String | Integer]] rows)\n      { (String key,\n         Integer value) -> bool } -> void",
            "(String body) -> void\n      | (\n          Integer body\n        ) -> void",
            "(\n      payload body\n    ) -> payload",
            "(\n      \"a # literal\" body # parameter comment\n    ) -> \"two words\"",
            "(String body, *Integer rest, ?flag: bool, **String keywords)\n      ?{ () -> void } -> void",
            "(\n      [String, Integer] row,\n      { status: Symbol, payload: Array[Hash[String, Integer?]] } record\n    ) -> bool",
            "[T] (\n      Array[T] rows\n    ) -> T?",
        ];
        for prefix in ["runtime/", "sig/runtime/"] {
            for receiver in ["", "self.", "self?."] {
                for shape in shapes {
                    for (name, reopen_name) in [
                        ("probe", "probe"),
                        ("`probe`", "probe"),
                        ("probe", "`probe`"),
                        ("`probe`", "`probe`"),
                    ] {
                        for reversed in [false, true] {
                            let primary = format!(
                                "module ActiveRecord\n  class Base\n    type payload = String | Integer\n\
                             \x20   %a{{deprecated}}\n    def {receiver}{name}: {shape}\n\
                             \x20   def untouched: () -> Integer\n\
                             \x20   alias retained untouched\n  end\nend\n"
                            );
                            let reopen = format!(
                                "class ActiveRecord::Base\n  def {receiver}{reopen_name}: (untyped body) -> untyped\nend\n"
                            );
                            let mut files = vec![
                                (format!("{prefix}active_record/base.rbs"), primary),
                                (
                                    format!("{prefix}active_record/connection.rbs"),
                                    reopen.clone(),
                                ),
                            ];
                            if reversed {
                                files.reverse();
                            }
                            resolve_runtime_sig_conflicts(&mut files).unwrap();
                            for (path, text) in &files {
                                ruby_rbs::node::parse(text)
                                    .unwrap_or_else(|e| panic!("{path}: {e}\n{text}"));
                            }
                            let kept = files.iter().find(|(p, _)| p.ends_with("base.rbs")).unwrap();
                            assert!(!kept.1.contains("probe"), "{}", kept.1);
                            assert!(!kept.1.contains("%a{"), "{}", kept.1);
                            assert!(kept.1.contains("type payload = String | Integer"));
                            assert!(kept.1.contains("def untouched: () -> Integer"));
                            assert!(kept.1.contains("alias retained untouched"));
                            assert_eq!(
                                files
                                    .iter()
                                    .find(|(p, _)| p.ends_with("connection.rbs"))
                                    .unwrap()
                                    .1,
                                reopen
                            );
                            let resolved = files.clone();
                            resolve_runtime_sig_conflicts(&mut files).unwrap();
                            assert_eq!(files, resolved, "resolution must be idempotent");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spinel_runtime_sig_conflicts_compare_continuations_and_preserve_members() {
        // Both declarations have the same opening line: the difference is
        // below it. Equal-ranked files must still fail closed.
        for tail in ["String body) -> void", "Integer body) -> String"] {
            let mut files = vec![
                (
                    "runtime/active_record/base.rbs".into(),
                    "class ActiveRecord::Base\n  def probe: (\n    Integer body) -> void\nend\n"
                        .into(),
                ),
                (
                    "sig/runtime/active_record/base.rbs".into(),
                    format!("class ActiveRecord::Base\n  def probe: (\n    {tail}\nend\n"),
                ),
            ];
            assert!(
                resolve_runtime_sig_conflicts(&mut files)
                    .unwrap_err()
                    .contains("ActiveRecord::Base#probe")
            );
        }
        // Equal overload prefixes must not hide a disagreement in a later arm.
        let mut files = vec![
            ("runtime/active_record/base.rbs".into(),
             "class ActiveRecord::Base\n  def probe: () -> void\n    | (String) -> String\nend\n".into()),
            ("sig/runtime/active_record/base.rbs".into(),
             "class ActiveRecord::Base\n  def probe: () -> void\n    | (String) -> Integer\nend\n".into()),
        ];
        assert!(resolve_runtime_sig_conflicts(&mut files).is_err());

        // Equal signatures can differ in layout/comments, including literal '#'.
        let mut files = vec![
            ("runtime/active_record/base.rbs".into(),
             "class ActiveRecord::Base\n  def probe: (String body) -> \"a # b\"\nend\n".into()),
            ("sig/runtime/active_record/base.rbs".into(),
             "class ActiveRecord::Base\n  def probe: (\n    String body # comment\n  ) -> \"a # b\"\nend\n".into()),
        ];
        let original = files.clone();
        resolve_runtime_sig_conflicts(&mut files).unwrap();
        assert_eq!(files, original);

        // Byte spans must preserve neighboring members even without newlines,
        // and a qualified root reopen must match the nested class.
        let mut files = vec![
            ("runtime/active_record/base.rbs".into(),
             "module ActiveRecord\r\n  class Base\r\n    def probe: (\r\n      String body\r\n    ) -> void\r\n    def self.probe: () -> Integer\r\n  end\r\nend".into()),
            ("runtime/active_record/connection.rbs".into(),
             "module Other\n  class ::ActiveRecord::Base def probe: () -> void end\nend\n".into()),
        ];
        resolve_runtime_sig_conflicts(&mut files).unwrap();
        assert!(!files[0].1.contains("String body"));
        assert!(files[0].1.contains("def self.probe: () -> Integer"));
        for (_, text) in &files {
            ruby_rbs::node::parse(text).expect("resolved signatures parse");
        }
    }

    #[test]
    fn spinel_runtime_sig_conflicts_keep_neighboring_scopes_and_same_line_members() {
        for scope in ["module", "interface"] {
            let name = if scope == "interface" { "_Helpers" } else { "Helpers" };
            let mut files = vec![
                ("runtime/action_view/view_helpers.rbs".into(),
                 format!("module ActionView {scope} {name}\n  def probe: (\n    String body\n  ) -> void\n  alias retained untouched\n  def untouched: () -> Integer\nend end\nmodule Other {scope} {name}\n  def probe: () -> String\nend end\n")),
                ("runtime/hash_to_query.rbs".into(),
                 format!("{scope} ActionView::{name}\n  def probe: () -> void\nend\n")),
            ];
            resolve_runtime_sig_conflicts(&mut files).unwrap();
            assert!(!files[0].1.contains("String body"));
            assert!(files[0].1.contains("def untouched: () -> Integer"));
            assert!(files[0].1.contains("alias retained untouched"));
            assert!(files[0].1.contains("def probe: () -> String"));
            for (_, text) in &files {
                ruby_rbs::node::parse(text).expect("module/interface scopes survive");
            }
        }
        for method in ["probe", "`probe`", "[]", "`[]`", "[]=", "+", "`a:b#c`", "`méthode`"] {
            let mut files = vec![
                ("runtime/active_record/base.rbs".into(),
                 format!("class ActiveRecord::Base def {method}: (String) -> void def untouched: () -> Integer end")),
                ("runtime/active_record/connection.rbs".into(),
                 format!("class ActiveRecord::Base def {method}: (untyped) -> void end")),
            ];
            resolve_runtime_sig_conflicts(&mut files).unwrap();
            assert!(!files[0].1.contains("String"));
            assert!(files[0].1.contains("def untouched: () -> Integer"));
            ruby_rbs::node::parse(&files[0].1).expect("same-line neighbors survive");
        }
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
        let rb_paths: crate::det::HashSet<String> = [
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
            manifest.contains("rqrcode = { git = \"https://github.com/roundhouse-rb/spinel-rqrcode\""),
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
            manifest.contains("ruby-vips = { git = \"https://github.com/roundhouse-rb/spinel-ruby-vips\""),
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

    /// `REXML::Document.new(xml)` in an app file: the emit drops the
    /// app's own `require "rexml/document"`, so the pass writes it back,
    /// and the Gemfile names rexml, a bundled gem since Ruby 3.0, which
    /// bundler otherwise keeps off the load path. Without the require,
    /// spinel resolved `REXML::Document` to the Nokogiri facade's
    /// `Document` and `Document.new(xml)` raised ArgumentError.
    #[test]
    fn bundled_requires_write_rexml_document_for_an_app_that_names_rexml() {
        let mut files = vec![
            ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
            (
                "app/models/feed.rb".to_string(),
                "class Feed\n  def titles(xml)\n    REXML::Document.new(xml).get_elements(\"//title\")\n  end\nend\n"
                    .to_string(),
            ),
        ];
        write_bundled_requires(&mut files);
        let get = |p: &str| &files.iter().find(|(q, _)| q == p).unwrap().1;
        let feed = get("app/models/feed.rb");
        assert!(feed.starts_with("require \"rexml/document\"\n"), "{feed}");
        assert!(get("Gemfile").contains("gem \"rexml\"\n"), "{}", get("Gemfile"));
    }

    /// `YAML.safe_load_file` with `rescue Psych::Exception`: Rails has
    /// loaded yaml before the app, so the app writes no require. The pass
    /// writes `require "yaml"` for YAML and `require "psych"` for Psych
    /// (spinel's packages/yaml and packages/psych), and the Gemfile gains
    /// nothing: both are default gems.
    #[test]
    fn bundled_requires_write_yaml_and_psych_for_an_app_that_names_them() {
        let mut files = vec![
            ("Gemfile".to_string(), "source \"https://rubygems.org\"\n".to_string()),
            (
                "app/models/registry.rb".to_string(),
                "class Registry\n  def self.read(path)\n    YAML.safe_load_file(path) || {}\n  rescue Psych::Exception\n    {}\n  end\nend\n"
                    .to_string(),
            ),
        ];
        write_bundled_requires(&mut files);
        let get = |p: &str| &files.iter().find(|(q, _)| q == p).unwrap().1;
        let registry = get("app/models/registry.rb");
        assert!(registry.contains("require \"yaml\"\n"), "{registry}");
        assert!(registry.contains("require \"psych\"\n"), "{registry}");
        assert_eq!(get("Gemfile"), "source \"https://rubygems.org\"\n");
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
            include_spans: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            conditional_definitions: Vec::new(),
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
                "    def rendered_html\n      Views::Layouts::ActionText::Contents.content(render_action_text_content + \"\\n\")\n    end\n"
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
            include_spans: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            conditional_definitions: Vec::new(),
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
        let ruby = ruby_runtime_files(&app, fixture, BuildTarget::Ruby.param_binds_enabled(None)).expect("ruby tree");
        let marked: Vec<&str> = ruby
            .iter()
            .filter(|(_, c)| c.contains("#<SPINEL_SOURCE>"))
            .map(|(p, _)| p.as_str())
            .collect();
        assert!(marked.is_empty(), "ruby tree carries spinel markers: {marked:?}");
    }

    /// DatabaseTasks keeps `Array[HashConfig]` for Roundhouse; Spinel
    /// emit must widen those to `Array[untyped]` or call sites see a
    /// String and refuse `with_temporary_connection`.
    #[test]
    fn spinel_emit_widens_hash_config_arrays_to_untyped() {
        let fixture = Path::new("fixtures/tiny-blog");
        if !fixture.is_dir() {
            eprintln!("skip: fixtures/tiny-blog absent");
            return;
        }
        let app = crate::ingest::ingest_app(fixture).expect("ingest tiny-blog");
        let files = spinel_base_files(&app, fixture).expect("spinel base files");
        let tasks = files
            .iter()
            .find(|(p, _)| p.ends_with("active_record/tasks.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("tasks.rbs in spinel tree");
        assert!(
            !tasks.contains("Array[ActiveRecord::DatabaseConfigurations::HashConfig]"),
            "Spinel must not keep Array[HashConfig]: {tasks}"
        );
        assert!(
            tasks.contains("def self.configs_for: (String env_name) -> Array[untyped]"),
            "Spinel must widen configs_for to Array[untyped]: {tasks}"
        );
        assert!(
            tasks.contains(
                "def self.with_temporary_connection: (ActiveRecord::DatabaseConfigurations::HashConfig db_config)"
            ),
            "Spinel must keep HashConfig on with_temporary_connection: {tasks}"
        );
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
            relation.contains("def initialize: (untyped model, ?bool scoped) -> void"),
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
            shared.contains("def initialize: (Base model, ?bool scoped) -> void"),
            "shared relation.rbs must keep Base for Bar A/B"
        );
        // CRuby/JRuby share `spinel_files` but must NOT get the rewrite.
        let ruby = ruby_runtime_files(&app, fixture, BuildTarget::Ruby.param_binds_enabled(None)).expect("ruby runtime files");
        let ruby_relation = ruby
            .iter()
            .find(|(p, _)| p.ends_with("active_record/relation.rbs"))
            .map(|(_, c)| c.as_str())
            .expect("relation.rbs in ruby tree");
        assert!(
            ruby_relation.contains("def initialize: (Base model, ?bool scoped) -> void"),
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

#[cfg(test)]
mod param_binds_tests {
    use super::{BuildTarget, ParamBindCapability};

    #[test]
    fn every_target_has_an_explicit_bind_capability() {
        use BuildTarget::*;
        use ParamBindCapability::*;
        let cases = [
            (Blog, Unsupported), (Spinel, DefaultOn), (Ruby, OptIn), (Jruby, OptIn),
            (Roda, Unsupported), (Futamura, Unsupported), (Crystal, Unsupported), (Elixir, Unsupported),
            (Go, Unsupported), (Kotlin, Unsupported), (Python, Unsupported),
            (Rust, Unsupported), (Swift, Unsupported), (CSharp, Unsupported),
            (Typescript, Unsupported), (TypescriptWorker, Unsupported),
        ];
        assert_eq!(cases.len(), BuildTarget::TRANSPILE.len() + 1);
        for (target, capability) in cases {
            assert_eq!(target.param_bind_capability(), capability, "{target:?}");
            for value in [None, Some(""), Some("unexpected")] {
                assert_eq!(target.param_binds_enabled(value), capability == DefaultOn,
                    "{target:?}: {value:?}");
            }
            assert_eq!(target.param_binds_enabled(Some("1")), capability != Unsupported,
                "{target:?}: =1");
            assert!(!target.param_binds_enabled(Some("0")), "{target:?}");
        }
    }
}
