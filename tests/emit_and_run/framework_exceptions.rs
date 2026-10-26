//! ActiveRecord exception classes the ruby runtime defines: a rescue
//! names the class, and RecordNotDestroyed keeps the record it was
//! built with. ConnectionFailed and ConnectionTimeoutError are
//! ConnectionNotEstablished.

use super::emit_and_run;

#[test]
fn active_record_exceptions_rescue_with_their_payload() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        )
        .write(
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  def self.destroyed_message(record)
    raise ActiveRecord::RecordNotDestroyed.new("still referenced", record)
  rescue ActiveRecord::RecordNotDestroyed => error
    error.record.title + ":" + error.message
  end

  def self.timeout_message
    raise ActiveRecord::ConnectionTimeoutError, "timed out"
  rescue ActiveRecord::ConnectionNotEstablished => error
    error.class.name + ":" + error.message
  end

  def self.decryption_message
    raise ActiveRecord::Encryption::Errors::Decryption, "bad key"
  rescue ActiveRecord::Encryption::Errors::Decryption => error
    error.message
  end
end
"#,
        )
        .run_ruby(
            r#"
note = Note.create!(title: "Ada")
raise Note.destroyed_message(note) unless Note.destroyed_message(note) == "Ada:still referenced"
raise Note.timeout_message unless Note.timeout_message == "ActiveRecord::ConnectionTimeoutError:timed out"
raise Note.decryption_message unless Note.decryption_message == "bad key"
"#,
        )
        .assert_passes();
}
