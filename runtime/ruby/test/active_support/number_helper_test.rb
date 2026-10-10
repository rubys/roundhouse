require_relative "../test_helper"

# Rails 8.1.4 public-interface cases for both number-helper surfaces.
class ActiveSupportNumberHelperTest < Minitest::Test
  def test_number_to_delimited_groups_signed_integer_and_fraction
    assert_equal "-1,234,567.89",
      ActiveSupport::NumberHelper.number_to_delimited("-1234567.89")
    assert_equal "1.234.567,89",
      ActiveSupport::NumberHelper.number_to_delimited("1234567.89", delimiter: ".", separator: ",")
  end

  def test_number_to_delimited_honors_custom_delimiter_pattern
    pattern = /\d(?=(\d{3})+\z)/
    assert_equal "123 456 789",
      ActiveSupport::NumberHelper.number_to_delimited("123456789", delimiter: " ", delimiter_pattern: pattern)
  end

  def test_number_to_rounded_fixed_and_nil_precision
    assert_equal "1.200",
      ActiveSupport::NumberHelper.number_to_rounded("1.2")
    assert_equal "1.2300",
      ActiveSupport::NumberHelper.number_to_rounded("1.2300", precision: nil)
    assert_equal "1.234,50",
      ActiveSupport::NumberHelper.number_to_rounded("1234.5", precision: 2,
        delimiter: ".", separator: ",")
  end

  def test_number_to_rounded_round_modes_match_rails
    assert_equal "-12.35",
      ActiveSupport::NumberHelper.number_to_rounded("-12.345", precision: 2, round_mode: :up)
    assert_equal "-12.34",
      ActiveSupport::NumberHelper.number_to_rounded("-12.345", precision: 2, round_mode: :down)
    assert_equal "12.34",
      ActiveSupport::NumberHelper.number_to_rounded("12.345", precision: 2, round_mode: :half_even)
    assert_equal "-12.35",
      ActiveSupport::NumberHelper.number_to_rounded("-12.341", precision: 2, round_mode: :floor)
  end

  def test_number_to_rounded_significant_digits_and_zero_stripping
    assert_equal "12000",
      ActiveSupport::NumberHelper.number_to_rounded(12345.6789, precision: 2, significant: true)
    assert_equal "1.2",
      ActiveSupport::NumberHelper.number_to_rounded(1.2, precision: 3, strip_insignificant_zeros: true)
    assert_equal "10",
      ActiveSupport::NumberHelper.number_to_rounded("9.99", precision: 2, significant: true)
    assert_equal "0.333",
      ActiveSupport::NumberHelper.number_to_rounded(Rational(1, 3), precision: 3)
    assert_equal "$0.13",
      ActiveSupport::NumberHelper.number_to_currency(Rational(1, 8), precision: 2)
  end

  def test_all_active_support_public_formatters_match_the_rails_oracle_cases
    helper = ActiveSupport::NumberHelper
    assert_equal "($1,234.51)",
      helper.number_to_currency("-1234.505", precision: 2, negative_format: "(%u%n)")
    assert_equal "1.23 Million", helper.number_to_human(1_234_567, precision: 3)
    assert_equal "1.18 MB", helper.number_to_human_size(1_234_567)
    assert_equal "100%", helper.number_to_percentage("99.999", precision: 0)
    assert_equal "+1-123-555-1234 x 55",
      helper.number_to_phone("1235551234", country_code: 1, extension: 55)
    assert_equal "-12.35",
      helper.number_to_rounded("-12.345", precision: 2, round_mode: :up)
  end

  def test_documented_options_cover_each_formatter
    helper = ActiveSupport::NumberHelper
    assert_equal "1.234,5 €", helper.number_to_currency(1234.5,
      unit: "€", format: "%n %u", precision: 1, delimiter: ".", separator: ",")
    assert_equal "$1.23", helper.number_to_currency("1.2300", precision: nil)
    assert_equal "$1,234.5", helper.number_to_currency("1234.505", precision: 2,
      round_mode: :down, strip_insignificant_zeros: true)
    assert_equal "1,23,456.78", helper.number_to_delimited("123456.78",
      delimiter: ",", delimiter_pattern: /(\d+?)(?=(\d\d)+\d(?!\d))/)
    assert_equal "123,5 Thousand", helper.number_to_human(123456,
      precision: 4, separator: ",", delimiter: ".")
    assert_equal "130 Thousand", helper.number_to_human(123456,
      precision: 2, round_mode: :up)
    assert_equal "[1.234/Million]", helper.number_to_human(1234567,
      precision: 4, format: "[%n/%u]", round_mode: :down,
      strip_insignificant_zeros: false)
    assert_equal "1_235 Quadrillion", helper.number_to_human("1234567890123456789",
      precision: 4, delimiter: "_")
    assert_equal "1.250 KB", helper.number_to_human_size(1280,
      precision: 3, significant: false, strip_insignificant_zeros: false)
    assert_equal "1,18 MB", helper.number_to_human_size(1234567, separator: ",")
    assert_equal "1 Byte", helper.number_to_human_size("1.5")
    assert_equal "-1 Bytes", helper.number_to_human_size("-1.5")
    assert_equal "105000000 ZB",
      helper.number_to_human_size("123456789012345678901234567890")
    assert_equal "105_000_000 ZB",
      helper.number_to_human_size("123456789012345678901234567890", delimiter: "_")
    assert_equal "130 KB", helper.number_to_human_size(123456,
      precision: 2, round_mode: :up)
    assert_equal "12000%", helper.number_to_percentage(12345.6789,
      precision: 2, significant: true)
    assert_equal "[12.3456789]", helper.number_to_percentage(12.3456789,
      precision: nil, format: "[%n]")
    assert_equal "[12,34]", helper.number_to_percentage("12.349", precision: 2,
      round_mode: :down, strip_insignificant_zeros: true, delimiter: ".",
      separator: ",", format: "[%n]")
    assert_equal "12.345,68%", helper.number_to_percentage("12345.678", precision: 2,
      delimiter: ".", separator: ",")
    assert_equal "(755) 6123.4567", helper.number_to_phone(75561234567,
      pattern: /(\d{1,4})(\d{4})(\d{4})$/, area_code: true, delimiter: ".")
    assert_equal "12.34", helper.number_to_rounded(12.34,
      precision: 4, strip_insignificant_zeros: true)
    assert_equal "12.000", helper.number_to_rounded(12345.6789,
      precision: 2, significant: true, delimiter: ".", separator: ",")
  end

  def test_nil_and_invalid_inputs_match_active_support_boundaries
    helper = ActiveSupport::NumberHelper
    assert_nil helper.number_to_currency(nil)
    assert_nil helper.number_to_delimited(nil)
    assert_nil helper.number_to_human(nil)
    assert_nil helper.number_to_human_size(nil)
    assert_nil helper.number_to_percentage(nil)
    assert_nil helper.number_to_phone(nil)
    assert_nil helper.number_to_rounded(nil)

    assert_equal "$12x34", helper.number_to_currency("12x34")
    assert_equal "-$abc", helper.number_to_currency("-abc")
    assert_equal "12x34", helper.number_to_delimited("12x34")
    assert_equal "12x34", helper.number_to_human("12x34")
    assert_equal "12x34", helper.number_to_human_size("12x34")
    assert_equal "12x34%", helper.number_to_percentage("12x34")
    assert_equal "12x34", helper.number_to_phone("12x34")
    assert_equal "12x34", helper.number_to_rounded("12x34")
  end

  def test_human_units_and_phone_patterns
    assert_equal "1 km",
      ActiveSupport::NumberHelper.number_to_human(1_000, units: { unit: "m", thousand: "km" })
    assert_equal "1km",
      ActiveSupport::NumberHelper.number_to_human(1_000,
        units: { unit: "m", thousand: "km" }, format: "%n%u")
    assert_equal "5 dm",
      ActiveSupport::NumberHelper.number_to_human("0.5", units: { unit: "m", deci: "dm" })
    assert_equal "133-1234-5678",
      ActiveSupport::NumberHelper.number_to_phone("13312345678", pattern: /(\d{3})(\d{4})(\d{4})$/)
  end

  def test_locale_registry_changes_defaults_and_rejects_unknown_locales
    ActiveSupport::NumberHelper.register_locale(:de_test, ",", ".")
    assert_equal "1.234,5",
      ActiveSupport::NumberHelper.number_to_delimited("1234.5", locale: :de_test)
    error = assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_delimited("1234", locale: :not_registered)
    end
    assert_match(/unsupported number locale/, error.message)
  end

  def test_registered_locale_translations_feed_every_locale_sensitive_formatter
    ActiveSupport::NumberHelper.register_locale(:fr_test, ",", " ", {
      currency: { unit: "€", format: "%n %u", negative_format: "-%n %u", precision: 2,
                 separator: ",", delimiter: " " },
      percentage: { format: "%n %", precision: 2, delimiter: "" },
      human: {
        format: { delimiter: "", precision: 3, significant: true, strip_insignificant_zeros: true },
        decimal_units: { format: "%n %u", units: { unit: "", thousand: "Millier", million: "Million" } },
        storage_units: { format: "%n %u", units: { byte: "Octets", kb: "Ko", mb: "Mo" } },
      },
    })
    helper = ActiveSupport::NumberHelper
    assert_equal "1 234,50 €", helper.number_to_currency(1234.5, locale: :fr_test)
    assert_equal "1,23 Million", helper.number_to_human(1_234_567, locale: :fr_test)
    assert_equal "1,18 Mo", helper.number_to_human_size(1_234_567, locale: :fr_test)
    assert_equal "12,50 %", helper.number_to_percentage(12.5, precision: 2, locale: :fr_test)
  end

  def test_action_view_facade_raises_for_invalid_input_and_escapes_html
    view = Class.new { include ActionView::Helpers::NumberHelper }.new
    assert_equal "$1,234.50", view.number_to_currency(1234.5)
    assert_equal "1.23 Million", view.number_to_human(1_234_567)
    assert_equal "1.18 MB", view.number_to_human_size(1_234_567)
    assert_equal "100%", view.number_to_percentage("99.999", precision: 0)
    assert_equal "+1-123-555-1234 x 55",
      view.number_to_phone("1235551234", country_code: 1, extension: 55)
    assert_equal "1,234", view.number_with_delimiter(1234)
    assert_equal "1234.50", view.number_with_precision(1234.5, precision: 2)
    assert_equal "&lt;b&gt;1.00",
      view.number_to_currency(1, unit: "<b>")
    invalid_html = view.number_to_currency("<script>")
    assert_equal "$<script>", invalid_html
    assert_equal String, invalid_html.class
    assert_equal "-$abc", view.number_to_currency("-abc")
    assert_equal "&lt;b&gt;", view.number_to_phone("<b>")
    assert_nil view.number_to_phone(nil)

    invalid_calls = [
      -> { view.number_to_currency("12x34", raise: true) },
      -> { view.number_to_human("12x34", raise: true) },
      -> { view.number_to_human_size("12x34", raise: true) },
      -> { view.number_to_percentage("12x34", raise: true) },
      -> { view.number_to_phone("12x34", raise: true) },
      -> { view.number_with_delimiter("12x34", raise: true) },
      -> { view.number_with_precision("12x34", raise: true) },
    ]
    invalid_calls.each do |call|
      assert_raises(ActionView::Helpers::NumberHelper::InvalidNumberError) do
        call.call
      end
    end
    error = assert_raises(ActionView::Helpers::NumberHelper::InvalidNumberError) do
      view.number_to_currency("12x34", raise: true)
    end
    assert_equal "12x34", error.number
    assert_equal "ActionView::Helpers::NumberHelper::InvalidNumberError", error.message
    assert_nil view.number_to_currency(nil)
    assert_nil view.number_to_human(nil)
    assert_nil view.number_to_human_size(nil)
    assert_nil view.number_to_percentage(nil)
    assert_nil view.number_to_phone(nil)
    assert_nil view.number_with_delimiter(nil)
    assert_nil view.number_with_precision(nil)
  end

  def test_unsupported_or_unknown_options_fail_instead_of_being_ignored
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_delimited("1234", mystery: true)
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_human_size(1234, format: "%n")
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_human(1234, units: :missing_scope)
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_phone("1235551234", locale: :en)
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_delimited("1234", delimiter_pattern: false)
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_rounded("1.2", round_mode: false)
    end
    assert_raises(ArgumentError) do
      ActiveSupport::NumberHelper.number_to_human(1000, units: { unknown: "ignored" })
    end
  end
end
