//! An unsaved owner's has_one / has_many reader (#1337). activerecord
//! 8.1.4 (oracle `unsaved_owner_reader2.rb`): while the owner is a new
//! record without its key, the reader doesn't query, so the has_one
//! answers nil and the has_many is empty, including for `as:`. A row
//! keyed 0 is not the owner's. An owner given its key (a custom
//! `primary_key:` column set before the save) does query, and after the
//! save both readers find the owner's rows.
//!
//! On the ruby family a new record holds `id` 0, and the readers queried
//! `<fk> = 0`, answering the rows keyed 0.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TABLES: &str = "  create_table \"comments\", force: :cascade do |t|";
const NEW_TABLES: &str = "  create_table \"summaries\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n  end\n\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n  end\n\n  create_table \"links\", force: :cascade do |t|\n    t.string \"article_slug\"\n    t.string \"url\"\n  end\n\n  create_table \"places\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n  create_table \"pictures\", force: :cascade do |t|\n    t.string \"imageable_type\"\n    t.integer \"imageable_id\"\n    t.string \"name\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", TABLES, NEW_TABLES)
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.string \"slug\"\n")
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  has_one :summary\n  has_many :notes\n  has_many :links, primary_key: :slug, foreign_key: :article_slug\n",
        )
        .write("app/models/summary.rb", "class Summary < ApplicationRecord\n  belongs_to :article, optional: true\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article, optional: true\nend\n")
        .write("app/models/link.rb", "class Link < ApplicationRecord\nend\n")
        .write(
            "app/models/place.rb",
            "class Place < ApplicationRecord\n  has_one :picture, as: :imageable\n  has_many :pictures, as: :imageable\nend\n",
        )
        .write("app/models/picture.rb", "class Picture < ApplicationRecord\n  belongs_to :imageable, polymorphic: true, optional: true\nend\n")
}

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
Summary.create!(text: "keyed 0", article_id: 0)
Note.create!(text: "keyed 0", article_id: 0)
Picture.create!(imageable_type: "Place", imageable_id: 0, name: "keyed 0")
Link.create!(article_slug: "s", url: "u")
a = Article.new(title: "t", body: "A body long enough to validate.")
check("has_one", a.summary.nil?, true)
check("has_many", a.notes.map(&:text), [])
check("has_many size", [a.notes.size, a.notes.length, a.notes.empty?], [0, 0, true])
check("has_many first", a.notes.first.nil?, true)
p = Place.new(name: "p")
check("has_one as:", p.picture.nil?, true)
check("has_many as:", p.pictures.map(&:name), [])
check("a custom primary_key: set before the save queries", Article.new(slug: "s").links.map(&:url), ["u"])
a.save!
Summary.create!(text: "mine", article_id: a.id)
Note.create!(text: "mine", article_id: a.id)
check("after the save, has_one", a.summary.text, "mine")
check("after the save, has_many", a.notes.map(&:text), ["mine"])
puts "unsaved owner passed"
"##;

#[test]
fn an_unsaved_owners_readers_answer_nothing_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("unsaved owner passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_unsaved_owners_readers_answer_nothing_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("unsaved owner passed"), "{}", run.stdout);
}
