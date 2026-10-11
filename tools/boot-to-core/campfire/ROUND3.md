# Campfire final bounded control — round three, 2026-10-10

**Passed: unchanged three-leaf original/Core/emitted comparison and latest
native/CLI controls. Still refused: mutable mention constant, generated
attributes, generated enum. No whole-app or DSL-replacement claim.** Earlier
results and scripts remain separately preserved; round three does not rerun
whole-app capture or update a baseline.

## Exact controls

- Campfire [66883b6fb1eda402245247592e0af5f54105c5eb](https://github.com/basecamp/once-campfire/commit/66883b6fb1eda402245247592e0af5f54105c5eb),
  tree `778c439b45e47059acc19142c5896bc0423da570`; 771 tracked files unchanged.
  Ruby 3.4.11 against the app's 3.4.10 requirement, Bundler 4.0.13, same 112-gem
  bundle and disposable SQLite. Rails 8.2.0.alpha at
  `e3d5c569d217c56c022b04acb127a8c24dbe9685`; lockfile SHA256
  `546716a0008021275f13f7b53555a43d1bf2a294b0791bfeb7c28a57b0f71b3a`.
- Same canonical compiler [83b6b1458adde2b2db728507fcabc0c2c0557612](https://github.com/rubys/roundhouse/commit/83b6b1458adde2b2db728507fcabc0c2c0557612),
  binary SHA256 `196121b04b441a9d362a4868bff91b0b8a8727bc480e08f60dbda9383e49175e`.
  Neither newer main nor the parent's compiler7a2d was substituted.
- Round-three upload SHA256
  `f213a414bbef19582710e82f7052d9298de478cb8b9cdbff9b3b1101cf2d3127`;
  inspected/verified, only its 20 listed generic files extracted. They still
  match exactly. Generic source is not locally edited or part of owned output.
- Executed capture.rb SHA256
  `bc3db5a5af1c733e0379f7452fab48cf9faa16f262f2b44110c2bdd41248bd36`;
  materialize.rb `e4f5bd01e2f475a6b6f3436d6a97417b3efc35cdbd1b22f6f10cb5d67f21f709`.
  Native extension rebuilt with Ruby 3.4.11 in `/tmp/core-observer-round3`;
  .so SHA256 `bb64fa914adc5ccb8cf029521add4dcabcf5e7da906e99b1ac2cd0b93f4bdf32`.
  Per-source, probe, dependency and failure-path hashes are in results.json.

## Fresh-process evidence

```text
native interpreter AND YJIT:
baseline/inactive/active: 56 assertions each; active: 18 notifications
PASS: collector preserves TERM, Interrupt, SystemExit and throw
PASS: original == native-inactive == native-active
CLI: 9 runs, 99 assertions, 0 failures, 0 errors, 0 skips
PASS reference: 16 assertions, /up=200; leaf original/Core/emitted: 3 assertions each
strict: 0 parse errors, 0 errors, 0 warnings, 0 gap notes, 0 survey gaps
```

The CLI runs standalone, not under Campfire's bundle. ActiveModel 8.1.4 **is
installed and executed**, so there is no missing-fixture skip. The earlier
8.2alpha-vs-8.1.4 bundle-activation failure is retained in round-two evidence.
Native controls also retain the intentionally failing Ruby-frame privacy
negative control; it is not an unexpected suite failure.

| Case | Original | Core / emitted | Exact final boundary |
|---|---:|---|---|
| Room#default_involvement; User#to_attachable_partial_path; User#to_editor_content_attachment_partial_path | 3 assertions | 3 / 3 assertions | All return `["mentions", "users/mention", "users/mention"]`; strict-clean. |
| User#attachable_content_type | 1 assertion | Not run | `constant MENTION_CONTENT_TYPE: String is not an immutable scalar`; real constant frozen? = false. |
| User#name and #name= (`attribute_ready`) | Original Rails reference verifies asymmetric names/initials/title | Not run | `User#_read_attribute: inherited runtime outside the declared cut`. |
| User#active? (`enum_capture`) | Original Rails reference verifies both enum states | Not run | `User#public_send: inherited runtime outside the declared cut`. |

These failures are **materializer refusal identities, not strict compiler
diagnostics**: failed cuts emit no Core project and cannot honestly be counted
as strict-check passes. Collector failures are **0 in every final lane**;
scalar snapshots are **0**. Opaque records: leaf 0, mention 0, attribute_ready 1,
enum_capture 46 (37 unmatched-installation, 6 inherited-copy, 3 opaque-eval).
Round two had 51 enum opaque records: that count change is not support coverage.
Exact first-refusal arrays compare unchanged across rounds two/three.

Leaf Core SHA256 remains
`adb87e1e24595e6c5ec71f1184562d3cd3b94348d937ae86b75ad497c1f919f9`.
Public dispatch/literal values and the projected User::Mentionable module are
preserved. The final inventory explicitly records **Room/User initializer
owner ActiveRecord::Core omitted, receiver instance-variable dependencies []**.
Thus this is not Rails-constructor equivalence: constructor state/side effects,
mutable string identity, reflection, attributes and callbacks are excluded.
Namespace bindings must not have been rebound since definition creation, and
source must remain unchanged; no snapshot of mutable/namespaced/autoload state
is claimed. No constants or app bodies were frozen, copied into stubs or patched.

Round two genuinely advanced whole-boot/first-generation capture into root
export, rather than silently admitting Rails runtime. Initial Time#to_time
refusal happened during initialization, not app-root export. Round three only
confirms the bounded outcomes with stricter observer/exporter controls.

## Inventory, savings and owned delivery

inventory.rb still finds **347 syntactic candidates in 14 present families**,
with exact sites/contexts and hashed compiler ownership: associations 26,
scopes 33, enums 3, validations 6, callbacks 12, password/token 2,
attributes/delegation 15, structured JSON 1, storage/rich text 12,
controller filters/auth 72, browser/rate 2, cable/jobs 7, routes 54, schema 102.
Direct app Ruby-generation sites: 0 (generation in Rails is not absent).
See ROUND2.md and inventory.stdout for ownership, not inferred support counts.
**Demonstrated savings: 0 DSL families replaced, 0 compiler/runtime LOC removed.**
This is only Campfire's contribution to the parent's three-app assessment.
Full app ABI, inherited runtime/mutable state, persistence, auth/controller/ERB
or HTTP equivalence, WebSockets/jobs, other targets, complete app suite,
performance and the Ruby-4 CI pin remain unproved.

Owned files, all under `tools/boot-to-core/campfire/`: **input.rb** (manifest),
**contract.rb** (behavior), **reference.rb** (real Rails reference), **probe.rb**
(final bounded runner), **inventory.rb**, **RESULTS.md** (initial report),
**ROUND2.md**, **ROUND3.md**. Final parent bundle contains only these app-specific
files and compact round-three logs/JSON/tiny successful Ruby outputs. No full
source archive, bundles, databases, compiled binaries or giant generated app.
Earlier scripts/evidence remain separately under review artifacts and scratch
results; not overwritten or regenerated.

## Reproduce in the same orb

Pinned checkout, clean bundle install and disposable db:prepare commands are
in RESULTS.md; no source/dependency changes or production seed required.
From the compiler83b6 worktree, after verifying/extracting the listed round-three
snapshot and copying only the owned app-specific files:

```sh
source_dir="$PWD/tools/native-observer"
build_dir=/tmp/core-observer-round3
mkdir -p "$build_dir"
(cd "$build_dir" && env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 ruby "$source_dir/extconf.rb" && make)
/home/user/.local/share/mise/installs/ruby/3.4.11/bin/ruby tools/boot-to-core/campfire/probe.rb /home/user/workspace/campfire-run /home/user/workspace/campfire-probe-build/bundle /home/user/workspace/campfire-probe-build/round3-final /home/user/workspace/campfire-core-probe/target/debug/roundhouse "$build_dir"
```

Use a **new** results directory. Runner allowlists RUBYLIB and environment in
every original/manifest subprocess, uses no private inherited env, runs native
interpreter/YJIT and standalone CLI first, then real reference, inventory and
four cuts. Dependency/config/application loading is outside capture in all
final cuts. Leaf/mention/attribute_ready initialize and eager-load outside;
enum_capture initializes outside, then captures first User load/enum generation
without prior eager-load. In attribute_ready, schema/primary_key/
attribute_types/_default_attributes are outside capture, first explicit
define_attribute_methods is inside. Nothing else is claimed captured.
Exact argv/environment/exit codes and failure source paths/hashes are recorded.
Keep sibling `results/results.json` and `round2-final/results.json` to check
unchanged compiler/app identities and refusal arrays. These are historical
controls, not rewritten baselines. Local/uncommitted; no publication or deploy.
