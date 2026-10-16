//! `has_markdown` records compose with Active Storage's ordinary
//! `has_many_attached` abstraction, including a nested concern module.
//! This proves reusable Rails behavior, not Writebook's renderer or upload
//! authorization.

use super::emit_and_run;

#[test]
fn markdown_uploads_attach_and_reload_through_the_action_text_record() {
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
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
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
"#,
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_markdown :body\nend\n",
        )
        .write(
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    belongs_to :record, polymorphic: true
    include ActionText::Markdown::Uploads
  end
end
"#,
        )
        .write(
            "app/models/action_text/markdown/uploads.rb",
            r#"module ActionText
  class Markdown < Record
    module Uploads
      extend ActiveSupport::Concern
      included do
        has_many_attached :uploads
      end
    end
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
article = Article.create!(title: "Markdown with upload")
article.body = "# A chapter"
article.save!
markdown = article.reload.body
markdown.uploads.attach("chapter-bytes", "chapter.md", "text/markdown")

reloaded = ActionText::Markdown.find(markdown.id)
raise "markdown content lost: #{reloaded.content.inspect}" unless reloaded.content == "# A chapter"
attachments = reloaded.uploads.attachments
raise "expected one upload, got #{attachments.length}" unless attachments.length == 1
upload = attachments.first
raise "upload missing" if upload.nil?
raise "upload filename: #{upload.filename}" unless upload.filename.to_s == "chapter.md"
raise "upload bytes lost" unless upload.blob.download == "chapter-bytes"
puts "ActionText Markdown upload persistence passed"
"##,
        )
        .assert_passes();
}
