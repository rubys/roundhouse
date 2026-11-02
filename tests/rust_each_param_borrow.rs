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
use std::sync::Arc;

use roundhouse::effect::EffectSet;
use roundhouse::emit::rust;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::{Param, ParamKind, Ty};

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

/// No app-level source types a model method's param as an Array yet,
/// so the signatures are set on the ingested IR before analysis.
fn emitted_post_model() -> String {
    let files: Vec<(&str, &str)> = vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table :posts do |t|
    t.string :title
  end
end
"#),
        ("app/models/post.rb", r#"class Post < ApplicationRecord
  def stamp(items)
    items.each { |item| item.title }
    title
  end

  def stamp_and_count(items)
    items.each { |item| item.title }
    items.size
  end
end
"#),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let post = app
        .models
        .iter_mut()
        .find(|m| m.name.0.as_str() == "Post")
        .expect("Post model");
    let items = Ty::Array {
        elem: Arc::new(Ty::Class { id: ClassId(Symbol::from("Post")), args: vec![].into() }),
    };
    for m in post.methods_mut() {
        let ret = match m.name.as_str() {
            "stamp" => Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() },
            _ => Ty::Int,
        };
        m.signature = Some(Ty::Fn {
            params: vec![Param { name: "items".into(), ty: items.clone().into(), kind: ParamKind::Required }].into(),
            block: None,
            ret: Arc::new(ret),
            effects: EffectSet::pure(),
        });
    }
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/post.rs"))
        .expect("emitted Post model")
        .content
}

fn method_fn<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("pub fn {name}("))
        .unwrap_or_else(|| panic!("no `{name}` method:\n{src}"));
    let rest = &src[start..];
    let end = rest.find("\n    }\n").map(|i| i + 6).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn instance_method_single_read_collection_param_is_declared_mut() {
    let src = emitted_post_model();
    let stamp = method_fn(&src, "stamp");
    assert!(
        stamp.starts_with("pub fn stamp(&self, mut items: Vec<Post>)"),
        "a param borrowed by `iter_mut()` must be `mut`:\n{stamp}"
    );
    assert!(
        stamp.contains("items.iter_mut().for_each("),
        "the single read borrows the param itself:\n{stamp}"
    );
}

#[test]
fn instance_method_cloned_collection_param_stays_immutable() {
    let src = emitted_post_model();
    let stamp_and_count = method_fn(&src, "stamp_and_count");
    assert!(
        stamp_and_count.starts_with("pub fn stamp_and_count(items: Vec<Post>)"),
        "a param only borrowed through a clone needs no `mut`:\n{stamp_and_count}"
    );
    assert!(
        stamp_and_count.contains("items.clone().iter_mut().for_each("),
        "the multi-read method keeps its clone:\n{stamp_and_count}"
    );
}
