# Campfire ordinary-Core probe: round two — 2026-10-10

**Boot observation now completes, but generated attributes/enums still refuse
at root closure. The same three public source leaves strict-check and execute
with identical original/Core/emitted values. No additional DSL-family execution
or whole-app ABI is established.** `RESULTS.md` is the unchanged round-one
report; retain its original archive/evidence rather than interpreting its boot
failures as current results.

## Controlled inputs and owned files

- Compiler: canonical [83b6b1458adde2b2db728507fcabc0c2c0557612](https://github.com/rubys/roundhouse/commit/83b6b1458adde2b2db728507fcabc0c2c0557612),
  binary SHA256 `196121b04b441a9d362a4868bff91b0b8a8727bc480e08f60dbda9383e49175e`.
  Same binary as round one, not newer canonical main. No compiler/runtime edits.
- Unmodified [Campfire 66883b6fb1eda402245247592e0af5f54105c5eb](https://github.com/basecamp/once-campfire/commit/66883b6fb1eda402245247592e0af5f54105c5eb),
  tree `778c439b45e47059acc19142c5896bc0423da570`; **771 tracked files, zero
  differences** after the final run. `.ruby-version` 3.4.10, actual Ruby 3.4.11,
  Bundler 4.0.13, 112 bundled gems, development/test groups excluded.
- Unchanged Gemfile.lock SHA256
  `546716a0008021275f13f7b53555a43d1bf2a294b0791bfeb7c28a57b0f71b3a`;
  Rails 8.2.0.alpha at `e3d5c569d217c56c022b04acb127a8c24dbe9685`.
  Full dependency versions and app/probe/failing-source hashes are in results.json.
- Uploaded round-two archive SHA256
  `9f1fcbc80f1ef1076740980b4ca82288f3800e24820f3b68dec6d948140f95c9`:
  verified before extraction; all eight extracted files still match exactly.
  No local generic exporter or observer changes. Executed capture.rb SHA256
  `557a5f56e5788cce1ae96149635c8da1d5a8ef568e8018339b0193f84a66005d`;
  materialize.rb `89e2e18aeeacb6c4b129a3b55ac383ae6c2005fbe3191661b7a8800d9b066ec6`.
- Native observer built with actual Ruby 3.4.11 in `/tmp/core-observer-round2`;
  executed .so SHA256 `b757c8bfa5266aa58489c7f324d5ba64a9186c9e4481beba97c83b0b1d849b54`.
  This is an executed-binary identity, not a reproducible-build guarantee.
- **Only app-specific source deliverables:** `tools/boot-to-core/campfire/`
  `input.rb` (manifest), `contract.rb` (behavior), `reference.rb` (real Rails
  reference), `probe.rb` (runner), `inventory.rb` (source-site/owner inventory),
  `ROUND2.md` (this report), `RESULTS.md` (unchanged initial report).
  Native source/CLI/compiler are supplied controls, not this probe's PR output.

## Executed comparison and refusal ledger

Every manifest loads `config/application` and dependencies **outside capture**.
All original/manifest subprocesses use an explicit clean allowlist, including
`RUBYLIB=/tmp/core-observer-round2`; Core/emitted processes are fresh standalone
Ruby, not Bundler/Rails, and reject loaded Rails/ActiveRecord. Disposable test
SQLite, artificial secret-key base, disabled telemetry, local test adapters;
no production secrets, workers, outbound messages, sign-in or app-table writes.

| Lane | Initialization/generation placement | Round-two boundary |
|---|---|---|
| whole_boot | Initialize + eager load inside capture; no explicit lazy attribute generation | Boot completes; root `User#name is not public` (lazy getter absent). 1,167 opaque records. Round one aborted in initialize at native Time#to_time alias, **not app-method export**. |
| user_load | Initialize outside; first User load + attributes inside | Generation completes; `User#_read_attribute: inherited runtime outside the declared cut`. 54 opaque. Previously inherited defined_enums wrapper aborted capture. |
| attribute | Initialize/eager load outside; first User attributes inside | Same `_read_attribute` refusal; 3 opaque. Previously _primary_key_definition wrapper aborted generation. |
| attribute_ready | Also schema, primary_key, attribute_types, _default_attributes outside; attributes inside | Same `_read_attribute` refusal; 1 opaque. Previously attribute_aliases wrapper aborted generation. |
| application / initials | Initialize/eager load + User/Room/Message attributes outside | `User::GeneratedAttributeMethods#name`: ambiguous/missing source at active_model/attribute_methods.rb:276. Generated bodies were not captured. |
| enum | Initialize/eager load outside, including enum DSL | `(anonymous module)#active?`: ambiguous/missing source at active_record/enum.rb:308. |
| mention | Initialize/eager load outside | Actual User#attachable_content_type reaches `MENTION_CONTENT_TYPE`; its String is **not frozen**. Refusal: `constant MENTION_CONTENT_TYPE: String is not an immutable scalar`. No app freeze/patch. |
| leaf | Initialize/eager load outside | Same three public source leaves execute successfully; no generated-method claim. |
| whole_attributes (new) | Initialize/eager load + User/Room/Message attributes inside | Root closure reaches `User#valid?: inherited runtime outside the declared cut`; 1,176 opaque. |
| enum_capture (new) | Initialize outside; first User load/enum generation inside | Root active? closure reaches `User#public_send: inherited runtime outside the declared cut`; 51 opaque. |
| platform (new) | Initialize/eager load outside | Read-only iOS/Android/mobile/desktop cut: `ApplicationPlatform#match?: inherited runtime outside the declared cut` (PlatformAgent). |
| sound (new) | Initialize/eager load outside | Read-only name/asset_path/image/text cut requires actual keyword initializer: `only required positional parameters are admitted`. |

These are exact **materializer refusal identities, not strict compiler
diagnostics**. Refused cuts produce no admitted Core input and are not
strict-checked/emitted. Each process stops at its first refusal, so this is
not an exhaustive failure set of all declared roots. results.json preserves
the initial/round-two first-refusal identity arrays separately; no totals-only
comparison or suppression. Full paths/stacks and root owner/source locations
are in sibling stderr logs.

```text
PASS reference: 16 assertions, /up=200; leaf original/Core/emitted: 3 assertions each
roundhouse check --strict .../leaf-core:
0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)
original = Core = actual emitted Ruby:
{"checks":3,"observations":["mentions","users/mention","users/mention"]}
```

Public leaves: **Room#default_involvement**, **User#to_attachable_partial_path**,
**User#to_editor_content_attachment_partial_path**. The mention methods retain
their actual included User::Mentionable owner, projected as an ordinary module.
Parameterless literal return values and public dispatch are preserved; Rails
inheritance/constructor state, mutable string identity, attributes, persistence,
reflection and callbacks are deliberately **not** equivalence claims. Core SHA256
`adb87e1e24595e6c5ec71f1184562d3cd3b94348d937ae86b75ad497c1f919f9` is unchanged.

Fresh original-only additional contracts pass: mention **1**, platform **16**
(iPhone/Android/Macintosh/nil), sound **8** (text/image constructors) assertions.
Their Core/emitted counts are **not run**, not zero-assertion passes. The original
Rails reference separately passes **16**, including /up 200/green markup and
asymmetric initials/title and both enum-state branches.

Opaque records are provenance limitations, not observer exceptions or a count
of unsupported app methods. Whole boot includes two native to_time copy records,
630 unproven installation snapshots, 200 opaque eval definitions and 243 copies
of those definitions; full grouped reasons/sites are retained. The installation
guard refuses either an unmatched method_added snapshot or differing installed
code; this evidence does not distinguish those alternatives. Native collector
failures = **0 in all 13 lanes**. Scalar-constant provenance = **empty in all
lanes**: mention is correctly refused, not a newly successful scalar snapshot.
Unqualified sealed direct lexical scalars do not imply mutable/namespaced,
autoload or namespace-fallback support.

Native contract runs **before app cases**, interpreter and YJIT: original,
inactive and active processes each pass **56 assertions**; active has **18
notifications**. Negative control detects Ruby-frame callback privacy change.
CLI contract initially failed **7 runs/70 assertions/1 failure** under the
Campfire bundle because its fixture requires ActiveModel 8.1.4, while this app
activates 8.2.0.alpha. This is not an observer failure: correct clean standalone
environment with ActiveModel 8.1.4 passes **7 runs/75 assertions/0 failures**.
Both logs are retained; no bundle or fixture version was changed.

## DSL-family inventory, ownership and savings

inventory.rb parses the pinned app with Prism, recording exact sites, lexical
contexts, source hashes and compiler-owner hashes. **347 syntax candidates**
in **14 present buckets** (15 selected). These are not generated-method counts,
DSL execution coverage, or support/removability claims. Dependencies implement
metaprogramming even though direct app ruby_generation sites = 0.

| Family | Sites | Current shared ownership (full paths in inventory JSON) |
|---|---:|---|
| Associations | 26 | ingest/model; lower/model_to_library/associations; ActiveRecord base |
| Scopes | 33 | ingest/model; model_to_library; ActiveRecord relation |
| Enums | 3 | ingest/model; model_to_library/schema; ActiveRecord base |
| Validations | 6 | ingest/model; model_to_library/validations; ActiveRecord base |
| Model callbacks | 12 | ingest/model; model_to_library/markers; ActiveRecord base |
| Secure password/token | 2 | lower/secure_password, secure_token, generates_token_for; bcrypt/token runtime |
| Attributes/delegation | 15 | ingest/delegate, current_attributes, class_attribute; schema/markers |
| Structured JSON | 1 | lower/has_json; Spinel schematized_json and Ruby overlay |
| Storage/rich text | 12 | lower/attached, rich_text; ActiveStorage/ActionText runtime |
| Controller filters/auth | 72 | ingest/controller; controller_to_library; ActionController runtime |
| Browser/rate policy | 2 | ingest/allow_browser, rate_limit; browser_blocker/rate_limiter runtime |
| Cable/jobs | 7 | ingest/channel_callbacks; Spinel Cable + overlay; ActiveJob runtime |
| Routes | 54 | ingest/routes; lower/routes/routes_to_library; ActionDispatch router |
| Schema | 102 | ingest/schema; analyze; model_to_library/schema |
| Direct Ruby generation | 0 | experimental capture/native observer; not production compiler replacement |

**Demonstrated savings: 0 DSL families replaced, 0 compiler/runtime lines
removed.** First generated attribute/enum capture is progress in observation,
not a replacement for inherited runtime, AttributeSet/mutable state, callback
scheduling, route/schema metadata or persistence. Three handwritten leaves do
not retire a DSL handler. This is the Campfire contribution to the parent's
three-app assessment; Writebook/Mastodon were not tested in this orb. Remaining
unproved claims include whole-app ABI, controller/ERB/auth/HTTP equivalence,
associations/jobs/WebSockets, mutable state, full app suite, other targets,
performance/savings, and the Ruby-4 CI app pin.

## Reproduce

Use `RESULTS.md` for the pinned public app checkout/export, clean bundle install,
db:prepare and compiler build commands. Its round-one probe archive retains
the initial scripts. For round two, start the worktree at compiler83b6, verify
the newer archive and extract **only** these intended snapshot entries, then
copy this owned Campfire directory there. No production seed/assets/oracle
benchmark preparation is needed. Existing scripts/campfire-oracle was inspected,
not executed as a benchmark.

```sh
# Working directory: /home/user/workspace/repo
printf '%s  %s\n' 9f1fcbc80f1ef1076740980b4ca82288f3800e24820f3b68dec6d948140f95c9 core-ruby-input-round2.tgz | sha256sum -c -
tar -xzf core-ruby-input-round2.tgz -C /home/user/workspace/campfire-core-probe tools/boot-to-core/capture.rb tools/boot-to-core/materialize.rb tools/native-observer bin/rh tests/rh_materialize_test.rb

# Working directory: /home/user/workspace/campfire-core-probe
source_dir="$PWD/tools/native-observer"
build_dir=/tmp/core-observer-round2
mkdir -p "$build_dir"
(cd "$build_dir" && /home/user/.local/share/mise/installs/ruby/3.4.11/bin/ruby "$source_dir/extconf.rb" && make)
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 RUBYLIB="$build_dir" ruby tools/native-observer/contract.rb
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 RUBYLIB="$build_dir" ruby --yjit tools/native-observer/contract.rb
# Standalone CLI contract needs installed activemodel 8.1.4, NOT Campfire bundle.
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 RUBYLIB="$build_dir" ruby tests/rh_materialize_test.rb
/home/user/.local/share/mise/installs/ruby/3.4.11/bin/ruby tools/boot-to-core/campfire/probe.rb /home/user/workspace/campfire-run /home/user/workspace/campfire-probe-build/bundle /home/user/workspace/campfire-probe-build/round2-final /home/user/workspace/campfire-core-probe/target/debug/roundhouse "$build_dir"
```

Results directory must be new, outside review artifacts. The runner performs
both native modes, inventory, real original contracts, all 13 materializer
lanes, strict-check, emit and standalone comparisons where admitted. Keep the
original `results/results.json` beside round-two output for unchanged app/source
and compiler controls. Review evidence contains reports, source, logs, JSON
and the tiny successful Core/emitted files; no bundles, native/compiler builds,
DB, production credentials or scratch are included. Work is local/uncommitted;
no push, PR, deployment or shared external state change.
