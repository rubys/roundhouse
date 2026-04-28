# Emit

Emitters turn the analyzed + lowered IR into target-language source
files. Each emitter is pure: `fn emit(app: &App) -> Vec<EmittedFile>`.
The caller (test harness, CLI, site builder) decides where to write
the result.

**Source:** `src/emit/` — one file per target.

## The contract

```rust
pub struct EmittedFile {
    pub path: PathBuf,
    pub content: String,
}

// Each target exposes:
pub fn emit(app: &App) -> Vec<EmittedFile>;
```

No I/O, no filesystem touching, no target-specific config threading
through the IR. The emitter reads `app.models`, `app.controllers`,
`app.views`, `app.routes`, `app.fixtures`, `app.seeds`, plus the
lowered forms it computes on demand (`lower_action`,
`lower_persistence`, `flatten_routes`, etc.), and produces a
`Vec<EmittedFile>` ready to drop onto disk.

## Current target status

All seven runnable targets pass the DOM-equivalence compare against
Rails on real-blog as a CI invariant — `.github/workflows/ci.yml`'s
`compare-<target>` jobs gate the Pages deploy, so any drift fails
the build.

| Target | Status | Notes |
|--------|--------|-------|
| **Ruby** | Source-equivalent for tiny-blog + most of real-blog | The round-trip identity partner; paired with ingest |
| **Rust** | Runnable end-to-end; DOM-equivalent | Boots axum HTTP + Action Cable stub, forms, validation, Turbo, Tailwind |
| **TypeScript** | Runnable end-to-end; DOM-equivalent | Node HTTP + Action Cable over WebSockets, better-sqlite3, Juntos-shape |
| **Go** | Runnable end-to-end; DOM-equivalent | Pass-2 HTTP router |
| **Crystal** | Runnable end-to-end; DOM-equivalent | Same shape as Go |
| **Elixir** | Runnable end-to-end; DOM-equivalent | Module-function conversion happens inside the emitter |
| **Python** | Runnable end-to-end; DOM-equivalent | Async emission uses `SqliteAsyncAdapter` |
| **Spinel** | Runnable end-to-end via CRuby; DOM-equivalent | Spinel-subset Ruby executed by CRuby until Spinel grows the surface roundhouse emits (test asymmetry: emit-app-only, run hand-written tests; see `project_spinel_test_asymmetry`) |

## Emitter anatomy

Every non-Ruby emitter follows the same shape:

1. **Embed the per-target runtime.** Hand-written files under
   `runtime/<target>/` get `include_str!`-copied (Rust) or
   equivalent into the output. Covers: DB connection, HTTP server,
   view helpers, Action Cable glue, test support. See
   [`runtime.md`](runtime.md).

2. **Emit per-model files.** Model class/struct declarations,
   validation render (one `if Check... { errors.push(...) }` per
   lowered `Check`), per-model persistence methods
   (`find`/`all`/`save`/`destroy`), association methods.

3. **Emit per-controller files.** For each controller:
   - Implement `CtrlWalker` as a small struct holding `WalkCtx` +
     `WalkState`.
   - Provide leaf `write_*` / `render_*` methods in the target's
     idiomatic syntax.
   - Call `walk_action_body` for each action and splice the result
     into a function body.

4. **Emit the router.** `flatten_routes(app)` → flat dispatch table
   in target syntax.

5. **Emit views.** Each ERB template's already-compiled Ruby IR gets
   re-rendered as a target-language function that builds a string.
   Static text becomes literal fragments; `<%= expr %>` interpolation
   becomes `render_expr(...)` calls.

6. **Emit the schema DDL.** `lower_schema(app.schema)` as a string
   constant the target's DB init reads.

7. **Emit the project shell.** `Cargo.toml` / `package.json` /
   `mix.exs` / `go.mod` / `requirements.txt` / `shard.yml`, plus
   `main.rs` / `main.ts` / `application.ex` / `main.go` / …

## The Ruby emitter is different

`src/emit/ruby.rs` pairs with `src/ingest.rs` as the round-trip
forcing function. Its job is to produce source byte-for-byte
identical to the ingest input — that's what validates no information
was lost. It skips all the runtime/server/shell machinery the other
emitters do: no DDL emission, no router, no project shell. Just
"re-emit the Ruby that went in."

This is why `src/emit/ruby.rs` is small and the other emitters are
large: they're doing different jobs.

## Each emitter implements `CtrlWalker`

The controller walker (`src/lower/controller_walk.rs::CtrlWalker`)
is where ~60% of per-target emitter work lives. The trait's default
`walk_stmt` provides the dispatch tree; each emitter's impl fills in
the leaves:

```rust
pub struct RsEmitter<'a> { /* ... */ }

impl<'a> CtrlWalker<'a> for RsEmitter<'a> {
    fn ctx(&self) -> &WalkCtx<'a> { &self.ctx }
    fn state_mut(&mut self) -> &mut WalkState { &mut self.state }
    fn indent_unit(&self) -> &'static str { "    " }

    fn write_assign(&mut self, name: &str, value: &Expr, indent: &str, out: &mut String) {
        let rendered = self.render_expr(value);
        writeln!(out, "{indent}let {name} = {rendered};").unwrap();
    }
    // ...and so on.
}
```

Adding a new target becomes: write the seven leaf methods, plus the
per-model and router rendering. The walker's shared dispatch handles
control flow.

## Async emission

Emitters that support async receive the active `DatabaseAdapter`
through `WalkCtx::adapter`. At each Send site,
`WalkCtx::expr_suspends(expr)` checks the expression's effect set
against `adapter.is_suspending_effect`. If any effect suspends, the
emitter prepends (TS: `"await "`) or appends (Rust: `".await"`) the
target's suspension marker.

- **TypeScript** emits `await` prefixes under `SqliteAsyncAdapter`.
  Emitted awaits are no-ops against `better-sqlite3` (sync) but
  validate the plumbing for real async backends later.
- **Rust** doesn't yet consume suspension — today's Rust emit is
  synchronous even under the async adapter. When rusqlite grows an
  async wrapper (or a Neon/Postgres-tokio adapter lands), the
  emitter will need a `.await` postfix hook; today's `CtrlWalker`
  trait would grow either `suspending_postfix` or
  `wrap_suspending(String) -> String` to accommodate.

Sync emitters (Ruby, Crystal, Elixir, Go) implement
`suspending_prefix()` returning `""` — nothing suspends in their
emission model, regardless of adapter.

## Per-target type rendering

Each emitter has a `ty.rs` (`src/emit/<target>/ty.rs`) that lowers
analyzer-produced `Ty` values to target syntax. The interesting
rows are the "special" variants:

| `Ty` | Rust | TypeScript | Python | Crystal | Go |
|------|------|------------|--------|---------|----|
| `Var` | `()` | `unknown` | `object` | `_` | `interface{}` |
| `Untyped` (RBS gradual) | `()` | `any` | `Any` | `_` | `interface{}` |
| `Bottom` (raise/return) | `!` | `never` | `Never` | `NoReturn` | `interface{}` |

`Var` represents an inference gap; an emitter rendering it implies
the analyzer didn't fully resolve a type. `Untyped` is an
author-signed gradual escape (RBS `untyped`); permissive targets
(TS `any`, Python `Any`) accept it cleanly while strict targets
(Rust, Go) are expected to elevate `GradualUntyped` warnings to
emit-time errors via the diagnostic pipeline. `Bottom` is filtered
out of unions during analysis (`union_of` / `union_many`), so
emitters typically only see it in fully-divergent positions — Rust's
`!` and TS's `never` are the natural targets.

## Keeping emitters honest

Several forcing functions gate emitter correctness:

| Test | What it catches |
|------|-----------------|
| `tests/emit_<target>.rs` (default) | Snapshot tests per emit pathway — catches accidental output drift |
| `tests/real_blog.rs::expected_files_round_trip_byte_for_byte` (Ruby) | Ingest → emit-ruby → inputs match |
| `tests/<target>_toolchain.rs` (`--ignored`) | Actually invoke `cargo build` / `tsc` / `go build` / `mix compile` on the emitted project |
| `tests/emit_rust.rs::boots_real_blog_server` | For Rust: start the emitted binary, issue HTTP requests, check responses |
| `roundhouse-compare` | Cross-runtime DOM equivalence (see [`verification.md`](verification.md)) |

## Key files

| File | Role |
|------|------|
| `src/emit/mod.rs` | `EmittedFile` + module layout |
| `src/emit/ruby.rs` | Round-trip identity partner |
| `src/emit/rust.rs` | Rust emitter — runs axum + rusqlite |
| `src/emit/typescript.rs` | TypeScript emitter — Juntos-shape, node http |
| `src/emit/go.rs` | Go emitter |
| `src/emit/crystal.rs` | Crystal emitter |
| `src/emit/elixir.rs` | Elixir emitter |
| `src/emit/python.rs` | Python emitter |

## Related docs

- [`lower.md`](lower.md) — the lowered forms emitters consume.
- [`runtime.md`](runtime.md) — the per-target glue libraries emitters
  embed.
- [`verification.md`](verification.md) — how we validate the emitted
  output is correct.
