# Controller-level cookie access — Rails' `cookies` CookieJar. `cookies[:k]`
# reads the inbound cookie; `cookies[:k] = v` (and `cookies.permanent[:k] = v`)
# records a write the dispatcher serializes as Set-Cookie.
#
# Ruby-family only, like current.rb beside this file: a CookieJar-typed field
# on Base must NOT transpile to the strict targets (they don't exercise
# cookies), so it lives in a reopen outside the strict-target tables rather
# than in the universal base.rb. Required by the action_controller.rb
# aggregator, which the ruby/jruby/spinel trees follow (the strict targets
# emit their runtime from tables and never see it).
#
# Keys normalize to String — exactly as ActionDispatch::Session does — so
# both CRuby's symbolized inbound cookies and spinel's string-keyed
# `Tep.str_hash` route through one store. This replaces the former CRuby-only
# overlay CookieJar; one typed implementation now serves all three ruby-family
# targets.
module ActionController
  class CookieJar
    def initialize(inbound = {})
      @inbound = {}
      @out = {}
      # Copy via `.each` (pair iteration), not `.keys`: the inbound hash is
      # the request's `Tep.str_hash` (a `Hash.new("")`), whose `.keys`
      # intrinsic yields a null array through the loosely-typed `req.cookies`
      # accessor. `.each` normalizes Symbol keys (CRuby) to String so both
      # the symbolized CRuby inbound and spinel's string-keyed hash share
      # one store.
      inbound.each { |k, v| @inbound[k.to_s] = v }
    end

    # A missing cookie reads as "" (not nil): every call site coerces with
    # `.to_s`, and a non-null String keeps the value off spinel's nullable-
    # String path (where `cookies[k].to_s.split(",")` otherwise yields a
    # null array). Rails returns nil here; "" is equivalent under `.to_s`.
    def [](key)
      k = key.to_s
      return @out[k] if @out.key?(k)
      return @inbound[k] if @inbound.key?(k)
      ""
    end

    def []=(key, value)
      @out[key.to_s] = value
      value
    end

    # `cookies.permanent[:k] = v` — expiry is not modeled; permanence is a
    # no-op returning the same jar so the index-assign lands on `[]=`.
    def permanent
      self
    end

    # Removing a cookie is recorded as an empty write; the dispatcher emits a
    # cleared Set-Cookie. (No separate tombstone type keeps @out a plain
    # String→String map for the strict typer.)
    def delete(key)
      @out[key.to_s] = ""
      ""
    end

    # Pending writes, for the dispatcher's Set-Cookie serialization.
    def to_set
      @out
    end
  end

  class Base
    # Lazily initialised so a read before the dispatcher assigns the real jar
    # (e.g. a unit-constructed controller) still returns a usable empty
    # CookieJar rather than nil — mirrors base.rb's eager `@session`, but from
    # a reopen so the CookieJar type stays off the strict targets.
    def cookies
      @cookies = ActionController::CookieJar.new if @cookies.nil?
      @cookies
    end

    def cookies=(value)
      @cookies = value
      value
    end
  end
end
