# Emit

Each emitter takes the analyzed, lowered IR and produces a complete,
runnable project for one target language. The current architecture
follows a **lower-once, render N-ways** bet: most logic moves into
target-neutral lowerings that produce a universal IR; per-target
emitters render that IR into idiomatic source.

**Source:** `src/emit/<target>/`, one directory per target (Ruby,
TypeScript, Crystal, Rust, Go, Python, Elixir, Spinel). Generic
`Expr` walkers live as `<target>/expr.rs`. Cross-cutting helpers live
in `src/emit/shared/`.

## Status

Roundhouse is mid-migration from per-target derivation to thin
emitters that consume the universal post-lowering IR
(`LibraryClass | LibraryFunction`). TypeScript was rebuilt from
scratch and now consumes both shapes for every artifact category;
the remaining six targets follow on a strangler-fig schedule.

| Target | Models | Views | Controllers | Tests | Schema/Routes/Seeds | Notes |
|--------|--------|-------|-------------|-------|---------------------|-------|
| Ruby | n/a | n/a | n/a | n/a | n/a | Round-trip identity partner — see `data/ruby-and-erb.md` |
| Spinel | thin | thin | thin | thin | thin | Reference shape — drove the universal IR contract |
| TypeScript | thin | thin (function) | thin | thin | thin (function) | Code-complete from a fresh rebuild; `tsc` not yet exercised |
| Crystal | per-target | thin (flag) | per-target | per-target | per-target | View thin scaffold landed; not yet default |
| Rust, Go, Python, Elixir | per-target | per-target | per-target | per-target | per-target | Migration deferred; rip-and-replace once shape stabilizes |

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

A thin emitter has these files per target:

```
src/emit/<target>/
  expr.rs              — generic Expr → target syntax (the heavy lifter)
  ty.rs                — Ty → target type rendering
  library.rs           — universal IR walker:
                           - emit_class_file (LibraryClass)
                           - emit_function_file (LibraryFunction)
                           - emit_module_file (LibraryFunction[] — same module_path)
                           - emit_views_aggregator (TS-specific)
                           - render_imports (cross-class refs → import lines)
                           - rewrite_for_class_method / rewrite_for_constructor /
                             rewrite_for_free_function (see below)
  package.rs           — project shell (package.json, tsconfig.json)
```

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

For TS, the runtime files ship inlined into the emit output under
`src/` (twelve files: `action_controller_base`, `active_record_base`,
`errors`, `http`, `inflector`, `parameters`, `router`, `server`,
`test_support`, `validations`, `view_helpers`, `view_helpers_generated`)
plus `src/juntos.ts` as the central runtime hub. See `runtime.md` for
the per-target file inventory.

## Generated TS project layout

A complete TS emit produces ~43 files for the real-blog fixture:

```
package.json, tsconfig.json
main.ts                            — boot shell (Schema + Seeds + startServer)
src/                               — framework runtime (12 files + juntos.ts + schema.ts)
app/
  models/<model>.ts                — one LibraryClass per file
  controllers/<controller>.ts      — one LibraryClass per file
  views.ts                         — aggregator namespace const
  views/<dir>/<template>.ts        — one LibraryFunction per file
  route_helpers.ts                 — RouteHelpers module file
  importmap.ts                     — Importmap module file
db/
  seeds.ts                         — Seeds.run module file
test/
  _runtime/minitest.ts             — test runtime adapter
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
- Land the new emitter behind an env-flag fork
  (`ROUNDHOUSE_<TARGET>_VIEW_THIN=1`) so the old path keeps working
  for other targets that haven't migrated yet.
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

Each `src/emit/<target>.rs` exposes `emit(app)` returning a flat list.
Callers (`bin/roundhouse`, `bin/build-site`, the toolchain tests)
write each `EmittedFile` to disk.

## Public surface re-exported from `src/emit/<target>.rs`

| Symbol | Role |
|--------|------|
| `emit(&App) -> Vec<EmittedFile>` | Main entry — full project emission |
| `emit_method(&MethodDef) -> String` | Standalone typed-method renderer (used by `bin/build-site` and runtime extraction) |
| `emit_library_class(&LibraryClass) -> Result<String>` | Class-shape renderer; public for tests + cross-target tooling |
| `emit_library_function(&LibraryFunction) -> Result<String>` | Function-shape renderer; same role |
| `<target>_ty(&Ty) -> String` | Type renderer — public for tests + cross-target tooling |

## Per-target type rendering

The three special `Ty` variants render differently per target:

| Variant | TS | Rust | Go | Crystal | Python | Elixir | Ruby |
|---------|----|------|----|---------|--------|--------|------|
| `Ty::Var(_)` | error at emit | error | error | error | error | error | inferred |
| `Ty::Untyped` | `any` | `()` (forces commit) | `interface{}` | `_` | `Any` | `term()` | n/a |
| `Ty::Bottom` | `never` | `!` | `interface{}` | `NoReturn` | `Never` | `none()` | n/a |

Strict targets (Rust, Go) elevate `Untyped` to a compile error rather
than rendering a permissive type — the three-bar test discipline (see
`project_ty_untyped_target_dependent`).

## Adding a new target

The lowerers-first bet says: most of the work has already been done.
A new target needs:

1. `src/emit/<new>/expr.rs` — the per-target expression renderer.
2. `src/emit/<new>/ty.rs` — type rendering.
3. `src/emit/<new>/library.rs` — universal-IR walker:
   - `emit_class_file` for `LibraryClass`
   - `emit_function_file` (and `emit_module_file` if multi-function
     module files are idiomatic)
   - `render_imports` mapping cross-artifact Const refs to import lines
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
| `src/emit/mod.rs` | `EmittedFile`, target dispatch |
| `src/emit/<target>.rs` | Per-target entry + `emit()` pipeline |
| `src/emit/<target>/expr.rs` | Generic Expr walker (the heavy lifter) |
| `src/emit/<target>/ty.rs` | Ty rendering |
| `src/emit/<target>/library.rs` | Universal-IR walker + import resolution |
| `src/emit/<target>/package.rs` | Project shell (manifest + tsconfig/equivalent) |
| `src/emit/shared/` | Cross-cutting helpers (binop classifiers, schema SQL renderer, etc.) |
| `src/lower/{model,view,controller,test_module,fixture}_to_library/` | LibraryClass producers |
| `src/lower/{routes,importmap,schema,seeds}_to_library/` | LibraryFunction producers |

## Related docs

- [`lower.md`](lower.md) — target-neutral lowerings; the producers of
  `LibraryClass` and `LibraryFunction`.
- [`analyze.md`](analyze.md) — typed IR that lowering consumes.
- [`runtime.md`](runtime.md) — per-target runtime layer.
- [`verification.md`](verification.md) — toolchain tests + DOM
  equivalence gate.
