# Ractor-unsafety ledger: campfire's CRuby emit

**Snapshot 2026-10-08.** Roundhouse 194f26cf, once-campfire 90b3300
(2026-09-26), Ruby 4.0.5 arm64-darwin. A dated measurement, not a status
page: re-run the probes before quoting it.

This ledger answers one question. What stands between the emitted Ruby
campfire and the deployment shape of Tobi's
[campfire-once-ruby-ractor](https://github.com/tobi/campfire-once-ruby-ractor):
boot once in the main Ractor, deep-freeze what can be frozen, then serve
requests from N worker Ractors. Each entry below is something a worker
Ractor **raises on**, with the evidence that it is actually reached.
Entries that a boot-time freeze clears are not listed as blockers.

## How it was measured

Two probes, both run inside the emitted tree:

```sh
roundhouse --target ruby ~/git/once-campfire -o EMIT --allow-unsupported
cd EMIT && bundle install
cp <seeded campfire DB> storage/development.sqlite3   # user1@example.com / secret123456
bundle exec ruby scripts/ractor-probe.rb all OUT.json    # path relative to this repo
bundle exec ruby scripts/ractor-probe-calls.rb
```

- [`scripts/ractor-probe.rb`](../scripts/ractor-probe.rb) boots the emit
  the way `config.ru` does and signs in. It serves the seven dynamic
  bench routes in the main Ractor (all 200), then:
  - traces which third-party methods each route reaches. The room page
    is also traced **cold**: a warm render hides the rich-text path
    behind fragment caches, and under Ractors every worker starts cold.
  - counts the constants and module ivars a worker could not read.
  - serves each route inside a worker Ractor.
- [`scripts/ractor-probe-calls.rb`](../scripts/ractor-probe-calls.rb)
  freezes every freezable constant, then makes one representative call
  per library in a fresh worker Ractor. A FAIL is therefore not an
  unfrozen constant. It is a C extension that doesn't declare Ractor
  safety, a mutable registry, a lock, or a Proc.

The full route walk stops at the first wall, which is our own `Db` pool
on every route (see [Ours](#ours-roundhouse-runtime-and-emit)). The
third-party entries therefore rest on the trace plus isolated calls.
They do not come from one end-to-end Ractor serve. That end-to-end serve
is the closing check, once our own walls are down.

## Summary: blockers on the bench routes

| # | Blocker | Kind | Routes that reach it | Owner |
|---|---|---|---|---|
| 1 | **sqlite3** 2.9.3 | C ext not declared Ractor-safe; module-level `ForkSafety` registry | every route that touches the DB (all but `/up`) | sqlite3-ruby (Mike Dalessio) |
| 2 | **nokogiri** 1.19.2, and through it **loofah** 2.25.1 and **rails-html-sanitizer** 1.7.0 | C ext not declared Ractor-safe | cold room page, messages page, POST message (rich-text sanitize) | Mike Dalessio (all three) |
| 3 | **connection_pool** 3.0.2, via **net-http-persistent** 4.0.8 | global `INSTANCES` WeakMap mutated in `#initialize` | POST message (first one builds the Web Push pool) | connection_pool (Mike Perham); net-http-persistent |
| 4 | **openssl** 4.0.2 `OpenSSL::Digest::SHA256.new` | subclass `#initialize` is a `define_method` lambda (unshareable Proc) | sign-in page, cold room page (key derivation) | ruby/openssl; **ours to route around** |
| 5 | **our runtime**: `Db`, caches, lazy loads | module-level mutable state | every route | us |

Off the bench routes but on campfire's paths (verified by isolated call
unless noted):

| # | Blocker | Kind | Path | Owner |
|---|---|---|---|---|
| 6 | **resolv** 0.8.0 `Resolv::DefaultResolver` | lazily built mutable resolver; freezing it breaks the **main** Ractor too (`FrozenError`) | outbound HTTP through our SSRF guard (`runtime/surfguard.rb`): unfurl, bot webhooks, Web Push | Ruby core (default gem). Tobi hit the same wall. |
| 7 | **websocket-driver** 0.8.0 `WebSocket::Mask.mask` | C ext not declared Ractor-safe | Action Cable frames, if a worker Ractor owns sockets | faye / websocket-driver |
| 8 | **sentry-ruby** 7.1.0 | `Sentry::MUTEX`, `Sentry::Hub::MUTEX` constants | error reporting (`Sentry.get_main_hub`) | getsentry |
| 9 | **ruby-vips** 2.3.0 | Procs in constants (`GLib::LOG_HANDLER`, `Vips::MARSHAL_*`), FFI struct layouts in class ivars | image variants (job path). **Static only, not called in a Ractor.** | libvips / ruby-vips |

## Third-party detail

**1. sqlite3.** `SQLite3::Database#open_v2` raises `Ractor::UnsafeError`
in a worker. The extension never calls `rb_ext_ractor_safe(true)`, and
that's still true in 2.9.6, the newest installed. Every DB route goes
through `Database#execute`/`#prepare` and the C methods on `Statement`
(`prepare`, `step`, `column_*`). This is the central blocker. Tobi's port
switched to **extralite**, whose `Init` declares Ractor safety, and still
had to patch it (a double finalize of FTS5 statements on close).

Past the declaration there is a second wall in Ruby code:
`SQLite3::ForkSafety.track` appends a `WeakRef` to every opened database
onto the module-level `@databases` array (`lib/sqlite3/fork_safety.rb`).

*Ask:* declare the C extension Ractor-safe after auditing its global
state, and make `ForkSafety` tracking work outside the main Ractor
(skip it there, or keep it per Ractor). A database handle would stay
owned by one Ractor; nothing here asks for sharing a connection.

**2. nokogiri → loofah → rails-html-sanitizer.** HTML5 (gumbo), HTML4,
and XML parsing all raise `Ractor::UnsafeError` in a worker, and Loofah
and the safe-list sanitizer fail through it. Not declared in 1.19.4,
the newest installed, either. Reached through
`ActionText::ContentHelper.sanitizer` on every message body that isn't
already cached. Under Ractors every worker starts cold, and new messages
always miss the cache. Tobi's port rewrote the rich-text pipeline in
pure Ruby to avoid it.

*Ask:* Nokogiri Ractor safety, at least for parse → traverse → serialize
of a document owned by one Ractor. That covers everything Loofah and
the sanitizer need. *Our fallback,* which wouldn't keep the real gem:
the pure-Ruby safe-list port the Spinel lane already uses
(`runtime/rails_html_sanitizer_spinel.rb`). The ledger records this as
a fallback, not as coverage.

**3. connection_pool via net-http-persistent.** `ConnectionPool#initialize`
writes `self` into `ConnectionPool::INSTANCES`, a process-wide
`ObjectSpace::WeakMap` kept so pools can be reloaded after fork
(`auto_reload_after_fork`, default true). Any pool built in a worker
raises. net-http-persistent builds one and doesn't pass that option
through. Campfire's `WebPush::Pool` builds its `Net::HTTP::Persistent`
on the first POST.

*Ask:* skip or localize the fork registry outside the main Ractor. Fork
and Ractors don't mix anyway.

**4. openssl `Digest::SHA256`.** `lib/openssl/digest.rb` defines each
named digest class with
`define_method(:initialize, ->(data = nil) { super(name, data) })`. The
lambda isn't shareable, so `.new` raises "defined with an un-shareable
Proc in a different Ractor". `OpenSSL::Digest.new("SHA256")`,
`OpenSSL::Digest.hexdigest`, HMAC, PBKDF2, and EC all work. Ours to fix
in `runtime/message_digest.rb:82`. The upstream report is still worth
filing.

## Ruby and stdlib

- **resolv** (#6 above).
- Fine in a worker, though a symbol-table check (`nm`) suggested
  otherwise: `digest/md5`, `digest/sha1`, `io/wait`, encoding transcode
  tables, `zlib`, `json`, `bcrypt`, `securerandom`, `base64`,
  `Addrinfo.getaddrinfo`, and `Net::HTTP` against a local server. Check
  any extension by calling it; the symbol check gives false alarms.
- Rack 3.2.6 is fine once eager-loaded and frozen (`Rack::Files`
  serves). Its constants are autoloaded, so the boot freeze must force
  them first. Only `Rack::MockRequest` (a test helper) keeps a
  module-level parser.

## Ours: roundhouse runtime and emit

Every route stops first at `Db.@pool`. The census lists 19 module
ivars in `runtime/`, 2 in `cable.rb`, and 1 in `main.rb` that a worker
can't read. Our constants account for 299 unshareable values, 293 of
which a boot freeze fixes. The work splits into five kinds:

| Kind | Instances | What the emit would do |
|---|---|---|
| Process-wide pool and locks | `Db.@pool/@mutex/@cv/@permit_lock/@permit_cv/@quarantined`, `ActiveJob.@drain_mutex` and `QUEUE_LOCK`, `GzipCache.@mutex/@pieces_mutex` | Per-Ractor DB pool. SQLite writes serialized by a writer Ractor or by `BEGIN IMMEDIATE`, the job Tobi's `Writer` Ractor does. Jobs drained by a job Ractor. |
| Per-process caches and memos | `MessageVerifier.@derived_keys`, `ViewHelpers.@slots`, `TypedStore.@cache_val`, `Rails.@cache_store`, `GzipCache.@store/@pieces/@runs` | Ractor-local (`Ractor[:k] ||= …`), with capacity split across workers. Tobi does the same. |
| Boot-time config | `Rails.@secret_key_base`, `ActiveRecord::Registry.@entries`, `Main.@route_table` | `Ractor.make_shareable` at boot. |
| Lazy loading | `Main.instantiate_controller` writes `@__ctl_*` and calls `require_relative` on first use | Eager-load at boot. |
| Cross-worker fan-out | `Cable::Reactor.@todo/@connections`, `Cable::Workers::QUEUE`, `Broadcasts` transport | A hub Ractor for pub/sub (Tobi's `Bus`). The 4-worker Puma emit already lacks cross-worker cable delivery; this is the same gap. |

The six constants a boot freeze cannot fix are `Cable::Workers::QUEUE`,
four mutexes (`Cable::Workers::START_MUTEX`, `Cable::Reactor::START_MUTEX`,
`Rails::Application::CONFIG_LOCK`, `ActiveJob::QUEUE_LOCK`), and
`Rails::Application::X_WEB_PUSH_POOL_SLOT`. They belong to the rows
above.

## Campfire's own source

Nothing here needs a campfire change. Campfire has no class variables,
and its `class << self` blocks define methods without holding state.
Its unfrozen constants (`TRANSLATIONS`, `REACTIONS`, `VERSIONS`,
`HUMANIZE_INVOLVEMENT`, `Sound::BUILTIN`) are cleared by the boot
freeze. The blockers are in its dependencies and in our runtime.

## Limits

- One platform (macOS arm64) and one Ruby (4.0.5).
- Bench routes only: no `/cable` socket path, no job path, and no
  outbound HTTP traced. Rows 6–9 come from isolated calls or the census.
- The census counts every loaded module. It is an upper bound; the
  trace is what says something is reached.
- Not measured: whether Ractors are *faster* for this app. This ledger
  only says what stands in the way.
