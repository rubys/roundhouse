<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-on-dark.svg">
    <img src="assets/logo.svg" alt="Roundhouse logo — a turntable at the center, six colored tracks radiating outward" width="200">
  </picture>
</p>

# Roundhouse

*Rails as a specification; deployment is a build flag.*

Roundhouse reads Ruby source — specifically, Rails applications — and
produces standalone projects in other target languages. The deployment
target (Rust or Swift binary, TypeScript bundle, Crystal or Go service,
Elixir OTP app, Kotlin/JVM or C#/.NET service, Python project, browser
bundle, or Spinel-compiled Ruby) becomes a compiler flag rather than a
runtime choice.

A roundhouse is the circular hub in a rail yard where engines rotate and
route onto different tracks. That's the pipeline shape: one Ruby source
at the center, analyzed and dispatched to one of N target tracks.

*For the case for doing this at all — the constraints that push
successful Rails apps off CRuby, and the option value of preserving
the choice — see [WHY.md](WHY.md).*

## Overview

The emitted projects compile clean and pass their tests. The way we
know they're correct is a conformance oracle: the same URL fetched from
Rails and from each target must produce the same response, checked
three ways — emitted unit tests against fixed expected values, a
differential compare gate against live Rails (DOM-node-for-DOM-node
for HTML, value-for-value for JSON), and end-to-end browser tests for
the dynamic behavior a static diff can't reach.

No type annotations are involved anywhere. Rails was already typed:
`has_many :comments` is a type declaration, and the framework's
conventions carry implicit type information that was simply never
written down. Roundhouse recovers it by whole-program inference —
which class an association returns, which columns a model has and what
they deserialize to, whether a `find_by` came back nil — statically,
from unmodified source, without booting the app or touching a
database.

That inference is a product in its own right, not just the compiler's
enabler. The same engine that emits Rust answers an editor's or an
agent's questions — *what's the type here? can this be nil?* — through
an LSP server, an MCP server, and an
[in-browser IDE](https://rubys.github.io/roundhouse/ide/): no
annotations, no app boot, no database, no warm server to babysit. A
whole-application pass over Mastodon — 1,173 files, all 337
controllers, HAML views included — takes about 1.5 seconds natively
and 2.3 seconds compiled to WebAssembly, which is how the IDE analyzes
Mastodon in a browser tab; individual queries like typed completion
answer in a couple of milliseconds from the last completed pass.
Static,
deep, and annotation-free is a cell of the Ruby tooling space nobody
else occupies: ruby-lsp is static but stops at names; ruby-lsp-rails
and Tidewave are deep but need a running app; Sorbet and Steep are
static and deep but you pay for it in annotations.

The performance story is partial evaluation. Rails is, operationally,
an interpreter for your application — routes, associations,
validations, and templates are data it consults on every request.
Every decision whose answer cannot differ between requests, Roundhouse
makes once at transpile time; only the per-request residue survives to
runtime. On the benchmark fixture, serving the HTML index on a fixed
Linux x86 server (July 2026 round):

| configuration | req/sec |
|---|--:|
| Rails on CRuby+YJIT | 326 |
| Rails on JRuby | 1,066 |
| Roundhouse emit on CRuby+YJIT | 3,292 |
| Roundhouse emit on JRuby | 24,172 |

Two effects compose there: stripping Rails' interpretive layers is
worth an order of magnitude on the same interpreter — same Ruby, same
YJIT, ~10× — and the static, monomorphic Ruby that remains is the
input the JVM JIT was built for, worth a further ~7× where stock
Rails gains ~3×. End to end that's ~74×, and the compiled
targets go further still: the Kotlin emit roughly doubles
emitted-Ruby-on-JRuby on the JSON endpoint, and the Rust binary serves
the same app in under 20 MB of memory where Rails holds ~320 MB. These
are ratios from a CPU-bound microbenchmark of a small fixture — real
workloads are I/O-bound to varying degrees, and the absolute numbers
shift between rounds as performance gates land. The live numbers,
per-run data, and environment capture are at
[bench](https://rubys.github.io/roundhouse/bench/), and the caveats
are spelled out honestly in the posts below.

The long-form versions of this overview:

- [Conformance vs Comprehension](https://intertwingly.net/blog/2026/06/27/Conformance-vs-Comprehension.html) — the project, and the conformance-oracle methodology behind it
- [Live Types for Rails](https://intertwingly.net/blog/2026/06/25/Live-Types-for-Rails.html) — the inference as a live type checker: LSP, MCP, and the competitive landscape
- [An IDE You Don't Install](https://intertwingly.net/blog/2026/07/06/An-IDE-You-Dont-Install.html) — Mastodon analyzed in a browser tab, and a correction to the Live-Types timing numbers
- [The Ruby JRuby Was Built to Run](https://intertwingly.net/blog/2026/06/11/The-Ruby-JRuby-Was-Built-to-Run.html) — the 2×2 experiment the table above is the current round of
- [Numbers Without Conclusions](https://intertwingly.net/blog/2026/05/25/Numbers-Without-Conclusions.html) — full benchmark methodology, and what the numbers are and aren't evidence of

## Pipeline

```
          ingest       analyze        lower         emit
Ruby ────▶ AST ─────▶ typed IR ────▶ IR ─────▶ target project
                         │             │
                         ▼             ▼
                    diagnostics    runtime/<target>/
```

Ingest normalizes Ruby + ERB into a small typed IR. Analyze annotates
every expression with a type and effect set, flowing types along the
edges Rails conventions already draw (schema → models, associations,
before_action, render → view, partials). Lower expands Rails-dialect
nodes into target-neutral IR — validations become `Check` enums, routes
become a flat dispatch table, controller bodies become a walker-ready
`LoweredAction`. Additional passes canonicalize controller idioms
(`params.to_h`, `redirect_to`, path helpers, association builders) and
query DSL (`order`, `includes`, `where`) into shapes each emitter
consumes directly. Emit walks the IR per target, consulting each
expression's type and effect where the target needs it, and each emitted
project links a small hand-written `runtime/<target>/` library for the
bits that don't belong in generated code (DB connection, HTTP server,
Action Cable).

Diagnostics surface anything the analyzer couldn't type or
intentionally left gradual — the subset of programs we can transpile
is defined by "zero error diagnostics" (RBS-declared `untyped` sites
surface as warnings; strict-target emitters elevate to errors at emit
time).

## Current state

The analyzer fully types the Phase-1 Rails 8 MVC fixture
(`fixtures/real-blog`) without annotations — schema-derived attributes,
associations, controller actions, `before_action` flow, views,
partials, and collection rendering all resolve to concrete types.
A test enforces zero error diagnostics on every commit. The framework
runtime (`runtime/ruby/`) is held to the same bar via
`every_runtime_method_body_is_fully_typed` — no inference gaps in any
method body.

Ten target emitters are live and DOM-equivalent against Rails on
real-blog as a CI invariant — Rust, TypeScript, Crystal, Elixir, Go,
Kotlin, Swift, Python, C#/.NET, and Spinel-shape Ruby. Each boots an
HTTP + Action Cable server, serves the generated blog with working
forms, validation error display, Turbo streams, and Tailwind styling. A
`compare-<target>` job in `.github/workflows/ci.yml` runs on every push
to `main` (JRuby is compared too, serving the same emit on the JVM), so
any drift turns CI red.

Cross-runtime correctness is enforced by `tools/compare/`, which
fetches the same URL from Rails and from any roundhouse-emitted runtime
and diffs the canonicalized DOM trees. A new ERB pattern that renders
differently between Rails and a target is a bug.

## See it for yourself

**Meet the fixture.** [rubys.github.io/roundhouse/demo](https://rubys.github.io/roundhouse/demo/)
describes the `fixtures/real-blog` app every target is built and tested
against — how `scripts/create-blog` scaffolds it, which Rails features
it exercises (associations, nested routes, Turbo Streams, Action
Cable, Tailwind), and the three test layers (per-target model/controller
tests, DOM-equivalence compare, and Playwright E2E).

**Browse the emitted outputs.** [rubys.github.io/roundhouse/browse](https://rubys.github.io/roundhouse/browse/)
shows what every target emitter produces from `fixtures/real-blog`,
updated on each push to `main` — Rust, TypeScript, Crystal, Elixir,
Go, Kotlin, Swift, Python, C#/.NET, plus Ruby and JRuby, and Spinel
(the lowered output that runs as the demo below).

**Compare performance.** [rubys.github.io/roundhouse/bench](https://rubys.github.io/roundhouse/bench/)
plots throughput, memory, latency, and req/sec/GB across the live
targets on the same `fixtures/real-blog`, run on a fixed Hetzner box.

**Run the demo.** A working transpiled blog — articles, comments,
real-time Turbo Stream broadcasts over WebSocket, SQLite persistence,
Tailwind styling, create + destroy flows — in two `bin/rh` commands:

```sh
git clone https://github.com/rubys/roundhouse
cd roundhouse
bin/rh fixture            # generate the Rails fixture (~60s)
bin/rh dev ruby           # transpile + assets + serve on :3000 (~3-5min cold)
```

Run `bin/rh doctor` first to see which prerequisites are installed and
which subcommands are available without a Rust toolchain (`bin/rh
fetch <target>` downloads pre-transpiled archives).

Building roundhouse itself needs Rust plus a working libclang: the
`ruby-prism-sys` / `ruby-rbs-sys` build scripts generate their C
bindings with bindgen, which loads clang's own resource headers. On
Debian/Ubuntu that means both packages —

```sh
sudo apt install clang libclang-dev
```

— installing only `libclang-dev` fails with
`fatal error: 'stddef.h' file not found` (clang's builtin headers ship
in the `clang` package). macOS with Xcode Command Line Tools needs
nothing extra.

Prerequisites and the architecture of what gets generated:
[`runtime/spinel/scaffold/README.md`](runtime/spinel/scaffold/README.md).

**Analyze your own app.** Point the checker at any Rails checkout to
see what the analyzer can type today — no annotations, no `bundle
install`, no booting, no database:

```sh
cargo run --release --bin roundhouse-check -- --continue /path/to/your/rails/app
```

`--continue` is the mode you want on a real app: constructs the
ingester doesn't recognize yet are recorded and skipped instead of
aborting, and a deduplicated punch list of them is printed at the end.
Read the output accordingly — `error`/`warning` diagnostics are sites
the analyzer understood but couldn't type (or typed gradually), while
the punch list and any gap-attributed notes are roundhouse's own
coverage gaps, not problems in your app. Expect a real app to produce
plenty of both today: the numbers are the project's honest to-do list,
and they drop week over week. (Without `--continue`, ingest is strict
and exits on the first unrecognized construct — the right mode for
fixtures the analyzer is expected to fully cover.)

## Workflow runner (`bin/rh`)

`bin/rh` is the single entry point for every workflow below. Ruby is
the only prerequisite for the onboarding subcommands; the build
subcommands shell out to `cargo`. Run `bin/rh --help` for the full
surface and `bin/rh <command> --help` for per-command options.

Onboarding (no Rust required):

- `bin/rh doctor` — check prerequisites; list which subcommands work today.
- `bin/rh fetch <target>` — download a pre-transpiled archive into `downloads/<target>/`.
- `bin/rh fixture` — generate the Rails source fixture via `rails new` + scaffold.

Build (requires Rust):

- `bin/rh transpile <target>` — build `fixtures/real-blog` into `build/transpiled-blog-<target>/`.
- `bin/rh dev | test | run <target>` — transpile, then run the emitted tree's dev/test/run action (ruby today).
- `bin/rh compare [<target>]` — fetch the same URL from Rails and the target, diff canonicalized DOM.
- `bin/rh bench [<target>...]` — HTTP throughput + RSS benchmark across targets.
- `bin/rh site` — build the full multi-target Pages site (the one linked above).

Cleanup: `bin/rh clean <target | fixture>`.

Targets: `spinel`, `ruby`, `jruby`, `crystal`, `csharp`, `elixir`, `go`,
`kotlin`, `python`, `rust`, `swift`, `typescript`, `typescript-worker`.

## Supporting pieces worth knowing

- **Method catalog** (`src/catalog/`) — one IDL-shaped table declaring
  effect class, chain semantics, and return-type facets for every AR
  method the compiler recognizes. Single source of truth; replaced five
  scattered places.
- **Database adapter** (`src/adapter.rs`) — `DatabaseAdapter` trait
  behind which effect classification and async-suspension decisions
  live. `SqliteAdapter` / `SqliteAsyncAdapter` today; Postgres /
  IndexedDB / D1 / Neon land as sibling impls.
- **Per-target runtimes** (`runtime/<target>/`) — hand-written glue
  (DB connection, HTTP, view helpers, Action Cable, test support)
  included verbatim by the matching emitter.

## Running the tests

```
cargo test                              # unit + analyze + ingest + emit
cargo test --test real_blog             # the Phase-1 forcing functions
cargo test --test rust_toolchain -- --ignored   # Rust end-to-end boot
```

The `real-blog` fixture is generated on demand — `bin/rh fixture` runs
`scripts/create-blog` and materializes it under `fixtures/real-blog/`.
CI regenerates the fixture once per run and shares it across the unit
job and each per-target toolchain job.

## Documentation

- [`DEVELOPMENT.md`](DEVELOPMENT.md) — day-to-day dev loop, the
  `roundhouse-ast` debugging tool, adding a new IR variant.
- [`docs/data/`](docs/data/) — the compiler's inputs, one doc each for
  Ruby + ERB, schema/routes/seeds, the method catalog, and the
  database adapter.
- [`docs/pipeline/`](docs/pipeline/) — pipeline internals: analyze,
  lower, emit, runtime integration, verification.

## Prior art

- [railcar](https://github.com/rubys/railcar) — the Crystal-based predecessor; taught us which bets were worth keeping and where the shape needed to change.
- [ruby2js](https://www.ruby2js.com) — transpiles Ruby to JavaScript; originator of the filter/escape-hatch pattern for per-app transformations.
- [Juntos](https://www.ruby2js.com/docs/juntos/) — ruby2js extension that transpiles entire Rails apps; validated the multi-target ambition against Basecamp's Writebook.

## Contributing

Issues and discussion are welcome. Architecture is still forming —
a quick conversation before a PR is usually the most helpful path.

## License

Dual-licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
