//! campfire's SQLite-observer caches on the CRuby overlay; the shared
//! contract and its native twin are described in
//! `tests/support/campfire_caches.rs`.

use super::campfire_caches_contract::{self as contract, Contract};
use super::emit_and_run;

fn assert_runs(contract: &Contract) {
    let run = contract.overlay().run_ruby(contract.script);
    run.assert_passes();
    assert_eq!(run.stdout, contract.expected, "stderr:\n{}", run.stderr);
}

#[test]
fn nested_multi_write_destructures_each_group() {
    assert_runs(&contract::NESTED_MULTI_WRITE);
}

#[test]
fn data_define_block_methods_belong_to_the_data_class() {
    let run = contract::DATA_BLOCK_METHODS
        .overlay()
        .write(
            "app/controllers/data_key_support_controller.rb",
            r#"class DataKeySupportController < ApplicationController
  def self.encode(value)
    "encoded-" + value
  end
end
"#,
        )
        .run_ruby(contract::DATA_BLOCK_METHODS.script);
    run.assert_passes();
    assert_eq!(run.stdout, contract::DATA_BLOCK_METHODS.expected, "stderr:\n{}", run.stderr);
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/data_block_probe.rb")).unwrap();
    assert!(emitted.contains("def cache_key"), "{emitted}");
    assert!(emitted.contains("def encoded_digest"), "{emitted}");
    assert!(emitted.contains("EmptyKey = Data.define(:value) do"), "{emitted}");
    assert!(emitted.contains("require_relative \"../controllers/data_key_support_controller\""), "{emitted}");
}

#[test]
fn response_helpers_run_as_rails_runs_them() {
    assert_runs(&contract::RESPONSE_HELPERS);
}

/// The same helpers where campfire calls them — a controller concern,
/// the strictly typed context the errors were reported in: negotiate
/// the encoding off the request header, gzip the body with a weak ETag,
/// snapshot the session, and key the page by its request facts.
#[test]
fn response_helpers_run_in_a_controller_concern() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/cached\", to: \"cached_pages#show\"\n",
        )
        .write(
            "app/controllers/concerns/page_reuse.rb",
            r#"require "zlib"

module PageReuse
  extend ActiveSupport::Concern

  private
    def negotiated_encoding
      Rack::Utils.select_best_encoding(%w[ gzip identity ], Rack::Utils.q_values(request.headers["Accept-Encoding"]))
    end

    def page_key(encoding)
      ActiveSupport::JSON.encode([ controller_path, request.fullpath, encoding, session.to_hash.except("_csrf_token") ])
    end
end
"#,
        )
        .write(
            "app/controllers/cached_pages_controller.rb",
            r##"class CachedPagesController < ApplicationController
  include PageReuse

  def show
    session[:visits] = "1"
    snapshot = session.to_hash.deep_dup
    encoding = negotiated_encoding
    html = "<p>cached page</p>"
    body = encoding == "gzip" ? Zlib.gzip(html) : html
    response.headers["ETag"] = %(W/"#{Digest::SHA256.hexdigest(body).byteslice(0, 32)}")
    response.headers["X-Page-Key"] = page_key(encoding)
    response.headers["X-Snapshot-Same"] = (snapshot == session.to_hash).to_s
    response.headers["X-No-Nonce"] = (!Rails.application.config.content_security_policy_nonce_generator).to_s
    response.headers["Content-Encoding"] = "gzip" if encoding == "gzip"
    render plain: body
  end
end
"##,
        )
        .write(
            "test/controllers/cached_pages_controller_test.rb",
            r#"require "test_helper"

class CachedPagesControllerTest < ActionDispatch::IntegrationTest
  test "gzip is negotiated and keyed" do
    get "/cached", headers: { "Accept-Encoding" => "br, gzip;q=0.5" }
    assert_response :success
    assert_equal "gzip", response.headers["Content-Encoding"]
    assert_equal "<p>cached page</p>", Zlib.gunzip(response.body)
    assert_match(/\AW\/"[0-9a-f]{32}"\z/, response.headers["ETag"])
    assert_equal "true", response.headers["X-Snapshot-Same"]
    assert_equal "true", response.headers["X-No-Nonce"]
    key = ActiveSupport::JSON.decode(response.headers["X-Page-Key"])
    assert_equal [ "cached_pages", "/cached", "gzip" ], key.first(3)
    assert_equal "1", key.last["visits"]
  end

  test "a refused identity and no acceptable encoding answers nil" do
    get "/cached", headers: { "Accept-Encoding" => "gzip;q=0, identity;q=0" }
    assert_response :success
    assert_nil response.headers["Content-Encoding"]
    assert_equal "<p>cached page</p>", response.body
    assert_nil ActiveSupport::JSON.decode(response.headers["X-Page-Key"])[2]
  end
end
"#,
        )
        .run_test("test/controllers/cached_pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn sqlite_observer_and_checkpointer_surface_runs() {
    assert_runs(&contract::SQLITE_OBSERVER);
}

#[test]
fn record_snapshots_and_the_bounded_store_run() {
    assert_runs(&contract::RECORD_SNAPSHOT);
}

#[test]
fn bounded_store_rejects_identity_hashes_without_retaining_aliases() {
    let run = emit_and_run::real_blog().run_ruby(
        r#"store = ActiveSupport::Cache::MemoryStore.new
identity = {}.compare_by_identity
identity["same".dup] = 1
identity["same".dup] = 2
begin
  store.write("identity", identity)
  puts "accepted"
rescue ArgumentError
  puts "rejected"
end
"#,
    );
    run.assert_passes();
    assert_eq!(run.stdout, "rejected\n", "stderr:\n{}", run.stderr);
}

// Spinel currently refuses this nested Hash-value mutation shape during
// alias analysis, before the cache behavior can run; keep the semantic
// regression on CRuby instead of masking that compiler limitation.
#[test]
fn bounded_store_copies_mutable_nested_hash_values() {
    let run = emit_and_run::real_blog().run_ruby(
        r#"store = ActiveSupport::Cache::MemoryStore.new
original = { "items" => [ "cached" ] }
store.write("nested", original)
original["items"] << "caller mutation"
read = store.read("nested")
read["items"] << "read mutation"
copied = store.read("nested")

defaulted = Hash.new([ "fallback" ])
store.write("default", defaulted)
size = store.inspect
defaulted.default << "caller mutation"
default_read = store.read("default")
default_read.default << "read mutation"

rejected_proc = begin
  store.write("proc", Hash.new { |hash, key| hash[key] = key })
  false
rescue ArgumentError
  true
end
cycle = []
cycle << cycle
rejected_cycle = begin
  store.write("cycle", cycle)
  false
rescue ArgumentError
  true
end
puts [copied, store.inspect == size, store.read("default")["missing"], rejected_proc, rejected_cycle].inspect
"#,
    );
    run.assert_passes();
    assert_eq!(
        run.stdout,
        "[{\"items\" => [\"cached\"]}, true, [\"fallback\"], true, true]\n",
        "stderr:\n{}",
        run.stderr
    );
}

/// Rails' fragment caching through the controller, as campfire's
/// `CachedResponses` overrides it: a view's `<% cache %>` is served only
/// while `perform_caching`, under `combined_fragment_cache_key`, from
/// `cache_store` — and each override's `super` reaches Rails' own. A
/// commit that does not touch `updated_at` is served stale until the
/// app's epoch moves the key, as it is in Rails; the test environment
/// caches nothing until a test turns it on.
#[test]
fn view_fragments_go_through_the_controllers_caching() {
    contract::fragments_overlay()
        .write(
            "test/controllers/fragments_controller_test.rb",
            r#"require "test_helper"

class FragmentsControllerTest < ActionDispatch::IntegrationTest
  class NilParentController < ActionController::Base
  end

  class NilChildController < NilParentController
  end

  setup do
    @article = Article.create!(title: "Original", body: "A sufficiently long article body.")
  end

  teardown do
    ActionController::Base.perform_caching = false
  end

  def foreign_title(title)
    ActiveRecord::Base.connection.execute("UPDATE articles SET title = '#{title}' WHERE id = #{@article.id}")
  end

  test "the test environment caches nothing" do
    get "/fragments/#{@article.id}"
    foreign_title("Changed")
    get "/fragments/#{@article.id}"
    assert_includes response.body, "Changed"
  end

  test "explicit nil cache settings override inherited values" do
    NilParentController.perform_caching = nil
    assert NilParentController.perform_caching.nil?
    assert NilChildController.perform_caching.nil?
    NilParentController.cache_store = nil
    assert NilParentController.cache_store.nil?
    assert NilChildController.cache_store.nil?
  end

  test "detached fragments do not use the shared cache" do
    previous_store = ActionController::Base.cache_store
    store = ActiveSupport::Cache::MemoryStore.new
    ActionController::Base.cache_store = store
    ActionController::Current.controller = nil
    controller = ActionView::ViewHelpers.fragment_controller
    name = ActiveSupport::Cache.expanded_key(controller.combined_fragment_cache_key("detached"))
    assert ActionView::ViewHelpers.fragment_read("views/detached").nil?
    assert_equal "body", ActionView::ViewHelpers.fragment_write("views/detached", "body", 0)
    assert store.read(name).nil?
  ensure
    ActionController::Current.controller = nil
    ActionController::Base.cache_store = previous_store
  end

  test "a cached fragment is served until the key moves" do
    ActionController::Base.perform_caching = true
    get "/fragments/#{@article.id}"
    assert_includes response.body, "Original"
    foreign_title("Changed")
    get "/fragments/#{@article.id}"
    assert_includes response.body, "Original"
    get "/fragments/#{@article.id}?nocache=1"
    assert_includes response.body, "Changed"
    post "/fragments/bump"
    get "/fragments/#{@article.id}"
    assert_includes response.body, "Changed"
    assert_includes FragmentsController::STORE.inspect, "entries=2"
  end
end
"#,
        )
        .run_test("test/controllers/fragments_controller_test.rb")
        .assert_passes();
}

#[test]
fn qr_code_capacity_error_is_rescued_by_name() {
    contract::qr_code_overlay()
        .write(
            "test/controllers/qr_codes_controller_test.rb",
            r#"require "test_helper"

class QrCodesControllerTest < ActionDispatch::IntegrationTest
  test "a short url renders and an oversized one answers 400" do
    get "/qr/10"
    assert_response :success
    assert_includes response.body, "<svg"
    get "/qr/8000"
    assert_response :bad_request
  end
end
"#,
        )
        .run_test("test/controllers/qr_codes_controller_test.rb")
        .assert_passes();
}

#[test]
fn a_cache_through_method_answers_its_blocks_value_on_hit_and_miss() {
    let run = contract::CACHE_THROUGH.overlay().run_ruby(contract::CACHE_THROUGH.script);
    run.assert_passes();
    assert_eq!(run.stdout, contract::CACHE_THROUGH.expected, "stderr:\n{}", run.stderr);
    let rbs = std::fs::read_to_string(run.emitted.join("sig/app/models/record_cache.rbs")).unwrap();
    contract::assert_cache_through_signature(&rbs);
}
