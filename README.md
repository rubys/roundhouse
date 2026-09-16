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

## What it does

Three things, from one binary and one analysis. Each has a door in the
[user guide](docs/guide/README.md).

**Analyze.** No type annotations are involved anywhere. Rails was
already typed: `has_many :comments` is a type declaration, and the
framework's conventions carry type information that was simply never
written down. Roundhouse recovers it by whole-program inference —
which class an association returns, which columns a model has and
what they deserialize to, whether a `find_by` came back nil — from
unmodified source, without booting the app or touching a database. A
pass over Mastodon (1,173 files, all 337 controllers, HAML views
included) takes about 1.5 seconds. That inference is a product in its
own right: `roundhouse check` for a terminal or a CI gate, an
[LSP server](docs/guide/editor.md) for your editor, an
[MCP server](docs/guide/mcp.md) for your agent — types, nil-safety,
static N+1 findings, and the full request trace for any action — and
an [in-browser IDE](https://rubys.github.io/roundhouse/ide/) that
analyzes a folder on your disk without uploading it. Static, deep and
annotation-free is a cell of the Ruby tooling space nobody else
occupies: ruby-lsp is static but stops at names; ruby-lsp-rails and
Tidewave are deep but need a running app; Sorbet and Steep are static
and deep but you pay in annotations. →
[`check`](docs/guide/check.md) · [editor](docs/guide/editor.md) ·
[agent](docs/guide/mcp.md) · [IDE](docs/guide/ide.md)

**Transpile.** The same analysis fed to a dozen emitters: a
standalone project in Rust, Go, TypeScript, Crystal, Elixir, Kotlin,
Swift, Python, C#, or Ruby, with its own tests and no Rails at
runtime. The way we know the output is correct is a conformance
oracle: the same URL fetched from Rails and from each target must
produce the same response — emitted tests, a differential compare
against live Rails (DOM node for DOM node, JSON value for value), and
browser end-to-end tests for what a static diff can't reach — and
every target on the list passes it on every push. →
[`--target`](docs/guide/transpile.md) · [targets](docs/guide/targets.md)
· [what of Rails comes through](docs/guide/rails-coverage.md) ·
[verifying](docs/guide/verifying.md)

**Compile.** The Ruby shape compiled ahead of time to one native
binary by [Spinel](https://github.com/matz/spinel), Matz's AOT Ruby
compiler: one executable, one SQLite file, no interpreter. Among the
compiled targets it has the closest behavior to Rails by a distance,
and will for the foreseeable future, because it runs the framework
runtime itself rather than a translation of it. Basecamp's Campfire
runs this way — every page and every cable frame compared against
live Rails on every push, and a
[Docker archive](https://rubys.github.io/roundhouse/apps/campfire.html)
you can run in minutes. → [Spinel](docs/guide/spinel.md)

## Get it

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/rubys/roundhouse/releases/latest/download/roundhouse-installer.sh | sh
roundhouse --version
roundhouse check --continue /path/to/your/rails/app
```

Releases are dated snapshots with binaries for macOS on Apple silicon
and Linux x86-64; [`RELEASES.md`](RELEASES.md) says what each one
contains. The binaries cover most needs, and `cargo build --release
--bin roundhouse` from a checkout is always there for the rest —
[`docs/guide/install.md`](docs/guide/install.md) has both paths and
the prerequisites.

## See it for yourself

- [**IDE**](https://rubys.github.io/roundhouse/ide/) — the analyzer
  in a browser tab, preloaded with the Rails Guides store, the blog,
  Lobsters, Campfire and Mastodon; *open folder…* for your own app.
- [**Campfire**](https://rubys.github.io/roundhouse/apps/campfire.html)
  — the compiled product, as a Docker archive.
- [**Browse**](https://rubys.github.io/roundhouse/browse/) — what every
  emitter produces from the blog fixture, updated on each push.
- [**Bench**](https://rubys.github.io/roundhouse/bench/) — throughput,
  memory and latency across the live targets on a fixed box, against
  Rails as it ships.
- [**Demo**](https://rubys.github.io/roundhouse/demo/) — the fixture
  every target is built and tested against, and its three test layers.

## Why it is fast

Rails is, operationally, an interpreter for your application —
routes, associations, validations and templates are data it consults
on every request. Every decision whose answer cannot differ between
requests, Roundhouse makes once at transpile time; only the
per-request residue survives to runtime. On the blog fixture, serving
the HTML index on a fixed Linux x86 server (July 2026 round):

| configuration | req/sec |
|---|--:|
| Rails on CRuby+YJIT | 326 |
| Rails on JRuby | 1,066 |
| Roundhouse emit on CRuby+YJIT | 3,292 |
| Roundhouse emit on JRuby | 24,172 |

Stripping the interpretive layers is worth ~10× on the same
interpreter; the static, monomorphic Ruby that remains is the input
the JVM JIT was built for, worth a further ~7×; the compiled targets
go further still. These are ratios from a CPU-bound microbenchmark of
a small fixture, and the live numbers with their environment capture
are on the [bench page](https://rubys.github.io/roundhouse/bench/).
The long-form versions:

- [Conformance vs Comprehension](https://intertwingly.net/blog/2026/06/27/Conformance-vs-Comprehension.html) — the project, and the conformance-oracle methodology behind it
- [Live Types for Rails](https://intertwingly.net/blog/2026/06/25/Live-Types-for-Rails.html) — the inference as a live type checker: LSP, MCP, and the competitive landscape
- [An IDE You Don't Install](https://intertwingly.net/blog/2026/07/06/An-IDE-You-Dont-Install.html) — Mastodon analyzed in a browser tab
- [Campfire Chats](https://intertwingly.net/blog/2026/09/01/Campfire-Chats.html) — a real product, compiled
- [The Ruby JRuby Was Built to Run](https://intertwingly.net/blog/2026/06/11/The-Ruby-JRuby-Was-Built-to-Run.html) — the 2×2 experiment the table above is the current round of
- [Numbers Without Conclusions](https://intertwingly.net/blog/2026/05/25/Numbers-Without-Conclusions.html) — benchmark methodology, and what the numbers are and aren't evidence of

## Documentation

Using it: the [user guide](docs/guide/README.md) — one page per door
above, starting at [install](docs/guide/install.md) — and
[`RELEASES.md`](RELEASES.md).

Working on it: [`DEVELOPMENT.md`](DEVELOPMENT.md) (build, test, the
`bin/rh` workflow runner, debugging tools, repo map),
[`AGENTS.md`](AGENTS.md) (the invariants not to break), and
[`docs/`](docs/README.md) — the architecture: the compiler's
[inputs](docs/data/), the [pipeline](docs/pipeline/) (analyze, lower,
emit, runtime, verification), and the working plans.
[`BETS.md`](BETS.md) is why this attempt is shaped differently from
its predecessors; [`WHY.md`](WHY.md) is why do it at all.

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

`runtime/spinel/tep/` carries code that began as
[tep](https://github.com/OriPekelman/tep) 0.8.1 by Ori Pekelman,
MIT-licensed; see `runtime/spinel/tep/NOTICE`. That directory ships
into every emitted spinel tree, under runtime/tep there.
