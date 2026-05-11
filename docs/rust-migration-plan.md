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
| 1 | Skeleton `rust2` parallel orchestrator | ½-1 | ✅ done |
| 1.5 | Base/Validations inheritance spike | 1-2 | ✅ done — Option A (trait + composition) |
| 2 | Framework runtime transpile (9 files, dependency-ordered) | 3-7 | next |
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

- End of Phase 1.5: lock A/B/C choice in writing before Phase 2 starts. ✅ See "Phase 1.5 result" below.
- End of Phase 4: are cross-target source idioms holding, or do we
  need new ones for rust? Adding `.to_h`-style patches with no rust
  analog signals IR-needs-adjustment, not source.
- End of Phase 6: if Phase 6 takes >1 week, root cause is probably
  IR/lowerer pressure. Strategic pause to assess.

## Phase 1.5 result — Base/Validations inheritance: **Option A (trait + struct composition)**

Three prototypes hand-written in `docs/rust-migration-spike/`. All
three compile + tests pass. Comparison:

| Axis | A: trait + composition | B: macro-driven | C: flat mega-struct |
|---|---|---|---|
| Per-model emit (LOC) | ~42 | ~10 (invocation) + ~80 macro infra | ~95 |
| Framework runtime (LOC) | ~150 | ~80 + macro definition | 0 (everything inline) |
| Spike total LOC | 335 | 167 | 205 |
| Tests passing | 5/5 | 2/2 | 4/4 |
| `Vec<Article>` (concrete) | ✅ | ✅ | ✅ |
| `Vec<Box<dyn _>>` (heterogeneous) | ✅ via `ActiveRecordObject` subtrait | ✅ same pattern | ❌ no shared trait |
| Maps cleanly from lowered IR? | ✅ `LibraryClass` → struct + impl trait | ⚠️ requires emitting macro invocations — diverges from crystal/ts IR consumption | ✅ but loses framework-runtime sharing |
| Infrastructure cost | None — pure rust 2024 | Proc-macro crate (separate Cargo workspace member, ~500-1500 LOC of macro impl for full validates_*_of catalog) | None |
| Compile time at scale (estimated) | Linear in model count; trait monomorphization is cheap | Proc-macro evaluation adds ~1-3s upfront; smaller post-expansion code | Largest codegen surface |

**Estimated emit cost for a writebook-scale 30-model app:**

- A: 30 × 42 + 150 = **~1,410 LOC**
- B: 30 × 10 + 250 + 80 = **~630 LOC** (terse) but with the proc-macro crate + IR-shape divergence overhead
- C: 30 × 95 = **~2,850 LOC**

**Decision: Option A** — trait + default methods + struct composition.

**Decisive factors (in priority order):**

1. **IR contract preservation.** The migration plan's central premise (and the existing `project_strategic_bet.md` memory) is that the lowered IR is constitutive — every target consumes the same `LibraryClass` shape. Option B requires the lowerer to emit a macro *invocation* for rust specifically, while crystal/typescript consume the same `LibraryClass` directly. That's an IR fork, and the cost compounds across future targets that might prefer the macro shape. Option A maps `LibraryClass` 1:1 to `struct + impl ActiveRecord + impl Validations` — same shape rust as crystal does for `class X < Base end`.
2. **Heterogeneous collections work cleanly.** The `ActiveRecordObject` subtrait pattern (~12 LOC: 4 method declarations + 4-line blanket impl) gives `Vec<Box<dyn ActiveRecordObject>>` without forcing the main `ActiveRecord` trait to be dyn-compatible. Option C makes this impossible by design.
3. **No new infrastructure.** Pure rust 2024 + std. No proc-macro crate to set up, test, version, ship. (B requires a sibling crate in the emitted Cargo workspace; that's ~3-5 days of additional Phase 3 scope.)
4. **Per-model verbosity acceptable.** 42 LOC per model is 4× B's terse form but ~½ C's flat form. For a 10-model real-blog: A ~420 + 150 = 570 vs B ~330. The ~240 LOC saving from B doesn't justify the proc-macro infrastructure + IR fork.

**Trade-offs accepted:**

- Two-trait split (`ActiveRecord` + `ActiveRecordObject`) for dyn-compat. Minor ergonomic cost at heterogeneous-collection sites: callers use `obj_id()` / `obj_persisted()` instead of `id()` / `persisted()`. Unambiguous and avoids method-resolution conflicts.
- `attributes()` returns `HashMap<&'static str, CellValue>` with a tagged enum (`Str | Int | Bool | Nil`). Same pattern Crystal solved with the `TestCellValue` alias in `runtime/crystal/framework_test_adapter.cr`. Cross-target consistent.
- Inherited fields embed via `pub base: BaseFields`. Field access is `self.base.id` instead of `self.id`. Lowerer hides this from emitted Ruby source — model fields look natural at the source level; only the rust struct shape differs.

**IR/lowerer-side scope (small):**

- Per-model rust2 lowerer needs to recognize "this class extends ActiveRecord::Base" and emit `pub base: BaseFields` as the first struct field. Mirrors crystal's existing `extends_active_record_base` flag in `src/emit/crystal/library.rs` (line 260+). ~30-50 LOC of lowerer adjustment.
- No `Ty` changes needed for the inheritance pattern itself.
- Future Phase 4 may surface `Ty::Ref(Box<Ty>)` pressure for closure lifetimes (independent of the inheritance choice).

**Spike artifacts:** preserved in `docs/rust-migration-spike/{option_a_trait_composition, option_b_derive_macro, option_c_flat_struct}/` for re-validation and comparison if the decision needs revisiting.

Phase 2 (framework runtime transpile) is now unblocked.
