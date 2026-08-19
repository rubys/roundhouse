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
variant](../../DEVELOPMENT.md#adding-a-new-ir-variant) for the
six-step pattern.

That's the strict path, and it's the default. Survey mode —
`roundhouse-check --continue`, or `ROUNDHOUSE_INGEST_SURVEY=1` —
records each gap and substitutes or skips instead of aborting, so one
unsupported construct doesn't hide every gap behind it. See
`src/ingest/survey.rs` (`unwrap_or_record`).

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
