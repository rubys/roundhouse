# `roundhouse --target` — transpile an app

```sh
roundhouse --target rust -o out/rust /path/to/your/rails/app
cd out/rust
```

That writes a standalone Rust project: models, controllers, views and
routes translated from your app, a small hand-written runtime library
for the parts that don't belong in generated code (the database
connection, the HTTP server, Action Cable), tests, and a `README.md`
that says how to build and run it. There is no Rails in the output and
no Ruby at runtime. The same command with a different `--target`
writes the same app as a Go module, a TypeScript package, a Crystal
shard, a Mix project, a Gradle build, a Swift package, a Python
project, a .NET solution, or a Ruby tree that runs on stock CRuby or
JRuby — or the Ruby shape that [Spinel](spinel.md) compiles to a
native binary.

## The command

```
roundhouse --target LANG [-o OUT] [INPUT] [--survey] [--allow-unsupported]
```

- `LANG` — one of `crystal`, `csharp`, `elixir`, `go`, `kotlin`,
  `python`, `rust`, `swift`, `typescript`, `typescript-worker`, `ruby`,
  `jruby`, `spinel`. [`targets.md`](targets.md) describes each.
- `INPUT` — the Rails app's root; default is the working directory.
- `-o OUT` — where to write; default is `./out/<lang>/`. The directory
  is created if needed; files are written over an existing one, and
  files a previous run left behind are not removed, so regenerate into
  a clean directory when the app has lost files.

Transpilation is the analysis from [`check.md`](check.md) followed by
lowering and emit, so it accepts an app in exactly the state `check`
reports as clean: zero errors, and every construct recognized. On such
an app the command prints nothing and exits 0. On any other app it
stops, and the two flags decide how.

## Apps that aren't fully covered yet

By default, ingest is strict — the first construct roundhouse does not
recognize aborts the run — and analysis errors fail the emit. That is
the right contract for an app you intend to ship from the output: a
transpile that silently dropped something is worse than one that
refused.

For a first look at an app you know is outside the current coverage:

```sh
roundhouse --target ruby --survey --allow-unsupported ~/src/mastodon -o out/mastodon
```

`--survey` keeps ingest going past unrecognized constructs, recording
each as a gap (with a nil placeholder in its place) and printing the
deduplicated punch list at the end — the same report `check --continue`
prints. `--allow-unsupported` lets emit proceed past the diagnostics
those gaps and the analyzer's own limits produce: a stub is emitted at
each unsupported site, the diagnostics are downgraded to warnings, and
the output is written anyway. Together they answer *how much of this
app comes through today, and what exactly doesn't* — every stub is
marked in the output, and the punch list is the inventory. What they
produce is a survey, not a deployable; a stubbed action renders
nothing useful.

Literal `CurrentAttributes#set` scopes are an explicit exception to
`--allow-unsupported`: recognized affected calls refuse unsupported
targets or block shapes **before writing output**, even with that flag.
Only native generated-project `--target ruby` is validated. JRuby,
Spinel, Roda, WASM (including its Ruby output) and other targets remain
unverified for this construct and refuse it; this is a coverage limit,
not a claim about those languages' capabilities. The verbatim Blog
export is exempt.

The supported form is `Current.set(user: value) { |context| ... }` on
an ingested CurrentAttributes class without a source `set` override:
nonempty literal unique keys with represented instance readers and writers,
an attached literal block, and ordinary required block parameters.
It executes the block, returns its value,
yields the current instance (extra required parameters receive nil),
and preserves lexical captures, `next`, `break`, method `return`, and
exception restoration. Values evaluate once in source order before
instance capture; save/write and restoration follow Rails' per-key
insertion order, including partial setup failures. Nested supported
`Current.set` scopes work. Loops, other nested block calls/closures,
converted/forwarded blocks, rest parameters and bindings the IR cannot
preserve (including block-local, optional, destructured and keyword
parameters) are explicitly refused anywhere in a supported body.
Dynamic hashes, no-block forms and other receivers retain their
existing support/diagnostics. When source classes share a relative path
(a bare name or a suffix such as `Tenant::Current`), literal `set`
receivers using that ambiguous path are conservatively refused: use an
absolute identity such as
`::Tenant::Current.set(...)` or `::Current.set(...)`. This avoids an
existing analyzer ambiguity without inventing lexical dispatch.
The same qualification is required for namespaced bare candidates in
bodies the analyzer does not type, such as association extensions,
and for relative qualified candidates whose resolved class identity
is not represented (for example `Tenant::Current` inside `Outer`).
Scopes in CurrentAttributes class-body initializers, direct route
helpers, association defaults/scopes, and test constants/inner classes
or helper defaults are also refused in this initial execution slice,
as are controller keyword defaults, represented strict-local defaults
and fixture ERB. Existing strict-local header ingestion accepts literal
defaults only; richer header expressions are not represented call sites
and are not validated or repaired by this change.
External eager constant initializers remain supported; Current's own
bootstrap ordering is not changed.
Source refusals stop shared lowering before lossy rewrites, retaining
the affected source and useful diagnostics; they never emit a partial
project. Merely declaring CurrentAttributes does not trigger refusal.

Before either flag, the `wont_lower` question is worth asking: which
constructs in this app have no lowering for this target at all. The
MCP server's [`wont_lower` tool](mcp.md) answers it per target without
running the emit.

## What comes out

Every emitted project has the same shape at the top: the translated
app under the target's conventional source layout, the runtime
library beside it, `db/seed.sql` (schema plus the app's seeds, self-
contained), prebuilt static assets when the app ships them, a test
suite, and for the server targets an `e2e/` Playwright suite. The
`README.md` in each one is short and specific — prerequisites, build,
setup, run, test — and it is executed verbatim by the project's CI
against the blog fixture, so the commands in it are known to work.

What does NOT come out is sorbet-runtime. An app that annotates with
it transpiles to a tree that does not need it at run time: a `sig` is
read (see [`check.md`](check.md)) and then dropped, along with
`extend T::Sig`, `abstract!` and the rest of the annotations;
`T.let` / `T.must` / `T.cast` and their siblings become the value they
wrap; `T.type_alias` constants go with the signatures that were their
only reader. The two constructs that are class GENERATORS rather than
annotations are lowered into the plain Ruby they stand for — a
`T::Struct` into readers plus the keyword constructor it generates
(and `==`, where it included `ActsAsComparable`), a `T::Enum` into its
members plus `serialize` / `values` / `deserialize` /
`try_deserialize` / `from_serialized` / `has_serialized?`. `T.absurd`
is the one that keeps behavior rather than losing it: it RAISES where
it stood, message and value included, because dropping it would turn
"this cannot happen" into "this returns nil".

What remains is a `const` or `prop` on a base class from a gem: nothing
in the tree says that DSL is a typed prop rather than some other one,
and guessing from the method name would rewrite calls that were never
props.

The server targets all serve the app on `http://localhost:3000` (`PORT`
overrides), with Action Cable at `/cable`, against a SQLite file at
the Rails-traditional `storage/development.sqlite3`. Setup is one
line:

```sh
sqlite3 storage/development.sqlite3 < db/seed.sql
```

then the target's build and run commands from its README — for Rust,
`cargo build --release` and `./target/release/app`; for Go,
`go mod tidy && go build -o server . && ./server`; and so on.

Emitted code is meant to be read. Method names, file layout and the
order of things follow the Ruby they came from; the runtime library is
a few files of ordinary code in the target language, not a framework.
The emitted tests are the app's own model and controller tests,
translated the same way the app is.

## Regenerating

The output is not meant to be edited and kept. Change the Rails app,
run the command again. Each README ends with the exact `roundhouse
--target` line that produced the tree, so a project can be regenerated
by someone who has only the output. If you find yourself patching the
emitted code, that is a bug report: either a construct roundhouse
should lower and doesn't, or a translation it gets wrong — and the
[compare oracle](verifying.md) is how to show the second kind.

## Which target

For a service you intend to run, the answer depends on what you and
your team already operate; every server target passes the same
conformance gate against the blog fixture, and [`targets.md`](targets.md)
says what each is additionally tested against. For the closest
behavior to Rails itself — the widest slice of the framework and
the fewest [deliberate divergences](rails-coverage.md) — the Ruby
shape compiled by Spinel is ahead of every other target and will stay
ahead for the foreseeable future, because its runtime *is* the
framework runtime the other targets get a translation of.
[`spinel.md`](spinel.md) is that door.
