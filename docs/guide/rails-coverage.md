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
Tailwind, JSON endpoints) is what **every server target** passes the
DOM-equivalence gate against on every push. What the blog uses is
supported everywhere.

**Campfire** (Basecamp's chat product — file attachments with image
variants, rich text, web push, bots and webhooks, full-text search,
signed cookies and sessions, `Current`, fragment caching, rate
limiting, ~70 routes) is what the **Ruby shape** — running on CRuby,
and compiled by [Spinel](spinel.md) — passes the same gate against,
along with Campfire's own test suite and its cable broadcasts. What
Campfire uses beyond the blog is supported on those two lanes, and
reaches the others as their emitters and runtimes catch up.

## Active Record

| | Blog tier (all targets) | Campfire tier (ruby, spinel) |
|---|---|---|
| Attributes | From `db/schema.rb` (migrations as fallback), typed per column; `id`, timestamps, defaults, nullability | + `enum`, `serialize`/`has_json` columns, `has_secure_token`, `has_secure_password` (real bcrypt), `normalizes` |
| Associations | `belongs_to`, `has_many` (with `dependent:`), `has_one`; association readers, builders, `<assoc>_ids` | + `has_many :through`, polymorphic, `touch:`, `has_one_attached`/`has_many_attached`, `has_rich_text` |
| Validations | `presence`, `absence`, `length` (min/max), `numericality` (bounds, `only_integer`), `format`, `inclusion`, `uniqueness`; `errors`, `full_messages`, `valid?` | + custom `validate` methods, conditional `if:`/`unless:` |
| Callbacks | `after_create_commit` and the Turbo `broadcasts_to` family | + `before_save`/`after_save`, `before_destroy`, `after_touch`, STI-aware callback inheritance |
| Queries | `where` (hash and string), `order`, `limit`, `find`, `find_by`, `first`/`last`, `count`, `exists?`, `includes`, `pluck`, named `scope`s — folded to SQL by the Arel builder | + `joins`, `left_joins`, `group`/`count`, `select`, `distinct`, `or`, `none`, `find_or_create_by`, `insert_all`, `update_all`, `in_batches`, `sum`; a live `Relation` for chains the builder can't fold |
| Persistence | `create`, `save`, `update`, `destroy`, `new`/`build` | + `destroy!`, `increment!`, `update_columns`, transactions, dirty tracking predicates |
| STI | — | `type` column dispatch, subclass scopes, `is_a?` on records |

## Action Controller

| | Blog tier | Campfire tier |
|---|---|---|
| Actions | The seven RESTful actions and any other; implicit render | + `head`, `send_file`, `rescue_from`, `rate_limit` (`to:`/`within:`/`by:`/`with:`/`only:`/`except:`, counted in the app's cache as in Rails) |
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
| Routing | `resources`/`resource` (nested, `only:`/`except:`, `member`/`collection`), `namespace`/`scope`, `root`, `get`/`post`/…, `constraints`, format suffixes, Active Storage's mounted engine. Not: `concern`, `direct` (a custom URL helper with an arbitrary body — dropped), `mount` of any other engine, Devise's/Doorkeeper's DSL. |
| Configuration | `config.x.*`, initializers that define constants or mix modules into models, `Rails.application.config` reads, the app's inflections. Not: `Rails.application.credentials`. |
| Caching | Fragment caching (`cache` in views, keyed by record) and `Rails.cache.fetch`, in-process. |
| Gems | The census names what is modeled. Modeled today: bcrypt, image_processing/ruby-vips (Spinel), rqrcode, useragent, web-push, net-http-persistent, concurrent-ruby's thread pool, importmap-rails, turbo-rails, stimulus-rails, tailwindcss-rails, jbuilder, propshaft. Everything else in a Gemfile is either infrastructure (never enters the analysis) or unknown. |

## What is not lowered, anywhere

The analyzer is whole-program and static. Whatever it cannot see
through at compile time it cannot type, and whatever it cannot type it
does not emit:

- **Metaprogramming that constructs names at runtime**: `send` and
  `public_send` with a non-literal method name, `define_method`,
  `method_missing`, `instance_variable_get`/`set`, `const_get`,
  `eval` in any form, `Class.new`.
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
fresh; a Turbo stream name is not signed; the query cache replays
small results only; `increment!` is a read-modify-write.

Anything not in that section that differs from Rails is a bug, and the
[compare oracle](verifying.md) is how to demonstrate it.

## Security posture

One divergence is worth stating on its own, because it decides whether
an emitted app can face the public internet today: **CSRF tokens are
issued but not verified.** Forms carry an `authenticity_token` and
pages carry the meta tags, exactly as Rails renders them — the compare
oracle requires it — but no lane checks the token on the request, so
`protect_from_forgery` and `skip_forgery_protection` are both no-ops.
Sessions and signed cookies are real (HMAC, Rails-compatible), as is
`has_secure_password`; what is missing is the one check that stops a
third-party page from submitting a form on a signed-in user's behalf.
Until it lands, put an emitted app behind something you trust, or
treat it as the demo it is.
