# Ruby + ERB

The primary input to roundhouse is a Rails application directory. This
doc covers the two source languages the compiler reads directly — Ruby
and ERB — and how they arrive at the IR.

## The short version

```
   *.rb  ──────────────►  Prism  ──► ingest::ingest_expr  ──►  Expr / App
                                         ▲
   *.erb ──► compile_erb ──► Ruby  ──────┘
              (src/erb.rs)
```

Ruby is parsed by [Prism](https://github.com/ruby/prism) via the
`ruby-prism` crate. ERB is compiled into Ruby source first, then fed
through the same Prism + ingest path — there is no second parser. The
ingester is a single switch over Prism node kinds with one arm per
supported `ExprNode`.

## Ruby → IR

### Entry points (`src/ingest.rs`)

- `ingest_app(dir)` — walks a full Rails directory. Calls the
  per-concern helpers below in a fixed order:
  `db/schema.rb` → `app/models/*` → `app/controllers/*` →
  `config/routes.rb` → `app/views/**` → `test/models/*`,
  `test/controllers/*` → `test/fixtures/*.yml` → `db/seeds.rb`.
- `ingest_model`, `ingest_controller`, `ingest_view`,
  `ingest_routes`, `ingest_schema`, `ingest_test_file`,
  `ingest_fixture_file`, `ingest_ruby_program` — per-concern
  front doors. Each returns an `IngestResult<T>`.
- `ingest_expr` — the core recursive descent. One arm per supported
  `ExprNode` kind.

### Error discipline

Unsupported constructs return `IngestError::Unsupported { file,
message }` rather than silently dropping them. "Loud by design" — a
missing arm is a signal that either the IR needs a new variant or the
recognizer needs to widen. See [Adding a new IR
variant](../../DEVELOPMENT.md#adding-a-new-ir-variant) for the
six-step pattern.

### Surface preservation

The round-trip identity (ingest → emit-ruby → ingest ≡ identity) is a
non-negotiable gate. That forces surface detail — which brace style
an array used, whether a call was parenthesized, whether a symbol
array was written `[:a, :b]` or `%i[a b]` — to live in the IR as a
dedicated field (e.g. `ArrayStyle`, `parenthesized: bool`, `BlockStyle`).

Any time you're tempted to normalize at ingest, check first whether the
emit side can reconstruct the original surface. If not, preserve the
distinction in the IR rather than losing it.

### Comments

Comments currently attach in a limited way — see `Comment` in
`src/dialect.rs` and the `ControllerBodyItem::Comment` variant.
Round-trip is preserved only for files listed in
`tests/real_blog.rs::EXPECTED_RUBY_FILES`; a comment-preservation plan
lives in the auto-memory (see `DEVELOPMENT.md`).

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

### Failure modes

If an ERB template round-trips structurally but not byte-for-byte, the
usual culprits are:

- Multi-line argument formatting (the Ruby emitter picks one canonical
  layout). Known gap — see `tests/real_blog.rs` exclusion notes.
- Whitespace inside `<% ... %>` tags (the compiler preserves
  surrounding text, but tag-interior whitespace isn't retained today).

Use `roundhouse-ast --stage compile-erb path.erb` to see the
compiler's Ruby output, then `--stage ingest` (or `--stages`) to see
where the divergence lands in the IR.

## Key files

| File | What it does |
|------|--------------|
| `src/ingest.rs` | Prism → IR for every supported construct |
| `src/erb.rs` | ERB → Ruby source |
| `src/expr.rs` | `Expr`, `ExprNode`, surface-preservation fields |
| `src/dialect.rs` | Rails-level structures (`Model`, `Controller`, …) |
| `src/emit/ruby.rs` | The round-trip identity partner |

## Related docs

- [`schema-routes-seeds.md`](schema-routes-seeds.md) — how particular
  Ruby files under `db/` and `config/` feed non-code structures.
- [`../pipeline/analyze.md`](../pipeline/analyze.md) — what happens
  once the IR is built.
- [`../pipeline/verification.md`](../pipeline/verification.md) —
  round-trip identity and source equivalence in detail.
