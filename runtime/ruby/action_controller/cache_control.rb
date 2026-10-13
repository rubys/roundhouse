# `response.cache_control` — Rails' mixed-Hash surface
# (`{public: true, max_age: 31556952}`) over the typed
# `ActionController::CacheControlStore` in base.rb.
#
# RUBY-FAMILY ONLY, like `cookies.rb` beside this file: a Hash-
# subscript surface on Base must NOT transpile to the strict targets,
# which reach Cache-Control only through `expires_in` (base.rb — see
# the comment there). Required by the `action_controller.rb`
# aggregator, which the ruby/jruby/spinel trees follow; the strict
# targets emit their runtime from the `runtime_loader` tables and
# never see this file.
#
# `replace(private: true, no_store: true)` in a
# `before_action :set_cache_control_defaults` filter is the shape
# rubys/roundhouse#679 asked for; `merge!`/`[]=`/`delete` round out
# Rails' surface. `commit_cache_control!` is the wire-side half —
# called once per request, right before each dispatcher copies
# `controller.headers` onto the outbound response, in every wire path
# that carries this file (the CRuby overlay's `main.rb`, the spinel
# scaffold's `main.rb`, and the spinel test harness).
module ActionController
  class CacheControlStore
    # `store[:max_age]` / `store[:max_age] = 60` — the subscript
    # surface Rails' own Hash answers. A bool flag reads `true` when
    # set and nil otherwise — NEVER `false`: Rails' Hash never carries
    # the key at all when the response is private, which is the shape
    # a bare `if cache_control[:public]` check relies on. A stated
    # `max_age` / `stale_while_revalidate` / `stale_if_error` answers
    # its Integer even when that Integer is 0 — a stated zero is still
    # a stated key in Rails' Hash, same as the typed reader beside
    # this one.
    def [](key)
      case key
      when :public then public? ? true : nil
      when :private then private? ? true : nil
      when :no_store then no_store? ? true : nil
      when :no_cache then no_cache? ? true : nil
      when :must_revalidate then must_revalidate? ? true : nil
      when :must_understand then must_understand? ? true : nil
      when :immutable then immutable? ? true : nil
      when :max_age then max_age? ? max_age : nil
      when :stale_while_revalidate then stale_while_revalidate? ? stale_while_revalidate : nil
      when :stale_if_error then stale_if_error? ? stale_if_error : nil
      when :extras then extras
      else nil
      end
    end

    # Dispatches to the typed setter for `key`: truthiness for the
    # seven bool flags (`store[:public] = nil` clears, same as Rails
    # deleting the key), `.to_i` for the three Integer fields (`nil`
    # clears the presence bool via the matching `clear_*` instead of
    # stating a 0), and a direct assign for `:extras`.
    #
    # An unrecognized key RAISES rather than silently doing nothing:
    # every other Rails option this store does not model is a call
    # that should be loud at the call site, the same discipline
    # `pagination.rb`'s keyword-only surface follows ("omitting the
    # keyword means such a call raises ArgumentError — loud, at the
    # call site, naming the keyword").
    def []=(key, value)
      case key
      when :public then self.public = value
      when :private then self.private = value
      when :no_store then self.no_store = value
      when :no_cache then self.no_cache = value
      when :must_revalidate then self.must_revalidate = value
      when :must_understand then self.must_understand = value
      when :immutable then self.immutable = value
      when :max_age
        value.nil? ? clear_max_age : self.max_age = value.to_i
      when :stale_while_revalidate
        value.nil? ? clear_stale_while_revalidate : self.stale_while_revalidate = value.to_i
      when :stale_if_error
        value.nil? ? clear_stale_if_error : self.stale_if_error = value.to_i
      when :extras
        self.extras = value.nil? ? [] : value
      else
        raise ArgumentError, "unrecognized Cache-Control option #{key.inspect}"
      end
      value
    end

    # Rails' `Hash#delete` — returns the key's prior `[]` reading
    # (nil for a key that was never set) and clears it the same way
    # `[]= key, nil` does.
    def delete(key)
      old = self[key]
      self[key] = nil
      old
    end

    # `response.cache_control.merge!(no_store: true, public: true)` —
    # every entry rides through `[]=`, so each follows the same
    # truthiness/`.to_i`/clear rules a single subscript write would.
    # Spinel supports a trailing `**opts` kwrest the same way
    # `GlobalID::Locator.locate_signed` does.
    def merge!(**opts)
      opts.each { |k, v| self[k] = v }
      self
    end

    # `response.cache_control.replace(private: true, no_store: true)`
    # — campfire-style `before_action :set_cache_control_defaults`
    # filters open with this. Rails' own `replace` is a wholesale
    # swap (clear, then take every entry of the argument), not a
    # merge onto whatever the store already held.
    def replace(**opts)
      clear
      merge!(**opts)
    end
  end

  class Base
    # `response.cache_control` — the Hash-like store itself, so a
    # filter can write `response.cache_control.replace(...)` and an
    # action can write `response.cache_control[:public] = true`
    # without either naming `response` twice.
    #
    # The nil-guard is never live (`initialize`, in base.rb, always
    # constructs the store), but it IS what lets this file's own flow
    # typer resolve `@cache_control`'s type — this reopen never
    # assigns the ivar otherwise, and runtime_src's per-file ivar
    # typing is flow-based (seeded from assignments it can see in
    # THIS file), not read from base.rbs' cross-file declaration.
    # Same idiom `cookies.rb` uses for `@cookies` beside base.rb's
    # eager `@session`.
    def cache_control
      @cache_control = ActionController::CacheControlStore.new if @cache_control.nil?
      @cache_control
    end

    # `response.cache_control`'s current state as a plain Hash —
    # exactly the Symbol-keyed subset `[]` answers non-nil for,
    # `:extras` included only when it is non-empty (matching the
    # store's own `empty?`). Feeds `commit_cache_control!`'s merge.
    def cache_control_as_hash
      store = cache_control
      hash = {}
      keys = [:public, :private, :no_store, :no_cache, :must_revalidate,
              :must_understand, :immutable, :max_age,
              :stale_while_revalidate, :stale_if_error]
      keys.each do |key|
        value = store[key]
        hash[key] = value unless value.nil?
      end
      extras = store[:extras]
      hash[:extras] = extras unless extras.nil? || extras.empty?
      hash
    end

    # Writes the composed `Cache-Control` header — merging whatever
    # `response.cache_control` STATES over whatever the action wrote
    # directly via `headers["Cache-Control"] = …`, the way Rails'
    # `merge_and_normalize_cache_control!` does (actionpack 8.1.4,
    # `ActionDispatch::Http::Cache::Response#before_committed`): an
    # action that writes `headers["Cache-Control"] = "private,
    # no-store"` and ALSO calls `expires_in 60, public: true` — two
    # independent writes, same request — must not have the typed
    # store silently overwritten by whichever one happened to run
    # last, and must not have the store's own directives silently
    # lost under the hand-written header either. Rails' answer, which
    # this mirrors exactly: parse the existing header, delete any
    # `no-cache`/`no-store` it carried (`expires_in`/`replace`/
    # `merge!` always win over those two — same rule `expires_in`
    # itself already applies to a PRIOR `no_store`), merge the store's
    # stated directives on top, fold `:extras` from both sides
    # (store's own first, existing's appended, deduplicated), and
    # render with the normal three-branch order.
    #
    # An untouched store (`response.cache_control` never read or
    # written) leaves a directly-written header RENORMALIZED but
    # otherwise unchanged — same as Rails: a header already in
    # canonical order round-trips byte for byte. A store with nothing
    # stated AND no existing header leaves `headers["Cache-Control"]`
    # unset, same as before this method existed.
    #
    # Called once per request, right before the header copy, by every
    # wire path that requires this file.
    def commit_cache_control!
      control = ActionController.parse_cache_control_header(@headers["Cache-Control"])
      stated = cache_control_as_hash
      return if control.empty? && stated.empty?

      unless stated.empty?
        control.delete(:no_cache)
        control.delete(:no_store)
        existing_extras = control.delete(:extras)
        unless existing_extras.nil?
          combined = []
          unless stated[:extras].nil?
            stated[:extras].each { |e| combined << e }
          end
          existing_extras.each { |e| combined << e }
          merged = []
          combined.each { |e| merged << e unless merged.include?(e) }
          stated[:extras] = merged
        end
        stated.each { |key, value| control[key] = value }
      end

      options = []
      if control[:no_store]
        options << "private" if control[:private]
        options << "must-understand" if control[:must_understand]
        options << "no-store"
      elsif control[:no_cache]
        options << "public" if control[:public]
        options << "no-cache"
        unless control[:extras].nil?
          control[:extras].each { |e| options << e }
        end
      else
        options << "max-age=#{control[:max_age].to_i}" unless control[:max_age].nil?
        options << (control[:public] ? "public" : "private")
        options << "must-revalidate" if control[:must_revalidate]
        unless control[:stale_while_revalidate].nil?
          options << "stale-while-revalidate=#{control[:stale_while_revalidate].to_i}"
        end
        unless control[:stale_if_error].nil?
          options << "stale-if-error=#{control[:stale_if_error].to_i}"
        end
        options << "immutable" if control[:immutable]
        unless control[:extras].nil?
          control[:extras].each { |e| options << e }
        end
      end

      @headers["Cache-Control"] = options.join(", ")
    end
  end

  # Rails' own directive names `cache_control_headers`
  # (ActionDispatch::Http::Cache::Response) recognizes BY NAME rather
  # than folding into `:extras` — deliberately narrower than this
  # store's own field list. Verified running actionpack 8.1.4: a
  # pre-existing header naming `immutable`, `stale-while-revalidate=N`
  # or `stale-if-error=N` is NOT parsed into those fields; it rides
  # through as a literal `:extras` entry, same as any directive Rails
  # itself does not recognize. This mirrors that gap exactly rather
  # than "fixing" it — a difference here would make an existing header
  # round-trip differently than it does under Rails.
  CACHE_CONTROL_SPECIAL_KEYS = ["no-store", "no-cache", "max-age", "public",
                                "private", "must-revalidate", "must-understand"]

  # Parses an existing `Cache-Control` header value the same way
  # Rails' `cache_control_headers` does: comma-separated segments
  # (spaces stripped first), each a bare flag or `directive=value`;
  # one of `CACHE_CONTROL_SPECIAL_KEYS` lands under its own
  # (underscored) Symbol key — a bare flag's value is `true` — and
  # anything else rides in as a literal String under `:extras`.
  def self.parse_cache_control_header(header)
    parsed = {}
    return parsed if header.nil? || header.empty?
    stripped = header.delete(" ")
    stripped.split(",").each do |segment|
      directive, argument = segment.split("=", 2)
      if CACHE_CONTROL_SPECIAL_KEYS.include?(directive)
        key = directive.tr("-", "_")
        parsed[key.to_sym] = argument.nil? ? true : argument
      else
        parsed[:extras] = [] if parsed[:extras].nil?
        parsed[:extras] << segment
      end
    end
    parsed
  end
end
