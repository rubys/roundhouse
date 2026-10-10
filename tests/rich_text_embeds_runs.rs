//! campfire 2393f01's `MessageTest` reads two pieces of Action Text and
//! Active Record that were missing:
//!
//! * `message.body.embeds` — Rails' record class declares
//!   `has_many_attached :embeds` and fills it before each save from the
//!   blobs its body's attachment nodes name; `with_rich_text_<attr>_
//!   and_embeds` preloads them beneath the rich text.
//! * `message.association(:rich_text_body).loaded?` — the reflection
//!   spelling of the rich text's load-once flag.
//!
//! and its `Page` scopes (`scope :last_page, -> { last_page_of(n) }`,
//! answering an Array subclass) were treated as relations, so
//! `page.first(2)` became the relation-only `first_n`. (Kept out of
//! tests/emit_and_run.rs so concurrent appends there do not conflict;
//! same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const TABLES: &str = r##"  create_table "action_text_rich_texts", force: :cascade do |t|
    t.text "body"
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "updated_at", null: false
    t.index ["record_type", "record_id", "name"], name: "index_action_text_rich_texts_uniqueness", unique: true
  end

  create_table "active_storage_attachments", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.index ["blob_id"], name: "index_active_storage_attachments_on_blob_id"
    t.index ["record_type", "record_id", "name", "blob_id"], name: "index_active_storage_attachments_uniqueness", unique: true
  end

  create_table "active_storage_blobs", force: :cascade do |t|
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.string "content_type"
    t.datetime "created_at", null: false
    t.string "filename", null: false
    t.string "key", null: false
    t.text "metadata"
    t.string "service_name", null: false
    t.index ["key"], name: "index_active_storage_blobs_on_key", unique: true
  end

  create_table "articles", force: :cascade do |t|
"##;

const ARTICLE_HEAD: &str = r##"class Article < ApplicationRecord
  has_rich_text :content

  class Batch < Array
    def self.load(relation, size)
      new(relation.last(size))
    end
  end

  scope :newest, -> { Batch.load(order(:id), 2) }
  scope :newest_with_content, -> { with_rich_text_content_and_embeds.newest }

  def content_loaded?
    association(:rich_text_content).loaded?
  end

  def self.first_of_newest
    newest.first(1)
  end
"##;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|\n",
            TABLES,
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", ARTICLE_HEAD)
}

/// The fill: an attachment node naming a blob attaches it on save, a
/// repeated node attaches it once, a stale reference is skipped, and a
/// body that drops the node detaches it.
#[test]
fn a_rich_text_attaches_the_blobs_its_body_embeds() {
    overlay()
        .run_ruby(
            r##"
blob = ActiveStorage::Blob.create_and_upload!("hello", "hello.txt", "text/plain")
other = ActiveStorage::Blob.create_and_upload!("world", "world.txt", "text/plain")
node = ->(b) { "<action-text-attachment sgid=\"#{ActionText::SignedGlobalId.generate("ActiveStorage::Blob", b.id)}\"></action-text-attachment>" }
stale = "<action-text-attachment sgid=\"#{ActionText::SignedGlobalId.generate("ActiveStorage::Blob", 999)}\"></action-text-attachment>"
article = Article.new(title: "Embeds", body: "A body long enough to pass.")
article.content = "<div>#{node.(blob)}#{node.(blob)}#{stale}#{node.(other)}</div>"
article.save!
ids = Article.find(article.id).content.embeds.attachments.map { |a| a.blob.id }
raise "embeds after create: #{ids.inspect}" unless ids == [blob.id, other.id]
names = []
Article.find(article.id).content.embeds.each { |a| names << a.filename.to_s }
raise "each: #{names.inspect}" unless names == ["hello.txt", "world.txt"]

article = Article.find(article.id)
article.content = "<div>only #{node.(other)}</div>"
article.save!
ids = Article.find(article.id).content.embeds.attachments.map { |a| a.blob.id }
raise "embeds after edit: #{ids.inspect}" unless ids == [other.id]
puts "embeds ok"
"##,
        )
        .assert_passes();
}

/// `with_rich_text_<attr>_and_embeds` preloads the embeds beneath the
/// rich text: once the group's preload has run, walking every record's
/// embeds asks the database nothing. `association(:rich_text_<attr>)
/// .loaded?` reports the load-once flag — false on a record nothing has
/// read, true once the rich text is in hand (an implicit receiver here;
/// the explicit one, as campfire's test spells it, below).
#[test]
fn the_embeds_preload_beneath_the_rich_text_and_loaded_reports_it() {
    overlay()
        .run_ruby(
            r##"
blob = ActiveStorage::Blob.create_and_upload!("hello", "hello.txt", "text/plain")
sgid = ActionText::SignedGlobalId.generate("ActiveStorage::Blob", blob.id)
2.times do |i|
  a = Article.new(title: "Embeds #{i}", body: "A body long enough to pass.")
  a.content = "<div>#{i}<action-text-attachment sgid=\"#{sgid}\"></action-text-attachment></div>"
  a.save!
end

plain = Article.find(Article.last.id)
raise "fresh record reports loaded" if plain.content_loaded?
plain.content
raise "read record reports unloaded" unless plain.content_loaded?

loaded = Article.with_rich_text_content_and_embeds.to_a
loaded.first.content
sql = Db.capture_sql do
  loaded.each { |a| a.content.embeds.each { |e| e.filename } }
end
raise "embeds were not preloaded: #{sql.inspect}" unless sql.empty?
raise "preloaded record reports unloaded" unless loaded.all?(&:content_loaded?)
puts "preload ok"
"##,
        )
        .assert_passes();
}

/// A scope whose body answers an Array (`Batch.load(...)`) still threads
/// the relation in, but what it returns is not a relation: `first(1)` on
/// it is `Array#first`, not the relation-only `first_n`.
#[test]
fn a_scope_answering_an_array_keeps_array_first() {
    overlay()
        .run_ruby(
            r##"
3.times { |i| Article.create!(title: "Batch #{i}", body: "A body long enough to pass.") }
got = Article.first_of_newest
raise "first_of_newest: #{got.inspect}" unless got.map(&:title) == ["Batch 1"]
batch = Article.newest_with_content
raise "newest is not the Batch: #{batch.class}" unless batch.is_a?(Article::Batch)
puts "batch ok"
"##,
        )
        .assert_passes();
}

/// The explicit-receiver spelling inside a test body, as campfire's
/// `MessageTest` writes it on a block parameter.
#[test]
fn association_loaded_reads_the_flag_from_a_test_body() {
    overlay()
        .write(
            "test/models/article_loaded_test.rb",
            r##"require "test_helper"

class ArticleLoadedTest < ActiveSupport::TestCase
  test "association loaded reports the rich text flag" do
    article = Article.new(title: "Loaded", body: "A body long enough to pass.")
    article.content = "<div>hi</div>"
    article.save!
    fresh = Article.find(article.id)
    assert_not fresh.association(:rich_text_content).loaded?
    fresh.content.to_plain_text
    assert fresh.association(:rich_text_content).loaded?
    unread = Article.where(id: article.id).to_a
    assert unread.none? { |a| a.association(:rich_text_content).loaded? }
    preloaded = Article.where(id: article.id).with_rich_text_content.to_a
    assert preloaded.all? { |a| a.association(:rich_text_content).loaded? }
  end
end
"##,
        )
        .run_test("test/models/article_loaded_test.rb")
        .assert_passes();
}

/// An embedded file renders through the app's `active_storage/blobs/
/// _blob` partial, as campfire's does: the node names its blob only by
/// sgid (`blob.attachable_sgid`), the partial is handed the Attachment
/// and reads the blob's filename and size through it, and the size is
/// Rails' `number_to_human_size`. `embeds.first.blob.variant(…)` with an
/// inline transformation is the blob under that Variation.
#[test]
fn an_embedded_blob_renders_through_the_apps_blob_partial() {
    overlay()
        .edit(
            "app/models/article.rb",
            "  def content_loaded?\n",
            "  def first_embed_variant\n    content.embeds.first.blob.variant(resize_to_limit: [ 10, 20 ])\n  end\n\n  def content_loaded?\n",
        )
        .write(
            "app/views/active_storage/blobs/_blob.html.erb",
            r##"<figure class="attachment attachment--file attachment--<%= blob.filename.extension %>">
  <figcaption class="attachment__caption">
    <% if caption = blob.try(:caption) %>
      <%= caption %>
    <% else %>
      <span class="attachment__name"><%= blob.filename %></span>
      <span class="attachment__size"><%= number_to_human_size blob.byte_size %></span>
    <% end %>
  </figcaption>
</figure>
"##,
        )
        .run_ruby(
            r##"
blob = ActiveStorage::Blob.create_and_upload!("x" * 1234, "notes.txt", "text/plain")
article = Article.new(title: "Embedded", body: "A body long enough to pass.")
article.content = %(<div>Here: <action-text-attachment sgid="#{blob.attachable_sgid}"></action-text-attachment></div>)
article.save!
html = Article.find(article.id).content.to_s
raise "no file name: #{html}" unless html.include?(%(<span class="attachment__name">notes.txt</span>))
raise "no human size: #{html}" unless html.include?(%(<span class="attachment__size">1.21 KB</span>))
raise "no extension class: #{html}" unless html.include?("attachment--txt")
variant = Article.find(article.id).first_embed_variant
raise "variant of #{variant.blob.id}" unless variant.blob.id == blob.id
raise "variation #{variant.variation.inspect}" unless variant.variation.encode.include?("10")
sizes = [ 1, 123, 1024, 12345, 1234567 ].map { |n| ActionView::ViewHelpers.number_to_human_size(n) }
raise "sizes #{sizes.inspect}" unless sizes == [ "1 Byte", "123 Bytes", "1 KB", "12.1 KB", "1.18 MB" ]
puts "blob partial ok"
"##,
        )
        .assert_passes();
}
