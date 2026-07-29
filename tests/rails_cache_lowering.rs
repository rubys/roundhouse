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
