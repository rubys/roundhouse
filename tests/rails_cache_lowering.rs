//! `Rails.cache.fetch(k, expires_in: t) { <String> }` → `fetch_str(k, t)`
//! (`lower::apply_rails_cache_lowering`).
//!
//! The shared runtime's `Rails.cache` keeps a recompute-every-call
//! `fetch` for blocks whose value could be anything, plus a typed
//! String-keyed/String-valued `fetch_str`. This pass decides which sites
//! reach the store: the ones whose block provably yields a String. In
//! lobsters that is the `render_to_string` page-fragment shape — `/u`
//! caches a ~292KB invite tree for 24 hours and recomputed it on every
//! one of its 15 visits in the benchmark sequence before this landed.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_rails_cache_lowering;

fn lowered(action_body: &str) -> String {
    lowered_src(&format!(
        "class ThingsController < ApplicationController\n  def index\n{action_body}\n  end\nend\n"
    ))
}

fn lowered_src(src: &str) -> String {
    let src = src.to_string();
    let tree = vec![(
        std::path::PathBuf::from("app/controllers/things_controller.rb"),
        src.into_bytes(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    apply_rails_cache_lowering(&mut app);
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("controller ingested");
    format!("{:?}", c.body)
}

#[test]
fn render_to_string_block_reaches_the_store() {
    let body = lowered(
        r#"    content = Rails.cache.fetch("users_tree_1", :expires_in => (60 * 60 * 24)) {
      @users = User.all
      render_to_string :action => "tree", :layout => nil
    }"#,
    );
    assert!(body.contains("fetch_str"), "expected fetch_str:\n{body}");
    // The TTL rides through `.to_i`, which reads seconds off an Integer
    // and off an ActiveSupport::Duration alike.
    assert!(body.contains("to_i"), "expected a to_i'd ttl:\n{body}");
}

#[test]
fn a_block_that_is_not_a_string_keeps_recomputing() {
    // lobsters' unread-replies counter: the block yields an Integer, and
    // the String store cannot hold it.
    let body = lowered(
        r#"    n = Rails.cache.fetch("user:1:unread_replies", :expires_in => 120) { Comment.count }"#,
    );
    assert!(!body.contains("fetch_str"), "Integer block should not reach the store:\n{body}");
}

#[test]
fn a_block_pass_site_is_left_alone() {
    // `&block` belongs to the caller, so this pass cannot see what it
    // yields — lobsters' front-page and story caches are this shape.
    let body = lowered_src(
        r#"class ThingsController < ApplicationController
  def get_stories_from_cache(key, &block)
    Rails.cache.fetch("stories #{key}", :expires_in => 45, &block)
  end
end
"#,
    );
    assert!(!body.contains("fetch_str"), "block-pass site should be left alone:\n{body}");
}

#[test]
fn a_fetch_on_something_else_is_left_alone() {
    // An app object that happens to answer `fetch` is not the store.
    let body = lowered(
        r#"    content = Sponge.new.fetch("x", :expires_in => 60) { render_to_string :action => "tree" }"#,
    );
    assert!(!body.contains("fetch_str"), "non-Rails.cache receiver:\n{body}");
}

#[test]
fn no_expires_in_gets_a_never_expiring_ttl() {
    let body = lowered(r#"    content = Rails.cache.fetch("k") { render_to_string :action => "tree" }"#);
    assert!(body.contains("fetch_str"), "expected fetch_str:\n{body}");
}

fn with_store(extra: Option<(&str, &str)>) -> String {
    let mut tree: Vec<(std::path::PathBuf, Vec<u8>)> = vec![
        (
            std::path::PathBuf::from("app/controllers/things_controller.rb"),
            b"class ThingsController < ApplicationController\n  def index\n    v = Rails.cache.fetch(\"k\", expires_in: 60) { render_to_string :action => \"x\" }\n  end\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/active_support/cache/custom_store.rb"),
            b"module ActiveSupport\n  module Cache\n    class CustomStore\n      def fetch(name, options = nil, &block)\n        yield\n      end\n    end\n  end\nend\n".to_vec(),
        ),
    ];
    if let Some((path, text)) = extra {
        tree.push((std::path::PathBuf::from(path), text.as_bytes().to_vec()));
    }
    let mut app = ingest_app_from_tree(tree.into_iter().collect()).expect("ingest");
    apply_rails_cache_lowering(&mut app);
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("controller ingested");
    format!("{:?}", c.body)
}

#[test]
fn an_app_cache_store_rails_cache_is_bound_to_keeps_fetch() {
    // `Rails.cache` is the app's store, which has no `fetch_str`, so the
    // String-block site must stay a plain `fetch`.
    let helper = "Rails.cache = ActiveSupport::Cache::CustomStore.new\n";
    let body = with_store(Some(("test/test_helper.rb", helper)));
    assert!(!body.contains("fetch_str"), "a bound app store must keep fetch:\n{body}");
}

#[test]
fn an_app_cache_store_nothing_binds_keeps_the_lowering() {
    // Defined but unused: `Rails.cache` is still the runtime's store, whose
    // plain `fetch` recomputes every call.
    let body = with_store(None);
    assert!(body.contains("fetch_str"), "an unbound app store must not disable fetch_str:\n{body}");
}

#[test]
fn a_configured_cache_store_keeps_the_lowering() {
    // No target installs `config.cache_store`, in `config/application.rb`
    // or an environment file: `Rails.cache` stays the runtime's store,
    // which has `fetch_str`, and standing down would only lose caching.
    let application = "require \"rails\"\n\nmodule Probe\n  class Application < Rails::Application\n    config.cache_store = :custom_store\n  end\nend\n";
    let body = with_store(Some(("config/application.rb", application)));
    assert!(body.contains("fetch_str"), "config.cache_store is not installed:\n{body}");
    let production = "Rails.application.configure do\n  config.cache_store = [:memory_store, { size: 64.megabytes }]\nend\n";
    let body = with_store(Some(("config/environments/production.rb", production)));
    assert!(body.contains("fetch_str"), "config.cache_store is not installed:\n{body}");
}

#[test]
fn a_rails_cache_assignment_whose_value_is_on_the_next_line_keeps_fetch() {
    let helper = "Rails.cache =\n  ActiveSupport::Cache::CustomStore.new\n";
    let body = with_store(Some(("test/test_helper.rb", helper)));
    assert!(!body.contains("fetch_str"), "a multiline binding must keep fetch:\n{body}");
}

#[test]
fn rails_cache_bound_to_an_app_object_keeps_fetch() {
    // Not an `ActiveSupport::Cache::*` class: any store object of the
    // app's need not have `fetch_str`.
    let helper = "Rails.cache = MyStore.new\n";
    let body = with_store(Some(("test/test_helper.rb", helper)));
    assert!(!body.contains("fetch_str"), "an app store object must keep fetch:\n{body}");
}

#[test]
fn rails_cache_bound_to_a_rails_store_keeps_the_lowering() {
    let helper = "Rails.cache = ActiveSupport::Cache::MemoryStore.new\n";
    let body = with_store(Some(("test/test_helper.rb", helper)));
    assert!(body.contains("fetch_str"), "a Rails store is the runtime's:\n{body}");
}
