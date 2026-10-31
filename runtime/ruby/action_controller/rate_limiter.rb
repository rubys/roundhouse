# frozen_string_literal: true
# `ActionController::RateLimiting` (actionpack 7.2+), the question behind
# `rate_limit to: 10, within: 3.minutes`: has this key been seen more
# than `to` times in the current window?
#
#   exceeded?  =  the window's count, after this request, > to
#
# The count lives in the declared `store:`, or the controller's class-level
# `cache_store` (`Rails.cache` unless the app sets another), Rails' default,
# under the key the lowering builds — `rate-limit:<controller_path>
# [:<name>]:<by>`, Rails' own shape — and the window is the entry's
# TTL: the first request in a window writes it with `within` seconds to
# live, and every later one increments it until it expires. The call is
# actionpack's own, `store.increment(key, 1, expires_in: within)`, so a
# test that stubs `SomeController.cache_store.stubs(:increment)` is
# answered by its stub.
#
# Called from the private method the lowering synthesizes (see
# `ingest::rate_limit`); the `with:` block — what to do when the answer
# is yes — is the app's, and runs in that method.
require_relative "../rails"

module ActionController
  module RateLimiter
    def self.exceeded?(store, key, within, to)
      count = store.increment(key, 1, expires_in: within)
      return false if count.nil?
      count > to
    end
  end
end
