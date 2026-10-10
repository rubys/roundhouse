# Writebook inventory

Roundhouse can inventory a pinned Writebook source as a large, real Rails
corpus. This is a **manual survey**, not a CI gate or whole-app conformance:
it collects ingest, analysis, lowering and Ruby/Spinel emission diagnostics,
but neither writes an emitted project nor runs Writebook. A passing inventory
does not claim runtime, native-compilation or UI parity.

The reviewed source pin is `f3fadd21907ad9b18cb23800d971c2cc25045e2a`.
Download it without installing Rails or gems, retaining Writebook's MIT
license in the source:

```sh
WRITEBOOK_SHA=f3fadd21907ad9b18cb23800d971c2cc25045e2a
curl -fsSL "https://codeload.github.com/basecamp/writebook/tar.gz/$WRITEBOOK_SHA" \
  -o /tmp/writebook.tar.gz
mkdir -p /tmp/writebook
tar -xzf /tmp/writebook.tar.gz -C /tmp/writebook --strip-components=1
WRITEBOOK_ROOT=/tmp/writebook \
  cargo test --test writebook -- --ignored --nocapture
cargo run --release --bin roundhouse -- check --continue /tmp/writebook
```

The checked-in JSON records app-relative diagnostics (severity, code,
location, message and multiplicity), lowering and emission residue, and
ingest gaps (file, message and multiplicity). Ingest gaps have no spans, so
same-message occurrences within one file cannot be distinguished. Corpus
identities cover models, library classes, controllers, dispatch routes and
their named-helper status, separate `direct` helpers with their parameters,
views, tests, fixtures and registered sources. The test permits an existing
finding to disappear, but rejects a new instance or lost corpus identity;
it does not merely compare totals. Warning identities are inventoried too.
Prism parse errors always fail, as does an explicitly run test without
`WRITEBOOK_ROOT`.

The CLI must produce its complete terminal summary and exit consistently
with its reported error count; a crash or missing summary cannot masquerade
as zero errors. Set `WRITEBOOK_INVENTORY_REPORT=/path/to/report.json` to save
the same machine-readable report locally.

After explicitly reviewing a pin or intended inventory change, refresh with:

```sh
WRITEBOOK_ROOT=/path/to/pinned/writebook \
ROUNDHOUSE_REFRESH_WRITEBOOK_INVENTORY=1 \
  cargo test --test writebook -- --ignored --nocapture
git diff -- tests/fixtures/writebook-inventory.json
```

Refresh after fixes to ratchet the inventory down. A fix that recovers skipped
source can also reveal additional diagnostics; inspect those changes rather
than hiding them to preserve a headline count. Changing the Writebook pin
requires a reviewed baseline refresh as well.

The logical-and typing correction in PR #286 adds two expression-level
`gradual_untyped` warnings at `uploads_controller.rb:58:30` and `:58:51`.
The calls at those positions already reported `untyped`; the enclosing
safe-navigation expressions now carry that type too, rather than the
incorrect `Book | untyped` union. The inventory admits exactly those two
additional warnings, without dropping the call warnings, changing the
Writebook pin, or relaxing error, gap, emission or corpus checks.

The anonymous keyword-forwarding recovery in PR #614 ingests five previously
skipped helper bodies across `ArrangementHelper`, `BooksHelper`, and
`LeavesHelper`. Their `tag.div`, `button_to`, `tag.li`, `tag.nav`, and
`form_with` destinations still lack verified retained keyword contracts, so
the inventory records five explicit all-target errors at those calls and ten
`gradual_untyped` warnings at their view callers. The five corresponding
ingest-gap occurrences and old Spinel keyword-rest declaration errors are
removed; corpus identities, lowering residue, and Ruby-emission residue are
unchanged. This reviewed inventory change records recovered source and its
remaining limits, not runnable Writebook helper or whole-app support. The
corpus pin and the manual inventory are unchanged.

## Roadmap, not a support claim

1. **Routes (resolved).** Empty resource action lists are supported, including
   nested routes under `resources :pages, only: []`; the behavior is covered by
   `tests/routes_dsl_scopes.rs::empty_only_list_means_no_actions`. The pinned
   inventory includes `GET /pages/:page_id/edits/:id`.
2. **Bounded model macros.** `positioned_within` now specializes at its literal
   call site into ordinary methods before inference and lowering. The shared
   ingester binds positional and required/optional keyword Symbol arguments
   per includer, preserves private visibility, and rejects the entire expansion
   on unsupported captures, lexical constants, side effects, ambiguous providers
   or method collisions. `tests/model_macro_expansion.rs` executes the generated
   helpers against an emitted Ruby database, including parent/filter selection,
   ordering, self-exclusion and private dispatch. This proves those helpers,
   not Positionable's complete locking/rebalancing behavior or native Writebook.
   A second emitted probe exercises scoped SQL extrema, first/end and block
   reordering, neighbor lookup, SQLite `with_lock`, and explicit rebalance SQL.
   Scalar and grouped extrema cast decimal columns to the model's `Float`
   contract, alongside schema-backed boolean and temporal conversions.
   Extrema analysis uses schema types for visible relation chains; locals and
   named scopes whose grouping state is not visible remain an inference gap.
   It deliberately does not claim deferred `after_save_commit` timing inside
   the lock or `around_create` insertion locking.
3. **Markdown declarations and runtime.** Bare `has_markdown :name` is claimed
   as a first-class named plain-text association (`lower::plain_text_attr`),
   the same shape as `has_rich_text`: scoped `markdown_<name>` storage on
   `ActionText::Markdown`, reader/predicate/writer through `.content`, ordinary
   autosave (including blanks), dependent destroy, and preload scopes.
   Assign/save/reload (including blank autosave) is pinned by abstract
   `emit_and_run`; destroy/preload synthesis by lowering unit tests — not by
   expanding the concern's interpolatable `class_eval` / leftover `has_one`
   template (generic string eval stays unsupported per
   [issue #30](https://github.com/rubys/roundhouse/issues/30)). Option-carrying
   forms (`strict_loading:`), Markdown rendering, and unmodeled gems remain
   separate obligations. The emitted Page overlay proves raw Markdown and
   empty content through `markable`, not the original Rails test harness.
   An emitted Action Text upload overlay includes a nested
   `ActionText::Markdown::Uploads` concern and proves generic
   `has_many_attached` persistence (attach, reload, filename and bytes), but
   not Redcarpet, `MarkdownRenderer`, upstream load-hook registration, or
   upload authorization.
4. **Authentication callbacks.** Composed same-concern filter macros now
   expand recursively with cycle/depth refusal; the regression verifies
   callback identity, action scope, zero analysis errors and emitted dispatch
   guards for `require_unauthenticated_access`. This does not claim full
   session/auth runtime parity or unrelated helper behavior.
5. **Original tests.** Run Writebook's own tests against the Ruby output,
   starting with positioning and Page behavior. Record total tests and named
   failures; ratchet passing tests upward. Add negative authorization tests
   for private uploads and revoked access, not just successful requests.
6. **Native and application parity.** Run the same tests on the actual Spinel
   binary with a recorded toolchain revision, then compare identically seeded
   Rails/output scenarios: create/edit/read a page, reorder, publish and upload.
   Keep upstream-master toolchain tracking advisory, separate from reproducible
   gates. Broaden to other emitters only after executable behavior is proven.

## Markdown / named plain-text association

At pin `f3fadd21907ad9b18cb23800d971c2cc25045e2a`, bare `has_markdown :body` on
`Page` is claimed by `lower::plain_text_attr` (ingest skips concern `class_eval`
expansion for that form so leftover interpolated `has_one`/scopes do not
fail-close). Association scoping by owner/name, build/assign/save/reload, and
ordinary autosave including blanks are pinned by
`tests/emit_and_run.rs::named_plain_text_attr_assign_save_reload`. Dependent
destroy and preload-scope synthesis are pinned by
`tests/plain_text_attr_lowering.rs` (not yet by a runtime destroy/preload
overlay). Storage for `ActionText::Markdown` (table `action_text_markdowns`,
attr `content`) was the prior prerequisite and remains covered by
`tests/action_text_markdown_ingest.rs` plus the storage-only emit overlay.
`delegated_type` singular readers composing with the plain-text attr
(`entry.page.body`) are pinned by
`tests/relation_delegated_reader_typing.rs` and
`tests/emit_and_run.rs::delegated_type_singular_reader_plain_text_body_runs`.

Model `delegate` declarations and delegates inside concern `included do`
blocks are lowered to ordinary methods after those items are spliced into
their models. Top-level `delegate` calls in module bodies remain unsupported:
their receiver and generated method surface depend on each eventual includer.
This covers
zero-argument forwarding, setters and fixed-arity operators, `prefix: true` or
an explicit Symbol/String prefix. `allow_nil: true` remains unsupported because
Rails distinguishes a nil target that responds to the delegated method from
one that does not; a simple nil guard would change behavior. Delegated names
that collide with the model's synthesized method surface are also left
unexpanded rather than silently choosing the wrong definition. The four
Writebook `Leafable#title` check errors are cleared; the behavior is also
exercised by abstract `emit_and_run` overlays, which execute emitted Ruby
through persisted `belongs_to` and polymorphic `delegated_type` associations.
The latter covers Leaf's `searchable_content`-style delegation. This does not
claim collection association proxies, arbitrary argument/block forwarding or
unsupported options such as `private:`; targets using `yield` or
`block_given?` are left unexpanded, as are declarations under a lexical
`private`/`protected` marker rather than being emitted with the wrong
visibility. The remaining `URI::HTTPS`
constant error is cleared by registering the bundled Ruby class value; this
does not claim the separate embed-provider or sanitizer integrations.
Renderer/Redcarpet, upstream `ActiveSupport.on_load` registration, upload
authorization, option-carrying `strict_loading:`, and unrelated load-hook
notifications remain separate. Generic string eval stays unsupported.

The pinned Page tests were attempted with `ruby -Itest -I.
test/models/page_test.rb`, but the upstream harness is blocked in this orb by
the unavailable pinned Ruby 3.4.7 and a missing locked `basecamp/useragent`
checkout. A separate emitted-app probe proves the two `markable` cases (raw
Markdown and empty content) against the shared runtime. The `html preview` and
`searchable_content` cases remain unverified; booting the upstream-shaped app
also reaches the unmodeled/uninstalled `Redcarpet::Markdown` boundary. No
native Spinel compilation or execution is claimed.
