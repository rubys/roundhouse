//! The source mapping must work in the emitted program, not just ingest.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Serialized values, NOT Sorbet member names, supply Rails' labels and
/// storage. Keep a same-named decoy enum and an unchanged record so scopes
/// and persisted bang writes cannot pass by returning a constant result.
#[test]
fn sorbet_serialized_enum_mapping_persists_and_scopes() {
    emit_and_run::real_blog()
        .write(
            "app/services/z_article_ratings.rb",
            r#"module ArticleRatings
  class Rating < T::Enum
    enums do
      High = new("severe")
      Low = new("mild")
    end
  end
end
"#,
        )
        .write(
            "app/services/a_decoy.rb",
            r#"module Decoy
  class Rating < T::Enum
    enums do
      High = new("urgent")
      Low = new("calm")
    end
  end
end
"#,
        )
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"severity\", default: \"mild\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :severity, ArticleRatings::Rating.values.to_h { |v| [v.serialize, v.serialize] }",
        )
        .write(
            "test/models/article_sorbet_enum_test.rb",
            r#"require "test_helper"

class ArticleSorbetEnumTest < ActiveSupport::TestCase
  test "serialized members drive stored values and generated methods" do
    assert_equal({"severe" => "severe", "mild" => "mild"}, Article.severities)
    assert_equal "severe", Article.severities[:severe]
    assert_equal "severe", ArticleRatings::Rating::High.serialize
    assert_equal "mild", ArticleRatings::Rating::Low.serialize
    assert_equal "urgent", Decoy::Rating::High.serialize
    article = articles(:one)
    other = articles(:two)
    assert article.mild?
    assert_not article.severe?
    article.severe!
    assert_equal "severe", ActiveRecord.adapter.find("articles", article.id)["severity"]
    assert_equal "severe", Article.find(article.id).severity
    assert article.reload.severe?
    assert_not article.mild?
    assert_equal article.id, Article.severe.first.id
    assert_equal other.id, Article.mild.first.id
    article.mild!
    assert_equal "mild", ActiveRecord.adapter.find("articles", article.id)["severity"]
    assert_equal "mild", Article.find(article.id).severity
    assert_nil Article.severe.first
  end
end
"#,
        )
        .run_test("test/models/article_sorbet_enum_test.rb")
        .assert_passes();
}

/// Exercise the real fixture/emission path, not just the miniature ingestion
/// tree. A mutating initializer must fail before stale mappings are emitted.
#[test]
fn initializer_mutation_refuses_emitting_stale_persisted_values() {
    let result = std::panic::catch_unwind(|| {
        emit_and_run::real_blog()
            .write("app/services/rating.rb", r#"class Rating < T::Enum
  enums do
    High = new("severe")
    Low = new("mild")
  end
end
"#)
            .write("config/initializers/rating.rb", r#"module WrongSerialization
  def serialize
    "critical"
  end
end
Rating.prepend(WrongSerialization)
"#)
            .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
                "create_table \"articles\", force: :cascade do |t|\n    t.string \"severity\", default: \"mild\"")
            .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
                "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :severity, Rating.values.to_h { |v| [v.serialize, v.serialize] }")
            .run_ruby(r#"article = Article.create!(title: "Guard probe", body: "An independent persistence probe.")
article.severe!
raise "not stored severe" unless ActiveRecord.adapter.find("articles", article.id)["severity"] == "severe"
raise "initializer not applied" unless Rating::High.serialize == "critical"
"#)
    });
    match result {
        Ok(run) => {
            run.assert_passes();
            panic!("emitted stale severe storage despite critical serialization");
        }
        Err(error) => {
            let message = error.downcast_ref::<String>().map(String::as_str)
                .or_else(|| error.downcast_ref::<&str>().copied()).unwrap_or("");
            assert!(message.contains("enum :severity mapping"), "unexpected refusal: {message}");
        }
    }
}

/// A class-method override of a sorbet method that calls `super`
/// (`Channel.try_deserialize` normalizes the value first)
/// reaches sorbet's own lookup, and the methods built on it
/// (`from_serialized`) dispatch to the override.
#[test]
fn an_override_calling_super_reaches_the_sorbet_method() {
    emit_and_run::real_blog()
        .write(
            "app/services/channel.rb",
            r#"class Channel < T::Enum
  enums do
    Web = new("web")
    Unknown = new("unknown")
  end

  class << self
    def try_deserialize(value)
      value = value&.downcase
      value = "web" if ["legacy_web", "mobile_web"].include?(value)
      super(value)
    end

    def has_serialized?(value)
      super || value == "legacy"
    end
  end
end
"#,
        )
        .run_ruby(
            r#"raise "override lost" unless Channel.try_deserialize("MOBILE_WEB").equal?(Channel::Web)
raise "lookup lost" unless Channel.try_deserialize("unknown").equal?(Channel::Unknown)
raise "nil lost" unless Channel.try_deserialize("nope").nil?
raise "from_serialized skips override" unless Channel.from_serialized("Legacy_Web").equal?(Channel::Web)
raise "bare super lost" unless Channel.has_serialized?("web") && Channel.has_serialized?("legacy") && !Channel.has_serialized?("x")
"#,
        )
        .assert_passes();
}

/// A bare `super` in an override with keyword parameters cannot be routed
/// to the synthesized method, so ingest names it instead of emitting a
/// `super` that finds nothing.
#[test]
fn an_override_with_keywords_calling_super_is_refused() {
    let source = b"class Channel < T::Enum\n  enums do\n    Web = new(\"web\")\n  end\n\n  def self.try_deserialize(value, strict: false)\n    super\n  end\nend\n";
    let error = roundhouse::ingest::ingest_library_classes(source, "app/services/channel.rb")
        .err()
        .expect("refused")
        .to_string();
    assert!(error.contains("`super` in `try_deserialize`"), "{error}");
}
