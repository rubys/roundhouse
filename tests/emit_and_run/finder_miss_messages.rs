//! A finder miss words its message as Rails 8.1.4 does
//! (`core.rb#find`, `FinderMethods#raise_record_not_found_exception!`).
//! Only the forms the runtime reproduces exactly are pinned here: a
//! scoped relation's message drops Rails' ` [WHERE ...]` suffix, which
//! Rails renders from Arel with `?` binds.

use super::emit_and_run;

const SCRIPT: &str = r##"article = Article.create!(title: "Present", body: "A sufficiently long body.")
def message(label)
  yield
  raise "#{label}: no RecordNotFound"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
{
  "find string" => [message("find string") { Article.find("999") }, "Couldn't find Article with 'id'=\"999\""],
  "find integer" => [message("find integer") { Article.find(999) }, "Couldn't find Article with 'id'=999"],
  "find nil" => [message("find nil") { Article.find(nil) }, "Couldn't find Article without an ID"],
  "relation find array" => [message("relation find array") { ActiveRecord::Relation.new(Article).find([article.id, 999]) }, "Couldn't find all Articles with 'id': (#{article.id}, 999) (found 1 results, but was looking for 2)."],
  "relation find nil" => [message("relation find nil") { ActiveRecord::Relation.new(Article).where(title: "Present").find(nil) }, "Couldn't find Article without an ID"],
  "unscoped relation find" => [message("unscoped relation find") { ActiveRecord::Relation.new(Article).find(999) }, "Couldn't find Article with 'id'=999"],
  "first!" => [message("first!") { ActiveRecord::Relation.new(Comment).first! }, "Couldn't find Comment"],
  "sole" => [message("sole") { ActiveRecord::Relation.new(Comment).sole }, "Couldn't find Comment"],
}.each do |label, (got, want)|
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
puts "messages match"
"##;

#[test]
fn a_finder_miss_uses_rails_wording() {
    let run = emit_and_run::real_blog().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("messages match"), "{}", run.stdout);
}
