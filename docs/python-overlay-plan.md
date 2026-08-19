# Python universal-IR overlay (the CtrlWalker retirement)

> **ACTIVE** — phases A–B landed 2026-08-19; C–E remaining.

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

- **C. Runtime wiring.** Route requests through the overlay:
  - `app/v2/routes.py` from `lower_routes_to_dispatch_functions`
    (`RouteTable` data) + the transpiled `app/router.py` match, OR
    the hand-written `http.py` Router matched by class registry —
    decide by whichever keeps `test_support.py`'s TestClient simplest.
  - Translate the returned controller's `status`/`body`/`location`/
    `content_type` into `http.ActionResponse`.
  - The view-helper gap: overlay views call
    `ActionView.ViewHelpers.*`; either graduate the transpiled
    `action_view/view_helpers` unit into `LIVE_FRAMEWORK` or shim the
    hand-written `app/view_helpers.py` under those names.

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
