# Rails coverage

Roundhouse does not implement Rails. It recognizes the Rails an app
uses — the conventions, the DSL, the helpers — and lowers each
recognized form to target-neutral IR that every emitter consumes. So
"is X supported?" has two parts: does the analyzer recognize X, and
does each target have a lowering and a runtime for it. This page is
the feature-level answer as of this snapshot; the per-app answer is
the tools:

- `roundhouse check --continue` ([`check.md`](check.md)) — the survey
  report lists every construct the analyzer did not recognize in
  *your* app, and the gem census lists every gem it does not model.
- the MCP `wont_lower` tool ([`mcp.md`](mcp.md)) — for a named target,
  the constructs in your app that have no lowering to it.

Those are authoritative and current; this page is the map.

## Two tiers

Coverage is proven by apps, not by feature lists. Two apps define the
tiers.

**The blog** (`fixtures/real-blog`, the Rails 8 scaffold with articles,
comments, nested routes, validations, Turbo Streams, Action Cable,
Tailwind, JSON endpoints) is the shared DOM-equivalence fixture for
**every server target** in scheduled full validation. Ordinary PRs run the
Ruby floor plus selected target lanes; pushes to canonical `main` run Ruby
plus Spinel. The extra-language matrix waits for the four-hour schedule or
`ci:full`. See [CI coverage](../ci/README.md).
A passing target's comparison proves the blog's features on that target.

**Campfire** (Basecamp's chat product — file attachments with image
variants, rich text, web push, bots and webhooks, full-text search,
signed cookies and sessions, `Current`, fragment caching, rate
limiting, ~70 routes) is what the **Ruby shape** — running on CRuby,
and compiled by [Spinel](spinel.md) — passes the same gate against,
along with Campfire's own test suite and its cable broadcasts. What
Campfire uses beyond the blog is supported on those two lanes, and
reaches the others as their emitters and runtimes catch up.

## Structural pattern matching

Ruby 3 `case/in`, predicate matches (`value in pattern`) and required
matches (`value => pattern`) are ingested separately from `case/when`.
The Ruby-family emitter preserves native patterns, guards, pins, captures,
array/find/hash destructuring and rest bindings, including bare `**` and
`**nil`. CRuby emit-and-run tests cover dispatch, escaping bindings,
partial bindings after failed guards, and mismatch exceptions; expression
tests also check syntax round-trips and once-only pin evaluation.

Other language targets reject these constructs before emitting a project,
including apparently simple literal/nil/binding patterns. Their existing
`case/when` renderers do not consistently preserve Ruby `===`, bindings,
or `NoMatchingPatternError`; no portable subset is claimed yet. The
Ruby-shaped Spinel output uses native syntax, not a separately implemented
matching runtime. Compile/runtime support still depends on the pinned
Spinel compiler. Deconstructed element types remain gradual where the
analyzer cannot determine the protocol's result shape.

## Local method visibility

Model and library/concern ingest preserve statically known `public`,
`protected`, and `private` on local definitions, including singleton
blocks and inline `private def` forms. Named changes must follow an
unambiguous local definition; forward references, inherited-only names,
dynamic names/conditional declarations, and visibility-sensitive
redefinitions remain unsupported. Concern class-method carriers have
their own lexical scope, distinct from the concern's own singletons.
`private_class_method`/`public_class_method` inside singleton blocks or
concern class-method carriers address a further singleton level and
remain unsupported, rather than changing the flattened methods.

The retained singleton form of `module_function` is public even when
the source instance method is private; `extend self` instead retains
the source visibility. Bare instance-visibility markers end the
`module_function` mode but not `extend self`. This does not add the
separate private instance copy or copy/redefinition semantics to the
existing one-method-per-name lowering.
`module_function` inside singleton blocks or concern class-method
carriers remains unsupported: its public copy belongs to the carrier,
not the includer, so it must not make the includer's method public.

The common Ruby/Spinel emitter writes named visibility immediately after
each definition, not sticky sections. A plain `private` does not affect
`def self.x`; constructors remain implicitly private unless explicitly
made public. CRuby emit-and-run tests prove wrapper calls, `send`,
`public_send`, and `respond_to?(name, include_private)` for model, library,
and concern methods. This is not a strict-target visibility/reflective
dispatch compatibility claim, nor a Spinel runtime verification.
Nonpublic model accessor macros that have not become local `MethodDef`s
are still diagnosed rather than silently emitted as public. Existing
model lowering's synthesized-name precedence is unchanged.

Thin builder-yielding form wrappers retain owner-local `data:` computations
through generated callable bridges when the expression is frame-independent
and defaults are literal. The bridge has required typed parameters, while
the original private helper methods remain private. Model/URL/namespace and
id/class syntax is not hidden behind bridges; executable defaults, captures
and shared wrapper-local frames are outside this correction. CRuby regression
tests cover helper-name shadowing, private dispatch and single evaluation;
this is not a general wrapper-inlining or compiled Spinel compatibility claim.

### Static concern method macros

Concern-provided class methods such as Writebook's `positioned_within`
can specialize parameterless `define_method` blocks into ordinary model
methods before inference. Required positional and required/optional
keyword arguments must bind immutable Symbols. Named visibility applies
only to methods defined in that invocation. Each includer gets its own
bindings; the supplying include must precede the call.

This is not general metaprogramming support. Ambiguous providers,
repeated/nested invocations, synthesized/inherited method collisions,
overridden macro primitives, mutable captures, splats/destructuring,
block parameters, constant references with unproven lexical binding,
control flow, and effects outside definitions remain unsupported.
Recognized but unrepresentable macros fail strict ingestion; survey mode
records the gap and retains the original model body without partial expansion.
Literal reflection is grounded only on public generated association/scope
APIs without app-owned dispatcher/target/reader overrides. The shared
Ruby/Spinel emission path then threads these calls through Relations.

CRuby emit-and-run tests prove the parent binding, filtering, ordering,
self-exclusion and reflective privacy of the generated helpers, not the
whole Positionable concern (locking/rebalancing/callbacks), strict-target
execution or compiled Writebook compatibility. Writebook remains a
diagnostic corpus until its independent framework and Spinel gaps close.

## Active Record

| | Blog tier (all targets) | Campfire tier (ruby, spinel) |
|---|---|---|
| Attributes | From `db/schema.rb` (migrations as fallback), typed per column; `id`, timestamps, defaults, nullability | + `enum`, `serialize`/`has_json` columns, `has_secure_token`, `has_secure_password` (real bcrypt) |
| Associations | `belongs_to`, `has_many` (with `dependent:`), `has_one`; association readers, builders, `<assoc>_ids` | + `has_many :through`, polymorphic, `touch:`, `has_one_attached`/`has_many_attached`, `has_rich_text` |
| Validations | `presence`, `absence`, `length` (min/max), `numericality` (bounds, `only_integer`), `format`, `inclusion`, `uniqueness`; `errors`, `full_messages`, `valid?` | + custom `validate` methods, conditional `if:`/`unless:` |
| Callbacks | `after_create_commit` and the Turbo `broadcasts_to` family | + `before_save`/`after_save`, `before_destroy`, `after_touch`, STI-aware callback inheritance |
| Queries | `where` (hash and string), `order`, `limit`, `find`, `find_by`, `first`/`last`, `count`, `exists?`, `includes`, `pluck`, named `scope`s — folded to SQL by the Arel builder | + `joins`, `left_joins`, `group`/`count`, `select`, `distinct`, `or`, `none`, `find_or_create_by`, `insert_all`, `update_all`, `in_batches`, `sum`; a live `Relation` for chains the builder can't fold |
| Persistence | `create`, `save`, `update`, `destroy`, `new`/`build` | + `destroy!`, `increment!`, `update_columns`, transactions, dirty tracking predicates |
| STI | — | `type` column dispatch, subclass scopes, `is_a?` on records |

### SQL identifier boundary

Compiler-generated SQLite DDL, indexes, seeds and Arel/model CRUD quote
physical identifiers without renaming them, including keywords, spaces,
hyphens and embedded quotes. Emitted models keep `table_name` raw and
provide the SQL spelling separately to `Relation`; ordinary filter/order
chains on those models use it too. A dot in one physical name stays part
of that name, not a new schema-qualified ingest feature. Quoting metadata
does not widen the Ruby table-name declarations ingest can interpret.

This is not complete quoting of every SQL path. Association JOIN strings,
nested dynamic qualifiers/column names and native adapter fallbacks remain
gaps: accepted source can still generate invalid SQL there, without a new
unsupported diagnostic. The compiler SQL dialect is SQLite, not
PostgreSQL/MySQL. `tests/sql_identifiers.rs` executes synthetic SQLite and
emitted CRuby cases; its other language checks prove DDL string preservation,
including literal interpolation markers, not whole-project execution on
every target. Native hand-written models still need SQL-ready identifiers;
the Base metadata fallback does not dynamically quote arbitrary names.

The runtime typing ledger records one additional unresolved self-send in
the isolated RBS probe and three additional gradual reads for raw
`RecordNotFound` messages through Relation's existing untyped model handle.
The strict fully-typed runtime gate and diagnostics are unchanged; this does
not add generic class-object/Relation support to strict targets.

## Action Controller

| | Blog tier | Campfire tier |
|---|---|---|
| Actions | The seven RESTful actions and any other; implicit render | + `head`, `send_file`, `rescue_from`, `rate_limit` (`to:`/`within:`/`by:`/`with:`/`only:`/`except:`, counted in the app's cache as in Rails), `invisible_captcha` (`only:`/`except:`/`prepend:` → honeypot `spam?` on `subtitle` only; rotated/custom names unmodeled), pretender `impersonates :scope` when the controller defines a local `current_<scope>` (wraps it as `true_<scope>`, adds `impersonate_` / `stop_impersonating_`; inherited-only Devise helpers stay unsupported) |
| Filters | `before_action` with `only:`/`except:`, ivar flow into views | + `around_action`, `after_action`, `if:`/`unless:` guards (symbol and lambda), `skip_before_action`, filters from concerns |
| Params | `params.expect`, `params.require(...).permit(...)`, `params[:id]`; typed by the schema they're assigned to | + nested permits, arrays, `params.merge`, indifferent access |
| Responses | `render` (template, partial, `json:`, `status:`), `redirect_to` (record, path, `status:`), `respond_to` with `format.html`/`format.json`, `flash` and `flash.now` | + `expires_in`, `stale?`/`fresh_when` (answered as always fresh — a deliberate divergence), `cookies` and `cookies.signed`/`.permanent`, `session`, `helper_method`, `layout` |
| Concerns | `include`d modules with `included do` filter blocks | + `class_methods`, concern-defined actions and helpers |
| Auth | — | `Current` attributes, `authenticate_by`, signed/global ids, `has_secure_password` sessions |

## Action View

| | Blog tier | Campfire tier |
|---|---|---|
| Templates | ERB; partials (`render "form"`, `render @articles`, `render partial:` with locals and collections); layouts; `content_for`/`yield` | + HAML is read by the analyzer; emit is ERB-only today |
| Helpers | `link_to`, `button_to`, `form_with` and the form builder (`label`, `text_field`, `text_area`, `submit`, `hidden_field`), `dom_id`, `pluralize`, `truncate`, `l`/`t` with the app's locale files, `csrf_meta_tags`, `stylesheet_link_tag`, `javascript_importmap_tags`, `turbo_stream_from`, `cache` | + `image_tag`/`asset_path` with digests, `tag.*` builders, `content_tag`, `sanitize` (safe-list sanitizer port), `turbo_frame_tag`, the rest of the form builder (`select`, `check_box`, `radio_button`, `button`, `url_field`, `email_field`, `password_field`, `file_field`, `rich_text_area`, `fields_for`), `hidden_field_tag`, `time_ago_in_words`, `number_to_human`, `url_for` |
| JSON | jbuilder views (`json.extract!`, `json.array!`, partials) | + `as_json`/`to_json` shapes on records and plain objects |
| Turbo | Turbo Streams broadcast on model commit, `turbo_stream` responses, frames | + custom stream targets and `broadcast_*_later` |

## The rest of the framework

| Component | Status |
|---|---|
| Action Cable | Every server target: `/cable`, `turbo_stream_from` subscriptions, model broadcasts. Campfire tier adds application channels with `subscribed`/`unsubscribed`, `stream_for`, and presence. |
| Active Job | Campfire tier: `perform_later` runs on an in-process queue in the app; `ActiveJob::TestHelper` assertions in the tests. No external queue adapter. |
| Active Storage | Campfire tier: blobs and attachments, the disk service, the engine's routes (redirect and representation), variants via libvips on Spinel. No cloud services. |
| Action Text | Campfire tier: `has_rich_text`, the safe-list sanitizer, attachment rendering. |
| Action Mailer | Ruby tier: mailer classes, `mail(...)`, `deliver_now`/`deliver_later` — delivery appends to `ActionMailer::Base.deliveries` (Rails' `:test` method, which the emitted tests assert against). No SMTP. |
| Routing | `resources`/`resource` (nested, `only:`/`except:`, `member`/`collection`), `namespace`/`scope`, `root`, `get`/`post`/…, `constraints`, format suffixes, Active Storage's mounted engine, Devise's `devise_for` (static default four: session/registration/password/confirmation + `controllers:` overrides for those mappings only — not model-module-driven), `authenticated`/`unauthenticated`/`devise_scope` as passthrough wrappers (auth not enforced). Not: `concern`, `direct` (a custom URL helper with an arbitrary body — dropped), `mount` of any other engine, Doorkeeper's DSL, Devise OmniAuth callback routes, Devise `skip:`/`only:`/`path:` options, model-driven module narrowing. |
| Configuration | `config.x.*`, initializers that define constants or mix modules into models, `Rails.application.config` reads, the app's inflections. Not: `Rails.application.credentials`. |
| Caching | Fragment caching (`cache` in views, keyed by record) and `Rails.cache.fetch`, in-process. |
| Gems | The census names what is modeled. Modeled today: bcrypt, devise (route helpers + `devise_for` / visibility wrappers), image_processing/ruby-vips (Spinel), invisible_captcha, pretender, representable (`Representable::Decorator` with `include Representable::JSON`: `property` / `collection`, rendered as JSON; Roar is not), rqrcode, useragent, web-push, net-http-persistent, concurrent-ruby's thread pool, importmap-rails, turbo-rails, stimulus-rails, tailwindcss-rails, jbuilder, propshaft. Everything else in a Gemfile is either infrastructure (never enters the analysis) or unknown. |

## What is not lowered, anywhere

The analyzer is whole-program and static. Whatever it cannot see
through at compile time it cannot type, and whatever it cannot type it
does not emit:

- **Metaprogramming that constructs names at runtime**: `send` and
  `public_send` with a non-literal method name, `define_method`,
  `method_missing`, `instance_variable_get`/`set`, `const_get`,
  `eval` in any form, `Class.new`, and `obj.extend Mod` on one live
  object (the site — or in a test, the whole test — becomes a raise
  that names the construct, so the rest of the file still compiles).
- **Reopening the framework**: monkey-patches of Rails or core classes
  from initializers (`Module#prepend` into a framework class is
  recorded and skipped on Spinel), `alias_method` inside
  `class << self`.
- **Dynamic loading**: `require` of a file computed at runtime,
  `autoload` of things outside the app's conventions.
- **Anything reached only through an unknown gem's DSL** — the census
  tells you which.
- **Ruby, not Rails**: refinements, `ObjectSpace`, `Fiber`/`Thread`
  used directly by the app, `binding`, `Method` objects.

Each of these is reported by name and location in the survey report,
so the answer for a given app is a list, not a guess. That includes a
class-body macro in a controller that roundhouse does not recognize
(Lobsters' `caches_page`, say): it is listed as a survey gap rather
than dropped in silence, because its effect — a guard, a filter, a
header — would otherwise vanish from the output with no trace.

## Deliberate divergences

Some Rails behaviors are reproduced differently on purpose, because
the Rails behavior depends on a runtime facility the targets don't
have or because it is an accident of implementation. Each is a
decision, recorded with its reasoning in the architecture docs:
[`docs/pipeline/runtime.md` §Deliberate divergences from Rails](../pipeline/runtime.md#deliberate-divergences-from-rails).
The ones a user is most likely to meet: an unsaved record's `id` is
`0`, not `nil`; attribute writers do not type-cast (the column type
is enforced at the boundary instead); conditional GET always answers
fresh; a Turbo stream name is signed only on the Ruby-family targets
(elsewhere it carries an `--unsigned` placeholder); the query cache
replays small results only; `increment!` is a read-modify-write.

Anything not in that section that differs from Rails is a bug, and the
[compare oracle](verifying.md) is how to demonstrate it.

## Security posture

**CSRF is verified on the ruby family** — the CRuby and Spinel lanes,
which is where Campfire deploys. Rails' `load_defaults` 5.2+ implicit
`protect_from_forgery with: :exception` heads every
`ActionController::Base` chain; a written macro re-registers the same
filter (with its `only:` / `except:` / `if:` / `unless:`);
`skip_forgery_protection` removes it. A non-GET request must carry the
session's masked token in the `authenticity_token` param or the
`X-CSRF-Token` header, and a present `Origin` must name the request's
own host; otherwise the answer is Rails' 422. The emitted test harness
turns the check off, as a generated `config/environments/test.rb`
does.

What differs from Rails, and why:

- **Tokens are masked on the ruby family.** Rails' per-render one-time-pad
  XOR lives in `ActionController::AuthenticityToken`; strict-target emit
  of `base.rb` still answers an empty token (no `Current.session`).
- **The Origin check compares hosts, not schemes.** Rails compares
  `request.base_url`. The ruby family now reads `X-Forwarded-Proto`
  (absolute URLs behind a TLS proxy are https, as in Rails), but
  `assume_ssl` is not modeled, so the check still compares hosts only.
- **The session and flash cookies are signed, not encrypted.** Rails'
  cookie store encrypts the session and keeps the flash inside it; the
  ruby family signs the session and gives each flash message
  (`flash_notice`, `flash_alert`) a signed cookie of its own — the same
  HMAC `cookies.signed` uses, keyed from `SECRET_KEY_BASE`. A client can
  read them but not forge them: a cookie that does not verify reads as
  an empty session or no message. The session payload is not Rails'
  JSON either, so a migration from Rails starts every session afresh;
  Campfire's login rides its own signed cookie and carries over.
- **An unset `SECRET_KEY_BASE` is generated, not fatal.** Rails
  development keeps a generated key in `tmp/local_secret.txt`, and
  production refuses to boot without one. The ruby family generates the
  key on first boot and keeps it in `storage/secret_key_base` (mode
  0600), the directory a deployment already persists for its database.
  Set the variable to share one key across instances, or to carry a key
  over from a Rails deployment.
- **Rails' implicit default is applied on every lane.** Under
  `load_defaults` 5.2+, every `ActionController::Base` controller runs
  `verify_authenticity_token` unless the app opts out. The ruby family
  mints and checks masked tokens; strict targets still issue no token and
  treat an empty session secret as "check none" so POSTs are not 422'd.
- **`with: :null_session` / `:reset_session` still register the 422
  handler.** Those strategies are recognized as forgery filters but are
  not modeled as empty-session / wipe pass-throughs. `prepend:` and a
  custom `store:` remain unmodeled.
- **Action Cable's Origin check uses Rails' defaults only.** On the
  ruby family, a `/cable` handshake must carry an `Origin` naming the
  request's own host (compared by host, as above), or in development
  any `localhost` port. Otherwise the answer is Rails' 404 and the app's
  `connect` never runs; a handshake with no `Origin` is refused, as in
  Rails. An app's own `config.action_cable.allowed_request_origins` and
  `disable_request_forgery_protection` are not read. The strict
  targets' sockets check no Origin.

The strict targets issue no token (`form_authenticity_token` is empty
there) and check none: an app emitted for them is not protected
against cross-site forgery. Put an unprotected app behind something
you trust.
