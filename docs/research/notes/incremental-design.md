# Incremental design

Background for [Incremental](../frontiers/incremental.md): the design behind `RH_WARM` on `fixpoint-warm`.

## What is stored

One record per body evaluation, in the order the evaluations ran:

- the unit's key and a fingerprint of its body that ignores source positions;
- each slot the evaluation read, with a hash of the value it saw;
- what the evaluation wrote.

Records name slots and units by key, not by a run's internal numbering. Provenance is just who wrote each fact, when, and from what.

## Cache guards and scheduling reads

Cache validity can inspect more state than a body evaluation reads for scheduling. In the historical
`RH_WARM` v1 prototype on `92844f68`, restoring conservative cache guards as S3 dependencies caused
five edited-input cold-shadow mismatches. V2 (`990137f7`) stores actual scheduling reads separately,
restores only those edges,
isolates each evaluation's read collector, and discards reads made while validating guards.
A class-key lookup regression checks a value guard that creates no scheduler dependency. Replay
correctness includes dependency metadata and controller contexts, not only cached type values.

**Sources:** [`fixpoint-warm`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-warm)
at `990137f7`; [Cache guards and cold-shadow outputs](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/receipts/cache-guards/README.md).

## How replay works

After an edit:

1. Drop the records of units whose body changed or disappeared.
2. Rebuild the state from ⊥, replaying the kept records in their original order. Apply a record without typing only if the rebuilt state already covers its reads (holds at least the values it read); otherwise queue its unit.
3. Requeue reused units whose reads grew, plus changed, new and uncovered units; continue until no transfer can add information.

Deletions need no retraction: a fact supported only by a dropped record, directly or around a cycle, is never re-applied, since no surviving record finds its reads covered first. Reads must be checked against the state being rebuilt; checking them against the old answer keeps a stale cycle alive.

The prototype runs inside the existing S3 driver, not a new solver, and tests "covered" conservatively, as equality of read hashes.

## When it is exact

Replay gives the cold answer when:

- **transfers are monotone:** more input never removes output;
- **records are valid:** each comes from a unit the edit didn't change and lists every value its evaluation read;
- **new writes are justified:** each is a transfer applied to the state it read, with complete reads;
- **the run saturates:** no transfer of the edited program can add information.

A record missing one read can silently keep a stale fact. Re-typing a sample of reused units checks validity, but no run can check monotonicity. Today's rules aren't all monotone, and the structure is found during typing ([order dependence](order-dependence.md)), so a warm run is one more schedule and can settle elsewhere. That is why the cold shadow is mandatory.

## Status

`Edit.lean` provides general replay certificates and an executable solver over a finite fact universe.
`Record.Valid` requires each cached write to follow from its reads through a monotone transfer
contained in the edited monotone operator. `IncRun.eq_kleene` requires a justified continuation to
saturation; `incrementalSolve_eq_kleene` proves the supplied replay-and-inflate solver equals the cold
least fixpoint. Both quantify over arbitrary lists of valid records, without a chronology hypothesis.
Checked deletion controls include `replayAgainst_old_keeps_cycle` and `incomplete_reads_not_least`.
The Rust implementation still must establish these premises.

**Receipt:** [Edit.lean](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/proof/ProofLean/Edit.lean), with its dependencies and
[build instructions](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/proof/run-replay.md). The [remaining proof modules](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/proof/PROOF.md) cover additive warm starts and deletion cycles.
