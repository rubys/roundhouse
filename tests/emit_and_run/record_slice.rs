//! `record.slice(:id, :title)` answers Rails' `ActiveRecord::Core#slice`: the named attributes, String-keyed. Campfire's typing channel broadcasts `current_user.slice(:id, :name)`, and without the method every typing notification raised.

use super::emit_and_run;

const SUMMARY: &str = "  include Sliceable\n  def summary = slice(:id, :title)\n  def shadowed(title) = slice(:title, \"title\")\n  def by_name(name) = slice(name)\n  def self.no_names_of(id) = find(id).slice\n  def self.slice(*names) = names.join(\",\")\n  def self.class_side = slice(:id, :title)\n";

/// A concern's bare `slice` reads the columns of the model it belongs to.
const CONCERN: &str = "module Article::Sliceable\n  extend ActiveSupport::Concern\n\n  def card = slice(:id, :title)\n  def card_shadowed(title) = slice(:title)\nend\n";

/// A module in the model's namespace whose class methods are not run on
/// a record: its bare `slice` is its own.
const UTILITIES: &str = "module Article::Utilities\n  def self.slice(*names) = names.size\n  def self.count_of = slice(:id, :title)\nend\n";

const ASSERTIONS: &str = r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
a = Article.create!(title: "Sliced", body: "A sufficiently long body.")
check("typed call site", a.summary, { "id" => a.id, "title" => "Sliced" })
check("concern call site", a.card, { "id" => a.id, "title" => "Sliced" })
check("concern shadowing local", a.card_shadowed("Local"), { "title" => "Sliced" })
check("model class method", Article.class_side, "id,title")
check("namespaced module class method", Article::Utilities.count_of, 2)
check("shadowing local", a.shadowed("Local"), { "title" => "Sliced" })
check("run-time name", a.by_name(:title), { "title" => "Sliced" })
check("untyped call site", Article.all.first.slice(:title, :body), { "title" => "Sliced", "body" => "A sufficiently long body." })
check("untyped String and Array names", Article.all.first.slice("title", [:body, :title]), { "title" => "Sliced", "body" => "A sufficiently long body." })
stamp = Article.all.first.slice(:created_at)["created_at"]
check("untyped timestamp is a Time", stamp.is_a?(Time), true)
check("untyped timestamp", stamp, Article.find(a.id).created_at)
check("no names", a.slice, {})
check("no names, found receiver", Article.no_names_of(a.id), {})
begin
  Article.no_names_of(-1)
  raise "an empty slice skipped its receiver"
rescue ActiveRecord::RecordNotFound
end
begin
  a.slice(:id, :comments)
  raise "a name that is not a column answered"
rescue NotImplementedError => e
  check("non-column message", e.message.include?("comments"), true)
end
puts "slice passed"
"##;

fn sliced_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/article/sliceable.rb", CONCERN)
        .write("app/models/article/utilities.rb", UTILITIES)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{SUMMARY}"),
        )
}

#[test]
fn a_records_slice_answers_its_named_columns() {
    let run = sliced_app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("slice passed"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_records_slice_answers_its_named_columns_on_spinel() {
    let run = sliced_app().run_spinel_with_db(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("slice passed"));
}

/// The cross-target probe the strict toolchain suites run, answered here by CRuby.
#[test]
fn the_cross_target_slice_probe_answers_as_rails() {
    let want = super::record_slice_app::EXPECTED.map(|s| format!("{s:?}")).join(", ");
    let run = super::record_slice_app::probe_app().run_ruby(&format!(
        "got = Article.sliced\nraise \"sliced: #{{got.inspect}}\" unless got == [{want}]\nputs \"probe passed\"\n"
    ));
    run.assert_passes();
    assert!(run.stdout.contains("probe passed"));
}

/// The concern's `slice` is lowered against its model, not left to the
/// runtime: the emitted method builds the Hash from reader calls.
#[test]
fn a_concerns_bare_slice_lowers_against_its_model() {
    let (emitted, errors) = sliced_app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let concern = std::fs::read_to_string(emitted.join("app/models/article/sliceable.rb")).expect("emitted concern");
    assert!(concern.contains("\"title\" => self.title"), "{concern}");
    assert!(!concern.contains("slice("), "{concern}");
}
