# Swift target migration plan

Roundhouse is lowerer-first: the Rails DSL is lowered to a universal post-lowering IR
(`LibraryClass` + explicit `MethodDef` bodies, no Rails surface), then each per-target
emitter is a pure Ruby→target renderer (`src/emit/mod.rs`). Adding Swift is an emitter +
runtime-wiring exercise, not a re-analysis — and it is the cheapest target we can add,
because **Kotlin is a near-exact template**. This plan covers **backend only**: models,
controllers, validations, callbacks, the transpiled framework runtime (`runtime/ruby/*`),
and HTML-string views — the same surface TS/Go/Crystal/Kotlin cover. Rails-views →
**SwiftUI** native UI is the deferred sub-target B (the `view_to_native_ui` lowerer
category, exact analog of the deferred Jetpack-Compose work) and is out of scope here.

**The Kotlin emitter (`src/emit/kotlin/`) is the primary template, not TypeScript.** Swift
and Kotlin share almost the same profile — modern OO, classes with single inheritance,
generics, value-vs-reference (Swift adds structs), `Optional<T>`/`T?` nullability, and a
**soft** strict-typing posture: `Ty::Untyped → Any?` admits the gradual escape rather than
forcing an emit-time error (cf. `src/ty.rs`). Everything we learned closing Kotlin from
"no controllers" to "compare 7/7 → benchmarked → concurrency-safe → CI" transfers; this
plan is organized around **what copies from Kotlin** and **the handful of places Swift
genuinely differs**.

## Why Swift specifically (strategic)

Same unique market as Kotlin — Rails has never reached native mobile, and Swift is the iOS
side of that (`project_kotlin_swift_target_path`). But Swift has a property Kotlin lacks:
it compiles to a **native AOT binary**, so it should land in the **rust/crystal/go memory
tier (tens of MB RSS), not the JVM's ~500 MB**. Kotlin's bench told this story precisely —
field-leading throughput, but its cost-efficiency (req/sec/GB) was dragged down by the JVM
tax. Swift plausibly gives **both** the mobile-deployment story *and* native memory
efficiency. That makes it the most strategically interesting backend target after Kotlin.

## Decisions locked in

1. **HTTP server: Hummingbird 2.x (SwiftNIO), pinned in `Package.swift`.** The thin,
   NIO-based lightweight server — the Swift analog of "Javalin = thin synchronous-shaped
   server." Vapor is too heavy/opinionated; raw NIO is too much boilerplate for the primitive.
   **Bridge detail (load-bearing):** Hummingbird 2 is async/await-first — handlers are
   `async throws`, and awaits can hop executors, which would break every thread-confinement
   assumption below. So the handler does NO async work itself: it immediately wraps the
   *entire* synchronous dispatch (parse request → `Router.match` → instantiate controller →
   `processAction` → format response) in a single `NIOThreadPool.runIfActive { ... }` closure,
   so the whole request runs on one stable pool thread — the Jetty-thread model, restored
   explicitly. Mirrors `Server.kt` + `runtime/crystal/server.cr`.
2. **Build/packaging: Swift Package Manager.** Emit `Package.swift` (replaces Kotlin's
   `build.gradle.kts`/`settings.gradle.kts`). `swift build -c release` → a single binary.
   **Simpler than Gradle**: no daemon, no wrapper, fast cold start, one executable product.
3. **DB driver: the system SQLite3 C API.** No third-party dependency — Swift calls
   `sqlite3_prepare_v2` / `sqlite3_step` / `sqlite3_column_*` / `sqlite3_finalize` directly.
   This maps **exactly** onto the lowered `_adapter_*` model surface (the same
   `prepare`/`step`/`columnInt`/`columnText`/`finalize` shape `Db.kt` wraps), so `Db.swift`
   is a thin port of `Db.kt` over the C API instead of JDBC. **Packaging caveat:**
   `import SQLite3` only exists on Apple platforms (Xcode ships the modulemap). On Linux —
   the CI and bench hosts — `Package.swift` must declare a `systemLibrary` target
   (conventionally `CSQLite`) with a hand-written `module.modulemap` wrapping `<sqlite3.h>`
   and linking `sqlite3`, and the host needs `libsqlite3-dev` installed. ~10 lines of
   packaging, emitted as part of the `package.rs` scaffold; still zero third-party Swift
   dependencies.
4. **Concurrency: thread-confined, per the Kotlin lesson.** The blocking SQLite + synchronous
   render runs on `NIOThreadPool` threads; per-thread connection + statement table + the
   `content_for` slot store via NIO's `ThreadSpecificVariable<T>` (the direct analog of the
   `ThreadLocal` fix that took Kotlin's DB endpoints from 7k→54k). Each request's
   dispatch is synchronous, so it stays on one pool thread — exactly the Jetty-thread model.
5. **Language mode: Swift 5 (strict-concurrency off) initially.** Avoids Swift 6's `Sendable`
   data-race checking blocking on the (deliberately thread-confined) global runtime state;
   revisit once it compiles.
6. **Validation gate order (same as Kotlin):** `swift build` clean → real-blog 0 emit
   diagnostics → `scripts/compare swift` 7/7 → (later) a `scripts/bench` cell. Bench is not
   part of the initial gate.

## Ty → Swift mapping (`src/emit/swift/ty.rs`, port of `kotlin/ty.rs`)

| `Ty` | Swift | Note vs Kotlin |
|---|---|---|
| `Int` | `Int` | Swift `Int` is 64-bit on 64-bit platforms — no `Long`/`L` suffix dance the Kotlin emitter needed. |
| `Float` | `Double` | same |
| `Bool` | `Bool` | same |
| `Str` / `Sym` | `String` | same |
| `Nil` | `Void` (return) / `Optional.none` | same outermost-only rule as `kotlin_return_ty` |
| `Array{T}` | `[T]` | **value type** — copied on assignment (see deltas) |
| `Hash{K,V}` | `[K: V]` | value type |
| `Union{T,Nil}` | `T?` | same nullable shorthand |
| `Class{id}` | last-segment name (`type_name` disambiguation) | reuse the `ActiveRecord::Base`/`ActionController::Base` → `ActiveRecordBase`/`ActionControllerBase` fix verbatim |
| `Record` | a generated `struct` (or `[String: Any]` for loose rows) | Kotlin used `MutableMap<String,Any?>`; Swift can use a real struct |
| `Untyped` / `Var` | `Any?` | the soft-strict escape, no diagnostic |
| `Bottom` | `Never` | direct analog of Kotlin `Nothing` |
| temporal classes | `String` | same `class_is_temporal` rule |

## What copies from Kotlin (the bulk of the work is already done)

`src/emit/swift/` mirrors `src/emit/kotlin/` file-for-file (`ty.rs`, `expr.rs`, `library.rs`,
`naming.rs`, `package.rs`, `primitives.rs`). The **lowerers are shared and unchanged** (Rails
→ IR is target-agnostic), so every lowering interaction we worked out for Kotlin applies. The
specific solved problems that transfer directly:

- **camelCase naming** — Swift convention is camelCase too; `naming::camel` ports as-is
  (keyword set changes: `func`, `class`, `import`, `guard`, `defer`, `where`, …).
- **`type_name` Base disambiguation** — same flat-module collision, same fix.
- **kwargs → named args** — *more* natural in Swift: `truncate(body, length: 100)` is literal
  Swift syntax (named params are the default), so the `METHOD_PARAMS` registry + the
  splat-when-the-callee-matches logic carries over and fits even better.
- **inherited-property / method resolution** (`ancestor_props`, `method_params_for` walking
  the ancestor chain), the **`new` view-method vs constructor** distinction, **StringBuilder
  IrHints** (`var io = ""` / `io += chunk` / `io`), **collection method shims**
  (`.keys`/`.values`/`.count`/`is_a?`), **return-position + constant hash typing**,
  **object body-ivars**, **jbuilder json views merged into the view enum**, **`from_params`
  via `lower_models_with_registry_and_params`**, **`processAction` switch dispatch** — all
  port with syntax substitutions.
- **The phase plan, the wiring-point checklist, the concurrency fix, and the compare/bench/CI
  harness arms** — same shapes (`emit_preview` arm, `scripts/compare`/`scripts/bench` arms,
  `tests/swift_toolchain.rs`, `toolchain-swift`/`compare-swift` jobs).

## Where Swift genuinely differs (the net-new work)

1. **Checked errors / `throws` (the biggest delta).** Kotlin exceptions are unchecked; Swift
   errors are checked. `raise RecordNotFound` → a `throw`, but every function on the path must
   be `throws` and every call `try`. Decision: split the two `raise` flavors —
   *control-flow* raises (`RecordNotFound` → 404, `RecordInvalid`) become real `throws`
   propagated to the server's catch; *"should never happen"* raises (`NotImplementedError` in
   the Base defaults, the dropped adapter path) become `fatalError(...)` (no `throws`
   ripple). The emitter needs a `throws`-propagation pass: mark a `MethodDef` `throws` if its
   body can throw or calls a throwing method; emit `try` at those call sites. This is the one
   piece with no Kotlin analog.
2. **`object` → caseless `enum`.** Swift has no `object` keyword. Singletons/modules
   (`Inflector`, `ViewHelpers`, `RouteHelpers`, `Importmap`, the view namespaces, `Db`,
   `Broadcasts`) become `enum X { static func … }` — the idiomatic Swift namespace. `static`
   members replace Kotlin's object members.
3. **Thread-local → `ThreadSpecificVariable`.** Kotlin's `ThreadLocal` for the DB connection
   and `content_for` slots maps to NIO's `ThreadSpecificVariable<T>`; reads/writes change from
   `.get()`/`.set()` to `.currentValue`. Same `OBJECT_TL_FIELDS` emit machinery, different
   accessor syntax.
4. **Force-unwrap `!` instead of `!!`.** The mutable-optional-property narrowing fix (a
   property proven non-null by an `if let`/nil-guard reads with `!`) is the same `NONNULL_PROPS`
   pass with `!!`→`!`. Swift also offers `guard let`/`if let` binding, which is cleaner — but
   `!` is the minimal port.
5. **String interpolation syntax.** `"${expr}"` → `"\(expr)"`. One change in `emit_string_interp`.
6. **`Any?` → scalar coercion (`emit_cast`) does not port by substitution.** Kotlin's
   column-write Cast path emits `(v).toString().toLong()`; Swift has no universal equivalent.
   The two cases split: when the box holds the target type already (sqlite column reads,
   `from_row`/`assign_from_row` writes), emit a downcast `v as! Int` / `v as! String`; when
   genuinely converting (string → number), emit `Int("\(v)")!` — Swift's `String → Int` is
   failable and must be force-unwrapped. Needs its own design row in `emit_cast`, same tier
   of work as `object`→`enum`; the `INSTANCE_PROP_TYPES` registry driving *where* casts go
   carries over unchanged.
7. **Value vs reference semantics.** Models must be `class` (reference, mutable, inheritance —
   `class Article: ApplicationRecord`). Arrays/dicts are value types (`[T]`, `[K:V]`); a
   `var` stored property holds them fine, but watch any lowered pattern that mutates a hash
   "in place" through a passed reference — verify against real-blog (likely a non-issue; the
   IR threads state through instances).
8. **Linux Foundation gaps.** The bench/CI host is Linux. `Time.kt`'s `OffsetDateTime`/ISO8601
   → Foundation's `ISO8601DateFormatter` (present on Linux) or a hand-rolled formatter; avoid
   Foundation APIs with known Linux divergence. The SQLite3 C API is available via the
   `CSQLite` systemLibrary target (decision 3) — NOT via `import SQLite3`, which is
   Apple-only.
9. **No build wrapper needed** — `swift build` is the toolchain directly (simpler CI than the
   Gradle setup).

## Phases (mirror the Kotlin arc; "[copy]" = template from Kotlin, "[new]" = Swift-specific)

- **R. Hand-written reference** `swift-reference/` — a standalone SPM project that serves
  GET /articles from real-blog's seeded sqlite (Hummingbird + CSQLite + the lowered shapes),
  transcribed from `dump_ir`. The byte-for-byte spec the emitter targets. **Build it on BOTH
  macOS and Linux (`docker run -v … swift:6.x swift build`) before moving to Phase 1** — this
  one cheap step de-risks the three platform deltas at once (CSQLite modulemap, the
  Hummingbird/`runIfActive` bridge, Foundation/ISO8601 divergence), where the Kotlin arc hit
  its mac-vs-host toolchain gotchas late. *[mostly new — the Swift idioms; cheap because the
  lowered IR is the spec]*
- **1. Skeleton + registration** — `src/emit/swift.rs` + `swift/{ty,expr,library,naming,package,primitives}.rs`;
  `BuildTarget::Swift` (TRANSPILE-only, excluded from `ALL` while building, like Kotlin);
  wiring points below. *[copy]*
- **2. Model emit** — `emit_class_file`, accessor→property, companion/static finders, column
  Casts. *[copy + value-type check]*
- **3. Framework runtime transpile** — `SWIFT_TARGET` + `SWIFT_RUNTIME` table in
  `runtime_loader.rs`, grown one file at a time (inflector → json_builder → router → errors →
  base → view_helpers → flash → session → action_controller/base). *[copy + throws pass [new]]*
- **4. Views** — view enums, RouteHelpers/Importmap, StringBuilder, kwargs. *[copy]*
- **5. Controllers + action_controller** — `processAction` switch, render/redirect/head kwargs,
  inherited props, from_params, jbuilder. *[copy]*
- **6. Server.swift (Hummingbird) + main.swift** — routes table + controller factory map +
  layout fn → `Server.start`. *[new server primitive; same shape as Server.kt]*
- **compare** — `scripts/compare swift` arm + `emit_preview` arm → 7/7 vs Rails.
- **concurrency** — `ThreadSpecificVariable` Db + slots (likely needed before bench, per the
  Kotlin lesson — do it proactively this time).
- **bench + CI** — `scripts/bench` arm; `tests/swift_toolchain.rs` (`swift build`);
  `toolchain-swift` + `compare-swift` jobs (`swift-actions/setup-swift`, no Gradle equivalent
  needed; runner needs `libsqlite3-dev`). **Scope: the same 2-of-4 jobs Kotlin landed.**
  framework-tests-swift (forces the generic SqliteAdapter CRUD path, currently an
  unimplemented-stub on Kotlin too) and e2e-swift (needs BuildTarget::ALL promotion + static
  asset serving + archive publication) are deliberately deferred, mirroring Kotlin's status —
  don't let them creep into the initial gate.

## Wiring points (same checklist as Kotlin)

`src/emit/mod.rs` (`pub mod swift`), `src/project.rs` (`BuildTarget::Swift` variant + TRANSPILE
+ `as_str`→`"swift"` + `target_files` arm + `target_readme` arm), `src/runtime_loader.rs`
(`SWIFT_TARGET` + `SWIFT_RUNTIME` + `swift_units` + `swift_format_import`/`_constant`/
`wrap_namespace`), `src/emit/diagnostics.rs` (a `SwiftFatalError` stub style),
`src/bin/{roundhouse,emit_preview}.rs`, `bin/rh` (TARGETS), `scripts/compare`, `scripts/bench`.

## Effort & risks

**Effort: meaningfully less than Kotlin.** The emitter machinery is a template (Kotlin ≈ 60-70%
reusable with syntax substitution), the lowerers are untouched, and the gotchas + phase plan +
harness wiring are all known. The genuinely new work is: the `throws`-propagation pass (the one
real design task), the Hummingbird `Server.swift` primitive (with the `runIfActive`
thread-confinement bridge), the `Db.swift` C-API port (+ CSQLite packaging), the `emit_cast`
redesign for `Any?` coercions, and `object`→`enum` / `!!`→`!` / interpolation substitutions.

**Top risks, ranked:**
1. **`throws` propagation** — the only piece without a Kotlin analog; a body-walk pass marking
   throwing methods + emitting `try`. Mitigation: `fatalError` for the "never happens" raises
   shrinks the throwing surface to just `find`/`find_by`/validation → controller → server.
2. **Linux Foundation** — the date/ISO8601 path; mitigate by hand-rolling the formatter if
   `ISO8601DateFormatter` diverges.
3. **Swift 6 Sendable** — sidestep with Swift 5 language mode initially.
4. **Hummingbird API churn** — pin a version in `Package.swift` (the SPM analog of pinning
   Javalin 6.4.0).

**Net:** Swift is the highest-leverage next target — it reuses the Kotlin investment almost
entirely, and it's the one that proves the mobile story *without* the JVM memory tax.
