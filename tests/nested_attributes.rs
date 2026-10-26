//! `accepts_nested_attributes_for` (#1335, part 1: the writer and building
//! new records). activerecord 8.1.4 (oracles `nested_attributes.rb`,
//! `nested_attributes2.rb`): `<assoc>_attributes=` builds a record per
//! entry (an Array of hashes, or a hash of hashes) for a has_many, one for
//! a has_one or belongs_to, including several associations named in one
//! call and one declared in a concern's `included do`; the records are
//! saved with the parent, a belongs_to one first; an invalid one fails the
//! parent's save with nothing written, its error under the association
//! ("Comments body can't be blank").
//!
//! Part 2 (oracle `nested_attributes3.rb`): an entry with an `id` updates
//! that record of the association (string or Integer id; `RecordNotFound`,
//! with Rails' message, when it is not one), and under `allow_destroy:
//! true` a truthy `_destroy` ("1", "true", "t", "on", `true`; not "0",
//! "false", "f") destroys it: an entry without an `id` so marked builds
//! nothing, and a belongs_to so destroyed leaves the parent's key nil.
//! Without `allow_destroy`, `_destroy` is ignored and the other keys apply.
//!
//! Polymorphic `as:` (oracle `nested_attributes5.rb`): a has_one or
//! has_many `as: :addressable` (with `class_name:`, or from a concern)
//! writes `addressable_id` and `addressable_type`, the type being the
//! owner's `polymorphic_name` (an STI subclass writes its base class); an
//! id is looked up among the owner's own records, so one of another owner
//! or owner type raises `RecordNotFound`. A custom `primary_key:` stays
//! reported.
//!
//! Replacement and required destroys (oracle `nested_attributes6.rb`): a
//! has_one entry without an `id` replaces the record already there, by
//! the association's `dependent:` (`:destroy` destroys it, `:delete`
//! deletes it, otherwise its key, and an `as:` type, are set to nil and
//! saved, and a record that then fails to save raises `RecordNotSaved`
//! with nothing written). A required belongs_to whose record is marked
//! for destruction fails the parent ("Writer must exist"), with the
//! record and the key kept.
//!
//! The declaration was reported and dropped, and no writer existed.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TABLES: &str = "  create_table \"comments\", force: :cascade do |t|";
const NEW_TABLES: &str = "  create_table \"authors\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"role\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"summaries\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\", force: :cascade do |t|";

const ARTICLE: &str = "class Article < ApplicationRecord\n  include Annotated\n  has_many :comments, dependent: :destroy\n  has_one :summary\n  belongs_to :author, optional: true\n  accepts_nested_attributes_for :comments, :summary, :author, allow_destroy: true\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
base = { title: "Nested", body: "A body long enough to validate." }
a = Article.new(**base,
  comments_attributes: [{ commenter: "c1", body: "one" }, { commenter: "c2", body: "two" }],
  summary_attributes: { text: "short" },
  author_attributes: { name: "Ann" },
  notes_attributes: [{ text: "n" }])
check("saved", a.save, true)
found = Article.find(a.id)
check("has_many built", found.comments.map(&:body).sort, ["one", "two"])
check("has_one built", Summary.where(article_id: a.id).first.text, "short")
check("belongs_to built first", Author.find(found.author_id).name, "Ann")
check("concern's included do", Note.where(article_id: a.id).count, 1)
h = Article.create!(**base, comments_attributes: { "0" => { "commenter" => "s", "body" => "string keys" } })
check("hash of hashes, string keys", Article.find(h.id).comments.map(&:body), ["string keys"])
extra = { name: "Bo" }
extra[:role] = "editor"
e = Article.create!(**base, author_attributes: extra)
check("a key set before assignment", Author.find(Article.find(e.id).author_id).role, "editor")
before = [Article.count, Comment.count]
bad = Article.new(**base, comments_attributes: [{ commenter: "c", body: "" }])
check("an invalid child fails the parent", bad.save, false)
check("its error under the association", bad.errors.include?("Comments body can't be blank"), true)
check("nothing written", [Article.count, Comment.count], before)
bad_author = Article.new(**base, author_attributes: { name: "" })
check("an invalid belongs_to fails the parent", bad_author.save, false)
u = Article.create!(**base)
u.update!(comments_attributes: [{ commenter: "late", body: "added" }])
check("update builds", Article.find(u.id).comments.map(&:body), ["added"])
puts "nested attributes passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", TABLES, NEW_TABLES)
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.integer \"author_id\"\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n", ARTICLE)
        .write("app/models/author.rb", "class Author < ApplicationRecord\n  validates :name, presence: true\nend\n")
        .write("app/models/summary.rb", "class Summary < ApplicationRecord\n  belongs_to :article\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article\nend\n")
        .write(
            "app/models/concerns/annotated.rb",
            "module Annotated\n  extend ActiveSupport::Concern\n\n  included do\n    has_many :notes\n    accepts_nested_attributes_for :notes\n  end\nend\n",
        )
}

#[test]
fn nested_attributes_build_and_save_with_the_parent_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested attributes passed"), "{}", run.stdout);
}

const UPDATE_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def art
  Article.create!(title: "Nested", body: "A body long enough to validate.",
    comments_attributes: [{ commenter: "c", body: "a" }, { commenter: "c", body: "b" }],
    summary_attributes: { text: "s" }, author_attributes: { name: "n" }, notes_attributes: [{ text: "n1" }])
end
a = art
c = a.comments.first
a.update!(comments_attributes: [{ "id" => c.id.to_s, "body" => "z" }])
check("update by a string id", Comment.find(c.id).body, "z")
a = art
c = a.comments.first
a.update!(comments_attributes: [{ id: c.id, body: "z" }, { commenter: "c", body: "new" }])
check("update and build in one call", Comment.where(article_id: a.id).map(&:body).sort, ["b", "new", "z"])
a = art
c = a.comments.first
check("an invalid update fails the parent", a.update(comments_attributes: [{ id: c.id, body: "" }]), false)
check("its error", a.errors.include?("Comments body can't be blank"), true)
check("nothing written", Comment.find(c.id).body, "a")
a = art
missing = begin
  a.update(comments_attributes: [{ id: 0, body: "q" }])
  "no error"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
check("an unknown id", missing, "Couldn't find Comment with ID=0 for Article with ID=#{a.id}")
["1", "true", "t", "TRUE", "on", true].each do |v|
  a = art
  c = a.comments.first
  a.update!(comments_attributes: [{ id: c.id, _destroy: v }])
  check("_destroy #{v.inspect} destroys", Comment.where(id: c.id).count, 0)
end
["0", "false", "f"].each do |v|
  a = art
  c = a.comments.first
  a.update!(comments_attributes: [{ id: c.id, _destroy: v }])
  check("_destroy #{v.inspect} keeps", Comment.where(id: c.id).count, 1)
end
a = art
a.update!(comments_attributes: [{ commenter: "c", body: "x", _destroy: "1" }])
check("a marked entry without an id builds nothing", Comment.where(article_id: a.id).map(&:body).sort, ["a", "b"])
a = art
c = a.comments.first
a.update!(comments_attributes: [{ id: c.id, body: "zz", _destroy: "1" }])
check("marked with attributes: destroyed", Comment.where(id: c.id).count, 0)
a = art
n = Note.where(article_id: a.id).first
a.update!(notes_attributes: [{ id: n.id, text: "t2", _destroy: "1" }])
check("no allow_destroy: kept and updated", Note.find(n.id).text, "t2")
a = art
s = Summary.where(article_id: a.id).first
a.update!(summary_attributes: { id: s.id, text: "t2" })
check("has_one update", Summary.find(s.id).text, "t2")
a = art
s = Summary.where(article_id: a.id).first
a.update!(summary_attributes: { id: s.id, _destroy: "1" })
check("has_one destroy", Summary.where(id: s.id).count, 0)
a = art
au = Author.find(a.author_id)
a.update!(author_attributes: { id: au.id, name: "m" })
check("belongs_to update", Author.find(au.id).name, "m")
a = art
au = Author.find(a.author_id)
a.update!(author_attributes: { id: au.id, _destroy: "1" })
check("belongs_to destroy", [Author.where(id: au.id).count, Article.find(a.id).author_id], [0, nil])
a = art
wrong = begin
  a.update(author_attributes: { id: 0, name: "m" })
  "no error"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
check("belongs_to wrong id", wrong, "Couldn't find Author with ID=0 for Article with ID=#{a.id}")
puts "nested update passed"
"##;

#[test]
fn nested_attributes_update_and_destroy_on_ruby() {
    let run = app().run_ruby(UPDATE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested update passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_attributes_update_and_destroy_on_spinel() {
    let run = app().run_spinel_with_db(UPDATE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested update passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_attributes_build_and_save_with_the_parent_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested attributes passed"), "{}", run.stdout);
}

const POLY_TABLES: &str = "  create_table \"people\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"type\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"places\", force: :cascade do |t|\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"locations\", force: :cascade do |t|\n    t.string \"line\"\n    t.string \"addressable_type\"\n    t.integer \"addressable_id\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"remarks\", force: :cascade do |t|\n    t.string \"text\"\n    t.string \"remarkable_type\"\n    t.integer \"remarkable_id\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\", force: :cascade do |t|";

fn polymorphic_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", TABLES, POLY_TABLES)
        .write("app/models/location.rb", "class Location < ApplicationRecord\n  belongs_to :addressable, polymorphic: true\nend\n")
        .write(
            "app/models/remark.rb",
            "class Remark < ApplicationRecord\n  belongs_to :remarkable, polymorphic: true\n  validates :text, presence: true\nend\n",
        )
        .write(
            "app/models/concerns/remarkable.rb",
            "module Remarkable\n  extend ActiveSupport::Concern\n\n  included do\n    has_many :remarks, as: :remarkable\n    accepts_nested_attributes_for :remarks, allow_destroy: true\n  end\nend\n",
        )
        .write(
            "app/models/person.rb",
            "class Person < ApplicationRecord\n  include Remarkable\n  has_one :home, as: :addressable, class_name: \"Location\"\n  accepts_nested_attributes_for :home, allow_destroy: true\nend\n",
        )
        .write("app/models/employee.rb", "class Employee < Person\nend\n")
        .write("app/models/place.rb", "class Place < ApplicationRecord\n  include Remarkable\nend\n")
}

const POLY_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def not_found
  yield
  "no error"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
p = Person.create!(name: "a", home_attributes: { line: "x" })
l = Location.where(addressable_id: p.id).first
check("has_one as:, class_name: builds", [l.line, l.addressable_type], ["x", "Person"])
e = Employee.create!(name: "e", home_attributes: { line: "y" }, remarks_attributes: [{ text: "r" }])
check("an STI owner writes its base class", Location.where(addressable_id: e.id).first.addressable_type, "Person")
check("has_many as: on it too", Remark.where(remarkable_id: e.id).first.remarkable_type, "Person")
pl = Place.create!(name: "p", remarks_attributes: [{ text: "r1" }, { text: "r2" }])
check("has_many as: from a concern", Remark.where(remarkable_type: "Place", remarkable_id: pl.id).count, 2)
r = Remark.where(remarkable_type: "Place", remarkable_id: pl.id).first
pl.update!(remarks_attributes: [{ id: r.id, text: "z" }])
check("update by id", Remark.find(r.id).text, "z")
mine = Remark.where(remarkable_type: "Person", remarkable_id: e.id).first
check("an id of another owner type", not_found { pl.update(remarks_attributes: [{ id: mine.id, text: "z" }]) },
  "Couldn't find Remark with ID=#{mine.id} for Place with ID=#{pl.id}")
other = Place.create!(name: "o")
check("an id of another owner", not_found { other.update(remarks_attributes: [{ id: r.id, text: "z" }]) },
  "Couldn't find Remark with ID=#{r.id} for Place with ID=#{other.id}")
pl.update!(remarks_attributes: [{ id: r.id, _destroy: "1" }])
check("destroy has_many as:", Remark.where(id: r.id).count, 0)
p.update!(home_attributes: { id: l.id, _destroy: "1" })
check("destroy has_one as:", Location.where(id: l.id).count, 0)
bad = Place.new(name: "b", remarks_attributes: [{ text: "" }])
check("an invalid child fails the parent", bad.save, false)
check("its error", bad.errors.include?("Remarks text can't be blank"), true)
puts "nested polymorphic passed"
"##;

#[test]
fn nested_attributes_over_polymorphic_as_on_ruby() {
    let run = polymorphic_app().run_ruby(POLY_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested polymorphic passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_attributes_over_polymorphic_as_on_spinel() {
    let run = polymorphic_app().run_spinel_with_db(POLY_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested polymorphic passed"), "{}", run.stdout);
}
const REPLACE_TABLES: &str = "  create_table \"owners\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n  create_table \"gones\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n    t.string \"tag\"\n  end\n\n  create_table \"wipes\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n  end\n\n  create_table \"kepts\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n  end\n\n  create_table \"plains\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n  end\n\n  create_table \"stucks\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n  end\n\n  create_table \"shields\", force: :cascade do |t|\n    t.integer \"owner_id\"\n    t.string \"text\"\n  end\n\n  create_table \"folks\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n  create_table \"spots\", force: :cascade do |t|\n    t.string \"line\"\n    t.string \"addressable_type\"\n    t.integer \"addressable_id\"\n  end\n\n  create_table \"writers\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"tag\"\n  end\n\n  create_table \"letters\", force: :cascade do |t|\n    t.string \"title\"\n    t.integer \"writer_id\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|";

fn replace_app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::real_blog().edit("db/schema.rb", TABLES, REPLACE_TABLES);
    for (class, required) in [("Gone", false), ("Wipe", false), ("Kept", false), ("Plain", false), ("Stuck", true)] {
        let optional = if required { "" } else { ", optional: true" };
        app = app.write(
            &format!("app/models/{}.rb", class.to_lowercase()),
            &format!("class {class} < ApplicationRecord\n  belongs_to :owner{optional}\nend\n"),
        );
    }
    app.write(
        "app/models/owner.rb",
        "class Owner < ApplicationRecord\n  has_one :gone, dependent: :destroy\n  has_one :wipe, dependent: :delete\n  has_one :kept, dependent: :nullify\n  has_one :plain\n  has_one :stuck\n  has_one :shield, dependent: :destroy\n  accepts_nested_attributes_for :gone, :wipe, :kept, :plain, :stuck, :shield, allow_destroy: true\nend\n",
    )
    .write(
        "app/models/folk.rb",
        "class Folk < ApplicationRecord\n  has_one :home, as: :addressable, class_name: \"Spot\", dependent: :destroy\n  has_one :spare, as: :addressable, class_name: \"Spot\"\n  accepts_nested_attributes_for :home, :spare\nend\n",
    )
    .write("app/models/spot.rb", "class Spot < ApplicationRecord\n  belongs_to :addressable, polymorphic: true, optional: true\nend\n")
    .write(
        "app/models/shield.rb",
        "class Shield < ApplicationRecord\n  belongs_to :owner, optional: true\n  before_destroy :guard\n\n  def guard\n    throw :abort if text == \"keep\"\n  end\nend\n",
    )
    .write("app/models/writer.rb", "class Writer < ApplicationRecord\nend\n")
    .write(
        "app/models/letter.rb",
        "class Letter < ApplicationRecord\n  belongs_to :writer\n  accepts_nested_attributes_for :writer, allow_destroy: true\nend\n",
    )
}

const REPLACE_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
o = Owner.create!(name: "o", gone_attributes: { text: "1" }, wipe_attributes: { text: "1" },
  kept_attributes: { text: "1" }, plain_attributes: { text: "1" })
old = [Gone, Wipe, Kept, Plain].map { |k| k.where(owner_id: o.id).first.id }
o.update!(gone_attributes: { text: "2" }, wipe_attributes: { text: "2" }, kept_attributes: { text: "2" }, plain_attributes: { text: "2" })
check("dependent: :destroy replaces", [Gone.where(id: old[0]).count, Gone.where(owner_id: o.id).map(&:text)], [0, ["2"]])
check("dependent: :delete replaces", [Wipe.where(id: old[1]).count, Wipe.where(owner_id: o.id).map(&:text)], [0, ["2"]])
check("dependent: :nullify replaces", [Kept.find(old[2]).owner_id, Kept.where(owner_id: o.id).map(&:text)], [nil, ["2"]])
check("no dependent nullifies", [Plain.find(old[3]).owner_id, Plain.where(owner_id: o.id).map(&:text)], [nil, ["2"]])
n = Owner.create!(name: "n")
n.update!(gone_attributes: { text: "x" })
check("nothing there: just builds", Gone.where(owner_id: n.id).map(&:text), ["x"])
s = Owner.create!(name: "s", stuck_attributes: { text: "1" })
stuck = Stuck.where(owner_id: s.id).first
raised = begin
  s.update(name: "renamed", stuck_attributes: { text: "2" })
  "no error"
rescue ActiveRecord::RecordNotSaved => e
  e.message
end
check("a required old record raises", raised, "Failed to remove the existing associated stuck. The record failed to save after its foreign key was set to nil.")
check("nothing written", [Stuck.find(stuck.id).owner_id, Stuck.where(owner_id: s.id).count, Owner.find(s.id).name], [s.id, 1, "s"])
f = Folk.create!(name: "f", home_attributes: { line: "1" })
old_home = Spot.where(addressable_id: f.id).first.id
f.update!(home_attributes: { line: "2" })
check("as: dependent: :destroy replaces", [Spot.where(id: old_home).count, Spot.where(addressable_type: "Folk", addressable_id: f.id).map(&:line)], [0, ["2"]])
g = Folk.create!(name: "g", spare_attributes: { line: "1" })
spare = Spot.where(addressable_id: g.id).first
g.update!(spare_attributes: { line: "2" })
check("as: nullify clears the id and type", [Spot.find(spare.id).addressable_id, Spot.find(spare.id).addressable_type], [nil, nil])
check("as: the new one", Spot.where(addressable_type: "Folk", addressable_id: g.id).map(&:line), ["2"])
w = Writer.create!(name: "w")
l = Letter.create!(title: "t", writer_id: w.id)
check("a required belongs_to marked for destruction fails", l.update(writer_attributes: { id: w.id, _destroy: "1" }), false)
check("its error", l.errors.include?("Writer must exist"), true)
check("the record and key kept", [Writer.where(id: w.id).count, Letter.find(l.id).writer_id], [1, w.id])
orphans = [Gone, Wipe, Kept, Plain].flat_map { |k| [k.create!(text: "orphan"), k.create!(text: "zero", owner_id: 0)] }
spot = Spot.create!(line: "orphan")
zero_spot = Spot.create!(line: "zero", addressable_type: "Folk", addressable_id: 0)
fresh = Owner.new(name: "fresh", gone_attributes: { text: "n" }, wipe_attributes: { text: "n" },
  kept_attributes: { text: "n" }, plain_attributes: { text: "n" })
check("a new owner saves", fresh.save, true)
check("rows with a nil or 0 key survive a new owner", orphans.map { |r| r.class.where(id: r.id).map(&:owner_id) },
  [[nil], [0]] * 4)
nf = Folk.new(name: "nf", home_attributes: { line: "n" })
nf.save!
check("as: rows with a nil or 0 key survive a new owner", [spot, zero_spot].map { |r| Spot.where(id: r.id).map(&:addressable_id) }, [[nil], [0]])
sh = Owner.create!(name: "sh", shield_attributes: { text: "keep" })
kept_shield = Shield.where(owner_id: sh.id).first
check("an old record whose destroy aborts: the update succeeds", sh.update(shield_attributes: { text: "new" }), true)
check("both rows stay, as in Rails", Shield.where(owner_id: sh.id).map(&:text).sort, ["keep", "new"])
def not_found
  yield
  "no error"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
t = Owner.create!(name: "t", gone_attributes: { text: "old" })
old = Gone.where(owner_id: t.id).first
t.gone_attributes = { text: "new" }
check("an id after a pending build", not_found { t.gone_attributes = { id: old.id, text: "x" } },
  "Couldn't find Gone with ID=#{old.id} for Owner with ID=#{t.id}")
check("the old record untouched", Gone.where(id: old.id).map(&:text), ["old"])
t.save!
check("the rescued entry changed nothing: the build saves", [Gone.where(id: old.id).count, Gone.where(owner_id: t.id).map(&:text)], [0, ["new"]])
t = Owner.create!(name: "t", gone_attributes: { text: "old" })
old = Gone.where(owner_id: t.id).first
t.gone_attributes = { id: old.id, text: "edited" }
check("a wrong id after an update", not_found { t.gone_attributes = { id: 0, text: "x" } },
  "Couldn't find Gone with ID=0 for Owner with ID=#{t.id}")
t.save!
check("the update still saves", Gone.find(old.id).text, "edited")
t = Owner.create!(name: "t", gone_attributes: { text: "old" })
old = Gone.where(owner_id: t.id).first
t.gone_attributes = { text: "a", tag: "t" }
t.gone_attributes = { text: "b" }
t.save!
check("two entries without an id: one record, merged", [Gone.where(id: old.id).count, Gone.where(owner_id: t.id).map { |g| [g.text, g.tag] }], [0, [["b", "t"]]])
t = Owner.create!(name: "t", gone_attributes: { text: "old" })
t.gone_attributes = { text: "a", tag: "t" }
t.gone_attributes = { text: "b", _destroy: "1" }
t.save!
check("a marked second entry leaves the built record", Gone.where(owner_id: t.id).map { |g| [g.text, g.tag] }, [["a", "t"]])
n = Owner.new(name: "n")
n.gone_attributes = { text: "a", tag: "t" }
n.gone_attributes = { text: "b" }
n.save!
check("a new owner: one record, merged", Gone.where(owner_id: n.id).map { |g| [g.text, g.tag] }, [["b", "t"]])
lw = Writer.create!(name: "old")
lt = Letter.create!(title: "l", writer_id: lw.id)
lt.writer_attributes = { name: "new" }
check("belongs_to: an id after a pending build", not_found { lt.writer_attributes = { id: lw.id, name: "x" } },
  "Couldn't find Writer with ID=#{lw.id} for Letter with ID=#{lt.id}")
lt.save!
check("belongs_to: the build still saves", [Writer.find(Letter.find(lt.id).writer_id).name, Writer.find(lw.id).name], ["new", "old"])
lt = Letter.create!(title: "l", writer_id: lw.id)
before = Writer.count
lt.writer_attributes = { name: "a", tag: "t" }
lt.writer_attributes = { name: "b" }
lt.save!
w = Writer.find(Letter.find(lt.id).writer_id)
check("belongs_to: two entries without an id", [[w.name, w.tag], Writer.count - before, Writer.where(id: lw.id).count], [["b", "t"], 1, 1])
lt = Letter.create!(title: "l", writer_id: lw.id)
lt.writer_attributes = { name: "a", tag: "t" }
lt.writer_attributes = { name: "b", _destroy: "1" }
lt.save!
w = Writer.find(Letter.find(lt.id).writer_id)
check("belongs_to: a marked second entry leaves the built record", [w.name, w.tag], ["a", "t"])
puts "nested replace passed"
"##;

#[test]
fn nested_attributes_replace_and_required_destroy_on_ruby() {
    let run = replace_app().run_ruby(REPLACE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested replace passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_attributes_replace_and_required_destroy_on_spinel() {
    let run = replace_app().run_spinel_with_db(REPLACE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested replace passed"), "{}", run.stdout);
}

/// Emit real-blog for `target` with Article's first two lines replaced by
/// `article_head`; the CLI's stderr.
fn emit_stderr(name: &str, target: &str, article_head: &str) -> String {
    let root = std::env::temp_dir().join(format!("roundhouse-nested-attributes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let model = std::fs::read_to_string(source.join("app/models/article.rb"))
        .unwrap()
        .replacen("class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n", article_head, 1);
    std::fs::write(source.join("app/models/article.rb"), model).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target, "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&root).ok();
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A custom `primary_key:` keys the records by another column than the
/// `id` the generated source reads: the declaration stays reported.
#[test]
fn a_custom_primary_key_stays_reported() {
    let stderr = emit_stderr(
        "primary-key",
        "ruby",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy, primary_key: :title\n  accepts_nested_attributes_for :comments\n",
    );
    assert!(stderr.contains("accepts_nested_attributes_for not supported"), "{stderr}");
}

/// A strict target's lowering of the generated source is not verified,
/// so there the declaration stays the reported gap it was.
#[test]
fn a_strict_target_keeps_the_declaration_reported() {
    let root = std::env::temp_dir().join(format!("roundhouse-nested-attributes-strict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let model = std::fs::read_to_string(source.join("app/models/article.rb")).unwrap().replacen(
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments\n",
        1,
    );
    std::fs::write(source.join("app/models/article.rb"), model).unwrap();
    let run = |target: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "--allow-unsupported", "-o"])
            .arg(root.join(format!("out-{target}")))
            .arg(&source)
            .output()
            .expect("run roundhouse");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let rust = run("rust");
    let ruby = run("ruby");
    std::fs::remove_dir_all(&root).ok();
    assert!(rust.contains("accepts_nested_attributes_for not supported"), "{rust}");
    assert!(!ruby.contains("accepts_nested_attributes_for not supported"), "{ruby}");
}

// Part 3 (oracle `nested_attributes4.rb`): `reject_if: :all_blank` skips
// an entry whose values, `_destroy` aside, are all blank (whitespace too);
// a lambda is called with the indifferent entry, `attributes['name']`; a
// Symbol calls that method of the model with it; a marked entry is never
// rejected, and an `id` counts as a value.
const REJECT_TABLES: &str = "  create_table \"comments\", force: :cascade do |t|";
const REJECT_NEW_TABLES: &str = "  create_table \"tags\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"marks\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"name\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|";
const REJECT_ARTICLE: &str = "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  has_many :tags\n  has_many :notes\n  accepts_nested_attributes_for :comments, reject_if: :all_blank, allow_destroy: true\n  accepts_nested_attributes_for :tags, reject_if: ->(attributes) { attributes['name'].blank? }\n  accepts_nested_attributes_for :notes, reject_if: :skip_note?\n  has_many :marks\n  accepts_nested_attributes_for :marks, reject_if: :locked?\n\n  def skip_note?(attributes)\n    attributes['text'] == \"skip\"\n  end\n\n  def locked?\n    title == \"locked\"\n  end\n";

const REJECT_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
base = { title: "Reject", body: "A body long enough to validate." }
a = Article.create!(**base, comments_attributes: [{ commenter: "", body: "" }, { commenter: "c", body: "x" }])
check(":all_blank skips an all-blank entry", Comment.where(article_id: a.id).count, 1)
a = Article.create!(**base, comments_attributes: [{ commenter: " ", body: "  ", _destroy: "0" }])
check(":all_blank: whitespace and _destroy", Comment.where(article_id: a.id).count, 0)
a = Article.create!(**base, comments_attributes: [{ commenter: "c", body: "x" }])
c = Comment.where(article_id: a.id).first
a.update!(comments_attributes: [{ id: c.id, commenter: "c", body: "y" }])
check("an id counts as a value", Comment.find(c.id).body, "y")
a.update!(comments_attributes: [{ id: c.id, commenter: "", body: "", _destroy: "1" }])
check("a marked entry is not rejected", Comment.where(id: c.id).count, 0)
a = Article.create!(**base, tags_attributes: [{ name: "" }, { "name" => "ok" }])
check("lambda reads the entry by String key", Tag.where(article_id: a.id).map(&:name), ["ok"])
a = Article.create!(**base, notes_attributes: [{ text: "skip" }, { text: "keep" }])
check("Symbol method", Note.where(article_id: a.id).map(&:text), ["keep"])
a = Article.create!(**base, marks_attributes: [{ name: "m" }])
check("a zero-arity method is called bare", Mark.where(article_id: a.id).count, 1)
a = Article.create!(title: "locked", body: base[:body], marks_attributes: [{ name: "m" }])
check("and its answer rejects", Mark.where(article_id: a.id).count, 0)
def not_found
  yield
  "no error"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
a = Article.create!(**base)
check("an unknown id raises before rejection", not_found { a.update(comments_attributes: [{ id: 0 }]) },
  "Couldn't find Comment with ID=0 for Article with ID=#{a.id}")
o = Article.create!(**base, tags_attributes: [{ name: "theirs" }])
theirs = Tag.where(article_id: o.id).first
check("another owner's id raises though the lambda rejects it", not_found { a.update(tags_attributes: [{ id: theirs.id, name: "" }]) },
  "Couldn't find Tag with ID=#{theirs.id} for Article with ID=#{a.id}")
mine = Article.create!(**base, tags_attributes: [{ name: "mine" }])
tag = Tag.where(article_id: mine.id).first
mine.update!(tags_attributes: [{ id: tag.id, name: "" }])
check("a rejected entry with an own id is skipped", Tag.find(tag.id).name, "mine")
puts "nested reject_if passed"
"##;

fn reject_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", REJECT_TABLES, REJECT_NEW_TABLES)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n", REJECT_ARTICLE)
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\n  belongs_to :article\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article\nend\n")
        .write("app/models/mark.rb", "class Mark < ApplicationRecord\n  belongs_to :article\nend\n")
}

#[test]
fn nested_attributes_reject_if_on_ruby() {
    let run = reject_app().run_ruby(REJECT_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested reject_if passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_attributes_reject_if_on_spinel() {
    let run = reject_app().run_spinel_with_db(REJECT_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested reject_if passed"), "{}", run.stdout);
}

/// Rails calls the lambda with the class as `self`: a receiverless call in
/// it is the class's method, which the generated instance method would
/// not reach, so the declaration stays the reported gap.
#[test]
fn a_reject_if_lambda_calling_its_class_stays_reported() {
    let stderr = emit_stderr(
        "reject-gap",
        "ruby",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments, reject_if: ->(attributes) { skip?(attributes) }\n\n  def self.skip?(attributes)\n    false\n  end\n",
    );
    assert!(stderr.contains("accepts_nested_attributes_for not supported"), "{stderr}");
}

/// A Symbol `reject_if:` the model doesn't define itself (inherited, or
/// from a concern not spliced in) can't be checked for Rails' arity
/// rule, so the declaration stays the reported gap.
#[test]
fn a_reject_if_method_the_model_lacks_stays_reported() {
    let stderr = emit_stderr(
        "reject-missing",
        "ruby",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments, reject_if: :missing?\n",
    );
    assert!(stderr.contains("accepts_nested_attributes_for not supported"), "{stderr}");
}
