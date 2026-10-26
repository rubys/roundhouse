use super::emit_and_run;

/// `default_scope { where(col: literal) }` applies to every query on the
/// model, as Rails' does: class finders and terminals, association
/// readers, bulk writes. It is applied when a relation is built, so
/// `unscoped`, `unscoped { }` and `unscope(where:)` lift it. It was a
/// reported gap, and the emitted tree read and deleted every row
/// (`Gadget.count` was `SELECT COUNT(*) FROM gadgets`; `delete_all` a bare
/// DELETE, #572). Answers are Rails 8.1.4's for the same models and rows.
fn bare() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
}

fn app() -> emit_and_run::Overlay {
    bare()
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
fn reads_on_a_default_scoped_model_apply_the_scope_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}missing = begin
  Gadget.find(gone.id)
  "found"
rescue ActiveRecord::RecordNotFound
  "not found"
end
got = [
  Gadget.count,
  Gadget.order(:name).map(&:name),
  Gadget.pluck(:name),
  Gadget.where(name: "gone").count,
  missing,
  Gadget.find(live.id).name,
  Gadget.find_by(id: gone.id).nil?,
  Gadget.exists?(name: "gone"),
  Gadget.first.name,
  Gadget.last.name,
  Gadget.where(archived: true).count,
  shelf.gadgets.count,
  Note.find(note.id).gadget.nil?,
  Shelf.joined_count,
  Shelf.first_with_gadgets.gadgets.size,
  Note.first_with_gadget.gadget.nil?,
]
want = [1, ["live"], ["live"], 0, "not found", "live", true, false, "live", "live", 0, 1, true, 1, 1, true]
raise "reads: #{{got.inspect}}" unless got == want
puts "default scope reads passed"
"#
        ))
        .assert_passes();
}

#[test]
fn unscoped_and_unscope_lift_the_default_scope_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}got = [
  Gadget.unscoped.count,
  Gadget.unscoped {{ Gadget.count }},
  Gadget.unscoped {{ Gadget.where(archived: true).pluck(:name) }},
  Gadget.count,
  Gadget.unscope(where: :archived).count,
  Gadget.unscoped.where(archived: true).count,
  gone.reload.name,
]
want = [2, 2, ["gone"], 1, 2, 1, "gone"]
raise "unscoped: #{{got.inspect}}" unless got == want
puts "default scope unscoped passed"
"#
        ))
        .assert_passes();
}

/// `unscoped { }` lifts the scope for the thread inside the block only:
/// Rails keeps it in per-thread state, so a request served on another
/// thread meanwhile still reads through the scope (Rails 8.1.4: 1).
#[test]
fn unscoped_block_lifts_the_scope_on_its_own_thread_only_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}inside = Queue.new
release = Queue.new
worker = Thread.new {{ Gadget.unscoped {{ inside << Gadget.count; release.pop }} }}
got = [inside.pop, Gadget.count]
release << :go
worker.join
got << Gadget.count
raise "threads: #{{got.inspect}}" unless got == [2, 1, 1]
puts "default scope threads passed"
"#
        ))
        .assert_passes();
}

#[test]
fn new_records_take_the_scope_attributes_like_rails() {
    app()
        .run_ruby(
            r#"fresh = Gadget.new
explicit = Gadget.new(archived: true)
badge = Badge.new
given = Badge.new(kind: "gold")
got = [fresh.archived, explicit.archived, badge.kind, given.kind]
raise "new: #{got.inspect}" unless got == [false, true, "basic", "gold"]
puts "default scope new passed"
"#,
        )
        .assert_passes();
}

#[test]
fn bulk_writes_on_a_default_scoped_model_keep_hidden_rows_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Gadget.update_all(name: "u")
renamed = Gadget.unscoped.order(:id).pluck(:name)
Gadget.clear
left = Gadget.unscoped.pluck(:name)
raise "bulk: #{{[renamed, left].inspect}}" unless [renamed, left] == [["u", "gone"], ["gone"]]
puts "default scope bulk writes passed"
"#
        ))
        .assert_passes();
}

/// `exists?(hash)` on a default-scoped model is a condition, not a
/// primary key: the scope's Relation answers it. It was cast as a key
/// first, which no Hash is, so every hash form answered `false`.
#[test]
fn exists_with_conditions_on_a_default_scoped_model_like_rails() {
    app()
        .write(
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  belongs_to :shelf, optional: true\n  default_scope { where(archived: false) }\n\n  def self.named?(name)\n    exists?(name: name)\n  end\nend\n",
        )
        .run_ruby(&format!(
            r#"{SEED}got = [
  Gadget.exists?(name: "live"),
  Gadget.exists?(name: "gone"),
  Gadget.named?("live"),
  Gadget.named?("gone"),
  Gadget.exists?(live.id.to_s),
  Gadget.exists?(gone.id),
  Shelf.exists?(name: "s"),
]
want = [true, false, true, false, true, false, true]
raise "exists: #{{got.inspect}}" unless got == want
puts "default scope exists passed"
"#
        ))
        .assert_passes();
}

/// The scope's literal is where `new` starts, and a given attribute
/// still wins over it, `false` and `nil` included, when the column has
/// no schema default of its own (Rails 8.1.4: `Sprocket.new(active:
/// false).active` is false, `.new(spare: nil).spare` is nil).
#[test]
fn given_attributes_win_over_the_scope_literal_without_a_schema_default() {
    bare()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"sprockets\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"active\", null: false\n    t.boolean \"spare\"\n  end\nend\n",
        )
        .write("app/models/sprocket.rb", "class Sprocket < ApplicationRecord\n  default_scope { where(active: true, spare: true) }\nend\n")
        .run_ruby(
            r#"fresh = Sprocket.new
off = Sprocket.new(active: false, spare: false)
blank = Sprocket.new(spare: nil)
got = [fresh.active, fresh.spare, off.active, off.spare, blank.active, blank.spare]
raise "new: #{got.inspect}" unless got == [true, true, false, false, true, nil]
puts "default scope given attributes passed"
"#,
        )
        .assert_passes();
}

/// An STI subclass of a default-scoped model keeps the base's scope:
/// Rails 8.1.4 reads `SubWidget.count` as `type = 'SubWidget' AND
/// active = TRUE`, and `SubWidget.new` starts with `active: true`.
#[test]
fn an_sti_subclass_keeps_the_base_default_scope_like_rails() {
    bare()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"active\"\n    t.string \"type\"\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  default_scope { where(active: true) }\nend\n")
        .write(
            "app/models/sub_widget.rb",
            "class SubWidget < Widget\n  def self.live_count\n    count\n  end\n\n  def self.live_names\n    order(:name).pluck(:name)\n  end\nend\n",
        )
        .run_ruby(
            r#"Widget.create!(name: "on", active: true)
Widget.create!(name: "off", active: false)
SubWidget.create!(name: "subon", active: true)
SubWidget.create!(name: "suboff", active: false)
got = [Widget.count, SubWidget.live_count, SubWidget.live_names, SubWidget.new.active, Widget.unscoped.count]
want = [2, 1, ["subon"], true, 4]
raise "sti: #{got.inspect}" unless got == want
puts "default scope sti passed"
"#,
        )
        .assert_passes();
}

/// Through the CLI: a target without the runtime Relation reports the
/// scope it cannot apply, once per model, instead of answering every
/// query from the whole table; a ruby-family target applies it and
/// reports nothing. A shape this does not lower (an `order`) keeps the
/// model-DSL warning on every target.
#[test]
fn the_cli_reports_default_scope_only_where_it_is_not_applied() {
    let root = std::env::temp_dir().join(format!("roundhouse-default-scope-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  default_scope { where(archived: false) }\nend\n"),
        ("app/models/shelf.rb", "class Shelf < ApplicationRecord\n  default_scope { order(:name) }\nend\n"),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"shelves\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"archived\", default: false, null: false\n  end\nend\n",
        ),
    ] {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let lines = |target: &str| -> Vec<String> {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "--allow-unsupported", "-o"])
            .arg(root.join(format!("out-{target}")))
            .arg(root.join("app_src"))
            .output()
            .expect("run roundhouse");
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.contains("default_scope not supported"))
            .map(str::to_string)
            .collect()
    };
    let rust = lines("rust");
    let ruby = lines("ruby");
    std::fs::remove_dir_all(&root).ok();
    let on = |lines: &[String], file: &str| lines.iter().filter(|l| l.contains(file)).count();
    assert_eq!(on(&rust, "gadget.rb"), 1, "rust reports the scope it cannot apply: {rust:#?}");
    assert_eq!(on(&ruby, "gadget.rb"), 0, "the ruby target applies the scope: {ruby:#?}");
    assert_eq!(on(&rust, "shelf.rb"), 1, "an unlowered shape still warns: {rust:#?}");
    assert_eq!(on(&ruby, "shelf.rb"), 1, "an unlowered shape still warns: {ruby:#?}");
}
