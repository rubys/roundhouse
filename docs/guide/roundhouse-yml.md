# `roundhouse.yml` — app configuration

Most apps need no configuration. Roundhouse reads the app as it is, and
an app without a `roundhouse.yml` gets the default behavior.

Add the file at the app root, next to `Gemfile`, when you need to tell
Roundhouse something it cannot learn from the source. The file has two
keys:

```yaml
test_paths:
  - test/unit
extensions:
  - config/roundhouse
```

- [`test_paths`](#test_paths) adds test folders to the default list.
- [`extensions`](#extensions) lists directories that describe the app's
  gems and class-body DSLs.
- [`spinel`](#spinel) passes a compiler mode the Spinel target needs
  (provisional).

Roundhouse reads the file during ingest, so every tool sees it:
[`roundhouse check`](check.md), the [editor](editor.md), the
[MCP server](mcp.md), the [in-browser IDE](ide.md) (from the folder you
open), and [`roundhouse --target`](transpile.md).

## Errors

A mistake in `roundhouse.yml` or in an extension file stops ingestion.
Roundhouse does not ignore the entry or guess what it meant. Unknown keys
are errors too, so a typo fails loudly:

```
$ roundhouse check .
roundhouse-check: ingest failed: parse error in ./roundhouse.yml: unknown field `extension`, expected `test_paths` or `extensions`
```

`check` exits 2 on these errors. `--continue` does not get past them,
even though the hint line suggests it.

## `test_paths`

Roundhouse reads every `.rb` file recursively from these folders by
default:

- `test/models`
- `test/controllers`
- `test/helpers`
- `test/channels`
- `test/lib`

`test_paths` adds folders:

```yaml
test_paths:
  - test/unit
  - quality/specs
```

`test_paths` adds folders. It does not replace the default folders. Each
value must name a literal directory relative to the app root. Roundhouse
does not expand globs, environment variables, or `~`.
Roundhouse rejects a configured path with a symbolic link. It does not
follow symbolic links below selected folders.

Roundhouse reads `test/test_helper.rb`, `test/test_helpers/`, and
`test/fixtures/` outside this folder list.

Transpiled projects include tests from the default folders and these
folders.

A file in these folders that defines a module and no test class is a
test helper (`test_paths: [test/support]` with `module ArgsHelper`). A
test class that includes the module gets it in its own emitted file,
with the module's assertions lowered like the test's own, and runs the
module's `test_*` methods as its tests, as Minitest does.

A helper written as an `ActiveSupport::Concern` (here or in
`test/test_helpers/`) has its `included do` block read as the
including class's own statements, where the `include` stands: its
`setup` and `teardown` callbacks (blocks or `:method_name`s), tests,
methods and includes. One that `test/test_helper.rb` includes into
`ActiveSupport::TestCase` runs ahead of every test class's own
callbacks, in Rails' order. Any other statement in the block is
refused by name.

### RSpec specs

A file whose name ends in `_spec.rb` in one of these folders is read as
an RSpec spec, so an app with an RSpec suite can check that its build,
the Spinel one included, still passes its specs:

```yaml
test_paths:
  - spec/models
```

Every example group that owns examples becomes one test class, named by
the spec file and the group: `spec/models/article_spec.rb`'s
`context "without title"` is `ModelsArticleWithoutTitleSpec`. The class
carries what RSpec gives its examples: the `let`s of every enclosing
group (an inner one overrides an outer one), the `before` hooks from the
outermost group in, and the group's helper methods. Each example is one
test, and the emitted runner reports `<Class>: <N> tests passed`.

The supported subset:

- `RSpec.describe`, `describe`, `context`, with a constant, a string,
  or both, and `type: :model`, `:lib` or `:request` metadata;
- `let`, `let!`, `subject`, `subject(:name)`, `described_class`, and the
  implicit `subject` (`described_class.new`);
- `before` and `after`, bare or `(:each)` / `(:example)`; `after` hooks
  run inner group first, after the example whether it passed or not;
- `it`, `specify`, `example`; an example without a block, and `xit`,
  are pending in RSpec and are not tests here;
- `expect(x)` and `is_expected` with `to`, `not_to` and `to_not`, and the
  matchers `eq`, `eql`, `equal`, `be(x)`, `be_nil`, `be_truthy`,
  `be_falsey`, `be_a`, `be_an`, `be_kind_of`, `be_a_kind_of`,
  `be_instance_of`, `be_an_instance_of`, `be_<predicate>`,
  `have_<predicate>`, and `include` of plain values;
- `expect { }.to raise_error` with no argument (any `Exception`), a
  class, a literal message (equal to the error's message, or matched by a
  Regexp), or both; `expect { }.not_to raise_error`;
- `expect { }.to change { value }` and `change(receiver, :message)`, bare
  or with `by`, `by_at_least`, `by_at_most`, `from` and `to`; a collection
  or String value is copied before the block runs, as RSpec does. RSpec
  also compares the value's `hash` before and after; roundhouse does not,
  so another object changed in place with an unchanged `==` is not seen
  as a change.

Anything else is refused by name: `roundhouse check` reports it, and
`--continue` lists every refused construct and leaves out the examples
that need it. Roundhouse does not read `spec/rails_helper.rb` or
`spec/spec_helper.rb` yet, so configuration there (included modules,
global hooks) does not reach the examples, and neither do test doubles,
shared examples or `around` hooks.

## `extensions`

Roundhouse models Rails and a set of common gems. Project extensions
describe other gems and class-body DSLs that Roundhouse does not know
about. The descriptions live in the app's own tree, so the compiler does
not have to carry them.

### When you need one

**A gem's value object fails at every use.** A gem adds a class-body
macro that turns columns into value objects:

```ruby
class Invoice < ApplicationRecord
  price_column :amount, currency: :usd

  def label = amount.currency
end
```

Roundhouse types `amount` from the schema, so each use of the value
object is an error:

```
app/models/invoice.rb:4:15: error[send_dispatch_failed]: no known method `currency` on Float?
```

An [`attribute_macros`](#macrosyml) entry makes the attribute untyped
and reports one error at the macro instead:

```
app/models/invoice.rb:2:3: error[unsupported]: price_column not supported (all targets): attribute macro on `Invoice` from a project extension: its attributes read untyped, and no target lowers the call
```

The error remains because no target translates the macro yet, but it
now names the cause once, rather than one error for each use.

**The `ruby` tree needs a gem it does not load.** The `ruby` and
`jruby` targets carry many of the app's gems, but not every gem is
required at boot or declared in the emitted Gemfile.
[`gems.yml`](#gemsyml) tells those targets how to load or declare a gem,
or records that the gem cannot load without Rails. Its facts can also
select a locked gem's RBI for type information. An RBI supplies types,
not executable code. Without a selected RBI, a constant from a gem
Roundhouse does not model can still produce an error:

```
app/services/report.rb:3:5: error[unsupported]: constant not supported (all targets): WidgetEngine::Thing — unmodeled gem `widget_engine`; emitted constant availability is unverified
```

### Directories

List extension directories in `roundhouse.yml`:

```yaml
extensions:
  - config/roundhouse
```

Each entry names a literal directory relative to the app root, as with
`test_paths`. It must not contain `..`, an absolute path, or a symbolic
link in the directory path. Directories load in the listed order.
The same directory must not appear twice.

A directory can hold `gems.yml` and `macros.yml`. Both files are
optional. Each file can be a symbolic link, including a link to a file
outside the app root, so several apps can share one description of a
gem. Roundhouse reports an error if it cannot read a linked file.

### `gems.yml`

```yaml
lock_gated_requires:
  - {require: "widgets/all", gem: "widgets"}
rails_dependent:
  - {gem: "widget-engine", why: "requiring it loads Rails framework files"}
runtime_gems:
  - {constant: "Widget", gem: "widgets"}
  - {any_text: ["Widget::Cache."], gem: "widget-cache"}
source_gems:
  - {gem: "pundit"}
```

- `lock_gated_requires`: when the app's `Gemfile.lock` resolves the gem,
  the `ruby` and `jruby` trees require the path at boot. Registry gems
  use the locked version. Git gems keep the locked URL and revision.
  PATH gems remain source inputs; this setting does not load them natively.
- `rails_dependent`: requiring the gem loads Rails, so compiled projects
  do not load it natively. References to its constants stay errors,
  including constants made known by an RBI. The error includes the gem
  name and the `why` text. App-owned constants keep their own definitions.
  The Ruby-core exemption applies to exact constants such as `String`,
  not to all constants inside their namespaces.
- `runtime_gems`: the `ruby` and `jruby` trees declare the gem in their
  Gemfile when a file under `app/` names the constant or contains one of
  the texts. Git gems keep their source and revision. This setting does
  not require the gem; use an application require or `lock_gated_requires`.
- `source_gems`: the gem's own Ruby is compiled with the app, as a
  `PATH` gem's is. Roundhouse reads the version
  `Gemfile.lock` resolves from where Bundler installed it (`BUNDLE_PATH`
  in `.bundle/config`, then `GEM_HOME` and `GEM_PATH`), and takes only the
  files `require "<gem>"` loads from its `lib/`: literal `require`,
  `require_relative` and `autoload` calls, followed from `lib/<gem>.rb`.
  An autoloaded file is compiled whether or not the app names its
  constant. Generators and
  test helpers the entry file does not load stay out. What the code
  needs that Roundhouse does not support is reported as for the app's
  own code. The census lists the gem as `source`. Run `bundle install`
  before `roundhouse check`; a gem that is not installed, not in the
  lockfile, or also named by another fact is an error.

A `PATH` or source gem's files also run as Ruby loads them on the `ruby`
and `jruby` trees. Each file's top level, its statements and its
`require`s in source order, is written as a program under
`app/models/roundhouse_files/`. `Bundler.require` loads the gem's entry
file at boot, before the app's own files, and that file's requires load
the rest. A literal-list loop of `require_relative` and a
`Dir[File.join(__dir__, "x/**/*.rb")]` glob become one require per file.
An `autoload :Name, "path"` (or `ActiveSupport::Autoload`'s path-less
`autoload :Name`) loads that file's program when the constant is first
read. Once the boot loads a gem, its classes and modules load only where
its own requires and autoloads reach them, as natively: a file nothing
requires defines nothing. The boot loads a gem through `Bundler.require`,
a literal `require` in `config/boot.rb`, `config/application.rb` or
`config/environment.rb`, or an app file's `require` of it.
A file-level `if` / `unless` that declares a class or module runs its
branches as Ruby does, so the class exists only where its branch ran.
A `require` of a Rails framework feature is left to the runtime, as are
`ActiveSupport.on_load` (see `on_load` reopens) and `I18n.load_path`
(the gem's locale files are compiled in). A
statement that cannot be carried is an error (`file_statement_unsupported`),
as is a `require_relative` of a file the tree does not carry
(`file_require_not_carried`). The `spinel` tree compiles the gem's
declarations only.

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

### `macros.yml`

```yaml
attribute_macros:
  - {name: price_column, positional: true}     # price_column :a, :b, opt: 1
  - {name: bag_columns, hash_key: attributes}  # bag_columns attributes: [:a, :b]
```

`attribute_macros`: the attributes a macro names read and write as
untyped. With `positional`, the names are the macro's leading symbol
arguments. With `hash_key`, they are the array under that keyword. No
target lowers the macro itself, so each call is reported as an error at
its declaration. This replaces a dispatch error at every place the value
object is used.

Configured attribute macros also apply in a concern's `included do`
block. Roundhouse retains direct statements from `ActiveSupport::Concern`
blocks and inserts them into each including model before analysis.
Each model gets the macro's attribute types and its declaration error.
A concern that no model includes does not change attribute types or
produce macro declaration errors. This does not add general class-body
execution or support for `class_attribute` on models.

### Rules

Extension rows are added after Roundhouse's own rows and never replace
them. Unknown keys, malformed rows, a missing directory, and a
`rails_dependent` gem that another fact loads or that Roundhouse models
all stop ingestion with an error that names the file. The `types` and
`overlay` entries are reserved and rejected until they are implemented.

The CLI and browser use the same built-in gem tables to validate
extensions.

Extension facts belong to the app that lists them. An app that lists no
extensions behaves as before.

## `spinel`

```yaml
spinel:
  int_overflow: promote
```

`int_overflow` is handed to Spinel as `--int-overflow` on every compile
the emitted Makefile runs: the app binary and each test binary. Ruby's
Integer never overflows; Spinel's default (`raise`) stops with a
`RangeError` when a value passes 64 bits, `promote` turns it into a
big integer as Ruby does, and `wrap` wraps it. An app needs `promote`
when it, or a gem compiled from source, computes past 64 bits: rotp
builds its Base32 secret in one shifted integer, so lobsters' two-factor
sign-in needs it. `promote` with the emitted RBS needs Spinel with
matz/spinel#10494.

This setting is provisional. Whether a program needs the mode should be
decided without asking the app, and the setting will go once it is.
