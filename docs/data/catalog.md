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
    pub receiver: ReceiverContext,   // Class | Instance | Relation
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
- `Relation` — called on a `Ty::Relation`-typed receiver
  (`Story.recent.where(...)`, `tag.stories.order(...)`). A large
  share of the catalog lives in this context — the chain-builder
  and terminal surface of the lazy query builder.

The key semantic rule for `Relation` entries (documented on the
variant in `src/catalog/mod.rs`): "Self" in a `ReturnKind` denotes
the relation's *element* model (`Relation { of }`'s `of`), so
`SelfOrNil` reads "element or nil" (`first`/`take`) and
`ArrayOfSelf` reads "materialized array of the element" (`to_a`).
Association reads deliberately fold into this context rather than
getting their own: an association read *is* a relation whose base
predicate is the FK match (`tag.stories` ≡
`Story.where(tag_id: tag.id)` modulo join tables), so the method
surface callable on it is exactly the relation surface. If a
CollectionProxy-only method (`tag.stories << story`) ever needs
cataloging, that's the moment to revisit — not before.

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

Entries are spread across all three kinds — the query-builder
surface is largely `Builder`, terminals and aggregates are
`Terminal`, and writes plus non-chain reads are `NotApplicable`
(read `AR_CATALOG` for the per-entry classification). The
distinction is live and observable: effect inference
(`src/analyze/effects.rs`) skips effect attachment for
`Builder`-marked reads, so the Relation accumulates the query and
only the `Terminal` step carries the `DbRead` — which is also where
`await` lands under an async adapter (`SqliteAsyncAdapter`), instead
of one spurious round-trip per chain link.

### `ReturnKind`

Representative variants: `SelfType`, `ArrayOfSelf`, `SelfOrNil`,
`RelationOfSelf` (an unmaterialized query preserving the element
model — what `Builder`-chain methods declare), `Int`, `Bool`,
`ClassRef("…")`, and `Untyped` (the gradual escape for values that
can't be shaped without argument analysis). The enum is larger than
this list and grows with the catalog — read `ReturnKind` in
`src/catalog/mod.rs` for the full set with per-variant rationale.
Each is parametric on the receiver's Self type; the analyzer
instantiates them per-model at init.

`None` means "not declared" — the analyzer doesn't populate a
signature entry and downstream type inference produces `Ty::Var(0)`.
That's the graceful-fallback contract.

## Current coverage

The bulk of the everyday AR surface — factory methods (`new`,
`create`, `create!`, `build`), class and relation reads (`find`,
`find_by`, `all`, `where`, `first`, `last`, `count`, `exists?`,
`pluck`, `limit`, `order`, `includes`, `joins`, and the rest of the
chain-builder surface), instance writes (`save`, `save!`, `destroy`,
`destroy!`, `update`, `update!`), attribute accessors (`attributes`,
`persisted?`, `new_record?`, `valid?`, `errors`). Counts rot; read
`AR_CATALOG` in `src/catalog/mod.rs` for the full table with
per-entry comments.

Consumers reach the table through a small lookup API in
`src/catalog/mod.rs`: `lookup(name, receiver)` for the single
`(name, receiver)` entry, `lookup_any(name)` for all entries across
receiver contexts, and `receivers_for(name)` for the set of contexts
a name appears under.

## The gem catalog (`src/catalog/gems.rs`)

`AR_CATALOG` has a sibling: `GEM_CATALOG` in `src/catalog/gems.rs`
(re-exported from `src/catalog/mod.rs`), covering the third-party
gem surface real apps call — Faker, Nokogiri, Mail, ROTP, Arel, and
so on. Its module doc states a deliberately different philosophy
from the AR catalog: coverage grows by **discovery**, not
enumeration — when a real app surfaces a call the analyzer can't
resolve, the gem's signature lands here. Entries carry a concrete
return type where one is knowable and the gradual escape (`Untyped`)
for gem objects we don't model structurally; either way the dispatch
*resolves* — never a hard `send_dispatch_failed`. `Untyped` is the
floor, not the ceiling: an entry is free to declare a precise type
the moment one is worth modeling. The entries are registered into
the analyzer's class registry by `src/analyze/registry/stdlib.rs`
(via `register_stdlib_class`, so a user class of the same name
always wins).

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
2. For most entries that's it — consumers read through the single
   source: adapter effect classification (`SqliteAdapter` in
   `src/adapter.rs` delegates to catalog lookup) and the analyzer's
   class/instance method registries (built from `AR_CATALOG` at
   `Analyzer::with_adapter` time). But two documented caveats mean
   "one entry" isn't always the whole job:
   - **Relation-context entries don't reach the adapter.**
     `SqliteAdapter::classify_ar_method` deliberately filters out
     `ReceiverContext::Relation` entries (see the comment at the
     lookup in `src/adapter.rs`): the trait's lookup is
     receiver-blind, and it must not start classifying names that
     only exist under the Relation context (`to_a`, `page`, `merge`,
     …) — that would attach effects to Sends the analyzer doesn't
     yet type as relations.
   - **Chain classification is a separate hand-curated list.**
     `is_query_builder_method` (`src/catalog/mod.rs`) is an explicit
     `matches!` list, and its doc comment warns against "simplifying"
     it into a catalog `chain ∈ {Builder, Terminal}` lookup — that
     catalog set is a strict *superset* of the list, and deriving the
     predicate from it would reclassify Sends as query chains (a
     behavior change, not a refactor). If your new method should mark
     a Send chain as a query-builder chain, it needs an entry in that
     list too; the unit test `query_builder_methods_are_all_cataloged`
     guards the one invariant that does hold (everything in the list
     has a Builder/Terminal catalog entry).
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
| `src/catalog/mod.rs` | `CatalogedMethod`, `AR_CATALOG`, `ReceiverContext`, `EffectClass`, `ChainKind`, `ReturnKind`, lookup API |
| `src/catalog/gems.rs` | `GEM_CATALOG`, `GemClass`, `GemTy` — the sibling gem-ecosystem catalog |
| `src/adapter.rs` | `SqliteAdapter` / `SqliteAsyncAdapter` — consume the catalog for effect classification |
| `src/analyze/mod.rs` | `Analyzer::with_adapter` seeds method registries from the catalog |
| `src/analyze/registry/stdlib.rs` | Registers `GEM_CATALOG` entries into the analyzer's class registry |
| `src/lower/controller/` | Pre-emit lowering passes that consult chain classification (`is_query_builder_method` lives in `src/catalog/mod.rs`) |

## Related docs

- [`adapter.md`](adapter.md) — `DatabaseAdapter` trait; the catalog's
  primary consumer.
- [`../pipeline/analyze.md`](../pipeline/analyze.md) — how method
  signatures flow into type inference.
