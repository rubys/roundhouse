# Order dependence

Background for [Any order](../frontiers/any-order.md). In an unpublished research build, every recorded key that differed between shuffled schedules on Chatwoot and Mastodon was classified by its local deciding rule; some upstream causes remain unresolved.

## The six classes

1. **Structure.** Slots, writers and recursive references depend on the values typed so far: a narrowing site gets a slot only if its input is currently a reference.
2. **Pending versus gradual producers.** A pending receiver still answers conversions such as `to_s`, and an `untyped` arm absorbs a union's dispatch whatever its tag; an early answer persists only in schedules that saw it.
3. **Projections.** An empty filter result falls back to a value. `remove_nil(nil)` is `nil`, but `remove_nil(nil | Integer)` is `Integer`, so more input removes `nil` from the output.
4. **Shape stamps.** A multi-value return is a tuple only once its positions are known and differ; an empty literal reads back the type stamped on it earlier.
5. **History cuts.** A nested copy of the previous return is cut to `untyped` before storing (`untie_recursive_return`), though matching the previous value isn't evidence of recursion.
6. **Writers.** Slots don't keep one entry per writer: some bindings count only while free of `untyped` and pending arms, and merging call sites drops `untyped` beside concrete arguments.

The [writer-rule implementation attempt](../attempts.md#writer-rule-implementation-boundaries)
records the diagnostic and representation boundaries behind the first step below.

## One proposed order

Dependencies set this order; it is argued, not yet carried out or proved. Step 2 could instead discover the structure monotonically, as the brief's [leads](../frontiers/any-order.md#leads) describe.

1. **Shapes and stamps:** tuple shape from syntax, each position joined on its own, stamps never read back, plus two writer rules: all-arms block binding pads both operands, and attribute assignment returns its right-hand side. The rules are small, but a prototype showed they reach diagnostic dispatch and emission classification, so the change is larger than the rules. It depends on nothing.
2. **Structure declared before typing,** from the program text with explicit candidates for dynamic calls. Unwritten slots read ⊥, and filters treat inline values and references alike. It defines the system later rules must be monotone on. One change, gated by an equal structure digest across schedules and from start to end.
3. **Pending split from gradual:** pending receivers contribute nothing, a union's dispatch joins its arms' answers, writers lose their gates, errors are counted after settling, and a slot left ⊥ prints as unresolved `untyped`. One change, scored by the [error census](../checks/error-census.md).
4. **Positive projections:** empty results are ⊥; dead branches are labeled after the run. It needs steps 2 and 3; otherwise a leftover ⊥ prints as a type like `Array[Bottom]`.
5. **History cuts removed:** with recursion declared, require zero cuts, then delete the cut, the cross-round joins and the re-applied harvests together, keeping one entry per writer.
