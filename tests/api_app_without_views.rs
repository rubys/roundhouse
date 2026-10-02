//! An app with no view templates, such as an API app whose controllers
//! only `render json:`, gets an `app/views.rb` that requires nothing.
//! It used to keep the scaffold blog's aggregator, which requires
//! `views/comments/_comment` and the rest of the blog's views. The tree
//! has none of them, so `spin build` stopped at the first one (#164).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

/// The app from #164.
const APP: &[(&str, &str)] = &[
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n"),
    (
        "app/controllers/widgets_controller.rb",
        "class WidgetsController < ApplicationController\n  before_action :set_widget, only: :show\n\n  def index\n    render json: Widget.all\n  end\n\n  def show\n    head :not_found unless @widget\n  end\n\n  private\n\n  def set_widget\n    @widget = Widget.find_by(id: params[:id])\n  end\nend\n",
    ),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: %i[index show]\nend\n"),
    (
        "db/schema.rb",
        "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
    ),
];

fn emit(target: BuildTarget) -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> =
        APP.iter().map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), target).expect("target files")
}

#[test]
fn an_app_without_views_gets_a_views_aggregator_that_requires_nothing() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = emit(target);
        assert!(!files.iter().any(|(p, _)| p.starts_with("app/views/")), "{target:?} emitted a view");
        let views = &files
            .iter()
            .find(|(p, _)| p == "app/views.rb")
            .unwrap_or_else(|| panic!("{target:?}: app/views.rb not emitted"))
            .1;
        // Nothing but comments: no view to load, so nothing to require.
        assert!(
            views.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')),
            "{target:?}:\n{views}"
        );
    }
}
