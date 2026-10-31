# Foundations

## The fixpoint

Roundhouse types each method body from what it knows about the rest of the app: the returns of the methods it calls, the types of instance variables, and the parameter types its callers pass. Typing one body changes what others see, so the analyzer repeats rounds until a round changes nothing. It runs three such loops (production, views and tests, absorb), each capped at 12 rounds.

On main the loops often reach the cap. Recursive data grows one level deeper each round, some rules replace a type instead of adding to it, and [#584](https://github.com/rubys/roundhouse/pull/584)'s size bound cuts types that grow too large. Hitting the cap doesn't establish a fixpoint, and changing the cap can change the answer.

## The proposal, in four ideas

1. **Shared types (S1).** Each distinct piece of a type is stored once and visited once, so large types stay cheap.
2. **Why each `untyped` exists (S2a).** A tag says whether a value is *pending* (not computed yet), *gradual* (genuinely dynamic) or *unresolved* (the analyzer couldn't tell).
3. **Monotone rules from ⊥ (S2b) and recursive types by origin (S2c).** A value only ever grows, a pending value counts as nothing (⊥) rather than as `untyped`, and a recursive type is stored as a reference to the slot it came from instead of being unrolled.
4. **Re-type only what changed, in dependency order (S3).**

Here ⊥ denotes pending inference with no evidence. It is distinct from language-level `Bottom`, meaning a computation never returns; the historical pending-as-bottom prototype fails the runtime witnesses ([F19](facts.md#f19)). These are the design's stages, not properties already established for every Rust rule.

## When order can't matter

A classical result, chaotic iteration: on a lattice of finite height, starting from ⊥ (nothing known), every fair order of applying a fixed, finite set of monotone equations, joining what they contribute, reaches the same least solution. For the analyzer that takes two conditions:

- **Structure fixed by the program.** The slots, the writers that feed each slot, and the slots that become recursive references come from the source, not from what typing happened to discover first. Slots may be created lazily, as long as reading one nobody has written gives ⊥ and which slot a write goes to never depends on current values.
- **Monotone writers.** More input never removes anything from a writer's output, and a slot's value is the join of its writers' contributions, under one join per slot that is commutative, associative and idempotent, with pending as its identity.

Neither condition is established for the analyzer yet; both belong to the [any-order frontier](frontiers/any-order.md). The lab's [Lean proof](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/proof) assumes both, plus finiteness.

## How results are measured

Against main at a named commit, on the inputs each check specifies, most often the [five pinned public apps](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/corpus) (Campfire, Mastodon, Chatwoot, Forem, Discourse). Some results add a large private Rails app, reported only as aggregates.

| Measure | How |
| --- | --- |
| Flags off changes nothing | Emitted code byte-identical on 105 fixture × target pairs; the default suite and typing ceilings as main |
| Settling | `RH_FIXPOINT_VERIFY=1` repeats a round after each loop and reports what moved |
| The same answer | `RH_FIXPOINT_DIGEST=1` digests the carried state; compare across runs and across `RH_SHUFFLE=<seed>` schedules |
| Errors by kind | `RH_ERRGATE=1 RH_PUBLIC_INPUT=1` records every call with a receiver and every arithmetic or comparison operator; `python3 tools/errgate.py BASE.stderr CANDIDATE.stderr` labels each difference: [the census](checks/error-census.md) |
| Precision | Share of expressions fully typed and share holding `untyped`, with the [runtime oracle](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/oracle) beside it |
| Cost | `check --continue` wall time and peak memory, interleaved runs |

The flags live on the [follow-up branch](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-next); the canaries (`RH_FIXPOINT_VERIFY`, `RH_FIXPOINT_DIGEST`, `RH_FIXPOINT_STATS`) are on main since [#657](https://github.com/rubys/roundhouse/pull/657).

On the October 10 baseline, flags-off suite parity includes three inherited failures ([F26](facts.md#f26)); it is not a green suite. The later [F29](facts.md#f29) trace supplies app-runtime evidence alongside the earlier static counts.

## Terms

- **Slot:** where a type is kept between bodies, such as a method's return, a parameter position or an instance variable.
- **Writer:** a call site, assignment or return that contributes to a slot.
- **Structure:** the slots, the writers of each, and which slots are recursive references.
- **Join:** the smallest type admitting both inputs, such as `Integer | String`.
- **Settles:** a loop ends because a round changed nothing, not because it hit the cap.
