# Independent Rails reference; run with ruby tests/support/enum_default_reference.rb.
gem "activerecord", "8.1.4"
require "active_record"
require "active_record/fixtures"
require "tmpdir"
require "minitest/autorun"

ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.define do
  create_table :articles do |t|
    t.string :title
    t.integer :state, default: 0
    t.string :severity, default: "mild", null: false
    t.integer :priority
    t.boolean :flag, default: true
    t.json :metadata
  end
end

class DirectBase < ActiveRecord::Base
  self.abstract_class = true
  self.table_name = "articles"
  enum :state, { draft: 0, live: 3 }
end

module LocalState
  extend ActiveSupport::Concern
  included { enum :state, { queued: 2, shipped: 7 } }
end

class SpecialArticle < DirectBase
  include LocalState
end

class Article < DirectBase
  enum :severity, { mild: "mild", severe: "severe" }, default: :severe
  enum :priority, { low: -3, high: 7 }, default: :high
end

class EnumDefaultReference < Minitest::Test
  def test_defaults_and_raw_hydration_are_not_user_assignments
    assert_nil SpecialArticle.new.state
    assert_equal "draft", Article.new.state
    shipped = SpecialArticle.create!(state: :shipped, flag: false)
    assert_equal "shipped", SpecialArticle.find(shipped.id).state
    assert_equal false, SpecialArticle.find(shipped.id).flag
    assert_equal 7, ActiveRecord::Base.connection.select_value("SELECT state FROM articles WHERE id = #{shipped.id}")
    assert_nil SpecialArticle.new(state: nil).state
    assert_raises(ArgumentError) { SpecialArticle.new(state: 0) }
    assert_nil SpecialArticle.new(state: false).state
    assert_raises(ArgumentError) { SpecialArticle.new(state: :draft) }
    ordinary = Article.create!(title: "Raw default")
    loaded = SpecialArticle.find(ordinary.id)
    assert_nil loaded.state
    assert_equal 0, loaded.state_before_type_cast
    assert_equal 0, ActiveRecord::Base.connection.select_value("SELECT state FROM articles WHERE id = #{ordinary.id}")
    assert_equal "draft", ordinary.reload.state
  end

  def test_fixture_defaults_are_schema_defaults
    assert_equal "severe", Article.new.severity
    assert_equal "high", Article.new.priority
    assert_equal "mild", Article.new(severity: :mild).severity
    assert_nil Article.new(priority: nil).priority
    assert_equal false, Article.new(flag: false).flag
    value = { "name" => "Mixed", "enabled" => false }
    mixed = Article.create!(metadata: value, flag: false)
    assert_equal "severe", mixed.severity
    assert_equal "high", mixed.priority
    assert_equal value, mixed.metadata
    assert_equal value, Article.find(mixed.id).metadata
    assert_equal false, Article.find(mixed.id).flag
    assert_equal '{"name":"Mixed","enabled":false}', ActiveRecord::Base.connection.select_value("SELECT metadata FROM articles WHERE id = #{mixed.id}")
    Dir.mktmpdir("enum-default-reference") do |dir|
      File.write(File.join(dir, "articles.yml"), <<~YAML)
        one:
          title: First fixture
        two:
          title: Explicit fixture
          severity: severe
          priority: -3
          flag: false
      YAML
      ActiveRecord::FixtureSet.create_fixtures(dir, "articles")
    end
    one = Article.find_by!(title: "First fixture")
    two = Article.find_by!(title: "Explicit fixture")
    assert_equal "mild", one.severity
    assert_nil one.priority
    assert_equal "severe", two.severity
    assert_equal "low", two.priority
    assert_equal false, two.flag
    assert_equal "mild", ActiveRecord::Base.connection.select_value("SELECT severity FROM articles WHERE id = #{one.id}")
    assert_nil ActiveRecord::Base.connection.select_value("SELECT priority FROM articles WHERE id = #{one.id}")
    assert_equal -3, ActiveRecord::Base.connection.select_value("SELECT priority FROM articles WHERE id = #{two.id}")
    assert_nil one.metadata
    assert_nil ActiveRecord::Base.connection.select_value("SELECT metadata FROM articles WHERE id = #{one.id}")
  end
end
