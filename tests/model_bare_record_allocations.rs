//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). Same harness.
//!
//! On the ruby family a record built bare, which is how every hydration
//! path starts one (`Model.new`, then a writer per column), allocates
//! itself and its two error lists, and no String per column: a column
//! that `attrs` does not name starts as a shared frozen `""`
//! (src/lower/lazy_model_state.rs), which the hydration writers then
//! replace. Those blanks were 120 of 3,258 objects per request on
//! campfire's room page.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

#[test]
fn a_bare_record_makes_no_string_per_column() {
    let (emitted, _errors) = emit_and_run::real_blog().emit(BuildTarget::Ruby);
    let article = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb emitted");
    assert!(
        article.contains("attrs[:created_at] || \"\".freeze"),
        "a column's blank default must be the shared frozen literal:\n{article}"
    );

    emit_and_run::real_blog()
        .run_ruby(
            r##"
3.times { Article.new }
before = GC.stat(:total_allocated_objects)
1000.times { Article.new }
per = (GC.stat(:total_allocated_objects) - before) / 1000.0
# The record, its `errors` and its `error_details`.
raise "#{per} objects per Article.new, want 3" unless per.round == 3

bad = Article.new(title: "", body: "")
raise "an invalid record passed" if bad.valid?
raise "messages: #{bad.errors.inspect}" if bad.errors.empty?
raise "details: #{bad.error_details.inspect}" unless bad.error_details.length == bad.errors.length
raise "save of an invalid record" if bad.save

Article.delete_all
Article.create!(title: "Saved", body: "Long enough body")
loaded = Article.first
raise "loaded: #{loaded.errors.inspect}" unless loaded.errors.empty? && loaded.valid?
loaded.title = ""
raise "an emptied title passed" if loaded.valid?
raise "loaded messages" if loaded.errors.empty?
puts "bare"
"##,
        )
        .assert_passes();
}
