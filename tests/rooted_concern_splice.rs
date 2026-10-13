//! A concern's `included do validates ... end` reaches a model that
//! includes it by a rooted path (`include ::Blog::Concerns::X`) or by a
//! relative path written inside the concern's namespace
//! (`module Blog; class Post; include Concerns::X`). The splice keyed the
//! include by its written path, so the rooted spelling matched no concern,
//! the validation vanished from the emitted model, and nothing said so.
//! Rails runs the `included` block on the includer either way: an invalid
//! record is rejected.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONCERN: &str = "module Blog\n  module Concerns\n    module HeadlineRequired\n      extend ActiveSupport::Concern\n\n      included do\n        validates :title, presence: true\n      end\n    end\n  end\nend\n";
const ROOTED: &str = "module Blog\n  class RootedPost < ApplicationRecord\n    self.table_name = \"articles\"\n    include ::Blog::Concerns::HeadlineRequired\n  end\nend\n";
const RELATIVE: &str = "module Blog\n  class RelativePost < ApplicationRecord\n    self.table_name = \"articles\"\n    include Concerns::HeadlineRequired\n  end\nend\n";

const SCRIPT: &str = r##"
[Blog::RootedPost, Blog::RelativePost].each do |model|
  invalid = model.new(title: "", body: "long enough body")
  raise "#{model}: blank title accepted" if invalid.valid?
  raise "#{model}: blank title saved" if invalid.save
  raise "#{model}: errors #{invalid.errors.to_a.inspect}" unless invalid.errors.to_a == ["Title can't be blank"]
  valid = model.new(title: "Hello", body: "long enough body")
  raise "#{model}: valid record rejected: #{valid.errors.to_a.inspect}" unless valid.save
end
"##;

/// Native Rails control: the same three files on ActiveRecord over an
/// in-memory sqlite `articles` table.
const NATIVE_PRELUDE: &str = r#"
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define { create_table(:articles) { |t| t.string :title; t.text :body } }
class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

#[test]
fn concern_validation_reaches_rooted_and_relative_includers() {
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("{NATIVE_PRELUDE}\n{CONCERN}\n{ROOTED}\n{RELATIVE}\n{SCRIPT}"))
        .output()
        .expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/models/blog/concerns/headline_required.rb", CONCERN)
        .write("app/models/blog/rooted_post.rb", ROOTED)
        .write("app/models/blog/relative_post.rb", RELATIVE)
        .run_ruby(SCRIPT)
        .assert_passes();
}
