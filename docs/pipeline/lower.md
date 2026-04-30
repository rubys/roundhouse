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

## The library-shape lowerers (current bet)

Three lowerers produce a single canonical IR — `LibraryClass` — that
every emitter reads. After the lowerer boundary, no Rails DSL
remains: just plain classes with explicit method bodies.

| Lowerer | Input | Output | Entry point |
|---------|-------|--------|-------------|
| `model_to_library` | `Model` (validations, associations, scopes) | `LibraryClass` with method bodies | `lower_model_to_library_class` |
| `view_to_library` | ERB-lowered view template | `LibraryClass` with one method per template + helper rewrites | `lower_view_to_library_class` |
| `controller_to_library` | `Controller` (actions, before-actions, callbacks) | `LibraryClass` with one method per action | `lower_controller_to_library_class` |

Each lowerer:

- Expands DSL surface into method bodies (e.g. `validates :title,
  presence: true` becomes a `validate` method that pushes
  `ValidationError`s).
- Rewrites helpers and form builders into runtime calls (e.g.
  `link_to` → `Roundhouse::ViewHelpers.link_to(...)`).
- Runs the body-typer over the rewritten bodies so emitters get
  fully-typed `Expr` trees.

The thin emitters that consume `LibraryClass` are small — they walk a
list of methods and emit class/method syntax. See `emit.md`.

## Other lowering passes

Lowerings that produce target-neutral forms other than `LibraryClass`:

| Pass | Source | Output |
|------|--------|--------|
| `lower_validations` | Model validations | `Vec<LoweredValidation>` — each attribute with its expanded `Check` enum list |
| `lower_persistence` | Model + Schema | `LoweredPersistence` with INSERT/UPDATE/DELETE/SELECT strings, `belongs_to` checks, dependent-destroy cascades |
| `flatten_routes` | `RouteTable` | `Vec<FlatRoute>` (one entry per `(method, path, controller, action)`) |
| `lower_fixtures` | YAML fixtures | `LoweredFixtureSet` (per-record load plan) |
| `lower_broadcasts` | Model `broadcasts_to` declarations | `LoweredBroadcasts` (turbo-stream actions per association edge) |
| `resolve_has_many` | Model associations | `HasManyRef` (target class + foreign key) |

Each pass is pure: same input → same output, no side effects, no
target awareness. Re-exports live in `src/lower/mod.rs`.

## Pre-emit lowering passes

Three passes rewrite the controller-body `Expr` tree to a normalized
form. They run inside `lower_action` (or directly when older
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

### `resolve_before_actions`

`before_action :set_article` doesn't produce any IR of its own in the
action body — it runs a method that assigns an ivar. This pass inlines
the before-action's effect at the top of each action it covers, so
the action body becomes self-contained.

## Legacy: per-target controller derivation

The older shape — `CtrlWalker` trait, `WalkCtx` / `WalkState`,
`SendKind` classifier in `src/lower/controller.rs` — is still in
place for emitters that haven't migrated to `LibraryClass`. It walks
controller bodies through a target-implemented dispatch trait, with
each target overriding leaf `write_*` / `render_*` methods.

This shape is being torn down as `controller_to_library` lands per
target. New work shouldn't extend it; existing per-target emitters
either migrate to thin or get rip-and-replaced (see `emit.md`'s
working policy section).

## Other lowering modules

### `src/lower/validations.rs`

Expands surface `Validation` rules into flat `Check` entries. A
source `Length { min: 10, max: 100 }` lowers to two checks
(`MinLength` then `MaxLength`), so the per-target render doesn't
carry optional-bound logic.

### `src/lower/routes.rs`

`RouteTable` → `Vec<FlatRoute>`. Expands `resources :articles` into
the seven scaffold actions, composes nested paths
(`articles/:article_id/comments/:id`), and computes the `as_name`
helper (`edit_article` → `edit_article_path`).

### `src/lower/persistence.rs`

Per-model SQL generation: INSERT / UPDATE / DELETE / SELECT-by-id /
SELECT-all / SELECT-last, column projection order, belongs_to
existence checks, and dependent-destroy cascade targets. SQLite-
specific today; a `Dialect` enum can later render Postgres/MySQL
without changing the consumer shape.

### `src/lower/broadcasts.rs`

Model `broadcasts_to` declarations → `LoweredBroadcasts`. Each
declaration produces a list of turbo-stream actions to emit on
create/update/destroy, scoped by the broadcast association.

### `src/lower/associations.rs`

`resolve_has_many` — target class + foreign key for an owner's
has_many reference. Prefers the owner's static type; falls back to
cross-model scan by association name.

### `src/lower/fixtures.rs`

YAML fixtures → structured load plan. Each record carries which
columns get literals vs. cross-fixture references; cross-fixture FK
references resolve through runtime lookup keyed on
`(target_fixture, target_label)`.

### `src/lower/erb_trim.rs`

ERB whitespace normalization — runs before `view_to_library` so the
view lowerer sees a stable-shaped template tree regardless of which
ERB trim style the source used.

### `src/lower/controller_test.rs`

Same lift pattern as legacy controller lowering, applied to
controller-test bodies. Classifies `get`/`post`/`assert_select`/
`assert_response` shapes into a `ControllerTestSend` enum.

## Key files

| File | Role |
|------|------|
| `src/lower/mod.rs` | Module layout + re-exports |
| `src/lower/model_to_library/` | Model dialect → `LibraryClass` |
| `src/lower/view_to_library/` | ERB view → `LibraryClass` |
| `src/lower/controller_to_library/` | Controller dialect → `LibraryClass` |
| `src/lower/controller.rs`, `controller_walk.rs` | Legacy per-target derivation (being torn down) |
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
  emitter shape that consumes `LibraryClass`.
- [`../data/catalog.md`](../data/catalog.md) — the AR method
  classification that some lowerings consult.
