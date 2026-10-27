//! A subclass of a concrete model whose table has no `type` column
//! (#1415): `Feature < Article`, `articles` without one. That isn't STI.
//! activerecord 8.1.4 shares the table and filters nothing, and every
//! row a subclass's finder loads is the subclass (`Feature.first.class`
//! is `Feature`). Here the finder is the base's, inherited, and loads
//! `Article`s. Counts and constructors already match.
//!
//! Each finder call on such a subclass is now a survey warning naming
//! both classes; the call is still emitted. Its oracle:
//!
//!     ruby tests/sti_untyped/oracle.rb

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-sti-untyped-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const PROBE: &str = r#"class FeatureProbe
  def self.first_title
    Feature.order(:id).first.title
  end

  def self.titles
    Feature.where(title: "f").map(&:title)
  end

  def self.scoped
    Feature.titled_f.to_a
  end

  def self.found(id)
    Feature.find(id)
  end

  def self.count
    Feature.count
  end

  def self.built
    Feature.create!(title: "n")
  end
end
"#;

fn app(src: &Path, columns: &str, probe: &str) {
    write(src, "Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(
        src,
        "db/schema.rb",
        &format!("ActiveRecord::Schema.define do\n  create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n{columns}  end\nend\n"),
    );
    write(src, "app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(src, "app/models/article.rb", "class Article < ApplicationRecord\n  scope :titled_f, -> { where(title: \"f\") }\nend\n");
    write(src, "app/models/feature.rb", "class Feature < Article\nend\n");
    write(src, "app/models/feature_probe.rb", probe);
}

fn survey(src: &Path, out: &Path) -> String {
    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "--survey", "-o"])
        .arg(out)
        .arg(src)
        .output()
        .unwrap();
    String::from_utf8_lossy(&run.stderr).into_owned()
}

/// `first`, `where`, the base's scope and `find` load records, so each
/// is a warning naming the class Rails answers and the one this build
/// does. `count` and `create!` already match Rails, so they aren't.
#[test]
fn a_finder_on_a_subclass_without_a_type_column_is_reported() {
    let src = scratch("plain-src");
    let out = scratch("plain-out");
    app(&src, "", PROBE);
    let stderr = survey(&src, &out);
    for call in ["Feature.order", "Feature.where", "Feature.titled_f", "Feature.find"] {
        assert!(
            stderr.contains(&format!(
                "`{call}` on a subclass of a model whose table has no `type` column not supported (all targets): Rails loads its records as `Feature`; this build loads them as `Article`"
            )),
            "{call} is reported: {stderr}"
        );
    }
    for call in ["Feature.count", "Feature.create!"] {
        assert!(!stderr.contains(&format!("`{call}`")), "{call} matches Rails: {stderr}");
    }
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// With a `type` column the subclass is STI, `lower::sti_scope` scopes
/// its finders, and nothing is reported.
#[test]
fn an_sti_subclass_is_not_reported() {
    let src = scratch("sti-src");
    let out = scratch("sti-out");
    app(&src, "    t.string \"type\"\n", PROBE);
    let stderr = survey(&src, &out);
    assert!(!stderr.contains("has no `type` column"), "{stderr}");
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

const AGGREGATES: &str = r#"class FeatureProbe
  def self.nothing
    Feature.none.to_a
  end

  def self.counted
    Feature.where(title: "f").count
  end

  def self.plucked
    Feature.order(:id).pluck(:title)
  end

  def self.found_inside
    Feature.where(title: Feature.find_by(title: "f").title).exists?
  end

  def self.find_then_count
    Feature.find([1, 2]).count
  end
end
"#;

/// `none` loads nothing, and an aggregate's relation (`where(…).count`,
/// `order(…).pluck`) answers a value, so neither is reported. A finder
/// in the aggregate's arguments still is. An eager `find` under `.count`
/// still loads rows, so it is reported.
#[test]
fn none_and_an_aggregates_relation_are_not_reported() {
    let src = scratch("value-src");
    let out = scratch("value-out");
    app(&src, "", AGGREGATES);
    let stderr = survey(&src, &out);
    for call in ["Feature.none", "Feature.where", "Feature.order"] {
        assert!(!stderr.contains(&format!("`{call}`")), "{call} loads no record: {stderr}");
    }
    assert!(stderr.contains("`Feature.find_by` on a subclass"), "the argument's finder is reported: {stderr}");
    assert!(
        stderr.contains("`Feature.find` on a subclass"),
        "eager find under count still loads rows: {stderr}"
    );
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The STI lowering rewrites test bodies too, so a finder in a test is
/// reported like one in the app.
#[test]
fn a_finder_in_a_test_is_reported() {
    let src = scratch("test-src");
    let out = scratch("test-out");
    app(&src, "", "class FeatureProbe\nend\n");
    write(
        &src,
        "test/models/feature_test.rb",
        "require \"test_helper\"\n\nclass FeatureTest < ActiveSupport::TestCase\n  test \"finds a feature\" do\n    assert_nil Feature.find_by(title: \"f\")\n  end\nend\n",
    );
    let stderr = survey(&src, &out);
    assert!(
        stderr.lines().any(|l| l.contains("feature_test.rb") && l.contains("`Feature.find_by` on a subclass")),
        "{stderr}"
    );
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}
