use super::emit_and_run;

/// `validates :attr, uniqueness: …` on the ruby-family targets, which ship
/// the runtime Relation the check queries at validation time (Rails 8.1's
/// `UniquenessValidator`: `where(attr: value)`, minus the record itself,
/// per `scope:` column, `exists?`). Answers below are Rails 8.1.4's for the
/// same schema, rows and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  belongs_to :account, optional: true

  validates :uri, uniqueness: true
  validates :name, uniqueness: { scope: :owner_id, case_sensitive: false }
  validates :code, uniqueness: { scope: :account, allow_nil: true }
  validates :label, uniqueness: { allow_blank: true }
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "accounts", force: :cascade do |t|
    t.string "handle"
  end

  create_table "widgets", force: :cascade do |t|
    t.string "uri"
    t.string "name"
    t.integer "owner_id"
    t.string "code"
    t.integer "account_id"
    t.string "label"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("app/models/account.rb", "class Account < ApplicationRecord\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

#[test]
fn uniqueness_matches_rails_on_the_ruby_target() {
    app()
        .run_ruby(
            r#"a1 = Account.create!(handle: "a1")
a2 = Account.create!(handle: "a2")
Widget.create!(uri: "u1", name: "Bob", owner_id: 1, code: "c1", account_id: a1.id, label: "")
Widget.create!(uri: nil, name: nil, owner_id: 2, code: nil, account_id: a1.id, label: "l")
def errors_for(**attrs)
  w = Widget.new(**attrs)
  w.valid?
  w.errors.to_a
end
got = [
  errors_for(uri: "u1"),
  errors_for(uri: "u2", name: "bob", owner_id: 1),
  errors_for(uri: "u3", name: "bob", owner_id: 9),
  errors_for(uri: nil),
  errors_for(uri: "u4", code: "c1", account_id: a1.id),
  errors_for(uri: "u5", code: "c1", account_id: a2.id),
  errors_for(uri: "u6", code: nil, account_id: a1.id),
  errors_for(uri: "u7", label: ""),
  errors_for(uri: "u8", label: "l"),
]
want = [
  ["Uri has already been taken"],
  ["Name has already been taken"],
  [],
  ["Uri has already been taken"],
  ["Code has already been taken"],
  [],
  [],
  [],
  ["Label has already been taken"],
]
raise "errors: #{got.inspect}" unless got == want
saved = Widget.first
saved.name = "BOB"
saved.valid?
raise "a persisted record collides with itself: #{saved.errors.to_a.inspect}" unless saved.errors.to_a == []
puts "uniqueness passed"
"#,
        )
        .assert_passes();
}

/// The strict targets have no runtime Relation to run the query, so the
/// rule stays a reported gap there: the emit raises no error (no dynamic
/// relation chain reaches their emitters).
#[test]
fn strict_targets_keep_uniqueness_a_gap_without_emit_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// Through the CLI, a strict target reports the rule it leaves out, and a
/// ruby-family target, which runs the check, reports nothing for it.
#[test]
fn the_cli_reports_uniqueness_only_where_it_is_not_lowered() {
    let root = std::env::temp_dir().join(format!("roundhouse-uniqueness-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        ("app/models/widget.rb", MODEL),
        ("db/schema.rb", SCHEMA),
    ] {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let warnings = |target: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "--allow-unsupported", "-o"])
            .arg(root.join(format!("out-{target}")))
            .arg(root.join("app_src"))
            .output()
            .expect("run roundhouse");
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.contains("uniqueness: not supported"))
            .count()
    };
    let rust = warnings("rust");
    let ruby = warnings("ruby");
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(rust, 4, "one warning per uniqueness rule on rust");
    assert_eq!(ruby, 0, "the ruby target lowers the check");
}

/// Rails excludes the record itself by the model's primary key
/// (`finder_class.primary_key => id_in_database`), not by an `id`
/// column: `"codes"."code" != ?` for `self.primary_key = "code"` over a
/// table keyed by `code`, and `"keystores"."key" != ?` for a table that keeps
/// its integer `id` but names `key` as the key. Rails 8.1.4 answers: a
/// persisted record is valid (it does not collide with itself), and a
/// new one with the same name is not.
#[test]
fn uniqueness_excludes_the_record_by_its_declared_primary_key() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "codes", id: :string, primary_key: "code", force: :cascade do |t|
    t.string "name"
  end

  create_table "keystores", force: :cascade do |t|
    t.string "key"
    t.string "name"
  end
end
"#,
        )
        .write(
            "app/models/code.rb",
            "class Code < ApplicationRecord\n  self.primary_key = \"code\"\n  validates :name, uniqueness: true\nend\n",
        )
        .write(
            "app/models/keystore.rb",
            "class Keystore < ApplicationRecord\n  self.primary_key = \"key\"\n  validates :name, uniqueness: true\nend\n",
        )
        .run_ruby(
            r#"a = Code.create!(code: "a", name: "x")
k = Keystore.create!(key: "k1", name: "x")
got = [a.valid?, Code.new(code: "b", name: "x").valid?, k.valid?, Keystore.new(key: "k2", name: "x").valid?]
raise "valid?: #{got.inspect}" unless got == [true, false, true, false]
puts "primary key exclusion passed"
"#,
        )
        .assert_passes();
}

/// A call-level `allow_nil:` / `allow_blank:` reaches the uniqueness
/// validator like every other one in the call (Rails passes the call's
/// options to each), and a key in the validator's own hash wins over it.
/// Rails 8.1.4 answers for the same rows: a nil uri and a blank label are
/// skipped, and `code` still collides on nil because its hash says
/// `allow_nil: false`.
#[test]
fn call_level_allow_nil_and_allow_blank_reach_uniqueness() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "uri"
    t.string "label"
    t.string "code"
  end
end
"#,
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  validates :uri, uniqueness: true, allow_nil: true
  validates :label, uniqueness: true, allow_blank: true
  validates :code, uniqueness: { allow_nil: false }, allow_nil: true
end
"#,
        )
        .run_ruby(
            r#"Widget.create!(uri: nil, label: "", code: "c")
Widget.create!(uri: nil, label: "", code: nil)
got = [
  { uri: nil, label: "x1", code: "z1" },
  { uri: "u", label: "", code: "z2" },
  { uri: "u2", label: "  ", code: "z3" },
  { uri: "u3", label: "x3", code: nil },
].map { |attrs| w = Widget.new(**attrs); w.valid?; w.errors.to_a }
want = [[], [], [], ["Code has already been taken"]]
raise "errors: #{got.inspect}" unless got == want
puts "call-level allow options passed"
"#,
        )
        .assert_passes();
}
