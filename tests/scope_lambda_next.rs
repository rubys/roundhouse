//! A scope lambda's early `next` is the class method's `return`.
//!
//! A scope body becomes `def self.<name>(…, __rel = …)`, where the
//! lambda's own `next v` (`next all if term.blank?`) does not parse in a
//! method. The body's own `next` now becomes `return` with the same
//! value; one in a nested block or a loop still ends that block's
//! iteration (a `while` body included).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCOPES: &str = r#"  scope :titled, ->(t) { next all if t.nil?; where(title: t) }
  scope :long_or_none, lambda { |flag|
    next none unless flag

    picked = []
    [1, 2, 3].each do |n|
      next if n == 2

      picked << n
    end
    where(id: all.map(&:id).first(picked.size))
  }
  scope :first_skipping, lambda { |skip|
    next none if skip.nil?

    i = 0
    kept = []
    while i < 3
      i += 1
      next if i == skip

      kept << i
    end
    where(id: all.map(&:id).first(kept.size))
  }

  validates :title, presence: true"#;

#[test]
fn a_scope_lambdas_next_returns_from_the_scope() {
    let run = emit_and_run::real_blog()
        .edit("app/models/article.rb", "  validates :title, presence: true", SCOPES)
        .run_ruby(
            r#"
Article.delete_all
%w[one two three].each { |t| Article.create!(title: t, body: "Long enough body") }
raise "titled(nil): #{Article.titled(nil).count}" unless Article.titled(nil).count == 3
raise "titled(two): #{Article.titled("two").map(&:title)}" unless Article.titled("two").map(&:title) == ["two"]
raise "long_or_none(false): #{Article.long_or_none(false).count}" unless Article.long_or_none(false).count == 0
raise "long_or_none(true): #{Article.long_or_none(true).count}" unless Article.long_or_none(true).count == 2
raise "first_skipping(nil): #{Article.first_skipping(nil).count}" unless Article.first_skipping(nil).count == 0
raise "first_skipping(2): #{Article.first_skipping(2).count}" unless Article.first_skipping(2).count == 2
puts "scope next"
"#,
        );
    let source =
        std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap_or_default();
    let parse = ruby_prism::parse(source.as_bytes());
    let parse_errors: Vec<String> = parse.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:?} in:\n{source}");
    run.assert_passes();
    assert_eq!(run.stdout, "scope next\n");
}
