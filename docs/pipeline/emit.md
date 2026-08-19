# Emit

Each emitter takes the analyzed, lowered IR and produces a complete,
runnable project for one target language. The current architecture
follows a **lower-once, render N-ways** bet: most logic moves into
target-neutral lowerings that produce a universal IR; per-target
emitters render that IR into idiomatic source.

**Source:** `src/emit/<target>.rs` plus `src/emit/<target>/` — the
entry file and its submodule directory are ONE module, not two
generations. (Rust, Go, and Elixir were rewritten as strangler-fig
`*2` modules behind thin shims; the rewrites took over and the `*2`
names were folded away in the 2026-08-19 rename — each module's
header records the lineage.) Targets: TypeScript (plus a
typescript-worker build via
`typescript::emit_with_profile`), Crystal, Rust, Go, Kotlin, Python,
Swift, C#/.NET, Elixir, Roda, and the Ruby family (Spinel / Ruby /
JRuby), which rides `emit::ruby` (`emit_spinel` / `emit_library`) plus
a verbatim walk of `runtime/spinel/` — there is no `emit::ruby::emit`.
Target dispatch lives in `src/project.rs::target_files`;
`src/emit/mod.rs` holds only the module declarations and
`EmittedFile`. Generic `Expr` walkers live as `<target>/expr.rs`.
Cross-cutting helpers live in `src/emit/shared/`.

## Status

Roundhouse has completed its migration from per-target derivation to
thin emitters that consume the universal post-lowering IR
(`LibraryClass | LibraryFunction`) for every runtime target except
Python. Spinel/Ruby, TypeScript, Crystal, Rust, Go, and Elixir run on
the thin path — the Rust/Go/Elixir strangler rewrites shipped, became
the default, and now own their targets' names outright — and
the newer Kotlin, Swift, and C#/.NET targets were built on it from
the start. Python is the remaining per-artifact emitter: one
submodule per output kind (`model.rs`, `controller.rs`, `view.rs`,
`route.rs`, …), controllers rendered through `lower`'s shared
`CtrlWalker` (`src/lower/controller_walk.rs`). Its `library.rs` universal-IR
walker exists but is dormant — reserved for strangling the
hand-written `runtime/python/*.py` files via
`runtime_loader::python_units`. Roda sits deliberately outside the
universal IR: it is a source-to-source Rails→Roda+Sequel converter
consuming the INGEST-shape `App` (see `src/emit/roda.rs`).

| Target | Models | Views | Controllers | Tests | Schema/Routes/Seeds | Notes |
|--------|--------|-------|-------------|-------|---------------------|-------|
| Spinel / Ruby | thin | thin | thin | thin | thin | Reference shape — drove the universal IR contract; Ruby emit collapsed to lowered-IR-only (2026-05-05) |
| TypeScript | thin | thin (function) | thin | thin | thin (function) | Rip-and-replace complete; `tsc` green sync + libsql under `tests/typescript_toolchain.rs` (2026-05-07) |
| Crystal | thin | thin (function) | thin | thin | thin | Rip-and-replace complete; compare-crystal 5/5 + framework_tests 8/8 (2026-05-06 → 2026-05-10) |
| Rust (`src/emit/rust/`) | thin | thin | thin | thin | thin | Strangler rewrite (rust2) landed 2026-05-20, renamed to `rust` 2026-08-19 |
| Go (`src/emit/go/`) | thin | thin | thin | thin | thin | Strangler rewrite (go2) shipped 2026-05-24; `entry.rs` assembles overlay + go.mod/root main |
| Elixir (`src/emit/elixir/`) | thin | thin | thin | thin | thin | Sole app-module path since Phase D; `entry.rs` emits the mix/Db/SchemaSQL shell around the `V2.*` overlay |
| Kotlin, Swift, C#/.NET | thin | thin | thin | thin | thin | Built on the universal IR from the start |
| Python | per-target | per-target | per-target | per-target | per-target | The remaining per-artifact emitter (`CtrlWalker` controllers); its `library.rs` is dormant, awaiting the runtime strangle |
| Roda | — | — | — | — | — | Ingest-shape source-to-source converter (issue #67); not on either path by design |

"thin" = consumes the universal IR (`LibraryClass` or
`LibraryFunction`) from a `*_to_library` lowerer. "per-target" =
derives from Rails-shape IR directly (the form being torn down).

## The universal IR contract

The bet (see `project_universal_post_lowering_ir` in auto-memory):
**after lowering, every emitter sees the same shape — either a
plain class with explicit method bodies, or a free function with an
explicit module path.** No Rails DSL surfaces past the lowerer
boundary.

```
ingest → analyze → lower → { LibraryClass | LibraryFunction } → emit
                                       │
                                       ▼
                       LibraryClass: name, parent, methods (with receiver)
                       LibraryFunction: module_path, name, params, body
                       bodies: Expr (typed)
```

The two shapes are exhaustive. See [`lower.md`](lower.md) for the
shape contract and which lowerers produce which shape.

## Per-target shape dispatch

The IR commits to the semantics; each target picks the surface form
that fits its language:

| Shape | Spinel/Crystal/Ruby | TypeScript | Python | Rust | Go | Elixir |
|-------|---------------------|------------|--------|------|----|----|
| `LibraryClass` (with parent) | `class X < Y` | `class X extends Y` | `class X(Y):` | `pub struct X` + `impl Y for X` | named struct + methods | `defmodule X.Y` (mixin via `use`) |
| `LibraryClass{is_module:true}` (no parent, all class methods) | `module X` | (collapses to LibraryFunction) | (collapses) | (collapses) | (collapses) | `defmodule X` |
| `LibraryFunction` | `module X; def self.f; end; end` | `export function f` in `<x>.ts` | `def f` in `<x>.py` | `pub fn f` in `<x>.rs` | `func F` in `<x>.go` | `def f` in `defmodule` |

**TypeScript-specific note:** TS doesn't have first-class
namespaces that span files. The function-per-file emit form needs
an aggregator (see below) so `Views.Articles.foo()` call sites
resolve through a single namespace import.

## Aggregator pattern (TS-specific)

When an artifact spans multiple files (views: one per template) AND
consumers reach into it via dotted access (`Views.Articles.show(x)`),
the TS emit lays down a single aggregator file at the top of the
hierarchy:

```ts
// app/views.ts (the aggregator)
import { article as articles_article } from "./views/articles/_article.js";
import { index as articles_index }     from "./views/articles/index.js";
// ...
export const Views = {
  Articles: {
    article: articles_article,
    index:   articles_index,
    // ...
  },
  Comments: { … },
  Layouts:  { … },
};
```

Per-template files emit one `export function` each; the aggregator
re-imports them (with name-mangled aliases to avoid `index` from
articles colliding with `index` from comments) and assembles a
nested `const` whose key structure mirrors the Ruby module path
(`Views::Articles::article` → `Views.Articles.article`). Consumer
imports become `import { Views } from "../../app/views.js"`; the
existing call sites resolve unchanged.

The single-segment module artifacts (`RouteHelpers`, `Importmap`,
`Schema`, `Seeds`) need no aggregator — `emit_module_file` writes
all functions plus a trailing `export const RouteHelpers = { … }`
into one file at the canonical path.

## Per-target emitter shape

A thin emitter's common trio is `expr.rs` / `ty.rs` / `library.rs`:

```
src/emit/<target>/
  expr.rs              — generic Expr → target syntax (the heavy lifter)
  ty.rs                — Ty → target type rendering
  library.rs           — universal IR walker:
                           - emit_class_file / emit_library_class (LibraryClass)
                           - emit_function_file (LibraryFunction)
                           - emit_module_file (LibraryFunction[] — same module_path)
                           - emit_views_aggregator (TS-specific)
                           - import resolution (TS: collect_imports /
                             collect_imports_for_function in
                             src/emit/typescript/library.rs; go has a
                             dedicated imports module)
                           - rewrite_for_class_method / rewrite_for_constructor /
                             rewrite_for_free_function (see below)
```

Beyond the trio the file set varies per target — compare
`src/emit/crystal/` (adds `method.rs`, `shared.rs`) with
`src/emit/go/` (adds `imports.rs`, `paths.rs`, plus the test emitters)
and `src/emit/rust/` (splits `expr` into a directory and adds
`decide/` for borrow/ownership decisions). Packaging is per-ecosystem
rather than a fixed `package.rs`: TypeScript, Kotlin, and C# carry a
`package.rs`; Python has `pyproject.rs`; Elixir's mix project lives in
`src/emit/elixir/mix.rs`.

`expr.rs` is the substantive per-target work — it embodies the target
language's expression-level semantics (operator dispatch, string vs.
symbol literals, hash key syntax, async suspension points). Even
under the rip-and-replace policy, expr.rs is **the notable exception
worth incremental investment** — it already encodes hard-won
target-specific knowledge that no lowerer can absorb.

The other files mostly walk a `LibraryClass` or `LibraryFunction`
and emit class/function syntax. They're small and replaceable.

## Three rewrite modes for body emission

Body Exprs come out of the lowerer in receiver-less Ruby form (bare
`foo(x)` rather than `self.foo(x)`). The TS body emitter applies one
of three rewrites depending on the body's call context:

| Mode | What it does | When to use |
|------|--------------|-------------|
| `rewrite_for_class_method` | Bare Sends → `this.method(...)`; `Super { args }` → `super.<enclosing>(args)` | Instance methods on a `LibraryClass` |
| `rewrite_for_constructor` | Like class_method but leaves `Super { args }` intact (TS spells parent-constructor calls as `super(args)`, not `super.initialize(args)`) | The `initialize` instance method |
| `rewrite_for_free_function` | Walks for recursion only — no SelfRef injection, no super rewrite | All `LibraryFunction` bodies (views, route helpers, etc.) |

Kernel calls (`raise`, `puts`, `print`, `p`, `pp`) are exempt from
SelfRef injection in all three modes — they keep `recv: None` so the
emit_send special-cases (`raise → throw`, `puts → console.log`) can
fire.

## Two-layer runtime

Each target ships with two layers of runtime (see `runtime/<target>/`
plus the transpiled framework runtime):

1. **Target primitives** (hand-written, small): DB connection
   lifecycle, HTTP server glue, WebSocket plumbing — anything genuinely
   target-idiomatic that no IR-level lowering can capture.
2. **Framework runtime** (transpiled from `runtime/ruby/`): Rails-shape
   surface that emitted apps call into — `ApplicationRecord`,
   `ActionController::Parameters`, `FormBuilder`, `link_to`, etc.
   Authored once in Ruby; transpiled per target via the same
   roundhouse pipeline that compiles user apps.

This split is the architectural commitment behind the unified-IR Phase
1 plan: framework Ruby is a forcing function that any new target must
support. If your emitter can transpile `runtime/ruby/`, it can
transpile a real Rails app.

For TS, the runtime files ship under the emit output's `src/`
directory. The authoritative inventory is the `include_str!` table at
the top of `src/emit/typescript.rs`: the `_base` / helper files are
emitter-generated from `runtime/ruby/`, the rest are hand-written
primitives copied from `runtime/typescript/`. The
`DeploymentProfile` selects among db/server variants (`db.ts` /
`db-libsql.ts` / `db_worker.ts`, `server.ts` / `server-libsql.ts` /
`server-worker.ts`) and adds worker-profile extras (`client.ts`, the
`juntos-worker.ts` bridge). `juntos.ts` is the worker bridge entry
point. See `runtime.md` for the per-target runtime shape.

## Generated TS project layout

A complete TS emit for the real-blog fixture:

```
package.json, tsconfig.json
main.ts                            — boot shell (Schema + Seeds + startServer)
src/                               — framework runtime (see above)
app/
  models/<model>.ts                — one LibraryClass per file
  controllers/<controller>.ts      — one LibraryClass per file
  views.ts                         — aggregator namespace const
  views/<dir>/<template>.ts        — one LibraryFunction per file
  route_helpers.ts                 — RouteHelpers module file
  routes.ts                        — flat route table
  importmap.ts                     — Importmap module file
db/
  seeds.ts                         — Seeds.run module file
test/
  _runtime/{minitest,setup}.ts     — test runtime adapter
  fixtures/<plural>.ts             — one LibraryClass per fixture file
  <model>.test.ts                  — one LibraryClass per test class
  <controller>.test.ts             — controller tests (LibraryClass)
```

Path conventions are TS-specific; other targets pick layouts that
match their ecosystem (`src/main/<target>` for Java-shape targets,
top-level for Go's flat package model, etc.).

## Working policy: rip-and-replace

When an emitter's shape is wrong relative to the universal IR, the
working policy is **rebuild from a clean design**, not refactor in
place. Experience: a fresh emitter against the universal IR shape
takes ~1 week; incrementally evolving the existing one takes
considerably longer because every step must keep the existing
toolchain test green.

Practical consequences:

- Disable the target's CI gate during migration.
- Build the new emitter as a strangler-fig `<target>2` module
  alongside the old one, behind the stable `src/emit/<target>.rs`
  entry point — the pattern rust, go, and elixir all followed. The
  public identity (`crate::emit::rust::emit`, the `--target` CLI
  surface) never moves; the entry file shrinks to a shim once the
  2-module carries everything. (The `ROUNDHOUSE_<TARGET>_V2` env
  flags from the early migrations are vestigial — see
  `docs/env-gates.md`.)
- Flip the default once the new path is green; delete the old.
- `expr.rs` is the exception — port forward, don't rewrite from
  scratch.

Ecosystem files (`Cargo.toml`, `package.json`, `shard.yml`,
`mix.exs`, `pyproject.toml`) carry no semantic divergence; they're
copied/templated and don't need rip-and-replace treatment.

## How emit reaches the file system

```rust
pub struct EmittedFile {
    pub path: PathBuf,
    pub content: String,
}

pub fn emit(app: &App) -> Vec<EmittedFile>;
```

Most `src/emit/<target>.rs` entry files expose `emit(app)` returning a
flat list; the Ruby family instead exposes `emit_spinel` /
`emit_library`, which `src/project.rs` combines with a verbatim walk
of `runtime/spinel/`. The `BuildTarget` → emitter mapping is
centralized in `src/project.rs::target_files`. Callers
(`bin/roundhouse` — both `--target LANG` and `--site` modes — plus
the toolchain tests) write each `EmittedFile` to disk.

## Public surface re-exported from `src/emit/<target>.rs`

The surface varies per target rather than being a uniform contract:

| Symbol | Role |
|--------|------|
| `emit(&App) -> Vec<EmittedFile>` | Main entry — full project emission. Most targets; the Ruby family exposes `emit_spinel` / `emit_library` instead |
| `emit_method(&MethodDef) -> String` | Standalone typed-method renderer for runtime extraction — present on crystal, go, python, ruby, rust, typescript |
| `emit_library_class(&LibraryClass) -> Result<String>` | Class-shape renderer; public on most thin targets for tests + cross-target tooling |
| `emit_library_function(&LibraryFunction) -> Result<String>` | Function-shape renderer — TypeScript-only |
| `<target>_ty(&Ty) -> String` | Type renderer. The `ty` submodules are mostly private (`mod ty;`); a handful re-export (`typescript::ts_ty`, `python::python_ty`) and go/rust are `pub(crate)` |

## Per-target type rendering

The three special `Ty` variants render differently per target (cells
from the `<target>/ty.rs` renderers — `ts_ty`, `rust_ty`, `go_ty`,
`crystal_ty`, `python_ty`):

| Variant | TS | Rust | Go | Crystal | Python |
|---------|----|------|----|---------|--------|
| `Ty::Var(_)` | `any` | `serde_json::Value` | `interface{}` | `String` | `object` |
| `Ty::Untyped` | `any` | `serde_json::Value` | `interface{}` | `String` | `Any` |
| `Ty::Bottom` | `never` | `!` | `interface{}` | `NoReturn` | `Never` |

The Ruby family renders types only into the RBS sidecars
(`emit::ruby`'s `ty_to_rbs`); Elixir emits untyped Elixir and has no
`ty` module.

No target elevates `Untyped` itself to a compile error — even the
strict targets commit to a permissive rendering (`serde_json::Value`,
`interface{}`). What actually guards the gaps is the emit diagnostics
sink (`src/emit/diagnostics.rs`): an unsupported construct both
records a `Diagnostic` and degrades to a target-appropriate
`raise`/`panic`/`throw` stub at that site. The cost of `Untyped` still
lands differently per target (see
`project_ty_untyped_target_dependent`) — a bag type that TS absorbs
silently prices every downstream operation in Rust.

## Adding a new target

The lowerers-first bet says: most of the work has already been done.
A new target needs:

1. `src/emit/<new>/expr.rs` — the per-target expression renderer.
2. `src/emit/<new>/ty.rs` — type rendering.
3. `src/emit/<new>/library.rs` — universal-IR walker:
   - `emit_class_file` for `LibraryClass`
   - `emit_function_file` (and `emit_module_file` if multi-function
     module files are idiomatic)
   - an import-resolution pass mapping cross-artifact Const refs to
     import lines (the models: `collect_imports` /
     `collect_imports_for_function` in `src/emit/typescript/library.rs`,
     or go's `imports` module)
4. `runtime/<new>/` — target primitives only (DB, HTTP). Framework
   runtime comes free via transpiling `runtime/ruby/`.
5. `tests/<new>_toolchain.rs` — the verification gate.

Models-first within a target: models are independent and exercise
most of the type-system surface. Web trio (controllers + views +
routes) is coordinated and should land together.

Aggregator decision: if the target has first-class namespaces that
span files (Crystal modules, Elixir's `defmodule` re-opens, Ruby's
open classes), no aggregator needed. If not (TS, Python, Rust, Go),
emit a per-app aggregator file or use the target's namespace
mechanism (Python's `__init__.py`, Rust's `mod.rs`).

## Key files

| File | Role |
|------|------|
| `src/emit/mod.rs` | `EmittedFile` + module declarations (dispatch lives in project.rs) |
| `src/project.rs::target_files` | `BuildTarget` → emitter dispatch, plus the Ruby-family runtime walk and tree-shake |
| `src/emit/<target>.rs` | Per-target entry + `emit()` pipeline |
| `src/emit/<target>/expr.rs` | Generic Expr walker (the heavy lifter) |
| `src/emit/<target>/ty.rs` | Ty rendering |
| `src/emit/<target>/library.rs` | Universal-IR walker + import resolution |
| `src/emit/diagnostics.rs` | Thread-local diagnostic sink — unsupported constructs self-report and degrade to a raise/panic stub |
| `src/emit/typescript/{js_ast,printer,sourcemap}.rs` | TS's JS-AST layer: build an AST, print it, carry source maps |
| `src/emit/rust/decide/` | Rust's borrow/ownership decision passes (last-use, parens, string coloring) |
| `src/emit/shared/` | Cross-cutting helpers (binop classifiers, schema SQL renderer, etc.) |
| `src/lower/{model,view,jbuilder,controller,test_module,fixture}_to_library/` | LibraryClass producers |
| `src/lower/{routes,importmap,schema,seeds}_to_library/` | LibraryFunction producers |

## Related docs

- [`lower.md`](lower.md) — target-neutral lowerings; the producers of
  `LibraryClass` and `LibraryFunction`.
- [`analyze.md`](analyze.md) — typed IR that lowering consumes.
- [`runtime.md`](runtime.md) — per-target runtime layer.
- [`verification.md`](verification.md) — toolchain tests + DOM
  equivalence gate.
