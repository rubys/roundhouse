# Load the gem before test_helper prepends runtime/ruby, whose local i18n.rb
# intentionally shadows the Rails dependency for transpiled applications.
require "rubygems"
require File.join(Gem::Specification.find_by_name("i18n").full_gem_path, "lib/i18n")
require_relative "test_helper"
require "active_support/inflector"
require "active_support/core_ext/integer/inflections"

class Integer
  alias_method :rails_814_ordinalize, :ordinalize
end

# Rails 8.1.4 default-config differential only. Custom acronym tables,
# `classify`/`tableize`, locale mutation, constantization, transliteration,
# and parameterization catalogs remain explicit unsupported boundaries.
module ActiveSupport::Inflector
  class << self
    alias_method :rails_814_camelize, :camelize
    alias_method :rails_814_deconstantize, :deconstantize
    alias_method :rails_814_foreign_key, :foreign_key
    alias_method :rails_814_upcase_first, :upcase_first
    alias_method :rails_814_downcase_first, :downcase_first
  end
end

require_relative "../active_support_inflections"

class ActiveSupportInflectionsTest < Minitest::Test
  def test_camelize_matches_rails_default_identifier_behavior
    [
      ["foo_bar", true], ["foo_bar", false], ["HTMLParser", true],
      ["HTMLParser", false], ["foo/bar", true], ["foo/bar", false],
      ["/foo/bar", true], ["::foo", true], ["foo__bar", true],
      ["_foo", false], ["foo_1", true], ["foo-foo", true],
      ["foo_BAR", true], ["foo/BAR", true], ["HTML_PARSER", true],
      ["foo_BAR-Baz", true], ["", true]
    ].each do |text, uppercase_first_letter|
      expected = ActiveSupport::Inflector.rails_814_camelize(text, uppercase_first_letter)
      assert_equal expected, ActiveSupport::Inflector.camelize(text, uppercase_first_letter),
        "camelize(#{text.inspect}, #{uppercase_first_letter.inspect})"
    end
  end

  def test_deconstantize_matches_rails
    ["foo", "Foo::Bar", "::Foo", "Foo::", "::", "A::B::C", ""].each do |text|
      expected = ActiveSupport::Inflector.rails_814_deconstantize(text)
      assert_equal expected, ActiveSupport::Inflector.deconstantize(text),
        "deconstantize(#{text.inspect})"
    end
  end

  def test_foreign_key_matches_rails_and_honors_separator_option
    ["Message", "Foo::Bar", "Admin::APIKey", "foo/bar"].each do |text|
      [true, false].each do |separate|
        expected = ActiveSupport::Inflector.rails_814_foreign_key(text, separate)
        assert_equal expected, ActiveSupport::Inflector.foreign_key(text, separate),
          "foreign_key(#{text.inspect}, #{separate.inspect})"
      end
    end
  end

  def test_first_character_case_helpers_match_rails
    ["hello", "WORLD", "", "aBC"].each do |text|
      assert_equal ActiveSupport::Inflector.rails_814_upcase_first(text),
        ActiveSupport::Inflector.upcase_first(text), "upcase_first(#{text.inspect})"
      assert_equal ActiveSupport::Inflector.rails_814_downcase_first(text),
        ActiveSupport::Inflector.downcase_first(text), "downcase_first(#{text.inspect})"
    end
  end

  def test_ordinalize_matches_rails_integer_rules
    [-113, -103, -23, -13, -12, -11, -3, -2, -1, 0, 1, 2, 3, 4, 10, 11, 12, 13, 14, 20, 21, 22, 23, 101, 111, 112, 113].each do |number|
      expected = number.rails_814_ordinalize
      assert_equal expected, ActiveSupport::Inflector.ordinalize(number), "ordinalize(#{number})"
    end
  end
end
