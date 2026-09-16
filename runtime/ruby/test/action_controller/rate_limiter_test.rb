require_relative "../test_helper"
require_relative "../../rails"
require_relative "../../action_controller/rate_limiter"

# `ActionController::RateLimiter.exceeded?` — the counter behind
# `rate_limit to:, within:` — and the `Rails::Cache#increment_str` it
# counts with. RUBY-FAMILY ONLY: the store is `runtime/ruby/rails.rb`'s,
# which the strict-target runtimes do not stage.
class RateLimiterTest < Minitest::Test
  def setup
    @cache = Rails::Cache.new
  end

  def test_increment_counts_from_one_within_a_window
    assert_equal 1, @cache.increment_str("rate-limit:t:a", 60)
    assert_equal 2, @cache.increment_str("rate-limit:t:a", 60)
    assert_equal 3, @cache.increment_str("rate-limit:t:a", 60)
    # Another key is another window.
    assert_equal 1, @cache.increment_str("rate-limit:t:b", 60)
  end

  def test_an_expired_window_starts_over
    @cache.increment_str("rate-limit:t:a", 1)
    @cache.increment_str("rate-limit:t:a", 1)
    # Expiry is lazy, on read, at whole-second resolution: a 1 s window
    # written at second N is gone at second N + 1.
    sleep 1.1
    assert_equal 1, @cache.increment_str("rate-limit:t:a", 1)
  end

  def test_exceeded_is_the_count_after_this_request_against_the_cap
    Rails.cache.forget("rate-limit:sessions:x")
    # `to: 2` allows two requests; the third is over.
    refute ActionController::RateLimiter.exceeded?("rate-limit:sessions:x", 60, 2)
    refute ActionController::RateLimiter.exceeded?("rate-limit:sessions:x", 60, 2)
    assert ActionController::RateLimiter.exceeded?("rate-limit:sessions:x", 60, 2)
    assert ActionController::RateLimiter.exceeded?("rate-limit:sessions:x", 60, 2)
  end
end
