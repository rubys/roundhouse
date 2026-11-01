//! A view param read once by `<%= render @posts %>` must be declared
//! `mut` in the Rust target.
//!
//! The collection render lowers to `posts.each { |p| … }`, which the
//! Rust emit bridges to `posts.iter_mut().for_each(…)`. A view that
//! reads `@posts` twice gets a defensive `posts.clone()` to borrow
//! from, but a single read borrows the param itself, and an immutable
//! `posts: Vec<Post>` binding is E0596.

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
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :posts, only: [:index]\n  get \"/archive\", to: \"posts#archive\"\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/posts_controller.rb", "class PostsController < ApplicationController\n  def index\n    @posts = Post.all\n  end\n\n  def archive\n    @posts = Post.all\n  end\nend\n"),
        ("app/views/posts/index.html.erb", "<ul><%= render @posts %></ul>\n"),
        ("app/views/posts/archive.html.erb", "<p><%= @posts.size %></p>\n<ul><%= render @posts %></ul>\n"),
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

fn view_fn<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("pub fn {name}("))
        .unwrap_or_else(|| panic!("no `{name}` view fn:\n{src}"));
    let rest = &src[start..];
    let end = rest.find("\n    }\n").map(|i| i + 6).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn single_read_collection_param_is_declared_mut() {
    let src = emitted_posts_views();
    let index = view_fn(&src, "index");
    assert!(
        index.starts_with("pub fn index(mut posts: Vec<Post>"),
        "a param borrowed by `iter_mut()` must be `mut`:\n{index}"
    );
    assert!(
        index.contains("posts.iter_mut().for_each("),
        "the single read borrows the param itself:\n{index}"
    );
}

#[test]
fn cloned_collection_param_stays_immutable() {
    let src = emitted_posts_views();
    let archive = view_fn(&src, "archive");
    assert!(
        archive.starts_with("pub fn archive(posts: Vec<Post>"),
        "a param only borrowed through a clone needs no `mut`:\n{archive}"
    );
    assert!(
        archive.contains("posts.clone().iter_mut().for_each("),
        "the multi-read view keeps its clone:\n{archive}"
    );
}
