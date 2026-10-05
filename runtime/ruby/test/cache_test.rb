require_relative "test_helper"

# `Rails::Cache#read_str` / `#write_str` — fragment-cache store.
# RUBY-FAMILY ONLY: lives in `runtime/ruby/rails.rb`, which strict-target
# runtimes do not stage.
class CacheStrTest < Minitest::Test
  def setup
    @cache = Rails::Cache.new
  end

  def test_read_str_miss_and_hit
    assert_nil @cache.read_str("views/fragment/a")
    @cache.write_str("views/fragment/a", "<p>hi</p>", 0)
    assert_equal "<p>hi</p>", @cache.read_str("views/fragment/a")
  end

  def test_empty_string_is_a_hit_not_a_miss
    @cache.write_str("views/empty", "", 0)
    assert_equal "", @cache.read_str("views/empty")
  end

  def test_ttl_zero_never_expires_on_read
    @cache.write_str("views/stable", "cached", 0)
    # Campfire fragment caches use ttl 0; entry must survive without expiry.
    assert_equal "cached", @cache.read_str("views/stable")
  end

  def test_non_string_key_is_coerced_like_to_s
    sym = :"views/sym-key"
    @cache.write_str(sym, "from-symbol", 0)
    assert_equal "from-symbol", @cache.read_str("views/sym-key")
  end

  def test_expired_entry_is_forgotten_on_read
    @cache.write_str("views/short", "old", 1)
    sleep 1.1
    assert_nil @cache.read_str("views/short")
    assert_nil @cache.read_str("views/short")
  end
end
