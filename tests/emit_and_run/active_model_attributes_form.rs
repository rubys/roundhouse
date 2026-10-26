//! A tableless `ActiveModel::Model` form that also includes
//! `ActiveModel::Attributes` (a class under `app/models`, so ingest
//! classifies it as a model). Rails 8.1.4: `new(count: "7")` assigns
//! through each attribute's casting writer, `:integer` `"7"` is 7,
//! `:string` 12 is `"12"`, a `:datetime` date string is that day at UTC
//! midnight, `:boolean` `"0"` is false; an `attr_accessor` beside them is
//! assigned as given; validations read the cast value.
//!
//! The model path's Attributes API cast only `:boolean`, and the include
//! was reported as unresolved.
use super::emit_and_run;

const FORM: &str = r#"class Form::Confirm
  include ActiveModel::Model
  include ActiveModel::Attributes

  attribute :count, :integer
  attribute :label, :string
  attribute :at, :datetime
  attribute :flag, :boolean
  attr_accessor :note

  validates :label, presence: true
end
"#;

const SCRIPT: &str = r##"
require_relative "app/models/form/confirm"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
f = Form::Confirm.new(count: "7", label: 12, at: "2026-10-14", flag: "0", note: "x")
check("integer", f.count, 7)
check("string", f.label, "12")
check("datetime", f.at, Time.utc(2026, 10, 14))
check("boolean", f.flag, false)
check("attr_accessor", f.note, "x")
f.count = "08"
check("writer casts", f.count, 8)
check("valid", f.valid?, true)
blank = Form::Confirm.new(label: "")
check("validation reads the cast value", blank.valid?, false)
puts "ActiveModel form attributes passed"
"##;

fn form() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/form/confirm.rb", FORM)
}

#[test]
fn a_form_model_casts_its_attributes_on_ruby() {
    let run = form().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel form attributes passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_form_model_casts_its_attributes_on_spinel() {
    let run = form().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel form attributes passed"), "{}", run.stdout);
}

/// The include is the model path's to provide: no "includes unresolved".
#[test]
fn a_form_models_attributes_include_is_resolved() {
    let (_emitted, errors) = form().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("includes unresolved ActiveModel::Attributes")), "{errors:#?}");
    let warnings = form().ingest_warnings();
    assert!(!warnings.iter().any(|e| e.contains("includes unresolved ActiveModel::Attributes")), "{warnings:#?}");
}

const RECORD_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = Article.new(title: "Typed", body: "A body long enough to validate.", rank: "7", tag: 12, flag: "0")
check("integer", a.rank, 7)
check("string", a.tag, "12")
check("boolean", a.flag, false)
a.rank = "08"
check("writer casts", a.rank, 8)
puts "ActiveRecord attributes passed"
"##;

/// An ActiveRecord model's `attribute :name, :type` (Rails 8.1.4) casts as
/// the ActiveModel form's does: `Record.new(count: "7").count` is 7 and
/// `label: 12` is `"12"`.
#[test]
fn a_record_casts_its_attribute_api_declarations_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :rank, :integer\n  attribute :tag, :string\n  attribute :flag, :boolean\n",
        )
        .run_ruby(RECORD_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveRecord attributes passed"), "{}", run.stdout);
}

const COLUMN_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def reload(a) = Article.find(a.id)
base = { title: "Typed", body: "A body long enough to validate." }
a = Article.new(**base, code: "abc", status: "archived", enabled: "true", shout: "yes", label: 12)
check("integer over a string column keeps the string", a.code, "abc")
check("string over a boolean column keeps the boolean", a.enabled, true)
check("boolean over a string column keeps the string", a.shout, "yes")
check("an enum keeps its mapping", a.status, "archived")
check("a matching attribute casts", a.label, "12")
a.save!
r = reload(a)
check("reloaded string column", r.code, "abc")
check("reloaded boolean column", r.enabled, true)
check("reloaded string column under :boolean", r.shout, "yes")
check("reloaded enum", r.status, "archived")
check("reloaded matching attribute", r.label, "12")
b = Article.new(**base)
b.code = "7"
b.enabled = "0"
b.shout = "0"
b.status = "archived"
b.label = 8
check("writer keeps the string", b.code, "7")
check("writer keeps the boolean cast", b.enabled, false)
check("writer keeps the string under :boolean", b.shout, "0")
check("enum writer", b.status, "archived")
check("matching writer casts", b.label, "8")
b.save!
r = reload(b)
check("reloaded writes", [r.code, r.enabled, r.shout, r.status, r.label], ["7", false, "0", "archived", "8"])
puts "attribute over column passed"
"##;

const COLUMN_MODEL: &str = "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :code, :integer\n  attribute :enabled, :string\n  attribute :shout, :boolean\n  attribute :label, :string\n  attribute :status, :integer\n  enum :status, { active: 0, archived: 1 }\n  attribute :tag_ids, :integer\n";

fn column_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"code\"\n    t.boolean \"enabled\"\n    t.string \"shout\"\n    t.string \"label\"\n    t.integer \"status\"\n    t.bigint \"tag_ids\", array: true",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            COLUMN_MODEL,
        )
}

/// `attribute :name, :type` over a column. Rails 8.1.4 lets the declared
/// type replace the column's everywhere, but here only the writer would
/// follow it while the reader, hydration and the SQL escape follow the
/// column. So a column keeps its own type unless the attribute names that
/// same type (`markers::attribute_matches_column`), through `new`, the
/// writer and a save and reload. An enum declared after the attribute
/// keeps its mapping (`"archived"` stays a label), as in Rails.
#[test]
fn an_attribute_over_a_column_of_another_type_keeps_the_column() {
    let run = column_app().run_ruby(COLUMN_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("attribute over column passed"), "{}", run.stdout);
}

/// The kept columns are reported, each naming the declared and the
/// column's type; a matching one and the enum's are not.
#[test]
fn an_attribute_over_a_column_of_another_type_is_reported() {
    let root = std::env::temp_dir().join(format!("roundhouse-attribute-over-column-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let schema = std::fs::read_to_string(source.join("db/schema.rb")).unwrap().replacen(
        "create_table \"articles\", force: :cascade do |t|",
        "create_table \"articles\", force: :cascade do |t|\n    t.string \"code\"\n    t.boolean \"enabled\"\n    t.string \"shout\"\n    t.string \"label\"\n    t.integer \"status\"\n    t.bigint \"tag_ids\", array: true",
        1,
    );
    std::fs::write(source.join("db/schema.rb"), schema).unwrap();
    let model = std::fs::read_to_string(source.join("app/models/article.rb")).unwrap().replacen(
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
        COLUMN_MODEL,
        1,
    );
    std::fs::write(source.join("app/models/article.rb"), model).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&root).ok();
    let diagnostics: Vec<String> = String::from_utf8_lossy(&out.stderr).lines().map(String::from).collect();
    for kept in ["`attribute :code, :integer` over a string column", "`attribute :enabled, :string` over a boolean column", "`attribute :shout, :boolean` over a string column", "`attribute :tag_ids, :integer` over a bigint array column"] {
        assert!(diagnostics.iter().any(|d| d.contains(kept)), "{kept} not reported: {diagnostics:#?}");
    }
    assert!(!diagnostics.iter().any(|d| d.contains("`attribute :label") || d.contains("`attribute :status")), "{diagnostics:#?}");
}

const ARRAY_COLUMN_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = Article.new(title: "Arrays", body: "A body long enough to validate.", tag_ids: [1, 2])
check("new keeps the Array", a.tag_ids, [1, 2])
a.tag_ids = [3]
check("the writer keeps the Array", a.tag_ids, [3])
a.save!
check("reloaded Array", Article.find(a.id).tag_ids, [3])
puts "attribute over array column passed"
"##;

/// A scalar `attribute` over an array column (`t.bigint "tag_ids", array:
/// true` with `attribute :tag_ids, :integer`) matches the element type, not
/// the column's: the column keeps its Array writer and storage, and the
/// declaration is reported.
#[test]
fn an_attribute_over_an_array_column_keeps_the_array() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", "create_table \"articles\", force: :cascade do |t|\n    t.bigint \"tag_ids\", array: true")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :tag_ids, :integer\n",
        )
        .run_ruby(ARRAY_COLUMN_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("attribute over array column passed"), "{}", run.stdout);
}

const DEFAULTS_FORM: &str = r#"class Form::Prefs
  include ActiveModel::Model
  include ActiveModel::Attributes

  CALLS = []

  attribute :flag, :boolean, default: true
  attribute :limit, :integer, default: "12"
  attribute :label, :string, default: "none"
  attribute :stamp, :integer, default: -> { CALLS << :call; CALLS.size }
  attribute :origin, :string, default: -> { name }
end
"#;

/// `attribute … default:` on a model (Rails 8.1.4, the same on a tableless
/// form and an ActiveRecord model): cast as an assigned value is; `new`
/// with the key, even nil, wins; a String default is fresh per instance; a
/// lambda is called on the first read, with the class as `self`, kept, and
/// skipped by an earlier write or `new` key.
const DEFAULTS_SCRIPT: &str = r##"
require_relative "app/models/form/prefs"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
p = Form::Prefs.new
check("defaults", [p.flag, p.limit, p.label], [true, 12, "none"])
check("new with nil", Form::Prefs.new(flag: nil).flag, nil)
check("new with a value", Form::Prefs.new(limit: "3").limit, 3)
check("fresh string", Form::Prefs.new.label.equal?(Form::Prefs.new.label), false)
Form::Prefs::CALLS.clear
o = Form::Prefs.new
check("lambda not called by new", Form::Prefs::CALLS.size, 0)
check("lambda on first read, kept", [o.stamp, o.stamp, Form::Prefs::CALLS.size], [1, 1, 1])
Form::Prefs::CALLS.clear
check("new key skips the lambda", [Form::Prefs.new(stamp: 5).stamp, Form::Prefs::CALLS.size], [5, 0])
w = Form::Prefs.new
w.stamp = 6
check("write before read skips the lambda", [w.stamp, Form::Prefs::CALLS.size], [6, 0])
check("lambda self is the class", Form::Prefs.new.origin, "Form::Prefs")
puts "model attribute defaults passed"
"##;

fn defaults_form() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/form/prefs.rb", DEFAULTS_FORM)
}

#[test]
fn a_form_models_attribute_defaults_on_ruby() {
    let run = defaults_form().run_ruby(DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("model attribute defaults passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_form_models_attribute_defaults_on_spinel() {
    let run = defaults_form().run_spinel(DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("model attribute defaults passed"), "{}", run.stdout);
}

const RECORD_DEFAULTS_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = Article.new(title: "Typed", body: "A body long enough to validate.")
check("defaults", [a.flag, a.rank], [true, 12])
check("new with nil", Article.new(flag: nil).flag, nil)
check("new with a value", Article.new(rank: "3").rank, 3)
puts "ActiveRecord attribute defaults passed"
"##;

#[test]
fn a_records_attribute_defaults_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :rank, :integer, default: \"12\"\n  attribute :flag, :boolean, default: true\n",
        )
        .run_ruby(RECORD_DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveRecord attribute defaults passed"), "{}", run.stdout);
}

// A model's own reader next to a defaulted attribute (activerecord
// 8.1.4, oracle `cr-oracle.rb`) is what's read, default or not.
const SOURCE_READER_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("the source reader over the default", Article.new.label, "source label")
check("the source reader after a write", Article.new(label: "x").label, "source label")
puts "source reader passed"
"##;

#[test]
fn a_source_reader_beside_a_default_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :label, :string, default: \"none\"\n\n  def label\n    \"source label\"\n  end\n",
        )
        .run_ruby(SOURCE_READER_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("source reader passed"), "{}", run.stdout);
}

// Two `default:` declarations this lowering doesn't model, so they stay
// reported rather than claimed and dropped: one over an existing column
// (Rails makes it the column's default), and one whose writer the model
// defines (Rails' default skips that writer, and the writer's `super`
// has no generated method to reach here).
fn reported_default(decl: &str) -> Vec<String> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("roundhouse-reported-default-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let model = std::fs::read_to_string(source.join("app/models/article.rb")).unwrap().replacen(
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
        &format!("class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n{decl}"),
        1,
    );
    std::fs::write(source.join("app/models/article.rb"), model).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.contains("attribute not supported") && l.contains("not lowered"))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_default_over_a_column_is_reported() {
    let warnings = reported_default("  attribute :title, :string, default: \"untitled\"\n");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn a_default_beside_a_source_writer_is_reported() {
    let warnings = reported_default(
        "  attribute :mood, :string, default: \"calm\"\n\n  def mood=(v)\n    super(v.to_s.upcase)\n  end\n",
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}
