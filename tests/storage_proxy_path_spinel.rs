//! Active Storage's `rails_storage_proxy_path` / `_url` and
//! `rails_storage_redirect_path` / `_url`, the two `direct` routes the
//! engine draws beside `rails_blob`. Written through
//! `Rails.application.routes.url_helpers` they lower to
//! `RouteHelpers.<name>`, which answered only the `rails_blob` pair, so
//! the CRuby tree raised NoMethodError and Spinel refused the class
//! method. The proxy path is also SERVED: Rails' `Blobs::ProxyController`
//! answers it with the bytes rather than a redirect.
//!
//! The native run needs a Spinel compiler, so it is `#[ignore]`d like
//! its sibling suites; CI's `spinel-framework` job runs it with
//! `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/doc.rb", r#"class Doc < ApplicationRecord
  has_one_attached :file

  def proxy_path
    Rails.application.routes.url_helpers.rails_storage_proxy_path(file, only_path: true)
  end

  def redirect_path
    Rails.application.routes.url_helpers.rails_storage_redirect_path(file, only_path: true)
  end

  def blob_proxy_path
    Rails.application.routes.url_helpers.rails_storage_proxy_path(file.blob, only_path: true)
  end

  def blob_redirect_path
    Rails.application.routes.url_helpers.rails_storage_redirect_path(file.blob, only_path: true)
  end
end
"#)
        .write("app/controllers/docs_controller.rb", r#"class DocsController < ApplicationController
  def proxy
    render plain: rails_storage_proxy_url(Doc.find(params[:id]).file, only_path: true)
  end

  def redirect
    render plain: rails_storage_redirect_url(Doc.find(params[:id]).file, only_path: true)
  end

  def blob_proxy
    render plain: rails_storage_proxy_url(Doc.find(params[:id]).file.blob, only_path: true)
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/docs/:id/proxy\", to: \"docs#proxy\"\n  get \"/docs/:id/redirect\", to: \"docs#redirect\"\n  get \"/docs/:id/blob_proxy\", to: \"docs#blob_proxy\"\nend\n")
}

/// The paths are activesupport/activestorage 8.1.4's for the same blob
/// (`url_helpers.rails_storage_proxy_path(doc.file, only_path: true)`,
/// the signed id masked; the Blob itself gives the same string as its
/// attachment): `/rails/active_storage/blobs/proxy/` and
/// `/blobs/redirect/`, then the signed id and the URL-encoded filename.
/// The served answer is Rails' `Blobs::ProxyController#show` for a
/// request without a Range header: 200, the bytes, the blob's content
/// type, `inline`, and `http_cache_forever(public: true)`'s
/// Cache-Control; an id that does not verify is a 404.
const SCRIPT: &str = r#"
doc = Doc.create!(name: "n")
doc.file.attach("proxied-bytes", "a b.png", "image/png")
sid = doc.file.blob.signed_id
proxy = "/rails/active_storage/blobs/proxy/" + sid + "/a%20b.png"
redirect = "/rails/active_storage/blobs/redirect/" + sid + "/a%20b.png"
raise "proxy path: " + doc.proxy_path unless doc.proxy_path == proxy
raise "redirect path: " + doc.redirect_path unless doc.redirect_path == redirect
urls = DocsController.new
urls.params = {"id" => doc.id.to_s}
urls.process_action(:proxy)
raise "proxy url: " + urls.body unless urls.body == proxy
urls = DocsController.new
urls.params = {"id" => doc.id.to_s}
urls.process_action(:redirect)
raise "redirect url: " + urls.body unless urls.body == redirect
raise "blob proxy path: " + doc.blob_proxy_path unless doc.blob_proxy_path == proxy
raise "blob redirect path: " + doc.blob_redirect_path unless doc.blob_redirect_path == redirect
urls = DocsController.new
urls.params = {"id" => doc.id.to_s}
urls.process_action(:blob_proxy)
raise "blob proxy url: " + urls.body unless urls.body == proxy

controller = ActiveStorage::Blobs::ProxyController.new
controller.params = {"signed_id" => sid, "filename" => "a b.png"}
controller.process_action(:show)
raise "proxy status " + controller.status.to_s unless controller.status == 200
raise "proxy body " + controller.body unless controller.body == "proxied-bytes"
raise "proxy type " + controller.content_type.to_s unless controller.content_type == "image/png"
disposition = controller.headers["Content-Disposition"].to_s
raise "proxy disposition " + disposition unless disposition.start_with?("inline")
cache = controller.headers["Cache-Control"].to_s
raise "proxy cache " + cache unless cache == "max-age=3155695200, public, immutable"

missing = ActiveStorage::Blobs::ProxyController.new
missing.params = {"signed_id" => "bogus", "filename" => "a.png"}
missing.process_action(:show)
raise "unverified id " + missing.status.to_s unless missing.status == 404
puts "storage proxy passed"
"#;

#[test]
fn storage_proxy_and_redirect_paths_match_rails_on_cruby() {
    app()
        .run_ruby(&format!("require_relative \"app/models/doc\"\nrequire_relative \"app/controllers/docs_controller\"\n{SCRIPT}"))
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn storage_proxy_and_redirect_paths_match_rails_on_spinel() {
    app()
        .run_spinel_with_rbs(&format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{SCRIPT}"
        ))
        .assert_passes();
}

/// Rails 8.1.4 never serves an uploaded active type inline: the Blob's
/// `content_type_for_serving` turns `content_types_to_serve_as_binary`
/// (HTML, SVG, XML, ...) into `application/octet-stream`, and
/// `forced_disposition_for_serving` makes anything outside
/// `content_types_allowed_inline` an attachment, whatever the URL asks.
/// Both serving routes apply it: the proxy, and the redirect's disk
/// URL. The expectations are activestorage 8.1.4's answers for the same
/// four uploads on both routes (the disk route reached through the 302).
const SERVING_SCRIPT: &str = r#"
[
  ["page.html", "text/html", "application/octet-stream", "attachment"],
  ["pic.svg", "image/svg+xml", "application/octet-stream", "attachment"],
  ["pic.png", "image/png", "image/png", "inline"],
  ["notes.txt", "text/plain", "text/plain", "attachment"],
].each do |row|
  doc = Doc.create!(name: row[0])
  doc.file.attach("<b>bytes</b>", row[0], row[1])
  sid = doc.file.blob.signed_id

  proxy = ActiveStorage::Blobs::ProxyController.new
  proxy.params = {"signed_id" => sid, "filename" => row[0], "disposition" => "inline"}
  proxy.process_action(:show)
  raise row[0] + " proxy type " + proxy.content_type.to_s unless proxy.content_type == row[2]
  disposition = proxy.headers["Content-Disposition"].to_s
  raise row[0] + " proxy disposition " + disposition unless disposition.start_with?(row[3] + ";")

  redirect = ActiveStorage::Blobs::RedirectController.new
  redirect.params = {"signed_id" => sid, "filename" => row[0]}
  redirect.process_action(:show)
  parts = redirect.location.to_s.split("/")
  disk = ActiveStorage::DiskController.new
  disk.params = {"encoded_key" => parts[parts.length - 2], "filename" => parts[parts.length - 1]}
  disk.process_action(:show)
  raise row[0] + " disk status " + disk.status.to_s unless disk.status == 200
  raise row[0] + " disk type " + disk.content_type.to_s unless disk.content_type == row[2]
  disposition = disk.headers["Content-Disposition"].to_s
  raise row[0] + " disk disposition " + disposition unless disposition.start_with?(row[3] + ";")
end
puts "serving rules passed"
"#;

#[test]
fn active_content_types_are_served_as_rails_serves_them_on_cruby() {
    app()
        .run_ruby(&format!("require_relative \"app/models/doc\"\n{SERVING_SCRIPT}"))
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn active_content_types_are_served_as_rails_serves_them_on_spinel() {
    app()
        .run_spinel_with_rbs(&format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{SERVING_SCRIPT}"
        ))
        .assert_passes();
}
