# Lobsters: pinned real-app breadth control

Executed against published exporter/compiler control
[`4480841`](https://github.com/thomasklemm/roundhouse/commit/4480841589a8fedca066822f6730bc3959abd316),
with canonical `rubys/roundhouse:main`
[`2f95d399`](https://github.com/rubys/roundhouse/commit/2f95d399) as an ancestor,
not fork main.
App: the repository's actual
[ruby-bench Lobsters pin](https://github.com/ruby/ruby-bench/tree/d771f81f1ce9db51376e03ca7b8e6a83160556d4/benchmarks/lobsters).
MRI 3.4.8; frozen Bundler 4.0.12 bundle; Rails 8.1.1; host Prism 1.9.0.
No app source, lockfile, gem, schema or compiler baseline was rewritten.

## Round4: exact generic snapshot, unchanged compiler

Verified `core-breadth-round4.tgz` SHA256
`cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28`
and its 22 exact regular-file members (no traversal, links or app-owned paths).
Only these generic files were extracted. Rust/runtime source, native C and the
original Roundhouse binary remain unchanged; binary SHA256
`7c8a5638b91cb08fba78044720915310e5899ad57db7a0eb6035d5e689af392e`.
Prior baseline evidence is retained, not regenerated or suppressed.

| Actual cut | Round4 original / Core / emitted | Change from published baseline |
|---|---|---|
| Public `ApplicationHelper#page_numbers_for_pagination` | **7 / 7 / 7**, strict-clean | Real module now admitted; 1 method, scalar `MAX_PAGES` |
| `StoriesPaginator` optional-positional constructor + accessor cut | **5 / 5 / 5**, strict-clean | Original defaults/arity retained; 3 methods |
| `Search` | 16 / 16 / 16, strict-clean | Unchanged |
| `ShortId` scalar accessor cut | 4 / 4 / 4, strict-clean | Unchanged; not ID generation |
| Full captured real boot → `Search` | 16 / 16 / 16, strict-clean | 1,471 opaque definitions; zero observer failures |
| `ShortId::CandidateId` | 4 / — / — | Now reaches `constant Utils: no proven direct lexical binding` |
| `TimeSeries#get_x_labels` | 3 / — / — | Now reaches actual inherited constructor, then `respond_to?` runtime refusal |
| `ModNote#username` delegate | 3 / — / — | Still refuses `ModNote#raise` runtime |
| `Search#to_url_params` | 2 / — / — | Still refuses `Search#send` runtime |

All five admitted cuts have zero errors/warnings and **actual emitted** equality
without importing Rails. Every original reference passes; all four refusals
leave no Core project. Narrow capture has 163 opaque definitions and zero
observer failures. The full capture sample takes approximately eight seconds.
All 222 pinned source/config/DB/lock identities are unchanged. The separate
emitted HTTP bootstrap failure below remains recorded for every admitted cut.

Controls rerun with the original native observer: CLI **11 tests / 119
assertions**, zero failures/errors/skips; micro **22 / 22 / 22** and Rails
**34 / 34 / 34** original/Core/emitted, with the real forwarding strict refusal
retained. Parameter-signature gaps remain 10/2. Interpreter and YJIT native
observer controls each match baseline/inactive/active, 56 assertions per mode.

The tested Lobsters cuts have no optional-only keyword groups. Their genuine
optional-parameter positive is **positional**, not evidence that the compiler's
known optional-keyword external ABI defect is fixed. No keyword support claim
is made from green Core/strict checks, and no generic compiler workaround was
added here. The runner now fails if **any** materialized lane does not match
strict + original/Core/actual emitted execution, including future ABI failures.

## Published baseline result (preserved)

| Actual cut | Original / Core / actual emitted Ruby | Baseline result |
|---|---|---|
| `Search` constructor, literal accessors, `what`, `page_count`, `persisted?` | 16 / 16 / 16 assertions | 16 methods; strict 0 errors / 0 warnings |
| `ShortId` constructor and scalar accessor cells only | 4 / 4 / 4 | 5 methods; strict 0 errors / 0 warnings |
| Full captured real Rails boot → same `Search` cut | 16 / 16 / 16 | Strict-clean; 1,746 opaque definitions, zero observer failures |
| `ShortId::CandidateId` namespaced accessor / `to_s` cut | 4 / — / — | Only simple named classes admitted |
| Public `ApplicationHelper#page_numbers_for_pagination` module method | 7 / — / — | Only simple named classes admitted |
| `StoriesPaginator#per_page/per_page=` and original optional constructor | 5 / — / — | Only required positional parameters admitted |
| `TimeSeries#get_x_labels` with actual inherited gem helpers | 3 / — / — | `TimeSeries#get_x_values`: inherited runtime outside cut |
| Actual generated `ModNote#username` delegate | 3 / — / — | `ModNote#raise`: inherited runtime outside cut |
| `Search#to_url_params` dynamic call | 2 / — / — | `Search#send`: inherited runtime outside cut |

Every original boots the complete unchanged app in a fresh process. Every
refusal leaves no Core output project; those are exporter refusals, not compiler
diagnostics. Narrow app-model capture has 165 opaque definitions and zero
observer failures. The full captured-boot sample takes about seven seconds;
it is not a performance benchmark or full-app export.

Emitted execution loads **unchanged emitted class files**, without Rails or
original app/generator code. The separate emitted `main.rb` boot attempt fails
on missing `app/controllers/application_controller` for these empty-controller
Core projects. Class-cut success does not erase that HTTP/bootstrap limitation.
The runner preserves both outcomes and hashes emitted class files before/after.

## Source/contract targets for the generic iteration

* `app/models/search.rb:7–55`: own constructor sets empty query, `stories`,
  `newest`, page 1, per-page 20, results `[]`, total -1. Actual literal accessors
  retain independent objects; `what` accepts only `comments`. `page_count` uses
  cap 100 for -1 or >100. Contracts use divisor 7, both sides of 7/98/100, zero,
  and a second object with divisor 33. RBS states the scalar API, not sampled
  types; no `valid?`, search SQL, results/AR proxy or FULLTEXT contract is claimed.
* `app/models/short_id.rb:1–7`: `ShortId#initialize(klass)` assigns `klass` and
  zero attempts through real generated setters. This cut admits only String
  cells and integer attempts. No class registry, `generate`, collision handling
  or storage protocol is called, exported, stubbed or supported.
* `app/models/short_id.rb:21–40`: nested `CandidateId#initialize(klass)` calls
  `generate_id`; that body's actual `Utils.random_str(6).downcase` reaches
  OpenSSL RNG. `valid?` calls `klass.exists?`. The original uses actual RNG then
  asymmetric `id` assignments. Namespace admission alone cannot prove this cut.
* `app/helpers/application_helper.rb:54–88`: public module body uses scalar
  `MAX_PAGES=15`, integer ranges and push/shift/unshift/pop. The interior window
  is **13** integers. Contract covers max 15/16, zero, middle/edge windows and
  `[16,8]` ending `14,"...",16`; no hand-written class wrapper is substituted.
* `app/models/stories_paginator.rb:6–11`: original
  `initialize(scope, page=1, user=nil)` and `STORIES_PER_PAGE=25`. Contract checks
  default/explicit calls, missing/excess args, and independent per-page writes;
  no query scope is replaced. Pagination query execution remains out of scope.
* `lib/time_series.rb:13–14`: `get_x_values.collect { |v|
  Time.at(v).utc.strftime(x_label_format) }`. Actual parent
  `SVG::Graph::TimeSeries#get_x_values` is at gem `TimeSeries.rb:208`, generated
  `x_label_format` at 118, inherited `Graph#initialize(config)` at
  `Graph.rb:104`. Constructor `init_with` uses dynamic `send`; helper reaches
  `x_label_range`/`timescale_divisions`. Original preserves defaults, separate
  graph widths and three daily UTC labels from asymmetric real points 17/29.
* `app/models/mod_note.rb:16`: only app delegate is `username`, to real AR
  `user`. Original contract copies the fixture's **existing** SQLite DB into
  real `:memory:` storage using the benchmark's own `SQLite3::Backup` recipe,
  then switches unsaved actual users `north17`/`south29`. It never calls the
  custom username writer or substitutes association/attribute storage.

The pinned `db/schema.rb` cannot load with this frozen SQLite adapter because
it retains MySQL `:unsigned` options. We recorded that failure, then used the
fixture's real prebuilt DB exactly as `benchmark.rb` does, without schema edits.
`TimeAgoInWords` additionally requires `Time.current` and Rails pluralization;
`IntervalHelper` reads a frozen **Hash** with mutable String descendants,
not a deeply frozen constant admitted by round4.
Neither has been presented as a supported Core contract.

## Reuse

Fetch/extract the pinned archive's `benchmarks/lobsters` without edits. Install
its own bundle with `BUNDLE_FROZEN=true`, disposable `BUNDLE_PATH`, and
`BUNDLE_WITHOUT=development:test`; Bundler honors the lock's 4.0.12 version.
Build the repository-pinned compiler with
`cargo build --locked --jobs 4 --bin roundhouse`. Build `tools/native-observer`
in a separate directory with its `extconf.rb` and `make`. Prism 1.9.0 must be
installed in the host Ruby, not added to the app's Gemfile.

```sh
BUNDLE_PATH=/disposable/lobsters-bundle ruby tools/boot-to-core/lobsters/run.rb \
  /disposable/lobsters "$PWD" /disposable/observer /disposable/new-results
```

For the exact unpacked round4 control, additionally set
`LOBSTERS_GENERIC_SNAPSHOT="$PWD/core-breadth-round4.tgz"`. The runner verifies
the archive hash and every extracted member's bytes, plus unchanged baseline
commit/binary, rather than accepting arbitrary uncommitted exporter changes.

The runner requires `sudo unshare -n`, discards inherited environment/secrets,
uses production's actual `:memory:` DB, and bounds each child to 120 seconds.
It checks 222 pinned source/config/DB/lock identities against a pristine archive
and verifies unchanged bytes afterwards. It accepts clean later compiler
commits for controlled reruns and records their exact hashes; it installs or
writes nothing in the app/gems. Raw command logs stay in the private output
directory. Exit zero requires every admitted cut and every original reference
to pass, with explicit refusals retained—not Rails DSL-family,
whole-app, native-target, cross-target, or fully inferred ABI support.
