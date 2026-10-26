//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). Same harness.
//!
//! On the ruby family a model's `initialize` calls `super(attrs)`
//! rather than `super()` when the only `initialize` above it is
//! `ActiveRecord::Base`'s (src/lower/lazy_model_state.rs). Base's
//! `initialize(_attrs = {})` ignores the argument, but `super()` made it
//! evaluate its `{}` default: one Hash per record built, which on
//! campfire's room page is every hydrated message, user and rich text.
//! Where a parent between the model and Base defines `initialize`, that
//! parent would read the argument, so `super()` stays as written.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

#[test]
fn a_model_under_application_record_forwards_attrs_to_base() {
    let (emitted, _errors) = emit_and_run::real_blog().emit(BuildTarget::Ruby);
    let article = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb emitted");
    assert!(article.contains("    super(attrs)\n"), "Article#initialize must forward attrs to Base:\n{article}");
    assert!(!article.contains("    super()\n"), "Article#initialize still calls a bare super():\n{article}");

    emit_and_run::real_blog()
        .run_ruby(
            r##"
a = Article.new(title: "x", body: "Long enough body")
raise "attrs: #{a.title.inspect}" unless a.title == "x" && a.new_record?
b = Article.new
raise "bare: #{b.title.inspect}" unless b.title.nil? && b.errors.empty?
Article.delete_all
Article.create!(title: "Saved", body: "Long enough body")
raise "load" unless Article.first.title == "Saved"
# Base's default Hash is no longer built: Article.new is the record,
# its errors Arrays and its column defaults, nothing for the argument.
3.times { Article.new }
before = GC.stat(:total_allocated_objects)
1000.times { Article.new }
per = (GC.stat(:total_allocated_objects) - before) / 1000.0
baseline = 1 + 2 + Article.new.instance_variables.count { |iv| (v = Article.new.instance_variable_get(iv)).is_a?(String) && !v.frozen? }
raise "#{per} objects per Article.new, want at most #{baseline}" unless per.round <= baseline
puts "forwarded"
"##,
        )
        .assert_passes();
}

#[test]
fn a_parent_with_its_own_initialize_keeps_bare_super() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n\n  def initialize(attrs = {})\n    super()\n  end\nend\n",
        )
        .emit(BuildTarget::Ruby);
    let article = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb emitted");
    assert!(
        !article.contains("    super(attrs)\n"),
        "a parent that defines initialize would read the forwarded attrs:\n{article}"
    );
}
