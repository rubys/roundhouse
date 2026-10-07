use super::emit_and_run;

fn schema() -> &'static str {
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
    t.datetime "created_at", null: false
  end
end
"#
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", schema())
        .write(
            "app/models/doc.rb",
            r#"class Doc < ApplicationRecord
  has_one_attached :file

  def self.attach_positional
    doc = create!(name: "positional")
    doc.file.attach("hello-bytes", "cover.png", "image/png")
    doc
  end

  def self.attach_literal
    doc = create!(name: "literal")
    File.write("literal.bin", "hello-bytes")
    File.open("literal.bin", "rb") do |file|
      doc.file.attach(io: file, filename: "cover.png")
    end
    doc
  end

  def self.with_upload
    File.write("cover.bin", "hello-bytes")
    File.open("cover.bin", "rb") do |file|
      yield({ io: file, filename: "cover.png" })
    end
  end

  def self.with_upload_kwargs
    File.write("cover2.bin", "hello-bytes")
    File.open("cover2.bin", "rb") do |file|
      yield io: file, filename: "cover.png"
    end
  end

  def self.attach_from_yield
    doc = create!(name: "bag")
    with_upload do |attachment|
      doc.file.attach(attachment)
    end
    doc
  end

  def self.attach_from_kwargs_yield
    doc = create!(name: "kwargs")
    with_upload_kwargs do |attachment|
      doc.file.attach(attachment)
    end
    doc
  end

  def self.attach_pdf_by_filename
    doc = create!(name: "pdf")
    File.write("report.bin", "hello-bytes")
    File.open("report.bin", "rb") do |file|
      doc.file.attach(io: file, filename: "report.pdf")
    end
    doc
  end

  def self.reattach_same_blob
    doc = attach_positional
    blob = doc.file.blob
    raise "missing blob" if blob.nil?
    doc.file.attach_blob(blob)
    doc
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn attach_positional_io_hash_and_yielded_bag_run() {
    app()
        .run_ruby(
            r##"
def check(doc, label)
  raise "#{label} attached?" unless doc.file.attached?
  raise "#{label} filename #{doc.file.filename}" unless doc.file.filename.to_s == "cover.png"
  blob = doc.file.blob
  raise "#{label} blob" if blob.nil?
  raise "#{label} bytes #{blob.download.inspect}" unless blob.download == "hello-bytes"
  raise "#{label} type #{doc.file.content_type}" unless doc.file.content_type == "image/png"
end

check(Doc.attach_positional, "positional")
check(Doc.attach_literal, "literal")
check(Doc.attach_from_yield, "bag")
check(Doc.attach_from_kwargs_yield, "kwargs")

pdf = Doc.attach_pdf_by_filename
raise "pdf type #{pdf.file.content_type}" unless pdf.file.content_type == "application/pdf"

same = Doc.reattach_same_blob
raise "same-blob still attached?" unless same.file.attached?
blob = same.file.blob
raise "same-blob missing" if blob.nil?
raise "same-blob bytes lost" unless blob.download == "hello-bytes"

puts "attach forms passed"
"##
        )
        .assert_passes();
}
