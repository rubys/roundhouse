//! A partial that reads both `local_assigns[:x]` and `@x` takes one `x`.
//! After the ivar→local rewrite both reads are the local `x`, and the
//! threaded closure ivar already carries it (#389). The call sites drop
//! the same name, or they pass one argument too many.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files};

const APP: &[(&str, &str)] = &[
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    (
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::Base\nend\n",
    ),
    (
        "db/schema.rb",
        "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n",
    ),
    (
        "app/models/post.rb",
        "class Post < ApplicationRecord\nend\n",
    ),
    (
        "app/views/layouts/application.html.erb",
        "<html><body><%= yield %></body></html>\n",
    ),
    (
        "app/views/posts/_note.html.erb",
        "<p><%= local_assigns[:error] %><%= @error %><%= local_assigns[:alert] %></p>\n",
    ),
];

const INDEX_ONLY: &str = "class PostsController < ApplicationController\n  def index\n    @error = \"bad\"\n  end\nend\n";
const ROUTES_INDEX: &str =
    "Rails.application.routes.draw do\n  resources :posts, only: %i[index]\nend\n";

/// The spinel tree for `APP` plus `files`.
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    emit(BuildTarget::Spinel, files)
}

/// The `target` tree for `APP` plus `files`.
fn emit(target: BuildTarget, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = APP
        .iter()
        .chain(files)
        .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), target).expect("target files")
}

/// The emitted source at `path`.
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

/// `source` parses as Ruby; the message names `path`.
fn assert_parses(source: &str, path: &str) {
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(
        errors.is_empty(),
        "{path} does not parse: {errors:?}\n{source}"
    );
}

/// The comma-separated list inside the first `open(`…`)` on a line.
fn arg_list(line: &str, open: &str) -> Option<Vec<String>> {
    let rest = line.split_once(open)?.1;
    let inner = &rest[..rest.rfind(')')?];
    Some(inner.split(", ").map(|p| p.trim().to_string()).collect())
}

/// The partial `posts/_<name>` takes one `param`, and every call in
/// `caller` fits its signature: `<name>_into(io, …)` has no defaults, so a
/// call passes exactly as many arguments; `<name>(…)` defaults its extras,
/// so a call passes at most as many.
fn assert_one_param_and_matching_calls(
    files: &[(String, String)],
    name: &str,
    param: &str,
    caller: &str,
) {
    let path = format!("app/views/posts/_{name}.rb");
    let partial = file(files, &path);
    assert_parses(partial, &path);
    let source = file(files, caller);
    assert_parses(source, caller);
    let def = |open: &str| {
        partial
            .lines()
            .find_map(|l| arg_list(l, open))
            .unwrap_or_else(|| panic!("no {open}:\n{partial}"))
    };
    let into_params = def(&format!("def self.{name}_into("));
    let plain_params = def(&format!("def self.{name}("));
    let count = into_params.iter().filter(|p| p.as_str() == param).count();
    assert_eq!(count, 1, "params {into_params:?}\n{partial}");
    let into_call = format!("Views::Posts.{name}_into(");
    let plain_call = format!("Views::Posts.{name}(");
    let mut calls = 0;
    for line in source
        .lines()
        .filter(|l| !l.trim_start().starts_with("def "))
    {
        if let Some(args) = arg_list(line, &into_call) {
            assert_eq!(
                into_params.len(),
                args.len(),
                "params {into_params:?}, args {args:?}\n{source}"
            );
            calls += 1;
        } else if let Some(args) = arg_list(line, &plain_call) {
            assert!(
                args.len() <= plain_params.len(),
                "params {plain_params:?}, args {args:?}\n{source}"
            );
            calls += 1;
        }
    }
    assert!(calls > 0, "no call to the partial in {caller}:\n{source}");
}

/// The issue's shape: a plain render, no locals.
#[test]
fn a_partial_reading_local_assigns_and_the_same_ivar_takes_one_param() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_INDEX),
        ("app/controllers/posts_controller.rb", INDEX_ONLY),
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/note\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(&files, "note", "error", "app/views/posts/index.rb");
}

/// A view render with locals binds the extras by name
/// (`partial_extras_map`): `error` is not one of them any more.
#[test]
fn a_view_render_with_locals_passes_the_same_arguments() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_INDEX),
        ("app/controllers/posts_controller.rb", INDEX_ONLY),
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/note\", error: \"x\", alert: \"a\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(&files, "note", "error", "app/views/posts/index.rb");
    // From a view the closure param takes the caller's `error` (its
    // `@error`), not the render-local "x": `note_into(io, post, error, …)`.
    let source = file(&files, "app/views/posts/index.rb");
    let args = source
        .lines()
        .find_map(|l| arg_list(l, "Views::Posts.note_into("))
        .unwrap_or_else(|| panic!("no note_into call:\n{source}"));
    assert_eq!(args[2], "error", "args {args:?}\n{source}");
}

const CARD: (&str, &str) = (
    "app/views/posts/_card.html.erb",
    "<div class=\"<%= local_assigns[:class] %> <%= @class %>\"></div>\n",
);
const CLASS_INDEX: &str = "class PostsController < ApplicationController\n  def index\n    @class = \"wide\"\n  end\nend\n";

/// A reserved word: the closure carries `@class` as `class_`, the extra
/// is the raw `class`. They are the same local, so still one param.
#[test]
fn a_reserved_word_local_and_ivar_take_one_param() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_INDEX),
        ("app/controllers/posts_controller.rb", CLASS_INDEX),
        CARD,
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/card\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(&files, "card", "class_", "app/views/posts/index.rb");
}

/// The same reserved word as a `locals:` key from a view: the raw key
/// `class` is the closure's `class_`, not a second param.
#[test]
fn a_reserved_word_view_local_passes_the_same_arguments() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_INDEX),
        ("app/controllers/posts_controller.rb", CLASS_INDEX),
        CARD,
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/card\", class: \"x\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(&files, "card", "class_", "app/views/posts/index.rb");
}

/// The same `locals:` key from a controller render
/// (`partial_call_contracts`).
#[test]
fn a_reserved_word_controller_local_passes_the_same_arguments() {
    let files = spinel(&[
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :posts, only: %i[index]\n  get \"posts/preview\", to: \"posts#preview\"\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    @class = \"wide\"\n  end\n\n  def preview\n    @class = \"wide\"\n    render partial: \"posts/card\", locals: { class: \"x\" }\n  end\nend\n",
        ),
        CARD,
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/card\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(
        &files,
        "card",
        "class_",
        "app/controllers/posts_controller.rb",
    );
    // As for `error` below, the same-named local wins over the ivar: the
    // key is `class`, the param it binds is `class_` (#571).
    let source = file(&files, "app/controllers/posts_controller.rb");
    assert!(source.contains("Views::Posts.card(nil, \"x\")"), "{source}");
}

const ROUTES_PREVIEW: &str =
    "Rails.application.routes.draw do\n  get \"posts/preview\", to: \"posts#preview\"\nend\n";

/// A controller render passes the ivar the action assigned, `@class`.
/// The partial's param is `class_`, but `@class_` is an ivar nothing
/// assigns: the partial would render with nil (#571).
#[test]
fn a_controller_render_passes_a_reserved_word_ivar_by_its_own_name() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = emit(
            target,
            &[
                ("config/routes.rb", ROUTES_PREVIEW),
                (
                    "app/controllers/posts_controller.rb",
                    "class PostsController < ApplicationController\n  def preview\n    @class = \"wide\"\n    render partial: \"posts/card\"\n  end\nend\n",
                ),
                CARD,
            ],
        );
        let source = file(&files, "app/controllers/posts_controller.rb");
        assert_parses(source, "app/controllers/posts_controller.rb");
        assert!(
            source.contains("Views::Posts.card(nil, @class)"),
            "{target:?}\n{source}"
        );
    }
}

/// A helper's render binds its `locals:` through the same contract
/// (`apply_library_partial_render_lowering`): the key `class` reaches
/// the param `class_`.
#[test]
fn a_reserved_word_helper_local_reaches_the_partial() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_INDEX),
        ("app/controllers/posts_controller.rb", CLASS_INDEX),
        CARD,
        ("app/views/posts/index.html.erb", "<%= card_html %>\n"),
        (
            "app/helpers/posts_helper.rb",
            "module PostsHelper\n  def card_html\n    render partial: \"posts/card\", locals: { class: \"x\" }\n  end\nend\n",
        ),
    ]);
    let (path, source) = files
        .iter()
        .find(|(p, _)| p.ends_with("posts_helper.rb"))
        .expect("posts_helper.rb");
    assert_parses(source, path);
    assert!(source.contains("Views::Posts.card(nil, \"x\")"), "{source}");
}

/// A controller-side partial render binds the extras through
/// `partial_call_contracts`.
#[test]
fn a_controller_render_with_locals_passes_the_same_arguments() {
    let files = spinel(&[
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :posts, only: %i[index]\n  get \"posts/preview\", to: \"posts#preview\"\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    @error = \"bad\"\n  end\n\n  def preview\n    @error = \"bad\"\n    render partial: \"posts/note\", locals: { error: \"x\", alert: \"a\" }\n  end\nend\n",
        ),
        (
            "app/views/posts/index.html.erb",
            "<%= render \"posts/note\" %>\n",
        ),
    ]);
    assert_one_param_and_matching_calls(
        &files,
        "note",
        "error",
        "app/controllers/posts_controller.rb",
    );
    // The contract lets a same-named local win over the ivar: `error`
    // receives the local "x", not `@error`.
    let source = file(&files, "app/controllers/posts_controller.rb");
    let args = source
        .lines()
        .find_map(|l| arg_list(l, "Views::Posts.note("))
        .unwrap_or_else(|| panic!("no note call:\n{source}"));
    assert_eq!(args[1], "\"x\"", "args {args:?}\n{source}");
}
