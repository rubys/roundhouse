# Releases

Roundhouse releases are dated snapshots. A tag names the day a
snapshot was cut; what it promises is this file's entry for it and the
CI that ran on that commit. Each snapshot carries prebuilt `roundhouse`
binaries for macOS on Apple silicon and Linux x86-64 (plus untested
macOS-Intel and Windows builds), an installer script, and the source.
[`docs/guide/install.md`](docs/guide/install.md) says how to get one;
[`docs/guide/`](docs/guide/README.md) says what to do with it.

An entry records, for the three things the binary does, where they
stood when the snapshot was cut — the apps each is proven on, and the
known gaps — so that a reader of a later snapshot can see what
changed. The numbers are the ones CI asserts; where an entry and CI
disagree, CI wins.

## Unreleased

The first snapshot is being prepared. What it will say:

**Analyze** (`check`, `lsp`, `mcp`, the browser IDE). Zero errors
and zero ingest gaps on the Rails Guides store, the blog fixture, the
Rails tutorial's sample app and ONCE Campfire; the store and the blog
are also free of warnings (Campfire carries 423 on the coverage
ledger, none of them findings). Mastodon, the stress case: 674 errors,
138 ingest gaps of 20 kinds, 61 gems the analyzer does not model — the
honest denominator for a large app outside the covered surface. A
controller class-body macro the analyzer does not recognize is now a
survey entry rather than a silent drop; `rate_limit` — which the store
uses twice — is lowered to a real filter over the app's cache.

**Transpile.** Twelve targets pass the DOM-equivalence compare
against live Rails on the blog fixture on every push: rust, go,
typescript, crystal, elixir, kotlin, swift, python, csharp, ruby,
jruby, spinel. Each emitted project's README is executed verbatim in
CI (build, seed, test, browser e2e). Known gaps on the store as a
*transpile* input: one type error in Action Text's generated
attachment partial (`ActionText::Attachment#caption`), and its test
suite's sign-in helper (`ActionDispatch::TestRequest` cookie jars,
`ActiveSupport.on_load`) is not modeled, so its controller tests stop
at sign-in.
**Security:** CSRF tokens are issued but not verified on any lane
([details](docs/guide/rails-coverage.md#security-posture)).

**Compile.** The Spinel lane passes the compare, the cable-frame
compare and Campfire's own suite (275 of 288 tests, 47 of 54 files
green on the compiled binary; the remaining 13 are catalogued in the
suite report). Tested against Spinel `2026.09.12` and its `master`
through `f04b62ca` (2026-09-16); the Campfire Docker archive is built
from that pairing.

**Binary.** One `roundhouse` executable: `check`, `lsp`, `mcp`, and
`--target` for every emitter. `--version` names the snapshot and the
commit.
