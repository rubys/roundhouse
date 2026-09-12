//! Two test-body rewrites that meet the harness where Rails is looser
//! than a typed helper:
//!
//! * A route helper's query option is typed `String?` (the name-based
//!   rule in `routes_to_library::param_ty`: only `id`/`*_id` keys are
//!   Integer), and the helper calls `to_s` on it. A test writing
//!   `after: messages(:tenth).id` — or a record, which Rails puts
//!   through `to_param` — handed spinel an Integer in a String slot.
//!   `project_route_helper_ids` now makes that `to_s` call one hop
//!   earlier: `.id.to_s` for a record, `.to_s` for an Integer under a
//!   non-id key. An `*_id` key keeps its Integer (it IS the segment
//!   type), and so does `format:`.
//! * `response.headers["X-Total-Count"]` — Rails' headers are
//!   case-insensitive, the harness's Hash is lowercase-keyed the way
//!   Rack 3 normalizes, so a literal key at a `.headers[…]` read is
//!   lowercased. Reads only; a controller's `headers["X"] = …` is
//!   the app's.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(body: &str) -> String {
    let src = format!(
        "require \"test_helper\"\n\nclass PostsControllerTest < ActionDispatch::IntegrationTest\n  test \"one\" do\n    {body}\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :posts, only: %i[ index ]\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/posts_controller.rb"),
            b"class PostsController < ApplicationController\n  def index\n    @posts = Post.where(\"id > ?\", params[:after])\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("test/fixtures/posts.yml"),
            b"one:\n  body: hi\n".to_vec(),
        ),
        (PathBuf::from("test/controllers/posts_controller_test.rb"), src.into_bytes()),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("posts_controller_test.rb"))
        .expect("no posts_controller_test.rb emitted")
        .content
        .clone()
}

#[test]
fn an_integer_under_a_string_query_key_is_stringified() {
    let src = emitted("get posts_url(after: posts(:one).id)");
    assert!(
        src.contains("after: PostsFixtures.one.id.to_s"),
        "the Integer under `after:` should be stringified:\n{src}"
    );
}

#[test]
fn a_record_under_a_query_key_projects_to_its_param() {
    let src = emitted("get posts_url(after: posts(:one))");
    assert!(
        src.contains("after: PostsFixtures.one.id.to_s"),
        "a record under `after:` should become its to_param:\n{src}"
    );
}

#[test]
fn an_id_shaped_query_key_keeps_its_integer() {
    let src = emitted("get posts_url(author_id: 3, format: :json)");
    assert!(src.contains("author_id: 3"), "`*_id` keys are Integer segments:\n{src}");
    assert!(!src.contains("3.to_s"), "no stringification under an id key:\n{src}");
}

#[test]
fn a_literal_header_key_is_read_lowercase() {
    let src = emitted("get posts_url\n    assert_equal \"41\", response.headers[\"X-Total-Count\"]");
    assert!(
        src.contains("headers[\"x-total-count\"]"),
        "the header read should be lowercase-keyed:\n{src}"
    );
    assert!(!src.contains("X-Total-Count"), "{src}");
}
