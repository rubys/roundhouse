//! `action_name` on a controller. The synthesized `process_action` calls
//! `assign_action_name` only in a controller that reads `action_name`,
//! so the other controllers emit the same code as before. The
//! lowered call and the reader must also type, because the lowered-typing
//! check requires every expression to have a type.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :posts do |t|\n    t.string :title\n  end\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :posts, only: [ :index ]\n  \
    resources :notes, only: [ :index ]\nend\n";

const POSTS: &str = "class PostsController < ApplicationController\n  \
    before_action :track\n\n  \
    def index\n    @posts = Post.all\n  end\n\n  \
    private\n\n  \
    def track\n    @tracked = action_name\n  end\nend\n";

const NOTES: &str = "class NotesController < ApplicationController\n  \
    def index\n    @notes = Note.all\n  end\nend\n";

fn app() -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (p, c) in [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/controllers/posts_controller.rb", POSTS),
        ("app/controllers/notes_controller.rb", NOTES),
        ("app/views/posts/index.html.erb", "<p><%= @tracked %></p>\n"),
        ("app/views/notes/index.html.erb", "<p>notes</p>\n"),
    ] {
        tree.insert(PathBuf::from(p), c.as_bytes().to_vec());
    }
    ingest_app_from_tree(tree).expect("ingest")
}

fn emitted_controller(name: &str) -> String {
    let mut app = app();
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers_with_layout(&app)
        .into_iter()
        .find(|f| f.path.ends_with(format!("{name}.rb")))
        .unwrap_or_else(|| panic!("{name}.rb emitted"))
        .content
}

#[test]
fn only_a_controller_that_reads_action_name_assigns_it() {
    let posts = emitted_controller("posts_controller");
    assert!(posts.contains("assign_action_name(routed_action)"), "{posts}");
    let notes = emitted_controller("notes_controller");
    assert!(!notes.contains("assign_action_name"), "{notes}");
}

/// The emitted Rust HTTP handler for `index` in `name`'s controller.
fn rust_index_handler(name: &str) -> String {
    let mut app = app();
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let file = roundhouse::emit::rust::emit(&app)
        .into_iter()
        .find(|f| f.path.ends_with(format!("{name}.rs")))
        .unwrap_or_else(|| panic!("{name}.rs emitted"))
        .content;
    let start = file.find("pub async fn _axum_index").expect("index handler emitted");
    let end = file[start..].find("\n}\n").map_or(file.len(), |i| start + i);
    file[start..end].to_string()
}

/// The Rust HTTP handler calls the action without `process_action`, so
/// it sets the action name itself.
#[test]
fn the_rust_http_handler_sets_the_action_name() {
    let posts = rust_index_handler("posts_controller");
    let assign = posts.find("c.assign_action_name(\"index\");");
    let call = posts.find("c.index();");
    assert!(
        matches!((assign, call), (Some(a), Some(c)) if a < c),
        "the handler sets the name before the action:\n{posts}"
    );
    let notes = rust_index_handler("notes_controller");
    assert!(!notes.contains("assign_action_name"), "{notes}");
}

fn send_types<'a>(e: &'a Expr, method: &str, out: &mut Vec<Option<&'a Ty>>) {
    if let ExprNode::Send { method: m, .. } = &*e.node {
        if m.as_str() == method {
            out.push(e.ty.as_ref());
        }
    }
    e.node.for_each_child(&mut |c| send_types(c, method, out));
}

#[test]
fn the_action_name_calls_type_as_strings() {
    let app = app();
    let lcs = roundhouse::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        Vec::new(),
        Some(&app.schema),
        &app.views,
    );
    let posts = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "PostsController")
        .expect("PostsController lowered");
    for method in ["assign_action_name", "action_name"] {
        let mut types = Vec::new();
        for m in &posts.methods {
            send_types(&m.body, method, &mut types);
        }
        assert!(!types.is_empty(), "no `{method}` call in the lowered controller");
        assert!(
            types.iter().all(|t| *t == Some(&Ty::Str)),
            "`{method}` types as {types:?}"
        );
    }
}
