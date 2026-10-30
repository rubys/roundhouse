//! A select-list alias reads back through the model's `[]`, by String or
//! Symbol key, as in Rails.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("db/schema.rb", include_str!("select_alias_index/schema.rb"))
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "app/models/article.rb",
            include_str!("select_alias_index/article.rb"),
        )
        .write(
            "app/models/comment.rb",
            include_str!("select_alias_index/comment.rb"),
        )
}

fn rails() -> String {
    let oracle = std::process::Command::new("ruby")
        .arg("tests/select_alias_index/oracle.rb")
        .output()
        .expect("run Rails oracle");
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    String::from_utf8_lossy(&oracle.stdout).into_owned()
}

#[test]
fn select_aliases_read_like_columns_and_match_rails() {
    let run = app().run_ruby(include_str!("select_alias_index/probe.rb"));
    run.assert_passes();
    assert_eq!(run.stdout, rails());
}

// An indexer the app writes reads its key as given, on the model, an app
// ancestor or an included module.
#[test]
fn an_app_defined_indexer_keeps_its_string_key() {
    let comment = "class Comment < ApplicationRecord\n  belongs_to :article\n  def body_text\n    self[\"body\"]\n  end\nend\n";
    let indexer = "  def [](key)\n    key\n  end\n";
    let read = |overlay: emit_and_run::Overlay| {
        let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
        assert!(errors.is_empty(), "{errors:?}");
        let model = std::fs::read_to_string(emitted.join("app/models/comment.rb")).unwrap();
        model
            .lines()
            .find(|line| line.contains("self["))
            .unwrap()
            .trim()
            .to_string()
    };
    assert_eq!(
        read(app().write("app/models/comment.rb", comment)),
        "self[:body]"
    );
    let overrides = [
        app().write(
            "app/models/comment.rb",
            &comment.replace("  def body_text", &format!("{indexer}  def body_text")),
        ),
        app().write("app/models/comment.rb", comment).write(
            "app/models/application_record.rb",
            &format!("class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n{indexer}end\n"),
        ),
        app()
            .write("app/models/concerns/keyed_reads.rb", &format!("module KeyedReads\n{indexer}end\n"))
            .write(
                "app/models/comment.rb",
                &comment.replace("  belongs_to", "  include KeyedReads\n  belongs_to"),
            ),
    ];
    for overlay in overrides {
        assert_eq!(read(overlay), "self[\"body\"]");
    }
}

// Strict targets ship no Relation to keep a selected row.
#[test]
fn only_ruby_family_indexers_read_the_selected_row() {
    for (target, file, reads) in [
        (BuildTarget::Ruby, "app/models/comment.rb", true),
        (BuildTarget::Spinel, "app/models/comment.rb", true),
        (BuildTarget::Crystal, "src/models/comment.cr", false),
    ] {
        let (emitted, errors) = app()
            .write("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments\nend\n")
            .write("app/models/comment.rb", "class Comment < ApplicationRecord\n  belongs_to :article\n  def self.projected\n    select(\"comments.*, 7 AS extra\")\n  end\nend\n")
            .emit(target);
        assert_eq!(errors.is_empty(), reads, "{target:?}: {errors:?}");
        let model = std::fs::read_to_string(emitted.join(file)).unwrap();
        assert_eq!(
            model
                .split("def [](")
                .nth(1)
                .unwrap()
                .split("\n  def ")
                .next()
                .unwrap()
                .contains("@__selected_aliases"),
            reads,
            "{target:?}: {model}"
        );
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn select_aliases_read_natively() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        include_str!("select_alias_index/probe.rb"),
    );
    let run = app().run_spinel_with_rbs(&script);
    run.assert_passes();
    assert_eq!(run.stdout, rails());
}

#[test]
fn a_select_alias_accepts_string_and_symbol_keys_on_the_same_record() {
    let script = format!(
        "{}\na = Article.counted.first\nputs [a[\"comments_total\"], a[:comments_total]].map {{ |n| n.to_s }}.join(\"=\")\n",
        include_str!("select_alias_index/probe.rb")
    );
    let run = app().run_ruby(&script);
    run.assert_passes();
    let oracle = std::process::Command::new("ruby")
        .arg("-e")
        .arg(
            r#"require_relative "tests/select_alias_index/oracle"
a = Article.counted.first
puts [a["comments_total"], a[:comments_total]].map { |n| n.to_s }.join("=")
"#,
        )
        .output()
        .expect("run Rails same-record oracle");
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    assert_eq!(run.stdout, String::from_utf8_lossy(&oracle.stdout));
    assert_eq!(run.stdout.lines().last(), Some("2=2"));
}

#[test]
fn app_readers_win_without_shadowing_the_selected_attribute() {
    let method = "  def comments_total\n    \"custom\"\n  end\n";
    let article = include_str!("select_alias_index/article.rb");
    let application = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n";
    let concern = format!("module SelectedReader\n  extend ActiveSupport::Concern\n{method}end\n");
    let cases = [
        (
            app().write(
                "app/models/article.rb",
                &article.replacen("  has_many", &format!("{method}  has_many"), 1),
            ),
            format!("class Article\n{method}end\n"),
        ),
        (
            app().write(
                "app/models/application_record.rb",
                &format!("{application}{method}end\n"),
            ),
            format!("class ApplicationRecord\n{method}end\n"),
        ),
        (
            app()
                .write("app/models/concerns/selected_reader.rb", &concern)
                .write(
                    "app/models/article.rb",
                    &article.replacen("  has_many", "  include SelectedReader\n  has_many", 1),
                ),
            format!("{concern}class Article\n  include SelectedReader\nend\n"),
        ),
    ];
    let reads = r##"a = Article.counted.first
puts "#{a.comments_total}=#{a["comments_total"]}=#{a[:comments_total]}"
"##;
    for (overlay, declarations) in cases {
        let script = format!("{}\n{reads}", include_str!("select_alias_index/probe.rb"));
        let run = overlay.run_ruby(&script);
        run.assert_passes();
        let oracle = std::process::Command::new("ruby")
            .arg("-e")
            .arg(format!(
                "require_relative \"tests/select_alias_index/oracle\"\n{declarations}\n{reads}"
            ))
            .output()
            .expect("run Rails override oracle");
        assert!(
            oracle.status.success(),
            "{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        assert_eq!(
            run.stdout,
            String::from_utf8_lossy(&oracle.stdout),
            "{declarations}"
        );
        assert_eq!(run.stdout.lines().last(), Some("custom=2=2"));
    }
}

#[test]
fn reflective_index_and_attribute_reads_match_rails() {
    let mut calls = Vec::new();
    for method in ["[]", "read_attribute"] {
        for key in ["\"comments_total\"", ":comments_total"] {
            for send in ["send", "public_send", "__send__"] {
                calls.push(format!("a.{send}(:{method}, {key})"));
                calls.push(format!("a.{send}(\"{method}\", {key})"));
            }
        }
    }
    let methods = format!(
        "\nclass Article\n  def self.reflective_reads\n    a = counted.first\n    [{}]\n  end\nend\n",
        calls.join(", ")
    );
    let script = format!(
        "{}\nputs Article.reflective_reads.map {{ |n| n.to_s }}.join(\"=\")\n",
        include_str!("select_alias_index/probe.rb")
    );
    let run = app()
        .write(
            "app/models/article.rb",
            &format!(
                "{}{}\nend\n",
                include_str!("select_alias_index/article.rb")
                    .trim_end()
                    .strip_suffix("end")
                    .unwrap(),
                methods
                    .strip_prefix("\nclass Article\n")
                    .unwrap()
                    .strip_suffix("end\n")
                    .unwrap()
            ),
        )
        .run_ruby(&script);
    run.assert_passes();
    let oracle = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require_relative \"tests/select_alias_index/oracle\"\n{methods}\nputs Article.reflective_reads.map {{ |n| n.to_s }}.join(\"=\")\n"))
        .output().expect("run Rails reflective oracle");
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    assert_eq!(run.stdout, String::from_utf8_lossy(&oracle.stdout));
    assert_eq!(
        run.stdout.lines().last(),
        Some(vec!["2"; 24].join("=").as_str())
    );
}

const REVIEW_SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table :probe_records do |t|
    t.string :name
    t.integer :number
  end
end
"#;
const REVIEW_APPLICATION: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

fn review_app(model: &str, runner: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get '/probe', to: 'probe_kit#index'\nend\n")
        .write("db/schema.rb", REVIEW_SCHEMA)
        .write("app/models/application_record.rb", REVIEW_APPLICATION)
        .write("app/models/alias_record.rb", model)
        .write("app/models/probe_kit_runner.rb", runner)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/probe_kit_controller.rb", "class ProbeKitController < ApplicationController\n  def index\n    @observations = ProbeKitRunner.observations\n    render plain: 'probe'\n  end\nend\n")
}

fn review_oracle(model: &str, runner: &str, script: &str) -> serde_json::Value {
    let run = review_app(model, runner).run_ruby(script);
    run.assert_passes();
    let oracle_program = format!(
        r#"require_relative "tests/support/rails_oracle"
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{REVIEW_SCHEMA}
{REVIEW_APPLICATION}
{model}
{runner}
{script}
"#
    );
    let oracle = std::process::Command::new(
        std::env::var("PROBE_RUBY").unwrap_or_else(|_| "ruby".to_owned()),
    )
    .arg("-e")
    .arg(oracle_program)
    .output()
    .expect("run review Rails oracle");
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let expected: serde_json::Value = serde_json::from_slice(&oracle.stdout).expect("oracle JSON");
    let actual: serde_json::Value = serde_json::from_str(&run.stdout).expect("emitted JSON");
    assert_eq!(actual, expected);
    actual
}

const REVIEW_SCRIPT: &str = r#"require "json"
AliasRecord.create!(id: 1, name: "visible", number: 12)
puts JSON.generate(ProbeKitRunner.observations)
"#;

// P2.1: preserve each key's Ruby type before JSON can erase Symbol identity.
#[test]
fn class_indexer_arguments_keep_strings_for_constants_locals_self_and_writers() {
    let runner = include_str!("select_alias_index/review_class_runner.rb");
    let calls = [
        "constant_index",
        "constant_write",
        "local_index",
        "self_index",
        "send_index",
        "public_send_index",
        "__send___index",
        "plain_record_index",
    ];
    let queries = [
        "query_class_index", "query_class_write", "query_class_local",
        "query_class_local_write", "query_class_helper", "query_class_helper_write",
        "instance_index", "instance_write",
    ];
    let observations = format!(
        "  def self.observations\n    [{}]\n  end\n",
        calls.map(|name| format!("probe_{name}")).into_iter()
            .chain(queries.map(|name| format!("AliasRecord.first.{name}")))
            .chain(["unknown_index", "unknown_write", "unknown_helper_index", "unknown_helper_write"]
                .map(|name| format!("AliasRecord.first.{name}(AliasRecord)")))
            .collect::<Vec<_>>().join(", ")
    );
    let runner = format!(
        "{}{observations}end\n",
        runner.trim_end().strip_suffix("end").unwrap()
    );
    let model = include_str!("select_alias_index/review_class_index.rb");
    let (emitted, errors) = review_app(model, &runner).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let source = std::fs::read_to_string(emitted.join("app/models/alias_record.rb")).unwrap();
    assert!(source.contains("self[:name] = \"changed\""), "{source}");
    assert!(source.contains("self[:name]"), "{source}");
    assert!(source.contains("receiver[\"name\"]"), "{source}");
    assert!(source.contains("receiver[\"name\"] = 1"), "{source}");
    let actual = review_oracle(
        model,
        &runner,
        r#"require "json"
AliasRecord.create!(id: 1, name: "visible", number: 12)
observations = ProbeKitRunner.observations
raise "indexer changed String key identity" unless observations.all? { |value| value.is_a?(String) }
symbol_key = AliasRecord[:name]
raise "indexer changed Symbol key identity" unless symbol_key.is_a?(Symbol) && symbol_key == :name
puts JSON.generate(observations + [symbol_key.class.name])
"#,
    );
    assert_eq!(
        actual,
        serde_json::json!([
            "name", "name", "name", "name", "name", "name", "name", "visible",
            "name", "name", "name", "name", "name", "name", "visible", "changed",
            "name", "name", "name", "name", "Symbol"
        ])
    );
}

// Both APIs, both key kinds, direct calls and all reflective spellings,
// with both String and Symbol selectors. Every method reloads its record.
fn review_read_runner(query: &str, key: &str, mutation: &str, reader_control: bool) -> String {
    let mut runner = String::from("class ProbeKitRunner\n");
    let mut observations = Vec::new();
    if reader_control {
        runner.push_str(&format!(
            "  def self.reader\n    AliasRecord.{query}.first.{key}\n  end\n"
        ));
        observations.push("reader".to_owned());
    }
    for api in ["[]", "read_attribute"] {
        for arg in [format!("\"{key}\""), format!(":{key}")] {
            let mut calls = vec![format!("record.{api}({arg})")];
            for send in ["send", "public_send", "__send__"] {
                calls.push(format!("record.{send}(:{api}, {arg})"));
                calls.push(format!("record.{send}(\"{api}\", {arg})"));
            }
            for call in calls {
                let name = format!("read_{}", observations.len());
                runner.push_str(&format!("  def self.{name}\n    record = AliasRecord.{query}.first\n{mutation}    {call}\n  end\n"));
                observations.push(name);
            }
        }
    }
    runner.push_str(&format!(
        "  def self.observations\n    [{}]\n  end\nend\n",
        observations.join(", ")
    ));
    runner
}

// P2.2: quoted SQL must be read after the declared integer writer casts it.
// The extra reader override proves that [] reads storage rather than a method.
#[test]
fn declared_select_alias_reads_its_cast_storage_through_every_spelling() {
    let runner = review_read_runner("projected", "comments_total", "", true);
    for overridden in [false, true] {
        let source = include_str!("select_alias_index/review_typed_alias.rb");
        let model = if overridden {
            format!(
                "{}  def comments_total\n    \"custom\"\n  end\nend\n",
                source.trim_end().strip_suffix("end").unwrap()
            )
        } else {
            source.to_owned()
        };
        let actual = review_oracle(&model, &runner, REVIEW_SCRIPT);
        let mut expected = vec![serde_json::json!(7); 29];
        if overridden {
            expected[0] = serde_json::json!("custom");
        }
        assert_eq!(actual, serde_json::Value::Array(expected));
    }
}

// P2.3: String entry paths must read the live column after an in-memory write.
#[test]
fn string_column_keys_read_current_storage_after_select_and_mutation() {
    let model = include_str!("select_alias_index/review_reflect_columns.rb");
    for query in ["all", "projected"] {
        let runner = review_read_runner(query, "name", "    record[:name] = \"changed\"\n", false);
        let actual = review_oracle(model, &runner, REVIEW_SCRIPT);
        assert_eq!(
            actual,
            serde_json::Value::Array(vec![serde_json::json!("changed"); 28])
        );
    }
}

// P2.3: a projection shadowing a real integer column keeps its existing cast.
#[test]
fn a_shadowed_integer_column_reads_cast_storage_through_every_spelling() {
    let model = "class AliasRecord < ApplicationRecord\n  self.table_name = \"probe_records\"\n  def self.shadowed\n    select(\"id, name, '007' AS number\")\n  end\nend\n";
    let runner = review_read_runner("shadowed", "number", "", true);
    let actual = review_oracle(model, &runner, REVIEW_SCRIPT);
    assert_eq!(
        actual,
        serde_json::Value::Array(vec![serde_json::json!(7); 29])
    );
}
