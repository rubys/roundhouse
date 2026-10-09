//! A blobs.yml row written as Rails' generator and Writebook write it,
//!
//!     cover_blob: <%= ActiveStorage::FixtureSet.blob filename: "notes.txt", service_name: "test" %>
//!
//! is the whole row rendered by ERB, not label → fields, and used to stop
//! the ingest ("is a scalar, where Rails expects label → fields"). Rails
//! reads the file from `test/fixtures/files`, measures and uploads it, and
//! inserts the row; an attachments.yml row then points at it by label.
//! The emitted program has to do the same: the attachment's blob is the
//! file, bytes and all. (Kept out of tests/emit_and_run.rs so concurrent
//! appends there do not conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ACTIVE_STORAGE_TABLES: &str = r#"  create_table "active_storage_blobs", force: :cascade do |t|
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

  add_foreign_key "comments", "articles"
"#;

fn blog_with_fixture_blobs(blobs_yml: &str, attachments_yml: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "  add_foreign_key \"comments\", \"articles\"\n", ACTIVE_STORAGE_TABLES)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  has_one_attached :cover\n",
        )
        .write("test/fixtures/files/notes.txt", "hello fixture\n")
        .write("test/fixtures/files/other.txt", "other bytes\n")
        .write("test/fixtures/active_storage/blobs.yml", blobs_yml)
        .write("test/fixtures/active_storage/attachments.yml", attachments_yml)
}

/// Writebook's shape: one blob from the generator's call, one attachment
/// naming it by label. Every column Rails' `unfurl` fills is checked
/// against the file, and the bytes against the service.
#[test]
fn a_fixture_set_blob_row_loads_the_file_as_the_attached_blob() {
    blog_with_fixture_blobs(
        "cover_blob: <%= ActiveStorage::FixtureSet.blob filename: \"notes.txt\", service_name: \"test\" %>\n",
        "cover:\n  name: cover\n  record: one (Article)\n  blob: cover_blob\n",
    )
    .write(
        "test/models/article_cover_test.rb",
        r##"require "test_helper"

class ArticleCoverTest < ActiveSupport::TestCase
  test "the fixture blob is the file" do
    cover = articles(:one).cover
    assert cover.attached?
    assert_equal "notes.txt", cover.filename.to_s
    assert_equal "text/plain", cover.content_type
    assert_equal 14, cover.byte_size
    assert_equal "hello fixture\n", cover.blob.download
    assert_equal "notes.txt", active_storage_blobs(:cover_blob).filename.to_s
    assert_not articles(:two).cover.attached?
  end

  test "the row carries Rails' checksum, service name and metadata" do
    row = ActiveRecord.adapter.select_rows("SELECT checksum, service_name, metadata FROM active_storage_blobs")
    assert_equal 1, row.length
    assert_equal "Xmdqx5njW8DiKpoqe3gQjA==", row[0]["checksum"]
    assert_equal "test", row[0]["service_name"]
    assert_equal "{\"identified\":true}", row[0]["metadata"]
  end
end
"##,
    )
    .run_test("test/models/article_cover_test.rb")
    .assert_passes();
}

/// Two rows from the call in one file: each gets its own key (the key
/// column is unique, and the key is derived from the clock, which a
/// test may have stopped) and its own id, so the attachment that names the
/// second label reaches the second file. `content_type:` overrides the
/// detected type, as Rails' `assign_attributes` after `unfurl` does.
#[test]
fn each_fixture_set_blob_row_is_its_own_blob() {
    blog_with_fixture_blobs(
        "first_blob: <%= ActiveStorage::FixtureSet.blob filename: \"notes.txt\" %>\n\
         again_blob: <%= ActiveStorage::FixtureSet.blob filename: \"notes.txt\" %>\n\
         other_blob: <%= ActiveStorage::FixtureSet.blob filename: \"other.txt\", content_type: \"application/x-custom\" %>\n",
        "cover:\n  name: cover\n  record: two (Article)\n  blob: other_blob\n",
    )
    .write(
        "test/models/article_cover_test.rb",
        r##"require "test_helper"

class ArticleCoverTest < ActiveSupport::TestCase
  test "the attachment reaches the row its label names" do
    cover = articles(:two).cover
    assert_equal "other.txt", cover.filename.to_s
    assert_equal "application/x-custom", cover.content_type
    assert_equal "other bytes\n", cover.blob.download
    assert_equal 3, ActiveStorage::Blob.count
    assert_not_equal active_storage_blobs(:first_blob).key, active_storage_blobs(:again_blob).key
  end

  # The key is derived from the clock; a stopped one (a test that froze
  # time while the fixtures reload) must not make two rows' keys equal.
  test "a stopped clock still gives each row its own key" do
    ActiveSupport.freeze(1_700_000_000)
    SchemaSetup.reset!
    assert_equal 3, ActiveStorage::Blob.count
  ensure
    ActiveSupport.travel(0)
  end
end
"##,
    )
    .run_test("test/models/article_cover_test.rb")
    .assert_passes();
}

fn blobs_fixture(yml: &str) -> Result<roundhouse::dialect::Fixture, roundhouse::ingest::IngestError> {
    roundhouse::ingest::ingest_fixture_file(
        yml.as_bytes(),
        std::path::Path::new("test/fixtures/active_storage/blobs.yml"),
        std::path::Path::new("test/fixtures"),
    )
}

/// The call's arguments become the row, at its file position, so the
/// id an attachment's `blob:` resolves to is the one Rails' order gives.
#[test]
fn a_fixture_set_blob_row_ingests_as_its_arguments() {
    let f = blobs_fixture(
        "a: <%= ActiveStorage::FixtureSet.blob filename: \"reading.webp\", service_name: \"test\" %>\n\
         b: <%= ActiveStorage::FixtureSet.blob(filename: \"x.png\", content_type: \"image/png\") %>\n",
    )
    .expect("a FixtureSet.blob row is a row");
    let labels: Vec<&str> = f.records.keys().map(|l| l.as_str()).collect();
    assert_eq!(labels, ["a", "b"]);
    let a = &f.file_blobs[&roundhouse::Symbol::from("a")];
    assert_eq!(a.filename, "reading.webp");
    assert_eq!(a.service_name.as_deref(), Some("test"));
    assert_eq!(a.content_type, None);
    let b = &f.file_blobs[&roundhouse::Symbol::from("b")];
    assert_eq!(b.content_type.as_deref(), Some("image/png"));
}

/// What the loader cannot pass through stays the gap it was, rather
/// than loading a row without it: an attribute beyond `service_name` /
/// `content_type`, a filename that is not a literal, a path out of
/// `test/fixtures/files`, and any other whole-row ERB.
#[test]
fn an_unmodeled_whole_row_tag_stays_a_gap() {
    for yml in [
        "a: <%= ActiveStorage::FixtureSet.blob filename: \"x.webp\", metadata: { analyzed: true } %>\n",
        "a: <%= ActiveStorage::FixtureSet.blob filename: name %>\n",
        "a: <%= ActiveStorage::FixtureSet.blob filename: \"../secrets.yml\" %>\n",
        "a: <%= ActiveStorage::FixtureSet.blob %>\n",
        "a: <%= Something.row %>\n",
    ] {
        let err = blobs_fixture(yml).expect_err(yml);
        assert!(format!("{err:?}").contains("is a scalar"), "{yml}: {err:?}");
    }
}

/// Only the ruby family has a storage service to upload the file to; the
/// other targets refuse the app rather than load the set without it.
#[test]
fn targets_without_a_storage_service_refuse_a_fixture_set_blob_row() {
    let root = roundhouse::fixtures::real_blog();
    let mut app = roundhouse::ingest::ingest_app(root).expect("ingest the blog");
    app.fixtures.push(
        blobs_fixture("cover_blob: <%= ActiveStorage::FixtureSet.blob filename: \"notes.txt\" %>\n")
            .expect("a FixtureSet.blob row is a row"),
    );
    let err = roundhouse::project::target_files(&app, root, roundhouse::project::BuildTarget::Rust)
        .expect_err("rust has no storage service");
    assert!(err.contains("ActiveStorage::FixtureSet.blob"), "{err}");
}
