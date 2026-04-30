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
emitters that consume a universal post-lowering IR (`LibraryClass`).
TypeScript is the proving ground; Crystal is in flight; the remaining
five targets follow.

| Target | Models | Views | Controllers | Notes |
|--------|--------|-------|-------------|-------|
| Ruby | n/a | n/a | n/a | Round-trip identity partner — see `data/ruby-and-erb.md` |
| Spinel | thin | thin | thin | Reference shape — defines the universal IR contract |
| TypeScript | thin | thin | per-target | Controller emitter still derived; thin variant in flight |
| Crystal | per-target | thin (flag) | per-target | View thin scaffold landed; not yet default |
| Rust, Go, Python, Elixir | per-target | per-target | per-target | Migration deferred; rip-and-replace once shape stabilizes |

"thin" = consumes `LibraryClass` from a `*_to_library` lowerer.
"per-target" = derives from Rails-shape IR directly (the form being
torn down).

## The universal IR contract

The bet (see `project_universal_post_lowering_ir` in auto-memory):
**after lowering, every emitter sees the same shape — a collection of
plain classes with explicit method bodies.** No Rails DSL surfaces
past the lowerer boundary.

```
ingest → analyze → lower → LibraryClass → emit
                              │
                              ▼
                      classes : Vec<LibraryClass>
                      methods : Vec<MethodDef>
                      bodies  : Expr (typed)
```

Three lowerers produce this shape:

- `src/lower/model_to_library/` — model dialect → `LibraryClass`.
  Validations, persistence, associations all expand into method
  bodies on a plain class.
- `src/lower/view_to_library/` — ERB-derived view → `LibraryClass`.
  Each template becomes a typed method; helpers + form builders
  resolve through library shapes, not per-target machinery.
- `src/lower/controller_to_library/` — controller dialect →
  `LibraryClass`. Actions become methods; before-actions inline;
  render/redirect resolve to runtime calls.

## Per-target emitter shape

A thin emitter has three files per output kind:

```
src/emit/<target>/
  expr.rs              — generic Expr → target syntax (the heavy lifter)
  ty.rs                — Ty → target type rendering
  model_from_library.rs — LibraryClass → model file
  view_thin.rs         — LibraryClass → view file
  controller_thin.rs   — LibraryClass → controller file (in flight)
  ...                  — schema, routes, importmap, project shell
```

`expr.rs` is the substantive per-target work — it embodies the target
language's expression-level semantics (operator dispatch, string vs.
symbol literals, hash key syntax, async suspension points). Even
under the rip-and-replace policy, expr.rs is **the notable exception
worth incremental investment** — it already encodes hard-won
target-specific knowledge that no lowerer can absorb.

The other files mostly walk a `LibraryClass` and emit class/method
syntax. They're small and replaceable.

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

See `runtime.md` for the per-target file inventory.

## Working policy: rip-and-replace

When an emitter's shape is wrong relative to the universal IR, the
working policy is **rebuild from a clean design**, not refactor in
place. Experience: a fresh emitter against the LibraryClass shape
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
3. Thin emitters (`model_from_library.rs`, `view_thin.rs`,
   `controller_thin.rs`) — mostly mechanical walks over `LibraryClass`.
4. `runtime/<new>/` — target primitives only (DB, HTTP). Framework
   runtime comes free via transpiling `runtime/ruby/`.
5. `tests/<new>_toolchain.rs` — the verification gate.

Models-first within a target: models are independent and exercise
most of the type-system surface. Web trio (controllers + views +
routes) is coordinated and should land together.

## Key files

| File | Role |
|------|------|
| `src/emit/mod.rs` | `EmittedFile`, target dispatch |
| `src/emit/<target>.rs` | Per-target entry + module list |
| `src/emit/<target>/expr.rs` | Generic Expr walker (the heavy lifter) |
| `src/emit/<target>/ty.rs` | Ty rendering |
| `src/emit/<target>/{model,view,controller}*.rs` | Output-kind emitters |
| `src/emit/shared/` | Cross-cutting helpers (binop classifiers, etc.) |
| `src/lower/{model,view,controller}_to_library/` | The producers of the universal IR |

## Related docs

- `analyze.md` — typed IR that lowering consumes.
- `lower.md` — target-neutral lowerings; the producers of `LibraryClass`.
- `runtime.md` — per-target runtime layer.
- `verification.md` — toolchain tests + DOM equivalence gate.
