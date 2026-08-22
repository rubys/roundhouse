# geared_pagination's controller concern — `set_page_and_extract_portion_from`
# and the `Page` object it parks in `@page`.
#
# The gem (basecamp/geared_pagination, 1.2.0 — the version campfire's
# Gemfile.lock pins) mixes `GearedPagination::Controller` into
# `ActionController::Base` from its engine initializer, so every
# controller in an app that carries the gem has the method whether or not
# it says so. That is why this is a Base reopen and not a module an app
# includes: nothing in the app's source names it.
#
# Ruby-family only, like cookies.rb and current.rb beside it: a
# `Page`-typed field on Base must NOT transpile to the strict targets,
# which have no app exercising pagination. Required by the
# action_controller.rb aggregator, which the ruby/jruby/spinel trees
# follow; the strict targets emit their runtime from the runtime_loader
# tables and never see this file. An app on one of those targets that
# calls the method gets an unresolved-method error, which is the honest
# ledger entry rather than a silent no-op.
#
# WHAT IS MODELED: the OFFSET portion — `set_page_and_extract_portion_from
# scope, per_page: N`, a fixed page size, `?page=N` off the query string.
# All three of campfire's call sites are exactly that shape.
#
# WHAT IS NOT, and why each is a real gap rather than an oversight:
#
# - `ordered_by:` (cursor pagination, `PortionAtCursor`). The gem's
#   cursor path Base64-encodes the last row's ordered attributes and
#   turns the next page into a `WHERE (a, b) > (…)` tuple comparison
#   built out of Arel. It is a different query shape, not a parameter
#   to this one, and no call site in the corpus passes it. Omitting the
#   keyword means such a call raises ArgumentError — loud, at the call
#   site, naming the keyword.
#
# - GEARED page sizes. The gem's name is its default: `per_page` may be
#   an ARRAY of ratios (`[15, 30, 50, 100]`, the default when the
#   keyword is omitted), so early pages are small and later ones grow.
#   Every campfire site passes a single Integer, which the gem treats as
#   the one-element array `[500]` — a fixed size for every page. Typing
#   the parameter as Integer keeps that concrete; an app that wants the
#   ratios would widen it here, and `per_page` becomes the whole reason
#   `page_count` is not simply `records_count / per_page`.
#
# - The `X-Total-Count` / `Link` response headers the gem's `after_action`
#   writes for JSON requests. base.rb's `headers` hash is buffered but
#   never sent (see its comment), so composing them would be work
#   nothing reads. `records_count` below is the value they need when the
#   harness starts emitting headers.
#
# - `Page#cache_key` / the `etag { @page }` declaration. Conditional GET
#   is always-stale in this runtime (base.rb's `fresh_when`), so an ETag
#   contribution has nothing to compare against.

# `Params.str` narrows the query-string scalar below. Required here
# rather than left to the entry point, for the reason base.rb's
# `message_digest` require gives: the framework runtime is copied as a
# unit by callers that hand-list what they stage, and a require living
# in boot.rb leaves those lists to guess.
require_relative "../params"

module ActionController
  # One page of a relation: the window, and the arithmetic that says
  # where it sits in the whole.
  #
  # Rails' Page is lazy — `records` and `page_count` memoize on first
  # ask. This one resolves both in the constructor. The COUNT is not
  # speculative work: every corpus view asks `last?` (to decide whether
  # to render the next-page frame) on the same request that renders
  # `records`, so the round-trip happens either way, and eager resolution
  # keeps every ivar single-assignment and concretely typed.
  class Page
    # Everything is computed through LOCALS and stored, rather than
    # derived on demand from the ivars. Not a style choice: a runtime
    # ivar's type is `T | Nil` (the body-typer reseeds every ivar
    # nullable, since nothing proves `initialize` ran first), so
    # `@records_count <= 0` is arithmetic on a nullable and types
    # gradual, while the same expression over a parameter or a local
    # stays Integer. The derived predicates below read through the
    # declared READERS for the same reason — which is also how the gem
    # spells them (`number == recordset.page_count`).
    def initialize(scope, number, per_page)
      n = number > 0 ? number : 1
      per = per_page > 0 ? per_page : 1
      # BEFORE the window is applied, for readability — though not for
      # correctness: `Relation#count` renders `count_sql`, which drops
      # LIMIT and OFFSET, so it answers the unwindowed total whichever
      # order these two run in. That is the same total the gem gets from
      # `records.unscope(:limit).unscope(:offset).count`.
      total = scope.count
      @number = n
      @per_page = per
      @records_count = total
      # The gem counts pages by walking the ratios (`while residual > 0;
      # residual -= ratios[count]`), which for a fixed size is a ceiling
      # division. An EMPTY recordset is one page, not zero — the gem's
      # `count > 0 ? count : 1` — so `first?` and `last?` are both true
      # on an empty first page and the view renders no next-page frame.
      @page_count = total <= 0 ? 1 : ((total + per - 1) / per)
      # `limit`/`offset` MUTATE and return the receiver (see the
      # Relation class comment), so this is the caller's own relation
      # windowed in place rather than a copy. Harmless at all three
      # campfire sites — the un-paginated scope is either unused
      # afterwards or already materialized (accounts#edit partitions it
      # first) — and it matches what the caller does with the return
      # value, which is render it.
      @records = scope.limit(per).offset((n - 1) * per)
    end

    def number
      @number
    end

    def per_page
      @per_page
    end

    def records
      @records
    end

    # Rows in the whole recordset, not in this page — the gem's
    # `Recordset#records_count`.
    def records_count
      @records_count
    end

    def page_count
      @page_count
    end

    def first?
      number == 1
    end

    # `==`, not `>=`, which is the gem's own spelling: a `?page=` beyond
    # the end is NOT the last page there, and answering `>=` here would
    # be a divergence dressed as a robustness fix.
    def last?
      number == page_count
    end

    def only?
      page_count == 1
    end

    def before_last?
      number < page_count
    end

    # What the view hands back as `?page=`. The gem's offset portion
    # answers the next page NUMBER (its cursor portion answers an opaque
    # string, which is why the gem renamed `next_number` to this).
    def next_param
      number + 1
    end
  end

  class Base
    # Sets `@page` and answers the windowed relation — the gem's
    # `set_page_and_extract_portion_from`. The assignment is the point:
    # controllers ignore the return value and the VIEW reads `@page`
    # (`@page.records`, `@page.last?`, `@page.next_param`), which is
    # why this is `set_…_and_extract_…` rather than a plain reader.
    def set_page_and_extract_portion_from(records, per_page:)
      paginated = ActionController::Page.new(records, current_page_number, per_page)
      @page = paginated
      paginated.records
    end

    # `params[:page]`, floored at 1 — the gem's `current_page_param`
    # through `page_number_from`. `params` is String-keyed with recursive
    # ParamValue values, so reading a scalar out of it is `Params.str`'s
    # whole job (narrowing in value position; params.rb records which
    # three targets a bare `is_a?` breaks). Going through the helper
    # rather than open-coding `fetch` also keeps this method free of
    # gradual sites: what comes back is a String, not a ParamValue.
    # A missing, blank, or non-scalar `page` reads as page 1, which is
    # what `"".to_i > 0 ? … : 1` gives the gem.
    def current_page_number
      n = Params.str(params, "page", "").to_i
      n > 0 ? n : 1
    end
  end
end
