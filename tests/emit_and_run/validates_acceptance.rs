use super::emit_and_run;

/// `validates :attr, acceptance:`. Rails' `AcceptanceValidator` adds
/// "must be accepted" unless `Array(accept).include?(value)`, with
/// `accept:` defaulting to `["1", true]` and `allow_nil:` to `true`. A
/// Boolean column is cast before the check, so `"1"` and `"yes"` are
/// `true` there and `""` is nil. Answers are Rails 8.1.4's for the same
/// model and records.
fn members() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"members\", force: :cascade do |t|\n    t.string \"name\"\n    t.boolean \"active\"\n  end\nend\n",
        )
        .write(
            "app/models/member.rb",
            r#"class Member < ApplicationRecord
  attr_accessor :agreement, :terms

  validates :agreement, acceptance: { allow_nil: false, accept: [true, 'true', '1'] }, on: :create
  validates :terms, acceptance: true
  validates :active, acceptance: true
end
"#,
        )
}

const MEMBERS: &str = r##"def errs(agreement, terms, active)
  m = Member.new
  m.name = "x"
  m.agreement = agreement
  m.terms = terms
  m.active = active
  m.valid?
  m.errors.to_a
end
got = [
  errs("1", nil, nil),
  errs(nil, "1", "1"),
  errs(true, true, true),
  errs("true", "true", "0"),
  errs("yes", "0", "yes"),
  errs(false, false, false),
]
want = [
  [],
  ["Agreement must be accepted"],
  [],
  ["Terms must be accepted", "Active must be accepted"],
  ["Agreement must be accepted", "Terms must be accepted"],
  ["Agreement must be accepted", "Terms must be accepted", "Active must be accepted"],
]
raise "member errors: #{got.inspect}" unless got == want
m = Member.new
m.name = "x"
m.agreement = "1"
m.save!
m.agreement = nil
raise "on: :create checked an update: #{m.errors.to_a.inspect}" unless m.valid?
puts "acceptance passed"
"##;

#[test]
fn acceptance_matches_rails() {
    let run = members().run_ruby(MEMBERS);
    run.assert_passes();
    assert!(run.stdout.contains("acceptance passed"), "{}", run.stdout);
}

/// The strict targets emit the check with no emit error. A Boolean
/// column compares only the accepted Booleans.
#[test]
fn strict_targets_emit_the_acceptance_check_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (emitted, errors) = members().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        if target == BuildTarget::Rust {
            let member = std::fs::read_to_string(emitted.join("src/models/member.rs")).expect("member.rs");
            let validate = &member[member.find("pub fn validate(").expect("validate")..];
            let validate = &validate[..validate.find("\n    }\n").unwrap_or(validate.len())];
            let active = validate.lines().find(|l| l.contains("self.active")).expect("the active check");
            assert!(active.contains("== true"), "{active}");
            assert!(!active.contains("\"1\""), "{active}");
        }
    }
}

const SIGNUP_SCHEMA: &str =
    "ActiveRecord::Schema.define do\n  create_table \"signups\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
const SIGNUP: &str = "class Signup < ApplicationRecord\n  attribute :consent, :boolean\n  validates :consent, acceptance: true\nend\n";
const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

/// `attribute :consent, :boolean` with no column: on the ruby family its
/// writer stores Rails' cast (nil and `""` are nil, and skipped), so the
/// check runs there. Rails 8.1.4's answers.
#[test]
fn acceptance_on_a_boolean_attribute_matches_rails() {
    let run = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SIGNUP_SCHEMA)
        .write("app/models/signup.rb", SIGNUP)
        .run_ruby(
            r##"got = ["1", "0", "yes", "", nil, true, false, "false"].map do |v|
  s = Signup.new
  s.name = "x"
  s.consent = v
  s.valid?
  s.errors.to_a
end
want = [[], ["Consent must be accepted"], [], [], [], [], ["Consent must be accepted"], ["Consent must be accepted"]]
raise "signup errors: #{got.inspect}" unless got == want
puts "attribute acceptance passed"
"##,
        );
    run.assert_passes();
    assert!(run.stdout.contains("attribute acceptance passed"), "{}", run.stdout);
}

/// A strict target's Boolean attribute field turns nil and `""` into
/// `false`, so there the check would reject what Rails skips: the rule
/// is left out and reported for that target, and the ruby target, which
/// runs it, reports nothing.
#[test]
fn acceptance_on_a_boolean_attribute_is_a_strict_target_gap() {
    let root = std::env::temp_dir().join(format!("roundhouse-acceptance-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/signup.rb", SIGNUP),
        ("db/schema.rb", SIGNUP_SCHEMA),
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
            .filter(|l| l.contains("validates :consent, acceptance: not supported"))
            .count()
    };
    let rust = warnings("rust");
    let ruby = warnings("ruby");
    let rust_model = std::fs::read_to_string(root.join("out-rust/src/models/signup.rs")).unwrap_or_default();
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(rust, 1, "the rule is reported on rust");
    assert_eq!(ruby, 0, "the ruby target lowers the check");
    assert!(!rust_model.contains("must be accepted"), "{rust_model}");
}

/// A name declared by `attribute` of another type is still the gap: its
/// writer stores the input verbatim, where Rails casts it.
#[test]
fn acceptance_on_a_string_attribute_is_a_reported_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SIGNUP_SCHEMA)
        .write("app/models/signup.rb", "class Signup < ApplicationRecord\n  attribute :code, :string\n  validates :code, acceptance: { accept: 'yes' }\nend\n")
        .ingest_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("validates :code, acceptance:") && w.contains("uncast")),
        "{warnings:?}"
    );
}

const UNDECLARED_MODEL: &str = "class Member < ApplicationRecord\n  validates :agreement, acceptance: true, presence: true\nend\n";

const UNDECLARED: &str = r##"def errs(attrs)
  m = Member.new({ name: "x" }.merge(attrs))
  m.valid?
  m.errors.to_a
end
got = [errs(agreement: "1"), errs(agreement: "0"), errs({}), errs(agreement: true), errs(agreement: "yes")]
want = [[], ["Agreement must be accepted"], ["Agreement can't be blank"], [], ["Agreement must be accepted"]]
raise "undeclared errors: #{got.inspect}" unless got == want
m = Member.new(name: "x")
m.agreement = "1"
raise "writer: #{m.agreement.inspect}" unless m.agreement == "1" && m.valid?
c = Member.create!(name: "x", agreement: "1")
raise "create!: #{c.agreement.inspect}" unless c.persisted? && c.agreement == "1"
raise "find: #{Member.find(c.id).agreement.inspect}" unless Member.find(c.id).agreement.nil?
puts "undeclared acceptance passed"
"##;

fn undeclared_members() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"members\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/member.rb", UNDECLARED_MODEL)
}

/// An attribute the model does not declare: Rails' `AcceptanceValidator`
/// defines a plain, uncast reader and writer for it, so `new`, the writer
/// and `create!` take it, the check reads it, and a loaded record has
/// none. The model gains the same `attr_accessor`; no gap is reported.
#[test]
fn acceptance_on_an_undeclared_attribute_matches_rails() {
    assert!(undeclared_members().ingest_warnings().is_empty());
    let run = undeclared_members().run_ruby(UNDECLARED);
    run.assert_passes();
    assert!(run.stdout.contains("undeclared acceptance passed"), "{}", run.stdout);
    for target in [roundhouse::project::BuildTarget::Rust, roundhouse::project::BuildTarget::Typescript, roundhouse::project::BuildTarget::Go] {
        let (_emitted, errors) = undeclared_members().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// The same script against activerecord 8.1.4 on sqlite3; skipped when
/// the gems aren't installed.
#[test]
fn rails_answers_the_undeclared_acceptance_table() {
    let probe = r#"gem "activerecord", "8.1.4"; require "active_record"; require "sqlite3""#;
    if !std::process::Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord 8.1.4 and sqlite3 not available");
        return;
    }
    let script = format!(
        r##"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:members) {{ |t| t.string :name }}
end
{APPLICATION_RECORD}
{UNDECLARED_MODEL}
{UNDECLARED}"##
    );
    let output = std::process::Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("undeclared acceptance passed"),
        "activerecord 8.1.4 disagrees\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

/// Refused acceptance validations ahead of an undeclared one: the
/// refusal removes them from the body, and the undeclared name's accessor
/// still lands ahead of its own validation.
#[test]
fn refused_acceptance_ahead_of_an_undeclared_one() {
    let app = || {
        emit_and_run::empty_app()
            .write("app/models/application_record.rb", APPLICATION_RECORD)
            .write("db/schema.rb", SIGNUP_SCHEMA)
            .write(
                "app/models/signup.rb",
                "class Signup < ApplicationRecord\n  attribute :code, :string\n  attribute :code2, :string\n  validates :code, acceptance: { accept: 'yes' }\n  validates :code2, acceptance: { accept: 'yes' }\n  validates :agreement, acceptance: true\nend\n",
            )
    };
    let warnings = app().ingest_warnings();
    for attr in ["code", "code2"] {
        assert!(
            warnings.iter().any(|w| w.contains(&format!("validates :{attr}, acceptance:")) && w.contains("uncast")),
            "{attr}: {warnings:?}"
        );
    }
    assert!(!warnings.iter().any(|w| w.contains(":agreement")), "{warnings:?}");
    let run = app().run_ruby(
        r##"s = Signup.new(name: "x", agreement: "0")
raise "rejected: #{s.errors.to_a.inspect}" unless !s.valid? && s.errors.to_a == ["Agreement must be accepted"]
raise "accepted" unless Signup.new(name: "x", agreement: "1").valid?
puts "refused ahead passed"
"##,
    );
    run.assert_passes();
}
