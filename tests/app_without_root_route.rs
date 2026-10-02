//! An app that declares no `root` route leaves `RouteTable.root` out of
//! its dispatch table. `main.rb` and the test harness used to call it,
//! but the routes emit defines it only for a route at `/`, so `spin
//! build` refused the call and the ruby tree raised NoMethodError on
//! its first request (#165).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

fn emit(routes: &str, target: BuildTarget) -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("config/routes.rb", routes),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
    ]
    .iter()
    .map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), target).expect("target files")
}

fn file<'a>(files: &'a [(String, String)], path: &str, target: BuildTarget) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{target:?}: {path} not emitted"))
        .1
}

#[test]
fn an_app_without_a_root_route_never_calls_route_table_root() {
    let routes = "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n";
    for target in [BuildTarget::Spinel, BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = emit(routes, target);
        assert!(!file(&files, "config/routes.rb", target).contains("def self.root"), "{target:?}");
        for path in ["main.rb", "test/test_helper.rb"] {
            let content = file(&files, path, target);
            assert!(!content.contains("RouteTable.root"), "{target:?} {path} calls RouteTable.root");
            assert!(content.contains("RouteTable.table + ActiveStorage::Routes.table"), "{target:?} {path}");
        }
    }
}

#[test]
fn an_app_with_a_root_route_keeps_it_first_in_the_table() {
    let routes = "Rails.application.routes.draw do\n  root \"widgets#index\"\n  resources :widgets, only: :index\nend\n";
    for target in [BuildTarget::Spinel, BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = emit(routes, target);
        assert!(file(&files, "config/routes.rb", target).contains("def self.root"), "{target:?}");
        for path in ["main.rb", "test/test_helper.rb"] {
            assert!(
                file(&files, path, target).contains("[RouteTable.root] + RouteTable.table"),
                "{target:?} {path}"
            );
        }
    }
}
