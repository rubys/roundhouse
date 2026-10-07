# Writebook inventory

Roundhouse inventories the pinned Writebook source as a large, real Rails
corpus. This is **not whole-app conformance**: the lane collects ingest,
analysis, lowering and Ruby/Spinel emission diagnostics, but neither writes
an emitted project nor runs Writebook. A passing inventory does not claim
runtime, native-compilation or UI parity.

The pin is `WRITEBOOK_SHA` in `.github/workflows/ci.yml`. Download it without
installing Rails or gems, retaining Writebook's MIT license in the source:

```sh
WRITEBOOK_SHA=$(sed -n 's/^  WRITEBOOK_SHA: //p' .github/workflows/ci.yml)
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

CI uploads the actual inventory and full CLI report even when the gate fails.
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

## Roadmap, not a support claim

1. **Routes.** [PR #199](https://github.com/rubys/roundhouse/pull/199) owns the
   `resources :pages, only: []` fix. This contribution does not duplicate it.
   Until it lands, the survey skips that resource and its nested edits route,
   while retaining the other routes; the inventory honestly records that gap.
   Refresh the baseline when the nested route is recovered.
2. **Bounded model macros.** `positioned_within` now specializes at its literal
   call site into ordinary methods before inference and lowering. The shared
   ingester binds positional and required/optional keyword Symbol arguments
   per includer, preserves private visibility, and rejects the entire expansion
   on unsupported captures, lexical constants, side effects, ambiguous providers
   or method collisions. `tests/model_macro_expansion.rs` executes the generated
   helpers against an emitted Ruby database, including parent/filter selection,
   ordering, self-exclusion and private dispatch. This proves those helpers,
   not Positionable's complete locking/rebalancing behavior or native Writebook.
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
   forms (`strict_loading:`), Markdown rendering, attachments and unmodeled
   gems remain separate obligations.
4. **Original tests.** Run Writebook's own tests against the Ruby output,
   starting with positioning and Page behavior. Record total tests and named
   failures; ratchet passing tests upward. Add negative authorization tests
   for private uploads and revoked access, not just successful requests.
5. **Native and application parity.** Run the same tests on the actual Spinel
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

Still separate: `delegate :title, to: :leafable` through Leafable,
renderer/Redcarpet, embeds/uploads, option-carrying `strict_loading:`, and
load-hook notifications. Generic string eval stays unsupported.

The original Page tests were emitted with `--target ruby --survey
--allow-unsupported` and attempted with `ruby -Itest -I. test/models/page_test.rb`.
Boot failed at the emitted `ActionText::Markdown < Record` with
`NameError: uninitialized constant Record`, before any test ran. The four cases
remain **blocked**, not individually failing or passing: `html preview`,
`markable returns raw markdown content`, `markable returns empty string when body
is empty`, and `searchable_content re-encodes HTML entities decoded by
to_plain_text`. No native Spinel compilation or execution is claimed.
