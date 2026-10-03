//! `class ApplicationController < ActionController::API`, the base
//! `rails new --api` writes, names a class the ruby-family runtime now
//! defines. It defined only `Base`, so the ruby tree raised NameError
//! loading the controller and every request to the spinel binary
//! answered 500 (`undefined method 'params='`, #163).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

#[test]
fn every_ruby_family_tree_defines_action_controller_api() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"widgets#index\"\nend\n"),
    ]
    .iter()
    .map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Spinel, BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = target_files(&app, Path::new("."), target).expect("target files");
        let file = |path: &str| {
            files
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, c)| c.as_str())
                .unwrap_or_else(|| panic!("{target:?}: {path} not emitted"))
        };
        assert!(
            file("app/controllers/application_controller.rb").contains("< ActionController::API"),
            "{target:?}"
        );
        // The controller requires the aggregator, which requires the class.
        assert!(
            file("runtime/action_controller.rb").contains("require_relative \"action_controller/api\""),
            "{target:?}"
        );
        assert!(file("runtime/action_controller/api.rb").contains("class API < Base"), "{target:?}");
    }
}
