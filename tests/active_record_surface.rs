//! The column indexer and class-side existence predicate share Rails' public inputs.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :email\n    t.integer :visits\n  end\nend\n")
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  def column(name)
    self[name]
  end

  def self.matching?(conditions)
    exists?(conditions)
  end
end
"#)
}

#[test]
fn string_column_keys_read_the_same_attribute_as_symbols() {
    app()
        .run_ruby(
            r#"
record = Widget.new(email: "alice", visits: 7)
key = ["email"].first
raise "String key misses its column" unless record[key] == "alice"
raise "String and Symbol reads differ" unless record[key] == record[:email]
raise "dynamic String read misses its column" unless record.column(key) == "alice"
raise "dynamic Symbol read regresses" unless record.column(:email) == "alice"
raise "Integer column loses its type" unless record.column("visits") == 7
raise "nil column read differs" unless Widget.new.column("email").nil?
raise "unknown String key changes behavior" unless record.column("missing").nil?
raise "unknown Symbol key changes behavior" unless record.column(:missing).nil?
record[key] = "updated"
raise "String write differs from its read" unless record.column(:email) == "updated"
record[:email] = "symbol"
raise "Symbol write regresses" unless record.column(key) == "symbol"
record["visits"] = 9
raise "Integer write loses its type" unless record.column(:visits) == 9
record["email"] = nil
raise "nil write differs from its read" unless record.column(key).nil?
"#,
        )
        .assert_passes();

    // String | Symbol stays dynamic on these soft-strict targets so
    // parameter-derived keys still compile. Base and its overrides agree.
    for (target, base, model, declaration) in [
        (
            roundhouse::project::BuildTarget::CSharp,
            "app/runtime/ActiveRecordBase.cs",
            "app/models/Widget.cs",
            "this[object? ",
        ),
        (
            roundhouse::project::BuildTarget::Swift,
            "Sources/App/ActiveRecordBase.swift",
            "Sources/App/app/models/Widget.swift",
            "subscript(name: Any?)",
        ),
    ] {
        let (emitted, errors) = app()
            .write(
                "app/models/widget.rb",
                "class Widget < ApplicationRecord\nend\n",
            )
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let base = std::fs::read_to_string(emitted.join(base)).unwrap();
        let model = std::fs::read_to_string(emitted.join(model)).unwrap();
        assert!(
            base.contains(declaration),
            "{target:?} Base key type differs from its model"
        );
        assert!(
            model.contains(declaration),
            "{target:?} model key type differs from Base"
        );
    }
}

#[test]
fn class_exists_applies_condition_hashes_and_preserves_id_queries() {
    app()
        .run_ruby(
            r#"
raise "empty table has a row" if Widget.exists?({})
Db.exec("INSERT INTO widgets (id, email, visits) VALUES (1, 'alice', 7), (2, 'bob', NULL)")
raise "Hash misses its row" unless Widget.exists?(email: "alice")
raise "Hash ignores its conditions" if Widget.exists?(email: "nobody")
raise "String Hash key misses its row" unless Widget.exists?({"email" => "alice"})
raise "compound conditions ignored" if Widget.exists?(email: "alice", visits: 9)
raise "NULL condition misses its row" unless Widget.exists?(visits: nil)
raise "array condition misses its row" unless Widget.exists?(email: ["alice", "nobody"])
raise "empty IN matches a row" if Widget.exists?(email: [])
raise "empty Hash misses rows" unless Widget.exists?({})
raise "model method loses conditions" unless Widget.matching?({email: "alice"})
raise "model method ignores conditions" if Widget.matching?({email: "nobody"})
raise "no argument regresses" unless Widget.exists?
raise "Integer id regresses" unless Widget.exists?(1)
raise "String id regresses" unless Widget.exists?("1")
raise "missing id exists" if Widget.exists?(99)
raise "nil id exists" if Widget.exists?(nil)
raise "false id exists" if Widget.exists?(false)
begin
  Widget.exists?(1, 2)
  raise "extra arguments accepted"
rescue ArgumentError
end
"#,
        )
        .assert_passes();
}
