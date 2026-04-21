# Method catalog

The method catalog is the IDL-shaped single source of truth for what
the compiler knows about framework method surfaces — ActiveRecord
today; view helpers, controller helpers, and the route DSL over time.
Each entry captures the facets every consumer needs: identity, effect
class, chain semantics, and return-type shape.

**Source:** `src/catalog/mod.rs` (`CatalogedMethod`, `AR_CATALOG`).

## Why it exists

Before the catalog, knowledge about ActiveRecord methods was scattered
across five places:

1. `SqliteAdapter::classify_ar_method` — effect classification.
2. `Analyzer::new` `class_methods` HashMap — return types.
3. `lower::controller::is_query_builder_method` — chain semantics
   (terminal vs. builder).
4. Hand-coded emitter templates — per-target emission shapes.
5. Per-target runtime stubs — actual implementations.

Adding a new AR method meant editing N places, and drift was
inevitable. One source now; consumers read from it.

## Entry shape

```rust
pub struct CatalogedMethod {
    pub name: &'static str,
    pub receiver: ReceiverContext,   // Class | Instance
    pub effect: EffectClass,         // DbRead | DbWrite | Pure
    pub chain: ChainKind,            // Terminal | Builder | NotApplicable
    pub return_kind: Option<ReturnKind>,
}
```

Each facet exists for a specific consumer:

| Facet | Consumed by | Why |
|-------|-------------|-----|
| `name` + `receiver` | All consumers | Identity. Same method on class vs. instance can mean different things — `find` on `User` vs. on `user.posts`. |
| `effect` | Analyzer's effect inference (via `DatabaseAdapter`) | Attaches `DbRead(table)` / `DbWrite(table)` to the Send's effect set. |
| `chain` | Controller walker, pre-emit lowering passes | `Builder`-marked calls (`where`, `limit`, `order`) don't attach DB effects; only the terminal step does. Also drives whether `await` appears under async adapters. |
| `return_kind` | Analyzer's type inference | Seeded into each model's `class_methods` / `instance_methods` registry at analyzer init. `ArrayOfSelf` for `Article` becomes `Ty::Array<Ty::Class(Article)>`. |

### `ReceiverContext`

- `Class` — called on the model class (`User.find(1)`).
- `Instance` — called on a model instance (`user.save`).

(Relation and Association contexts will land when the analyzer grows
`Relation<T>` / `Association<T>` type kinds. Today's catalog stops at
the two the analyzer can detect.)

### `EffectClass`

- `DbRead` — executes a SELECT-equivalent.
- `DbWrite` — executes INSERT / UPDATE / DELETE.
- `Pure` — in-memory only. `Model.new` constructs an instance without
  touching the DB; `.save` on that instance is the first write.

### `ChainKind`

- `Terminal` — executes the query (`all`, `find`, `first`, `to_a`,
  `count`, `pluck`).
- `Builder` — extends the query without executing (`where`, `limit`,
  `order`, `includes`, `joins`).
- `NotApplicable` — writes, and reads that aren't part of a relation
  chain.

**Current simplification:** most methods are marked `Terminal` because
the emitter doesn't yet distinguish builder from terminal calls and
`SqliteAdapter` (the only current adapter) is sync, so the distinction
is unobservable. When async adapters and `Relation<T>` typing land,
`Builder`-marked methods stop producing DbRead effects on their own
— the Relation accumulates them and the `Terminal` step emits them.

### `ReturnKind`

One of: `SelfType`, `ArrayOfSelf`, `SelfOrNil`, `Int`, `Bool`,
`HashSymStr`, `ClassRef("…")`. Each is parametric on the receiver's
Self type; the analyzer instantiates them per-model at init.

`None` means "not declared" — the analyzer doesn't populate a
signature entry and downstream type inference produces `Ty::Var(0)`.
That's the graceful-fallback contract.

## Current coverage

~45 AR methods — factory methods (`new`, `create`, `create!`, `build`),
class reads (`find`, `find_by`, `all`, `where`, `first`, `last`,
`count`, `exists?`, `pluck`, `limit`, `order`, `includes`, `joins`),
instance writes (`save`, `save!`, `destroy`, `destroy!`, `update`,
`update!`, `update_attribute`), attribute accessors (`attributes`,
`persisted?`, `new_record?`, `valid?`, `errors`).

Grep `src/catalog/mod.rs` for `AR_CATALOG` to see the full table with
per-entry comments.

## What the catalog is *not*

- **Not an external DSL.** Entries live as Rust code (a static table).
  If/when externalization is needed (gem-author RBS files, user
  annotations), a parser will populate the same `CatalogedMethod`
  struct. The in-code form stays authoritative.
- **Not a type system.** The analyzer still owns type inference; the
  catalog just declares what's available for dispatch.
- **Not a capability profile.** Adapters declare *which* catalog
  entries they support (e.g. an IndexedDB adapter may not support
  `pluck` with an arbitrary column). The catalog itself is adapter-
  neutral.

## Extending the catalog

Adding a new AR method:

1. Add one entry to `AR_CATALOG` in `src/catalog/mod.rs`. Fill in
   `name`, `receiver`, `effect`, `chain`. Set `return_kind` to
   `Some(...)` if you want the analyzer to type it; `None` is a valid
   placeholder.
2. Don't touch anything else. Consumers read through the single
   source:
   - Adapter effect classification — `SqliteAdapter` (`src/adapter.rs`)
     delegates to catalog lookup.
   - Analyzer class/instance method registries — built from
     `AR_CATALOG` at `Analyzer::with_adapter` time.
   - Controller walker chain detection — `is_query_builder_method`
     checks `chain == ChainKind::Builder`.
3. Run the tests. If a new fixture exercises the method, the
   round-trip and toolchain tests will confirm the emission path.

## Future growth

The catalog shape is designed to grow. Expected facets (not yet
added):

- **Per-target runtime symbol maps.** Today emitters hand-render each
  method; a `render: RenderTable` facet would let each target specify
  its output shape in the catalog entry and remove the per-target
  dispatch in `src/emit/*.rs`.
- **Capability gates.** A `requires: CapabilitySet` facet so adapters
  can advertise which entries they support, and diagnostics fire
  before emission when an emitted project wouldn't work.
- **Non-AR surfaces.** View helpers (`form_with`, `link_to`,
  `render`), controller helpers (`render`, `redirect_to`, `head`),
  and the route DSL all fit the same shape. Extensions land in
  sibling tables (`VIEW_HELPERS_CATALOG`, `ROUTES_DSL_CATALOG`) or as
  sections of a unified table once the shape stabilizes.

## Key files

| File | Role |
|------|------|
| `src/catalog/mod.rs` | `CatalogedMethod`, `AR_CATALOG`, `ReceiverContext`, `EffectClass`, `ChainKind`, `ReturnKind` |
| `src/adapter.rs` | `SqliteAdapter` / `SqliteAsyncAdapter` — consume the catalog for effect classification |
| `src/analyze.rs` | `Analyzer::with_adapter` seeds method registries from the catalog |
| `src/lower/controller.rs` | `is_query_builder_method` — chain classification consumer |

## Related docs

- [`adapter.md`](adapter.md) — `DatabaseAdapter` trait; the catalog's
  primary consumer.
- [`../pipeline/analyze.md`](../pipeline/analyze.md) — how method
  signatures flow into type inference.
