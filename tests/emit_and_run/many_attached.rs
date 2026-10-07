//! `has_many_attached`, `ActiveStorage::Attachment` finders, and
//! `GlobalID::Locator.locate_signed` — abstract overlays (Invariant 6).

use super::emit_and_run;

fn storage_schema() -> &'static str {
    r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
  end
  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
  end
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.string "slug"
    t.datetime "created_at", null: false
  end
end
"#
}

fn many_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", storage_schema())
        .write(
            "app/models/doc.rb",
            r#"class Doc < ApplicationRecord
  has_many_attached :uploads

  def self.attach_two
    doc = create!(name: "many")
    doc.uploads.attach("one-bytes", "a.png", "image/png")
    doc.uploads.attach("two-bytes", "b.png", "image/png")
    doc
  end

  def self.attach_literal_array
    doc = create!(name: "arr")
    File.write("u1.bin", "hello-bytes")
    File.open("u1.bin", "rb") do |file|
      doc.uploads.attach([{ io: file, filename: "u1.bin" }])
    end
    doc
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn has_many_attached_array_and_attachments_last_run() {
    many_app()
        .run_ruby(
            r##"
doc = Doc.attach_two
raise "not attached" unless doc.uploads.attached?
atts = doc.uploads.attachments
raise "expected 2 attachments, got #{atts.length}" unless atts.length == 2
last = atts.last
raise "missing last" if last.nil?
raise "last filename #{last.filename}" unless last.filename.to_s == "b.png"
raise "last url blank" if last.url.to_s.empty?

one = Doc.attach_literal_array
raise "literal array not attached" unless one.uploads.attached?
raise "literal last missing" if one.uploads.attachments.last.nil?
raise "literal filename #{one.uploads.attachments.last.filename}" unless one.uploads.attachments.last.filename.to_s == "u1.bin"

puts "has_many_attached passed"
"##
        )
        .assert_passes();
}

#[test]
fn attachment_find_by_and_find_run() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", storage_schema())
        .write(
            "app/models/doc.rb",
            "class Doc < ApplicationRecord\n  has_one_attached :file\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r##"
doc = Doc.create!(name: "findable")
doc.file.attach("hello-bytes", "cover.png", "image/png")
row = doc.file
raise "missing attachment row" unless row.attached?
# The join row is what Attachment finds.
att = ActiveStorage::Attachment.where(record_type: "Doc", record_id: doc.id, name: "file").first
raise "Attachment.where missed" if att.nil?
found = ActiveStorage::Attachment.find(att.id)
raise "Attachment.find missed" if found.nil?
raise "find id mismatch" unless found.id == att.id
by_name = ActiveStorage::Attachment.find_by(name: "file", record_id: doc.id)
raise "find_by missed" if by_name.nil?
raise "filename via model #{found.filename}" unless found.filename.to_s == "cover.png"
raise "url blank" if found.url.to_s.empty?
puts "Attachment finders passed"
"##
        )
        .assert_passes();
}

#[test]
fn locate_signed_mint_and_find_run() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        )
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r##"
doc = Doc.create!(name: "signed")
sgid = GlobalID.signed("Doc", doc.id, :uploads)
found = GlobalID::Locator.locate_signed(sgid, only: Doc, for: :uploads)
raise "locate_signed missed" if found.nil?
raise "wrong id #{found.id}" unless found.id == doc.id
raise "wrong name #{found.name}" unless found.name == "signed"
bad = GlobalID::Locator.locate_signed(sgid, only: Doc, for: :other)
raise "wrong purpose must be nil" unless bad.nil?
puts "locate_signed passed"
"##
        )
        .assert_passes();
}
