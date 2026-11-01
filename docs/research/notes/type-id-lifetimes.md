# Type-ID lifetimes

Background for [Cost](../frontiers/cost.md).

`96519abf` implements an operation-boundary bridge: the registry, IR stamps, and fold side table
still hold `Ty`. Its address index keeps payload owners alive to prevent address reuse;
shared-payload identities change on mutation. The arena and every dependent memo reset together
at analysis entry and at the 262,144-entry bound between operations. Persistent solver IDs require
the analyzer to own the arena's lifetime before those holders are introduced. The memo's current
reset policy cannot be carried over while stored IDs survive.

**Source:** [`fixpoint-arena`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-arena)
at `96519abf`; [Ownership and reset boundaries](https://github.com/bunnykong/roundhouse-fixpoint-lab/blob/d824c4355c7faae4b9bb4a77b25daa178a3df36c/receipts/type-id-lifetimes/README.md).
