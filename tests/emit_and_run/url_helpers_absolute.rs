//! `Rails.application.routes.url_helpers.x_url(…)` in a class with no
//! request (a model, a plain class, a mailer) is grounded as
//! `"<protocol>://<host><x_path(…)>"` by
//! `emit::ruby::library::rewrite_url_helpers_absolute`.
//!
//! Rails' `url_for` keeps every host option for itself: `port:` goes
//! into the authority after the host (and is left out when it is the
//! scheme's own), `only_path: true` returns the path alone, and none of
//! them reaches the path helper. The rewrite knew only `host:` and
//! `protocol:` and forwarded the rest to `RouteHelpers.x_path`, which
//! has no parameter for them: an `ArgumentError`, or with Active
//! Storage's `rails_blob_url`, whose helper does take `only_path:`, the
//! absolute URL with an empty host (`http:///rails/…`).
//!
//! Every expected value is Rails 8.1.4's for the same call.

use super::emit_and_run;

const MODEL_METHODS: &str = r##"
  def link_with_port
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", port: 8080)
  end

  def https_link_with_port
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", protocol: "https", port: 8443)
  end

  def link_with_standard_port
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", port: 80)
  end

  def https_link_with_standard_port
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", protocol: "https", port: 443)
  end

  def link_with_host
    Rails.application.routes.url_helpers.article_url(self, host: "example.com")
  end

  def link_on(port)
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", port: port)
  end

  def https_link_on(port)
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", protocol: "https", port: port)
  end

  def link_with(protocol, port)
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", protocol: protocol, port: port)
  end

  def link_on_env_port
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", port: ENV.fetch("ARTICLE_LINK_PORT", "8080"))
  end

  def path_despite_host
    Rails.application.routes.url_helpers.article_url(self, host: "example.com", port: 8080, only_path: true)
  end
"##;

const SERVICE: &str = r##"class ArticleLinks
  def initialize(article)
    @article = article
  end

  def url
    Rails.application.routes.url_helpers.article_url(@article, host: "example.com", port: 8080)
  end
end
"##;

const MAILER: &str = r##"class ArticleMailer < ApplicationMailer
  def link_for(article)
    Rails.application.routes.url_helpers.article_url(article, host: "example.com", port: 8080)
  end
end
"##;

const TEST: &str = r##"require "test_helper"

class ArticleTest < ActiveSupport::TestCase
  test "a model's port follows the host" do
    article = Article.first
    assert_equal "http://example.com:8080/articles/#{article.id}", article.link_with_port
    assert_equal "https://example.com:8443/articles/#{article.id}", article.https_link_with_port
  end

  test "the scheme's own port is left out" do
    article = Article.first
    assert_equal "http://example.com/articles/#{article.id}", article.link_with_standard_port
    assert_equal "https://example.com/articles/#{article.id}", article.https_link_with_standard_port
    assert_equal "http://example.com/articles/#{article.id}", article.link_with_host
  end

  test "a port known only at run time is decided at run time" do
    article = Article.first
    id = article.id
    assert_equal "http://example.com/articles/#{id}", article.link_on(nil)
    assert_equal "http://example.com/articles/#{id}", article.link_on(80)
    assert_equal "http://example.com:443/articles/#{id}", article.link_on(443)
    assert_equal "http://example.com:8080/articles/#{id}", article.link_on(8080)
    assert_equal "http://example.com:8080/articles/#{id}", article.link_on("8080")
    assert_equal "http://example.com/articles/#{id}", article.link_on("80")
    assert_equal "https://example.com/articles/#{id}", article.https_link_on(nil)
    assert_equal "https://example.com:80/articles/#{id}", article.https_link_on(80)
    assert_equal "https://example.com/articles/#{id}", article.https_link_on(443)
    assert_equal "https://example.com:8443/articles/#{id}", article.https_link_on(8443)
    assert_equal "https://example.com/articles/#{id}", article.link_with("https", 443)
    assert_equal "http://example.com:443/articles/#{id}", article.link_with("http", 443)
    assert_equal "http://example.com:8080/articles/#{id}", article.link_on_env_port
  end

  test "only_path wins over the host options" do
    article = Article.first
    assert_equal "/articles/#{article.id}", article.path_despite_host
  end

  test "a plain class and a mailer render the port too" do
    article = Article.first
    assert_equal "http://example.com:8080/articles/#{article.id}", ArticleLinks.new(article).url
    assert_equal "http://example.com:8080/articles/#{article.id}", ArticleMailer.new.link_for(article)
  end
end
"##;

#[test]
fn url_helpers_url_honours_port_and_passes_no_host_option_on() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            &format!("{MODEL_METHODS}\n  validates :title, presence: true\n"),
        )
        .write("app/services/article_links.rb", SERVICE)
        .write("app/mailers/article_mailer.rb", MAILER)
        .write("test/models/article_test.rb", TEST)
        .run_test("test/models/article_test.rb")
        .assert_passes();
}

const BLOB_SCHEMA: &str = r#"ActiveRecord::Schema.define do
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

/// Active Storage's `rails_blob_url` names no app route, so only the
/// library rewrite sees it. Rails answers
/// `/rails/active_storage/blobs/redirect/<signed id>/cover.png`.
#[test]
fn url_helpers_rails_blob_url_with_only_path_is_the_path() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", BLOB_SCHEMA)
        .write(
            "app/models/doc.rb",
            r#"class Doc < ApplicationRecord
  has_one_attached :file

  def self.attached
    doc = create!(name: "doc")
    doc.file.attach("hello-bytes", "cover.png", "image/png")
    doc
  end

  def blob_link
    Rails.application.routes.url_helpers.rails_blob_url(file, only_path: true)
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r##"
link = Doc.attached.blob_link
raise "not a path: #{link.inspect}" unless link.start_with?("/rails/active_storage/blobs/redirect/")
raise "filename: #{link.inspect}" unless link.end_with?("/cover.png")
puts "ok"
"##,
        )
        .assert_passes();
}
