//! An enum mapping that assigns its constants as it is built.
//!
//! A mapping may define a constant for each label as it is written:
//!
//! ```ruby
//! enum :status, { pending: PENDING = "pending", … }
//! ```
//!
//! The stored value was carried as the whole `PENDING = "pending"`
//! expression into the generated predicate, bang writer and scopes,
//! where a constant assignment does not parse ("dynamic constant
//! assignment"), and the constants were never defined in the class
//! body. The writes are now class-body statements ahead of the enum,
//! and each label stores the written value.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn enum_mapping_constant_writes_define_the_constants() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true",
            "  enum :title, {\n    one: ONE = \"one\",\n    two: TWO = \"two\",\n  }\n\n  def self.second\n    TWO\n  end\n\n  validates :title, presence: true",
        )
        .run_ruby(
            r#"
raise "ONE: #{Article::ONE.inspect}" unless Article::ONE == "one"
raise "second: #{Article.second.inspect}" unless Article.second == "two"
Article.delete_all
a = Article.create!(title: "one", body: "Long enough body")
raise "one?: #{a.title.inspect}" unless a.one? && !a.two?
raise "scope one: #{Article.one.count}" unless Article.one.count == 1 && Article.two.count == 0
a.two!
raise "two!: #{a.reload.title.inspect}" unless a.reload.title == "two" && Article.two.count == 1
puts "enum constants"
"#,
        );
    let source =
        std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap_or_default();
    let parse = ruby_prism::parse(source.as_bytes());
    let parse_errors: Vec<String> = parse.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:?} in:\n{source}");
    run.assert_passes();
    assert_eq!(run.stdout, "enum constants\n");
}

/// The comment above an `enum` rides its first class-body item, which is
/// the first constant write, so it stays above the constants.
#[test]
fn the_enums_comment_stays_above_its_constant_writes() {
    use roundhouse::ModelBodyItem;

    let source = br#"
class Article < ApplicationRecord
  # Titles with a fixed vocabulary.
  enum :title, {
    one: ONE = "one",
    two: TWO = "two",
  }
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema, &Default::default())
        .unwrap()
        .unwrap();

    let ModelBodyItem::Unknown { leading_comments, .. } = &model.body[0] else {
        panic!("expected the first constant write first, got {:?}", model.body[0]);
    };
    assert_eq!(leading_comments.len(), 1);
    assert_eq!(leading_comments[0].text.as_str(), "# Titles with a fixed vocabulary.");
    assert!(matches!(model.body[1], ModelBodyItem::Unknown { .. }));
    for item in &model.body[1..] {
        assert!(item.leading_comments().is_empty(), "comment duplicated on {item:?}");
    }
}
