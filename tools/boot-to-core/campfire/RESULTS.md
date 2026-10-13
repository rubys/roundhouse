# Campfire ordinary-Core probe — 2026-10-10

**Result: real Campfire boots; a three-method source cut strict-checks and
matches original/Core/emitted Ruby. Whole initialization capture and the tested
ActiveRecord/generated roots remain blocked. This is not whole-app support.**

## Exact inputs

- Roundhouse: canonical `rubys/roundhouse:main` commit
  `83b6b1458adde2b2db728507fcabc0c2c0557612`, in an isolated worktree.
  Canonical main was refreshed; its later tip was not substituted for this pin.
  No compiler/runtime changes; the parent checkout and its uploaded archive
  were preserved. No push, PR, deployment, or external write.
- Materializer upload SHA256:
  `0607454626deed7338d3403684ba92c9294429bd3f80437543bb972f3275bc26`.
  Verified before extraction. `capture.rb`, `materialize.rb`, and `bin/rh`
  remain byte-identical to that upload.
- Campfire: `basecamp/once-campfire` commit
  `66883b6fb1eda402245247592e0af5f54105c5eb`, tree
  `778c439b45e47059acc19142c5896bc0423da570`, immediately before its Ruby 4
  upgrade. Its `.ruby-version` is **3.4.10**; execution used installed
  **Ruby 3.4.11**. All **771 tracked app files** were compared byte-for-byte
  with the pinned checkout after setup: **zero differences**.
- The current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` was inspected
  (`.ruby-version` = 4.0.7), not executed. Installing another Ruby was avoided.
- Campfire lockfile SHA256:
  `546716a0008021275f13f7b53555a43d1bf2a294b0791bfeb7c28a57b0f71b3a`.
  Bundler **4.0.13**, **112 gems**, development/test groups excluded;
  **no lockfile changes**, including no ffi workaround.
- Rails **8.2.0.alpha**, actual locked Git revision
  `e3d5c569d217c56c022b04acb127a8c24dbe9685`.
  Other Git dependencies: surfguard `59e278c01a537755f22791429051891949231ead`,
  turbo-rails `30cd8fcc6f82c1ad4edd1ed6069ba878f21f02b3`,
  importmap-rails `51c1a531327fc04ed4552bb0fd523eb43561b817`,
  propshaft `dc979db89cd07c72ee4d11d415ae1cb4fd072623`.
  Notable loaded dependencies: Prism 1.9.0, sqlite3 2.9.6, Rack 3.2.7,
  Puma 7.2.1, lexxy 0.9.24, ffi 1.17.2, ruby-vips 2.2.5,
  redis 5.4.1, resque 2.7.0. Full loaded versions are in `results.json`.
- Compiler built locally with Rust **1.98.1** and `cargo build --locked --jobs 4
  --bin roundhouse --bin dump_ir`. Compiler SHA256:
  `196121b04b441a9d362a4868bff91b0b8a8727bc480e08f60dbda9383e49175e`.
  An existing `unused_assignments` build warning was retained.

## Safety and reference

Every test subprocess has an **allowlisted environment** (`unsetenv_others`),
`RAILS_ENV=test`, telemetry disabled, a deliberately artificial secret-key base,
and an explicit disposable SQLite URL. No production credentials or encrypted
credentials files were supplied. Test ActiveStorage is local disk, caching is
null, and ActionCable uses the app's test adapter. Redis points to unused local
port 6399; no Redis or job workers were started. Only unsaved model objects and
the health request are exercised. No production database, listener, sign-in,
outbound message, or app-table write was needed. Bundling requires public
dependency downloads; this is not a network-isolation claim.

The original Rails process passed **16 behavioral assertions**: actual Rack
`GET /up` returns 200 and green health markup; asymmetric name/bio cases verify
`User#initials` and `#title`; enum mutation checks both active/deactivated
branches; the notification/mention selectors and mention content type match
their literal expected values. This is an original-app reference, not an
emitted-app HTTP or full test-suite pass.

## Capture/export ledger

Dependencies (`config/application`, Rails/all, Bundler dependencies) load
outside capture in every lane. Each lane runs in a **fresh process**.

| Lane | Additional work outside capture | Outcome and exact boundary |
|---|---|---|
| `whole_boot` | None | Fails **during `Rails.application.initialize!` capture**, before root export: native `Time#to_time` alias has no readable/captured source. Stack: `active_support/core_ext/time/compatibility.rb:7` → `core_ext/module/redefine_method.rb:12` → exporter `source_record`. Eager load/root export never reached. |
| `user_load` | Ordinary initialize only | Capture of first `User` load fails in inherited `defined_enums` class-attribute redefinition: `active_record/enum.rb:292` → `active_support/class_attribute.rb:28`. Attribute generation/export never reached. |
| `attribute` | Ordinary initialize + eager load | First `User.define_attribute_methods` capture (`ATTRIBUTE_BEFORE false`) fails at inherited `_primary_key_definition` wrapper: `active_record/attribute_methods/primary_key.rb:129` → `active_support/class_attribute.rb:28`. |
| `attribute_ready` | Also `User.columns`, `primary_key`, `attribute_types`, `_default_attributes` | First attribute generation still reports false before capture. It now reaches generation of the name getter/setter, then fails at `alias_attribute :id_value, :id` (`active_record/attribute_methods.rb:115`), through inherited `attribute_aliases` wrapper (`active_model/attribute_methods.rb:213`). No partial failed boot was exported. |
| `application` | Initialize, eager load, and User/Room/Message attribute generation | Root export fails at `User#name`: generated body lacks captured provenance (`active_model/attribute_methods.rb:276`). Declared room/message/controller roots are not established as supported by this first failure. |
| `initials` | Same attribute generation outside capture | `User#initials` (`app/models/user.rb:27`) closure follows `name` and hits the same generated-source provenance boundary. This is no longer a lazy, not-yet-defined-getter failure. |
| `enum` | Initialize + eager load, including enum DSL generation | `User#active?` generated Proc lacks capture: `active_record/enum.rb:308`; source-only recovery cannot find an ordinary DefNode. |
| `mention` | Initialize + eager load | `User#attachable_content_type` (`app/models/user/mentionable.rb:6`) export rejects `MENTION_CONTENT_TYPE` as `Prism::ConstantReadNode`. The constant was not inlined or hidden. |
| `leaf` | Initialize + eager load | Three real, unchanged source bodies export, strict-check, emit and execute. Capture events = 0; no generated body is claimed. |

The useful but deliberately small positive cut is `Room#default_involvement`
(notification policy), `User#to_attachable_partial_path`, and
`User#to_editor_content_attachment_partial_path` (mention rendering selectors).
The latter two retain their actual included `User::Mentionable` owner projected
as an ordinary module, rather than moving their bodies into the class.

```text
roundhouse check --strict .../leaf-core
0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)

original = Core = actual emitted Ruby:
{"checks":3,"observations":["mentions","users/mention","users/mention"]}
```

Core and emitted contracts execute in fresh **non-Bundler Ruby processes** and
reject loaded Rails/ActiveRecord. No RBS, stub getters, class-body patches,
generator rewrites, compiler edits, or diagnostic suppression were used.
These are parameterless/state-independent leaves: no argument-boundary or
mutable-heap inference is justified. Core SHA256:
`adb87e1e24595e6c5ec71f1184562d3cd3b94348d937ae86b75ad497c1f919f9`.

## Reproduce in this orb

`scripts/campfire-oracle` was inspected before setup; its production benchmark
preparation, Redis coupling, asset precompilation and seed are not this test
lane. The following is the clean-environment setup used here (paths are outside
review artifacts). Run installation/preparation from `campfire-run` and the
probe from the isolated Roundhouse worktree:

```sh
# Working directory: /home/user/workspace/repo; initial setup in a fresh orb
git fetch --quiet https://github.com/rubys/roundhouse.git main
git worktree add -b thomasklemm/campfire-core-probe /home/user/workspace/campfire-core-probe 83b6b1458adde2b2db728507fcabc0c2c0557612
printf '%s  %s\n' 0607454626deed7338d3403684ba92c9294429bd3f80437543bb972f3275bc26 core-ruby-input-source.tgz | sha256sum -c -
tar -xzf core-ruby-input-source.tgz -C /home/user/workspace/campfire-core-probe
# Copy this probe's tools/boot-to-core/campfire/ directory into that worktree.
git clone --filter=blob:none https://github.com/basecamp/once-campfire.git /home/user/workspace/campfire-source
git -C /home/user/workspace/campfire-source checkout --detach 66883b6fb1eda402245247592e0af5f54105c5eb
mkdir -p /home/user/workspace/campfire-run /home/user/workspace/campfire-probe-build
git -C /home/user/workspace/campfire-source archive HEAD | tar -x -C /home/user/workspace/campfire-run

# Working directory: /home/user/workspace/campfire-run
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 BUNDLE_PATH=/home/user/workspace/campfire-probe-build/bundle BUNDLE_WITHOUT=development:test BUNDLE_JOBS=4 bundle install
diff -u /home/user/workspace/campfire-source/Gemfile.lock Gemfile.lock
env -i HOME=/home/user PATH=/home/user/.local/share/mise/installs/ruby/3.4.11/bin:/home/user/.local/bin:/usr/bin:/bin LANG=C.UTF-8 BUNDLE_PATH=/home/user/workspace/campfire-probe-build/bundle BUNDLE_WITHOUT=development:test RAILS_ENV=test SKIP_TELEMETRY=1 SECRET_KEY_BASE=disposable-campfire-core-probe-not-production REDIS_URL=redis://127.0.0.1:6399 DATABASE_URL=sqlite3:/home/user/workspace/campfire-probe-build/probe.sqlite3 bundle exec ruby bin/rails db:prepare

# Working directory: /home/user/workspace/campfire-core-probe
cargo build --locked --jobs 4 --bin roundhouse --bin dump_ir
/home/user/.local/share/mise/installs/ruby/3.4.11/bin/ruby tools/boot-to-core/campfire/probe.rb /home/user/workspace/campfire-run /home/user/workspace/campfire-probe-build/bundle /home/user/workspace/campfire-probe-build/results
```

Results directory must be new; a repeat can use a different name beside the
same disposable DB. `probe.rb` writes exact argv, per-command environment
overrides, exit statuses, stdout/stderr, root source sites, full failure stacks,
loaded versions, and source SHA256s into `results.json` and sibling logs.
An initial install failed because the sanitized PATH omitted `mise`, which
the RubyGems reshim plugin invokes; adding only `~/.local/bin` fixed setup.
An initial direct-Ruby reference activated default JSON before Bundler;
running original/manifest processes through `bundle exec` fixed the activation
order without changing dependencies. Core/emitted processes remain standalone.

## Claims deliberately unproved

No whole Campfire materialization, mutable ActiveRecord AttributeSet snapshot,
generated attribute/enum ABI, full validation/callback scheduling, database
persistence, associations, auth/controller/ERB equivalence, WebSockets, jobs,
push delivery, complete Campfire suite, production deployment, benchmarks,
other compiler targets, or execution of the Ruby-4 CI pin is proved. The
positive leaves do not increase generated-DSL coverage. All failed cuts remain
reported unsupported; none was fed to the compiler as if materialized.
