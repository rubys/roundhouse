//! `validates :image, presence: true` on a `has_one_attached :image`
//! (or `has_many_attached :images`) names the ATTACHMENT, and Rails asks
//! its reader — `read_attribute_for_validation` is `send(:image)`, and
//! the proxy is blank when nothing is attached (`Attached::One#blank?`
//! is `!attached?`; `Attached::Many` answers `blank?` through
//! `attachments.empty?`). An attachment has no `@image` slot, so the
//! column check read an ivar nothing writes and rejected every record,
//! attached or not.
//!
//! Each expectation below is what Rails 8.1.4 answers for the same
//! models (ActiveStorage on SQLite, Disk service): a record with no
//! attachment is invalid, one with an attachment staged by the writer
//! or saved is valid, `on: :update` skips the create context and
//! `if:` gates the rule. (Kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const TABLES: &str = r#"  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
    t.index ["key"], name: "index_active_storage_blobs_on_key", unique: true
  end

  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end

  create_table "photos", force: :cascade do |t|
    t.string "title"
    t.boolean "live"
  end

  create_table "galleries", force: :cascade do |t|
    t.string "title"
  end

  add_foreign_key "comments", "articles"
"#;

fn blog_with_attachments() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "  add_foreign_key \"comments\", \"articles\"\n", TABLES)
        .write(
            "app/models/photo.rb",
            "class Photo < ApplicationRecord\n  has_one_attached :image\n  validates :image, presence: true, if: :live?\nend\n",
        )
        .write(
            "app/models/gallery.rb",
            "class Gallery < ApplicationRecord\n  has_many_attached :images\n  validates :images, presence: true, on: :update\nend\n",
        )
}

#[test]
fn presence_of_an_attachment_asks_whether_it_is_attached() {
    blog_with_attachments()
        .write(
            "test/models/attachment_presence_test.rb",
            r##"require "test_helper"

class AttachmentPresenceTest < ActiveSupport::TestCase
  def blob
    ActiveStorage::Blob.create_and_upload!(io: StringIO.new("abc"), filename: "a.txt", content_type: "text/plain")
  end

  test "a has_one_attached with nothing attached is blank" do
    photo = Photo.new(title: "t", live: true)
    assert_not photo.valid?
    assert_equal ["Image can't be blank"], photo.errors.full_messages
  end

  test "a staged has_one_attached is present before the save" do
    photo = Photo.new(title: "t", live: true)
    photo.image = blob
    assert photo.valid?, photo.errors.full_messages.inspect
    assert photo.save
  end

  test "a saved has_one_attached is present on a fresh read" do
    photo = Photo.new(title: "t", live: true)
    photo.image = blob
    photo.save!
    assert Photo.find(photo.id).valid?
  end

  test "the rule's if: still gates the attachment check" do
    assert Photo.new(title: "t", live: false).valid?
  end

  test "a has_many_attached with no attachments is blank on update" do
    gallery = Gallery.create!(title: "g")
    assert_not gallery.valid?
    assert_equal ["Images can't be blank"], gallery.errors.full_messages
  end

  test "a has_many_attached with one attachment is present" do
    gallery = Gallery.create!(title: "g")
    gallery.images.attach("abc", "a.txt", "text/plain")
    assert Gallery.find(gallery.id).valid?
  end
end
"##,
        )
        .run_test("test/models/attachment_presence_test.rb")
        .assert_passes();
}

/// The strict targets get the same check through the reader, which
/// every target synthesizes, rather than a read of an `@image` /
/// `@images` field no target declares. Shown on the Spinel tree, whose
/// Ruby is the shape every other emitter prints from.
#[test]
fn the_check_reads_the_reader_not_an_ivar() {
    let (emitted, errors) = blog_with_attachments().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let photo = std::fs::read_to_string(emitted.join("app/models/photo.rb")).expect("photo.rb");
    let gallery = std::fs::read_to_string(emitted.join("app/models/gallery.rb")).expect("gallery.rb");
    assert!(photo.contains("if @image_pending.nil? && !(self.image.attached?)"), "{photo}");
    assert!(gallery.contains("if !(self.images.attached?)"), "{gallery}");
    assert!(!photo.contains("@image.nil?") && !gallery.contains("@images.nil?"), "{photo}\n{gallery}");
}
