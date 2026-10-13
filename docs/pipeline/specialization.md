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
- `futamura` runs the analysis and none of the lowerings. Its own
  specializations (`src/emit/futamura/`) read the typed IR and emit
  Ruby that calls into Rails rather than replacing it.

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
constants (`ActiveRecord`, `ActionController`, …), so it cannot be
loaded into Rails' namespace. The one exception is
[whole-route replacement](#whole-route-replacement), which loads the
runtime into a `Ruby::Box` and lets only the session, the flash and
the database connection cross, never a model object.

**Stage 0 is the identity.** Nothing is specialized, so the residue is
the whole app and the output is the app as source control holds it:
dotfiles, `.keep` placeholders, binary assets and executable bits, but
no run-time state (`log/`, `tmp/`, `storage/`, built assets) and no
`config/master.key`. It exists so the gate exists before the first
specialization does.

**The gate.** The app's own suite, run by Rails, must give the same
result against the emitted tree as against the source. Integration
tests go through the full Rack stack, so they exercise every
specialized path in-process. `scripts/futamura-suite APP` runs it for
any app (`--mise` on a box whose shells lack mise's prompt hook);
`tests/futamura.rs` holds the synthetic checks and the
`fixtures/real-blog` gate. Every specialized site keeps the original
expression as a fallback block, and `FUTAMURA_VERIFY=1` runs both and
raises on any difference, so the app's suite run under it is a
differential test of each specialization it reaches. Performance
claims are measured against stage 0 of the same app, which *is* the
Rails app, on the same machine and seed.

**Order of specializations.** Profiles set the order, not whether a
specialization is built: each is built, gated, measured and published,
small or null results included. Each lands as a shared lowering, never
as a per-app patch ([Invariant 2](../../AGENTS.md)). The candidates
follow the layers Rails interprets: queries, rendering (partial lookup
resolved at build time, collection renders unrolled, helpers dispatched
statically), and dispatch (route table and filter chain as direct
calls). Profiles of the two benchmark apps (October 2026, CRuby + YJIT,
in-process) disagree on what dominates:

- Campfire caches aggressively. On its cached pages the controller is
  80-90% of a request, spent on collection-cache keys and decoding,
  Active Record, and gzip; route recognition is negligible. With the
  cache off, rendering costs ~8 ms per message, spread across Active
  Record, Active Support, ActionView and Action Text's sanitizer.
- Lobsters barely caches. Active Record and Active Model are a third of
  self time against 11% in SQLite itself.

### Specialization 1: static query chains

`src/emit/futamura/query.rs`. A chain whose root is a model constant or
a plain `has_many` reader, whose links are scopes, `where(column:
value)`, literal `order`/`preload`/`includes` and `limit`, and which
ends in a terminal (`to_a`, `first`/`last` with or without a count,
`take`, `find_by`) is rewritten as a Relation over bind placeholders in
the generated initializer. Rails builds it once into an
`ActiveRecord::StatementCache`, the mechanism behind its own
`find_by`, so the SQL, the preload trees and the strict-loading setting
are Rails' own. The binding-time analysis here decides which parts are
static. A scope that only builds a relation from literals is called by
name, so Rails evaluates it, including the Active Storage and Action
Text scopes whose trees depend on boot-time configuration. Scopes with
parameters, or with a terminal inside, are inlined. Values only known
per request become binds, evaluated at the call site. `first`/`last`
use Rails' own `ordered_relation`, so an unordered chain gets Rails'
implicit order.

Coverage (`ROUNDHOUSE_FUTAMURA_LEDGER=1` lists every site left to
Rails, with its reason): Campfire 90b3300, 6 sites, including both
default message-page chains; the rest are string conditions over
records and scopes that return objects rather than relations. Lobsters,
46 of 58 query sites, chiefly the single-record lookups
`Model.where(column: params[:x]).first`. Campfire edbc779's
`last_page` returns a deferred-preload `Page`, not a relation, and is
correctly left alone.

Verified: Campfire 90b3300's suite gives the same result on both trees
with `FUTAMURA_VERIFY=1` (407 runs), its specialized sites compiled to
statements, not the fallback, with Rails' own preload trees. Every
Lobsters benchmark request (228) returns the same response on both
trees in verify mode, after normalizing per-request tokens and a
clock-dependent minutes count.

Measured (October 2026, rubix3, CRuby + YJIT), a null result:

| | Rails | Futamura |
|---|--:|--:|
| Campfire room-page chain, in-process | 4.83 ms | 4.61 ms |
| Campfire room page, in-process | 40.97 ms | 41.07 ms |
| Campfire room page, harness c=1 (req/s, median of 3) | 20.1 | 20.4 |
| Campfire room page, harness c=16 | 57.2 (55.9-57.4) | 53.7 (52.9-57.3) |
| Lobsters benchmark pass, in-process, one core | 1,353 ms | 1,358 ms |

The SQL is the same and so is most of the work. What this removes,
building the relation and compiling Arel per request, is ~0.2 ms; of
the room page's 4.8 ms chain, ~4.1 ms is preloading, which still runs
through Rails' Preloader. Harness rows for unspecialized routes vary by
about 2% between the two images, and Lobsters per-route deltas scatter
±9% around zero, so neither app shows a measurable change. That made preload specialization the next query specialization.

### Specialization 2: preloads planned once

`Futamura::Preload`, in the generated initializer's runtime. For each
preload entry (one element of the built relation's `preload_values` or
`includes_values`), the generic work Rails' Preloader repeats on every
call is done once: the branch tree, each association's scope (the
target's `scope_for_association`, the polymorphic type condition, the
reflection's own scope) and the grouping of loaders whose queries match.
Each call then runs only what `Preloader::Association` does per call, in
its order: key the owners, skip owners already loaded, load the missing
keys with `scope.where(key => keys)` (one query per group of matching
loaders on a level, as `Preloader::Batch` groups them), set inverses as
records load, and assign targets.

An entry is planned only when every association in its tree can have
its scope built once: not `through:`, not a polymorphic `belongs_to`, no
composite key, no owner-dependent scope, no `default_scope` on the
target, and a scope that is absent, written inside Active Storage or
Action Text (read through Rails' builder wrapper), or certified static
by Roundhouse from the app's own declarations
(`Futamura::Preload.static_scopes`, emitted from the IR: a scope like
`-> { where(user: Current.user) }` is not certified, so it stays
evaluated per request). Any other entry goes to Rails' Preloader whole.
Under `FUTAMURA_VERIFY=1` the loaded association targets are compared
with Rails' own, through the whole tree.

Verified: Campfire 90b3300's suite SAME under verify (407 runs), with
three of the room page's four entries planned (the fourth is
`through:`); all 228 Lobsters benchmark requests identical under verify.

Measured (October 2026, rubix3, CRuby + YJIT):

| | Rails | Futamura |
|---|--:|--:|
| Campfire room-page chain, in-process | 4.78 ms | 3.82 ms |
| Campfire room page, in-process (6 rounds) | 40.99-41.39 ms | 39.84-40.19 ms |
| `Session.find_by(token:)` / `Account.first`, every request | 115 / 167 us | 104 / 98 us |
| harness c=16 req/s, median of 3: room | 54.6 | 58.7 |
| sidebar | 137.3 | 143.8 |
| search | 99.1 | 102.3 |
| messages | 103.1 | 104.7 |
| post | 76.9 | 75.7 |

The first measurable gain. The sidebar and search improvements come
from specialization 1's every-request lookups (the session and
`Current.account`), the room page's additionally from the preloads.
What remains of the room chain is SQLite (21%), instantiation (8%), the
`through:` entry still on Rails' Preloader (~22%), and compiling the
per-call `where(key => keys)` relations (10%): `through:` associations
and cached per-arity statements are the next steps for queries.

### Where specialization inside Rails runs out

With both specializations, Lobsters runs at 1.02x of Rails (rubix3,
CRuby + YJIT, 1,149 vs 1,167 ms per benchmark pass), against 4.2x for
`--target ruby`. The profile of that pass is flat. Relation building
is the largest share at 12%, then SQLite 11%, dispatch 8%, Active
Record instantiation 8% and Active Support time 7.5%. Charging time
inclusively to model objects bounds what a faster read path inside
Rails' own model classes could save. Rails' attribute-lookup machinery
is ~1.3% and instantiation ~2%. The rest of attribute reading is the
casting itself (YAML-backed stores, times, booleans), which any reader
still pays. That is ~3-4%, the size of specializations 1 and 2.
Keeping real Active Record and Active Support objects caps the target
at roughly 2x. The rest of the distance to 4.2x is those objects, and
it is removed only when a whole request avoids them.

### Whole-route replacement

Specializing inside Rails keeps Rails' objects, and the profile above
caps that at roughly 2x. The rest is reached by not running Rails at
all for a request: `--target futamura` also ships the app's
`--target ruby` emit, and serves a route from it when the analysis
says nothing on that route is left for Rails. It is the residue policy
of this target applied at route granularity: a route either runs
entirely on the lowered app, or entirely on Rails.

**Ruby 4.1's `Ruby::Box` is a hard dependency.** The Roundhouse runtime
defines Rails' own top-level constants (`ActiveRecord`,
`ActionController`, ...), so it cannot share Rails' namespace. A
`Ruby::Box` gives it its own: the lowered app is loaded into a box
inside the Rails process (`vendor/futamura/lowered/`), and its
`ActiveRecord` is not Rails'. Boxes need Ruby 4.1 started with
`RUBY_BOX=1`. Ruby 4.0.x (4.0.5, 4.0.7) segfaults under `RUBY_BOX=1`
as soon as Rails loads `net/imap`, before any box exists. The
generated initializer refuses to boot without a box;
`FUTAMURA_WHOLE_ROUTE=off` runs the same tree as plain Rails. It is
SQLite-only for now, because the lowered runtime reads through the
connection Rails lends it and only its SQLite shim can take one; on any
other primary database the initializer turns whole-route off and says
so, and every route is Rails'.

**Which routes.** `src/emit/futamura/routes.rs` decides per GET
action. It walks the classes the `ruby` target emits (controllers with
their filter chain spelled out in `process_action`, views, models,
library classes), starting from one action's arm of `process_action`,
and closes over calls. An error diagnostic from the `ruby` analysis
means a construct may be emitted wrong, so an action whose walk
reaches one stays on Rails. The walk over-approximates: a send whose
receiver the analysis did not type reaches every app instance method
of that name, and a `case` on a value it cannot decide reaches every
arm. Errors are attributed to the method whose body holds the
expression; methods the lowering synthesized (attribute readers and
writers, `from_stmt`, row classes) borrow spans from the source and do
not own errors. Also left to Rails, whatever the walk says: every verb
but GET (writes keep the model callbacks gems add, which the lowered
models do not have), and any action a route the lowered router refused
could shadow (Rails would match the refused route first).

`ROUNDHOUSE_FUTAMURA_LEDGER=1` prints the decision for every GET
action, and for each one left to Rails the first error and the call
path to it (`=2` also lists every error with the methods that own it):

```
rails  GET /top home#top: reaches 1 error(s), first app/controllers/home_controller.rb:267
       `>` with incompatible operand types: Integer | String | ... | nil? > Integer
       via HomeController#process_action -> HomeController#top
```

That makes the `ruby` target's error ledger read as a list of routes,
ordered by what they cost: clearing an error moves the routes it
blocked onto the lowered path.

**What crosses the boundary.** A middleware just after Rails' session
store hands a GET to the lowered dispatcher; the dispatcher declines
any action not on the list before app code runs, and Rails serves it.
For a served request:

- *The session* goes in as a Hash of its String values
  (`roundhouse.session`), and the keys the lowered app changes are
  written back into Rails' session, which Rails' own store persists.
  Nothing is signed or verified twice. CSRF tokens are Rails' format on
  both sides, so a form the lowered app renders posts to Rails and
  verifies.
- *The flash* goes in from Rails' session (`flashes` minus `discard`)
  and what survives the request goes back.
- *The database connection* is Rails' own, checked out of its pool and
  lent for the request (`Db.with_lent_connection`). A box loads its own
  copy of every native extension, `sqlite3` included, so the lowered
  app opening the database file itself would put two SQLite libraries
  on one file in one process, which SQLite documents as a corruption
  hazard (each copy can release the other's POSIX locks). Calling
  through Rails' connection object runs Rails' copy. It also keeps
  Rails' open transaction in view, so a test's fixtures are what the
  lowered app reads, and its writes roll back with the test.

No model object built on one side ever reaches the other, so the
coupling that makes model stand-ins hard (class identity in `is_a?`,
policy and serializer lookup by class name) never arises on a replaced
route.

**Not shared.** `Rails.cache`: the lowered app keeps its own in-memory
store, and a key Rails deletes stays cached on the lowered side until
it expires. Sharing the store is not a matter of pointing at it: a
cached value holding model objects would load as the other side's
classes.

**Gates.** Real-blog's own suite on the emitted tree under Ruby 4.1 and
`RUBY_BOX=1` gives the same result as on the source (21 runs, 0
failures, its GET requests served by the lowered app inside Rails' test
transactions). Lobsters' 26 parity routes are content-equivalent to
plain Rails under `scripts/lobsters-content-diff`.

**Measured** (October 2026, rubix3, Ruby master 46033eff6c + YJIT, the
frozen 114-visit sequence; the analysis clears 48 of Lobsters' 84 GET
actions, which is 40% of the visits; no request fell back):

| | ms per pass |
|---|--:|
| Rails | 1,264 |
| Rails with a memory cache store instead of Lobsters' file store | 1,203 |
| Futamura, specializations 1-2 only | 1,249 |
| the same under `RUBY_BOX=1` | 1,490 |
| Futamura with whole-route replacement | 1,085 |

Per route (p25, ms), lowered routes against Rails and Rails under a
box:

| | Rails | Rails, `RUBY_BOX=1` | whole-route |
|---|--:|--:|--:|
| `/s/:id` | 14.05 | 16.65 | 2.98 |
| `/comments` | 17.69 | 20.39 | 4.61 |
| `/settings` | 7.20 | 8.13 | 1.72 |
| `/u` | 5.60 | 5.94 | 2.47 |
| `/hottest` (left to Rails) | 23.55 | 31.46 | 29.97 |

A lowered route runs 2.3-4.7x faster than Rails. The pass as a whole
gains 1.17x, because `RUBY_BOX=1` slows Rails itself by ~19% and every
route left to Rails pays that. Peak RSS is 386 MB against Rails' 347.
With every GET lowered (the October 12 prototype) the pass ran at
377 ms, 3.3x; the distance between the two is the error ledger.

What it does not settle:

- **The Box tax.** ~19% on every request Rails serves, measured with
  no box created at all; it is the cost of `RUBY_BOX=1` alone. Until it
  shrinks, whole-route pays only when most of the traffic is lowered.
  Under it, a `warn` in a required file prints even with `$VERBOSE`
  nil, so Rails' silenced probe for the `msgpack` gem shows up as a
  warning.
- **Writes.** GET only. A lowered write would skip the model callbacks
  gems add.
- **The cache.** See above.
- **Precision.** The walk is conservative; a `case` over a view's
  partial name reaches every partial, which is what keeps Lobsters'
  front pages on Rails (one nullable comparison in `_single_tag`).

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
