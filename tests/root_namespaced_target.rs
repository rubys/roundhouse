//! `root to: "rails/health#show"` dispatches to `Rails::HealthController`.
//!
//! The root route camelized its controller with `camelize`, which keeps
//! the `/` of a namespaced target, so the spinel `main.rb` read
//! `when :rails_health then Rails/healthController.new` — a division by
//! an undefined local, raising NameError on every `GET /`. The explicit
//! verb form (`get "up" => "rails/health#show"`) already named the class.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

fn spinel(routes: &str) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    render plain: \"widgets\"\n  end\nend\n",
        ),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", routes),
    ] {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
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

#[test]
fn a_namespaced_root_target_names_the_namespaced_controller() {
    let files = spinel(
        "Rails.application.routes.draw do\n  root to: \"rails/health#show\"\n  get \"widgets\" => \"widgets#index\"\nend\n",
    );
    let main = file(&files, "main.rb");
    assert!(main.contains("when :rails_health then Rails::HealthController.new"), "{main}");
    assert!(!main.contains("Rails/health"), "{main}");
}

#[test]
fn the_positional_root_form_does_too() {
    let files = spinel(
        "Rails.application.routes.draw do\n  root \"rails/health#show\"\n  get \"widgets\" => \"widgets#index\"\nend\n",
    );
    let main = file(&files, "main.rb");
    assert!(main.contains("Rails::HealthController.new"), "{main}");
}
