# Verifying — the compare oracle

How do you know the emitted app is the same app? Not from its tests
passing: the tests were translated too, and a wrong translation of a
test agrees with a wrong translation of the code. The answer
roundhouse uses is a differential oracle — the same URL fetched from
live Rails and from the emitted server must produce the same response
— and it is available for your app, not only the fixtures.

## What "the same" means

`roundhouse-compare` (in `tools/compare/`) fetches each URL from both
servers and compares the responses as a browser would see them, not
as bytes:

- HTML is parsed into a DOM on both sides and the trees are walked in
  lockstep. The element tree must match exactly, text nodes must match
  byte for byte (whitespace included), attribute *order* is ignored,
  and HTML comments are dropped.
- JSON (`.json` paths) is compared structurally, value for value.
- Values that legitimately differ between any two servers are replaced
  by placeholders before the diff: CSRF tokens, session ids, asset
  fingerprints, the signature half of a Turbo stream name. The default
  rules cover Rails' own; a config file (`--config`, see
  `tools/compare/config.example.yaml`) adds your app's.

Anything else that differs is reported as a failure, with the first
divergence and its path in both trees. The bar is deliberately a
browser's: if a scraper or a stylesheet selector would see a
difference, it is a difference.

## Running it against your app

Build the tool once (it is a separate small crate, not part of the
snapshot binary):

```sh
cd tools/compare && cargo build --release
```

Then, with your Rails app running on one port and the emitted app on
another — both seeded with the same rows —

```sh
tools/compare/target/release/roundhouse-compare \
  --reference http://localhost:4000 \
  --target    http://localhost:3000 \
  --path / --path /articles --path /articles/1 --path /articles/1.json
```

`--verbose` prints the whole diff rather than the first divergence.
Exit status is 0 on all-match and non-zero on any difference, so it
runs as a CI gate.

Seeding both sides identically is the part that takes care. The
emitted app ships `db/seed.sql` — schema plus the app's `db/seeds.rb`
rows as SQL — and the Rails side should be `db:seed`ed from the same
`seeds.rb`. Ids, timestamps and anything derived from them must line
up; the timestamps are the usual culprit, and freezing them in the
seeds is the usual fix.

## The scripted loop

For the fixtures, `scripts/compare <target>` drives the whole thing —
regenerate, seed both databases, boot Rails and the target, run the
tool over the standard page list, tear down — and is what the CI
compare matrix runs:

```sh
bin/rh compare rust          # any server target; ruby / spinel too
```

`scripts/campfire-compare` is the Campfire half: the same walk on
both lanes, plus the piece no page compare can carry — the Turbo
Stream frames that arrive over `/cable` are captured on both sides and
diffed after the same normalization. Both scripts are readable and
short; adapting one to your app is mostly replacing the path list.

## Reading a failure

A compare failure is one of three things, and the report usually says
which:

1. **A value that should have been masked** — a fresh nonce, a
   generated id, a time. Add a rule to the config; this is not a bug.
2. **A deliberate divergence** — something in
   [the ledger](rails-coverage.md#deliberate-divergences). The fixture
   compares carry a mask for each of these, printed when applied, so
   new differences are not drowned out by known ones. Your compare
   should do the same rather than accept the whole page.
3. **A translation bug.** Everything else. A view helper producing a
   different attribute, a partial rendered with the wrong locals, a
   query returning rows in a different order. Report it with the two
   URLs and the diff — that is the entire reproduction.

The third kind is what the oracle exists to find, and the project's
position is that every one of them is a roundhouse bug, never an
acceptable difference. That is why the compare matrix runs on every
push and why a target that goes red stays off the
[supported list](targets.md) until it is green.

## Beyond the page

Two more layers sit around the DOM compare in the project's own CI,
and both are available to an emitted app:

- **The app's own tests**, translated, run against the emitted code —
  the `Test` section of every emitted README.
- **Browser end-to-end tests** (Playwright, in the emitted `e2e/`),
  for the behavior a static diff can't reach: a form submitted, a
  Turbo Stream arriving in a second tab, a confirm dialog answered.

The compare is the strongest of the three because it needs no
expectations written: Rails is the expectation.
