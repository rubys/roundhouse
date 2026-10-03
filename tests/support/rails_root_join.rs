//! One `Rails.root.join` contract shared by the interpreted and native output lanes.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord",
        r#"class Article < ApplicationRecord
  def self.source_root
    Rails.root.join.to_s
  end

  def self.source_dir
    Rails.root.join("source", "posts").to_s
  end

  def self.source_file
    Rails.root.join("source", "posts", "index.md").to_s
  end

  def self.storage_dir
    (Rails.root + "storage").to_s
  end
"#,
    )
}

pub const ASSERTIONS: &str = r#"
root = Rails.root.to_s
raise "no parts: #{Article.source_root.inspect}" unless Article.source_root == root
raise "two parts: #{Article.source_dir.inspect}" unless Article.source_dir == root + "/source/posts"
raise "three parts: #{Article.source_file.inspect}" unless Article.source_file == root + "/source/posts/index.md"
raise "plus: #{Article.storage_dir.inspect}" unless Article.storage_dir == root + "/storage"
puts "Rails.root.join contract passed"
"#;
