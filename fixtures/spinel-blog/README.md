# spinel-blog

A complete blog application — models with validations and associations,
Hotwire-style Turbo Stream broadcasts, controllers with strong
parameters and flash, layouts styled with Tailwind — implemented in
pure CRuby with no metaprogramming and no Rails dependency at runtime.
The shape that a future Roundhouse Phase-1 lowerer must produce when
targeting Ruby; also the seed for compiling the same blog into a
native binary via [Spinel](https://github.com/matz/spinel).

This is a working specimen, not a framework. Read it to see what
"Rails-shape Ruby without metaprogramming" actually looks like
across an end-to-end MVC + WebSocket stack.

## Quick start

```sh
bundle install                     # gems: sqlite3, minitest, rake, turbo-rails
make                               # build assets, run all tests + linter
make dev                           # start the dev server on http://localhost:3000
```

Then open `http://localhost:3000/articles` in a browser. Create an
article. The index page has a `<turbo-cable-stream-source>` that opens
a WebSocket back to the dev server; subsequent creates from any other
browser tab — or even from `curl` — appear in the index in real time
without a page refresh.

`make` targets:

| target | what |
|---|---|
| `make` | `make assets test lint` |
| `make assets` | `static/app.css` (Tailwind, ~13.5KB minified) + `static/turbo.min.js` (105KB, copied from turbo-rails gem) |
| `make build` | `spinel main.rb -o build/blog` (requires `spinel` on PATH; not used by the rest) |
| `make test` | 224 minitest runs / 577 assertions |
| `make lint` | the spinel-subset rule scanner over production code |
| `make dev` | runs `server/dev_server.rb` on :3000 |
| `make run` | one-shot CGI smoke (`REQUEST_METHOD=GET PATH_INFO=/articles ruby main.rb`) |
| `make clean` | `rm -rf build/ static/` |

## Architecture

```
fixtures/spinel-blog/
  app/
    controllers/                 ApplicationController + Articles + Comments
    models/                      ApplicationRecord + Article + Comment
    views/
      articles/{index,show,new,edit,_form,_article}.rb
      comments/_comment.rb       (.rb, not .erb — see "No ERB at runtime")
      layouts/application.rb
    assets/tailwind.css          single-line: @import "tailwindcss";
  config/
    routes.rb                    flat route table (no resources DSL)
    schema.rb                    CREATE TABLE strings
  runtime/
    active_record/{base,validations,broadcasts,errors}.rb
    action_view/{view_helpers,route_helpers}.rb
    action_controller/{base,parameters}.rb
    action_dispatch/router.rb
    sqlite_adapter.rb            sqlite3 gem-backed
    in_memory_adapter.rb         pure-Ruby Hash-backed; spinel-target candidate
    cgi_io.rb                    CGI/1.1 request parser + response writer
    broadcasts.rb                in-memory log + file IPC for the dev server
    inflector.rb                 pluralize (verbatim from runtime/ruby/)
  server/
    dev_server.rb                HTTP + WebSocket front-end (CRuby only)
  test/
    {runtime,models,controllers,integration,views,tools}/*_test.rb
  tools/
    check_spinel_subset.rb       grep-based linter
  main.rb                        CGI entry point (the file spinel compiles)
  Gemfile  Rakefile  Makefile
```

**Two layers, intentional split:**

1. The **fixture proper** (`main.rb` + `runtime/` + `app/` + `config/`)
   is metaprogramming-free Ruby that reads CGI requests from `ENV` +
   `$stdin` and writes responses to `$stdout`. No sockets. This is the
   layer that spinel will eventually compile.
2. The **dev server** (`server/dev_server.rb`) is a separate Ruby
   process that listens on TCP, terminates HTTP and WebSocket
   connections, dispatches dynamic requests to the fixture by
   `IO.popen`-ing `main.rb`, and watches a directory for broadcast
   fragments. This layer uses Threads, Mutex, Sockets — things spinel
   doesn't support and never needs to compile.

The contract between them: **the fixture writes broadcast fragments
to `$BROADCAST_DIR/<stream>__<ts>.frag`; the dev server watches that
directory and forwards new fragments over WebSocket to subscribed
Turbo clients.** Atomic publish (`.tmp` write + rename) means the
watcher never sees a half-written file.

## What's implemented

| Concern | Status |
|---|---|
| ActiveRecord-shape models | `attr_accessor` for columns, typed `initialize`/`attributes`/`[]`/`[]=`/`update`, `has_many` + `belongs_to` lowered to typed methods, `dependent: :destroy` cascade |
| Validations | `validates_presence_of`, `validates_length_of`, `validates_numericality_of`, `validates_inclusion_of`, `validates_format_of`, `validates_absence_of` (block-based attr access — no `instance_variable_get`) |
| Lifecycle callbacks | `before_*` / `after_*` for save/create/update/destroy + `*_commit` variants; auto-fill `created_at`/`updated_at` |
| Adapters | `SqliteAdapter` (sqlite3 gem) + `InMemoryAdapter` (pure Ruby Hash); same interface, swappable |
| Routing | Pattern-matching path → controller dispatch with nested resources |
| Controllers | `params.require.permit`, `before_action`-equivalent, `render`/`redirect_to`/`head`, symbolic statuses (`:see_other`, `:unprocessable_entity`) |
| Views | One `Views::<Controller>.<action>` method per template; HTML built via `String#<<` concatenation; no ERB at runtime |
| View helpers | `link_to`, `button_to`, `dom_id`, `content_for`, `turbo_stream_from`, `truncate`, `pluralize`, `stylesheet_link_tag`, `javascript_importmap_tags`, FormBuilder (label/text_field/text_area/submit) |
| Layouts | `Views::Layouts.application(body)` consumes `content_for(:title)`, emits importmap referencing Turbo |
| Broadcasts | Turbo Stream fragments via after-commit hooks; in-memory log for tests + file IPC for dev-server fan-out |
| Flash | Cookie-based round-trip (`flash_notice`, `flash_alert` cookies); render-path clears, redirect-path emits |
| Forms | POST/PATCH/DELETE via hidden `_method` field; method override on the dev-server side; full create/update/destroy flow |
| HTTP entry point | `main.rb` parses CGI, dispatches, writes response with status + headers + body |
| Dev server | TCPServer + HTTP/1.1 parser + static-file serving + WebSocket (RFC 6455 framing, `actioncable-v1-json` subprotocol, 3s ping, atomic broadcast forwarding) |
| Asset pipeline | Tailwind v4 via `npx @tailwindcss/cli`; turbo.min.js copied from gem dir |

## No metaprogramming

**Production code** (everything under `runtime/`, `app/`, `config/`)
is scanned by `tools/check_spinel_subset.rb` on every `make`. The
ruleset rejects:

- `instance_variable_get` / `instance_variable_set`
- `define_method`
- `method_missing`
- `eval`, `class_eval`, `instance_eval`, `module_eval`
- `.send(` (any form, even with literal symbols — call methods directly)
- `__send__`
- `Thread`, `Mutex`

Tests and tools are exempt; they may use any Ruby. The dev server
(`server/dev_server.rb`) is also exempt — it intentionally uses
threads and sockets, which spinel forbids, but spinel never sees it.

The cost of the no-metaprogramming rule is paid at the model level:
each model writes out its accessors, its `initialize`, its
`attributes`, its `[]`/`[]=`, and its `update` method explicitly per
column. `Article` is ~85 lines; `Comment` is ~95. A real Rails
`Article < ApplicationRecord` is ~10 lines. The verbosity is the
contract — that's what a future transpiler will emit.

## No ERB at runtime

Each template is a Ruby method that builds an HTML string. The
view file `app/views/articles/show.rb` defines `Views::Articles.show`
that takes typed args (`article`, `notice:`) and returns a String:

```ruby
def show(article, notice: nil)
  io = String.new
  ViewHelpers.content_for_set(:title, "Showing article")
  io << %(<div class="md:w-2/3 w-full">\n)
  io << %(  <h1 class="font-bold text-4xl">)
  io << ViewHelpers.html_escape(article.title)
  io << "</h1>\n\n"
  # ... (continues for ~70 lines)
  io
end
```

A future transpiler will compile ERB templates into this shape.
Today, written by hand.

## Rails free, except for shared assets

The fixture has zero Rails dependency at runtime. The Gemfile pulls
`turbo-rails` only as a vehicle for its bundled `turbo.min.js` (the
Makefile copies the file out of the gem's directory; nothing in the
fixture's Ruby code requires the gem). At compile/run time the only
gems involved are `sqlite3`, `minitest`, `rake`. Spinel-target
runs would drop `sqlite3` for `InMemoryAdapter`.

The shared assets are the Turbo JS file and Tailwind v4 (via npm),
both of which are language-neutral artifacts.

## Limitations and known gaps

These are real and load-bearing — read before drawing conclusions
from this fixture:

### Persistence

- **Default DB is `:memory:`** — single-process tests start fresh.
  The dev server writes to `tmp/blog.sqlite3` so state survives
  *across requests within one dev-server run*, but that file is
  gitignored and discarded between sessions. There's no backup,
  no migration system, no schema versioning.
- **InMemoryAdapter loses everything on process exit.** Useful for
  the spinel-compile path (no FFI required) but unsuitable for any
  persistence scenario.
- **No transactions, no foreign-key enforcement at DB level**, no
  prepared-statement caching, no connection pooling. The application
  doesn't need them; production wouldn't be served by this stack.

### Security

- **No CSRF tokens.** `ViewHelpers.csrf_meta_tags` emits stub meta
  tags with empty content. Any form on the site is forgeable.
- **No signed stream names.** ActionCable's
  `signed_stream_name` is treated as a literal stream name on both
  ends. Any WebSocket client can subscribe to any stream by name.
- **Unsigned cookies.** The flash cookie carries plain text; anyone
  can construct a `Cookie: flash_notice=Hello` and the next page
  will display "Hello" as if it came from a successful action.
- **No HTTPS.** No `Secure` flag on cookies; `HttpOnly` is set but
  `SameSite` is not.
- **No authentication, no authorization.** No user model, no login,
  no per-user scoping. Anyone who reaches the site can create,
  edit, and destroy anything.

This is a demo. It would not survive a public deployment.

### Rails feature gaps

Things real-blog uses that this fixture *doesn't* reproduce:

- **No `respond_to do |format|`.** Every action returns HTML; no
  JSON endpoints (despite real-blog having `*.json.jbuilder` views).
- **No Migration DSL.** Schema is raw `CREATE TABLE` strings in
  `config/schema.rb`; no `t.string`, no `t.references`, no
  `t.timestamps`, no `add_index`.
- **No `validates_uniqueness_of`** and no `belongs_to`-presence
  fallback (real Rails: `validates :article, presence: true` on
  Comment falls back to checking `article_id` if there's no
  `article` column reader).
- **No concerns / module mixin patterns**, no
  `extend ActiveSupport::Concern`. Each model is a flat class.
- **No `before_action :only/:except`** declarations; controllers
  use explicit `if ACTIONS_NEEDING_X.include?(name)` checks inside
  `process_action`.
- **No polymorphic dispatch.** `link_to "Edit", @article` is not
  supported; call sites pass explicit `RouteHelpers.article_path`.
- **No `flash.now`** — only the redirect-bound `flash[:notice]` /
  `flash[:alert]`.
- **No content negotiation, no asset digesting (cache busting),
  no `image_tag` / `asset_path` helpers.** Asset URLs are hardcoded
  to `/assets/app.css` etc.
- **No I18n, no time zones, no multi-language.**
- **No mailers, no background jobs, no file uploads, no PWA
  manifest** despite mailer/manifest references in real-blog views.
- **The form builder is small.** `text_field`, `text_area`, `label`,
  `submit` — that's it. No `select`, no `collection_select`, no
  `check_box`, no `fields_for`. Real-blog forms only need the
  small set; richer forms would need extension.

### Server-side limitations

The dev server (`server/dev_server.rb`) is deliberately small:

- **Single-process Ruby** with one thread per connection. Fine for
  one developer in a browser; will not scale.
- **CGI per-request fork.** Each request execs `ruby main.rb`,
  paying ~50ms of Ruby boot per request. The spinel-compiled
  binary would be much faster, but we haven't measured.
- **No graceful shutdown.** Ctrl-C kills threads abruptly.
- **No request logging.** Errors are warned; happy paths are silent.
- **Watcher is polling-based.** 100ms tick over `BROADCAST_DIR`,
  not real filesystem events. Would need `inotify`/`FSEvents`/etc.
  for production.
- **WebSocket sends are blocking.** A slow client briefly stalls
  the watcher's broadcast dispatch loop. No backpressure handling.
- **No frame fragmentation, no WS extensions, no compression.**
  All sends are single-frame text; client masking is required and
  enforced; non-text/control frames from the client are silently
  dropped.

### Testing posture — this is the important one

- **224 minitest runs / 577 assertions, all passing.** Coverage:
  every model method, every controller action's happy + sad path,
  every view file's structural fragments, full request-dispatch
  cycle through the CGI entry, cookie round-trips, broadcast log
  contents.
- **Manual smoke tests pass**: `curl` through the dev server for
  GET/POST/static, plus a hand-written WebSocket client script that
  walks the full handshake → subscribe → broadcast cycle.
- **Not E2E tested with a human in a browser.** Nobody has loaded
  `http://localhost:3000` in Chrome/Firefox/Safari and clicked
  through the UI. Unverified visually:
  - Tailwind classes render correctly (we know the CSS file
    contains `.bg-blue-600` etc., but haven't seen the styled page)
  - Turbo Stream broadcasts actually mutate the DOM in real time
    (we know fragments arrive over WS in the right format, but
    haven't watched the index page update when another tab posts)
  - Forms submit, redirects follow, flash notices appear and
    disappear (we know the wire format is correct; haven't watched
    a browser do the round-trip)
  - The `_method` hidden field correctly drives PATCH/DELETE via
    the form (browser actually emits a POST + `_method=patch`)
- **No Selenium / Playwright / browser-driver tests.** The cost of
  setting that up exceeds the benefit for a demo of this scope; a
  human running through the demo once is the planned validation.

If something doesn't work in a browser, the most likely culprits
are: (a) Turbo's expectations about the connection identifier or
broadcast envelope shape (we matched what `runtime/python/cable.py`
does, which is known-working in the larger roundhouse codebase), or
(b) some Tailwind class name we use that didn't get included in
the build (the `--content` glob covers `app/**/*.rb` and
`runtime/**/*.rb`, and the JIT extracts utility names from
strings, but unusual constructions could escape it).

### Spinel compatibility

- Linter-clean across all 34 production files.
- Three known soft-blockers exist as small CRuby-isms:
  - `force_encoding("UTF-8")` in `cgi_io.rb`'s `url_decode`
    (CRuby-specific; spinel "assumes UTF-8/ASCII" so it's a no-op
    there)
  - `Time.now.utc.iso8601` (Time is in spinel's supported types
    but the exact `.iso8601` method may need verification)
  - `__FILE__ == $PROGRAM_NAME` guard (both globals are
    fundamental; should compile)
- **The fixture has not actually been compiled by spinel yet.**
  That's a separate session's work, with its own findings.
  Nothing here claims to be confirmed-spinel-compatible — only
  shaped-to-be-spinel-compatible by careful adherence to the
  enumerated subset.

## How to read this fixture

If you're trying to understand the metaprogramming-removal pattern,
the most informative reads in order:

1. `app/models/article.rb` — the explicit per-column shape that
   replaces `attr_accessor` magic.
2. `runtime/active_record/base.rb` — what's left of `Base.rb` once
   the metaprogramming is gone (CRUD scaffolding + lifecycle hook
   protocol; subclasses provide all the per-column knowledge).
3. `runtime/active_record/validations.rb` — block-based attribute
   access (`validates_presence_of(:title) { @title }`) instead of
   `instance_variable_get("@#{attr}")`.
4. `app/views/articles/show.rb` — what compiled-ERB looks like.
5. `app/controllers/articles_controller.rb` — `before_action`
   lowered to an explicit case statement; strong parameters via
   `params.require.permit` over a hand-rolled `Parameters` class.
6. `runtime/cgi_io.rb` — CGI/1.1 in ~140 lines, no `cgi` stdlib.
7. `server/dev_server.rb` — pure-Ruby HTTP + WebSocket in ~390
   lines. Reference the top docstring for the wire-format details.

## Relationship to roundhouse

This fixture is a *contract*, not a deliverable. The
expected progression:

1. **(Done)** Hand-write the fixture; tests pass; `make dev`
   produces a working blog.
2. **(Next)** Roundhouse adopts the metaprogramming-free
   `runtime/active_record/*.rb` portion, replacing the
   reflective version in `runtime/ruby/active_record/`.
   This makes the existing `runtime_src_integration` typer test
   pass for free.
3. **(Future)** Lower Rails-pattern recognizers (the parts that
   today juntos handles via filters) into the roundhouse Phase-1
   pipeline. The output: typed IR.
4. **(Future)** Add a Phase-2 Ruby emitter that takes the typed
   IR and emits the shape this fixture demonstrates by hand.
5. **(Future)** Spinel ingests the emitted Ruby and produces a
   native binary; the dev-server pattern (or its C/Rust
   successor) wraps it for HTTP + WebSocket termination.

When step 4 lands, the test that the emitter is correct is:
*generate `fixtures/spinel-blog/`-shaped output from
`fixtures/real-blog/` source*. This fixture is what success
looks like.
