//! Enum declaration defaults survive canonical mapping expansion.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::Literal;
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn both_declaration_forms_preserve_the_default_label() {
    for declaration in [
        "enum :priority, { low: -3, high: 7 }, default: :high",
        "enum priority: { low: -3, high: 7 }, _default: :high",
        "enum :priority, { low: -3, high: 7 }, default: \"high\"",
    ] {
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.integer \"priority\", default: -3\n  end\nend\n".to_string()),
            ("app/models/article.rb", format!("class Article < ApplicationRecord\n  {declaration}\nend\n")),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> = files
            .into_iter()
            .map(|(path, source)| (PathBuf::from(path), source.into_bytes()))
            .collect();
        let app = ingest_app_from_tree(tree).expect("ingest default declaration");
        let article = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == "Article")
            .unwrap();
        assert_eq!(
            article.enum_defaults.get(&Symbol::from("priority")),
            Some(&Literal::Int { value: 7 }),
            "{declaration}"
        );
    }
}

#[test]
fn a_serialized_default_keeps_its_stored_value() {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"severity\", default: \"mild\"\n  end\nend\n"),
        ("app/services/rating.rb", "class Rating < T::Enum\n  enums do\n    High = new(\"severe\")\n    Low = new(\"mild\")\n  end\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  enum :severity, Rating.values.to_h { |v| [v.serialize, v.serialize] }, default: :severe\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest serialized default");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert_eq!(
        article.enum_defaults.get(&Symbol::from("severity")),
        Some(&Literal::Str {
            value: "severe".into()
        })
    );
}
//! Defaults are constructor inputs, not validating writes or fixture data.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::Literal;
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn both_declaration_forms_preserve_the_default_label() {
    for declaration in [
        "enum :priority, { low: -3, high: 7 }, default: :high",
        "enum priority: { low: -3, high: 7 }, _default: :high",
        "enum :priority, { low: -3, high: 7 }, default: \"high\"",
    ] {
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.integer \"priority\", default: -3\n  end\nend\n".to_string()),
            ("app/models/article.rb", format!("class Article < ApplicationRecord\n  {declaration}\nend\n")),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> = files
            .into_iter()
            .map(|(path, source)| (PathBuf::from(path), source.into_bytes()))
            .collect();
        let app = ingest_app_from_tree(tree).expect("ingest default declaration");
        let article = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == "Article")
            .unwrap();
        assert_eq!(
            article.enum_defaults.get(&Symbol::from("priority")),
            Some(&Literal::Int { value: 7 }),
            "{declaration}"
        );
    }
}

#[test]
fn a_serialized_default_keeps_its_stored_value() {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"severity\", default: \"mild\"\n  end\nend\n"),
        ("app/services/rating.rb", "class Rating < T::Enum\n  enums do\n    High = new(\"severe\")\n    Low = new(\"mild\")\n  end\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  enum :severity, Rating.values.to_h { |v| [v.serialize, v.serialize] }, default: :severe\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest serialized default");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert_eq!(
        article.enum_defaults.get(&Symbol::from("severity")),
        Some(&Literal::Str {
            value: "severe".into()
        })
    );
}

#[test]
fn fixture_omissions_take_schema_defaults_not_model_enum_defaults() {
    for (mapping, inherited) in [
        ("{ mild: \"mild\", severe: \"severe\" }", false),
        (
            "Rating.values.to_h { |v| [v.serialize, v.serialize] }",
            false,
        ),
        ("{ mild: \"mild\", severe: \"severe\" }", true),
    ] {
        let declarations = format!("enum :severity, {mapping}, default: :severe\n  enum :priority, {{ low: -3, high: 7 }}, default: :high");
        let mut overlay = emit_and_run::real_blog();
        let parent = if inherited {
            overlay = overlay.write("app/models/default_base.rb", &format!(
            "class DefaultBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  {declarations}\nend\n"));
            "DefaultBase"
        } else {
            "ApplicationRecord"
        };
        overlay
        .write("app/services/rating.rb", "class Rating < T::Enum\n  enums do\n    High = new(\"severe\")\n    Low = new(\"mild\")\n  end\nend\n")
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"severity\", default: \"mild\", null: false\n    t.integer \"priority\"\n    t.boolean \"flag\", default: true")
        .edit("app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            &format!("class Article < {parent}\n  has_many :comments, dependent: :destroy\n  {}", if inherited { "" } else { &declarations }))
        .write("test/fixtures/articles.yml", "one:\n  title: First fixture\n  body: A long enough first fixture body.\ntwo:\n  title: Explicit fixture\n  body: A long enough second fixture body.\n  severity: severe\n  priority: -3\n")
        .write("test/models/default_phases_test.rb", r#"require "test_helper"
class DefaultPhasesTest < ActiveSupport::TestCase
  test "model defaults apply to new but not omitted fixture columns" do
    assert_equal "severe", Article.new.severity
    assert_equal "high", Article.new.priority
    assert_equal "mild", Article.new(severity: :mild).severity
    assert_nil Article.new(priority: nil).priority
    assert_equal false, Article.new(flag: false).flag
    one = articles(:one)
    two = articles(:two)
    assert_equal "mild", one.severity
    assert_nil one.priority
    assert_equal "severe", two.severity
    assert_equal "low", two.priority
    assert_equal "mild", ActiveRecord.adapter.find("articles", one.id)["severity"]
    assert_nil ActiveRecord.adapter.find("articles", one.id)["priority"]
    assert_equal "severe", ActiveRecord.adapter.find("articles", two.id)["severity"]
    assert_equal -3, ActiveRecord.adapter.find("articles", two.id)["priority"]
  end
end
"#)
        .run_test("test/models/default_phases_test.rb").assert_passes();
    }
}

#[test]
fn raw_defaults_and_both_hydration_factories_bypass_enum_assignment_validation() {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0\n    t.boolean \"flag\", default: true")
        .write("app/models/direct_base.rb", "class DirectBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  enum :state, { draft: 0, live: 3 }\nend\n")
        .write("app/models/concerns/local_state.rb", "module LocalState\n  extend ActiveSupport::Concern\n  included { enum :state, { queued: 2, shipped: 7 } }\nend\n")
        .write("app/models/special_article.rb", "class SpecialArticle < DirectBase\n  self.table_name = \"articles\"\n  include LocalState\n  def stored_state\n    state_before_type_cast\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < DirectBase")
        .run_ruby(r#"raise "child default should read nil" unless SpecialArticle.new.state.nil?
raise "parent mapping leaked" unless Article.new.state == "draft"
raise "explicit nil lost" unless SpecialArticle.new(state: nil).state.nil?
shipped = SpecialArticle.create!(title: "Shipped", body: "A long enough shipped body.", state: :shipped, flag: false)
raise "explicit child value lost" unless SpecialArticle.find(shipped.id).state == "shipped"
raise "false attrs lost" unless SpecialArticle.find(shipped.id).flag == false
raise "wrong stored child value" unless ActiveRecord.adapter.find("articles", shipped.id)["state"] == 7
[0, "draft", "bogus"].each do |invalid|
  begin
    SpecialArticle.new(state: invalid)
  rescue ArgumentError
    next
  end
  raise "invalid explicit assignment accepted: #{invalid}"
end
ordinary = Article.create!(title: "Raw default", body: "A long enough raw default body.")
raw = ActiveRecord.adapter.find("articles", ordinary.id)
raise "SQL default lost" unless raw["state"] == 0
row = SpecialArticleRow.from_raw(raw)
loaded = SpecialArticle.from_row(row)
raise "raw row validated" unless loaded.state.nil? && loaded.stored_state == 0
raise "find validated storage" unless SpecialArticle.find(ordinary.id).state.nil?
stmt = Db.prepare("SELECT id, state, flag, title, body, created_at, updated_at FROM articles WHERE id = #{ordinary.id}")
begin
  raise "missing SQL row" unless Db.step?(stmt)
  positional = SpecialArticle.from_stmt(stmt)
  raise "raw stmt validated" unless positional.state.nil? && positional.stored_state == 0
ensure
  Db.finalize(stmt)
end
raise "child altered parent" unless Article.find(ordinary.id).state == "draft"
"#).assert_passes();
}

#[test]
fn fixture_population_does_not_add_a_constructor_phase_argument() {
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"severity\", default: \"mild\", null: false\n  end\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  enum :severity, { mild: \"mild\", severe: \"severe\" }, default: :severe\n  after_initialize { self.severity = :severe }\nend\n"),
        ("test/fixtures/articles.yml", "one: {}\n"),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest fixture constructor");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let model = &files.iter().find(|f| f.path.ends_with("app/models/article.rb")).unwrap().content;
    let fixture = &files.iter().find(|f| f.path.ends_with("test/fixtures/articles.rb")).unwrap().content;
    assert!(model.contains("def initialize(attrs = ActiveRecord::Base::EMPTY_ATTRS)"), "{model}");
    assert!(!model.contains("schema_defaults"), "fixture policy must not enter initialize: {model}");
    // Retain the existing hook machinery and constructor-before-population
    // order, not Rails callback-free fixture parity (a separate runtime gap).
    assert!(model.contains("after_initialize if !(attrs.equal? ActiveRecord::Base::HYDRATE_ATTRS)"), "{model}");
    let construction = fixture.find("instance = Article.new\n").expect("ordinary construction");
    let population = fixture.find("instance._write_severity_raw \"mild\"").expect("raw schema population");
    let save = fixture.find("instance.save_after_validation").expect("existing fixture save");
    assert!(construction < population && population < save, "{fixture}");
}

#[test]
#[ignore = "requires ActiveRecord 8.1.4 and sqlite3"]
fn native_rails_enum_default_and_hydration_reference() {
    let output = std::process::Command::new("ruby")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/enum_default_reference.rb"))
        .output().expect("ruby must be installed");
    assert!(output.status.success(), "native Rails reference failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
