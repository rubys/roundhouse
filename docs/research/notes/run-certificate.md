# A run certificate

Background for [Any order](../frontiers/any-order.md).

`Run.eq_kleene` certifies a finite trace for a fixed monotone operator over a finite fact universe:
empty start, justified writes, and final saturation suffice; ascent is not assumed. Its twin-operator
example has identical visited states and operator answers but different leastness outcomes. A separate
finite routing model yields different answers with zero descents when routing is chosen at first
evaluation. These abstract checks do not certify the Rust analyzer's structure, transfer semantics,
or runtime soundness.

**Receipts:** [Certificate.lean](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/proof/ProofLean/Certificate.lean),
the [proof build](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/proof/run-replay.md), and the
[finite routing model](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/reproductions/routing_model.py).
