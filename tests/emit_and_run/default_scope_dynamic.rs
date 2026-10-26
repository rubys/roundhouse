use super::emit_and_run;

/// `default_scope { where(col: <expr>) if <cond> }`: a scope whose values
/// are read when each relation is built (here from a `Current`
/// attribute), as Rails evaluates the block. It was a reported gap, and
/// with `--allow-unsupported` every query read every row. Every answer
/// below is what Rails 8.1.4 returns for the same models and rows on
/// sqlite3.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"projects\", force: :cascade do |t|\n    t.integer \"account_id\"\n    t.string \"name\"\n  end\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"project_id\"\n    t.string \"body\"\n  end\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"archived\", default: false, null: false\n  end\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"region\"\n  end\n  create_table \"gizmos\", force: :cascade do |t|\n    t.integer \"rank\"\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/models/current.rb",
            "class Current < ActiveSupport::CurrentAttributes\n  attribute :account #: Account\n  attribute :show_archived #: bool\n  attribute :region #: String\n  attribute :rank #: Integer\nend\n",
        )
        .write("app/models/account.rb", "class Account < ApplicationRecord\n  has_many :projects\nend\n")
        .write(
            "app/models/project.rb",
            "class Project < ApplicationRecord\n  belongs_to :account, optional: true\n  default_scope { where(account_id: Current.account.id) if Current.account }\n\n  def self.clear\n    delete_all\n  end\nend\n",
        )
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :project, optional: true\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  default_scope { where(archived: false) unless Current.show_archived }\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  default_scope { where(region: Current.region) }\nend\n")
        .write("app/models/gizmo.rb", "class Gizmo < ApplicationRecord\n  default_scope { where(rank: [Current.rank, 2]) if Current.rank }\nend\n")
}

const SEED: &str = r#"a = Account.create!(name: "a")
b = Account.create!(name: "b")
pa = Project.create!(name: "pa", account_id: a.id)
pb = Project.create!(name: "pb", account_id: b.id)
nb = Note.create!(project_id: pb.id, body: "nb")
na = Note.create!(project_id: pa.id, body: "na")
"#;

#[test]
fn reads_apply_a_scope_read_when_the_relation_is_built_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}none = [Project.count, Project.new.account_id]
Current.account = a
missing = begin
  Project.find(pb.id)
  "found"
rescue ActiveRecord::RecordNotFound
  "not found"
end
got = [
  Project.count,
  Project.pluck(:name),
  Project.order(:name).map(&:name),
  Project.where(name: "pb").count,
  missing,
  Project.find(pa.id).name,
  Project.find_by(id: pb.id).nil?,
  Project.exists?(pb.id),
  Project.exists?(name: "pb"),
  Project.first.name,
  Project.last.name,
  a.projects.count,
  Project.where(account_id: b.id).count,
  Note.find(nb.id).project.nil?,
  Note.find(na.id).project.name,
]
want = [1, ["pa"], ["pa"], 0, "not found", "pa", true, false, false, "pa", "pa", 1, 0, true, "pa"]
raise "reads: #{{[none, got].inspect}}" unless none == [2, nil] && got == want
Current.reset
raise "reset: #{{Project.count}}" unless Project.count == 2
puts "dynamic default scope reads passed"
"#
        ))
        .assert_passes();
}

/// The block runs when the relation is BUILT: a relation built under one
/// account and loaded under another answers the first; one built with
/// no account answers every row (Rails 8.1.4: `["pa"]`, `["pa"]`,
/// `["pa", "pb"]`).
#[test]
fn a_relation_keeps_the_scope_of_the_moment_it_was_built_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
built_a = Project.all
built_a_where = Project.where("name IS NOT NULL")
Current.account = b
got = [built_a.pluck(:name), built_a_where.pluck(:name)]
Current.account = nil
built_none = Project.all
Current.account = a
got << built_none.order(:name).pluck(:name)
raise "timing: #{{got.inspect}}" unless got == [["pa"], ["pa"], ["pa", "pb"]]
puts "dynamic default scope timing passed"
"#
        ))
        .assert_passes();
}

/// An association whose foreign key is the scope's column replaces the
/// scope's condition rather than ANDing it: under account a,
/// `b.projects` reads b's project, while `Project.where(account_id:
/// b.id)` reads none (Rails 8.1.4: 1, `["pb"]`, 0).
#[test]
fn an_association_on_the_scoped_key_replaces_the_scope_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
got = [b.projects.count, b.projects.map(&:name), Project.where(account_id: b.id).count, a.projects.map(&:name)]
raise "merge: #{{got.inspect}}" unless got == [1, ["pb"], 0, ["pa"]]
puts "dynamic default scope association merge passed"
"#
        ))
        .assert_passes();
}

#[test]
fn unscoped_lifts_a_dynamic_scope_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
got = [
  Project.unscoped.count,
  Project.unscoped {{ Project.count }},
  Project.unscoped {{ Project.new.account_id }},
  Project.unscope(where: :account_id).count,
  Project.count,
]
raise "unscoped: #{{got.inspect}}" unless got == [2, 2, nil, 2, 1]
puts "dynamic default scope unscoped passed"
"#
        ))
        .assert_passes();
}

/// `new` takes the scope's values as they are when it is called; a
/// given attribute wins, `nil` included (Rails 8.1.4).
#[test]
fn new_records_take_the_scope_values_of_the_moment_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
got = [
  Project.new.account_id == a.id,
  Project.new(account_id: b.id).account_id == b.id,
  Project.new(account_id: nil).account_id,
  Project.new(name: "x").account_id == a.id,
]
raise "new: #{{got.inspect}}" unless got == [true, true, nil, true]
puts "dynamic default scope new passed"
"#
        ))
        .assert_passes();
}

#[test]
fn bulk_writes_keep_the_rows_a_dynamic_scope_hides_like_rails() {
    app()
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
Project.update_all(name: "u")
renamed = Project.unscoped.order(:id).pluck(:name)
Project.clear
left = Project.unscoped.order(:id).pluck(:name)
raise "bulk: #{{[renamed, left].inspect}}" unless [renamed, left] == [["u", "pb"], ["pb"]]
puts "dynamic default scope bulk writes passed"
"#
        ))
        .assert_passes();
}

/// `where(literal) unless <cond>` applies while the guard is off, and a
/// guarded-off scope seeds nothing; `where(col: <expr>)` with no guard
/// is read every time, `nil` meaning `IS NULL` (Rails 8.1.4).
#[test]
fn guarded_and_unguarded_dynamic_scopes_like_rails() {
    app()
        .run_ruby(
            r#"Gadget.create!(name: "live")
Gadget.create!(name: "gone", archived: true)
Widget.create!(name: "eu", region: "eu")
Widget.create!(name: "us", region: "us")
Widget.create!(name: "none", region: nil)
got = [Gadget.count]
Current.show_archived = true
got << Gadget.count
got << Gadget.new.archived
Current.show_archived = nil
got << Gadget.new(archived: true).archived
got << Widget.pluck(:name)
got << Widget.new.region
Current.region = "eu"
got << Widget.pluck(:name)
got << Widget.new.region
want = [1, 2, false, true, ["none"], nil, ["eu"], "eu"]
raise "guards: #{got.inspect}" unless got == want
puts "dynamic default scope guards passed"
"#,
        )
        .assert_passes();
}

/// Through the CLI: a target without the runtime Relation reports the
/// dynamic scope it cannot apply, once per model; a ruby-family target
/// applies it and reports nothing. A value this does not lower (a bare
/// method call, whose receiver differs between the block and `new`)
/// keeps the model-DSL warning on every target.
#[test]
fn the_cli_reports_a_dynamic_default_scope_only_where_it_is_not_applied() {
    let root = std::env::temp_dir().join(format!("roundhouse-default-scope-dyn-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n  attribute :account_id #: Integer\nend\n"),
        ("app/models/project.rb", "class Project < ApplicationRecord\n  default_scope { where(account_id: Current.account_id) if Current.account_id }\nend\n"),
        ("app/models/shelf.rb", "class Shelf < ApplicationRecord\n  default_scope { where(account_id: current_account_id) }\nend\n"),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"projects\", force: :cascade do |t|\n    t.integer \"account_id\"\n  end\n  create_table \"shelves\", force: :cascade do |t|\n    t.integer \"account_id\"\n  end\nend\n",
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
    // Without `--allow-unsupported` the strict target refuses the tree.
    let strict = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "rust", "-o"])
        .arg(root.join("out-rust-strict"))
        .arg(root.join("app_src"))
        .output()
        .expect("run roundhouse");
    let strict_err = String::from_utf8_lossy(&strict.stderr).to_string();
    let rust = lines("rust");
    let typescript = lines("typescript");
    let ruby = lines("ruby");
    std::fs::remove_dir_all(&root).ok();
    let on = |lines: &[String], file: &str| lines.iter().filter(|l| l.contains(file)).count();
    assert_eq!(on(&rust, "project.rb"), 1, "rust reports the scope it cannot apply: {rust:#?}");
    assert!(
        !strict.status.success()
            && strict_err.lines().any(|l| l.contains("project.rb") && l.contains("error") && l.contains("default_scope")),
        "without --allow-unsupported the scope is an error on rust: {strict_err}"
    );
    assert_eq!(on(&typescript, "project.rb"), 1, "typescript reports the scope it cannot apply: {typescript:#?}");
    assert_eq!(on(&ruby, "project.rb"), 0, "the ruby target applies the scope: {ruby:#?}");
    assert_eq!(on(&rust, "shelf.rb"), 1, "an unlowered shape still warns: {rust:#?}");
    assert_eq!(on(&ruby, "shelf.rb"), 1, "an unlowered shape still warns: {ruby:#?}");
}

/// A `joins` into a model with a dynamic scope is not composed at
/// transpile time, since no SQL written there can read the scope's
/// values: it raises when run rather than join the rows the scope hides
/// (Rails answers 1 under account a; the scope in the join's ON clause
/// is a later stage).
#[test]
fn joins_into_a_dynamic_scope_refuses_rather_than_leak() {
    app()
        .write(
            "app/models/account.rb",
            "class Account < ApplicationRecord\n  has_many :projects\n\n  def self.joined_count\n    joins(:projects).count\n  end\nend\n",
        )
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
joined = begin
  Account.joined_count
rescue RuntimeError
  "refused"
end
raise "joins: #{{joined.inspect}}" unless joined == "refused"
puts "dynamic default scope joins passed"
"#
        ))
        .assert_passes();
}

/// An eager load ANDs the target's scope with the owners' keys, unlike
/// the lazy reader, which replaces the scope's condition on the key:
/// under account a, `includes`, `preload` and `eager_load` give b no
/// projects, while a lazy `b.projects` reads b's (Rails 8.1.4:
/// `[["pa"], []]` for each eager form, `[["pa"], ["pb"]]` lazily and
/// with no account). The hints here are written in the script, which
/// the compiler never sees, and in the model's own class methods.
#[test]
fn eager_loads_and_the_dynamic_scope_like_rails() {
    app()
        .write(
            "app/models/account.rb",
            "class Account < ApplicationRecord\n  has_many :projects\n\n  def self.with_projects\n    includes(:projects).order(:id).to_a\n  end\nend\n",
        )
        .run_ruby(&format!(
            r#"{SEED}names = ->(rel) {{ rel.order(:id).map {{ |x| x.projects.map(&:name) }} }}
Current.account = a
got = [
  names.(Account.includes(:projects)),
  names.(Account.preload(:projects)),
  names.(Account.eager_load(:projects)),
  Account.with_projects.map {{ |x| x.projects.map(&:name) }},
  names.(Account.all),
]
Current.account = nil
got << names.(Account.includes(:projects))
got << names.(Account.preload(:projects))
got << names.(Account.eager_load(:projects))
anded = [["pa"], []]
both = [["pa"], ["pb"]]
want = [anded, anded, anded, anded, both, both, both, both]
raise "eager: #{{got.inspect}}" unless got == want
puts "dynamic default scope eager loads passed"
"#
        ))
        .assert_passes();
}

/// A has_many with its own scope has no batch loader, so `includes`
/// reads through its lazy reader. That reader therefore keeps the AND
/// rather than replace the target's scope on the key: an eager load
/// answers as Rails does (`[["pa"], []]`), and a lazy `b.ordered_projects`
/// under account a answers `[]` where Rails answers `["pb"]` — fewer
/// rows, never another account's.
#[test]
fn a_scoped_association_never_reads_across_the_dynamic_scope() {
    app()
        .write(
            "app/models/account.rb",
            "class Account < ApplicationRecord\n  has_many :projects\n  has_many :ordered_projects, -> { order(:name) }, class_name: \"Project\"\nend\n",
        )
        .run_ruby(&format!(
            r#"{SEED}Current.account = a
got = [
  Account.includes(:ordered_projects).order(:id).map {{ |x| x.ordered_projects.map(&:name) }},
  b.ordered_projects.map(&:name),
  a.ordered_projects.map(&:name),
]
raise "scoped assoc: #{{got.inspect}}" unless got == [[["pa"], []], [], ["pa"]]
puts "dynamic default scope scoped association passed"
"#
        ))
        .assert_passes();
}

fn read_tree(dir: &std::path::Path, ext: &str) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read emitted dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == ext) {
                out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    out
}

/// A local captured from outside the block does not exist in the method
/// the scope is lowered into, so such a scope is not lowered: it keeps
/// the model-DSL warning rather than emit a reference to an undefined
/// local.
#[test]
fn a_scope_reading_a_captured_local_is_not_lowered() {
    let (emitted, _errors) = app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"crates\", force: :cascade do |t|\n    t.integer \"account_id\"\n  end\nend\n")
        .write("app/models/crate.rb", "class Crate < ApplicationRecord\n  acct = 5\n  default_scope { where(account_id: acct) }\nend\n")
        .remove("app/models/project.rb")
        .remove("app/models/note.rb")
        .remove("app/models/gadget.rb")
        .remove("app/models/widget.rb")
        .remove("app/models/account.rb")
        .emit(roundhouse::project::BuildTarget::Ruby);
    let ruby = read_tree(&emitted.join("app/models"), "rb");
    assert!(!ruby.contains("_default_scope_where"), "a captured local is not lowered: {ruby}");
}

/// A target without the runtime Relation has no `scope_attributes` to read,
/// so `new` does not seed from the scope there; the scope's own method is
/// still emitted, and the ruby target still seeds.
#[test]
fn seeding_is_skipped_on_targets_without_the_runtime_relation() {
    let (ruby_out, _) = app().emit(roundhouse::project::BuildTarget::Ruby);
    let ruby = read_tree(&ruby_out.join("app/models"), "rb");
    assert!(ruby.contains("Project.all.scope_attributes"), "the ruby target seeds: {ruby}");
    let (rust_out, _) = app().emit(roundhouse::project::BuildTarget::Rust);
    let rust = read_tree(&rust_out.join("src/models"), "rs");
    assert!(rust.contains("_default_scope_where"), "the scope itself is still emitted: {rust}");
    assert!(!rust.contains("scope_attributes"), "a target without the runtime Relation does not seed: {rust}");
}

/// Only an equality condition seeds `new`, as Rails' `scope_for_create`
/// (`where_clause.to_h(equality_only: true)`) and the runtime Relation's
/// `merge_seedable_scope` do: an array (IN) scope filters the query and
/// leaves the column unset, while `nil` (IS NULL) and a scalar seed it.
/// Rails 8.1.4: `[1, nil, 5, nil, 1]`, `[["w0"], nil, "us"]`,
/// `[["weu"], "eu"]` (a range scope also leaves the column unset there).
#[test]
fn only_equality_scope_values_seed_new_records_like_rails() {
    app()
        .run_ruby(
            r#"Gizmo.create!(rank: 1, name: "g1")
Gizmo.create!(rank: 3, name: "g3")
Widget.create!(name: "w0")
Current.region = "eu"
Widget.create!(name: "weu")
Current.region = nil
Current.rank = 1
got = [
  [Gizmo.count, Gizmo.new.rank, Gizmo.new(rank: 5).rank, Gizmo.create!(name: "n").rank, Gizmo.where(name: "g1").count],
  [Widget.pluck(:name), Widget.new.region, Widget.new(region: "us").region],
]
Current.region = "eu"
got << [Widget.pluck(:name), Widget.new.region]
want = [[1, nil, 5, nil, 1], [["w0"], nil, "us"], [["weu"], "eu"]]
raise "seed: #{got.inspect}" unless got == want
puts "dynamic default scope seeding passed"
"#,
        )
        .assert_passes();
}
