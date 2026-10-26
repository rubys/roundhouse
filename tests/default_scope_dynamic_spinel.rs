//! `default_scope { where(col: <expr>) if <cond> }` in the Spinel binary:
//! the scope's values are read when each relation is built, a relation
//! keeps the values it was built with, `new` takes them (an array value
//! filters only, as in Rails), an eager load
//! ANDs the scope with the owners' keys, and `unscoped` lifts the scope. The same answers as
//! `emit_and_run::default_scope_dynamic` on CRuby, which are Rails
//! 8.1.4's. Needs a Spinel compiler, so `#[ignore]`d; CI's
//! `spinel-framework` job runs it (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"projects\", force: :cascade do |t|\n    t.integer \"account_id\"\n    t.string \"name\"\n  end\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"project_id\"\n    t.string \"body\"\n  end\n  create_table \"gizmos\", force: :cascade do |t|\n    t.integer \"rank\"\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n  attribute :account #: Account\n  attribute :rank #: Integer\nend\n")
        .write("app/models/account.rb", "class Account < ApplicationRecord\n  has_many :projects\nend\n")
        .write(
            "app/models/project.rb",
            "class Project < ApplicationRecord\n  belongs_to :account, optional: true\n  default_scope { where(account_id: Current.account.id) if Current.account }\n\n  def self.clear\n    delete_all\n  end\nend\n",
        )
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :project, optional: true\nend\n")
        .write("app/models/gizmo.rb", "class Gizmo < ApplicationRecord\n  default_scope { where(rank: [Current.rank, 2]) if Current.rank }\nend\n")
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_dynamic_default_scope_applies_in_the_spinel_binary_like_rails() {
    app()
        .run_spinel_with_db(
            r#"a = Account.create!(name: "a")
b = Account.create!(name: "b")
pa = Project.create!(name: "pa", account_id: a.id)
pb = Project.create!(name: "pb", account_id: b.id)
nb = Note.create!(project_id: pb.id, body: "nb")
none = Project.count
Current.account = a
built = Project.all
got = [
  none,
  Project.count,
  Project.pluck(:name),
  Project.find_by(id: pb.id).nil?,
  Project.exists?(pb.id),
  a.projects.count,
  b.projects.count,
  Project.where(account_id: b.id).count,
  Note.find(nb.id).project.nil?,
  Project.new.account_id == a.id,
  Project.unscoped.count,
  Project.unscoped { Project.count },
  Project.unscope(where: :account_id).count,
]
eager = []
Account.includes(:projects).order(:id).to_a.each { |x| eager << x.projects.map(&:name) }
got << eager
Current.account = b
got << built.pluck(:name)
want = [2, 1, ["pa"], true, false, 1, 1, 0, true, true, 2, 2, 2, [["pa"], []], ["pa"]]
raise "spinel: #{got.inspect}" unless got == want
Current.account = a
Project.clear
left = Project.unscoped.pluck(:name)
raise "delete_all: #{left.inspect}" unless left == ["pb"]
Gizmo.create!(rank: 1, name: "g1")
Gizmo.create!(rank: 3, name: "g3")
Current.rank = 1
seeded = [Gizmo.count, Gizmo.new.rank.nil?, Gizmo.new(rank: 5).rank, Gizmo.create!(name: "n").rank.nil?]
raise "seed: #{seeded.inspect}" unless seeded == [1, true, 5, true]
puts "dynamic default scope spinel passed"
"#,
        )
        .assert_passes();
}
