//! `self.inheritance_column = nil` (or `false`) on a model with a `type`
//! column: activerecord 8.1.4 turns STI off for it. `type` is a plain
//! attribute, and a subclass is the plain subclass of a concrete model
//! (`tests/sti_untyped.rs`): it shares the table, filters nothing, and
//! its constructor leaves `type` alone. Its oracle:
//!
//!     ruby tests/sti_disabled/oracle.rb

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROBE: &str = r#"class DisabledProbe
  def self.seed
    note = Article.create!(title: "a", body: "a body long enough", type: "Whatever")
    sticky = Feature.create!(title: "s", body: "s body long enough")
    [note.id, sticky.id]
  end

  def self.seed_plain
    plain = Article.create!(title: "a", body: "a body long enough")
    sticky = Feature.create!(title: "s", body: "s body long enough")
    [plain.id, sticky.id]
  end

  def self.loaded(id)
    record = Article.find(id)
    [record.class.name, record.type]
  end

  def self.counts
    [Article.count, Feature.count]
  end

  def self.created_type(id)
    Article.find(id).type
  end
end
"#;

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
note, sticky = DisabledProbe.seed
check("a row's type is its own", DisabledProbe.loaded(note), ["Article", "Whatever"])
check("the subclass filters nothing", DisabledProbe.counts, [2, 2])
check("the subclass's constructor leaves type alone", DisabledProbe.created_type(sticky), nil)
puts "sti disabled passed"
"##;

fn app(value: &str) -> emit_and_run::Overlay {
    app_with(&format!("  self.inheritance_column = {value}\n"), "")
}

/// real-blog with a `type` column, `article` written into `Article`'s
/// body and `feature` into `Feature < Article`'s.
fn app_with(article: &str, feature: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"type\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            &format!("{article}  has_many :comments, dependent: :destroy\n"),
        )
        .write("app/models/feature.rb", &format!("class Feature < Article\n{feature}end\n"))
        .write("app/models/disabled_probe.rb", PROBE)
}

#[test]
fn a_model_with_sti_turned_off_keeps_type_as_an_attribute_on_ruby() {
    for value in ["nil", "false"] {
        let run = app(value).run_ruby(SCRIPT);
        run.assert_passes();
        assert!(run.stdout.contains("sti disabled passed"), "{value}: {}", run.stdout);
    }
}

const SUBCLASS_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
plain, sticky = DisabledProbe.seed_plain
check("the subclass filters nothing", DisabledProbe.counts, [2, 2])
check("the subclass's constructor leaves type alone", DisabledProbe.created_type(sticky), nil)
puts "sti disabled passed"
"##;

const STI_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
plain, sticky = DisabledProbe.seed_plain
check("the subclass is scoped", DisabledProbe.counts, [2, 1])
check("the subclass's constructor stamps type", DisabledProbe.created_type(sticky), "Feature")
puts "sti disabled passed"
"##;

/// The last assignment wins (review): `nil` then `"type"` is STI on,
/// `"type"` then `nil` is off.
#[test]
fn the_last_inheritance_column_assignment_wins_on_ruby() {
    let on = app_with("  self.inheritance_column = nil\n  self.inheritance_column = \"type\"\n", "").run_ruby(STI_SCRIPT);
    on.assert_passes();
    assert!(on.stdout.contains("sti disabled passed"), "nil then type: {}", on.stdout);
    let off = app_with("  self.inheritance_column = \"type\"\n  self.inheritance_column = nil\n", "").run_ruby(SUBCLASS_SCRIPT);
    off.assert_passes();
    assert!(off.stdout.contains("sti disabled passed"), "type then nil: {}", off.stdout);
}

/// `Sticky < Article`, a sibling of a `Feature` that turns STI off, and
/// the checks that `Article` still is STI.
const SIBLING_PROBE: &str = r#"class SiblingProbe
  def self.seed
    Sticky.create!(title: "t", body: "t body long enough").id
  end

  def self.counts
    [Article.count, Feature.count, Sticky.count]
  end

  def self.loaded(id)
    record = Article.find(id)
    [record.class.name, record.type]
  end
end
"#;

const SIBLING_SCRIPT: &str = r##"
sibling = SiblingProbe.seed
check("a sibling is scoped", SiblingProbe.counts, [3, 3, 1])
check("a sibling's row is its class", SiblingProbe.loaded(sibling), ["Sticky", "Sticky"])
puts "sibling sti passed"
"##;

/// `app_with` plus the sibling.
fn app_with_sibling(article: &str, feature: &str) -> emit_and_run::Overlay {
    app_with(article, feature)
        .write("app/models/sticky.rb", "class Sticky < Article\nend\n")
        .write("app/models/sibling_probe.rb", SIBLING_PROBE)
}

/// A subclass that sets `nil` itself isn't STI, while its base still is
/// (review): a sibling without the setting is scoped and stamps its type.
#[test]
fn a_subclass_that_turns_sti_off_is_a_plain_subclass_on_ruby() {
    let run = app_with_sibling("", "  self.inheritance_column = nil\n").run_ruby(&format!("{SUBCLASS_SCRIPT}{SIBLING_SCRIPT}"));
    run.assert_passes();
    assert!(run.stdout.contains("sti disabled passed"), "{}", run.stdout);
    assert!(run.stdout.contains("sibling sti passed"), "{}", run.stdout);
}

/// The setting is inherited from an abstract parent model (review): an
/// `ApplicationRecord` that sets `nil` turns STI off for `Article`, so
/// `Feature < Article` is a plain subclass.
#[test]
fn an_abstract_parents_setting_is_inherited_on_ruby() {
    let run = app_with("", "")
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n  self.inheritance_column = nil\n",
        )
        .run_ruby(SUBCLASS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("sti disabled passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_model_with_sti_turned_off_keeps_type_as_an_attribute_on_spinel() {
    let run = app("nil").run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("sti disabled passed"), "{}", run.stdout);
}

/// A fresh directory per call: the survey tests run in parallel in one
/// process, so the pid alone would share one.
fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("roundhouse-sti-disabled-{name}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn survey(value: &str) -> String {
    survey_with(&format!("  self.inheritance_column = {value}\n"), "")
}

fn survey_with(note: &str, sticky: &str) -> String {
    let src = scratch("src");
    let out = scratch("out");
    write(&src, "Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(
        &src,
        "db/schema.rb",
        "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"type\"\n  end\nend\n",
    );
    write(&src, "app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(&src, "app/models/note.rb", &format!("class Note < ApplicationRecord\n{note}end\n"));
    write(&src, "app/models/sticky.rb", &format!("class Sticky < Note\n{sticky}end\n"));
    write(&src, "app/models/sticky_probe.rb", "class StickyProbe\n  def self.first_title\n    Sticky.first.title\n  end\nend\n");
    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "rust", "--allow-unsupported", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .unwrap();
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
    String::from_utf8_lossy(&run.stderr).into_owned()
}

/// `nil` and `false` are modelled, so neither is reported, and the
/// subclass's finder is the untyped subclass's warning. A column name is
/// a custom inheritance column, which isn't modelled: it stays reported.
#[test]
fn turning_sti_off_is_modelled_and_a_custom_column_is_reported() {
    for value in ["nil", "false"] {
        let stderr = survey(value);
        assert!(!stderr.contains("inheritance_column="), "{value}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "`Sticky.first` on a subclass of a model with STI turned off (`inheritance_column = {value}`)"
            )),
            "{value}: {stderr}"
        );
    }
    let stderr = survey("\"kind\"");
    assert!(stderr.contains("inheritance_column= not supported"), "{stderr}");
}

/// A subclass's own `nil` is modelled too (review): not reported as a
/// dropped class-body call, and its finder is the warning, naming it.
#[test]
fn a_subclass_turning_sti_off_is_modelled() {
    let stderr = survey_with("", "  self.inheritance_column = nil\n");
    assert!(!stderr.contains("inheritance_column="), "{stderr}");
    assert!(
        stderr.contains("`Sticky.first` on a subclass that turns STI off (`inheritance_column = nil`)"),
        "{stderr}"
    );
}
