# CRuby-only cookie support.
#
# `cookies` is a controller feature lobsters uses but the blog does not.
# Rather than carry a CookieJar in the shared, transpiled
# runtime/ruby/action_controller/base.rb (where it would have to satisfy
# every strict target's type system for a feature none of them exercise
# yet), it lives here and ships only to the CRuby/JRuby trees via the Ruby
# overlay. When lobsters is brought up on another target, that target gets
# its own CookieJar implementation.
#
# Keys are Symbols throughout (the request parser symbolizes cookie names,
# and controllers index with Symbol constants like `cookies[:tag_filters]`),
# so no key normalization is needed under CRuby's dynamic typing.
module ActionController
  class CookieJar
    def initialize(inbound = {})
      @inbound = inbound
      @out = {}
    end

    def [](key)
      @out.key?(key) ? @out[key] : @inbound[key]
    end

    def []=(key, value)
      @out[key] = value
    end

    # `cookies.permanent[:k] = v` — expiry isn't modeled; permanence is a
    # no-op that returns the same jar so the index-assign lands here.
    def permanent
      self
    end

    def delete(key)
      @out[key] = nil
    end

    # Pending writes, for the dispatcher's Set-Cookie serialization.
    # Iterate the request's effective cookie set — inbound overlaid
    # with same-request writes (lobsters' remove_unknown_cookies walks
    # every cookie to delete unrecognized keys).
    def each(&block)
      merged = {}
      @inbound.each { |k, v| merged[k] = v }
      @out.each { |k, v| merged[k] = v }
      merged.each(&block)
      nil
    end

    def to_set
      @out
    end
  end

  # Add the `cookies` accessor to the (already-defined) base controller.
  class Base
    attr_accessor :cookies
  end
end
