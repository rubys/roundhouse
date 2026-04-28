# Bytecode

An experimental additional emission target alongside the seven
source-code targets (Ruby, Rust, TypeScript, Go, Crystal, Elixir,
Python, Spinel — all DOM-equivalent in CI). Instead of emitting
target-language source, emit a typed stack-based bytecode and
execute it in a VM that reuses the existing Rust runtime.

**Source:** `src/bytecode/` (format types, VM, IR→bytecode walker),
`tests/bytecode_format.rs`, `tests/bytecode_vm.rs`, `tests/bytecode_emit.rs`.

Status: experimental. M1 (format), M2 (minimal VM), and M3a
(IR→bytecode walker, `src/bytecode/walker.rs`) have landed on `main`.
M3a covers the subset of `ExprNode` whose lowering maps cleanly to the
M2 VM's opcodes: `Lit`, `Var`, `Let`, `Seq`, `If`, `Assign` to
`LValue::Var`, and `Send` for arithmetic/comparison on `Ty::Int`
receivers. Everything else returns `WalkError::NotYetSupported` until
the corresponding M3b+ work lands. Full integration with the runtime
(execution against the real-blog fixture) is the remaining piece —
the source-code-target gate that was previously cited as a
prerequisite has been met. See [the plan](#the-plan) below.

## Why a bytecode target

The seven existing emitters each produce source code compiled or
interpreted by the target language's toolchain. A bytecode target
changes shape in three ways:

- The artifact is binary, not source code.
- The execution engine ships as part of roundhouse itself (or a thin
  companion binary), not the target language's runtime.
- The whole path — ingest → analyze → lower → emit-bytecode → VM — can
  live in one process without a separate compile step.

What that buys, if it works end-to-end:

- **`rails server`-class dev loop.** Change a `.rb` file → re-emit
  bytecode in milliseconds → VM reloads. No `cargo build` between edits.
- **Smaller cold start and image.** No Rust toolchain, no bundled JS
  runtime; one roundhouse-shaped binary plus per-app bytecode.
- **A third benchmark point.** Ruby/Rails and AOT Rust bracket the
  performance envelope; bytecode-plus-VM measures the interpreted
  middle. The answer informs whether a JIT tier is worth pursuing.
- **Static-dispatch-all-the-way-down.** Because the analyzer has
  already resolved every call, the VM never does dynamic method lookup
  and the allocator can be arena-per-request with no general tracing GC.

## The plan

Phase A (framework) and Phase C (emitter + integration) were originally
separated by Phase B (**complete the existing targets first**), so
that lifts from bringing each target to 5/5 parity could benefit the
bytecode emitter the same way they benefit each other. Phase B is now
met — all seven runnable targets are DOM-equivalent in CI — so Phase C
work (M3a walker landed; M3b+ in flight) is unblocked.

| # | Milestone | Phase | Status |
|---|---|---|---|
| M1 | Bytecode format + roundtrip tests | A | ✅ landed |
| M2 | Minimal VM: arithmetic, locals, branches | A | ✅ landed |
| — | Variant experiments (stack vs register, dispatch strategies) | A | ⏳ parked |
| — | Python / Crystal / Elixir / Go / Ruby round-trip at 5/5 | B | ⏳ in flight |
| M3 | Bytecode emitter for tiny-blog | C | ⏳ parked |
| M4 | VM runs one controller action end-to-end | C | ⏳ parked |
| M5 | tiny-blog controller tests pass through VM | C | ⏳ parked |
| M6 | real-blog controller tests pass through VM | C | ⏳ parked |
| M7 | `scripts/compare bytecode` hits 5/5 on real-blog | C | ⏳ parked |
| M8 | Benchmark table: Rails vs bytecode VM vs AOT Rust | D | ⏳ parked |

Each milestone produces something demonstrable and non-regressable
once green, matching the forcing-function methodology the rest of the
pipeline follows.

## Format design

Three baseline choices in the M1 opcode set. All three are measurable
rather than dogmatic; the variant-experiments slot between M2 and M3
revisits each empirically if the benchmark data argues for it.

**Stack-based.** Instructions operate on an implicit operand stack.
Easier to emit from the tree IR than a register machine. The
register-based alternative (typical ~26% speedup per Shi/Casey/Ertl/Gregg,
*Virtual Machine Showdown: Stack Versus Registers*, VEE 2005) is an
explicit follow-up experiment, not a deferred decision.

**Typed opcodes.** `LoadI64` / `AddI64` / `ConcatStr` rather than
polymorphic `Load` / `Add` with runtime type tags. The analyzer already
resolves every expression's type, so the emitter picks the right opcode
and the VM never needs to type-check at runtime.

**Direct runtime dispatch.** `CallRt` carries an `RtFnId` that indexes
into `Program::runtime_fns`; the VM resolves each name to a Rust
function pointer once at load time. No per-call hashmap lookup.

## Format (M1)

Defined in [`src/bytecode/format.rs`](../../src/bytecode/format.rs).

Top-level container:

```rust
pub struct Program {
    pub format_version: u32,
    pub string_pool: Vec<String>,
    pub symbol_pool: Vec<String>,
    pub runtime_fns: Vec<String>,
    pub user_fns: Vec<UserFn>,
    pub code: Vec<Op>,
}
```

31 opcodes across seven categories: literal pushes, locals, integer
arithmetic, string concatenation, typed comparisons, control flow
(jumps carry signed `i32` offsets relative to the next instruction),
calls (user and runtime), stack manipulation, collections, string
interpolation.

The serialization format itself is deferred. Types derive
`serde::{Serialize, Deserialize}`, so any serde-compatible format
works. M1 tests use serde_json for legibility; later milestones swap
in a binary format (bincode, postcard, or hand-rolled) once VM load
time matters.

## VM (M2)

Defined in [`src/bytecode/vm.rs`](../../src/bytecode/vm.rs).

**Value representation** — tagged enum `Value::Int | Float | Bool | Str | Nil`.
Alternatives (separate stacks per type, universal word slots) are
variant-experiment material; this is the baseline.

**State** — operand stack (`Vec<Value>`), locals (`Vec<Value>`),
program counter (`usize`). No call-frame stack yet: M2's scope is
straight-line execution with branches. M3+ introduces frames for
function calls.

**Dispatch** — `match` on `Op` inside a loop. Each `Op` clones per
iteration (a memcpy of ~16 bytes; `Op` has no heap fields) to avoid
the borrow dance of matching on a reference while also mutating
`pc`. Computed-goto / direct-threaded dispatch is one of the variant
experiments.

**Deferred opcodes** — `CallUser`, `CallRt`, `ConcatStr`, `NewArray`,
`NewHash`, `IndexLoad`, `IndexStore`, `InterpStr` return
`VmError::NotYetSupported(name)` rather than panicking. Tests catch
accidental reliance before the emitter lands.

## Variant experiments (between M2 and M3)

Once the framework is in place the cost of picking any specific design
is low: write both, measure both, keep the winner.

| Variant | Hypothesis |
|---|---|
| `stack_typed_v1` | Baseline. What M1/M2 currently build. |
| `stack_poly_v1` | Polymorphic opcodes with type tags — smaller bytecode, more runtime dispatch. |
| `register_typed_v1` | Register-based, typed — harder to emit, faster to execute. |
| `threaded_dispatch` | Same bytecode as baseline but computed-goto / direct-threading VM. |
| `minimal_runtime_calls` | No `CALL_RT` opcode, everything inlined — isolates the runtime-call overhead itself. |

Each is a few days of work once the framework exists. Each answers a
specific question. Variants that lose stay under `bench/variants/` as
published measurement; the winner advances to M3.

Variant-picking benchmarks are cheap and fast (microbenchmark + one
real-blog endpoint at fixed concurrency, ~1 minute per variant) and
live in a separate rig from the publication benchmarks (Rails vs
roundhouse, hour-long runs, pinned-governor Hetzner host).

## What's still to decide

- **Where the VM ultimately ships.** `src/bytecode/vm.rs` is where M2
  lives today (compiles as part of the roundhouse crate, tests run in
  the repo). For M3+, the VM becomes the thing a user invokes to run a
  compiled bytecode file — either extract to `runtime/rust/vm/` and
  copy like the other runtime files, or ship a `roundhouse-vm` binary
  in the roundhouse crate. Clean options either way; defer until the
  decision has consequences.
- **On-disk format.** serde_json today is a placeholder for tests; a
  binary format is wanted before benchmarks. `bincode` is a one-line
  swap once the format stabilizes.
- **Whether to publish interpreter numbers separately.** Once M8 lands,
  the benchmark table has three columns: Rails, bytecode VM, AOT Rust.
  The numbers decide whether interpreter-mode is a deployment choice or
  purely research.

## See also

- [`emit.md`](emit.md) — the seven source-code emitters that share the
  analyzer, lower, and runtime integration this target will consume.
- [`lower.md`](lower.md) — target-neutral IR. The bytecode emitter
  will consume the same `LoweredAction`, `LoweredPersistence`,
  `LoweredValidation`, `ViewHelperKind` as every other target.
- [`runtime.md`](runtime.md) — per-target runtime shape. The bytecode
  VM reuses the Rust runtime rather than growing its own.
