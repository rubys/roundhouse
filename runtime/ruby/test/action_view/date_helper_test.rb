require_relative "../test_helper"

# Direct unit tests for the date helpers in
# `runtime/ruby/action_view/view_helpers_ext.rb`.
#
# RUBY-FAMILY ONLY, deliberately — same reasoning as
# `action_controller/cookies_test.rb` beside it. `view_helpers_test.rb`
# in this directory IS wired into the crystal lane, so these cases
# cannot live there: the entry computation is `Time - Time`, which
# doesn't transpile uniformly (Crystal yields Time::Span, the JVM wants
# java.time.Duration), and `view_helpers_ext.rb` is deliberately absent
# from runtime_loader's strict-target tables for exactly that reason.
# Hence a separate file, referenced only by `framework_tests_ruby` and
# `framework_tests_spinel`.
#
# The expected strings are actionview's built-in :en wording, not
# paraphrases — per-route output parity against a real Rails lobsters
# depends on the exact text ("about 1 hour", "less than a minute").
#
# Every call is spelled out rather than routed through a local `dist`
# helper: the spinel lane's test transpile synthesizes an RBS for each
# test-class method, and a helper whose body is a single call to
# another file's module function comes back `-> nil`, which turns every
# `assert_equal "str", dist(...)` into an unsupported String-vs-nil
# comparison. Calling the runtime directly keeps the return typed.
class ActionViewDateHelperTest < Minitest::Test
  # A fixed base instant. `distance_of_time_in_words` is a pure function
  # of its two arguments, so nothing here reads the clock — the one
  # exception is time_ago_in_words, tested separately at the bottom.
  def setup
    @t = Time.utc(2020, 6, 15, 12, 0, 0)
  end

  # ── the sub-minute / minutes buckets ────────────────────────
  # Rails rounds to whole minutes first, so 0-29s reads as zero minutes
  # and 30-89s as one.

  def test_zero_distance_is_less_than_a_minute
    assert_equal "less than a minute",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t)
  end

  def test_twenty_nine_seconds_is_less_than_a_minute
    assert_equal "less than a minute",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 29)
  end

  # 30..89s all round to exactly 1 minute, which is still inside the
  # 0..1 bucket and so reads "1 minute"; 90s rounds to 2 and falls
  # through to the plural bucket. Pin both sides of that edge.
  def test_sixty_seconds_is_one_minute
    assert_equal "1 minute",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 60)
  end

  def test_eighty_nine_seconds_is_still_one_minute
    assert_equal "1 minute",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 89)
  end

  def test_ninety_seconds_crosses_into_two_minutes
    assert_equal "2 minutes",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 90)
  end

  def test_five_minutes
    assert_equal "5 minutes",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 300)
  end

  def test_forty_four_minutes_is_still_minutes
    assert_equal "44 minutes",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 2640)
  end

  # ── hours / days / months / years ───────────────────────────

  def test_forty_five_minutes_rounds_to_about_one_hour
    assert_equal "about 1 hour",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 2700)
  end

  def test_two_hours
    assert_equal "about 2 hours",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 7200)
  end

  def test_one_day
    assert_equal "1 day",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 86400)
  end

  def test_three_days
    assert_equal "3 days",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 259200)
  end

  def test_one_month_is_singular
    assert_equal "about 1 month",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 2592000)
  end

  def test_three_months
    assert_equal "3 months",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 7776000)
  end

  def test_one_year
    assert_equal "about 1 year",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 31536000)
  end

  # ── argument order ──────────────────────────────────────────
  # Rails swaps the pair when they arrive reversed, so the result is the
  # absolute distance. lobsters relies on this: it passes
  # `(Time.zone.now, created_at + delay)` without ordering them.

  def test_reversed_arguments_give_the_same_distance
    assert_equal "5 minutes",
      ActionView::ViewHelpers.distance_of_time_in_words(@t + 300, @t)
  end

  # ── include_seconds ─────────────────────────────────────────
  # Off by default; only consulted inside the 0..1 minute bucket.

  def test_include_seconds_refines_the_sub_minute_bucket
    assert_equal "less than 5 seconds",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 3, include_seconds: true)
    assert_equal "half a minute",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 25, include_seconds: true)
  end

  def test_include_seconds_does_not_affect_larger_buckets
    assert_equal "5 minutes",
      ActionView::ViewHelpers.distance_of_time_in_words(@t, @t + 300, include_seconds: true)
  end

  # ── time_ago_in_words ───────────────────────────────────────
  # Reads the clock, so assert only what a wall-clock-relative call can
  # guarantee rather than pinning a bucket a slow CI box could cross
  # mid-test.

  def test_time_ago_in_words_measures_back_from_now
    assert_equal "less than a minute",
      ActionView::ViewHelpers.time_ago_in_words(Time.now)
    assert_equal "about 2 hours",
      ActionView::ViewHelpers.time_ago_in_words(Time.now - 7200)
  end
end
