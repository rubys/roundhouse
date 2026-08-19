# Database adapter

The `DatabaseAdapter` trait is the seam between effect inference and
backend-specific capability. Today's analyzer consults the adapter at
every ActiveRecord Send site to decide two things:

1. **Does this call carry a DB effect?** (`DbRead` / `DbWrite` /
   nothing.)
2. **Does that effect suspend under this backend?** (Drives whether
   emitters insert `await`.)

**Source:** `src/adapter.rs` (`DatabaseAdapter`, `ArMethodKind`,
`SqliteAdapter`, `SqliteAsyncAdapter`).

## The trait

```rust
pub trait DatabaseAdapter: Send + Sync {
    fn classify_ar_method(&self, method: &str) -> ArMethodKind;
    fn is_suspending_effect(&self, effect: &Effect) -> bool { false }
    fn async_seed_methods(&self) -> &'static [&'static str] { &[] }
}

pub enum ArMethodKind { Read, Write, Unknown }
```

`Send + Sync` so `Analyzer` can hold a boxed adapter and share it
across threads. The default `is_suspending_effect` returns `false`
uniformly — sync-backend adapters need no override.
`async_seed_methods` names the methods on the runtime AR adapter
object that this backend implements asynchronously — the seed set
for the async function-coloring pass (see below). The default empty
slice means "nothing is async"; sync backends leave it alone.

## Why backend-specific, not language-specific

Pick `SqliteAdapter` and you get SQLite semantics regardless of which
target language emits the generated project. `rusqlite`,
`better-sqlite3`, `exqlite`, `crystal-db`, `modernc.org/sqlite`,
stdlib `sqlite3` — every language's SQLite driver behaves the same at
the AR-method level, so one adapter covers them all.

The alternative — per-language adapters — would force us to re-
declare the same AR-method classification six times. The actual
dimension of variation is the **backend**: SQLite, Postgres,
IndexedDB, Cloudflare D1, Neon. Each plugs in as its own adapter
impl.

## Current adapters

### `SqliteAdapter`

- Accepts the full AR query-builder surface. Delegates to
  `crate::catalog::AR_CATALOG` for classification — anything the
  catalog marks `DbRead` → `ArMethodKind::Read`, `DbWrite` →
  `Write`, `Pure` → `Unknown` (no effect attached).
- **Filters out `ReceiverContext::Relation` entries before
  classifying.** The trait's lookup is receiver-blind (a method
  name, no receiver context), so — per the comment on the lookup in
  `src/adapter.rs` — it must not classify names that only exist
  under the Relation context (`to_a`, `page`, `merge`, …): that
  would attach effects to Sends the analyzer doesn't yet type as
  relations.
- `is_suspending_effect` stays at default `false`. Everything is
  synchronous.

### `SqliteAsyncAdapter`

- Same classification as `SqliteAdapter` — same catalog lookup,
  same method coverage.
- Overrides `is_suspending_effect` to return `true` for `DbRead` and
  `DbWrite`. Emitters that check suspension (TypeScript, eventually
  Rust + Python) will insert `await` at every AR call site.

**Why does `SqliteAsyncAdapter` exist if SQLite isn't actually async?**
It's the minimum-divergence second adapter — the plumbing test. The
underlying drivers (`better-sqlite3`, `rusqlite`, stdlib `sqlite3`)
are all sync; `await` of a ready value is a no-op. That means we can
validate the async-emission machinery against a backend we know works
before introducing a real async backend (IndexedDB, D1, Postgres-on-
Node) where the awaits become load-bearing. If the emitter can route
awaits through `SqliteAsyncAdapter` correctly, switching to a real
async backend becomes "new adapter impl, same emit path."

## How the analyzer uses it

1. `Analyzer::new(app)` uses `SqliteAdapter` by default —
   behavior-preserving for code that predates the adapter refactor.
2. `Analyzer::with_adapter(app, Box::new(...))` swaps in a different
   adapter when the caller knows which backend the generated project
   will ship against.
3. During the effect walk (`visit_effects` in
   `src/analyze/effects.rs`), each `Send` on an AR receiver hands its
   method name to `adapter.classify_ar_method(name)`. The result
   attaches `Effect::DbRead { table }` or `Effect::DbWrite { table }`
   — or nothing, if `Unknown`.

Which adapter is active comes from the deployment profile
(`DeploymentProfile` in `src/profile.rs`): its `adapter()` method
maps the profile's `Database` to an impl — `SqliteSync` →
`SqliteAdapter`, `SqliteAsync` → `SqliteAsyncAdapter`, and
`Postgres` / `D1` / `IndexedDb` currently resolve to
`SqliteAsyncAdapter` as a fallback (all three are async
sqlite-shaped at the AR-method level) until their dedicated
adapters land.

## How emitters use it

The controller walker (`src/lower/controller_walk.rs::WalkCtx`)
carries a reference to the active adapter. Async-capable emitters
call `WalkCtx::expr_suspends(expr)` at statement-level expression
sites — RHS of an assign, condition of an `if`, body of an
expression-statement — to decide whether `await` belongs in the
emitted output.

Which target/database pairings emit sync vs async code is a profile
question, not a fixed per-language list — see `Target` and
`Database` (with its `is_async()`) in `src/profile.rs`. Emitters for
sync profiles ignore the suspension bit entirely; async-capable
emitters consult it at every site.

## Async coloring

Suspension isn't only a per-call-site question — whole methods
change color. `SqliteAsyncAdapter::async_seed_methods` names the
Promise-returning methods on the runtime adapter/Db surface (`all`,
`find`, `exec`, `prepare`, …); the coloring pass in
`src/analyze/async_color.rs` marks those seeds `is_async = true` and
then propagates the flag transitively through the call graph to a
fixed point, so anything that calls an async method — directly or
through framework Ruby — becomes async itself. The TypeScript
emitter consumes the flag, declaring colored methods `async` and
inserting `await` at their call sites. Under a sync profile the seed
list is empty and the pass is a no-op — emit output is unchanged.

## Extending with a new adapter

Minimum viable adapter:

```rust
pub struct PostgresAdapter;

impl DatabaseAdapter for PostgresAdapter {
    fn classify_ar_method(&self, method: &str) -> ArMethodKind {
        // For now, accept the same catalog coverage as SqliteAdapter —
        // Postgres supports the full AR query builder.
        // Reject methods Postgres-but-not-SQLite can't express later.
        //
        // Keep the Relation-context exclusion (same as SqliteAdapter):
        // this lookup is receiver-blind, and Relation-only names
        // (`to_a`, `page`, `merge`, …) must not pick up effects here.
        for entry in crate::catalog::lookup_any(method)
            .filter(|e| e.receiver != crate::catalog::ReceiverContext::Relation)
        {
            match entry.effect {
                crate::catalog::EffectClass::DbRead => return ArMethodKind::Read,
                crate::catalog::EffectClass::DbWrite => return ArMethodKind::Write,
                crate::catalog::EffectClass::Pure => return ArMethodKind::Unknown,
            }
        }
        ArMethodKind::Unknown
    }

    // Override if the target uses an async Postgres driver:
    fn is_suspending_effect(&self, effect: &Effect) -> bool {
        matches!(effect, Effect::DbRead { .. } | Effect::DbWrite { .. })
    }
}
```

Adapters that refuse some AR methods — e.g., an `IndexedDbAdapter`
that can't express arbitrary `joins` — return `ArMethodKind::Unknown`
for those. Downstream, the diagnostic layer catches "this call site
had a receiver with a bound table, but the adapter couldn't classify
it" and surfaces the gap rather than silently emitting broken code.

## Future trait growth

Planned additions (not yet present — each lands when a consumer
demands it):

- **`supports_method(name, receiver) -> bool`** for richer
  diagnostics than "Unknown means no effect."
- **`async_suspending_effects() -> EffectSet`** for a declarative
  listing rather than per-effect polling.
- **`DbOpaque` handling** for raw-SQL / `connection.execute` sites
  that bypass the signature table.
- **Capability-gated classification** that takes receiver context so
  adapters can differentiate `User.find(1)` from `user.posts.find(1)`.

## Key files

| File | Role |
|------|------|
| `src/adapter.rs` | Trait + the two SQLite impls |
| `src/catalog/mod.rs` | The method table the adapters consult |
| `src/analyze/effects.rs` | Consumer: effect inference (`visit_effects`, the `classify_ar_method` call site) |
| `src/analyze/async_color.rs` | Consumer: async coloring — seeds from `async_seed_methods`, propagates `is_async` |
| `src/profile.rs` | `DeploymentProfile::adapter()` — which adapter a `Database` selects |
| `src/lower/controller_walk.rs` | Consumer: `await` emission decision |
| `src/effect.rs` | `Effect` enum (`DbRead`, `DbWrite`, `Io`, …) |

## Related docs

- [`catalog.md`](catalog.md) — the AR method table adapters classify
  against.
- [`../pipeline/analyze.md`](../pipeline/analyze.md) — effect
  inference pass that consumes the adapter.
- [`../pipeline/emit.md`](../pipeline/emit.md) — how emitters consume
  suspending-effect classifications.
