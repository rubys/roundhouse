# Ruby + ERB

The primary input to roundhouse is a Rails application directory. This
doc covers how source languages arrive at the IR: Ruby, which the
compiler reads directly, and the template engines — ERB is the worked
example here — that compile to Ruby first.

## The short version

```
   *.rb   ──────────────────►  Prism  ──► ingest::ingest_expr  ──►  Expr / App
                                             ▲
   *.erb  ──► compile_erb  ──► Ruby  ────────┤
               (src/erb.rs)                  │
   *.haml ──► compile_haml ──► Ruby  ────────┘
               (src/haml.rs)
```

Ruby is parsed by [Prism](https://github.com/ruby/prism) via the
`ruby-prism` crate. Templates are compiled into Ruby source first, then
fed through the same Prism + ingest path — there is no second parser.
The ingester is a single switch over Prism node kinds with one arm per
supported `ExprNode`.

## Ruby → IR

### Entry points (`src/ingest/`)

- `ingest_app(dir)` — walks a full Rails directory (`src/ingest/app.rs`).
  Calls the per-concern helpers below: the classic order first —
  schema → models → controllers → routes → views → tests → fixtures →
  seeds — plus a growing set of secondary inputs (`db/migrate/` as a
  fallback when `schema.rb` is absent, `config/application.rb` and
  initializers, `app/helpers/`, `config/routes/` split files,
  `config/importmap.rb`, `sig/**/*.rbs` sidecars, shared test
  helpers, …). `src/ingest/app.rs` is the authority on what the walk
  covers.
- `ingest_model`, `ingest_controller`, `ingest_view`,
  `ingest_routes`, `ingest_schema`, `ingest_test_file`,
  `ingest_fixture_file` — per-concern front doors, one module each
  under `src/ingest/` (see `src/ingest/mod.rs` for the full roster).
  Each returns an `IngestResult<T>`. Whole-program helpers like
  `ingest_ruby_program` (used for seeds and fixture preambles) live
  inside `src/ingest/expr.rs` rather than in a module of their own.
- `ingest_expr` (`src/ingest/expr.rs`) — the core recursive descent.
  One arm per supported `ExprNode` kind.

### Error discipline

Unsupported constructs return `IngestError::Unsupported { file,
message }` rather than silently dropping them. "Loud by design" — a
missing arm is a signal that either the IR needs a new variant or the
recognizer needs to widen. See [Adding a new IR
variant](../development/compiler-changes.md#adding-an-ir-variant) for the
development checklist.

That's the strict path, and it's the default. Survey mode —
`roundhouse-check --continue`, or `ROUNDHOUSE_INGEST_SURVEY=1` —
records each gap and substitutes or skips instead of aborting, so one
unsupported construct doesn't hide every gap behind it. See
`src/ingest/survey.rs` (`unwrap_or_record`).

A support file (`lib/`, a gem's source) whose class ingest fails is
left out and the walk goes on, so one gap does not abort the app. It is
reported, not dropped in silence: `App::ingest_drops` holds it, and
analysis reports `error[ingest_unsupported]` at the construct that
failed it (`ingest::error_site` notes the innermost statement as the
error passes up through the walk).

### Surface preservation

Expression-level round-trip (ingest → emit-ruby → ingest ≡ identity,
checked via `roundhouse-ast --round-trip` on Ruby input — it rejects
ERB) forces surface detail — which brace style an array used, whether
a call was parenthesized, whether a symbol array was written
`[:a, :b]` or `%i[a b]` — to live in the IR as a dedicated field
(e.g. `ArrayStyle`, `parenthesized: bool`, `BlockStyle`).

Whole-app source equivalence was retired as a goal: the Ruby emitter
consumes lowered IR only, and compile-equivalence via Spinel replaced
source-equivalence (see the header of `src/emit/ruby.rs`). The live
guards are `tests/real_blog.rs` (`ingests_without_errors`,
`type_analysis_coverage`) on the ingest side and
`tests/lowered_ruby_emit.rs` + `tests/spinel_toolchain.rs` on the
emit side.

Any time you're tempted to normalize at ingest, check first whether the
emit side can reconstruct the original surface. If not, preserve the
distinction in the IR rather than losing it.

### Comments

Comments attach to the item they precede: every `ControllerBodyItem`
variant carries `leading_comments: Vec<Comment>` plus a
`leading_blank_line` flag — see `Comment` and `ControllerBodyItem` in
`src/dialect.rs`.

### Load-time conditional definitions

A library class or module body may define methods under a load-time
`if` / `elsif` / `else` or `unless` / `else` (`if Rails.env.test?` around
two `def`s). Each branch may hold `def`s (`private def` too), plain
`attr_reader` / `attr_writer` / `attr_accessor`s and nested conditionals
of the same shape, and `class` / `module` declarations, each loaded from
its own file inside its branch (one nested in a class shares that
class's file and is an error). Every branch's methods are ordinary methods of the
class, so analysis types each one; `LibraryClass::conditional_definitions`
records which branch defines which. The Ruby and JRuby trees write the
conditional around them where the source had it, so the branch Ruby
selects at load defines them. Other targets would define every branch's
methods, and report each such statement as an error
(`conditional_definition`). A named visibility change (`private :mode`)
applies to such a method only when every branch defines it and the
conditional ends in `else`. A model body's conditional definitions, a
visibility marker inside a branch, and any other statement in one stay
refused, each named at the statement that keeps it refused, as is a
`class` / `module` declared in one inside `class << self`. A class
declared in a branch and also, unconditionally, in another file is an
error at the declaration: the merged class would load only with the
branch.

### Body statements only Ruby answers

A library class or module body may run code as it loads that defines
no method the walk can read. The Ruby and JRuby trees write the
following statements back where the source has them, among the class's methods (`ingest::body_statements`,
`expr::BODY_STATEMENT`):

- a loop over a literal that `class_eval`s a heredoc or
  `define_method`s per element;
- a call to one of Ruby's own `Module` methods (`freeze`,
  `self.define_method`);
- an `alias_method` of a method an app superclass defines, or one
  [an alias of an inherited method](#an-alias-of-an-inherited-method)
  resolves (the walk already copies an alias of one the body defines;
  an alias of a method only a dropped macro defines would raise
  NameError as the class loads, so it stays dropped);
- a write through a class-side writer the class or an app ancestor
  defines (`self.require_name = true`, `self.strict ||= :on`);
- such a write in a class that, itself or through an app ancestor,
  includes an app module with an `included` hook, which may define the
  writer at run time, when its superclasses are the app's own up to a
  core class.

Each statement must call nothing on the class but Ruby's own `Module`
methods, `delegate` (the Ruby family's runtime defines it) and class
methods the app defines. A module whose body is only such a call
(`module BooleanType; freeze; end`) is declared for it.

The class's own `class << self` keeps its scope there
(`expr::SINGLETON_BODY`): its constants live on the singleton class,
and the analyzer qualifies a read of one to `X.singleton_class::NAME`.
A statement in it runs inside a `class << self` when the class is a
module, or a class whose superclasses are the app's own up to a core
class. Spinel and the other targets keep the constants on the class.
They drop the statements and report each one by name, as they report
any other dropped class-body call.

### Class attributes in library classes and included hooks

ActiveSupport's `class_attribute` and its `cattr_*` / `mattr_*` macros
define methods from names computed at run time, so the emitted runtime
does not define them. A library class's own `class_attribute :live`, and
one an app module's `self.included(base)` hook declares on `base`
(`base.class_attribute :ssl_strict`, `base.cattr_accessor :pem_file`),
become plain defs on the declaring class or on each app class that
includes the module (`ingest::hook_class_attribute`). A class attribute
reads the class's own value once written, else its superclass's. A
class variable accessor reads and writes `@@name`. The declaration
becomes its seed. A hook's `if base.respond_to?(:class_attribute)`
whose branch holds only declarations is that branch, since natively it
is true.

The Ruby trees write an `include` whose hook calls those writers after
the defs it needs (`hooked_include_position`), including in a body
written in source order. A hook with a declaration this cannot read (a
computed name or flag, a block, an unknown option) keeps its text and
still fails at the include. So does a model's include of such a module.

A model's own `class_attribute`, and one a Concern's `included do`
declares, are the same defs on the model; an app class below it reads
the model's value until it writes its own. A Concern's `included` block
runs in the model for each of the Concern's own Concern includes first,
as `append_features` runs them, wherever the Concern lives (app, `lib/`
or a gem's source). A model body's call of its own class method
(`custom_strategy :title, with: proc { … }`, written there or spliced
from a Concern's block) runs as the class loads, after the Concern's
class methods it may call.

### An include written in another body

`Other.include(M)`, where `Other` and `M` are the app's own and Ruby
resolves both from the body the call is written in, is an include of
`Other` (`ingest::external_include`): Ruby gives `Other`'s existing
includers a module it includes later, so the ancestors are the same.
activemerchant's `module Billing` ends `Compatibility::Model.include
Rails::Model`, and a namespace module holding such a call is declared
for it. The same call inside a railtie's `initializer`,
`config.to_prepare` or `ActiveSupport.on_load` block makes `M` a
dependency of `Other`, which is what a Concern does with it, and the
block stays where it is.

### An alias of an inherited method

`alias to from` (and `alias_method :to, :from`) in a `lib/` or gem
source class, naming a method the class body has not defined above it,
is an alias of an ancestor's method: graphql-client's
`OperationDefinition < Definition` writes `alias operation_name
definition_name`. The support walk captures it as the `alias_method`
call it is, and once every file is walked looks the original up as
Ruby does, through its app superclasses and their includes
(`ingest::inherited_alias`). The class's own includes must be app
modules written above the alias that do not define the original; with
a `prepend`, the alias stays refused. The Ruby and JRuby trees write the call
back, in the class's own `class << self` for a class-side alias, so
`instance_method(:to).original_name` is the original's. Analysis,
Spinel and the other targets see a copy of the original under the
alias's name, unless the original calls `super`. A file with an alias
no app ancestor defines is refused at the alias, including one of a
core superclass's method (`class Errors < Hash; alias count size`),
which needs the core classes' signatures.

## ERB → Ruby → IR

### Why compile to Ruby?

ERB's control-flow tags (`<% if ... %>`, `<% each do %>`, `<% end %>`)
are Ruby fragments interleaved with template text. Rather than write a
second parser that understands both, `src/erb.rs::compile_erb` produces
an equivalent Ruby source program:

```erb
<h1><%= article.title %></h1>
<% if article.comments.any? %>
  <ul>...</ul>
<% end %>
```

compiles to roughly:

```ruby
_buf = ""
_buf = _buf + "<h1>"
_buf = _buf + (article.title).to_s
_buf = _buf + "</h1>\n"
if article.comments.any?
  _buf = _buf + "  <ul>...</ul>\n"
end
_buf
```

The compiled Ruby is handed to Prism like any other source file, and
the existing ingest pipeline takes it from there. `<% %>` control flow
becomes regular Ruby AST; views inherit every recognizer the
controller/model paths already have.

### Design choices worth knowing

- **`_buf = _buf + X`, not `_buf += X`.** The ingester already handles
  `LocalVariableWriteNode`; `LocalVariableOperatorWriteNode` would add
  a second path for no gain. Commit the simpler lowering.
- **Block-expression output tags** — `<%= form_with(x) do |f| %>…<% end %>`
  — use a compile-time block stack. The opener emits
  `_buf = _buf + (form_with(x) do |f|` (no closing paren); the matching
  `<% end %>` emits `end).to_s`. Ordinary `<% ... do %>` blocks push a
  `Pass` marker and close with a plain `end`. See the `BlockKind` enum
  in `src/erb.rs`.
- **Comment tags** (`<%# ... %>`) drop silently without flushing the
  pending-text buffer. That's what lets adjacent text chunks merge into
  a single string literal, which in turn lets round-trip succeed across
  comment-bearing ERB.
- **Erubi trim semantics.** Under Rails' default trim mode, `<%-`
  opens like a plain `<%`, a closing `-%>` on an output tag drops the
  trailing newline, and a code or comment tag alone on its line
  contributes nothing to the output — its indentation and its newline
  both vanish. See the trim rule in `src/erb.rs`.

### Span mapping

`compile_erb_mapped` returns the compiled Ruby plus a segment table
(`ErbSegment`) mapping compiled-Ruby byte ranges back to template byte
ranges; `translate_spans` re-anchors every span in the ingested IR to
template coordinates (both in `src/erb.rs`, applied by
`ingest_template` in `src/ingest/view.rs`). Diagnostics against a view
therefore point at the template, not at the synthesized `_buf`
program.

### Debugging a template

Byte-for-byte source round-trip of a template is not a checked
property (see "Surface preservation" above — the Ruby emitter is
lowered-IR-only). When a template ingests wrong, use
`roundhouse-ast --stage compile-erb path.erb` to see the compiler's
Ruby output, then `--stage ingest` (or `--stages`) to see where the
divergence lands in the IR.

## The template-engine seam

ERB is the worked example, not the only engine. `src/ingest/view.rs`
owns the shared seam: `ViewEngine` maps a file extension (`Erb`,
`Haml`) to its compile function, and `ingest_template` runs the
common body — compile to Ruby, ingest, translate spans back to
template coordinates. Adding an engine = one `ViewEngine` arm + its
`compile_*_mapped` fn. HAML rides this today via
`src/haml.rs::compile_haml_mapped` (the disciplined subset Mastodon
actually uses, producing the same `_buf` output shape).

Jbuilder deliberately bypasses the seam: a `.json.jbuilder` source is
already Ruby, so `src/ingest/app.rs` dispatches it separately to
`src/ingest/jbuilder.rs` with no compile step.

### Markdown templates

`*.md.erb` templates use the ERB handler and the `md` format.
The lowerer names their methods `<action>_md`, separate from their HTML siblings.
An explicit `render formats: :md` or a request format of `md` selects the Markdown template.
The response uses `text/markdown; charset=utf-8`.

A YAML header remains ordinary template text. The compiler does not parse or remove it.
ERB expressions in the header and body use normal HTML escaping.
Expressions marked `html_safe` and literal template text remain unchanged.

The Markdown regression in [`tests/emit_and_run.rs`](../../tests/emit_and_run.rs)
checks both render paths and the HTML sibling against emitted Ruby.

## Key files

| File | What it does |
|------|--------------|
| `src/ingest/` | Prism → IR; one module per concern (`app`, `expr`, `model`, `controller`, `view`, `routes`, …) — see `src/ingest/mod.rs` for the full list |
| `src/ingest/view.rs` | The template-engine seam (`ViewEngine`, `ingest_template`) |
| `src/ingest/survey.rs` | Survey mode — record gaps instead of aborting |
| `src/ingest/roda_app.rs` | Alternate front door for Roda apps (with `src/ingest/sequel_model.rs` + `src/ingest/sequel_migration.rs`); whole-app dispatch in `src/ingest/app.rs` |
| `src/erb.rs` | ERB → Ruby source, plus the span segment table |
| `src/haml.rs` | HAML → Ruby source (Mastodon subset) |
| `src/expr.rs` | `Expr`, `ExprNode`, surface-preservation fields |
| `src/dialect.rs` | Rails-level structures (`Model`, `Controller`, …) |
| `src/emit/ruby.rs` | Lowered-IR → spinel-shape Ruby (the emit-side partner) |

## Related docs

- [`schema-routes-seeds.md`](schema-routes-seeds.md) — how particular
  Ruby files under `db/` and `config/` feed non-code structures.
- [`../pipeline/analyze.md`](../pipeline/analyze.md) — what happens
  once the IR is built.
- [`../pipeline/verification.md`](../pipeline/verification.md) —
  how the pipeline's output is verified.
