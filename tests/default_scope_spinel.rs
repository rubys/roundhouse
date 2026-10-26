//! `default_scope { where(col: literal) }` in the Spinel binary: the
//! runtime Relation applies the scope when it is built, `unscoped` and
//! `unscoped { }` lift it, and `delete_all` keeps the rows it hides. The
//! same answers as `emit_and_run::default_scope_static` on CRuby, which
//! are Rails 8.1.4's. Needs a Spinel compiler, so `#[ignore]`d; CI's
//! `spinel-framework` job runs it (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"shelves\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"archived\", default: false, null: false\n    t.integer \"shelf_id\"\n  end\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"gadget_id\"\n    t.string \"body\"\n  end\n  create_table \"badges\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n",
        )
        .write(
            "app/models/shelf.rb",
            "class Shelf < ApplicationRecord\n  has_many :gadgets\n\n  def self.joined_count\n    joins(:gadgets).count\n  end\n\n  def self.first_with_gadgets\n    Shelf.includes(:gadgets).first\n  end\nend\n",
        )
        .write(
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  belongs_to :shelf, optional: true\n  default_scope { where(archived: false) }\n\n  def self.clear\n    delete_all\n  end\nend\n",
        )
        .write("app/models/badge.rb", "class Badge < ApplicationRecord\n  default_scope { where(kind: \"basic\") }\nend\n")
        .write(
            "app/models/note.rb",
            "class Note < ApplicationRecord\n  belongs_to :gadget, optional: true\n\n  def self.first_with_gadget\n    Note.includes(:gadget).first\n  end\nend\n",
        )
}

const SEED: &str = r#"shelf = Shelf.create!(name: "s")
live = Gadget.create!(name: "live", shelf_id: shelf.id)
gone = Gadget.create!(name: "gone", shelf_id: shelf.id)
gone.archived = true
gone.save!
note = Note.create!(gadget_id: gone.id, body: "n")
"#;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_default_scope_applies_in_the_spinel_binary_like_rails() {
    app()
        .run_spinel_with_db(&format!(
            r#"{SEED}got = [
  Gadget.count,
  Gadget.pluck(:name),
  Gadget.find_by(id: gone.id).nil?,
  Gadget.exists?(gone.id),
  Gadget.exists?(name: "live"),
  Gadget.exists?(name: "gone"),
  shelf.gadgets.count,
  Note.find(note.id).gadget.nil?,
  Shelf.joined_count,
  Gadget.unscoped.count,
  Gadget.unscoped {{ Gadget.count }},
  Gadget.unscope(where: :archived).count,
  Badge.new.kind,
]
want = [1, ["live"], true, false, true, false, 1, true, 1, 2, 2, 2, "basic"]
raise "spinel: #{{got.inspect}}" unless got == want
Gadget.clear
left = Gadget.unscoped.pluck(:name)
raise "delete_all: #{{left.inspect}}" unless left == ["gone"]
puts "default scope spinel passed"
"#
        ))
        .assert_passes();
}
