//! The public finder casts request strings before its schema-typed adapter.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Zero and boundary rows make silent coercion return a visibly wrong record.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/article.rb", "class Article < ApplicationRecord\nend\n")
        .write("app/models/small_key.rb", "class SmallKey < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    render plain: Article.find(params[:id]).title\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:show]\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
  create_table "small_keys", id: :integer, force: :cascade do |t|
    t.string "title"
  end
end
"#)
}

const ASSERTIONS: &str = r#"
Db.exec("INSERT INTO articles (id, title) VALUES (0, 'zero'), (1, 'one'), (10, 'ten'), (9223372036854775807, 'maximum'), (-9223372036854775808, 'minimum')")
Db.exec("INSERT INTO small_keys (id, title) VALUES (2147483647, 'maximum32'), (-2147483648, 'minimum32'), (2147483648, 'beyond32')")
[["0", "zero"], ["1suffix", "one"], ["1_0", "ten"], ["1__0", "one"], ["0_0", "zero"], ["+0001junk", "one"], ["-0000junk", "zero"], [" \t+1suffix", "one"], ["9223372036854775807", "maximum"], ["-9223372036854775808", "minimum"]].each do |input, title|
  record = Article.find(input)
  raise "wrong row for #{input}: #{record.title}" unless record.title == title
end
raise "integer zero" unless Article.find(0).title == "zero"
raise "integer maximum" unless Article.find(9223372036854775807).title == "maximum"
raise "integer minimum" unless Article.find(-9223372036854775808).title == "minimum"
minimum_id = Article.find("-9223372036854775808").id
raise "MIN key became nil" if minimum_id.nil?
raise "MIN key hydration" unless minimum_id == -9223372036854775808
[nil, "", " ", "abc", "+", "9223372036854775808", "-9223372036854775809", "999999999999999999999999999999999999999"].each do |input|
  begin
    Article.find(input)
    raise "invalid input matched a row: #{input.inspect}"
  rescue ActiveRecord::RecordNotFound
  end
end
raise "maximum32" unless SmallKey.find("2147483647").title == "maximum32"
raise "minimum32" unless SmallKey.find("-2147483648").title == "minimum32"
raise "SQLite integer keys are 64-bit" unless SmallKey.find("2147483648").title == "beyond32"
puts "PASS schema-typed primary-key casting"
"#;

// Rails adds charset=utf-8 to plain responses; this preserves the existing
// emitted header while checking the real generated controller call path.
const CONTROLLER_ASSERTIONS: &str = r#"
require_relative "app/controllers/articles_controller"
[["-9223372036854775808", "minimum"], ["9223372036854775807", "maximum"], ["1suffix", "one"]].each do |input, expected|
  controller = ArticlesController.new
  controller.params = {"id" => input}
  controller.process_action(:show)
  raise "controller status" unless controller.status == 200
  raise "controller body" unless controller.body == expected
  raise "controller content type" unless controller.content_type == "text/plain"
end
"#;

#[test]
/// Execute model and controller lookups with real zero and boundary rows.
fn request_primary_keys_are_cast_before_the_emitted_ruby_adapter() {
    app().run_ruby(&format!("{ASSERTIONS}\n{CONTROLLER_ASSERTIONS}")).assert_passes();
}

/// Include real lowered Float call sites, not only a post-emission consumer.
fn float_input_app() -> emit_and_run::Overlay {
    app().edit("app/models/article.rb", "class Article < ApplicationRecord\nend\n", r#"class Article < ApplicationRecord
  def self.tiny_float_title
    find(1e-7).title
  end
  def self.huge_float_title
    find(1e20).title
  end
end
"#)
}

const FLOAT_ASSERTIONS: &str = r#"
Db.exec("INSERT INTO articles (id, title) VALUES (0, 'zero'), (1, 'one'), (-1, 'negative one')")
raise "lowered Float call aliased row one" unless Article.tiny_float_title == "zero"
[-1e-7, 1e-7, 0.0].each do |input|
  raise "direct tiny Float" unless Article.find(input).title == "zero"
  raise "relation tiny Float" unless Article.all.find(input).title == "zero"
end
raise "positive fraction" unless Article.find(1.9).title == "one"
raise "negative fraction" unless Article.find(-1.9).title == "negative one"
raise "String exponent retains decimal-prefix semantics" unless Article.find("1e-7").title == "one"
[1e20, -1e20, 9223372036854775808.0, 1.0 / 0.0, -1.0 / 0.0, 0.0 / 0.0].each do |input|
  begin
    Article.find(input)
    raise "invalid Float matched a direct row"
  rescue ActiveRecord::RecordNotFound
  end
  begin
    Article.all.find(input)
    raise "invalid Float matched a relation row"
  rescue ActiveRecord::RecordNotFound
  end
end
begin
  Article.huge_float_title
  raise "lowered overflowing Float call aliased row one"
rescue ActiveRecord::RecordNotFound
end
puts "PASS numeric Float finder inputs"
"#;

#[test]
/// Scientific notation and non-finite values must never select an unrelated row.
fn float_primary_key_inputs_preserve_numeric_values_in_emitted_ruby() {
    float_input_app().run_ruby(FLOAT_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain and RBS extractor"]
/// Exercise finite truncation and rejection independently of the existing MIN controls.
fn float_primary_key_inputs_preserve_numeric_values_natively() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{FLOAT_ASSERTIONS}");
    run_seeded_native(float_input_app(), &script);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
/// Keep the native full-range control visible, including upstream MIN failures.
fn request_primary_keys_are_cast_before_the_native_adapter() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}\n{CONTROLLER_ASSERTIONS}");
    run_seeded_native(app(), &script);
}

#[test]
#[ignore = "requires the Spinel toolchain and RBS extractor"]
/// Execute changed request semantics independently so a MIN dependency failure
/// cannot hide invalid-input checks or return a real zero/maximum row unnoticed.
fn request_casting_cases_execute_independently_of_native_minimum_dependencies() {
    run_seeded_native(app(), r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
Db.exec("INSERT INTO articles (id, title) VALUES (0, 'zero'), (1, 'one'), (10, 'ten'), (9223372036854775807, 'maximum')")
[["0", "zero"], ["1suffix", "one"], ["1_0", "ten"], ["1__0", "one"], ["0_0", "zero"], ["+0001junk", "one"], ["-0000junk", "zero"], [" \t+1suffix", "one"], ["9223372036854775807", "maximum"]].each do |input, expected|
  raise "direct finder changed row" unless Article.find(input).title == expected
  raise "relation finder changed row" unless Article.all.find(input).title == expected
end
raise "integer zero" unless Article.find(0).title == "zero"
raise "integer maximum row" unless Article.find(9223372036854775807).title == "maximum"
[nil, "", " ", "abc", "+", "9223372036854775808", "-9223372036854775809", "999999999999999999999999999999999999999"].each do |input|
  begin
    Article.find(input)
    raise "invalid input matched a direct row"
  rescue ActiveRecord::RecordNotFound
  end
  begin
    Article.all.find(input)
    raise "invalid input matched a relation row"
  rescue ActiveRecord::RecordNotFound
  end
end
puts "PASS seeded request casting independent of minimum dependencies"
"#);
}

/// String/named keys share the public finder but must not enter numeric casting.
fn string_key_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  self.primary_key = \"identifier\"\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", primary_key: "identifier", id: :string, force: :cascade do |t|
    t.string "name"
  end
end
"#)
}

const STRING_ASSERTIONS: &str = r#"
Db.exec("INSERT INTO widgets (identifier, name) VALUES ('6bc805da-cbb2-4319-a164-694810a2495f', 'uuid text'), ('1_0', 'literal underscore'), ('', 'empty key'), ('12', 'integer input')")
[["6bc805da-cbb2-4319-a164-694810a2495f", "uuid text"], ["1_0", "literal underscore"], ["", "empty key"]].each do |key, expected|
  record = Widget.find(key)
  raise "string key body" unless record.name == expected
  raise "string key hydration" unless record.id == key
end
raise "integer input string key" unless Widget.find(12).name == "integer input"
begin
  Widget.find(nil)
  raise "nil matched string key"
rescue ActiveRecord::RecordNotFound
end
puts "PASS named String primary key preservation"
"#;

#[test]
/// Check exact named-key identity separately from numeric normalization.
fn string_primary_keys_preserve_the_public_input_in_emitted_ruby() {
    string_key_app().run_ruby(STRING_ASSERTIONS).assert_passes();
}

#[test]
/// A custom noninteger key must retain its adapter conversion and select the
/// fractional row, even when the truncated integer would match another record.
fn fractional_primary_keys_preserve_the_existing_emitted_ruby_adapter_behavior() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/article.rb", "class Article < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", id: :float, force: :cascade do |t|
    t.string "title"
  end
end
"#)
        .run_ruby(r#"
Db.exec("INSERT INTO articles (id, title) VALUES (1, 'one'), (1.5, 'fraction')")
["1.5", 1.5].each do |input|
  record = Article.find(input)
  raise "fractional key truncated" unless record.title == "fraction"
  raise "fractional key hydration" unless record.id == 1.5
end
puts "PASS existing fractional-key adapter behavior"
"#).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
/// Exercise the String-key model with the same emitted RBS used in production.
fn string_primary_keys_preserve_the_public_input_natively() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{STRING_ASSERTIONS}");
    run_seeded_native(string_key_app(), &script);
}

const RELATION_ASSERTIONS: &str = r#"
Db.exec("INSERT INTO articles (id, title) VALUES (0, 'zero'), (1, 'one'), (10, 'ten'), (9223372036854775807, 'maximum'), (-9223372036854775808, 'minimum')")
[["1suffix", "one"], ["1_0", "ten"], ["0_0", "zero"], ["9223372036854775807", "maximum"], ["-9223372036854775808", "minimum"]].each do |input, expected|
  record = Article.all.find(input)
  raise "relation key cast" unless record.title == expected
end
[nil, "abc", "9223372036854775808", "-9223372036854775809"].each do |input|
  begin
    Article.all.find(input)
    raise "invalid relation key matched a row"
  rescue ActiveRecord::RecordNotFound
  end
end
puts "PASS existing Relation.find cast helper"
"#;

#[test]
/// Existing Relation.find must receive the same cast as the direct finder.
fn relation_find_reuses_the_shared_cast_in_emitted_ruby() {
    app().run_ruby(RELATION_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain; retains the signed-minimum dependency control"]
/// Retain a seeded native relation control, including the minimum-value dependency.
fn relation_find_reuses_the_shared_cast_natively() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{RELATION_ASSERTIONS}");
    run_seeded_native(app(), &script);
}

/// Compile untouched generated models with their emitted RBS and report both
/// compiler diagnostics and execution failures. Scalar-only consumers can
/// replace a primitive to expose conversions hidden by SQLite escape helpers.
fn run_seeded_native(app: emit_and_run::Overlay, script: &str) {
    use std::process::Command;
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("strict_adapter.rb"), format!("require_relative \"boot\"\n{script}")).unwrap();
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let build = Command::new(&compiler).args(["--rbs", ".", "strict_adapter.rb", "-o", "strict_adapter"])
        .current_dir(&emitted).output().expect("compile seeded adapter consumer");
    std::fs::write(emitted.join("compile.stdout"), &build.stdout).unwrap();
    std::fs::write(emitted.join("compile.stderr"), &build.stderr).unwrap();
    assert!(!String::from_utf8_lossy(&build.stderr).contains("type seeds are unavailable"), "RBS extractor is required: {}", String::from_utf8_lossy(&build.stderr));
    assert!(build.status.success(), "seeded compile in {}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&build.stdout), String::from_utf8_lossy(&build.stderr));
    let run = Command::new(emitted.join("strict_adapter")).current_dir(&emitted).output().unwrap();
    assert!(run.status.success(), "seeded execution in {}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
}

#[test]
#[ignore = "requires the Spinel toolchain and RBS extractor"]
/// An Integer-only primitive must never compile a dead String conversion branch.
fn schema_selected_normalization_reaches_an_integer_only_adapter() {
    run_seeded_native(app(), r#"
class Article
  def self._adapter_find_by_id(id)
    record = Article.new
    record.title = (id + 1).to_s
    record
  end
end
raise "integer adapter input" unless Article.find("1suffix").title == "2"
begin
  Article.find("abc")
  raise "invalid input reached adapter"
rescue ActiveRecord::RecordNotFound
end
puts "PASS seeded Integer-only adapter boundary"
"#);
}

#[test]
#[ignore = "requires the Spinel toolchain and RBS extractor"]
/// String normalization must reach a String-only primitive without numeric coercion.
fn schema_selected_normalization_reaches_a_string_only_adapter() {
    run_seeded_native(string_key_app(), r#"
class Widget
  def self._adapter_find_by_id(id)
    record = Widget.new
    record.name = id.upcase
    record
  end
end
raise "string adapter input" unless Widget.find("literal_key").name == "LITERAL_KEY"
raise "integer input string adapter" unless Widget.find(12).name == "12"
begin
  Widget.find(nil)
  raise "nil reached adapter"
rescue ActiveRecord::RecordNotFound
end
puts "PASS seeded String-only adapter boundary"
"#);
}

#[test]
/// Ruby-family raw request inputs must not widen a strict target's scalar
/// adapter or its call-site coercions, or introduce an unshipped runtime owner.
fn strict_rust_finders_retain_the_existing_scalar_contract() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let base = std::fs::read_to_string(emitted.join("src/active_record_base.rs")).unwrap();
    let model = std::fs::read_to_string(emitted.join("src/models/article.rs")).unwrap();
    let controller = std::fs::read_to_string(emitted.join("src/controllers/articles_controller.rs")).unwrap();
    assert!(!base.contains("IntegerKeyCast"), "strict runtime must not ship union casting");
    assert!(!model.contains("_find_primary_key_input"), "strict model must retain scalar dispatch");
    assert!(model.contains("pub fn find(id: i64)"), "{model}");
    assert!(controller.contains(".as_i64().unwrap()"), "{controller}");
}

#[test]
#[ignore = "requires the Rust toolchain; compiles and executes an emitted app"]
/// Compile the independently Rails-generated blog and execute its scalar
/// finder against SQLite. This is preservation coverage, not raw-ID support.
fn strict_rust_scalar_finder_still_executes() {
    let (emitted, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(emitted.join("tests/scalar_finder.rs"), r#"
use app::{db::{self, Db}, models::Article, schema_sql};
/// Preserve a real scalar-key SQLite lookup through the emitted Rust model.
#[test]
fn scalar_finder() {
    db::setup_test_db(schema_sql::CREATE_TABLES);
    Db::exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (1, 'one', 'body', '2026-01-01', '2026-01-01')");
    assert_eq!(Article::find(1).title().as_deref(), Some("one"));
}
"#).unwrap();
    let output = std::process::Command::new("cargo")
        .args(["test", "--test", "scalar_finder", "-j", "2"])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(&emitted).output().expect("execute emitted Rust finder");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
