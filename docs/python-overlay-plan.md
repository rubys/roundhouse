# Python universal-IR overlay (the CtrlWalker retirement)

> **ACTIVE** — phases A–D landed 2026-08-19; Phase E's controller leg
> landed the same day: **`CtrlWalker` is deleted**
> (`src/lower/controller_walk.rs`, the per-artifact controller emit,
> the legacy `app/routes.py` module-handler registration, and
> `http.py`'s Router/`ActionContext`/`FormatRouter` are all gone —
> `http.py` is now just `ActionResponse`). Remaining = the
> per-artifact model/view/test emit, which still ships alongside the
> overlay's own models/views.

Python is the last emitter deriving controllers per-artifact through
`lower::CtrlWalker` (`src/emit/python/controller.rs` implements the
trait; `src/lower/controller_walk.rs` exists only for it). Every
other target consumes the universal post-lowering IR. This plan
migrates Python the same way go/elixir migrated — a strangler overlay
— after which `controller_walk.rs` is deleted and the "features land
once" invariant holds with no Python exception.

The port can't switch controllers alone: library-path controller
bodies reference the whole lowered world (`Db`-backed models with
`from_stmt`/`from_params`, `Views.<Resource>.*`, `RouteHelpers.*`,
the transpiled `ActionController::Base`). So the overlay carries
models + views + controllers + route helpers + dispatch together.

## What's already banked

- `LIVE_FRAMEWORK` ships the transpiled `app/action_controller_base.py`
  and `app/router.py` (plus flash/session/errors/active_record_base) —
  the framework side of the thin path is live.
- The walker-coverage bill is **paid**: the inventory gate
  (`tests/python_controller_library_emit.rs`) measured all-green on
  its first run — real-blog controllers 5/5, models 5/5, views 9/9,
  tiny-blog likewise. The remaining work is wiring, not coverage.
- `runtime/python/db.py`'s `class Db` was already written to the
  lowered IR's contract (`from_stmt(stmt)` protocol).

## Phases

- **A. Inventory forcing function — DONE.**
  `tests/python_controller_library_emit.rs`: lower both fixtures with
  the same call chain the Go emitter runs, drive every LC family
  through `emit::python::emit_library_class`, py_compile everything.
  Pinned all-green; a regression is loud.

- **B. Overlay assembly — DONE.**
  `src/emit/python/overlay.rs`: family files under `app/v2/`
  (models / views / controllers / route_helpers / dispatch), explicit
  imports (a star import would shadow the two `Base` classes), views
  merged per resource under a `Views` facade, models↔views cycle
  broken by the bottom-import trick, `dispatch.py` implementing the
  construct-seed-run-read cycle (crystal server.cr steps 4–5).
  Not wired into `emit()`; the test gate compiles the whole set.
  `module_funcs_to_library_class` moved to its shared home in
  `src/lower/` (was duplicated in the rust and go emitters).

- **C. Runtime wiring — dispatch leg DONE.** The driver tests
  (`*_overlay_serves_index`) boot an in-memory DB and serve
  `GET index` end-to-end through `app/v2/dispatch.py` on both
  fixtures. The run surfaced and fixed a chain of
  ship-but-unexercised runtime gaps, most in the Python walker:
  Ruby `Hash#fetch`/`key?` on Hash receivers now map to `[k]` /
  `.get(k, v)` / `in` (STATUS_CODES and `self.params` were
  AttributeErrors); the Action* framework namespaces collapse in
  Const paths (Kotlin's rule — `ActionDispatch.Session` was an
  undefined name); `Base64.strict_encode64`/`JSON.generate` map to
  the Python stdlib; ivars honor `SELF_REF` so module-singleton
  state (`@slots`) binds `cls`; a bare zero-arg send naming a method
  parameter is a parameter read, not an implicit-self call; the
  session/view_helpers units carry their stdlib imports;
  `Db.escape_int_list` joined the hand-written primitives; and
  `dispatch()` resets view slots per request like every other
  target's glue. The transpiled view_helpers ships at
  `app/v2/view_helpers.py` (the hand-written one stays for the
  legacy world until Phase E).
  Remaining wiring: route table + `http.py`/TestClient integration —
  translate the returned controller's `status`/`body`/`location`/
  `content_type` into `http.ActionResponse` and dispatch by class
  registry (or the transpiled `app/router.py` match).

- **D. Switchover — DONE.** `emit()` ships the overlay including
  `app/v2/routes.py` (RouteTable from
  `lower_routes_to_dispatch_functions`) and a `handle()` bridge in
  dispatch.py: known-format extension strip BEFORE match (the
  crystal/ts glue rule — an unconstrained `:id` would capture
  "1.json" on the exact pass), transpiled `Router.match`, bracket-key
  param nesting (`article[title]` → nested, the strong-params shape),
  dispatch, JSON-vs-layout branch, `Flash.to_persisted` diff for the
  cookie. `server.py` and TestClient both route through it. The
  drive-to-green fixed, at their semantic homes: jbuilder views
  merged into the overlay `Views` classes; `Hash#merge` and a
  **nested-ternary parenthesization bug** (Python's right-associative
  conditional silently reordered `dom_id`'s persisted/suffix logic)
  in the walker; nilable receivers accepted by `emit_for_each`;
  kwargs-marked call-site hashes render as Python keyword arguments
  inside library-emitted bodies only (legacy defs take positional
  dicts; the hand-written `Broadcasts` glue is dual-accepting);
  `Params.str`→`str_`; module-alias imports for
  Inflector/JsonBuilder/Roundhouse.RhDateTime/Broadcasts. Gates run:
  python_toolchain 3/3, 21/21 emitted unittests, compare 7/7.

- **E. Teardown — controller leg DONE.** Deleted:
  `src/lower/controller_walk.rs` + its `lower::mod` re-exports
  (`CtrlWalker`/`Stmt`/`WalkCtx`/`WalkState`),
  `src/emit/python/controller.rs` (the last trait impl), the legacy
  `app/routes.py` emission and its side-effect imports (main.py,
  emitted tests), and `http.py`'s Router / `ActionContext` /
  `FormatRouter` / stubs — the module is now just `ActionResponse`.
  `docs/pipeline/lower.md`'s legacy section and `adapter.md`'s
  consumer story updated (`is_suspending_effect` stays as the
  contract's suspension oracle; per-site `expr_suspends` died with
  the walker). Remaining: the per-artifact model/view/test emit
  (`model.rs` / `view.rs` / hand-written `view_helpers.py`) — the
  overlay already emits its own models/views, so this is a
  consumer-by-consumer swap of the emitted tests and glue.

## Traps named

- Two framework classes are both called `Base` (ActionController's
  and ActiveRecord's) — explicit imports only, never star imports, in
  any file that touches both worlds.
- `views.py` imports model names at its top; `models.py` binds
  `Views` at its BOTTOM (broadcast bodies render partials). Breaking
  this ordering deadlocks the import cycle — see the note in
  `src/emit/python/model.rs`.
- Python's emitted controller tests are `@unittest.skip`-decorated on
  the legacy path; do not read their passing as dispatch coverage —
  the real gates are toolchain + compare + smoke.
