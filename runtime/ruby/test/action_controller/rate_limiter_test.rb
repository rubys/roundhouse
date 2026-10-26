# frozen_string_literal: true
require_relative "../test_helper"
require_relative "../../rails"
require_relative "../../action_controller/rate_limiter"

# `ActionController::RateLimiter.exceeded?` — the counter behind
# `rate_limit to:, within:` — and the `Rails::Cache#increment` it
# counts with. RUBY-FAMILY ONLY: the store is `runtime/ruby/rails.rb`'s,
# which the strict-target runtimes do not stage.
class RateLimiterTest < Minitest::Test
  def setup
    @cache = Rails::Cache.new
  end

  def test_increment_counts_from_one_within_a_window
    assert_equal 1, @cache.increment("rate-limit:t:a", 1, expires_in: 60)
    assert_equal 2, @cache.increment("rate-limit:t:a", 1, expires_in: 60)
    assert_equal 3, @cache.increment("rate-limit:t:a", 1, expires_in: 60)
    # Another key is another window.
    assert_equal 1, @cache.increment("rate-limit:t:b", 1, expires_in: 60)
  end

  def test_increment_adds_its_amount
    assert_equal 5, @cache.increment("n", 5, expires_in: 60)
    assert_equal 7, @cache.increment("n", 2, expires_in: 60)
  end

  def test_an_expired_window_starts_over
    @cache.increment("rate-limit:t:a", 1, expires_in: 1)
    @cache.increment("rate-limit:t:a", 1, expires_in: 1)
    # Expiry is lazy, on read, at whole-second resolution: a 1 s window
    # written at second N is gone at second N + 1.
    sleep 1.1
    assert_equal 1, @cache.increment("rate-limit:t:a", 1, expires_in: 1)
  end

  def test_exceeded_is_the_count_after_this_request_against_the_cap
    # `to: 2` allows two requests; the third is over.
    refute ActionController::RateLimiter.exceeded?(@cache, "rate-limit:sessions:x", 60, 2)
    refute ActionController::RateLimiter.exceeded?(@cache, "rate-limit:sessions:x", 60, 2)
    assert ActionController::RateLimiter.exceeded?(@cache, "rate-limit:sessions:x", 60, 2)
    assert ActionController::RateLimiter.exceeded?(@cache, "rate-limit:sessions:x", 60, 2)
  end

  # The count is kept in the store it is handed — the controller's
  # `cache_store` — and nowhere else.
  def test_exceeded_counts_in_the_store_it_is_given
    Rails.cache.forget("rate-limit:sessions:y")
    ActionController::RateLimiter.exceeded?(@cache, "rate-limit:sessions:y", 60, 2)
    assert_nil Rails.cache.read_str("rate-limit:sessions:y")
    assert_equal "1", @cache.read_str("rate-limit:sessions:y")
  end
end
