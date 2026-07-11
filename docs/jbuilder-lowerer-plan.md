> **COMPLETED — historical design record.** The work described in this plan has shipped; see [README](../README.md) for the current state of these targets. Retained for the design rationale that source comments still reference — this is not a live roadmap.

# Jbuilder lowerer plan

Add a Jbuilder/JSON-IR lowerer that compiles `*.json.jbuilder` templates
to canonical lowered IR consumed by all per-target library emitters.
Closes the JSON-rendering path for every active target with one slice
rather than N hand-written per-target Jbuilder DSL implementations.

## Strategic context

- railsbench (`~/git/ruby-bench/benchmarks/railsbench`) is the
  benchmark we want to publish numbers against. It ships
  `*.json.jbuilder` templates for `index` / `show` / `_post`. Without a
  Jbuilder path, every JSON-rendering benchmark row is blocked.
- "API-server" is one of the natural framings for the published
  numbers — Rails-as-JSON-API is more interesting on req/sec/GB than
  Rails-as-HTML because the GVL contention story compounds with the
  serializer cost.
- Mirrors the architectural pattern proved by `src/lower/view_to_library/`
  (ERB → `LibraryClass` body). Same shape: one ingest module, one
  lowerer module producing canonical method bodies in `runtime/ruby/`
  shape, zero per-target emitter changes for the common case.
- Highest-leverage single parallel slice currently available. Smaller
  than the Rust migration (16-26 days; already underway in
  `docs/rust-migration-plan.md`), larger than rack-adapter glue
  (~½ day) or railsbench HTML gaps (~few days).

## Decisions locked in (provisional — confirm at Phase 0)

- **Lower to canonical method bodies, not to a runtime DSL call.** A
  `.json.jbuilder` template lowers to a method that builds a string via
  the same `io = String.new; io << ...; io` shape ERB views already use,
  with helper rewrites to `JsonBuilder.<x>` for the few primitives that
  need runtime support. This keeps the per-target emit surface the same
  as for HTML views — `library.rs` consumes a `LibraryClass`, end.
- **No per-target Jbuilder runtime crate.** The lowerer emits straight
  to `String.new` + interpolated JSON, with three small helpers in
  `runtime/ruby/json_builder.rb` (canonical escaping, array join, hash
  pair join). Targets inherit the helper via the same transpile path
  that already covers `view_helpers.rb`.
- **Stay inside `String.new` / `io <<` shape.** Don't introduce a
  separate `JSON.generate`-on-Hash path: that pushes a Hash literal
  through the whole pipeline and trips Group 2 emitters (Rust/Go) on
  `Ty::Untyped` Hash erasure. Concatenated literals stay typed.
- **Format-aware controller dispatch is out of scope** for this plan —
  see `docs/rust-migration-plan.md` for Phase 2.5 Parameters
  specialization, and assume `respond_to do |format| format.json {...} end`
  lowering lands separately. This plan delivers the Jbuilder→library
  path and the helper runtime; the controller side is a small follow-on.

## Phase status

| # | Phase | Days | Status |
|---|---|---|---|
| 0 | Audit + fixture lock-in | ½ | not started |
| 1 | Ingest `.json.jbuilder` (new `src/ingest/jbuilder.rs`) | ½-1 | not started |
| 2 | `json_builder.rb` runtime helper + transpile slot | ½ | not started |
| 3 | Lowerer: `json_to_library` covering railsbench DSL surface | 1½-2 | not started |
| 4 | Per-target verification: Ruby + Spinel (smallest cycle) | ½-1 | blocked on 3 |
| 5 | Per-target verification: Crystal | ½-1 | blocked on 4 |
| 6 | Per-target verification: TypeScript | ½-1 | blocked on 4 |
| 7 | Per-target verification: Rust (lands on `rust2` post-migration) | ½-1 | blocked on Rust Phase 4 |
| 8 | Controller `format.json { render :index }` plumbing | ½-1 | parallel-able with 4-7 |

Total: **~5-8 working days** for full five-target consumption, or
**~3-4 working days** if trimmed to Ruby + Spinel only (Phases 0-4 + 8).

## Reference fixtures (lock at Phase 0)

v1 validates against `fixtures/real-blog/app/views/articles/`, which
already ships the same three-template surface (articles instead of
posts; no `:published` column otherwise byte-equivalent to railsbench):

```ruby
# _article.json.jbuilder
json.extract! article, :id, :title, :body, :created_at, :updated_at
json.url article_url(article, format: :json)
```

```ruby
# index.json.jbuilder
json.array! @articles, partial: "articles/article", as: :article
```

```ruby
# show.json.jbuilder
json.partial! "articles/article", article: @article
```

The railsbench triple at
`~/git/ruby-bench/benchmarks/railsbench/app/views/posts/*.json.jbuilder`
exercises the identical four primitives and is a follow-on benchmark
target (no additional lowerer work expected — same DSL).

These templates exercise the four primitives the lowerer must
handle:

1. `json.extract! obj, :a, :b, :c` → emit a JSON object of the named
   attributes, value from `obj.<attr>`.
2. `json.<key> <expr>` → emit a single `"<key>": <json-encoded expr>`
   pair. (Catches `json.url ...`.)
3. `json.array! @collection, partial: <name>, as: :var` → emit a JSON
   array, calling the partial per element.
4. `json.partial! <name>, <var>: <expr>` → inline the partial's body
   with `<var>` bound to `<expr>`.

Three-template fixture set is the minimum-shippable surface. Stretch
coverage (deferred unless a target benchmark demands it):

5. `json.<key> do ... end` → nested object.
6. `json.<key>(@collection) do |item| ... end` → nested array with
   per-element block.
7. `json.merge!(hash)` / `json.set!(key, value)` — programmatic forms.
8. `json.key_format!` / `json.deep_format_keys!` — global formatting.
9. `json.cache!`, `json.cache_if!` — caching directives (no-op for AOT).

## Phase details

### Phase 0 — Audit + fixture lock-in (½ day)

- Catalog all `.json.jbuilder` templates we want to support at v1.
  railsbench's three are the must-cover set; the four "stretch" forms
  above stay out unless a chosen benchmark needs them.
- Read `src/ingest/view.rs` (38 LOC) — ERB ingest pattern to mirror.
- Read `src/lower/view_to_library/mod.rs` head + `walker.rs` — the
  shape the new lowerer mirrors (io-accumulator body, helper-call
  rewrites, framework-stub registry merge).
- Read `src/lower/view_to_library/partial.rs` (145 LOC) — partial
  dispatch pattern; Jbuilder's `partial!` reuses the same plumbing.
- Decide: does the new lowerer live in
  `src/lower/jbuilder_to_library/` (parallel to `view_to_library/`)
  or as a sub-module under `view_to_library/`? **Recommend separate
  module** — different syntactic DSL even if some helpers reuse.

### Phase 1 — Ingest (½-1 day)

`src/ingest/jbuilder.rs` (new, ~80-120 LOC):

- Source is straight Ruby (no template wrapper to compile, unlike
  ERB). Just `ingest_ruby_program(source, file)`.
- Path-extension shape: `posts/_post.json.jbuilder` →
  name=`posts/_post`, format=`json`. Mirror `src/ingest/view.rs`'s
  rsplit logic but strip `.jbuilder` first, then `.json`.
- Yields a `View` (reuse the dialect type — same shape: name, format,
  locals, body). The `format == "json"` flag is the discriminator
  the lowerer uses to route to Jbuilder lowering.
- Wire into `src/ingest/app.rs` / wherever `*.html.erb` discovery
  happens; grep `ingest_view` callers.

### Phase 2 — Runtime helper (`runtime/ruby/json_builder.rb`, ½ day)

Small pure-Ruby module (no FFI, no stdlib `JSON` dependency — the
goal is for the helper to transpile cleanly to every target):

```ruby
module JsonBuilder
  def self.encode_string(s)
    # Canonical JSON string escape: " \\ control chars to \uXXXX.
  end

  def self.encode_value(v)
    # Dispatch on type: nil → "null", bool → "true"/"false",
    # Integer/Float → to_s, String → encode_string, else → to_s.
  end

  def self.array_join(items)
    "[" + items.join(",") + "]"
  end

  def self.object_pairs(pairs)
    "{" + pairs.map { |k, v| "\"#{k}\":#{v}" }.join(",") + "}"
  end
end
```

- Add to each target's runtime manifest (the per-target lists that
  drive what `runtime/ruby/` transpiles). Search:
  `grep -rn "view_helpers" src/emit/ | grep RUNTIME` to find the
  per-target manifest constants (`RUST_RUNTIME`, `CRYSTAL_RUNTIME`,
  `TYPESCRIPT_RUNTIME`, ...).
- For Group 2 targets (Rust/Go/Elixir/Python) that don't yet consume
  the transpiled framework runtime, ship a hand-written equivalent in
  `runtime/rust/json_builder.rs` / etc. Same primitives, same names.
- Verify CRuby test: write `test/runtime/json_builder_test.rb` with
  ~10 cases (escape, encode_value dispatch, edge cases — empty
  array/object, nil/bool/number/string).

### Phase 3 — Lowerer (1½-2 days)

`src/lower/jbuilder_to_library/mod.rs` (~300-450 LOC):

The four primitives → lowered output sketches (using the railsbench
templates as worked examples):

**`json.extract! post, :id, :title, :body, :published, :created_at, :updated_at`** lowers to:

```ruby
io << "{"
io << "\"id\":" << JsonBuilder.encode_value(post.id) << ","
io << "\"title\":" << JsonBuilder.encode_value(post.title) << ","
# ... one literal pair per attribute, with comma separators built in
io << "\"updated_at\":" << JsonBuilder.encode_value(post.updated_at)
io << "}"
```

The commas are inlined at lowering time — no runtime "first attribute
or not" check needed because the lowerer knows the attribute list
statically. Closing brace likewise.

**`json.url post_url(post, format: :json)`** at the top level lowers
to a single pair, but if it follows an `extract!` in the same
template, the lowerer must compose them into one object — i.e. the
template is a single JSON object built from multiple `json.<...>`
calls. State machine in the walker tracks "are we accumulating an
object body": opens `{` on first emission, joins pairs with `,`,
closes `}` at template end.

**`json.array! @posts, partial: "posts/post", as: :post`** lowers to:

```ruby
io << "["
first = true
@posts.each do |post|
  io << "," unless first
  first = false
  io << Views::Posts.post_json(post)
end
io << "]"
```

`Views::Posts.post_json` is the lowerer's name for the lowered
`_post.json.jbuilder` partial — same naming pattern `view_to_library`
already uses (`Views::Posts.post` for the HTML partial; `_json` suffix
disambiguates by format).

**`json.partial! "posts/post", post: @post`** lowers to a single
method call:

```ruby
io << Views::Posts.post_json(@post)
```

This reuses `view_to_library/partial.rs`'s dispatch — the partial
target lookup is identical, only the name suffix differs.

**Lowerer architecture (mirror `view_to_library/`):**

- `mod.rs` — entry: `lower_jbuilder_to_library_classes(views, app, extras)`,
  parallel to `lower_views_to_library_classes`. Filters input by
  `format == "json"` and routes only those through; HTML stays on
  the ERB path.
- `walker.rs` — walks the parsed Ruby body. Recognizes the DSL
  primitives as `Send` nodes on a `json` receiver and routes each
  to a primitive-specific emitter.
- `object_builder.rs` (new) — manages the open/close/comma state for
  the implicit JSON object that wraps non-`array!`/`partial!`
  templates.
- `partial.rs` (new or extension to `view_to_library/partial.rs`) —
  partial dispatch, returns the lowered method name to call.

**Framework-stub registry:** the lowerer adds `JsonBuilder` to the
shared registry (parallel to how `view_to_library/mod.rs::insert_framework_stubs`
adds `ViewHelpers`, `RouteHelpers`, `Inflector`). Stub signatures:
all four methods return `String`.

### Phase 4 — Ruby + Spinel verification (½-1 day)

- Use `fixtures/real-blog/`'s existing three Jbuilder templates
  (already render `index.json` and `show.json` against the articles
  controller — no new fixture dir needed).
- Drive a roundhouse build through `bin/rh transpile ruby`; verify
  byte output of `index.json` and `show.json` against a Rails
  reference rendering of the same data (Rails dev server one-shot).
- Same for `bin/rh transpile spinel` — AOT compile, run, snapshot.
- Add a `compare-json` mode to `tools/compare/` if not already
  present (the existing `compare` walks `_path` request URLs; the
  `.json` requests should drop in alongside).

### Phase 5 — Crystal verification (½-1 day)

- Crystal already consumes `runtime/ruby/` end-to-end (Group 1
  target, framework_tests 8/8). The transpiled `json_builder.rb`
  should land naturally; the lowered library classes should compile
  with no Crystal-specific tweaks.
- Verify `make crystal-transpile` + `make crystal-test` and add
  Jbuilder fixtures to the Crystal compare set.
- Risk: Crystal's strict-typed `IO::Memory` may diverge from the
  `String.new + io << x` shape if the lowerer emits anything that
  trips type inference. Mirror exactly what HTML views do (they're
  already passing).

### Phase 6 — TypeScript verification (½-1 day)

- TypeScript is a Group 1 target with full transpile parity. Same
  pattern as Crystal — confirm the lowered methods compile through
  `src/emit/typescript/library.rs`, and the transpiled
  `json_builder.ts` produces correct output.
- Risk: TS string concatenation vs. template-literal preference. The
  emitter already picks one; verify Jbuilder bodies don't surprise
  it.

### Phase 7 — Rust verification (½-1 day, gated)

- Blocked on `docs/rust-migration-plan.md` reaching Phase 4
  (`framework_tests_rust` 8/8) — until then, Rust still consumes
  hand-written emit and Jbuilder lowering isn't visible to it.
- Once `rust2` consumes `runtime/ruby/` + lowered library classes,
  the same Jbuilder slice should land. May surface
  `Ty::Untyped`-on-attribute-access pressure if a model's column
  isn't fully typed at the call site; track if it bites.

### Phase 8 — Controller `format.json` plumbing (½-1 day, parallel-able)

Independent of the lowerer body work — can start any time after
Phase 0. The controller fixture pattern:

```ruby
respond_to do |format|
  format.html { render :index }
  format.json { render :index }  # picks index.json.jbuilder
end
```

needs the controller-side dispatch to route to the right view name
by format. `src/lower/controller_to_library/` is where this lives.
Scope:

- Confirm `respond_to do |format| ... end` already lowers somewhere
  (grep `format.html` / `format.json` in `src/lower/controller_to_library/`).
- If not lowered today, lower it to a content-type switch + per-format
  render call. The two render targets are
  `Views::Posts.index` (HTML) vs. `Views::Posts.index_json` (JSON).
- Content-type header set: `application/json` on the JSON branch.

Defer until at least one target's framework-test gate confirms the
JSON rendering primitives work; otherwise debugging is two-axis.

## Per-target consumption matrix

| Target | Group | Library emit consumes lowered IR? | Runtime path | When does Jbuilder land? |
|---|---|---|---|---|
| Ruby (CRuby+YJIT) | 1 | yes — runtime/ruby/ transpiles to itself | `json_builder.rb` direct | Phase 4 |
| Spinel | 1 | yes | `json_builder.rb` transpiled | Phase 4 |
| Crystal | 1 | yes | `json_builder.rb` transpiled | Phase 5 |
| TypeScript | 1 | yes | `json_builder.rb` transpiled | Phase 6 |
| Rust | 2→1 (mid-migration) | not yet — depends on rust2 progress | hand-written `runtime/rust/json_builder.rs` until rust2 lands | Phase 7 (gated) |
| Go / Elixir / Python | 2 | no | hand-written runtime equivalent + per-target emit work — out of scope this plan | n/a |

## Scope cuts (explicit)

- **`json.merge!`, `json.set!` programmatic forms** — railsbench
  doesn't use them. If a benchmark target adds them, scope is
  ~½ day each.
- **`json.cache!` / `json.cache_if!`** — caching is a runtime
  decoration; AOT transpile treats them as the body's identity
  function (emit the body, drop the cache call).
- **`json.key_format!` / `json.deep_format_keys!`** — global key
  case transformation. Doable as a lowerer flag if needed; not in v1.
- **Format-aware error responses** (`render json: { error: ... },
  status: 422`) — separate path that doesn't go through Jbuilder.
  Out of scope.
- **Inline `render json: <expr>` in controllers** (no Jbuilder
  template) — should already work via existing `JSON.generate`
  rewrite in `src/emit/typescript/expr.rs:1737-1746` and equivalents.
  Verify; if missing for any target, add to that target's emit
  pass (small, target-local).

## Risk callouts

1. **Object-builder state machine in the lowerer.** Mixing `json.foo`
   (single pair) and `json.extract!` (multi-pair) in one template
   requires the walker to track "we are accumulating an object body"
   correctly. The state machine spans the whole template body — not
   per-statement. Easy to get subtly wrong; cover with a fixture that
   interleaves both forms.
2. **Partial body inlining vs. method call.** `json.partial!` could
   either be inlined or compiled to a method call. **Recommend
   method call** (parallel to HTML `render` already doing this) for
   cross-cutting consistency, but inline-only is simpler if the
   partial is used exactly once. Don't over-engineer; method call
   is fine.
3. **String escaping correctness.** `JsonBuilder.encode_string`
   must produce RFC 8259-compliant output. Test against Rails'
   `ActiveSupport::JSON.encode` for ~30 cases including embedded
   quotes, backslashes, control chars, unicode. Drift here surfaces
   as "wire format mismatch" deep in the benchmark and is annoying
   to bisect.
4. **`json.array! @posts` when `@posts` is empty.** Should emit
   `[]`, not `[,]` or `[]` with stray separators. State machine
   must handle zero-iteration loops correctly.
5. **Order-of-keys stability.** Rails preserves source-order. Don't
   sort attribute names; emit in the order written by the developer.
   Some JSON tooling assumes alphabetic order — match Rails, not
   that tooling.
6. **`partial: "posts/post"` vs `partial: "post"`** — path resolution.
   Mirror `view_to_library/partial.rs`'s resolver verbatim; do not
   diverge.

## Mid-stream decision points

- **End of Phase 1**: confirm `View` (existing dialect type)
  carries everything the lowerer needs. If a `Jbuilder`-specific
  dialect node would help, decide now before Phase 3 wires through
  it.
- **End of Phase 3**: are the four primitives a clean catalog, or
  did the walker grow ad-hoc branches per template? Refactor before
  any per-target verification phase if so — fewer headaches than
  fixing it after 5 targets pinned to the shape.
- **End of Phase 4**: byte-equivalence with Rails reference output
  on all three railsbench templates. If not equivalent, debug
  string-escaping or key-order before fanning to other targets.
- **After Phase 5/6**: do we add stretch primitives (`json.cache!`,
  `json.merge!`, nested-block forms) now, or defer? Defer unless a
  specific benchmark fixture demands them.

## Self-contained startup checklist (for picking this up on another machine)

1. Pull `~/git/roundhouse` (this repo) to current state.
2. Read this file end-to-end.
3. Read `docs/rust-migration-plan.md` for the architectural pattern
   this plan inherits (strangler-fig + IR contract + Group 1 vs 2).
4. Read `src/ingest/view.rs` (38 LOC) and
   `src/lower/view_to_library/mod.rs` (head 100 LOC of 1276) for
   the direct precedent the new lowerer mirrors.
5. Survey `~/git/ruby-bench/benchmarks/railsbench/app/views/posts/*.jbuilder`
   (3 files, ~5 LOC total — the must-cover fixture set).
6. Run `bin/rh transpile ruby` + `bin/rh test ruby` to confirm baseline
   green before starting. The compare-ruby CI gate is the post-phase
   checkpoint each verification phase aims for.
7. Phase 0 starts with the fixture audit; Phase 1 is the smallest
   self-contained slice to write first.

Total estimate: **5-8 working days** for full five-target consumption;
**3-4 days** if trimmed to Ruby + Spinel only.
