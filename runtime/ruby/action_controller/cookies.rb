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
      raw(key)
    end

    def []=(key, value)
      raw_set(key, value)
    end

    # `cookies.permanent[:k] = v` — expiry is not modeled; permanence is a
    # no-op returning the same jar so the index-assign lands on `[]=`.
    def permanent
      self
    end

    # `cookies.signed[:k]` — a view that signs on the way out and
    # verifies on the way in, delegating storage back here so a write
    # through it lands in the same pending map the dispatcher
    # serializes. Rails hands back a distinct jar object too.
    #
    # A view rather than a flag on this object: a second CookieJar
    # sharing @inbound/@out by assignment widened their element type —
    # the ivars are seeded from a `{}` literal, so a second assignment
    # from a parameter left spinel inferring a poly hash, and `to_h`'s
    # `merge` then had no arm (a NoMethodError at runtime, which is what
    # the spinel framework test caught and `cargo test` could not).
    # Delegation keeps every ivar single-assignment and concrete.
    def signed
      ActionController::SignedCookieJar.new(self)
    end

    # Storage, under the signing view above. `[]`/`[]=` are the same
    # pair without signing.
    def raw(key)
      k = key.to_s
      # @out wins whenever the KEY is present, not whenever its value is
      # non-empty: `delete` records a cleared write as "", and falling
      # through on emptiness would read the deleted cookie's inbound
      # value straight back (the shape cookies_test's
      # delete-from-inside-each guards).
      return @out[k] if @out.key?(k)
      return @inbound[k] if @inbound.key?(k)
      ""
    end

    def raw_set(key, value)
      @out[key.to_s] = value
      value
    end

    # Removing a cookie is recorded as an empty write; the dispatcher emits a
    # cleared Set-Cookie. (No separate tombstone type keeps @out a plain
    # String→String map for the strict typer.)
    def delete(key)
      @out[key.to_s] = ""
      ""
    end

    # Pending writes, for the dispatcher's Set-Cookie serialization. NOT
    # `to_h`: the siblings ActionDispatch::Flash and ActionDispatch::Session
    # both spell the whole store `to_h` and the dispatcher-facing subset
    # something intent-named (`to_persisted` / `to_cookie`), and Rails' own
    # CookieJar#to_h yields every cookie. This is only @out, so it takes the
    # intent name and leaves `to_h` free for the merged view `[]` reads.
    # (Nor `to_set`, the name this carried until 2026-07-25: spinel rewrites
    # any zero-arg `x.to_set` to `Set.new(x.to_a)` whenever a Set class is in
    # the program, overriding the user-defined method — matz/spinel#3378.)
    def pending
      @out
    end

    # The whole jar as a Hash — inbound overlaid by this request's pending
    # writes, i.e. exactly what `[]` reads, materialized. This is the name
    # `pending` above deliberately left free, and it matches Rails, whose
    # CookieJar#to_h also yields every cookie rather than just the writes.
    # (`@inbound.merge(@out)` rather than an empty-literal accumulator: a
    # bare `{}` seed types its values Untyped — the Hash-side accumulator
    # refinement gap — and `merge` of two Hash[String, String] stays
    # concrete, which the fully-typed runtime gate requires.)
    def to_h
      @inbound.merge(@out)
    end

    # Iterate the merged view, as Rails' CookieJar does (it is Enumerable
    # over the same whole-jar view). Yielding over the fresh Hash `to_h`
    # builds — rather than over @inbound/@out directly — is load-bearing,
    # not incidental: the shape this exists for is lobsters'
    # `remove_unknown_cookies`, which calls `cookies.delete(key)` from
    # inside the block, and `delete` records a write into @out. Walking a
    # snapshot keeps that allowed, the way `Hash#each` + `delete` on a dup
    # is; iterating a live store would mutate the collection being walked.
    def each
      to_h.each { |k, v| yield k, v }
      self
    end
  end

  # The `cookies.signed` view: same storage, signed on the way out and
  # verified on the way in. Rails' SignedKeyRotatingCookieJar, minus the
  # rotation.
  #
  # A cookie that does not verify reads as "" — indistinguishable from
  # absent, which is what Rails does (it answers nil and the app treats
  # the request as signed out). That covers a tampered payload, a bad
  # signature, a value signed for a different cookie name, and a string
  # that is not of the form at all.
  class SignedCookieJar
    def initialize(jar)
      @jar = jar
    end

    def [](key)
      raw = @jar.raw(key)
      return "" if raw == ""
      ActionController::MessageVerifier.verified(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        raw, "cookie." + key.to_s, true
      )
    end

    # Rails takes either a bare value or an options Hash carrying
    # `value:` beside `httponly:`/`same_site:` — campfire writes the
    # latter. The transport attributes are not modeled (the dispatcher
    # emits Path=/ + HttpOnly for every cookie it writes), so what
    # survives is the value.
    def []=(key, value)
      signed = ActionController::MessageVerifier.generate(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        ActionController::SignedCookieJar.value_of(value),
        "cookie." + key.to_s, true
      )
      @jar.raw_set(key, signed)
      value
    end

    # `cookies.signed.permanent[:k] = v` — permanence is not modeled
    # (same as the unsigned jar's), so this is the identity that keeps
    # the index-assign landing on `[]=` above.
    def permanent
      self
    end

    def delete(key)
      @jar.delete(key)
    end

    # The value out of either write form. Kept a class method with an
    # untyped parameter rather than an `is_a?` branch inside `[]=`: the
    # options-Hash form carries mixed value types (String, bool, Symbol),
    # and confining the poly read to one place keeps it off the jar's
    # own String-typed surface.
    def self.value_of(value)
      return value[:value].to_s if value.is_a?(Hash)
      value.to_s
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
