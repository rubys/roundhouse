# Rust target migration plan

Migration of the Rust target from Group 2 (hand-written Rails-domain
emit + per-target framework runtime) to Group 1 (lowered IR + transpiled
`runtime/ruby/`). Same pattern Crystal completed 2026-05-10
(framework_tests 8/8). See `MEMORY.md → project_rust_migration_plan.md`
for full strategic context.

## Decisions locked in

- **Strangler-fig (not rip-and-replace).** Existing `src/emit/rust.rs`
  + `src/emit/rust/{controller, model, view, ...}.rs` keeps producing
  shipping real-blog. New emit grows in parallel under
  `src/emit/rust2.rs` + `src/emit/rust2/`. Switchover is one commit
  at Phase 7. CI stays green throughout.
- **Phase 1.5 spike inserted** before framework runtime transpile.
  `include Module` (Validations into Base) has no Rust analog —
  Crystal emits it literally. Three options to evaluate via hand-
  written prototypes (~150 LOC each): (A) trait + default methods +
  struct composition, (B) `#[derive(ActiveRecord)]` proc-macro, (C)
  flat mega-struct.
- **Pre-migration tag:** `git tag rust-pre-migration` (local) marks
  the working state — `cargo test --test rust_toolchain -- --ignored`
  → 2/2 pass at this point.

## Phase status

| # | Phase | Days | Status |
|---|---|---|---|
| 0 | Audit + tag | ½ | ✅ done |
| 1 | Skeleton `rust2` parallel orchestrator | ½-1 | next |
| 1.5 | Base/Validations inheritance spike | 1-2 | blocked on 1 |
| 2 | Framework runtime transpile (9 files, dependency-ordered) | 3-7 | blocked on 1.5 |
| 3 | Hand-written primitive runtime + abstract adapter base | 2-3 | parallel-able with 2 |
| 4 | `framework_tests_rust` gate (8/8 target) | 1-2 | blocked on 2+3 |
| 5 | Per-file model/view/controller emit | 3-5 | blocked on 4 |
| 6 | Real-blog parity via `rust2` | 2-4 | blocked on 5 |
| 7 | Switchover commit + prune legacy | 1-2 | blocked on 6 |
| 8 | Add `framework-tests-rust` CI; close out | ½ | blocked on 7 |

Total estimate: **14-26 working days** (~3-5 weeks at sustainable
pace). Crystal precedent was ~2 days for the rip-and-replace bulk;
rust's borrow-checker + inheritance-decision + lifetime-annotation
cascade pushes it 2-3× longer.

## Phase 0 audit — `src/emit/rust/` + `src/emit/rust.rs`

Total 4,852 LOC across 13 files. Categories:

| File | LOC | Category | Notes |
|---|---|---|---|
| `rust.rs` | 552 | mixed → port | Top-level orchestrator. Replaced by `rust2.rs` mirroring `crystal.rs` shape. |
| `controller.rs` | 1233 | **mixed** | Rails-domain (`emit_controller_axum`, `RsEmitter`, `try_emit_assoc_create`, `is_known_model_method`) **+ salvageable infra** (`EmitCtx`, `emit_body`, `emit_stmt`, `emit_expr`, `emit_send`, `is_bare_rs_ident`, `rewrite_ruby_dot_call`, `apply_rust_chain_modifier`, `map_instance_method`, `args_tuple_or_single`, `emit_block_body`). Split during Phase 5: lift generic emit machinery into `rust2/expr.rs` + `rust2/method.rs`, retire the rest. ~600 LOC salvage / ~600 LOC retire. |
| `view.rs` | 1348 | Rails-domain → retire | ERB-IR-to-rust view fn emit. Replaced by view-lowerer + generic library emit. |
| `model.rs` | 542 | Rails-domain → retire | Struct + impl + persistence + validations + broadcasters. Replaced by lowerer-emitted `LibraryClass` consumed by generic `library.rs`. |
| `spec.rs` | 502 | Rails-domain → retire | Test-module emit. Some axum-test glue (~50 LOC) may need keeping in `test_support.rs` runtime, not emitter. |
| `fixture.rs` | 194 | Rails-domain → retire | YAML fixtures → rust modules. |
| `route.rs` | 164 | Rails-domain → retire | Router + route_helpers emit. |
| `cargo.rs` | 88 | retire | Inline `CARGO_TOML_TEMPLATE` const in `rust2.rs` (matches crystal `SHARD_YML` pattern). |
| `main.rs` | 80 | retire | Inline `MAIN_RS_TEMPLATE` const in `rust2.rs`. |
| `ty.rs` | 64 | already-generic → keep | `rust_ty(ty: &Ty) -> String`. Move to `rust2/ty.rs` largely as-is. |
| `importmap.rs` | 35 | retire | Replaced by importmap-lowerer + generic emit. |
| `schema_sql.rs` | 29 | retire | Inline into `rust2.rs`. |
| `shared.rs` | 21 | already-generic → keep | `emit_literal`. Move to `rust2/shared.rs` as-is. |

**Roll-up:**
- Retire outright: ~3,140 LOC (view + model + spec + fixture + route + importmap + schema_sql + cargo + main).
- Salvage to new emit: ~700 LOC (controller.rs's generic infra + ty.rs + shared.rs + chunks of rust.rs orchestration).
- Net deletion at Phase 7: ~3,150 LOC from emitter + ~1,500-2,000 LOC from `runtime/rust/{runtime.rs, view_helpers.rs}` retirement = **~4.6-5.1K LOC removed**, replaced by ~700 LOC ported infra + new generic emit (~2,000 LOC mirroring crystal's footprint).

## Risk callouts (in priority order)

1. **Phase 1.5 decision is foundational.** Affects every transpiled
   model AND every test fixture pattern. Getting it wrong forces
   redo of phases 5-6. The spike exists specifically to de-risk this.
2. **`include Module` has no rust precedent.** Phase 1.5 invents
   the translation.
3. **IR pressure on `Ty`.** Rust may need `Ty::Ref(Box<Ty>)` or
   `Ty::Owned` vs `Ty::Borrowed`. Ripples to all targets. Test
   crystal/ts still pass after each `src/ty.rs` change.
4. **Async coloring deferred.** Default rusqlite (sync) inside async
   axum needs `tokio::task::spawn_blocking`. Out of scope for first
   migration.
5. **Compile time in `tests/`** (project-root cargo convention).
   ~1-2s per file. Fine for 8 framework tests; defer writebook scale.

## Mid-stream decision points

- End of Phase 1.5: lock A/B/C choice in writing before Phase 2 starts.
- End of Phase 4: are cross-target source idioms holding, or do we
  need new ones for rust? Adding `.to_h`-style patches with no rust
  analog signals IR-needs-adjustment, not source.
- End of Phase 6: if Phase 6 takes >1 week, root cause is probably
  IR/lowerer pressure. Strategic pause to assess.
