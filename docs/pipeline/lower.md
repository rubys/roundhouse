# Lower

The lowering layer sits between analyze and emit. Its job: take
Rails-dialect IR (validations, associations, routes, controller
bodies) plus the analyzer's annotations, and produce **target-
neutral** forms that all seven emitters can consume.

**Source:** `src/lower/` — one file per lowering concern.

## Why lower?

Before this layer existed, each target emitter independently
re-implemented the same analysis: "what are the SQL strings for this
model's persistence?", "how do I walk a controller body deciding
what's a render vs. a Send?", "how do I translate a validation rule
to a runtime check?". Per-target copies, slight drift, large
maintenance surface.

The architectural bet in Phase 4 is: **lower once, render N ways**.
Extract the logic that's identical across targets (SQL generation,
validation evaluation, route dispatch tables, controller-body walk
skeleton) as IR-level lowerings. Each emitter consumes the lowered
form and renders it in target-specific code. Adding a new target
becomes "write renders" rather than "re-implement the logic."

## The lowering passes

| Pass | Source | Output |
|------|--------|--------|
| `lower_validations` | Model validations | `Vec<LoweredValidation>` — each attribute with its expanded `Check` enum list |
| `lower_schema` | Schema | Single `String` of CREATE TABLE DDL (SQLite today) |
| `lower_persistence` | Model + Schema | `LoweredPersistence` with INSERT/UPDATE/DELETE/SELECT strings, `belongs_to` checks, dependent-destroy cascades |
| `resolve_has_many` | Model associations | `HasManyRef` (target class + foreign key) |
| `flatten_routes` | `RouteTable` | `Vec<FlatRoute>` (one entry per `(method, path, controller, action)`) |
| `lower_fixtures` | YAML fixtures | `LoweredFixtureSet` (per-record plan) |
| `lower_action` | Controller action | `LoweredAction` (classified, normalized body) |
| Pre-emit passes | Controller body | Rewritten `Expr` (see below) |

Each pass is pure: same input → same output, no side effects, no
target awareness. Emitters consume them via their public return
types; re-exports live in `src/lower/mod.rs`.

## Pre-emit lowering passes

Three passes rewrite the controller-body `Expr` tree to a
normalized form every emitter sees. Lifted into `src/lower/` so the
emitters don't each re-discover the same rewrites:

### `synthesize_implicit_render`

Rails actions frequently end without an explicit `render` — the
framework supplies one implicitly from the action name (`index` →
`render :index`). This pass detects bodies that lack a trailing
response terminal and appends the synthesized render call, so every
downstream pass sees a uniform "body ends with a response" shape.

### `unwrap_respond_to`

`respond_to do |format| format.html; format.json end` blocks get
collapsed to the HTML branch (the only format every target emits
today). Keeps the controller walker from needing to special-case
format dispatch; JSON/Turbo Stream formats will re-enter as
separate rendered outputs once a second format matters.

### `resolve_before_actions`

`before_action :set_article` doesn't produce any IR of its own in
the action body — it runs a method that assigns an ivar. This pass
inlines the before-action's effect (typically `@article =
Article.find(params[:id])`) at the top of each action it covers, so
the action body becomes self-contained: every ivar it reads has a
visible assign in the body.

## The controller walker

**Source:** `src/lower/controller_walk.rs`.

The controller walker is the single largest piece of lift-into-lower
work. Every target's emitter used to have its own parallel
implementation of "walk a controller body, classify each statement,
emit target syntax." The ten-line dispatch tree (Seq / Assign with
Create-pattern or default / If with Update-pattern or default / Send
via render table / other via expr) was structurally identical across
all six targets; only the emitted syntax differed.

### The `CtrlWalker` trait

```rust
pub trait CtrlWalker<'a>: Sized {
    fn ctx(&self) -> &WalkCtx<'a>;
    fn state_mut(&mut self) -> &mut WalkState;
    fn indent_unit(&self) -> &'static str;

    // Leaf rendering — per-target syntax:
    fn write_assign(...);
    fn write_create_expansion(...);
    fn write_if(...);
    fn write_update_if(...);
    fn write_response_stmt(...);
    fn write_expr_stmt(...);
    fn render_expr(...) -> String;
    fn render_send_stmt(...) -> Option<Stmt>;
    fn suspending_prefix(&self) -> &'static str { "" }

    // Shared dispatch — provided by default impl:
    fn walk_action_body(&mut self, body: &Expr) -> String;
    fn walk_stmt(&mut self, expr: &Expr, out: &mut String, depth: usize, is_tail: bool);
}
```

The default `walk_stmt` provides the entire dispatch — targets don't
override it. They only implement the leaf `write_*` / `render_*`
methods for their idiomatic syntax.

### `WalkCtx` and `WalkState`

- **`WalkCtx`** is the target-neutral context bundle: known models,
  model class name, resource name, nested-parent (for nested
  resources), permitted strong-param fields, the active
  `DatabaseAdapter`. Borrowed from the enclosing `LoweredAction`.
- **`WalkState`** is mutable walker state: whether the render table
  touched `context.*`, the last-bound local's name, and whether it
  came from a Create expansion (Elixir's post-save id rebind gates
  on this).

### `WalkCtx::expr_suspends`

Async-capable emitters call this at each Send site. It checks the
expression's effect set against `adapter.is_suspending_effect`. If
any effect suspends, the emitter emits the target's suspending
prefix (`"await "` / `"await "` / `".await"`) at that site.

Sync emitters simply ignore the adapter — their
`suspending_prefix()` override returns `""` unconditionally.

## `SendKind` — the shared send classifier

**Source:** `src/lower/controller.rs::classify_controller_send`.

Controller bodies are dominated by `Send` expressions:
`redirect_to(...)`, `render(...)`, `params.require(:x)`,
`Article.find(id)`, `@article.update(article_params)`. Each emitter
needs to match these same shapes; `SendKind` classifies them once.

```rust
pub enum SendKind<'a> {
    Render { ... },
    RedirectTo { ... },
    Head { ... },
    StrongParams { ... },
    ModelFind { ... },
    ModelAll,
    ResourceParams { ... },
    Save { ... },
    Destroy { ... },
    Update { ... },
    // ...plus more
    Unknown,  // falls through to generic expr render
}
```

Variants live here when the shape appears in at least three of the
four Phase-4c emitters (Rust, Crystal, Go, Elixir) — validation that
they're shape-shaped, not target-shaped. Target-specific rewrites
(Elixir's struct-method-to-Module-function conversion) stay in the
emitter.

## Other lowering modules

### `src/lower/validations.rs`

Expands surface `Validation` rules into flat `Check` entries. A
source `Length { min: 10, max: 100 }` lowers to two checks
(`MinLength` then `MaxLength`), so the per-target render doesn't
carry optional-bound logic. Each check has a default error message
plus emitter-specific render forms — see the render table in the
module doc.

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

### `src/lower/schema_sql.rs`

Schema → CREATE TABLE DDL (one call; all tables; joined with blank
lines). Primary keys use SQLite's `INTEGER PRIMARY KEY
AUTOINCREMENT` so rowids stay stable across inserts.

### `src/lower/associations.rs`

`resolve_has_many` — target class + foreign key for an owner's
has_many reference. Prefers the owner's static type; falls back to
cross-model scan by association name.

### `src/lower/fixtures.rs`

YAML fixtures → structured load plan. Each record carries which
columns get literals vs. cross-fixture references; cross-fixture FK
references resolve through runtime lookup keyed on
`(target_fixture, target_label)`.

### `src/lower/controller_test.rs`

Same lift pattern as `controller.rs`, applied to controller-test
bodies. Classifies `get`/`post`/`assert_select`/`assert_response`
shapes into a `ControllerTestSend` enum; per-target emitters render
each variant.

## How emitters consume the lowered form

Each target emitter is structured as:

1. Run the lowering passes once per app (or per model/controller).
2. Implement `CtrlWalker` to render each action body.
3. Render each `LoweredValidation` using the target's syntax for the
   `Check` variants.
4. Render each `FlatRoute` as an entry in the target's router table.
5. Embed `lower_schema(...)` and per-model `LoweredPersistence` SQL
   strings as constants in the generated project.

The emitter file size is now dominated by *rendering* — the
analysis it used to carry has mostly moved to `lower/`.

## Key files

| File | Role |
|------|------|
| `src/lower/mod.rs` | Module layout + re-exports |
| `src/lower/controller.rs` | `SendKind`, `lower_action`, `LoweredAction`, pre-emit passes |
| `src/lower/controller_walk.rs` | `CtrlWalker` trait + shared dispatch |
| `src/lower/controller_test.rs` | Test-body walker |
| `src/lower/validations.rs` | `LoweredValidation`, `Check` enum |
| `src/lower/routes.rs` | `flatten_routes`, `FlatRoute` |
| `src/lower/persistence.rs` | `LoweredPersistence` |
| `src/lower/schema_sql.rs` | DDL generator |
| `src/lower/fixtures.rs` | Fixture load plan |
| `src/lower/associations.rs` | has_many resolution |

## Related docs

- [`analyze.md`](analyze.md) — the typed IR that lowering consumes.
- [`emit.md`](emit.md) — the per-target consumers of lowered forms.
- [`../data/catalog.md`](../data/catalog.md) — the AR method
  classification that some lowerings consult.
