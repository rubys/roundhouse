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
| `importmap_to_library` | `app.importmap` | Two `LibraryFunction`s (`json`, `tags`) under `module_path: ["Importmap"]` |
| `schema_to_library` | `Schema` | One `LibraryFunction` (`create_tables`) under `module_path: ["Schema"]`; body is the rendered DDL as a `Lit::Str` |
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
work.

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

### `unwrap_respond_to`

`respond_to do |format| format.html; format.json end` blocks get
collapsed to the HTML branch (the only format every target emits
today). JSON / Turbo Stream formats will re-enter as separate
rendered outputs once a second format matters.

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
| `erb_trim::trim_view` | ERB-derived view tree | Whitespace-normalized view tree | `view_to_library` (runs first) |

Each pass is pure: same input → same output, no side effects, no
target awareness. Re-exports live in `src/lower/mod.rs`.

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

## Status of legacy per-target derivation

The older shape — `CtrlWalker` trait, `WalkCtx` / `WalkState`,
`SendKind` classifier in `src/lower/controller.rs` — is still in
place for emitters that haven't migrated to the universal IR. It
walks controller bodies through a target-implemented dispatch trait,
with each target overriding leaf `write_*` / `render_*` methods.

This shape is being torn down as `controller_to_library` lands per
target. New work shouldn't extend it; existing per-target emitters
either migrate to the universal IR or get rip-and-replaced (see
`emit.md`'s working policy section).

## Key files

| File | Role |
|------|------|
| `src/lower/mod.rs` | Module layout + re-exports |
| `src/dialect.rs` | `LibraryClass`, `LibraryFunction`, `MethodDef`, `AccessorKind` |
| `src/lower/typing.rs` | `fn_sig`, `lit_str`, `type_method_body`, `with_ty` — shared typing helpers used by every shape-producing lowerer |
| `src/lower/model_to_library/` | Model dialect → `LibraryClass` |
| `src/lower/view_to_library/` | ERB view → `LibraryClass` (registry) + `LibraryFunction` (emit, via `flatten_lcs_to_functions`) |
| `src/lower/controller_to_library/` | Controller dialect → `LibraryClass` |
| `src/lower/test_module_to_library/` | Minitest class → `LibraryClass` |
| `src/lower/fixture_to_library/` | YAML fixtures → `LibraryClass` per fixture file |
| `src/lower/routes_to_library/` | Routes → `LibraryFunction` (RouteHelpers) |
| `src/lower/importmap_to_library/` | Importmap → `LibraryFunction` (Importmap module) |
| `src/lower/schema_to_library/` | Schema → `LibraryFunction` (`Schema.create_tables`) |
| `src/lower/seeds_to_library/` | Seeds → `LibraryFunction` (`Seeds.run`) |
| `src/lower/controller.rs`, `controller_walk.rs` | Legacy per-target derivation (being torn down) |
| `src/lower/validations.rs` | `LoweredValidation`, `Check` enum |
| `src/lower/routes.rs` | `flatten_routes`, `FlatRoute` |
| `src/lower/persistence.rs` | `LoweredPersistence` |
| `src/lower/broadcasts.rs` | `LoweredBroadcasts` |
| `src/lower/fixtures.rs` | Fixture load plan |
| `src/lower/associations.rs` | has_many resolution |
| `src/lower/controller_test.rs` | Test-body classification |
| `src/lower/erb_trim.rs` | ERB whitespace normalization |

## Related docs

- [`analyze.md`](analyze.md) — the typed IR that lowering consumes.
- [`emit.md`](emit.md) — the universal IR contract + per-target
  emitter shape that consumes `LibraryClass` and `LibraryFunction`.
- [`../data/catalog.md`](../data/catalog.md) — the AR method
  classification that some lowerings consult.
- [`../data/schema-routes-seeds.md`](../data/schema-routes-seeds.md) —
  the ingest IR for schema/routes/seeds (input to the `*_to_library`
  passes documented here).
