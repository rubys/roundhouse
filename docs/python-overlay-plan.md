# Python universal-IR overlay (the CtrlWalker retirement)

> **ACTIVE** — phases A–B landed 2026-08-19; C's dispatch leg proven
> the same day (both fixtures serve `index` end-to-end through
> `app/v2/dispatch.py` in the driver tests); remaining = http.py /
> TestClient wiring (C), switchover (D), teardown (E).

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

- **D. Switchover.** `emit()` ships the overlay; `app/routes.py` and
  TestClient dispatch through it; the per-artifact controller path
  (`emit_controllers_pass2` + the `CtrlWalker` impl) is deleted.
  Gates: `python_toolchain` (tiny-blog), `compare` matrix python leg
  (real-blog DOM), `smoke (python)` floor.

- **E. Teardown.** Delete `src/lower/controller_walk.rs` and its
  `lower::mod` re-exports; update `docs/pipeline/lower.md` +
  `emit.md` (the "legacy path" section disappears); decide the fate
  of the remaining per-artifact model/view emit (they can follow the
  same switch once the overlay proves out, retiring `model.rs` /
  `view.rs` / the `FormatRouter` machinery in `http.py`).

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
