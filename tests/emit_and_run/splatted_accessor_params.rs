use super::emit_and_run;

const ACCESSORS: &str = r#"
  ATTRIBUTES = %i[note]
  WRITERS = %i[writer_note]
  READERS = %i[readonly_note]
  attr_accessor :literal_note, *ATTRIBUTES
  attr_writer *WRITERS
  attr_reader *READERS

  attr_reader :writer_note

  def readonly_note
    "fixed"
  end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table :entries do |t|
    t.string :title
  end
end
"#)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/entry.rb", &format!(r#"class Entry < ApplicationRecord
{ACCESSORS}
  PRIVATE_ATTRIBUTES = %i[secret_note]
  attr_accessor :literal_secret_note, *PRIVATE_ATTRIBUTES

  private

  def secret_note=(value)
    raise "private splatted writer called"
  end

  def literal_secret_note=(value)
    raise "private literal writer called"
  end
end
"#))
        .write("app/models/delivery_form.rb", &format!(r#"class DeliveryForm
  include ActiveModel::Model
  include ActiveModel::Attributes
  attribute :identifier, :integer
{ACCESSORS}
  PRIVATE_ATTRIBUTES = %i[secret_note]
  attr_accessor *PRIVATE_ATTRIBUTES

  private

  def secret_note=(value)
    raise "private writer called"
  end
end
"#))
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/entries_controller.rb", r#"class EntriesController < ApplicationController
  def create
    entry = Entry.new(entry_params)
    render json: { title: entry.title }
  end

  private

  def entry_params
    params.expect(entry: %i[title note literal_note writer_note readonly_note secret_note literal_secret_note])
  end
end
"#)
        .write("app/controllers/delivery_forms_controller.rb", r#"class DeliveryFormsController < ApplicationController
  def create
    form = DeliveryForm.new(delivery_form_params)
    render json: { identifier: form.identifier }
  end

  private

  def delivery_form_params
    params.expect(delivery_form: %i[identifier note literal_note writer_note readonly_note secret_note])
  end
end
"#)
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  post "/entries", to: "entries#create"
  post "/delivery_forms", to: "delivery_forms#create"
end
"#)
}

const TABLELESS: &str = r#"
params = DeliveryFormParams.from_raw({"delivery_form" => {"identifier" => "12", "note" => "form note", "literal_note" => "literal", "writer_note" => "writer", "readonly_note" => "ignored", "secret_note" => "ignored"}})
form = DeliveryForm.from_params(params)
raise form.identifier.inspect unless form.identifier == 12
raise form.note.inspect unless form.note == "form note"
raise form.literal_note.inspect unless form.literal_note == "literal"
raise form.writer_note.inspect unless form.writer_note == "writer"
raise form.readonly_note.inspect unless form.readonly_note == "fixed"
raise form.secret_note.inspect unless form.secret_note.nil?
puts "tableless splatted accessor params passed"
"#;

const SCHEMA_FACTORY: &str = r#"
params = EntryParams.from_raw({"entry" => {"title" => "Title", "note" => "entry note", "literal_note" => "literal", "writer_note" => "writer", "readonly_note" => "ignored", "secret_note" => "ignored", "literal_secret_note" => "ignored"}})
entry = Entry.from_params(params)
raise entry.title.inspect unless entry.title == "Title"
raise entry.note.inspect unless entry.note == "entry note"
raise entry.literal_note.inspect unless entry.literal_note == "literal"
raise entry.writer_note.inspect unless entry.writer_note == "writer"
raise entry.readonly_note.inspect unless entry.readonly_note == "fixed"
raise entry.secret_note.inspect unless entry.secret_note.nil?
raise entry.literal_secret_note.inspect unless entry.literal_secret_note.nil?
puts "schema splatted accessor params passed"
"#;

const SCHEMA_UPDATE: &str = r#"
entry = Entry.create!(title: "Initial")
raise "update failed" unless entry.update(title: "Updated", note: "updated note", literal_note: "literal", writer_note: "writer", readonly_note: "ignored")
raise entry.note.inspect unless entry.note == "updated note"
raise entry.literal_note.inspect unless entry.literal_note == "literal"
raise entry.writer_note.inspect unless entry.writer_note == "writer"
raise entry.readonly_note.inspect unless entry.readonly_note == "fixed"
raise entry.secret_note.inspect unless entry.secret_note.nil?
raise entry.literal_secret_note.inspect unless entry.literal_secret_note.nil?
raise "title not saved" unless Entry.find(entry.id).title == "Updated"
puts "schema splatted accessor update passed"
"#;

const SCHEMA_PARAMS_UPDATE: &str = r#"
entry = Entry.create!(title: "Initial")
params = EntryParams.from_raw({"entry" => {"title" => "Updated params", "note" => "updated note", "literal_note" => "literal", "secret_note" => "ignored", "literal_secret_note" => "ignored"}})
raise "update failed" unless entry.update_from_entry_params(params)
raise entry.note.inspect unless entry.note == "updated note"
raise entry.literal_note.inspect unless entry.literal_note == "literal"
raise entry.secret_note.inspect unless entry.secret_note.nil?
raise entry.literal_secret_note.inspect unless entry.literal_secret_note.nil?
raise "title not saved" unless Entry.find(entry.id).title == "Updated params"
puts "schema params update skips private accessor writers"
"#;

#[test]
fn tableless_factory_assigns_splatted_accessor_and_writer_values() {
    app().run_ruby(TABLELESS).assert_passes();
}

#[test]
fn schema_factory_assigns_splatted_accessor_and_writer_values() {
    app().run_ruby(SCHEMA_FACTORY).assert_passes();
}

#[test]
fn schema_update_assigns_splatted_accessor_and_writer_values() {
    app().run_ruby(SCHEMA_UPDATE).assert_passes();
}

#[test]
fn schema_params_update_skips_private_accessor_writers() {
    app().run_ruby(SCHEMA_PARAMS_UPDATE).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn spinel_gate_splatted_accessor_params() {
    app()
        .run_spinel_with_rbs(&format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{TABLELESS}\n{SCHEMA_FACTORY}\n{SCHEMA_UPDATE}\n{SCHEMA_PARAMS_UPDATE}"
        ))
        .assert_passes();
}

fn merge_app(key: &str) -> emit_and_run::Overlay {
    app()
        .write("app/models/entry_builder.rb", &format!(r#"class EntryBuilder
  def self.build(entry_params)
    Entry.new(entry_params.merge({key}: "merged"))
  end
end
"#))
        .edit("app/controllers/entries_controller.rb", "Entry.new(entry_params)", "EntryBuilder.build(entry_params)")
}

#[test]
fn params_merge_declines_private_accessor_writers() {
    for key in ["secret_note", "literal_secret_note"] {
        let (emitted, errors) = merge_app(key).emit(roundhouse::project::BuildTarget::Ruby);
        assert!(errors.is_empty(), "{errors:?}");
        let builder = std::fs::read_to_string(emitted.join("app/models/entry_builder.rb")).unwrap();
        assert!(builder.contains("entry_params.merge("), "{builder}");
        assert!(!builder.contains("_pm0"), "{builder}");
    }
}

#[test]
fn params_merge_keeps_public_accessor_writers_and_columns() {
    for key in ["note", "literal_note", "writer_note", "title"] {
        merge_app(key).run_ruby(&format!(r#"
params = EntryParams.from_raw({{"entry" => {{"title" => "Title"}}}})
entry = EntryBuilder.build(params)
raise entry.{key}.inspect unless entry.{key} == "merged"
"#)).assert_passes();
    }
}

#[test]
fn params_merge_keeps_columns_with_private_writers() {
    let (emitted, _, _) = merge_app("title")
        .edit("app/models/entry.rb", "  private\n", "  private\n\n  def title=(value)\n    raise value\n  end\n")
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    let builder = std::fs::read_to_string(emitted.join("app/models/entry_builder.rb")).unwrap();
    assert!(builder.contains("_pm0.title = \"merged\""), "{builder}");
}
