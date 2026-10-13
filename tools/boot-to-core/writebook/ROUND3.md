# Writebook round three: safe Current refusal, unchanged QR execution

**Verified on the real locked app with the unchanged compiler83b6 control.**
The earlier Current false-positive materialization is now refused before output.
QR still passes original/Core/actual emitted Ruby execution. The representative
AR attribute cut remains unsupported. This is an admission-correctness improvement,
not added Rails-generator support or demonstrated lowering-code savings.

## Controlled identities

| Input | Exact identity |
|---|---|
| Compiler | [rubys/roundhouse initial commit](https://github.com/rubys/roundhouse/commit/83b6b1458adde2b2db728507fcabc0c2c0557612) |
| Writebook | [basecamp/writebook pin](https://github.com/basecamp/writebook/commit/f3fadd21907ad9b18cb23800d971c2cc25045e2a) |
| Rails | locked Git `3a4961048ad251b50991ae83135d760a8a9e8ae3`, 8.2.0.alpha |
| Ruby | MRI 3.4.7, revision `7a5688e2a2`, Prism 1.9.0 |
| Supplied round-three archive SHA256 | `f213a414bbef19582710e82f7052d9298de478cb8b9cdbff9b3b1101cf2d3127` |
| Unchanged compiler binary SHA256 | `80515e3a6837da1fd1195e7928360891b91bb3328b617958617d0ad01920e252` |
| Rebuilt observer binary SHA256 | `62da664d927706f24f9eb4c95a6fe762119f726bcf524ae827939413ce5744a1` |

The archive was verified before extraction. Only its 20 listed generic files were
extracted in the isolated compiler worktree; every file still matches the upload.
No production compiler/runtime differences, app patches, dependency substitutions,
baseline updates, commits or publication. Canonical main was refreshed without
changing the controlled compiler or using fork origin/main as the base.

Private delivery evidence `evidence/round3/audit.json` compares all 490 original app files:
zero changed/missing files. Gemfile, lock, schema, Ruby and loaded QR gem versions
match round two. The actual app contracts, manifest and provenance reporter are
unchanged. The runner only adds explicit bounded lane selection.

## Same contracts, fresh processes and databases

| Actual app cut | Original assertions | Round two | Round three |
|---|---:|---|---|
| `QrCodeLink#url` + own initializer | 4 | original/Core/emitted pass | same results in both focused and unobserved controls; strict 0 errors/0 warnings |
| `Current#user`, `user=` | 5 | strict clean; Core and emitted fail on nil `@attributes` | focused materializer exits 1, names omitted inherited initializer and state; no output directory |
| `Section#body`, `body=`, `markable`, `searchable_content` | 5 | focused refuses `_read_attribute` | same inherited-runtime refusal; no output directory |

All **14 original assertions**, including asymmetric instances, Unicode, empty
strings and nil reassignment, match round two exactly. QR's Core source and emitted
app-method source are also byte-identical. No Rails/app/bundle/observer is loaded
in standalone Core/emitted contract processes. The QR positive cut still excludes
signing/verifiers/secrets, HTTP, database and request state, QR rendering and
non-String inputs; it exercises an ordinary literal accessor, not a Rails generator.

Current's decisive refusal is:

```text
rh materialize: BootToCore::Unsupported: Current: inherited initializer ActiveSupport::CurrentAttributes#initialize omitted; receiver state dependency @attributes
```

The round-two Core/emitted nil-state failure, its sources and logs remain intact
under `evidence/current.round2.*` and `evidence/decisive-round2.log`. Round three
creates neither the focused nor unobserved Current source directory. It cannot
be mistaken for a clean check: no Current Core check/emission/execution runs.

Unobserved Current and Section controls still refuse their generated-source
provenance at the same locked framework locations. Focused Current metadata
retains 20 records/12 opaque; focused Section retains 256/21. Every attempted cut
has zero deferred collector failures and zero scalar events. Opaque reasons remain
recorded, not admitted. Their identities/multiplicities and output-existence checks
are in private `evidence/round3/comparison.json` and `evidence/round3/run.json`.

The unobserved boot/eager-load gate passes. The runner also repeats the strict
app diagnostic gate: **29 errors/258 warnings**, with the exact identity/multiplicity
multiset unchanged, zero added/removed occurrences. Full observed capture and the
other application lanes were deliberately not repeated in this bounded round.

## Native and CLI controls

Both interpreter and YJIT native runs pass **56 assertions in each baseline,
inactive and active mode**, with 18 active notifications. The Ruby-frame privacy
negative control is detected. Additional public controls verify actual TERM
termination and preservation of Interrupt/SystemExit exception identity and
nonlocal `throw`, including installed bodies and no inappropriate deferral.
Private logs: `evidence/round3/native.log`, `evidence/round3/native-yjit.log`.

The **same latest contract against the retained round-two observer binary**
(`f28be5efe144d0d24d619fc6801651e9af8623563221123f9d8e03f441ab3d75`)
still passes the 56 ordinary comparisons but exits 1 with
`collector swallowed termination: TERM swallowed`, under both interpreter and
YJIT. The round-three extension passes that exact contract. See the
private `evidence/round3/native-round2-control.log`,
`evidence/round3/native-round2-yjit-control.log` and
`evidence/round3/native-control.json`. Only `RUBYLIB`
differs within each before/after pair; app dependencies and source are untouched.

Latest CLI suite: **9 tests, 93 assertions, 0 failures, 0 errors, 1 skip**.
The reentrant alias, omitted-initializer and scalar/refusal tests execute. The
legacy fixture explicitly needs ActiveModel/ActiveSupport 8.1.4, which is absent
from Writebook's unchanged Rails 8.2 Git lock. No alternate gems were installed;
the parent's 99-assertion fixture result is not claimed here.
Private logs: `evidence/round3/cli.log`, `evidence/round3/decisive.log`.

## Reproduce this bounded round

With the verified snapshot extracted into the exact compiler worktree:

```sh
RUBY="$HOME/.local/share/mise/installs/ruby/3.4.7/bin/ruby"
build_dir=/tmp/core-observer-round3
mkdir -p "$build_dir"
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  sh -c 'cd /tmp/core-observer-round3 && ruby /tmp/writebook-probe-compiler/tools/native-observer/extconf.rb && make'
# Native and CLI commands run from the compiler worktree:
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" "$RUBY" tools/native-observer/contract.rb
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" RUBYOPT=--yjit "$RUBY" tools/native-observer/contract.rb
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" BUNDLE_GEMFILE=/tmp/writebook-probe-app/Gemfile \
  BUNDLE_PATH=/tmp/writebook-gems BUNDLE_FROZEN=true \
  "$RUBY" -rbundler/setup tests/rh_materialize_test.rb --verbose
# App probe runs from this source checkout; use a NEW output path:
WRITEBOOK_LANES='qr current attributes' "$RUBY" tools/boot-to-core/writebook/run.rb \
  /tmp/writebook-probe-compiler /tmp/writebook-probe-app /tmp/writebook-round3-app \
  /tmp/writebook-gems "$build_dir"
```

Original/manifest subprocesses explicitly allowlist `RUBYLIB` with
`unsetenv_others: true`; no private environment is propagated. The run ledger
contains exact commands, allowed environments, exits and probe hashes. Exit 0
means the experiment was recorded, not that unsupported cuts passed.

The [141-site/13-family inventory and ownership/savings assessment](RESULTS.md#app-generation-inventory--compiler-responsibilities)
remain applicable: **zero framework responsibilities proved replaceable**.
Current construction/defaults/thread-local state/forwarding/reset and AR schema,
SQL/relations, associations, callbacks and native/mutable state remain necessary.
No additional scalar coverage, performance benchmark, native target execution,
full app ABI or cross-target support is claimed.

Round-three evidence is separate under `evidence/round3`. Earlier evidence is
unchanged and hash-ledgered. The compact delivery overlays owned sources/reports
and round-three evidence only; it excludes immutable source archives and generated
projects. All work is local/uncommitted. The exact app, bundle, compiler, native
build and all three full output directories remain available in this orb.
