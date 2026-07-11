> **COMPLETED — historical design record.** The work described in this plan has shipped; see [README](../README.md) for the current state of these targets. Retained for the design rationale that source comments still reference — this is not a live roadmap.

# C# / .NET target migration plan

Roundhouse is lowerer-first: the Rails DSL is lowered into a universal post-lowering IR
(`LibraryClass` + explicit `MethodDef` bodies with no Rails DSL surface), then each
per-target emitter is a pure Ruby→target renderer (`src/emit/mod.rs`: "Each emitter takes an
`&App` and produces a set of files… Emitters are pure: no I/O"). Adding C# is therefore
mostly an emitter + runtime-wiring exercise, not a re-analysis. This plan covers **backend
only**: models, controllers, validations, callbacks, the transpiled framework runtime
(`runtime/ruby/*`), and HTML-string views — the surface TypeScript/Go/Crystal/Kotlin/Swift
already cover.

**The Kotlin emitter is the primary template.** C# and Kotlin share a profile almost
exactly: nominal, GC'd, statically-typed OO with generics and declared nullability, plus a
gradual escape hatch. C# is a **soft** strict target like Kotlin — `Ty::Untyped` maps to
`object?` rather than forcing a type commit or an emit-time error (cf. the `Ty::Untyped` doc
in `src/ty.rs`). Crucially, C# needs **none** of Rust's ownership/lifetime coloring (the
hardest part of `rust2`), and it has native `async`/`await`, value tuples, and nullable
reference types that line up cleanly with the inference IR. Net: comparable effort to the
Kotlin/Swift targets, below Rust/Spinel.

## Decisions locked in

1. **HTTP server: ASP.NET Core / Kestrel** (via `Microsoft.NET.Sdk.Web`). The hand-written
   per-target primitive runtime supplies a Kestrel adapter (`Server.Start`) that parses
   request → dispatches through the transpiled `ActionDispatch::Router.match` → instantiates
   a controller → calls `process_action` → formats response. Mirrors
   `runtime/kotlin/server.kt`, `runtime/crystal/server.cr`, `runtime/go/v2/server.go`.
2. **Build/packaging: the .NET SDK.** A single `roundhouse-app.csproj` replaces Kotlin's
   `build.gradle.kts` pair (`src/emit/csharp/package.rs`). `TargetFramework=net10.0`.
3. **DB driver: `Microsoft.Data.Sqlite`** (ADO.NET). The hand-written `Db` primitive (analog
   of `runtime/kotlin/db.kt`, `runtime/typescript/db.ts`, `runtime/go/v2/db.go`) wraps
   `SqliteConnection`/`SqliteCommand`. Synchronous prepared statements first.
4. **JSON: System.Text.Json**; **WebSockets (Action Cable): built-in ASP.NET Core
   WebSockets** — no extra NuGet packages beyond `Microsoft.Data.Sqlite`.
5. **Naming: PascalCase members, `Roundhouse` namespace** (`src/emit/csharp/naming.rs`).
   Flat namespace like Kotlin's flat `roundhouse` package, so `Base`-suffixed framework
   classes concatenate segments (`ActiveRecordBase`).
6. **Validation gate order:** `dotnet build` compiles clean → real-blog 0 emit diagnostics →
   `scripts/compare csharp` passes 5/5 vs Rails → (later) a `scripts/bench` cell. **Bench is
   not part of the initial gate.**
7. **NativeAOT is a later lever, not the initial path.** JIT first (simplest `dotnet run`);
   `<PublishAot>` is a deployment-profile refinement once correctness gates pass — it is the
   single biggest knob on the memory/efficiency line (the reason C# is expected to beat the
   JVM targets on `req/sec/GB`).

**Scope boundary (locked):** BACKEND-ONLY first. HTML views are emitted as
**string-concatenation render functions** (the path TS/Go/Crystal/Kotlin use — views flow
through `lower_views_to_library_classes` / `lower_jbuilder_to_library_classes`). Rails-views
→ native UI (MAUI/Blazor) is **explicitly deferred** and out of scope here.

## Ty → C# mapping table

Grounded in the real `Ty` enum in `src/ty.rs`. Implemented in `src/emit/csharp/ty.rs`,
mirroring `src/emit/kotlin/ty.rs` with C# spellings.

| `Ty` variant | C# type | Notes / precedent |
|---|---|---|
| `Int` | `long` | Rails IDs are 64-bit on sqlite; matches Kotlin's `Long`. |
| `Float` | `double` | |
| `Bool` | `bool` | |
| `Str` | `string` | |
| `Sym` | `string` | C# has no symbol; route symbols to string keys. |
| `Nil` | `void` (return slot) / `T?` (value slot via union) | A `csharp_return_ty` helper refines the outermost slot in Phase 2, like Kotlin/TS. |
| `Bottom` | `object?` | C# has no bottom type (`!`/`Nothing`); `throw` is convertible to any type, so the slot rarely renders concretely. |
| `Array{elem}` | `List<T>` | Mutable default (AR result sets / view accumulators); tighten to `IReadOnlyList<T>` later. |
| `Hash{k,v}` | `Dictionary<K,V>` | Mutable default. |
| `Tuple{elems}` | `(T1, T2, …)` | Native value tuples of any arity — no Pair/Triple ceiling (the one place C# is *less* awkward than Kotlin). |
| `Union{variants}` | `T?` for `T \| Nil`; else `object?` | Heterogeneous unions → a generated sealed record hierarchy is a Phase 2+ refinement. |
| `Class{id,args}` | `Name<args>` | Last-segment naming; `Date`/`Time`→`string`, `Regexp`→`Regex`, `Hash`→`Dictionary`. |
| `Record{…}` | `Dictionary<string, object?>` | No anonymous record; matches the lowerer's emitted dictionary. |
| `Fn{params,ret}` | `Action<…>` / `Func<…, R>` | `Action` when the result is `void`/`Nil`, `Func` otherwise. |
| `Var`, `Untyped` | `object?` | The soft-strict escape — no emit diagnostic. |

## Phases

- **Phase 1 — scaffold (done).** `emit` produces the .NET project scaffold
  (`roundhouse-app.csproj`, `Program.cs`, `.gitignore`) via `package::scaffold`; `ty` and
  `naming` complete; `expr`/`library`/`primitives` are documented stubs. `--target csharp`
  works and registers in `BuildTarget` (`ALL` + `TRANSPILE`). The placeholder `Program.cs`
  boots a Kestrel app serving a single route.
- **Phase 2 — models → `dotnet build` clean (done).** `csharp/expr.rs` (statement-aware:
  `emit_stmt`/`emit_expr`, `switch` expr/stmt, `??`/`!`, collection literals, indexers, casts)
  + `csharp/library.rs` (constructor from `initialize`, `[]`/`[]=` → one indexer,
  `companion` → `static`, auto-properties, target-typed `new()` returns) render the lowered
  models (`Article`, `Comment`, `ApplicationRecord`, `<Model>Row`/`<Model>Params`) to
  `app/models/*.cs`. Hand-written `runtime/csharp/` primitives (`ActiveRecordBase`, `Db`,
  `Time`, `Broadcasts`, `Errors`, `RhRuntime`) + Phase-2 view stubs let the whole layer
  `dotnet build` clean (0 errors/warnings). `Db` is a no-op stub; persistence is Phase 3.
- **Phase 3 — controllers + transpiled framework runtime.** Wire a `csharp_units` into
  `runtime_loader.rs` (mirror `kotlin_units` + `KOTLIN_TARGET`/`KOTLIN_RUNTIME`), grow the
  runtime one file at a time (inflector → json_builder → router → errors → base → view_helpers
  → action_controller/base), emit controllers, add `Server`/adapter primitives.
- **Phase 4 — views + e2e.** HTML-string + Jbuilder views; `Broadcasts`/`Cable`
  (built-in WebSockets); add `csharp` to `ships_e2e` + `ensure_e2e` boot command
  (`dotnet roundhouse-app.dll` or `./bin/.../roundhouse-app`); `scripts/compare csharp`,
  `scripts/e2e csharp`, README `## End-to-end` block.
- **Later — bench + NativeAOT.** `scripts/bench` cell; `<PublishAot>` deployment profile for
  the low-memory/high-efficiency story.

## Toolchain

`dotnet` is pinned per-host, not in-repo (no committed `mise.toml`). On the bench host and
locally: `mise use dotnet@10` (or `brew install dotnet`). `scripts/bench-env` probes
`dotnet --version` so `env.json` records the SDK version when a bench cell runs.

## Out of scope / future

- Rails-views → native UI (MAUI desktop/mobile, Blazor web).
- EF Core (the hand-written ADO.NET `Db` primitive is the deliberate two-layer-runtime
  choice; EF Core would re-introduce an ORM the lowered IR already replaces).
- The full-stack target-pair story (#31) — a C# client peer is a separate question.
