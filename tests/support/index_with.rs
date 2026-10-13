//! One `index_with` contract shared by the interpreted and native lanes.
//!
//! `index_with` is ActiveSupport's `Enumerable` reopen, the mirror of
//! `index_by`: each element becomes a key, and the value is the argument
//! or what the block answers. The expected values are what Rails 8.1
//! answers for the same calls. A class-body constant is the case that
//! mattered: on Spinel an ungrounded `index_with` left the constant
//! "defined nowhere", with no `check` diagnostic.
//!
//! Lowering rewrites only Array / Relation / Tuple receivers — a Hash
//! (or a gradual receiver that may be one) yields pairs to `index_with`
//! but keys alone to `to_h`. The contract therefore pins Array-typed
//! call sites (literals); method-param receivers stay unsupported until
//! typed.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord",
        r#"class Article < ApplicationRecord
  UNSET = %w[draft live].index_with(nil).freeze
  SIZES = %w[a bb].index_with(&:size)

  def self.counts
    %w[x yy].index_with { |n| n.length * 10 }
  end

  def self.zeroes
    %w[p].index_with(0)
  end

  def self.empty_zeroes
    [].index_with(0)
  end
"#,
    )
}

pub const ASSERTIONS: &str = r#"
raise "value form: #{Article::UNSET.inspect}" unless Article::UNSET == { "draft" => nil, "live" => nil }
raise "symbol block: #{Article::SIZES.inspect}" unless Article::SIZES == { "a" => 1, "bb" => 2 }
raise "block: #{Article.counts.inspect}" unless Article.counts == { "x" => 10, "yy" => 20 }
raise "value: #{Article.zeroes.inspect}" unless Article.zeroes == { "p" => 0 }
raise "empty: #{Article.empty_zeroes.inspect}" unless Article.empty_zeroes == {}
puts "index_with contract passed"
"#;
