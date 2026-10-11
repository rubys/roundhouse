# Specialization and residue

Rails is, operationally, an interpreter for the application: routes,
associations, callbacks and templates are data it consults on every
request. Every Roundhouse target is a specializer over that
interpreter. It makes, once and at build time, the decisions whose
answers cannot differ between requests, and leaves the rest as
*residue* for something else to handle. This page is the frame the
targets share, and the two axes along which they differ.

It describes architecture, not status. What each target currently
covers is recorded by its gates and, for the published targets, in the
[user guide](../guide/targets.md).

## One decision procedure, three residue policies

What can be decided statically is a property of the typed IR. It is
the same question whichever target asks it, answered by the same
[analysis](analyze.md). Targets differ in two things only: the
**vocabulary** a specialized construct is emitted in, and the
**policy** for a construct the analysis cannot decide.

| Target | Vocabulary | Residue policy | Residue becomes |
|---|---|---|---|
| `spinel`, `ruby`, `jruby`, and the other language targets | the Roundhouse runtime ([`runtime.md`](runtime.md)) | **forbidden** | an error diagnostic |
| `futamura` | real Rails APIs | **runs** | Ruby that Rails executes, with the app's gems |
| `roda` | Roda + Sequel | **handed to a person** | a `# ROUNDHOUSE-TODO` comment carrying the source |

The strict targets are not a different kind of thing. They are the
specialization that admits no residue, which is why
[Invariant 1](../../AGENTS.md) reads "zero error diagnostics is the
contract": under that policy an undecided construct is unsupported by
definition. That is also why `ruby` stays Spinel-shaped rather than
loosening. It measures how far the analysis gets with nothing to fall
back on, and that measurement is what the per-target ledger records.

Under `futamura` the same construct is not a failure, because Rails is
still there to run it. An undecided site costs performance, not
correctness, so the diagnostic class that matters is different: a
site emitted *wrong* is an error, while a site left to Rails is a
residual, the target's own ledger of performance debt.

## Analysis is shared; lowering is per vocabulary

`session::analyze_and_lower` is two steps:

```rust
analyzer.analyze(app);                              // types onto the IR
lower::apply_post_analyze_lowerings(app, registry)  // rewrite into runtime vocabulary
```

The [post-analyze lowerings](lower.md) are specialization *into the
runtime vocabulary*: SQL-folded queries, `Views::` calls, synthesized
dispatchers. A target with a different vocabulary needs the first step
and not the second.

- `roda` currently skips both (`bin/roundhouse`), because the lowered
  IR is the wrong level to re-idiomize into Sequel and Roda. Only the
  lowerings are wrong for it. Analysis alone would let its conversions
  rely on types (a receiver known to be a `Relation`) rather than on
  surface syntax.
- `futamura` at stage 0 also skips both, because it specializes
  nothing. From stage 1 on it needs analysis, plus lowerings of its
  own whose output calls into Rails rather than replacing it.

The emitter is shared as well. Specialized Ruby is emitted through
`emit::ruby` (`emit_expr`), as `roda` already does. A specialization
is a decision made on the typed IR that changes what runs; rewriting
or reformatting a file is not one.

## Residue that runs: the `futamura` target

`--target futamura` is Rails → Ruby that keeps the gems. Its mandate
is partial evaluation in the [Futamura](https://en.wikipedia.org/wiki/Partial_evaluation#Futamura_projections)
sense: the interpreter (Rails) and the program (the app) go in, and a
residual program comes out that does less work per request. The
projection is built by hand, one specialization at a time, not by an
automatic specializer. The name records where the target is headed,
not a claim that it is there.

**Residue granularity.** Unspecialized code takes one of two forms.
A file no specialization touches is copied byte for byte, keeping the
app's comments and formatting, with no AST involved. A residual
expression *inside* a specialized artifact (an unresolved helper call
in an otherwise compiled template) is emitted from the IR. That relies
on the expression-level round trip of
[Invariant 4](../../AGENTS.md): ingest, emit Ruby, ingest again
reaches a fixed point.

**One object model.** Specialized code calls the real framework and
receives real objects: real `ActiveRecord` models, the real router,
the real helpers. It removes the work Rails does to *interpret* the
app, not Rails itself. This is what lets the target scale to apps
with hundreds of gems. Gems that hook models (callbacks, auditing,
authentication) keep firing, because there is only ever one `Message`
class. The alternatives (loading the Roundhouse runtime beside Rails,
or splitting routes across two processes) would bypass those hooks on
every specialized path. The runtime also defines Rails' own top-level
constants (`ActiveRecord`, `ActionController`, …), so it cannot share
a process with Rails as it stands.

**Stage 0 is the identity.** Nothing is specialized, so the residue is
the whole app and the output is the app as source control holds it:
dotfiles, `.keep` placeholders, binary assets and executable bits, but
no run-time state (`log/`, `tmp/`, `storage/`, built assets) and no
`config/master.key`. It exists so the gate exists before the first
specialization does.

**The gate.** The app's own suite, run by Rails, must give the same
result against the emitted tree as against the source. Integration
tests go through the full Rack stack, so they exercise every
specialized path in-process. `tests/futamura.rs` holds the synthetic
identity check and the `fixtures/real-blog` suite gate. Performance
claims are measured against stage 0 of the same app, which *is* the
Rails app, on the same machine and seed.

**Order of specializations.** Profile first, then specialize the
largest share of Rails' per-request cost. The candidates follow the
layers Rails interprets: rendering (partial lookup resolved at build
time, collection renders unrolled into loops, helpers dispatched
statically), dispatch (route table and filter chain become direct
calls), and queries (`Relation` chains become prepared SQL on Active
Record's own connection, hydrated with `instantiate`, so callers still
receive real models). Each lands as a shared lowering, never as a
per-app patch ([Invariant 2](../../AGENTS.md)).

## Why `roda` and `futamura` stay separate targets

They share the decision procedure and the emitter, but not their
gates, because they differ in who owns the output.

- **`roda` is a port.** People maintain the output from then on, so it
  must be idiomatic and readable. Its gates are the ported oracle and
  Jeremy's round trip: re-ingest the Roda output and diff the IR
  against the Rails ingest.
- **`futamura` is a compile.** The output is a build artifact,
  regenerated on every release and never edited. It is free to be
  unidiomatic (unrolled, inlined, specialized per call site) wherever
  measurement says so. Its gate is behavior, not readability.

There is a hybrid between them. A Roda app can mount another Rack app
(`r.run`), so Roda's residue could fall through to the Rails app at
request time instead of becoming TODO comments. That is the residue
policy of `futamura` with the vocabulary of `roda`: a partial
conversion that runs from the first day.
