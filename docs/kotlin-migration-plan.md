> **COMPLETED — historical design record.** The work described in this plan has shipped; see [README](../README.md) for the current state of these targets. Retained for the design rationale that source comments still reference — this is not a live roadmap.

# Kotlin target migration plan

Roundhouse is lowerer-first: the Rails DSL is lowered into a universal post-lowering IR
(`LibraryClass` + explicit `MethodDef` bodies with no Rails DSL surface — the shape
`runtime/spinel/` and the emitters consume), then each per-target emitter is a pure
Ruby→target renderer (`src/emit/mod.rs:1-9`: "Each emitter takes an `&App` and produces a
set of files… Emitters are pure: no I/O"). Adding Kotlin is therefore mostly an emitter +
runtime wiring exercise, not a re-analysis. This plan covers **backend only**: models,
controllers, validations, callbacks, the transpiled framework runtime (`runtime/ruby/*`),
and HTML-string views — exactly the surface TypeScript/Go/Crystal already cover.

The TypeScript emitter is the primary template because Kotlin shares its profile (modern OO,
generics, declared nullability, gradual escape hatch). Kotlin differs from TS in being
*statically typed*, but it is a **softer** strict target than Rust/Crystal: `Ty::Untyped`
maps to `Any?` rather than forcing a type commit or an emit-time error, so the gradual
escape survives emission (cf. the `Ty::Untyped` doc in `src/ty.rs` distinguishing targets
that "admit a gradual escape hatch" from "strict targets … expected to elevate any reachable
`Untyped` to an emit-time error").

## Decisions locked in

1. **HTTP server: Javalin.** Thin, synchronous, Go-`net/http`-shaped. The hand-written
   per-target primitive runtime supplies a Javalin adapter (`Server.start`) that parses
   request → dispatches through the transpiled `ActionDispatch::Router.match` → instantiates
   a controller → calls `process_action` → formats response. Mirrors
   `runtime/crystal/server.cr:1-20` and `runtime/go/v2/server.go`.
2. **Build/packaging: Gradle.** Replaces TS's `package.json`/`tsconfig.json` pair
   (`src/emit/typescript/package.rs`) with `build.gradle.kts` + `settings.gradle.kts`.
3. **DB driver: JDBC SQLite (xerial `sqlite-jdbc`).** The hand-written `Db` primitive
   (analog of `runtime/typescript/db.ts`, `runtime/crystal/db.cr`, `runtime/go/v2/db.go`)
   wraps `org.sqlite.JDBC`. Synchronous prepared statements — no coroutines.
4. **Validation gate order:** `kotlinc` compiles clean → real-blog 0 emit diagnostics →
   `scripts/compare kotlin` passes 5/5 vs Rails → (later) a `scripts/bench` cell.
   **Bench is not part of the initial gate.**

**Scope boundary (locked):** BACKEND-ONLY first. HTML views are emitted as
**string-concatenation render functions** (the same path TS/Go/Crystal use — views flow
through `lower_views_to_library_classes` / `lower_jbuilder_to_library_classes`,
`src/emit/typescript.rs:225-266`). Rails-views → Jetpack-Compose native UI is **explicitly
deferred** to a separate `view_to_native_ui` lowerer category and is out of scope here
(see "Out of scope / future").

## Ty → Kotlin mapping table

Grounded in the real `Ty` enum in **`src/ty.rs`** (not `dialect.rs`). The complete variant
set is `Int, Float, Bool, Str, Sym, Nil, Array{elem}, Hash{key,value}, Tuple{elems},
Record{row}, Union{variants}, Class{id,args}, Fn{params,block,ret,effects}, Var{var},
Untyped, Bottom`. The Kotlin renderer (`src/emit/kotlin/ty.rs`) mirrors the structure of
`src/emit/crystal/ty.rs` (the closest strict analog) but softens the fallbacks toward `Any?`
like `src/emit/typescript/ty.rs`.

| `Ty` variant | Kotlin type | Notes / precedent |
|---|---|---|
| `Int` | `Long` | Rails IDs are 64-bit on sqlite; matches Crystal's `Int64` choice (`crystal/ty.rs`). `Int` reserved only where IR proves 32-bit. |
| `Float` | `Double` | Mirrors Crystal `Float64`. |
| `Bool` | `Boolean` | |
| `Str` | `String` | |
| `Sym` | `String` | Kotlin has no symbol; TS/Crystal route symbols to string keys. |
| `Nil` | `Unit` (return slot) / `Nothing?` (value slot) | Mirrors TS `ts_return_ty` mapping bare `Ty::Nil`→`void` at the outermost level only (`typescript/ty.rs:17-32`); a `kotlin_return_ty` helper applies the same outermost-only rule. |
| `Array{elem}` | `MutableList<T>` (default) / `List<T>` (read-only positions) | Use `MutableList` by default because AR result sets and `@slots` accumulate; tighten to `List` where `mutates_self` analysis proves no mutation. |
| `Hash{key,value}` | `MutableMap<K, V>` | Cf. Crystal `Hash(K,V)`. Params/HWIA shape is `MutableMap<String, ParamValue>`. |
| `Tuple{elems}` | `Pair`/`Triple` (2-3) else a generated `data class` | Crystal uses `Tuple(...)`; Kotlin lacks N-tuples, so >3 elems need a data class or `List<Any?>`. Flag as a risk. |
| `Record{row}` | generated `data class` with typed fields | Crystal renders `NamedTuple(k: T, ...)` for `Router.match`'s typed return; Kotlin needs a named `data class` (e.g. `RouteMatch`). |
| `Union{variants}` | `T?` when `{T, Nil}`; else sealed type or `Any?` | Special-case `T \| Nil` → `T?` exactly like `crystal/ty.rs::render_union`. Non-nil unions (rare in this IR) degrade to `Any?` initially. |
| `Class{id,args}` | last-segment class name, generic args in `<>` | Same last-segment rule as `ts_class_ty`. Temporal classes (`Date`/`Time`/`DateTime`/`ActiveSupport::TimeWithZone`) → `String` (matches `class_is_temporal` in `typescript/ty.rs`). `Regexp`→`Regex`, `Hash`→`MutableMap<String, Any?>`. |
| `Fn{params,ret,..}` | `(P1, P2) -> R` | Function type; blocks become trailing lambdas. |
| `Var{var}` | `Any?` | Inference gap → permissive (softer than Crystal's `String` fallback). |
| `Untyped` | `Any?` | **The soft-strict decision.** TS maps to `any`; Kotlin maps to `Any?` and *does not* raise an emit diagnostic (unlike Rust/Go which "elevate any reachable `Untyped` to an emit-time error", `src/ty.rs`). |
| `Bottom` | `Nothing` | Direct analog of TS `never` / Crystal `NoReturn` / Rust `!` (`src/ty.rs` Bottom doc). Used in union-filtering so `if c then raise else x` types as `typeof(x)`. |

A `has_untyped(&Ty)` helper (cf. `crystal/ty.rs:has_untyped`) decides whether to emit a
signature annotation or rely on Kotlin inference for locals.

## Phase 0 audit

**Read before writing any Kotlin code** (the template surface, in this order):

- `src/emit/typescript.rs` (≈108 KB, 2,400+ lines) — the emit entry. Key spans: `emit()` at
  `:146`, the runtime-file push block `:149-212`, the lowering pipeline `:218-323` (views
  lowered twice, models, jbuilder, controllers, fixtures, tests), the `typescript_units`
  consumption `:661-707`, and per-artifact output paths `:765-879`. This is the orchestration
  Kotlin's `emit()` clones.
- `src/emit/typescript/ty.rs` (3.2 KB) — the per-target Ty renderer template (above).
- `src/emit/typescript/expr.rs` (≈141 KB) — `Expr` → target syntax. The largest and hardest
  port; the bulk of Kotlin emitter work lives in its analog.
- `src/emit/typescript/library.rs` (≈78 KB) — the kind-agnostic `LibraryClass` walker
  (`emit_class_file`, `emit_function_file`, `extras_from_lcs`). Kotlin needs the same three
  entry points.
- `src/emit/typescript/package.rs` (4.3 KB) — ecosystem-file emit; Kotlin's analog emits
  Gradle files.
- `src/emit/typescript/naming.rs` (1.3 KB) — identifier mangling.
- **Cross-check against `src/emit/crystal/`** (`ty.rs`, `library.rs`, `expr.rs`, `method.rs`,
  `shared.rs`) for the *static-typed* idioms TS doesn't exercise: nullable shorthand,
  explicit signatures, `NamedTuple`/`data class` shapes.

**Wiring points enumerated — every file that must gain a `kotlin`/`Kotlin` arm:**

1. `src/emit/mod.rs:13` — add `pub mod kotlin;` (and the `kotlin/` directory + `kotlin.rs`).
2. `src/project.rs` — `enum BuildTarget` (`:40-62`) add `Kotlin`; `ALL` (`:67-79`);
   `TRANSPILE` (`:85-96`); `as_str` (`:100-114`) → `"kotlin"`; `target_readme` match
   (`:139-266`); `target_files` dispatch (`:293-308`) →
   `BuildTarget::Kotlin => Ok(sort_files(emit::kotlin::emit(app)))`.
3. `src/runtime_loader.rs` — add `const KOTLIN_TARGET: TargetEmit` (cf. `CRYSTAL_TARGET`
   `:105-113`), `const KOTLIN_RUNTIME: &[RuntimeEntry]` (cf. `CRYSTAL_RUNTIME` `:430`), and
   `pub fn kotlin_units<F>(…)` (cf. `crystal_units` `:543-555`, calling
   `transpile_entry(entry, &KOTLIN_TARGET, "//", …)`). Plus the Kotlin `format_import` /
   `format_constant` / `wrap_namespace` helpers.
4. `src/emit/diagnostics.rs` — `StubStyle` enum (`:88-100`) add a `KotlinThrow` variant
   (`throw RuntimeException("…")` in expression position via `run { throw … }`), `for_target`
   arm (`:105-114`) for `"kotlin"`, and `render` arm (`:120`). Note: unknown targets fall
   back to `Raise`, so this is correctness polish, not a blocker.
5. `bin/rh` — `TARGETS` array (`:20`), the `unlocks`/doctor toolchain entry (`:158-159`, add
   a JDK/Gradle check), and the transpile/compare command help.
6. `src/bin/roundhouse.rs` — `--target` help text only (`:39-55`); dispatch already routes
   through `BuildTarget::from_str` (`:181-189`), so no new arm needed once `project.rs` is
   updated.
7. `scripts/compare` — target normalization `case` (`:106-118`), the JSON-path target list
   (`:142-146`), the `BUILD_DIR`/`DB_DEST` case (`:166-215`), the transpile/build case
   (`:335-377`), and the start case (`:444-492`).
8. `scripts/bench` — usage block, `case` normalization (`:123-136`), and the `*_OUT`
   build-dir wiring (`:147-174`). **Deferred to Phase 7.**

## Phased build plan

The pipeline-first architecture means lowering is already done; phases track emitter +
runtime maturity. Structure mirrors `docs/rust-migration-plan.md` (Phase 0 audit →
file-by-file runtime transpile → strict-target inheritance spike → verification gates) and
`docs/jbuilder-lowerer-plan.md` (reference-fixture lock-in first).

**Phase R (reference-output lock-in) — forcing function, do first.**
Before touching the emitter, hand-write a minimal Kotlin reference of real-blog covering one
of each artifact class, by hand-running the existing lowering and reading `bin/dump_ir`:
- one model: `fixtures/real-blog/app/models/article.rb` → `Article.kt` (data-class row + AR
  methods).
- one controller: `fixtures/real-blog/app/controllers/articles_controller.rb` →
  `ArticlesController.kt`.
- one HTML view: `fixtures/real-blog/app/views/articles/index.html.erb` → an
  `Articles.index()` render function returning a `String`.
- one framework-runtime file: `runtime/ruby/inflector.rb` → `Inflector.kt` (the smallest
  `Mode::Module` entry, first in every target's RUNTIME table).
- the hand-written primitives: `Db.kt` (xerial JDBC), `Server.kt` (Javalin), `ParamValue.kt`,
  `build.gradle.kts`.

**Gate:** the hand-written reference compiles with `kotlinc`/Gradle and serves `/articles`
against a staged sqlite. This is the spec the emitter must reproduce.

**Phase 1 — emitter skeleton + target registration.**
Create `src/emit/kotlin.rs` + `src/emit/kotlin/{ty.rs, expr.rs, library.rs, package.rs,
naming.rs}`. Wire all the registration points in the Phase 0 list (items 1, 2, 4, 5, 6).
`ty.rs` is complete (the table above); `expr.rs`/`library.rs` start as stubs that emit `TODO`
panics. `package.rs` emits `build.gradle.kts` + `settings.gradle.kts`.

**Gate:** `cargo build` clean; `roundhouse --target kotlin` runs and emits the Gradle scaffold
+ empty `src/`.

**Phase 2 — model emit → kotlinc clean.**
Port enough of `kotlin/expr.rs` + `kotlin/library.rs` (from the TS analogs, hardened with
Crystal's static-typed idioms) to emit the model `LibraryClass`es. Decide the **AR base-class
inheritance shape** here (the Kotlin analog of rust-migration Phase 1.5): Kotlin `open class` +
`abstract`/default methods (Kotlin has interface default methods and `open` classes — no
proc-macro problem Rust faced, so this is lower-risk). Leverage the `mutates_self` IR flag
(`src/dialect.rs:368-379`, computed by `src/analyze/mutates_self.rs`, which already names
"Kotlin/Swift") to pick `val`/`var` and `List`/`MutableList`.

**Gate:** emitted `app/models/*.kt` + transpiled `Inflector.kt` compile with `kotlinc`
standalone.

**Phase 3 — controllers + framework-runtime transpile.**
Populate `KOTLIN_RUNTIME` file-by-file in the dependency order Crystal/Rust use
(inflector → json_builder → action_dispatch/router (+ flash, session) →
action_view/view_helpers → active_record/base (+ errors) → action_controller/base). Wire
`kotlin_units` into `kotlin::emit()` exactly as `typescript_units` is consumed at
`src/emit/typescript.rs:661-707` (with `treeshake::filter_runtime_class`). Emit controllers
via the same `lower_controllers_with_arel_views_and_assocs` path. **Avoid coroutines
entirely** — never set `is_async` (the default `node-sync`-equivalent profile keeps the async
seed list empty, so `is_async` stays false everywhere, `src/dialect.rs:362-367`); Javalin's
synchronous handlers match.

**Gate:** controllers + all `KOTLIN_RUNTIME` files compile with the hand-written
`Server.kt`/`Db.kt` primitives.

**Phase 4 — HTML views as strings.**
Confirm the view lowerers (`lower_views_to_library_classes`,
`lower_jbuilder_to_library_classes`) produce render functions whose bodies are pure string
concatenation, and that `kotlin/expr.rs` renders Ruby string-building (`<<`, interpolation,
`html_safe`) into Kotlin string templates. No new lowerer — reuse the TS/Go/Crystal path.

**Gate:** `articles/index`, `show`, `_form`, jbuilder `index.json`/`show.json` render
functions compile.

**Phase 5 — real-blog 0 errors.**
Run `roundhouse --target kotlin -o build/transpiled-blog-kotlin fixtures/real-blog`, then
Gradle-compile the whole tree. Drive `Ty::Untyped`→`Any?` to absorb residual gradual
positions without emit diagnostics. Use `roundhouse::emit::diagnostics::scope` (already
wrapped at `src/bin/roundhouse.rs:217`) to assert **0 unsupported-construct diagnostics**.

**Gate:** full real-blog emit compiles via Gradle with 0 roundhouse diagnostics and 0
`kotlinc` errors.

**Phase 6 — compare 5/5 vs Rails.**
Wire `scripts/compare` (Phase 0 item 7): `BUILD_DIR=/tmp/rh-kt-pass2`,
`DB_DEST=$BUILD_DIR/storage/development.sqlite3` (JVM convention), transpile via
`cargo run --bin emit_preview -- --target kotlin "$FIXTURE"`, build via
`./gradlew installDist` (or `shadowJar`), start via the resulting launcher script on `$PORT`.
Add `kotlin` to the JSON-path target set (`:142-146`) so `/articles/1.json` is exercised.

**Gate:** `scripts/compare kotlin` passes all 5 HTTP comparisons against Rails.

**Phase 7 — bench cell (post-gate, not blocking).**
Add `kotlin` to `scripts/bench` (`:123-136`, `*_OUT` wiring). Surface the JVM warmup caveat
in the report (cold-start RSS and first-request latency are inherently worse than AOT targets
— Go/Rust/Crystal).

**Gate:** bench produces throughput + RSS numbers; no parity assertion.

## Runtime stack

Two layers, matching every existing target:

- **Hand-written primitive runtime** (`runtime/kotlin/`, inlined via `include_str!` in
  `src/emit/kotlin.rs` exactly as TS does at `typescript.rs:23-39` and Go does at
  `go2.rs:42-62`):
  - `Server.kt` — Javalin listener; parse request → `Router.match` → controller
    `process_action` → format response (port of `runtime/crystal/server.cr:1-20`).
  - `Db.kt` — xerial `sqlite-jdbc` wrapper: `exec`/`prepare`/query, synchronous (port of
    `runtime/typescript/db.ts` / `runtime/go/v2/db.go`).
  - `ParamValue.kt` — the recursive `String | Map | List` params type (port of
    `runtime/typescript/param_value.ts` / `runtime/crystal/param_value.cr`).
  - `Broadcasts.kt` (stub initially; Turbo Streams parity is later).
  - `build.gradle.kts` + `settings.gradle.kts` (the Gradle analog of `package.rs`).
- **Transpiled framework runtime** (`KOTLIN_RUNTIME` table in `src/runtime_loader.rs`,
  generated from `runtime/ruby/*.rb` + `.rbs`): `inflector`, `json_builder`,
  `action_dispatch/router` (+ `flash`, `session`), `action_view/view_helpers`,
  `active_record/base` (+ `errors`), `action_controller/base`. Same source set as
  `CRYSTAL_RUNTIME` (`src/runtime_loader.rs:430-540`). The Ruby source is the single source
  of truth — no checked-in Kotlin artifact to drift (`runtime_loader.rs:1-13`).

## Risk callouts (priority order)

1. **Nullability mapping is the load-bearing decision.** Kotlin's null-safety is enforced at
   compile time, unlike TS `any`. The `Ty::Untyped → Any?` soft choice is what keeps this a
   soft-strict port; if too many real positions are `Untyped`, the emitted code becomes
   `Any?`-soup that needs casts at every use site. Mitigation: lean on the `T | Nil → T?`
   union special-case and the model row data-class typed fields
   (`src/lower/model_to_library/row.rs:166`, which already names "future Kotlin/Swift" needing
   an explicit `<Row>.new()`). Audit `Untyped` density on real-blog early in Phase 2.
2. **JDBC SQLite performance / connection model.** xerial is synchronous and not thread-safe
   per connection; Javalin is multi-threaded by default. Need a connection-per-request or
   pooled strategy in `Db.kt`. Bench (Phase 7) will expose it but it must be correct in
   Phase 6.
3. **Gradle in the bench/compare harness.** Gradle's daemon + first-build cost is far higher
   than `go build`/`cargo build`. `scripts/compare`'s `wait_for_port` budget (`:231-246`,
   default 30s) likely needs raising for the JVM, like the existing JRuby note (`:234-235`).
   Use `installDist` for a fast launcher rather than `gradle run` per start.
4. **Async/coroutines avoidance.** Locked: never color methods `is_async`. The default profile
   keeps the async seed empty (`src/dialect.rs:362-367`), so propagation is a no-op and
   Javalin's synchronous handlers suffice. Do **not** import the libsql/worker complexity TS
   carries (`typescript.rs:55-97`).
5. **View rendering parity.** HTML-string views must match Rails byte-for-byte enough to pass
   `compare`. Risk is string-template escaping (`html_safe`, ERB `<%= %>` vs `<%== %>`).
   Mirror the jbuilder string-concatenation discipline (`docs/jbuilder-lowerer-plan.md`
   Phase 6 risk).
6. **Tuple/Record → data class.** Kotlin lacks N-tuples; `Router.match`'s typed return and any
   `Ty::Tuple{elems}` with >3 elements need generated `data class`es. Crystal gets
   `NamedTuple` for free; Kotlin needs codegen.

## Open decision points (deferred to mid-stream)

- **End of Phase 2:** lock the AR base inheritance shape in writing (open class vs
  interface-with-defaults vs composition) before Phase 3 transpiles `active_record/base.rb` —
  same gating discipline as rust-migration Phase 1.5.
- **End of Phase 2:** decide `Int` vs `Long` policy where the IR can't prove width (default
  `Long`; revisit if interop with Javalin/JDBC `int` APIs forces casts).
- **End of Phase 3:** `Db.kt` connection strategy (per-request vs pool) — correctness-gated at
  Phase 6, perf-gated at Phase 7.
- **End of Phase 4:** confirm whether any Kotlin string-template idiom needs a new
  `kotlin/expr.rs` lowering hook vs reusing the shared path.
- **After Phase 6:** whether to add `MutableList`→`List` tightening driven by `mutates_self`,
  or leave everything `Mutable*` for simplicity.

## Out of scope / future

- **Rails views → Jetpack Compose** (`view_to_native_ui` lowerer category). This is a
  fundamentally different lowering — Rails ERB/Turbo → native Android UI tree — not a
  string-emit. It is a separate project gated on this backend landing first. The IR hooks that
  already name "future Kotlin/Swift" (`src/dialect.rs:374`, `src/analyze/mutates_self.rs:6`,
  `src/lower/model_associations.rs:8`, `src/lower/model_to_library/row.rs:166`,
  `src/emit/go2/lower.rs:41`) classify Kotlin as a **strict target with mutating-flag
  conventions**, which the backend emitter leverages; the native-UI work would add new
  lowerer-side categories, not emitter arms.
- **Swift sibling.** The same hooks pre-classify Swift alongside Kotlin. A Swift target is the
  natural follow-on (shared optionals/`mutates_self`/strict-but-soft profile) but is not in
  this plan.
- **Turbo Streams / ActionCable parity** beyond a `Broadcasts.kt` stub.
- **Coroutine-based async / worker deployment profiles** (the TS libsql/SharedWorker axes) —
  deliberately not ported.
