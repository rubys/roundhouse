# Campfire breadth probe — round four

**Two newly executable real cuts; one newly executable generated family
(`attr_accessor`), plus inherited pure-Ruby constructor/helper closure. No
generated delegation or whole-app support.** Original, Core and actual emitted
Ruby run independently, with no Rails loaded in either standalone process.

## Decisive fresh-process results

| Cut / public roots | Original | Core | Emitted | Strict |
|---|---:|---:|---:|---|
| Existing Room#default_involvement and User#to_attachable_partial_path / #to_editor_content_attachment_partial_path | 3 | 3 | 3 | Zero errors/warnings |
| Opengraph::Location#url, #url=, #parsed_url= | 16 | 16 | 16 | Zero errors/warnings |
| ApplicationPlatform#ios?, #android?, #mobile?, #desktop? | 25 | 25 | 25 | Zero errors/warnings |

Counts are assertions **per process**, not boot/capture events. All three
strict checks also report zero parse errors, gap notes and survey gaps. The
ordinary Rails reference passes 16 assertions including GET /up = 200.
Original delegation contracts pass Current 12, Filter 8, Presentation 7;
none of those refused cuts has a Core/emitted execution or strict result.

Location preserves its own initialize(url), nil/non-nil mutable String input,
URL storage/setter identity, asymmetric assignments and writer isolation.
Its private parsed_url parser is **omitted**, not claimed equivalent: the
contract checks only that it is not public. URI/DNS/validation/ActiveModel
behavior is outside the cut. No app validation is replaced with a stub.

Platform preserves the actual inherited PlatformAgent initialize (line 4),
private user_agent_string accessor (110) and match? (112), projected into an
ordinary owner module. It checks iPhone/Android/Macintosh/nil/iPad/mixed input
against four predicates and helper privacy. Bodies include
`self.user_agent_string = user_agent_string` and
`user_agent_string.to_s.match?(pattern)`. Owners, parameters, source sites and
hashes are recorded by the original process. UserAgent/browser delegation,
superclass identity/reflection and the full PlatformAgent ABI are excluded.

## Exact remaining first refusals

| Cut | Boundary / movement from round three |
|---|---|
| User#attachable_content_type | `constant MENTION_CONTENT_TYPE: String is not an immutable scalar/collection`; the actual app constant remains mutable. |
| User#name / #name= | `User#caller: inherited runtime outside the declared cut`; now reaches the generated getter's native diagnostic helper, beyond the earlier _read_attribute refusal. |
| User#active? | `User#public_send: inherited runtime outside the declared cut`; unchanged dynamic dispatch boundary. |
| Current request/request=/request_host/request_protocol | `ActiveSupport::CurrentAttributes#defaults: string eval also contains non-instance-method statements (not exported)`; inherited initialization is now attempted, not replaced by seeded @attributes. |
| ActionText::Content::Filter#fragment | `body contains Prism::ConstantPathNode`; generated rescue references ::NoMethodError, ::Kernel and ::ActiveSupport::DelegationError. |
| Messages::AttachmentPresentation#link_to | Same ConstantPathNode refusal in the real generated rescue. |

These are **root-export refusals, not compiler diagnostic identities**; no
failed cut is represented as a strict-clean project. All nine lanes reach
root export and have zero deferred native collector failures. Opaque record
counts: attribute_ready 1, enum_capture 46, delegate_current 7; other lanes 0.
No scalar or frozen-collection snapshots are used by the successful cuts.

NEXT.md retains real declarations, generated bodies, receiver ownership and
independently derived forwarding/exception contracts. Current's real targets
remain ActionDispatch HTTP/Rack; Filter uses ActionText Fragment/Nokogiri;
Presentation uses ActionView NavigationHelper. Admitting qualified exception
constants alone would not prove those receiver graphs standalone. App delegates
are public even after lexical `private`; their target readers are private.
No actual app declaration has `private: true`, so none was invented.

The successful cuts do not contain keyword groups. The parent's known compiler
optional-only keyword ABI bug is **not exercised or fixed here**; all prepared
delegation cases refuse before Core/emission, so no emitted keyword comparison
exists for them. A green strict check alone never counts as execution support.

## Inputs, hashes and controls

- Unmodified Campfire [66883b6fb1eda402245247592e0af5f54105c5eb](https://github.com/basecamp/once-campfire/commit/66883b6fb1eda402245247592e0af5f54105c5eb): all 771 tracked files byte-identical; tree `778c439b45e47059acc19142c5896bc0423da570`.
- Ruby 3.4.11 (app requires 3.4.10), Bundler 4.0.13, unchanged 112-gem bundle, Rails 8.2.0.alpha locked at `e3d5c569d217c56c022b04acb127a8c24dbe9685`, platform_agent 1.0.1. Lock SHA256 `546716a0008021275f13f7b53555a43d1bf2a294b0791bfeb7c28a57b0f71b3a`.
- Unchanged canonical compiler [83b6b1458adde2b2db728507fcabc0c2c0557612](https://github.com/rubys/roundhouse/commit/83b6b1458adde2b2db728507fcabc0c2c0557612), binary SHA256 `196121b04b441a9d362a4868bff91b0b8a8727bc480e08f60dbda9383e49175e`. No newer compiler substituted.
- Round-four archive SHA256 `cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28`: all 22 regular-file member paths inspected, extracted only to isolated worktree, all still byte-identical. No Rust/runtime/Cargo changes or local generic edits.
- Executed capture.rb SHA256 `dcbf2c0610c6271692266387b97a5e74e9ea0ce9e69070120d377f13a5790a6b`; materialize.rb `34f9b72abad9c762113311f02d3cc67af6cd02a611499d2247fa49bfc380a05b`.
- Actual-Ruby native extension from round three reused because C/extconf are unchanged: .so SHA256 `bb64fa914adc5ccb8cf029521add4dcabcf5e7da906e99b1ac2cd0b93f4bdf32`.
- Core source SHA256s: leaf `adb87e1e24595e6c5ec71f1184562d3cd3b94348d937ae86b75ad497c1f919f9`; Location `88fb0a564587b0c6fd035238559fec19bfea50b1845c7a42a5ab07609bc483af`; Platform `4b46e81f1bcc437b788a6dce449029739f896a9770cd2c6291ac8b20de7ce764`.

CLI: **11 tests, 119 assertions, 0 failures/errors/skips**. Standalone
ActiveModel 8.1.4 is installed/executed; Campfire itself uses its locked Rails
bundle. Native baseline/inactive/active: **56 assertions each**, interpreter
and genuine YJIT, active 18 notifications; TERM/Interrupt/SystemExit/throw pass.
An intentionally failing Ruby-wrapper privacy negative control is retained.

**YJIT evidence correction:** the generic contract starts child Rubies without
inheriting a parent-only `--yjit` argument. Earlier logs labelled YJIT actually
show child `yjit: false` and must not be treated as YJIT proof. Only the app
runner changed: native_contract_yjit now explicitly allowlists
`RUBYOPT=--yjit`; the final child runtimes say `+YJIT`, `yjit: true`. All app
cases were rerun afterward. Earlier logs/reports remain unchanged separately.

## Reproduce / deliver

RESULTS.md contains exact pinned checkout, bundle install and disposable
db:prepare commands; ROUND3.md contains the actual-Ruby extension build.
After verifying round-four archive/member paths and extracting its 22 generic
files into compiler83b6, use a new results directory:

```sh
# Working directory: /home/user/workspace/campfire-core-probe
/home/user/.local/share/mise/installs/ruby/3.4.11/bin/ruby tools/boot-to-core/campfire/probe.rb /home/user/workspace/campfire-run /home/user/workspace/campfire-probe-build/bundle /home/user/workspace/campfire-probe-build/round4-confirmed
# Exact native YJIT control (runner supplies the same clean env):
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 RUBYLIB=/tmp/core-observer-round3 RUBYOPT=--yjit ruby tools/native-observer/contract.rb
```

Every original/manifest subprocess allowlists RUBYLIB and env, inherits no
private env, uses a fake secret-key base and disposable SQLite; no workers,
listeners, DB writes in contracts or production secrets. No Rails import in
standalone output. Dependency/config/application loads outside capture.
Location/Platform/leaf/mention ordinarily initialize and eager-load outside;
Current/Presentation capture first class load after ordinary initialization;
Filter preloads real ActionText dependencies outside, captures first extension
require, then initializes outside (its premature-load warning is retained).
AR schema/default preparation is outside capture, first explicit attributes
inside; enum captures first User load after ordinary initialization.
No full-app capture was repeated in this round.

Owned files under tools/boot-to-core/campfire/: input.rb (manifest), contract.rb
(prior controls), next_contract.rb (breadth contracts), reference.rb, probe.rb
(runner), inventory.rb, RESULTS.md, ROUND2.md, ROUND3.md, NEXT.md, ROUND4.md.
Compact package includes these, final logs/JSON/provenance and tiny Core/emitted
model files; no generic source, full source archives, gems, DB or giant runtimes.
Initial/round2/round3/next-readiness evidence stays separate and unchanged.

Inventory remains 347 syntax candidates in 14 present buckets with source
sites and compiler/runtime ownership; syntax counts are not support counts.
**Savings: one newly executable accessor-generation family, zero executable
delegation families, zero production compiler/runtime LOC removed.** Platform
is an additional useful inherited-source closure, not a second DSL family.
This is Campfire's contribution only, not independent testing of the other two
apps. Full app ABI, AR attributes/enums/persistence, controller/HTTP/ERB,
jobs/WebSockets, other targets, full app suite and performance remain unproved.
All work is local/uncommitted; no push, publication or deployment.

## Parent-final hard-gate rerun

The uploaded campfire-probe-parent-final.rb differs from the executed runner
by exactly two lines after the lane loop: every materialized cut must have
strict three-way verification, otherwise the runner fails. The owned runner
now matches that upload byte-for-byte; SHA256
`88f8a81522c8ea619439712c5bd7a94d4286d38ab4d0cd33b6c0e61b8e969b83`.

Reran the same command with output directory
`/home/user/workspace/campfire-probe-build/round4-gated`: exit 0; Location
16/16/16, Platform 25/25/25, leaf 3/3/3, all strict-clean. The six refusal
arrays, Core hashes, app/dependency/generic/compiler identities and original
behavioral results are unchanged. CLI again 11 tests/119 assertions, zero
failures/errors/skips; all native child modes confirm interpreter false and
YJIT true respectively. Zero observer failures in every lane.

An inline validation of the exact two gate lines accepts verified/refused
cuts and rejects materialized cuts with false or absent verification. No
generic/compiler/app/dependency edits were made. The new gated bundle contains
fresh results.json and sibling logs; previous round4-confirmed receipts and
the previous bundle remain unchanged separately. Parent broad Rust checks are
not part of this orb's evidence or a claimed result.
