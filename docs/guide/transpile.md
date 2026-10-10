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

## Test folders

Roundhouse reads every `.rb` file recursively from these folders by default:

- `test/models`
- `test/controllers`
- `test/helpers`
- `test/channels`
- `test/lib`

The app root can contain a `roundhouse.yml` file that adds folders:

```yaml
test_paths:
  - test/unit
  - quality/specs
```

`test_paths` adds folders. It does not replace the default folders. Each
value must name a literal directory relative to the app root. Roundhouse
does not expand globs, environment variables, or `~`. A configuration
error stops ingestion.
Roundhouse rejects a configured path with a symbolic link. It does not
follow symbolic links below selected folders.

Roundhouse reads `test/test_helper.rb`, `test/test_helpers/`, and
`test/fixtures/` outside this folder list.

Transpilation is the analysis from [`check.md`](check.md) followed by
lowering and emit. It requires zero analysis errors and a runtime for
the modeled constructs on the selected target. A clean target-independent
`check` does not imply availability on every target (see Date below).
On a covered app the command prints nothing and exits 0. Otherwise it
stops, and the two flags decide how.

### Date-only values

`t.date` is modeled as `Date` (nullable as `Date?`), not `Time`.
The `ruby` target hydrates a native Date from YYYY-MM-DD storage text,
normalizes Date or ISO date text writes, and preserves SQL NULL as nil.
`Date#>>` uses Ruby's calendar arithmetic, including end-of-month clamping;
Date JSON is an ISO date without a clock or zone. DateTime/time columns
retain their timestamp type and runtime.

Date-only emission is currently supported and executed on `ruby` and
`spinel` (Spinel with its own bounded `Date`; see
[the runtime notes](../pipeline/runtime.md)).
`jruby`, `roda` and the non-Ruby targets (including
`typescript-worker`) reject Date at the project boundary before emitting
files, even with `--allow-unsupported`. This is an observable support
boundary, replacing silent timestamp treatment, not evidence that these
languages cannot represent dates. JRuby's date-only adapter path remains
unverified. The `blog` source archive is not a transpilation target.

Of ActiveSupport's Date extensions, `Date.current` (today in the app's
zone, under `travel` too), `beginning_of_month` / `end_of_month` (Date)
and `beginning_of_day` / `end_of_day` / `midnight` (a zone-local Time),
with their `at_*` aliases, are lowered to runtime functions. This is a
bounded Date surface, not all of the extensions: unmodeled methods still
diagnose, and nonliteral strict-local defaults
remain an existing ingestion gap.

### Forwarding a computed Proc

Ruby emission preserves `items.map(&@callback)` and
`items.map(&factory())`, including evaluating `factory()` once before
the call. Arbitrary `&expr` forwarding is not implemented for `rust`,
`crystal`, `go`, `python`, `kotlin`, `swift` or `elixir`: these targets
reject it at the project boundary, even with `--allow-unsupported`,
rather than dropping the callback or evaluating its producer per item.
Existing literal blocks, local block variables and bound-method
references retain their target-specific support and limitations.

Roundhouse currently refuses generated enum `*_before_type_cast` readers.
Rails returns the original input after an unsaved assignment and stored data
after persistence; the compiler has only the stored enum slot. Negative enum
scopes are supported. Source-defined readers keep their ordinary behavior.

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
`T.let` / `T.cast` and their siblings become the value they
wrap, and `T.must` becomes the nil check sorbet-runtime makes;
`T.type_alias` constants go with the signatures that were their only
reader. The two constructs that are class GENERATORS rather than
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
The emitted tests come from the default test folders and any additional
folders in `roundhouse.yml`.

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
