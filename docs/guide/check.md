# `roundhouse check` — analyze an app from the command line

`check` runs the whole analysis — ingest, type and effect inference,
diagnosis — over a Rails checkout and prints what it found. It is the
same engine the editor and the MCP server use, in the shape that fits
a terminal and a CI job: one command, one exit code.

```sh
roundhouse check --continue /path/to/your/rails/app
```

Nothing is booted and nothing is installed. The analyzer reads
`db/schema.rb` (or the migrations), `config/routes.rb`, the models,
controllers, concerns, helpers, views and `Gemfile.lock`, and infers
from Rails' own conventions what was never written down: which class
`has_many :comments` returns, what a column deserializes to, whether a
`find_by` can come back nil.

## What is read, and what you can write down

Ingest walks `db/schema.rb` (or the migrations), `config/routes.rb`,
`Gemfile.lock`, and the whole of `app/`: models, controllers, views,
helpers and concerns each have a pass of their own, and every other
directory under `app/` — `services`, `jobs`, `interactors`,
`presenters`, whatever the app calls its layers — is walked for the
classes it holds. Rails autoloads all of `app/*`, so the list is
discovered rather than guessed. `lib/` and `extras/` are walked too,
plus anything `config/application.rb` adds to `config.autoload_paths`
or `config.eager_load_paths`. `config.autoload_lib(ignore: %w[assets
tasks])` is honored: a directory the app takes off its own load path
is off the walk as well, which is where a RuboCop cop or a test-support
tree under `app/` goes if you do not want it analyzed. Packwerk's
`packs/*/app/*` layout is not walked yet.

One thing the walk carries that no emitted tree can: a class extending
a Rails base the runtime does not port. `ApplicationMailbox <
ActionMailbox::Base` (and every mailbox under it), an
`ActiveModel::EachValidator`, a `Rails::Generators::NamedBase` — `check`
types their bodies like any other app code, but there is no such
constant in an emitted tree to subclass, so the transpile drivers drop
the class, its subclasses with it, and report a `class dropped:` warning
naming the base. The bases that *are* ported — `ActiveRecord::Base`,
`ActionController::Base`/`API`, `ActionMailer::Base`, `ActiveJob::Base`,
the two ActionCable roots, `ActiveSupport::CurrentAttributes` — carry as
before, and so does a gem's own base (`SVG::Graph::TimeSeries`,
`ActiveModel::Serializer`): that class is kept and its body replayed for
the gem to run.

Inference goes as far as the conventions carry it. Where they run out —
a value handed back from a gem the census does not model, a method
whose return type depends on a branch the analyzer cannot follow — the
app can write the answer down, in either of two places that feed the
same table:

- **`sig/**/*.rbs`** — RBS written for roundhouse, keyed by class and
  method. This is the sidecar the analyzer has always read; it wins
  wherever both sources declare the same method. `instance` is
  readable here, and `self` on an instance-side member: both mean an
  instance of the class that RECEIVED the call, so one line on a base
  class types the inherited factory for every subclass
  (`def self.instance: () -> instance`) instead of one line per
  subclass. Declare a namespaced class with nested `module` blocks —
  a declaration written `class A::B` is currently keyed as `B`.
- **Sorbet `sig` blocks** — annotations an app on sorbet-runtime has
  already written, read in place from `app/` and `lib/`. A
  `sig { params(user: User).returns(T.nilable(String)) }` above a `def`
  becomes that method's signature; `const` and `prop` in a `T::Struct`
  declare typed readers (and, for `prop`, writers). The grammar read is
  `params` / `returns` / `void`, the modifiers (`override`, `abstract`,
  `checked`, …), and for types `T.nilable`, `T.any`, `T.untyped`,
  `T::Boolean`, `T::Array[…]`, `T::Hash[…, …]` and class constants.
  `T.attached_class` reads as the self type — an instance of the class
  that RECEIVED the call, substituted with that class at dispatch, so
  a factory declared once on a base class answers with an instance of
  whichever subclass called it. `T.self_type` reads the same way on an
  instance-side `def`; on a `def self.x` — or on any `def` inside
  `class << self`, which is class-side too — it means the class
  object, which has no type to give it, so it stays unread. A `sig`
  outside all that — `type_parameters`, shapes, proc types, a
  parameter name the `def` does not have — is dropped whole and the
  method is inferred as before; a `sig` above `attr_reader` or above
  `private def` is not paired.
  `T.let`, `T.cast`, `T.must`, `T.bind`,
  `T.unsafe` and `T.assert_type!` unwrap to the value they wrap so its
  own inferred type flows on; the annotation on those is discarded, so
  `T.must(x)` does not narrow `x` past what inference already knows.

The two sources above settle against each other — the sidecar wins
where both declare the same method. Against INFERENCE, neither wins:
where the analyzer can type a method's body, the return harvested from
that body overwrites the declared one at dispatch, whichever source
declared it. A sidecar saying `Integer` over a body that plainly
returns a string reads as `String` at the call site, and the table
still holds the `Integer` that lost. So a declaration is worth writing
where inference runs OUT — the boundary these annotations exist for —
and is not a way to correct inference where it has an answer you
disagree with. Silent either way, today.

The seeds feed the analyzer's dispatch table, which is what `check`,
the editor, the MCP server and the emitters' typing read; the Spinel
lane's own `.rbs` sidecar is typed from the lowered code rather than
from them.

## The two modes

`--continue` is the mode you want on a real app. Constructs the
ingester does not recognize yet are recorded and skipped rather than
aborting, and a deduplicated list of them is printed at the end.

Without it (the default, also spelled `--strict`), ingest stops at the
first unrecognized construct and exits 2. That is the right mode for an
app you expect roundhouse to cover completely — a CI gate that should
go red if a new construct sneaks in — and the wrong one for a first
look at an unfamiliar codebase.

`ROUNDHOUSE_INGEST_SURVEY=1` in the environment is the same as
`--continue`; useful where the command line is not yours to edit.

## A clean run

The Rails Guides store — the app the *Getting Started with Rails*
guide builds, checked in at `fixtures/store` — checks clean:

```
$ roundhouse check fixtures/store
roundhouse-check: 24 gems: 9 framework, 2 modeled, 13 infrastructure, 0 unknown
roundhouse-check: fixtures/store — 0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)
```

Exit status 0. Everything the analyzer saw, it typed.

## Reading the output of a real app

A real app prints more. Every line is one of five kinds, and the
summary line at the end counts them. Which are *your* problem and which
are roundhouse's is the whole point of the layout.

```
$ roundhouse check --continue ~/src/mastodon
config/initializers/inflections.rb:5:46: error[parse]: expected an expression after `=`
app/controllers/concerns/web_app_controller_concern.rb:25:7: error[send_dispatch_failed]: no known method `[]` on ENV
app/controllers/about_controller.rb:9:16: warning[gradual_untyped]: method call resolves to RBS `untyped` (gradual escape)
app/views/admin/collections/show.html.haml:21:105: warning[missing_preload]: iterating this relation reads `x.account`, but the query at app/views/admin/collections/show.html.haml:21 does not preload :account — add `.includes(:account)`
app/helpers/domain_control_helper.rb:16:7: note[send_dispatch_failed]: no known method `blocked?` on DomainBlock — likely roundhouse coverage, not an app error (ingest gap in app/models/domain_block.rb: unsupported statement inside `class << self`: AliasMethodNode)
…

── Survey: 138 ingest gap(s), 20 distinct kind(s) ──
  [30×] class-body macro not expanded: `vary_by` from CacheConcern holds a statement that is not filter DSL
        app/controllers/activitypub/collections_controller.rb
        … and 29 more file(s)
  [3×] unsupported routes DSL: `concern`
        config/routes.rb
        config/routes/admin.rb
        config/routes/api.rb
  …

roundhouse-check: 154 gems: 6 framework, 6 stdlib, 11 modeled, 70 infrastructure, 61 unknown (active_model_serializers, base58, …)
roundhouse-check: /Users/you/src/mastodon — 6 parse error(s), 674 error(s), 5035 warning(s), 704 gap-attributed note(s), 138 survey gap(s)
```

(Paths are printed absolute; shortened here.)

**`error[parse]`** — Prism could not parse the file. These lead the
output because a malformed file is usually the root cause of whatever
analysis noise follows it. Real syntax errors are yours; a construct
Prism handles that roundhouse's ingest of the recovered tree does not
is roundhouse's. Either way, look at these first.

**`error[…]`** — the analyzer understood the site and could not type
it: an instance variable no filter or action assigns, a method dispatch
on a receiver with no such method, an operator applied to incompatible
operands. Anything left at error severity after attribution (below) is
a finding: either the app has a latent bug, or the analyzer is wrong
about something it thinks it understands. Both are worth a look, and
the second is a bug report roundhouse wants.

**`warning[…]`** — two populations share this severity.
`gradual_untyped` and `unresolved_type` are the coverage ledger: a call
resolved to an RBS `untyped` (the gradual escape hatch) or to nothing
at all. There are hundreds on any real app; they are neither errors in
your code nor, individually, interesting. `missing_preload` is the one
warning that *is* a finding about your app: a static N+1, naming the
association read inside the loop, the query that built the relation,
and the `.includes` that fixes it.

**`note[…] — likely roundhouse coverage, not an app error`** — a
diagnostic that would have been an error, downgraded because roundhouse
can trace it to its own gap. Two tails: *(ingest gap in …)* means a
construct listed in the survey report below prevented the analyzer
from seeing a definition, and *(the `X` gem is in the Gemfile and
roundhouse does not model it)* means the receiver comes from a gem the
census lists as unknown. Skip these on a first read. They exist so the
error count above means "findings", not "shadows of gaps".

**The survey report** — printed only with `--continue`: every construct
ingest skipped, bucketed by kind, most frequent first, with the files
it occurred in. This is roundhouse's own to-do list for your app.
Nothing in it is a problem with your code.

**The gem census** — printed when there is a `Gemfile.lock`: each
direct dependency classified as *framework* (Rails and its plumbing),
*stdlib*, *modeled* (roundhouse resolves its app-facing surface),
*infrastructure* (servers, linters, test drivers, adapters — never
enters the analysis), or *unknown*. The unknown list is printed in
full. A dispatch into an unknown gem's DSL or classes cannot be typed,
and those failures are labelled as notes rather than counted as
errors — so on an app with a long unknown list, the census is the
first thing to read: it says how much of the error count is even
reachable today.

## Exit status

| Status | Meaning |
|---|---|
| 0 | No parse errors and no analysis errors. Warnings, notes and survey gaps do not affect it. |
| 1 | At least one parse error or analysis error. |
| 2 | Bad arguments; a path that is not a directory or has no `app/` (a typo must not check clean); or, without `--continue`, ingest aborted at an unsupported construct. |

That makes `roundhouse check app/` a usable CI gate for an app that
checks clean today, and `roundhouse check --continue app/` a usable
one for an app that doesn't: the second stays green while roundhouse's
coverage grows and goes red only when the *findings* change.

## What the numbers are and aren't

On the store, the blog fixture, Campfire and the Rails tutorial's
sample app, `check` reports zero errors and zero warnings, and CI
asserts that on every commit. On Mastodon it reports the thousands
above. Both numbers are honest: the first says those apps are inside
the analyzer's coverage, the second says how far Mastodon is outside
it, and where. The counts on large apps drop week over week as
constructs move from the survey report into the analyzer; the
snapshot's [`RELEASES.md`](../../RELEASES.md) entry records where
they stood when it shipped.

The same diagnostics, filtered and queryable, are what the editor
shows in its Problems panel ([`editor.md`](editor.md)) and what an
agent gets from the `diagnostics` tool ([`mcp.md`](mcp.md)).
