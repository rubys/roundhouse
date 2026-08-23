//! `rescue_from` wraps the synthesized dispatcher
//! (`controller_to_library::collect_rescue_handlers`).
//!
//! Rails registers the handlers on `process_action`, so they cover the
//! FILTER CHAIN as well as the action — a `before_action` that raises is
//! caught exactly as an action that raises is. And it walks the registry
//! in REVERSE, so a later declaration wins; Ruby matches `rescue`
//! clauses in source order, which is why the clauses come out reversed.
//!
//! The declaration arrived as an `Unknown` class-body item, which is
//! where it had always been — dropped, silently, because nothing asked.
//! campfire's `Users::AvatarsController` is
//! `rescue_from(ActiveSupport::MessageVerifier::InvalidSignature) {
//! head :not_found }` over an avatar URL carrying a signed id, and with
//! the handler gone the signature error was a 500.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(controller: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :posts\nend\n".to_vec(),
        ),
        (PathBuf::from("app/controllers/posts_controller.rb"), controller.as_bytes().to_vec()),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("posts_controller.rb"))
        .map(|f| f.content)
        .expect("posts_controller emitted")
}

#[test]
fn a_block_handler_wraps_the_whole_dispatcher() {
    let src = emitted(
        r#"class PostsController < ApplicationController
  rescue_from(ActiveRecord::RecordNotFound) { head :not_found }

  def show
    @post = Post.find(params[:id])
  end
end
"#,
    );
    let dispatch = src.split("def process_action").nth(1).expect("dispatcher emitted");
    assert!(dispatch.contains("begin"), "the dispatcher body is wrapped:\n{src}");
    assert!(
        dispatch.contains("rescue ActiveRecord::RecordNotFound"),
        "…by the declared class:\n{src}"
    );
    assert!(dispatch.contains("head(:not_found)"), "…running the block:\n{src}");
}

/// `with: :handler` names a method instead of a block, and the
/// exception rides along only when that method takes one — Rails allows
/// both arities and the method is on this controller, so the arity is
/// known at lower time.
#[test]
fn a_with_handler_is_called_at_the_arity_it_declares() {
    let src = emitted(
        r#"class PostsController < ApplicationController
  rescue_from ActiveRecord::RecordNotFound, with: :not_found
  rescue_from ActiveRecord::RecordInvalid, with: :bad_request

  def show
    @post = Post.find(params[:id])
  end

  private
    def not_found(error)
      head :not_found
    end

    def bad_request
      head :bad_request
    end
end
"#,
    );
    let dispatch = src.split("def process_action").nth(1).expect("dispatcher emitted");
    assert!(
        dispatch.contains("not_found(__rescued)"),
        "a one-arg handler receives the exception:\n{src}"
    );
    assert!(
        dispatch.contains("self.bad_request()") || dispatch.contains("self.bad_request"),
        "a zero-arg handler is called bare:\n{src}"
    );
}

/// LAST DECLARED WINS, which is Rails' order — its handler registry is
/// walked in reverse. Ruby matches `rescue` clauses in source order, so
/// the later declaration must be the earlier clause.
#[test]
fn the_last_declaration_is_the_first_clause() {
    let src = emitted(
        r#"class PostsController < ApplicationController
  rescue_from(StandardError) { head :internal_server_error }
  rescue_from(ActiveRecord::RecordNotFound) { head :not_found }

  def show
    @post = Post.find(params[:id])
  end
end
"#,
    );
    let dispatch = src.split("def process_action").nth(1).expect("dispatcher emitted");
    let first = dispatch.find("rescue ActiveRecord::RecordNotFound").expect("both clauses");
    let second = dispatch.find("rescue StandardError").expect("both clauses");
    assert!(
        first < second,
        "the later declaration matches first:\n{src}"
    );
}

/// A controller that declares none emits the dispatcher it always did.
#[test]
fn no_declaration_means_no_wrapper() {
    let src = emitted(
        r#"class PostsController < ApplicationController
  def show
    @post = Post.find(params[:id])
  end
end
"#,
    );
    let dispatch = src.split("def process_action").nth(1).expect("dispatcher emitted");
    assert!(!dispatch.contains("begin"), "unchanged for a controller with none:\n{src}");
}
