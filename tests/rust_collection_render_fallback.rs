//! `<%= render(@posts) || "No posts" %>` must declare its rendered flag
//! mutable in the Rust target.
//!
//! The lowering binds `__rendered_N = false` before the collection loop
//! and sets it to `true` inside the per-item block, so the flag is
//! assigned twice. The `let mut` pre-walk did not look inside block
//! bodies, saw a single assignment and emitted `let __rendered_N`,
//! which the closure's write then failed with E0594.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_posts_views() -> String {
    let files: Vec<(&str, &str)> = vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table :posts do |t|
    t.string :title
  end
end
"#),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :posts, only: [:index]\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/posts_controller.rb", "class PostsController < ApplicationController\n  def index\n    @posts = Post.all\n  end\nend\n"),
        ("app/views/posts/index.html.erb", "<ul><%= render(@posts) || \"No posts\" %></ul>\n"),
        ("app/views/posts/_post.html.erb", "<li><%= post.title %></li>\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("views/posts.rs"))
        .expect("emitted posts views")
        .content
}

#[test]
fn rendered_flag_set_inside_the_collection_block_is_mutable() {
    let src = emitted_posts_views();
    assert!(src.contains("let mut __rendered_"), "the rendered flag must be declared `let mut`:\n{src}");
    assert!(!src.contains("let __rendered_"), "no rendered flag may be declared immutable:\n{src}");
}
