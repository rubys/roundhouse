//! A finder condition whose value is an Array normalizes each element
//! (activerecord 8.1.4, oracle `rails-oracle.rb`): with `normalizes
//! :title, with: ->(title) { title.strip.downcase }`, `where(title: ["
//! ALPHA ", "Beta "])` finds both "alpha" and "beta", from a literal or
//! from a local. A nil element stays nil, so `[..., nil]` still matches
//! `IS NULL`, and it does so under `apply_to_nil: true` too: a finder
//! never normalizes nil (`where(note: [nil])` is `"note" IS NULL`, so
//! it doesn't find the record whose assigned nil was stored as "n/a").
//!
//! The whole Array went to the normalizer: `_normalize_title(["
//! ALPHA ", "Beta "])` normalized the Array's `to_s` (CRuby found
//! nothing), and Spinel's `--rbs .` refused an Array in its String
//! parameter.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const ARTICLE: &str = "  has_many :comments, dependent: :destroy\n  normalizes :title, with: ->(title) { title.strip.downcase }\n  normalizes :note, with: ->(note) { note || \"n/a\" }, apply_to_nil: true\n\n  def self.titled_literal\n    where(title: [\" ALPHA \", \"Beta \"]).count\n  end\n\n  def self.titled_local\n    titles = [\" ALPHA \", \"Beta \"]\n    where(title: titles).count\n  end\n\n  def self.titled_or_untitled\n    where(title: [\"ALPHA\", nil]).count\n  end\n\n  def self.first_titled\n    find_by(title: [\" BETA \"])\n  end\n\n  def self.unnoted\n    where(note: [nil]).count\n  end\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
body = "A body long enough to validate."
Article.create!(title: "Alpha", note: "x", body: body)
Article.create!(title: "beta", note: nil, body: body)
check("a literal Array", Article.titled_literal, 2)
check("an Array in a local", Article.titled_local, 2)
check("a nil element", Article.titled_or_untitled, 1)
check("find_by", Article.first_titled.title, "beta")
check("apply_to_nil leaves a nil element", Article.unnoted, 0)
puts "array conditions passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.string \"note\"\n")
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", ARTICLE)
}

#[test]
fn an_array_condition_normalizes_each_element_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("array conditions passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_array_condition_normalizes_each_element_on_spinel_with_rbs() {
    let run = app().run_spinel_with_rbs(&format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{SCRIPT}"
    ));
    run.assert_passes();
    assert!(run.stdout.contains("array conditions passed"), "{}", run.stdout);
}
