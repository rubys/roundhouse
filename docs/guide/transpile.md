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

## Project extensions

An app can describe gems and class-body DSLs that Roundhouse does not
know about. The descriptions live in the app's own tree, so the compiler
does not have to carry them. List extension directories in
`roundhouse.yml`:

```yaml
extensions:
  - config/roundhouse
```

Each entry names a literal directory relative to the app root, as with
`test_paths`. It must not contain `..`, an absolute path, or a symbolic
link in the directory path. Directories load in the listed order.
The same directory must not appear twice.

The `gems.yml` and `macros.yml` files are optional. Each file can be a
symbolic link, including a link to a file outside the app root.
Roundhouse reports an error if it cannot read a linked file.

Example files:

```yaml
# gems.yml
lock_gated_requires:
  - {require: "widgets/all", gem: "widgets"}
rails_dependent:
  - {gem: "widget-engine", why: "requiring it loads Rails framework files"}
runtime_gems:
  - {constant: "Widget", gem: "widgets"}
  - {any_text: ["Widget::Cache."], gem: "widget-cache"}
```

```yaml
# macros.yml
attribute_macros:
  - {name: price_column, positional: true}
  - {name: bag_columns, hash_key: attributes}
```

- `lock_gated_requires`: when the app's `Gemfile.lock` resolves the gem,
  the `ruby` and `jruby` trees require the path at boot. Registry gems
  use the locked version. Git gems keep the locked URL and revision.
  PATH gems remain source inputs; this setting does not load them natively.
- `rails_dependent`: requiring the gem loads Rails, so compiled projects
  do not load it natively. References to its constants stay errors,
  including constants made known by an RBI. The error includes the gem
  name and the `why` text. App-owned constants keep their own definitions.
- `runtime_gems`: the `ruby` and `jruby` trees declare the gem in their
  Gemfile when a file under `app/` names the constant or contains one of
  the texts. Git gems keep their source and revision. This setting does
  not require the gem; use an application require or `lock_gated_requires`.
- `attribute_macros`: the attributes a macro names read and write as
  untyped, either from its leading symbol arguments (`positional`) or
  from the array under a keyword (`hash_key`). No target lowers the
  macro itself, so each call is reported as an error at its declaration.
  This replaces a dispatch error at every place the value object is used.

Extension gem facts also select the locked gems' Tapioca RBIs under
`sorbet/rbi/gems/`. This includes gems classified as infrastructure or
standard-library gems. An RBI supplies types and ownership, not executable
code. A gem in `rails_dependent` stays refused even when its RBI declares
the referenced class, module, or value. Other targets refuse constants
that depend on a native extension gem instead of emitting missing classes.

Roundhouse records extension boot loads on each app. A lockfile dependency
alone does not establish that a gem loads. A `runtime_gems` row does not
become a boot require.

For Git-backed native gems, run the emitted project's `bundle install`.
At boot, Roundhouse activates the locked checkouts under
`<gem home>/bundler/gems/`, including Git-backed dependencies, before the
guarded requires. It reports an error if source metadata or a checkout
is missing, or if a published copy is already active. It never replaces
a locked Git source with a published gem of the same name.
The Gemfile also preserves Git `glob` and `submodules` settings.
The runtime uses the locked glob to find each gemspec.

A guarded require stays quiet when the optional feature itself is absent.
If a present feature fails to load an internal dependency, the runtime
reports both the requested feature and the load error on standard error.

Configured attribute macros also apply in a concern's `included do`
block. Roundhouse retains direct statements from `ActiveSupport::Concern`
blocks and inserts them into each including model before analysis.
Each model gets the macro's attribute types and its declaration error.
A concern that no model includes does not change attribute types or
produce macro declaration errors. This does not add general class-body
execution or support for `class_attribute` on models.

Extension rows are added after Roundhouse's own rows and never replace
them. Unknown keys, malformed rows, a missing directory, and a
`rails_dependent` gem that another fact loads or that Roundhouse models
all stop ingestion with an error that names the file. The `types` and
`overlay` entries are reserved and rejected until they are implemented.

The CLI and browser use the same built-in gem tables to validate
extensions.

Extension facts belong to the app that lists them. An app that lists no
extensions behaves as before.

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

Boot code gets its own ledger. A statement in `config/initializers/`, in
an engine class body or in `config/application.rb` that no ingest
recognizer reads is dropped from the compiled app, so each one is
reported as `warning[unconsumed_initializer]` with its file and line
(never an error: the ledger is a warning, and it is not counted as an
ingest gap). A normal run prints one count line; `--survey` and
`--allow-unsupported` list every statement, and `--survey` ends with an
"Initializer ledger" section grouped by statement. Statements that
provably do nothing in a compiled app (`config.filter_parameters += […]`,
`config.action_dispatch.cookies_serializer = …`) are not reported; the
list and its reasons live in `src/ingest/initializer_ledger.rs`.

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
