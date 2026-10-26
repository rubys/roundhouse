# Paired per-route benchmarks

`scripts/bench` measures how much throughput each target sustains under
load. [`scripts/bench-paired`](../../scripts/bench-paired) answers a
narrower question: on one route, is the target slower or faster than the
reference, by more than the noise? It is the per-route counterpart to
[`roundhouse-compare`](../guide/verifying.md): the compare proves that two
servers return the same response for a path, and this times that path on
both.

An average across routes can hide a route that got slower. An emitted app
can be several times faster than Rails overall and still regress on one
route that takes a residue or fallback path. This tool reports each
route separately and does not average them away.

## Method

The protocol is adapted from Yuan et al., *Where Does the Performance Go?
Understanding and Recovering Performance Regressions in C-to-Rust
Translation* (arXiv:2610.23044). That paper found translation slowdowns
varied widely between operations of the same project, for example
−0.3% and +62% on two hash functions in one library.

- **One route is one operation.** A route is a complete request whose
  response can be checked. Check it with `roundhouse-compare` on the same
  paths and seeded rows before timing it; `bench-paired` only times.
- **A sample is per-request cost.** One keep-alive connection sends
  requests back to back (`wrk -t1 -c1`) for `--sample-sec` seconds. The
  sample is the mean time per request.
- **Pairs alternate.** Each pair is a reference sample and a target
  sample taken back to back, and the order flips every pair. Drift
  (thermals, a neighbour, a JIT compiling elsewhere) then lands on both
  sides equally. Warm-up pairs are taken first and discarded.
- **Unstable batches are retaken.** A batch is stable when the
  coefficient of variation (CV) of its pairwise ratios is at most
  `--max-cv` (3%). An unstable batch is retaken up to `--retries` times.
  If it never settles, it is reported as unstable rather than dropped.
- **Outliers are trimmed, then the median is taken.** Pairwise ratios
  outside Tukey's 1.5 × IQR fences are dropped. The route's ratio is the
  median of the rest, with a seeded bootstrap 95% interval.

The ratio is target time divided by reference time, so a value below 1
means the target is faster. Each route gets a verdict by comparing its
interval with `--tolerance` (default 3%):

| Verdict | Meaning |
|---|---|
| `slower` | the whole interval is above 1 + tolerance |
| `faster` | the whole interval is below 1 − tolerance |
| `same` | the whole interval is inside 1 ± tolerance |
| `inconclusive` | none of the above: take more pairs or quieten the machine |

`--fail-on-slower` exits 1 when any route is `slower`. An `inconclusive`
or unstable route does not fail the run. Read the CV column before you
trust a verdict.

Client and loopback costs are added equally to both sides, which pulls
every ratio toward 1. That understates a large speedup or slowdown, but
never reverses the direction.

## Real-blog: Rails against the emitted Ruby app

Seed both servers from the same database, the way `scripts/compare` and
`scripts/bench` do. Start each in the configuration you want to compare:
production environment, the same JIT and worker settings, each pinned to
its own core. Then time the compare paths:

```sh
bin/rh fixture && bin/rh transpile ruby
sqlite3 fixtures/real-blog/storage/development.sqlite3 \
  ".backup fixtures/real-blog/storage/production.sqlite3"
mkdir -p build/transpiled-blog-ruby/storage
sqlite3 fixtures/real-blog/storage/development.sqlite3 \
  ".backup build/transpiled-blog-ruby/storage/development.sqlite3"

(cd fixtures/real-blog && RAILS_ENV=production SECRET_KEY_BASE=bench \
   RUBYOPT=--yjit taskset -c 2 bin/rails server -p 4000 -e production) &
(cd build/transpiled-blog-ruby && \
   BUNDLE_GEMFILE=../../runtime/spinel/scaffold/Gemfile \
   BLOG_DB=storage/development.sqlite3 RAILS_ENV=production PORT=3000 \
   RUBYOPT=--yjit taskset -c 4 bundle exec puma -C config/puma.rb config.ru) &

tools/compare/target/release/roundhouse-compare \
  --reference http://localhost:4000 --target http://localhost:3000 \
  --path /articles --path /articles/1      # the responses must match first
taskset -c 6 scripts/bench-paired \
  --reference http://localhost:4000 --target http://localhost:3000 \
  --label-reference rails --label-target ruby --out paired.json
```

With no `--path`, it times the paths that `scripts/compare` checks for
the ruby target. `taskset` is Linux-only; on macOS, leave it out and
expect noisier batches.

## Gating a change to the emitter

To check a Roundhouse change for performance regressions, emit the
same fixture with the baseline build and with the candidate build. Serve
both trees the same way, and time the candidate against the baseline:

```sh
scripts/bench-paired --reference http://localhost:3001 \
  --target http://localhost:3002 --fail-on-slower --out gate.json
```

Here the reference is the baseline emit. A route is flagged `slower`
only when the whole interval clears the tolerance, so noise alone does
not fail the gate.

## Re-reading a result

`--out` writes every sample, the configuration, a `scripts/bench-env`
snapshot of the machine, and the summary. `--analyze FILE` re-reads such
a file without measuring again. Pass `--tolerance`, `--max-cv` or `--seed`
to re-judge the same samples with different settings.
