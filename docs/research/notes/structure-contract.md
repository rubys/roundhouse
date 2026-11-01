# The whole equation system

Background for [Any order](../frontiers/any-order.md).

For the predeclared design, add explicit identities for constants and closure parameters/results to
the slot inventory in Any order. Concern return and parameter keys distinguish definition owner,
includer, and class/instance side; expression keys carry source owner and syntax path. Declare writer
identities and a may-call graph from syntax, library summaries, and explicit dynamic-send candidates.
The measured structure prototype still lacks a complete logical slot/writer declaration with
membership checks, and its routing canary does not audit every inline/reference read path.

**Sources:** [Predeclared equation design](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/receipts/structure-contract/README.md) and
[Partial-structure integration regressions](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/receipts/writer-boundaries/README.md).
