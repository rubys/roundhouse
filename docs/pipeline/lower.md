# Lower

The lowering layer sits between analyze and emit. Its job: take
Rails-dialect IR (validations, associations, routes, controller
actions, view templates) plus the analyzer's annotations, and produce
**target-neutral** forms that all emitters consume.

**Source:** `src/lower/` — one file or subdirectory per concern.

## Why lower?

Before this layer existed, each target emitter independently
re-implemented the same analysis: SQL strings for persistence, view
helper rewrites, validation rule evaluation, router dispatch tables.
Per-target copies, slight drift, large maintenance surface.

The architectural bet: **lower once, render N ways.** Extract the
logic that's identical across targets as IR-level lowerings; each
emitter consumes the lowered form. Adding a new target becomes "write
renders" rather than "re-implement the logic."

## The post-analyze pass pipeline

The organizing principle of the directory: after the analyzer
converges, a pipeline of several dozen small passes rewrites the App
before emit. `POST_ANALYZE_PASS_ORDER` in `src/lower/mod.rs` is the
single authority on their ordering — a topologically-ordered table of
`(pass_name, &[passes_that_must_run_before_it])` entries whose list
order is the intended call order. The ordering knowledge that used to
live only in prose scattered across the passes ("AFTER
send_dispatch, by contract") now lives in that one table; soundness
(every declared predecessor precedes its dependent) and code↔list
correspondence are enforced by debug assertions and a unit test.

The entry point is `apply_post_analyze_lowerings(&mut App, registry)`
(`src/lower/mod.rs`): it mutates the App in place, pass by pass, and
returns the residue diagnostics — sites a pass had to leave dynamic,
with the reason. `registry` is the analyzer's post-fixpoint class
table (`Analyzer::class_registry`); passes that synthesize dispatches
consult it to stamp what analyze would have computed. The pipeline is
invoked from `src/session.rs::analyze_and_lower`, the shared seam
every emit-bound driver runs after ingest.

### Pass families

A map of the families, not an inventory (`src/lower/mod.rs` declares
the full list):

- **Query algebra** — `src/lower/arel/` (compile-time query IR),
  `scope_chain.rs`, `chain.rs`.
- **Functionalize** — `src/lower/functionalize/`:
  imperative→functional rewrites (while→recursion,
  mutation→struct-return), deliberately Elixir-only — an explicit
  exception to lower-once-render-N-ways, since the recursion form is
  strictly worse for imperative targets, which keep the native
  `while`.
- **JSON / serialization** — `src/lower/jbuilder_to_library/`,
  `as_json_shape.rs`, `as_json_writer.rs`.
- **ActiveSupport grounding** — `blank.rs`, `duration.rs`,
  `inquiry.rs`, ….
- **Rails-API grounding** — `secure_password.rs`, `signed_id.rs`,
  `rich_text.rs`, ….
- **Params / strong parameters** — `params_merge.rs`, `kwsplat.rs`, ….
- **Dispatch / typing** — `send_dispatch.rs`,
  `ty_coerce_insertion.rs`.
- **View classification** — `view.rs`, the shared view-helper
  classifier (distinct from `view_to_library/`).

## The two-shape contract

After the lowerer boundary, every emitter sees the IR in one of two
shapes. The choice depends on what the artifact *is*, not what
language it'll render to:

### `LibraryClass` — class-shaped artifacts

A user-defined class with instance state, optional inheritance, and a
mix of class and instance methods. Models, controllers, and tests are
the canonical producers. The IR carries:

- `name: ClassId`
- `parent: Option<ClassId>` (e.g. `ApplicationRecord`, `ActionController::Base`)
- `is_module: bool` (Ruby's `module` vs `class` distinction — preserved
  for surface-form fidelity)
- `includes: Vec<ClassId>` (mixin modules)
- `methods: Vec<MethodDef>` (each with `MethodReceiver::Instance | Class`)

Per-target rendering: `class Foo extends Bar { … }` in TS, `pub struct
Foo` + `impl Foo` in Rust, `class Foo < Bar` in Ruby/Spinel/Crystal,
`defmodule Foo` in Elixir, etc.

### `LibraryFunction` — module-of-functions artifacts

A top-level callable: no instance state, no inheritance, fully
resolvable at the call site as `<module_path>.<name>(args)`. Views,
route helpers, importmap helpers, schema initializer, and seeds are
the canonical producers. The IR carries:

- `module_path: Vec<Symbol>` (e.g. `["Views", "Articles"]`, `["RouteHelpers"]`)
- `name: Symbol`
- `params: Vec<Param>`, `body: Expr`, `signature: Option<Ty>`, `effects: EffectSet`

The IR commits to the semantics; per-target emitters pick the idiomatic
surface form:

| Target | Surface form |
|--------|--------------|
| Spinel / Crystal / Ruby | `module M::N; def self.f(x); …; end; end` |
| TypeScript | `export function f(x: T): R { … }` in `m/n.ts` |
| Python | `def f(x): …` in `m/n.py` |
| Rust | `pub fn f(x: &T) -> R { … }` in `m/n.rs` |
| Go | `func F(x *T) R { … }` in `m/n.go` |
| Elixir | `defmodule M.N do; def f(x), do: …; end` in `m/n.ex` |

The two shapes are exhaustive: every lowerer produces one or the
other (or both — view lowerers run a flatten pass to expose the
class-shape registry to the body-typer while emitting the function
shape).

## Lowerers that produce `LibraryClass`

| Lowerer | Input | Output | Bulk entry point |
|---------|-------|--------|------------------|
| `model_to_library` | `Model` (validations, associations, scopes) | `LibraryClass` (one per model + per-association classes like `ArticleCommentsProxy`) | `lower_models_to_library_classes` / `lower_models_with_registry` |
| `controller_to_library` | `Controller` (actions, before-actions, callbacks) | `LibraryClass` with one method per public action + synthesized `process_action` dispatcher | `lower_controllers_to_library_classes` |
| `test_module_to_library` | `TestModule` (Minitest test class) | `LibraryClass` with one method per `test "…" do` + setup-inlined per test | `lower_test_modules_to_library_classes` |
| `fixture_to_library` | `Fixture` (parsed YAML) | `LibraryClass` per fixture file (`<Plural>Fixtures` with one class method per label) | `lower_fixtures_to_library_classes` |

Each lowerer:

- Expands DSL surface into method bodies (e.g. `validates :title,
  presence: true` becomes a `validate` method that pushes
  `ValidationError`s).
- Rewrites helpers and form builders into runtime calls (e.g.
  `link_to` → `Roundhouse::ViewHelpers.link_to(...)`).
- Runs the body-typer over the rewritten bodies so emitters get
  fully-typed `Expr` trees.

## Lowerers that produce `LibraryFunction`

| Lowerer | Input | Output |
|---------|-------|--------|
| `view_to_library` (via `flatten_lcs_to_functions`) | ERB-lowered view template | One `LibraryFunction` per template; `module_path` derived from view directory (`["Views", "Articles"]`) |
| `routes_to_library` | `app.routes` (after `flatten_routes`) | One `LibraryFunction` per named route under `module_path: ["RouteHelpers"]`; body is a typed `StringInterp` building the path from path-params |
| `importmap_to_library` | `app.importmap` | Two `LibraryFunction`s (`pins`, `entry`) under `module_path: ["Importmap"]` |
| `schema_to_library` | `Schema` | One `LibraryFunction` (`statements`) under `module_path: ["Schema"]`; returns the rendered DDL as an `Array<Str>` of `Lit::Str` statements |
| `seeds_to_library` | `app.seeds` (typed Expr) | One `LibraryFunction` (`run`) under `module_path: ["Seeds"]`; body is the seeds Expr verbatim |

Why this group: each is "module of functions" rather than "class with
state." Forcing them through `LibraryClass{is_module:true}` with
class methods worked but produced shape mismatches in TS (literal
`::` in class headers, `new Views.X(...)` mis-emitted as a
constructor call). `LibraryFunction` says exactly what these are.

The view lowerer is dual-shape: `lower_views_to_library_classes`
returns the class-shape (consumed by the body-typer registry to type
cross-class dispatch like `Views::Articles.article(x)`),
`flatten_lcs_to_functions` pivots that output into per-template
`LibraryFunction`s for emission. Both share the same body-typing
work. `jbuilder_to_library` (`src/lower/jbuilder_to_library/`) joins
the same pipeline for `*.json.jbuilder` templates:
`lower_jbuilder_to_library_classes` produces `<name>_json` methods on
the same `Views::*` modules, in the same string-accumulator shape ERB
bodies use.

`routes_to_library` also emits the dispatch surface —
`lower_routes_to_dispatch_functions` builds `RouteTable.table` /
`RouteTable.root` under `module_path: ["RouteTable"]` — plus
URL-option helpers (`lower_url_option_helpers`) for helper calls
carrying extra query options, and its `direct.rs` lowers
`direct :name` custom URL helpers into real `RouteHelpers` functions.

## Pre-emit lowering passes

Three passes rewrite the controller-body `Expr` tree to a normalized
form. They run inside `controller_to_library` (or directly when older
per-target emitters consume them):

### `synthesize_implicit_render`

Rails actions frequently end without an explicit `render` — the
framework supplies one implicitly from the action name (`index` →
`render :index`). This pass detects bodies that lack a trailing
response terminal and appends the synthesized render call, so every
downstream pass sees a uniform "body ends with a response" shape.

### `unwrap_respond_to_with_format_dispatch`

`respond_to do |format| … end` blocks are lowered by
`unwrap_respond_to_with_format_dispatch`
(`src/lower/controller/body.rs`), which `controller_to_library`
calls. A `FormatBreadth` widening lattice decides which non-HTML arms
survive as `request_format` conditionals — json and rss arms are
preserved per emit path's capability (the two widenings have
different runtime costs; see the struct's doc). The plain
`unwrap_respond_to` — collapse to the HTML branch only — survives for
per-target paths that can't emit the dispatch; its own doc comment
calls that the legacy behavior.

### `resolve_before_actions` + `inline_before_filters`

`before_action :set_article` doesn't produce any IR of its own in the
action body — it runs a method that assigns an ivar. The resolution
pass identifies which actions a filter applies to;
`controller_to_library` then inlines the filter body at the top of
each action it covers, so the action body becomes self-contained
without a runtime filter chain.

## Support lowerings

Lowerings that produce target-neutral forms other than `LibraryClass`
or `LibraryFunction` — these feed *into* the shape-producing lowerers
above:

| Pass | Source | Output | Consumer |
|------|--------|--------|----------|
| `lower_validations` | Model validations | `Vec<LoweredValidation>` — each attribute with its expanded `Check` enum list | `model_to_library` |
| `lower_persistence` | Model + Schema | `LoweredPersistence` with INSERT/UPDATE/DELETE/SELECT strings, `belongs_to` checks, dependent-destroy cascades | `model_to_library` |
| `flatten_routes` | `RouteTable` | `Vec<FlatRoute>` (one entry per `(method, path, controller, action)`) | `routes_to_library`, controller-test dispatch |
| `lower_broadcasts` | Model `broadcasts_to` declarations | `LoweredBroadcasts` (turbo-stream actions per association edge) | `model_to_library` |
| `resolve_has_many` | Model associations | `HasManyRef` (target class + foreign key) | `model_to_library`, view-helper resolution |

These support passes and the `*_to_library` shape producers are
derivations: same input → same output, no target awareness. The
post-analyze pass family is deliberately not that — it mutates the
App in place and accumulates a residue ledger. Re-exports live in
`src/lower/mod.rs`.

## Self-describing IR

The lowerer landed a working principle: **when the lowerer knows a
fact, the IR records it.** Three concrete instances:

- `MethodDef.kind: AccessorKind::{Method, AttributeReader, AttributeWriter}` —
  attr_reader/writer/accessor are lowered to synthetic methods, but
  the `kind` field tells emitters which collapse rules to apply
  (e.g. fold matching reader+writer into a class field).
- `LibraryFunction.signature` — every function ships with its full
  `Ty::Fn` (param types + return + block + effects), set at lower
  time, so the body-typer registry doesn't have to rediscover them.
- `Send.parenthesized` — set during lowering for Method-kind
  dispatches, so emitters know whether to add `()` without a
  type-aware lookup.

The contrast: pre-principle, emitters re-derived facts the lowerer
already knew (was this a method or an attr? does this Send need
parens? is this body's return Nil?). Each rediscovery was a place
two emitters could disagree.

## Legacy per-target derivation: retired

Every shipped target consumes the universal IR for controllers. The
older shape — a `CtrlWalker` trait walking controller bodies through
target-implemented leaf methods — was deleted 2026-08-19 when its
last consumer (Python's per-artifact controller emit) switched to the
overlay (`docs/python-overlay-plan.md`). Spinel/Ruby, TypeScript, and
Crystal were rip-and-replaced end-to-end; Kotlin, Swift, and C#/.NET
were built on the universal IR from the start; Rust, Go, and Elixir
arrived via strangler rewrites whose `*2` names folded away in the
same-day rename (each module's header records the lineage). The
`SendKind` classifier (`src/lower/controller/send.rs`) survives — it
serves the shared controller lowering, not the retired walker.

One target sidesteps these lowerings entirely: the Roda target
(`emit::roda`, the issue #67 conversion spike) emits Roda + Sequel
source that runs on the real gems, so it works from the ingest-shape
App — the transpile driver (`src/bin/roundhouse.rs`) skips
`analyze_and_lower` for it (see the `BuildTarget::Roda` doc comment
in `src/project.rs`).

New work shouldn't extend the legacy form; existing per-target
emitters either migrate to the universal IR or get rip-and-replaced
(see `emit.md`'s working policy section).

## Key files

| File | Role |
|------|------|
| `src/lower/mod.rs` | `POST_ANALYZE_PASS_ORDER` + `apply_post_analyze_lowerings` (the pass pipeline), module layout, re-exports |
| `src/dialect.rs` | `LibraryClass`, `LibraryFunction`, `MethodDef`, `AccessorKind` |
| `src/lower/typing.rs` | `fn_sig`, `lit_str`, `type_method_body`, `with_ty` — shared typing helpers used by every shape-producing lowerer |
| `src/lower/model_to_library/` | Model dialect → `LibraryClass` |
| `src/lower/view_to_library/` | ERB view → `LibraryClass` (registry) + `LibraryFunction` (emit, via `flatten_lcs_to_functions`) |
| `src/lower/controller_to_library/` | Controller dialect → `LibraryClass` |
| `src/lower/test_module_to_library/` | Minitest class → `LibraryClass` |
| `src/lower/fixture_to_library/` | YAML fixtures → `LibraryClass` per fixture file |
| `src/lower/routes_to_library/` | Routes → `LibraryFunction` (RouteHelpers) |
| `src/lower/importmap_to_library/` | Importmap → `LibraryFunction` (Importmap module) |
| `src/lower/schema_to_library/` | Schema → `LibraryFunction` (`Schema.statements`) |
| `src/lower/jbuilder_to_library/` | `*.json.jbuilder` → `<name>_json` methods on the `Views::*` classes |
| `src/lower/seeds_to_library/` | Seeds → `LibraryFunction` (`Seeds.run`) |
| `src/lower/controller/` | Shared controller-body machinery — `send.rs` (`SendKind`), `body.rs` (`FormatBreadth`, respond_to lowering), `actions.rs` (before-action resolution) |
| `src/lower/validations.rs` | `LoweredValidation`, `Check` enum |
| `src/lower/routes.rs` | `flatten_routes`, `FlatRoute` |
| `src/lower/persistence.rs` | `LoweredPersistence` |
| `src/lower/broadcasts.rs` | `LoweredBroadcasts` |
| `src/lower/fixtures.rs` | Fixture load plan |
| `src/lower/associations.rs` | has_many resolution |
| `src/lower/controller_test.rs` | Test-body classification |

## Related docs

- [`analyze.md`](analyze.md) — the typed IR that lowering consumes.
- [`emit.md`](emit.md) — the universal IR contract + per-target
  emitter shape that consumes `LibraryClass` and `LibraryFunction`.
- [`../data/catalog.md`](../data/catalog.md) — the AR method
  classification that some lowerings consult.
- [`../data/schema-routes-seeds.md`](../data/schema-routes-seeds.md) —
  the ingest IR for schema/routes/seeds (input to the `*_to_library`
  passes documented here).
