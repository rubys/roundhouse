//! A leading `/` means "from the top level" in a route target and in a
//! partial path. It is not an empty namespace segment.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
#[allow(dead_code)]
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

#[allow(dead_code)]
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

#[allow(dead_code)]
fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const ROUTES: &str = r#"Rails.application.routes.draw do
  namespace :api do
    namespace :v1 do
      resources :widgets, only: %i[show]
      match "*unmatched", to: "/errors#routing", via: :all
    end
  end
end
"#;

const WIDGETS: &str = r#"module Api
  module V1
    class WidgetsController < ApplicationController
      def show
        @widget = Widget.find(params[:id])
      end
    end
  end
end
"#;

fn files() -> Vec<(String, String)> {
    spinel(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("config/routes.rb", ROUTES),
        ("app/controllers/errors_controller.rb", "class ErrorsController < ApplicationController\n  def routing\n    head :not_found\n  end\nend\n"),
        ("app/controllers/api/v1/widgets_controller.rb", WIDGETS),
        ("app/views/api/v1/widgets/show.json.jbuilder", "json.partial!(\"/api/v1/widgets/widget\", widget: @widget)\n"),
        ("app/views/api/v1/widgets/_widget.json.jbuilder", "json.call(widget, :id, :name)\n"),
    ])
}

#[test]
fn an_absolute_route_target_is_the_top_level_controller() {
    let files = files();
    assert_parses(&files, "main.rb");
    let main = file(&files, "main.rb");
    assert!(main.contains("then ErrorsController.new"), "{main}");
    assert!(!main.contains("::::"), "{main}");
}

#[test]
fn an_absolute_partial_path_is_the_views_module() {
    let files = files();
    assert_parses(&files, "app/views/api/v1/widgets/show_json.rb");
    let view = file(&files, "app/views/api/v1/widgets/show_json.rb");
    assert!(view.contains("Views::Api::V1::Widgets.widget_json("), "{view}");
}
