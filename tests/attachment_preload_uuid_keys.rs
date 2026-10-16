//! An attachment preload over uuid record keys.
//!
//! `includes(photos_attachments: :blob)` (and `with_attached_photos`)
//! batch-load every record's attachment rows in one query. The emitted
//! preloader built that query's `IN (...)` with the integer list helper
//! and keyed the rows by `record_id.to_i`, so over a uuid primary key the
//! list held each id's leading digits, no row matched, and every
//! preloaded record answered no attachments, where the lazy reader on
//! the same record found them. A has_many :through batch keyed by the
//! owner's id grouped its rows the same way.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn attachment_preload_finds_rows_over_uuid_keys() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.string "name"
  end

  create_table "tags", force: :cascade do |t|
    t.string "name"
  end

  create_table "taggings", force: :cascade do |t|
    t.uuid "widget_id", null: false
    t.integer "tag_id", null: false
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
    t.uuid "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end
end
"#,
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  has_many :taggings\n  has_many :tags, through: :taggings\n  has_many_attached :photos\n  has_one_attached :cover\nend\n",
        )
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .write("app/models/tagging.rb", "class Tagging < ApplicationRecord\n  belongs_to :widget\n  belongs_to :tag\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def show
    plain = Widget.find(params[:id])
    preloaded = Widget.includes(photos_attachments: :blob, cover_attachment: :blob).find(params[:id])
    render json: {
      plain: plain.photos.map { |p| p.filename.to_s },
      preloaded: preloaded.photos.map { |p| p.filename.to_s },
      cover: preloaded.cover.attached?,
      tags: Widget.includes(:tags).find(params[:id]).tags.map(&:name)
    }
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: %i[show]\nend\n")
        .run_ruby(
            r##"
w = Widget.create!(id: "00000000-0000-4000-8000-000000000001", name: "alpha")
ActiveRecord.adapter.insert("active_storage_blobs", { "key" => "k1", "filename" => "a.jpg", "content_type" => "image/jpeg", "metadata" => "{}", "service_name" => "local", "byte_size" => 1, "checksum" => "x", "created_at" => "2026-01-01 00:00:00" })
ActiveRecord.adapter.insert("active_storage_blobs", { "key" => "k2", "filename" => "c.png", "content_type" => "image/png", "metadata" => "{}", "service_name" => "local", "byte_size" => 1, "checksum" => "y", "created_at" => "2026-01-01 00:00:00" })
ActiveRecord.adapter.insert("active_storage_attachments", { "name" => "photos", "record_type" => "Widget", "record_id" => w.id, "blob_id" => 1, "created_at" => "2026-01-01 00:00:00" })
ActiveRecord.adapter.insert("active_storage_attachments", { "name" => "cover", "record_type" => "Widget", "record_id" => w.id, "blob_id" => 2, "created_at" => "2026-01-01 00:00:00" })
t = Tag.create!(name: "red")
Tagging.create!(widget_id: w.id, tag_id: t.id)
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/#{w.id}", "QUERY_STRING" => "", "HTTP_ACCEPT" => "application/json", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
want = %({"plain":["a.jpg"],"preloaded":["a.jpg"],"cover":true,"tags":["red"]})
raise "#{status} #{text}" unless status == 200 && text == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}
