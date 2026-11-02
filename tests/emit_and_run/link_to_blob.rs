//! #1592: `link_to blob` (the `active_storage/blobs/_blob.html.erb`
//! override's shape). Rails resolves a Blob, an Attachment and an
//! Attached through `resolve("ActiveStorage::Blob")` to the redirect
//! route, as `rails_blob_path` does (rails 8.1.4, `oracle.out`):
//! `/rails/active_storage/blobs/redirect/<signed_id>/notes%20v1.txt`,
//! `?disposition=attachment` with one. The lowering read the local's NAME
//! and linked `RouteHelpers.blob_path(blob.id)`, which no app defines.

use super::emit_and_run;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
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
"#;

const SHOW: &str = concat!(
    "<p id=\"block\"><%= link_to @doc.file.blob do %>file<% end %></p>\n",
    "<p id=\"text\"><%= link_to \"file\", @doc.file.blob %></p>\n",
    "<p id=\"attached\"><%= link_to \"file\", @doc.file %></p>\n",
    "<p id=\"helper\"><%= link_to \"file\", rails_blob_path(@doc.file.blob, disposition: \"attachment\") %></p>\n",
    "<p id=\"override\"><%= render \"active_storage/blobs/blob\", blob: @doc.file.blob %></p>\n",
    "<p id=\"attachment\"><%= link_to \"file\", ActiveStorage::Attachment.find(1) %></p>\n",
);

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write(
            "app/models/doc.rb",
            "class Doc < ApplicationRecord\n  has_one_attached :file\n\n  def self.attached\n    doc = create!(name: \"doc\")\n    doc.file.attach(\"hello\", \"notes v1.txt\", \"text/plain\")\n    doc\n  end\nend\n",
        )
        .write("app/controllers/docs_controller.rb", "class DocsController < ApplicationController\n  def show\n    @doc = Doc.find(params[:id])\n  end\nend\n")
        .write("app/views/docs/show.html.erb", SHOW)
        .write("app/views/active_storage/blobs/_blob.html.erb", "<%= link_to blob do %>file<% end %>\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :docs, only: [:show]\nend\n")
}

const TEST: &str = r##"require "test_helper"

class DocsControllerTest < ActionDispatch::IntegrationTest
  test "a blob links to its redirect route" do
    doc = Doc.attached
    get doc_url(doc)
    assert_response :success
    href = %r{<a href="/rails/active_storage/blobs/redirect/[^/"]+/notes%20v1\.txt">file</a>}
    %w[block text attached override attachment].each do |id|
      # The override partial's own trailing newline stays in, as in Rails.
      assert_match(%r{<p id="#{id}">#{href.source}\n?</p>}, response.body)
    end
    assert_match(%r{<p id="helper"><a href="/rails/active_storage/blobs/redirect/[^/"]+/notes%20v1\.txt\?disposition=attachment">file</a></p>}, response.body)
  end

  test "a record with no file links nowhere instead of raising" do
    Doc.attached
    empty = Doc.create!(name: "empty")
    get doc_url(empty)
    assert_response :success
    assert_match(%r{<p id="text"><a href="">file</a></p>}, response.body)
  end
end
"##;

#[test]
fn a_blob_links_to_its_redirect_route() {
    app()
        .write("test/controllers/docs_controller_test.rb", TEST)
        .run_test("test/controllers/docs_controller_test.rb")
        .assert_passes();
}

/// The show page in the spinel binary, where #1592 was found.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_blob_links_to_its_redirect_route_on_spinel() {
    app()
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "doc = Doc.attached\n",
            "request = ActionDispatch::Request.new\n",
            "request.request_method = \"GET\"\n",
            "request.path = \"/docs/\" + doc.id.to_s\n",
            "ActionController::Current.request = request\n",
            "controller = DocsController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.params = {\"id\" => doc.id.to_s}\n",
            "controller.process_action(:show)\n",
            "body = controller.body\n",
            "href = '<a href=\"/rails/active_storage/blobs/redirect/'\n",
            "[\"block\", \"text\", \"attached\", \"override\"].each do |id|\n",
            "  at = body.index('<p id=\"' + id + '\">' + href)\n",
            "  raise id + \": \" + body if at.nil?\n",
            "end\n",
            "raise body unless body.include?('/notes%20v1.txt\">file</a>')\n",
            "raise body unless body.include?('/notes%20v1.txt?disposition=attachment\">file</a>')\n",
            "puts \"link_to blob passed\"\n",
        ))
        .assert_passes();
}


/// Action Text renders an app's `_blob` partial with the attachment
/// (`render_attachment`'s `ActiveStorage::Blob` arm passes it as `blob`),
/// so the partial's record isn't typed `ActiveStorage::Blob`: campfire's
/// Spinel build refused `runtime/action_text.rb` handing it an
/// `ActionText::Attachment` there.
#[test]
fn the_blob_partial_takes_action_texts_attachment() {
    let (out, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let runtime = std::fs::read_to_string(out.join("runtime/action_text.rb")).unwrap();
    assert!(runtime.contains("Views::ActiveStorage::Blobs.blob(attachment)"), "{runtime}");
    let sig = std::fs::read_to_string(out.join("app/views/active_storage/blobs/_blob.rbs")).unwrap();
    assert!(!sig.contains("ActiveStorage::Blob blob"), "{sig}");
}
