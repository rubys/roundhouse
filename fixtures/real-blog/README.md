# real-blog fixture

Source of truth: the output of
[`ruby2js/test/blog/create-blog`](https://github.com/ruby2js/ruby2js/blob/master/test/blog/create-blog),
distributed as
[demo-blog.tar.gz](https://ruby2js.github.io/ruby2js/releases/demo-blog.tar.gz).

A modernized Rails 8 blog with:
- Article and Comment models with `has_many :comments, dependent: :destroy` / `belongs_to :article`
- Turbo Streams broadcasting via `broadcasts_to` and `turbo_stream_from`
- Tailwind CSS class attributes (string + hash-of-conditions)
- Rails 8 strong params using `params.expect(...)`
- Form helpers: `form_with`, `form.label`, `form.text_field`, `form.textarea`, `form.submit`
- View helpers: `pluralize`, `link_to`, `render @articles`, `content_for`
- Nested `resources :articles do resources :comments, only: [...] end` routes
- `respond_to do |format| ... format.html ... format.json ... end` controller pattern
- Migrations (no generated `db/schema.rb`; Rails 8 regenerates from migrations)

## What's included here

A subset of the full Rails app — the files that matter for ingest, INCLUDING
tests (per the target implementation workflow: model tests → controller tests
→ browser tests is the forcing-function sequence for each target):

- `app/models/*.rb`
- `app/controllers/*.rb`
- `app/views/articles/*.html.erb`, `app/views/comments/_comment.html.erb`
- `app/views/layouts/application.html.erb`
- `config/routes.rb`
- `db/migrate/*.rb`
- `test/models/*.rb` — unit tests; first behavioral validation layer
- `test/controllers/*.rb` — integration tests; second validation layer
- `test/system/*.rb` — browser/acceptance tests; final validation layer
- `test/fixtures/*.yml` — data fixtures for tests
- `test/test_helper.rb`, `test/application_system_test_case.rb`
- `Gemfile`

Deliberately excluded:
- `app/helpers/`, `app/jobs/`, `app/mailers/`, `app/javascript/`, `app/assets/`
- `.json.jbuilder` templates (separate template language)
- `config/` beyond `routes.rb` (Rails env + initializers; out of current scope)
- `bin/`, `tmp/`, `log/`, `public/`, `storage/`, `vendor/`
- `test/integration/`, `test/mailers/`, `test/helpers/` (empty or out of scope)

## Why it's here

This fixture is the target for Phase 1 of the multi-target plan
(see `project_roundhouse_strategy.md` in the auto-memory). Ingest
against it identifies Rails dialect gaps, which drive priority for
ingest/emit/analyzer work. Expected failures on any given day; the
fixture moves from "mostly rejected" to "fully ingested" incrementally.

## Known gaps as of 2026-04-17

Cleared (the fixture now fully ingests):

- **ArrayNode with style preservation** (`[:a]`, `%i[a b]`, `%w[a b]`)
- **String interpolation** (`"x#{y}z"` → `ExprNode::StringInterp`)
- **ParenthesesNode** — unwrapped transparently at ingest.
- **ERB output-block tags** (`<%= form_with do |f| %>...<% end %>`)
- **ERB comment tags** (`<%# ... %>`) drop + merge surrounding text
- **Short-circuit operators** (`&&` / `||` / `and` / `or`) →
  `ExprNode::BoolOp { op, surface }`
- **`yield :sym`** → `ExprNode::Yield`
- **Routes DSL**: `root "c#a"`, `resources :name [, only:, except:]`,
  nested `resources ... do resources ... end` → `RouteSpec` enum.
  `config/routes.rb` now round-trips byte-for-byte and sits on the
  `EXPECTED_RUBY_FILES` inclusion list.
- **Model body**: source-ordered `Vec<ModelBodyItem>`; unknown class-
  body calls (`broadcasts_to`, `primary_abstract_class`, …) now
  preserved as `Unknown` rather than silently dropped. `Model.parent`
  field added so `class ApplicationRecord < ActiveRecord::Base`
  round-trips. `app/models/application_record.rb` on the inclusion
  list.
- **Controller body**: same refactor — source-ordered
  `Vec<ControllerBodyItem>` with `Filter`, `Action`, `PrivateMarker`,
  `Unknown`. `private` keyword preserved at its source position;
  `allow_browser`, `stale_when_importmap_changes` et al. no longer
  silently drop.
- **LambdaNode** (`->(x) { ... }`) — ingests as `ExprNode::Lambda`.
- **RescueModifierNode** (`expr rescue fallback`) — preserved as
  `ExprNode::RescueModifier`.
- **Length validation rule**: `validates :body, length: { minimum: 10 }`
  now ingests as `ValidationRule::Length { min, max }` and round-trips.
- **Ruby comments on class-body items**: ported ruby2js's
  `associate_comments` — each `ModelBodyItem` / `ControllerBodyItem`
  carries its leading `# …` lines. Emit reproduces them at the right
  indent before the item.
- **Blank lines between body items**: detected from source offsets
  (two consecutive newlines between prev stmt's end and this item's
  leading area). Emitted as a bare blank line before leading comments.
  Together with the comment work, this promoted
  `app/controllers/application_controller.rb` and
  `app/models/article.rb` onto the inclusion list.
- **Block delimiter style** (`{ … }` vs `do … end`): preserved on
  `ExprNode::Lambda` via a `BlockStyle` enum, detected from Prism's
  `BlockNode.opening_loc` bytes. Promoted `app/models/comment.rb`
  onto the inclusion list.

Remaining (in rough priority order):

1. **`broadcasts_to`/`broadcast_replace_to`** now preserved as
   `Unknown` model-body entries (round-trip OK). Remaining gap:
   `after_create_commit { ... }` callbacks with attached blocks,
   plus blank-line / comment preservation between body entries
   (required for `article.rb` / `comment.rb` byte-for-byte).
1. **`params.expect(...)`** (Rails 8 strong-params) — works
   syntactically as a generic Send, but needs a recognizer for
   analyzer effect/shape.
2. **`respond_to do |format| ... end`** — CallNode with block;
   inside, `format.html { ... }` / `format.json { ... }` are
   renders. Needs a recognizer for render targets.
3. **Migration ingest** — currently we read `db/schema.rb`; here we
   have `db/migrate/*.rb`. Need to either generate schema.rb from
   migrations or ingest migrations directly.
4. **`t.references`, `t.timestamps`** schema shorthands.
5. **Symbol table names** (`create_table :articles do |t|`) vs the
   string form we currently handle.
6. **Extra validation rules beyond Presence/Absence/Length** —
   `uniqueness:`, `format: { with: … }`, `numericality:`,
   `inclusion: { in: … }` all still drop at ingest.
7. **ERB comments** (`<%# %>`) are dropped and surrounding text
   merged; the Ruby `#` form on class-body items is now preserved.
   Per-expression (inside method bodies) comments still drop.
    ERB comments now merge surrounding text so IR round-trips, but
    the comment content is lost.
11. **View helpers** (`link_to`, `form_with`, `form.label`,
    `pluralize`, `content_for`, `render @articles`,
    `turbo_stream_from`) emit via the generic Send path. Target
    emitters will need per-helper translation rules.
12. **Multi-line argument formatting** in ERB output tags — works
    semantically but doesn't round-trip byte-for-byte.
13. **Unknown class-body calls** (`allow_browser`,
    `stale_when_importmap_changes`, `primary_abstract_class`) —
    silently dropped by the controller/model recognizers.

## Workflow

`tests/real_blog.rs` pairs three forcing functions against this fixture:

- **`ingests_without_errors`** — fails loudly if any recognizer
  regresses. Ingest is expected to complete today.
- **`expected_files_round_trip_byte_for_byte`** — compares emitted
  Ruby against the fixture source for every file on the inclusion
  list. The list is empty today; as gaps close, promote individual
  files onto it.
- **`ir_is_fixed_under_emit_ingest`** — ingest → emit → ingest must
  yield identical IR. Already passing.
