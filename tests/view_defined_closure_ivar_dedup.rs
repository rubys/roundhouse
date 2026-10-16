//! An action view or a layout that does `defined?(@gadget)` AND reads
//! `@gadget`, with a controller that assigns `@gadget`.
//!
//! #389 fixed this for a PARTIAL: `drop_closure_names` ran only inside
//! `if is_partial { … }`, so a partial's `defined?`/`locals:` extras
//! were deduped against its threaded closure ivars
//! (`tests/partial_local_assigns_ivar.rs`), but an action view's or a
//! layout's were not — both thread closure ivars too (an action view's
//! primary params ARE its closure ivars; a layout threads them the same
//! way a partial does), so `@gadget` surfaced twice: once as the
//! closure param, once again as a nil-default `defined?` extra. The
//! emitted method took `gadget` twice —
//!
//!   def self.show_into(io, gadget, widget, notice, alert, gadget)
//!   def self.application_into(io, body, gadget, notice, alert, gadget)
//!
//! — a duplicate argument name, a Ruby syntax error. Fix: run
//! `drop_closure_names` unconditionally, for every view kind.

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
        "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n",
    ),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
];

/// The spinel tree for `APP` plus `files`, with a plain layout unless
/// `files` supplies its own `app/views/layouts/application.html.erb`.
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = APP
        .iter()
        .map(|(path, content)| (PathBuf::from(*path), content.as_bytes().to_vec()))
        .collect();
    tree.entry(PathBuf::from("app/views/layouts/application.html.erb"))
        .or_insert_with(|| b"<html><body><%= yield %></body></html>\n".to_vec());
    for (path, content) in files {
        tree.insert(PathBuf::from(*path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

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
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

/// The comma-separated list inside the first `open(`…`)` on a line —
/// paren-depth aware, so a nested call in an argument position (the
/// layout wrap passes `Views::Widgets.index(...)` as the layout's
/// `body` argument) doesn't get split on ITS commas.
fn arg_list(line: &str, open: &str) -> Option<Vec<String>> {
    let rest = line.split_once(open)?.1;
    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    end = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let inner = &rest[..end?];
    let mut parts = Vec::new();
    let mut part_start = 0;
    let mut depth = 0i32;
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(inner[part_start..i].trim().to_string());
                part_start = i + 1;
            }
            _ => {}
        }
    }
    let last = inner[part_start..].trim();
    if !last.is_empty() || !parts.is_empty() {
        parts.push(last.to_string());
    }
    Some(parts)
}

/// A def param's bare name, with its ` = default` (if any) stripped —
/// so a positional `gadget` and a nil-default `gadget = nil` count as
/// the same name for a duplicate check.
fn param_name(p: &str) -> &str {
    p.split(" = ").next().unwrap_or(p)
}

/// `name` appears exactly once in `def_open`'s param list, and the call
/// found in `caller` at `call_open` passes no more arguments than that
/// (the `_into` form has no defaults, so its call always matches
/// exactly; the plain wrapper defaults its trailing extras, so a call
/// may omit some — same rule `assert_one_param_and_matching_calls` in
/// `tests/partial_local_assigns_ivar.rs` uses).
fn assert_no_duplicate_param_and_matching_call(
    def_source: &str,
    def_path: &str,
    def_open: &str,
    caller_source: &str,
    caller_path: &str,
    call_open: &str,
    dup_name: &str,
) {
    assert_parses(def_source, def_path);
    assert_parses(caller_source, caller_path);
    let params = def_source
        .lines()
        .find_map(|l| arg_list(l, def_open))
        .unwrap_or_else(|| panic!("no {def_open} in {def_path}:\n{def_source}"));
    let count = params.iter().filter(|p| param_name(p) == dup_name).count();
    assert_eq!(count, 1, "params {params:?}\n{def_source}");
    let args = caller_source
        .lines()
        .find_map(|l| arg_list(l, call_open))
        .unwrap_or_else(|| panic!("no call to {call_open} in {caller_path}:\n{caller_source}"));
    assert!(
        args.len() <= params.len(),
        "params {params:?}, args {args:?}\n{caller_source}"
    );
}

const ROUTES_SHOW: &str =
    "Rails.application.routes.draw do\n  resources :widgets, only: %i[show]\nend\n";

/// The issue's action-view shape: `show.html.erb` reads `@gadget` through
/// both a `defined?(@gadget)` guard and a plain `@gadget` read, and also
/// reads `@widget` (an ordinary closure ivar, no `defined?`). The
/// controller assigns both.
#[test]
fn an_action_view_with_a_defined_guard_on_a_closure_ivar_takes_it_once() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES_SHOW),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    @gadget = \"g\"\n  end\nend\n",
        ),
        (
            "app/views/widgets/show.html.erb",
            "<% if defined?(@gadget) %><%= @gadget %><% end %><%= @widget.title %>\n",
        ),
    ]);
    let view = file(&files, "app/views/widgets/show.rb");
    let controller = file(&files, "app/controllers/widgets_controller.rb");
    assert_no_duplicate_param_and_matching_call(
        view,
        "app/views/widgets/show.rb",
        "def self.show(",
        controller,
        "app/controllers/widgets_controller.rb",
        "Views::Widgets.show(",
        "gadget",
    );
    // The underlying `_into` method (no defaults) must not duplicate
    // the argument name either.
    let into_params = view
        .lines()
        .find_map(|l| arg_list(l, "def self.show_into("))
        .unwrap_or_else(|| panic!("no show_into:\n{view}"));
    assert_eq!(
        into_params.iter().filter(|p| p.as_str() == "gadget").count(),
        1,
        "params {into_params:?}\n{view}"
    );
}

/// The layout shape: `application.html.erb` reads `@gadget` the same
/// way, threaded in by the layout wrap from a controller action that
/// assigns it. No action-view ivar competes here — the layout's own
/// `defined?`/plain-read pair is the whole story.
#[test]
fn a_layout_with_a_defined_guard_on_a_closure_ivar_takes_it_once() {
    let files = spinel(&[
        (
            "app/views/layouts/application.html.erb",
            "<% if defined?(@gadget) %><%= @gadget %><% end %><%= yield %>\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[index]\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @gadget = \"g\"\n  end\nend\n",
        ),
        ("app/views/widgets/index.html.erb", "<p>hi</p>\n"),
    ]);
    let layout = file(&files, "app/views/layouts/application.rb");
    let controller = file(&files, "app/controllers/widgets_controller.rb");
    assert_no_duplicate_param_and_matching_call(
        layout,
        "app/views/layouts/application.rb",
        "def self.application(",
        controller,
        "app/controllers/widgets_controller.rb",
        "Views::Layouts.application(",
        "gadget",
    );
    // The underlying `_into` method (no defaults) must not duplicate
    // the argument name either.
    let into_params = layout
        .lines()
        .find_map(|l| arg_list(l, "def self.application_into("))
        .unwrap_or_else(|| panic!("no application_into:\n{layout}"));
    assert_eq!(
        into_params.iter().filter(|p| p.as_str() == "gadget").count(),
        1,
        "params {into_params:?}\n{layout}"
    );
}
