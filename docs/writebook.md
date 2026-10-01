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
3. **Markdown declarations and runtime.** `has_markdown` remains unsupported.
   Its statically known `class_eval` template could be parsed without executing
   application Ruby, but allowing that would amend the boundary in
   [issue #30](https://github.com/rubys/roundhouse/issues/30). First establish
   association scoping by owner/name, build/assign/save/reload behavior,
   inverse/autosave/destruction semantics and load-hook installation. Expanding
   only its methods would not make the declaration work. Markdown rendering,
   attachments and unmodeled gems remain separate obligations.
4. **Original tests.** Run Writebook's own tests against the Ruby output,
   starting with positioning and Page behavior. Record total tests and named
   failures; ratchet passing tests upward. Add negative authorization tests
   for private uploads and revoked access, not just successful requests.
5. **Native and application parity.** Run the same tests on the actual Spinel
   binary with a recorded toolchain revision, then compare identically seeded
   Rails/output scenarios: create/edit/read a page, reorder, publish and upload.
   Keep upstream-master toolchain tracking advisory, separate from reproducible
   gates. Broaden to other emitters only after executable behavior is proven.
