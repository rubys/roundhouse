# Why bother

Rails optimizes for time to market, and at small scale CRuby on a
Puma worker pool is a perfectly fine production target. But
successful applications acquire constraints their authors didn't
anticipate at line one of code, and rewriting the whole thing in
another language is an expensive answer. This page is an argument
that it doesn't have to be the only one.

One thing up front: Roundhouse is not ready for production use
today. The work is far enough along that the argument is testable;
not far enough that anyone should bet a live deployment on it. The
rest of this page assumes that caveat is in scope and doesn't
repeat it.

## Rails is a great early bet

Schema, routes, controllers, views, jobs, mailers, real-time
updates — a small team can put a working product in front of users
in days. The conventions are load-bearing: they encode answers to a
hundred questions you'd otherwise have to argue about, and they let
everyone on the team navigate a codebase they didn't write. CRuby +
Puma on a single VM — Heroku, Fly, Render, a Kamal deployment to
your own hardware — is a perfectly reasonable production target for
the first several years. If the application fails to find an
audience, the runtime was never going to be the problem.

But suppose it succeeds.

## The shape of success

Successful applications acquire constraints their authors didn't
anticipate at line one of code. Most of these constraints have
nothing to do with the application itself — they're imposed by the
world the application now operates in:

* **Cost economics.** A 10× growth in traffic on CRuby + Puma is a
  10× growth in worker processes is a 10× growth in resident memory
  is a 10× growth in the cloud bill. At small scale this is a
  rounding error; at large scale it's a line item the CFO asks
  about.
* **Geography.** Users notice 200 ms more often than they notice
  50 ms. A US-east deployment is fine until your audience is in
  Tokyo or São Paulo. Edge presence is a different deployment
  shape, not a configuration of the same one.
* **Reachability.** Some targets aren't running CRuby and never
  will. Cloudflare Workers, Vercel Edge, Deno Deploy, the user's
  browser tab, an offline-first mobile WebView. If any of these
  matters, "tune your Rails app" is the wrong sentence.
* **Specialized hot paths.** Most large applications discover that
  one or two endpoints are doing 80% of the work. Their cost
  dominates the bill, but rewriting the whole codebase in a faster
  language to win on those routes is a steep price for a localized
  problem.
* **Ecosystem integration.** Embedded in a TypeScript monorepo.
  Shipped as a desktop app via Tauri or Electron. Targeting a
  runtime your platform team already operates. None of these were
  on the original whiteboard.

The cost-economics constraint is worth dwelling on, because it's
the most measurable of the five and the one most often dismissed.
The standard objection — "Rails apps are I/O-bound, the runtime
doesn't matter" — inverts the problem. Because of the GVL, CRuby
scales by forking Puma workers, each carrying its own ~200–400 MB
copy of the framework. Fifty concurrent requests means ten workers
and several GB resident, and the I/O-bound framing is exactly why
that's wasteful: those processes mostly sit blocked on the database
while still pinning memory. A Rust, Go, or Elixir target handles
the same concurrency through threads sharing a single image, at a
fraction of the RAM. The metric that captures this is **requests
per second per gigabyte of resident memory** — and whether you're
paying GB-hours to a PaaS or amortizing a Kamal box, it's what the
bill is actually charging for.

These pressures don't show up evenly. Most applications hit one or
two of them; very few hit all five. The hard part is that you can't
predict in advance which ones will land on you, or when.

## The cost of getting out

The traditional answer when these constraints arrive is to rewrite.
The traditional outcome is that the rewrite takes longer than
planned, costs more than budgeted, and runs concurrently with the
system it's meant to replace for a multi-year period that nobody
enjoys. A meaningful fraction of rewrites are abandoned outright.
The ones that succeed do so because the company invested its best
engineering talent in a project that produced no new user-visible
features for the duration.

This is not an argument against rewrites. They are sometimes the
right call. It is an argument that the *option* to rewrite is
expensive enough to be worth preserving cheaply, ahead of time,
when you have the choice.

## Deployment as a build flag

Rails is unusually amenable to a different bet. Rails has been
carrying a declarative type system in its DSL for twenty years:
schema → model attributes, associations → relationships,
validations → constraints, `before_action` → controller flow,
`render` → view. The imperative parts — controller actions,
helpers, jobs — are small and stylized. From a compiler's vantage,
the application's shape is recoverable from the conventions Rails
already declares, not from arbitrary runtime behaviour.

That makes the typing problem tractable without annotations. A
compiler that follows Rails conventions can type the application
from schema and method flow alone — no RBS, no Sorbet sigs, no
developer effort beyond writing idiomatic Rails. Roundhouse types
the Phase-1 Rails 8 MVC fixture this way, with zero diagnostics
enforced on every commit.

If the shape is recoverable and the types are inferable, Rails is
already most of a specification. The remaining work is to define
the subset of Ruby semantics that Rails applications actually use,
lower it through a target-neutral IR, and emit projects in whatever
runtime the deployment requires: Rust binary on a small VM,
TypeScript bundle for the edge, Crystal or Go service, Elixir OTP
application, Python project, browser bundle backed by IndexedDB,
or — for staying inside the Ruby ecosystem with native-binary
performance — Spinel-compiled Ruby-to-C. The Rails-shape of the
application doesn't have to be coupled to the CRuby-shape of the
runtime.

The financial term for this is option value. The cost of preserving
optionality — staying inside the Rails-as-specification subset,
accepting some discipline in what your application is allowed to
do — is modest. The payoff, in the worlds where one of the five
constraints above lands on you and rewriting would otherwise be the
only path, is large enough to dwarf the cost.

## State of the work

This is the third iteration of the bet.
[Juntos](https://www.ruby2js.com/docs/juntos/) showed that a
Rails-shaped application could be transpiled source-to-source into
JavaScript and run in browsers, on Node, or on edge platforms; it
remains the working proof for the V8-isolate and offline-browser
targets. [Railcar](https://github.com/rubys/railcar) was a
Crystal-based predecessor that taught which bets were worth keeping
and where the shape needed to change.
[Roundhouse](https://github.com/rubys/roundhouse) is the current
attempt: a typed IR with multiple target emitters sharing one
analyze-and-lower pipeline, so a new target is glue plus a runtime,
not a fork of the compiler.

Targets in flight — Rust, TypeScript, Crystal, Elixir, Go, Python,
and Spinel (Matz's Ruby-to-C compiler, used as a stay-in-Ruby
native-binary path) — share that pipeline. The Phase-1 Rails 8 MVC
fixture transpiles end-to-end through analyze, lower, and emit; a
DOM-diff harness compares emitted runtimes against the original
Rails app, so a template that renders differently in any target is
a bug. Per-target runtime integration is the work that remains.

The point of this page isn't to claim the work is done. It's to
argue that the work is worth doing — that "Rails is great for time
to market" and "Rails locks you into CRuby for a decade" are two
halves of the same trade, and the trade is one that doesn't have
to stand.
