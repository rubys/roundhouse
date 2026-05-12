# Parameters elimination plan

Finish the `ActionController::Parameters` retirement across TypeScript
and Crystal targets (Rust already done; Ruby/Spinel may also drop with
the same change). Mirrors the Validations retirement landed on
2026-05-12 (commit 964ba3a) but with one extra prerequisite: the
lowerer's params-rewrite needs to be extended to collapse the last
runtime-method call sites before the module can come off.

## Strategic context

- One of the two remaining `runtime/ruby/` files that exists to
  hand-roll polymorphism typed targets have natively (the other was
  Validations; HWIA already retired from typed targets per Phase 2.5(b)
  of `docs/rust-migration-plan.md`).
- The Validations retirement (commit 964ba3a) proved the pattern:
  inline-lower at IR time, drop runtime file, replace framework test
  with inline lowerer unit tests. Same shape applies here; the
  difference is *finishing* the lowerer rather than just deleting the
  module.
- Runtime savings on spinel: modest but real. Per-request Parameters
  allocation (recursive on nested bodies) drops; a handful of method
  dispatches collapse to direct Hash access. Bigger win is on
  spinel-compile time + emitted binary size — the recursive
  `String | Parameters` value union no longer threads through every
  `params[k]?` access in whole-program inference.
- Architectural symmetry: rust2 already dropped parameters.rb as a
  side-effect of HWIA elimination. Once TS/Crystal drop it, four of
  the four typed Group 1 targets are off; only Ruby/Spinel (which use
  the .rb file as their actual runtime) need a choice.

## What's already done (do not redo)

The lowerer that synthesizes per-resource `<Resource>Params` classes
**already exists, target-agnostic, runs for Ruby + TS + Crystal**:

- `src/lower/controller_to_library/params.rs` (696 LOC). Three entry
  points: `collect_specs`, `synthesize_params_classes`,
  `rewrite_to_from_raw`.
- Called from `src/emit/ruby.rs:92`, `src/emit/typescript.rs:229`,
  `src/emit/crystal.rs:170`.
- Recognizes both source forms: `params.expect(article: [:title, :body])`
  (Rails 8) and `params.require(:article).permit(:title, :body)` (older).
- Emits a `LibraryClass` per resource with `attr_accessor` columns +
  `self.from_raw(params)` that calls `params.fetch("title", "")` etc.
- Tags the result with `LibraryClassOrigin::ResourceParams { resource,
  fields }` so per-target collapsers can group them with their
  controllers.

Rust already dropped `parameters.rb` from `RUST_RUNTIME` as part of
the HWIA-elimination sweep (commits e4c5207, 441655a, 696a629,
53e733a). This work brings TS + Crystal to the same state.

## What's left

Three categories of `Parameters.<method>` call sites still survive the
existing rewrite. These are the actual blockers:

1. **`params.require(:r).to_h()` prefix on `from_raw` argument.** The
   current `rewrite_to_from_raw` preserves this prefix — the rewritten
   body is `<Resource>Params.from_raw(@params.require("article").to_h())`
   rather than `<Resource>Params.from_raw(@params)`. So `Parameters.require`
   and `Parameters.to_h` are still load-bearing. Verified in
   `tests/browser_smoke/.emitted/dist/assets/worker-*.js`:
   ```
   async article_params() {
     return await ArticleParams.from_raw(
       this.params.require("article").to_h()
     );
   }
   ```
2. **`params.expect(:id)` scalar form.** The non-permit form (one
   symbol, no field list) reads a scalar from the top-level params.
   Used by `fixtures/real-blog/app/controllers/comments_controller.rb`
   (`params.expect(:id)`, `params.expect(:article_id)`) and the same
   in `articles_controller.rb`. No rewrite path today; lands as a
   direct call to `Parameters#expect`.
3. **`@params = ActionController::Parameters.new({})`** in
   `runtime/ruby/action_controller/base.rb:37`. Even with every call
   site rewritten, the initial-value type for `@params` keeps the
   runtime alive — `@params : Parameters` declared in `base.rbs` is
   the typed-target's seed. Swapping this to `Hash[String, untyped]`
   touches both `.rb` and `.rbs`, plus the harness (`main.rb` and
   `test_helper.rb`) which currently passes
   `controller.params = ActionController::Parameters.new(merged)`.

## Decisions to lock in at Phase 0

- **Ruby/Spinel: keep or drop?** `docs/rust-migration-plan.md` line 85
  said "stays for CRuby/Spinel as canonical Ruby runtime." That was
  before the Validations retirement set the symmetric-drop precedent.
  Two paths:
  - **(a) Symmetric drop.** Once the rewrite covers every call site,
    `parameters.rb` is dead code on Ruby/Spinel too. Drop it
    everywhere. Same shape as the Validations commit.
  - **(b) Asymmetric keep.** TS/Crystal drop; Ruby/Spinel keep the
    module as a "canonical Ruby runtime" the framework_tests can
    exercise directly. Costs ~150 LOC of shipped Ruby on those
    targets, gains a hand-call testable surface.
  - Recommend **(a)** for consistency with Validations. The framework
    test (`parameters_test.rb`) can be rewritten to exercise the
    lowerer (mirror the Validations swap).
- **`from_raw` signature.** Today's synthesized `from_raw(params)`
  expects either a Parameters or a Hash (uses `.fetch(k, default)`
  which works on both). After this change, the input is always a
  Hash, sourced from `@params["article"]` directly. Update the synth
  template to reach into the resource key before extracting fields:
  ```ruby
  def self.from_raw(params)
    instance = new
    instance.title = params.fetch("title", "")
    instance.body  = params.fetch("body", "")
    instance
  end
  ```
  becomes the same body but called with `params["article"]` extracted
  by the rewriter, OR the synthesis includes the resource-key fetch:
  ```ruby
  def self.from_raw(params)
    sub = params.fetch("article", {})
    instance = new
    instance.title = sub.fetch("title", "")
    ...
  end
  ```
  Either works; pick at Phase 1.

## Phase status

| # | Phase | Days | Status |
|---|---|---|---|
| 0 | Audit residual Parameters call sites + lock decisions | ½ | not started |
| 1 | Extend `rewrite_to_from_raw` to drop `.require(:r).to_h()` prefix | ½-1 | not started |
| 2 | Rewrite `params.expect(:id)` scalar form to direct hash access | ½ | not started |
| 3 | Swap `@params` type in `action_controller/base.rb` + `.rbs` from Parameters to Hash; update harness | ½-1 | not started |
| 4 | Add `#[cfg(test)] mod tests` to `src/lower/controller_to_library/params.rs` covering new rewrites | ½ | parallel-able with 1+2 |
| 5 | Drop `parameters.rb` `RuntimeEntry` from `TYPESCRIPT_RUNTIME` + `CRYSTAL_RUNTIME`; drop `"parameters"` from crystal RUNTIME_ORDER; drop `Parameters` import on TS base.rb entry | ½ | blocked on 1-3 |
| 6 | Drop `require_relative "parameters"` from `action_controller/base.rb`; drop `ParameterMissing` import handling in TS library.rs | ½ | blocked on 5 |
| 7 | Remove `parameters_test_passes_under_*` from `framework_tests_{ruby,typescript,crystal}.rs` (3 files) | ½ | blocked on 4 |
| 8 | Decide Ruby/Spinel symmetric drop or keep; if drop, delete .rb/.rbs/_test.rb files | ½ | blocked on 0 decision + 7 |
| 9 | Run cargo test --tests + framework_tests_{ruby,typescript,crystal} --ignored + real-blog gates | ½ | blocked on 5-8 |

Total: **~3-5 working days** for the full TS+Crystal+(optional)Ruby+Spinel
sweep. Trimmed to TS+Crystal only (Phases 0-7, deferring Ruby/Spinel
drop): **~2-3 days**.

## Reference fixtures

Real-blog (`fixtures/real-blog/app/controllers/`) is the must-cover
surface. Five call sites span the four primitive forms:

```ruby
# articles_controller.rb:63 — scalar expect
@article = Article.find(params.expect(:id))

# articles_controller.rb:68 — permit expect (already rewritten)
params.expect(article: [ :title, :body ])

# comments_controller.rb:14, 22 — scalar expect on association FK
@comment = @article.comments.find(params.expect(:id))
@article = Article.find(params.expect(:article_id))

# comments_controller.rb:26 — permit expect (already rewritten)
params.expect(comment: [ :commenter, :body ])
```

The two `permit expect` forms already go through `rewrite_to_from_raw`.
The three `scalar expect` forms are the new rewrite-work.

Spinel-blog (`runtime/spinel/scaffold/test/integration/`) is the
secondary surface — verify any direct `params.<x>` uses in test
fixtures don't bypass the rewriter.

## Phase details

### Phase 0 — Audit + lock decisions (½ day)

- Grep `params\.` across `fixtures/real-blog/` and
  `runtime/spinel/scaffold/` to confirm the call-site catalog above is
  complete. Stretch forms (`.merge`, `.fetch` on params directly,
  `.permitted?`) may surface; decide rewrite vs. defer.
- Decision: Ruby/Spinel symmetric drop or keep? (See "Decisions to
  lock in" above.) Recommend symmetric drop.
- Decision: `from_raw` signature with resource-key fetch inline, or
  rewriter passes pre-extracted sub-hash. Pick one before Phase 1.
- Read `src/lower/controller_to_library/params.rs` end-to-end
  (696 LOC) — the lowerer file all subsequent work edits.

### Phase 1 — Drop `.require(:r).to_h()` prefix (½-1 day)

Today's `rewrite_to_from_raw` produces:
```ruby
def article_params
  ArticleParams.from_raw(@params.require("article").to_h)
end
```

Target shape:
```ruby
def article_params
  ArticleParams.from_raw(@params)
end
```

The `from_raw` body (synthesized in
`synthesize_params_classes`) gains the resource-key dive:
```ruby
def self.from_raw(params)
  sub = params.fetch("article", {})
  instance = new
  instance.title = sub.fetch("title", "")
  ...
end
```

Implementation: in `rewrite_to_from_raw`, recognize the
`.require(<sym>).to_h` chain on `@params` and replace the whole
argument with `@params`. The synthesis template (in
`synthesize_params_classes`) gains the `sub = params.fetch(<resource>,
{})` line — resource symbol already in the `ParamsSpec`.

Risk: `from_raw` is called in two paths today — `<resource>_params`
helpers (the simple form rewriting handles) AND
`<Resource>Params.from_raw(...)` direct sites in tests. Audit the
second category before changing the synthesis template.

### Phase 2 — `params.expect(:id)` scalar rewrite (½ day)

Three live call sites in `fixtures/real-blog/` (see above). Rewrite:
```ruby
params.expect(:id)         →    @params.fetch("id", nil)
params.expect(:article_id) →    @params.fetch("article_id", nil)
```

The rewrite recognizes `Send { recv: Some(params_var), method: "expect",
args: [Symbol] }` where the single arg is a Symbol literal (not a Hash
kwarg — those go through Phase 1). Replace with `Send { recv: @params,
method: "fetch", args: [Str(symbol_name), Nil] }`.

`params.expect(:id)` returns a String in Rails 8 (typed coercion); the
direct `Hash#fetch` returns the underlying value, which is a String
when the request body is form-urlencoded. For ID coercion (`Article.find`
takes Integer), the call site already wraps in `Integer(...)` or the
emitter does coercion via `Number(this.params.fetch(...))` (TS) — keep
that as-is, the typed coercion isn't the rewrite's job.

### Phase 3 — Swap `@params` type (½-1 day)

`runtime/ruby/action_controller/base.rb:37`:
```ruby
@params = ActionController::Parameters.new({})
```
becomes:
```ruby
@params = {}
```

`base.rbs` line 16-17:
```rbs
def params: () -> Parameters
def params=: (Parameters) -> Parameters
```
becomes:
```rbs
def params: () -> Hash[String, untyped]
def params=: (Hash[String, untyped]) -> Hash[String, untyped]
```

Harness side — wherever `controller.params = Parameters.new(merged)`
appears (`main.rb`, `test_helper.rb`, per-target shims):
```ruby
controller.params = merged    # raw Hash, not Parameters wrapper
```

Verify per-target Hash shape matches what the rewriter produces. TS
emits `Map`-style access on raw objects; Crystal emits `Hash(String,
JSON::Any)` style — both work with `.fetch("k", default)`. Spinel
inferences `@params : Hash[String, untyped]` from the new initial.

### Phase 4 — Inline lowerer tests (½ day, parallel-able)

Mirror the Validations commit's test pattern. Add `#[cfg(test)] mod
tests` to `src/lower/controller_to_library/params.rs` covering:

- `match_permit_call` recognition: `params.expect(article: [:title,
  :body])` → ParamsSpec with resource=`article`, fields=`[title, body]`.
- Same for `params.require(:article).permit(:title, :body)` (older
  form).
- `synthesize_params_classes`: input ParamsSpec → output LibraryClass
  with the right attr_accessor list + from_raw body shape.
- `rewrite_to_from_raw`: input controller body with `params.expect(...)`
  → output body with `<Resource>Params.from_raw(@params)`. Plus Phase
  1's new rewrite for `.require(:r).to_h()` prefix elimination.
- `rewrite_params_expect_scalar` (Phase 2): input `params.expect(:id)`
  → output `@params.fetch("id", nil)`.

~8-10 cases total. Replaces the deleted `parameters_test.rb` as the
regression boundary.

### Phase 5 — Drop runtime entries (½ day)

Mirror today's Validations commit:
- `src/runtime_loader.rs`: remove `parameters.rb` `RuntimeEntry` from
  `TYPESCRIPT_RUNTIME` (line 354 area) and `CRYSTAL_RUNTIME` (line 487
  area).
- Same file: drop `("Parameters", "./parameters.js")` from the TS
  base.rb entry's `imports` (line 342 area).
- `src/emit/crystal.rs`: drop `"parameters"` from `RUNTIME_ORDER`.
- `src/emit/typescript/library.rs`: drop `("ParameterMissing",
  "parameters")` from the RUNTIME_SRC_IMPORTS table (line 701) — the
  `ParameterMissing` error class isn't raised anywhere after the
  rewrites land (the runtime's `require` was the only raiser).

### Phase 6 — Drop runtime Ruby includes (½ day)

- `runtime/ruby/action_controller/base.rb` line 1: drop
  `require_relative "parameters"`.
- `runtime/ruby/action_controller/base.rbs`: drop any Parameters
  references already handled in Phase 3.

### Phase 7 — Remove framework test runners (½ day)

Three files; mirror today's Validations commit:
- `tests/framework_tests_ruby.rs`: delete `parameters_test_passes_under_cruby`.
- `tests/framework_tests_typescript.rs`: delete `parameters_test_passes_under_tsx`.
- `tests/framework_tests_crystal.rs`: delete `parameters_test_passes_under_crystal`.

Also: `tests/runtime_src_emit_typescript.rs` — remove the
`parameters.rb` / `parameters.rbs` entry from `RUNTIME_PAIRS`.

### Phase 8 — Decide + delete (½ day)

Per Phase 0 decision:
- **If symmetric drop:** delete `runtime/ruby/action_controller/
  parameters.rb` + `.rbs` + `runtime/ruby/test/action_controller/
  parameters_test.rb`. Net: -174 LOC framework + -test_helper.
- **If asymmetric keep:** files stay in `runtime/ruby/` for Ruby/Spinel
  use; `parameters_test.rb` is kept but moved out of the
  framework_tests gates (it tests a module only Ruby/Spinel use).

### Phase 9 — Verify (½ day)

- `cargo test --tests`: all pass (lowerer tests + spinel_blog_library
  + emit assertions).
- `framework_tests_ruby --ignored`: 6/6 (drops parameters → 7→6).
- `framework_tests_typescript --ignored`: 6/6.
- `framework_tests_crystal --ignored`: 6/6.
- `ruby_toolchain --ignored real_blog_spinel_tests_pass`: ok.
- Real-blog compare for at least one target (Ruby + TS recommended) to
  confirm byte-output of controllers unchanged.

## Risk callouts

1. **`from_raw` callers outside controllers.** If any test or fixture
   directly calls `ArticleParams.from_raw(some_hash)` with a hash that
   doesn't have an `"article"` sub-key (e.g. it pre-extracted), the
   resource-key dive in the synthesis template breaks that call site.
   Audit during Phase 1. Mitigation: synthesis template uses
   `params.fetch(<resource>, params)` — falls back to top-level if the
   sub-key is missing, so callers passing pre-extracted sub-hashes
   still work.

2. **TS strict-mode `@params` typing.** Today `@params: Parameters` is
   a known class; tsc resolves all method calls. Swapping to
   `Hash[String, untyped]` → `Record<string, unknown>` in TS means
   `params.fetch("k", "")` becomes a Map-method call on a Record —
   doesn't typecheck. Per-target emit likely needs adjustment: TS
   should emit `params["k"] ?? ""` rather than `params.fetch("k", "")`.
   Audit before declaring Phase 3 done.

3. **`ParameterMissing` exception class.** Today's
   `Parameters#require` raises `ParameterMissing` when the key is
   absent or empty. The Phase 1 rewrite replaces require chains with
   direct `from_raw(@params)` — no `ParameterMissing` raise. Verify no
   test (or controller) catches `ParameterMissing` explicitly; if any
   do, they need updating to handle the missing-resource case some
   other way (e.g. nil-check on `params.fetch("article", nil)`).

4. **HWIA interaction.** HWIA was already retired from typed targets
   (Phase 2.5(b)). `@params` typing for typed targets shouldn't go
   back through HWIA; verify by grepping `HashWithIndifferentAccess`
   in `runtime/typescript/`, `runtime/crystal/` after Phase 3.

5. **Symbol/string key drift.** Rails' Parameters auto-coerces symbol
   keys to strings (HWIA-style). Real-blog uses `params.expect(:id)`
   (symbol). After rewrite, the access is `@params.fetch("id", nil)`
   (string key). The request-body parser already produces string keys
   (URL-encoded forms are string-keyed at the wire level), so this is
   a no-op in practice — but worth confirming the harness's
   `controller.params = merged` is passing string-keyed merged data,
   not symbol-keyed.

6. **Magnitude vs. effort.** Per the perf discussion: spinel runtime
   speedup is modest (1-2% real-blog, 5-10% params-heavy
   microbenchmark). Architectural and compile-time wins are the larger
   benefit. Don't oversell as a perf change; lead with
   architectural-symmetry-with-Rust + simpler-inference framing.

## Mid-stream decision points

- **End of Phase 1:** can the rewrite cleanly produce
  `from_raw(@params)` for all `permit expect` forms in real-blog
  controllers? Sanity-check the emit-preview against the Rails
  reference rendering for one controller before continuing.
- **End of Phase 2:** scalar `params.expect(:id)` rewrite — does it
  handle compound calls (`Integer(params.expect(:id))`)? If the rewrite
  has to walk through `Integer(...)` wrapping, add to test cases.
- **End of Phase 3:** swap `@params` type — does TS strict-mode
  type-check? If `params.fetch()` doesn't resolve on Record, decide
  fast: per-target emit fix-up vs. defer the runtime-type swap by
  keeping `Parameters` as a thin Hash-wrapper.
- **End of Phase 5/6:** all four typed targets dropped. Decide
  Ruby/Spinel symmetric drop or keep — pure judgment call, costs are
  symmetric.

## Self-contained startup checklist

1. Pull `~/git/roundhouse` (this repo) to current state.
2. Read this file end-to-end.
3. Read `docs/rust-migration-plan.md` Phase 2.5 (lines 79-86) for the
   architectural context.
4. Read commit 964ba3a (Retire ActiveRecord::Validations…) for the
   shape this plan mirrors. The diff is the template.
5. Read `src/lower/controller_to_library/params.rs` end-to-end
   (696 LOC). This is the file Phases 1-4 edit.
6. Read `runtime/ruby/action_controller/parameters.rb` (132 LOC) and
   `parameters.rbs` (42 LOC) — the runtime being retired.
7. Survey `fixtures/real-blog/app/controllers/{articles,comments}_
   controller.rb` for the five live call sites.
8. Run `cargo test --tests` + `cargo test --test framework_tests_ruby
   -- --ignored` to confirm baseline green before starting.
9. Phase 0 starts with the residual-call-site audit; Phase 1 is the
   smallest self-contained slice to write first.

Total estimate: **3-5 days** for TS+Crystal+(optional)Ruby+Spinel;
**2-3 days** for TS+Crystal only.
