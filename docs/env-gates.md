# Environment-variable gates

Every `ROUNDHOUSE_*` environment variable the codebase reads, what it does, and
(for the migration toggles) when it can be deleted. Regenerate the authoritative
list with:

```sh
grep -rhoE 'ROUNDHOUSE_[A-Z0-9_]+' src/ scripts/ | sort -u
```

A variable is **live** if a `std::env::var(...)` (Rust) or `${VAR:-…}` / `[[ "$VAR" … ]]`
(shell) actually reads it. Several `_V1`/`_V2`/`_LEGACY` names now appear **only in
comments** — their gate was removed when the new path became unconditional; those are
flagged *vestigial (remove)* below.

## Build / analysis inputs (permanent)

| Var | Default | Read at | Effect |
|-----|---------|---------|--------|
| `ROUNDHOUSE_APP_ROOT` | working directory | `src/mcp.rs:77` | MCP server's app root when `argv[1]` is absent. |
| `ROUNDHOUSE_ASSETS_DIR` | unset → no injection | `src/project.rs:859` | Directory of pre-compiled assets injected into the site archive as `static/assets/*` (the build-site CI job compiles once, then points every target's archive at it). |
| `ROUNDHOUSE_INGEST_SURVEY` | `0` (off; strict path) | `src/bin/roundhouse-check.rs:36` | `=1` (or `--continue`) activates survey mode: ingest keeps going past per-file errors instead of failing fast. |

## Emitted-app runtime (not a roundhouse gate)

| Var | Default | Read at | Effect |
|-----|---------|---------|--------|
| `ROUNDHOUSE_BASE` | `/` | emitted TS: `src/emit/typescript.rs:1272` | Base URL path baked into the TypeScript target's router (`process.env.ROUNDHOUSE_BASE`, trailing slash required, e.g. `/roundhouse/blog/`). Read by the *generated* app at its own runtime, not by roundhouse. |

## Feature flags (permanent, opt-in/opt-out)

| Var | Default | Read at | Effect |
|-----|---------|---------|--------|
| `ROUNDHOUSE_PARAM_BINDS` | `0` (off) | `src/lower/arel/visitor.rs:51` | `=1` emits the placeholder-bind form for `Db.prepare` read paths (prototype; default-off pending lobsters spinel hit-rate measurement). |
| `ROUNDHOUSE_RUST_V2_EMIT_TESTS` | on (only `=0` disables) | `src/emit/rust2.rs:1148` | Opt-*out* for emitting the rust2 target's test files. |

## Migration toggles (temporary — track for removal)

The rule for these: they exist to keep an old code path reachable during a
strangler-fig migration. Once the new path is unconditional and the old files are
deleted, the toggle has nothing to fall back to and should be removed along with any
comments that reference it.

| Var | Default | State | When to remove |
|-----|---------|-------|----------------|
| `ROUNDHOUSE_ELIXIR_V1` | `0` | **live** (`scripts/compare:131,154,512`) — selects the legacy `App.Main.run` entry point and drops the `.json` paths. | Delete when the v1 Elixir app shell is removed (per the comment at `scripts/compare:129`). |
| `ROUNDHOUSE_RUST_V2` | — | **vestigial** — `rust::emit` delegates to `rust2::emit` unconditionally (`src/emit/rust.rs:400`); name survives only in comments. | Remove now; there is no v1 rust path left. |
| `ROUNDHOUSE_RUST_V2_LEGACY` | — | **vestigial** — the escape hatch retired with the legacy submodule (`src/emit/rust.rs:398`); comment-only. | Remove now. |
| `ROUNDHOUSE_GO_V2` | — | **vestigial** — `go::emit` delegates to `go2::emit_overlay_files` unconditionally and `scripts/compare:383` builds from `cmd/v2/` unconditionally (Phase 6 step 2, 2026-05-24); comment-only. | Remove now; scrub the `scripts/compare:344-348` comment. |
| `ROUNDHOUSE_GO_V2_MODELS` | — | **vestigial** — model/controller/view/test emit ships whenever the app has models (`src/emit/go2.rs:180`); comment-only. | Remove now. |

> The vestigial rows are documented (not deleted) deliberately: this catalog is a
> Phase 1 docs-only step. Removing the dead toggle references is a separate,
> behavior-neutral cleanup.
