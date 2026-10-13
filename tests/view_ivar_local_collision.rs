//! A view that reads `@class` and `@class_` (#753).
//!
//! An ivar becomes a view local, and a reserved word gets a trailing
//! `_` on the way (`@class` → `class_`), because `class` cannot be a
//! parameter name. `class_` is also what `@class_` becomes, so the two
//! ivars shared one local: the emitted method took `class_` twice, a
//! Ruby syntax error ("duplicated argument name"), and the body read
//! one value for both.
//!
//! The view's ivars are now renamed together: a reserved word still
//! gets its `_`, and one more `_` while that name is another ivar of
//! the same view. The parameter list, the body and the call sites all
//! read the same rename, so each ivar keeps its own value.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :posts, only: %i[show]\nend\n";

const CONTROLLER: &str = r#"class PostsController < ApplicationController
  def show
    @post = Post.find(params[:id])
    @class = "wide"
    @class_ = "tall"
  end
end
"#;

const SHOW: &str = "<p class=\"<%= @class %> <%= @class_ %>\"><%= @post.title %></p>\n";

fn app() -> roundhouse::App {
    let mut app = app_from(&[("app/views/posts/show.html.erb", SHOW)]);
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// The posts app with `views` as its templates, ingested.
fn app_from(views: &[(&str, &str)]) -> roundhouse::App {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        ),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/posts_controller.rb", CONTROLLER),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    for (path, source) in views {
        files.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    ingest_app_from_tree(files).expect("ingest")
}

fn emitted(files: Vec<roundhouse::emit::EmittedFile>) -> Vec<(String, String)> {
    files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect()
}

fn find<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ends with {suffix}; got {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            )
        })
}

/// The view takes one parameter per ivar, and the controller passes
/// the two ivars in the same order.
#[test]
fn each_ivar_gets_its_own_parameter() {
    let app = app();
    let views = emitted(ruby::emit_lowered_views(&app));
    let show = find(&views, "app/views/posts/show.rb");
    let parsed = ruby_prism::parse(show.as_bytes());
    let errors: Vec<String> = parsed.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "show.rb does not parse: {errors:?}\n{show}");
    assert!(show.contains("def self.show(class__, class_, post,"), "{show}");

    let controllers = emitted(ruby::emit_lowered_controllers(&app));
    let controller = find(&controllers, "posts_controller.rb");
    assert!(controller.contains("Views::Posts.show(@class, @class_, @post"), "{controller}");
}

/// Write `files` (emitted path → source) under a scratch app tree,
/// check each with `ruby -c`, load them in order and print `call`.
/// Returns the rendered HTML.
fn render(files: &[(&str, &str)], call: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-view-ivar-local-collision-{}-{}",
        std::process::id(),
        files.len()
    ));
    let mut requires = String::new();
    for (path, source) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    // Each view opens with `require_relative "../../views"`.
    std::fs::write(dir.join("app/views.rb"), "").unwrap();

    let checks: Vec<_> = files
        .iter()
        .map(|(path, _)| Command::new("ruby").arg("-c").arg(dir.join(path)).output().unwrap())
        .collect();
    let script = format!(
        r#"module ActionView
  module ViewHelpers
    def self.html_escape(s) = s.to_s
  end
end
module ViewBufferCap
  def self.alloc(_key) = String.new
  def self.store(_key, _size) = nil
end
{requires}Post = Struct.new(:title)
print {call}
"#
    );
    let run = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    for ((path, source), check) in files.iter().zip(checks) {
        assert!(
            check.status.success(),
            "ruby -c rejects {path}:\n{}\n{source}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// `ruby -c` accepts the view, and rendering it keeps the two values
/// apart.
#[test]
fn the_view_renders_both_values() {
    let app = app();
    let views = emitted(ruby::emit_lowered_views(&app));
    let show = find(&views, "app/views/posts/show.rb");
    let html = render(
        &[("app/views/posts/show.rb", show)],
        r#"Views::Posts.show("wide", "tall", Post.new("Hello"))"#,
    );
    assert!(html.contains("<p class=\"wide tall\">Hello</p>"), "{html}");
}

const SHOW_WITH_PARTIAL: &str =
    "<p class=\"<%= @class %> <%= @class_ %>\"><%= @post.title %></p>\n<%= render \"posts/badge\" %>\n";
const BADGE: &str = "<b class=\"<%= @class_ %> <%= @class %>\"></b>\n";

/// A partial that reads the pair takes them as two params too, and the
/// view that renders it passes each one its own value.
#[test]
fn a_rendered_partial_gets_both_values() {
    let mut app = app_from(&[
        ("app/views/posts/show.html.erb", SHOW_WITH_PARTIAL),
        ("app/views/posts/_badge.html.erb", BADGE),
    ]);
    roundhouse::session::analyze_and_lower(&mut app);
    let views = emitted(ruby::emit_lowered_views(&app));
    let badge = find(&views, "app/views/posts/_badge.rb");
    let show = find(&views, "app/views/posts/show.rb");
    let html = render(
        &[("app/views/posts/_badge.rb", badge), ("app/views/posts/show.rb", show)],
        r#"Views::Posts.show("wide", "tall", Post.new("Hello"))"#,
    );
    assert!(html.contains("<p class=\"wide tall\">Hello</p>"), "{html}");
    assert!(html.contains("<b class=\"tall wide\"></b>"), "{html}");
}
