//! A routed action with a template and no method behind it is an
//! action. Rails dispatches `show` whether or not `def show` exists:
//! the filters run and the implicit render finds the template. Ingest
//! writes the empty method (`synthesize_template_only_actions`), so the
//! `before_action` feeds the template and every target has an action to
//! route to. `tests/emit_and_run.rs` pins that the emitted app serves it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

const NOTES: &str = r#"class NotesController < ApplicationController
  before_action :set_note, only: [:show, :edit]

  def edit
  end

  private

  def set_note
    @note = Note.find(params[:id])
  end
end
"#;

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ("app/views/notes/show.html.erb", "<p><%= @note.body %></p>\n"),
    ("app/views/notes/edit.html.erb", "<p><%= @note.body %></p>\n"),
];

fn analyzed(extra: &[(&str, &str)]) -> (roundhouse::App, Vec<String>) {
    let tree: HashMap<PathBuf, Vec<u8>> = BASE
        .iter()
        .chain(extra)
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    (app, diagnostics)
}

fn actions_of(app: &roundhouse::App, controller: &str) -> Vec<String> {
    app.controllers
        .iter()
        .find(|c| c.name.0.as_str() == controller)
        .unwrap_or_else(|| panic!("{controller}"))
        .actions()
        .map(|a| a.name.as_str().to_string())
        .collect()
}

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :notes, only: [:show, :edit]\nend\n";

#[test]
fn a_routed_template_without_a_method_is_fed_by_its_before_action() {
    let (app, found) = analyzed(&[
        ("config/routes.rb", ROUTES),
        ("app/controllers/notes_controller.rb", NOTES),
    ]);
    assert!(found.is_empty(), "{found:#?}");
    let actions = actions_of(&app, "NotesController");
    assert_eq!(actions.iter().filter(|a| *a == "show").count(), 1, "{actions:?}");
    // The method the author wrote is not written twice.
    assert_eq!(actions.iter().filter(|a| *a == "edit").count(), 1, "{actions:?}");
}

#[test]
fn a_template_no_route_reaches_is_not_an_action() {
    let (app, _) = analyzed(&[
        ("config/routes.rb", ROUTES),
        ("app/controllers/notes_controller.rb", NOTES),
        ("app/views/notes/orphan.html.erb", "<p>unrouted</p>\n"),
    ]);
    let actions = actions_of(&app, "NotesController");
    assert!(!actions.iter().any(|a| a == "orphan"), "{actions:?}");
}

#[test]
fn an_action_a_parent_controller_defines_is_not_written_again() {
    let (app, found) = analyzed(&[
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  namespace :admin do\n    resources :notes, only: [:show, :edit]\n  end\nend\n",
        ),
        (
            "app/controllers/notes_controller.rb",
            "class NotesController < ApplicationController\n  def show\n    @note = Note.find(params[:id])\n  end\n\n  def edit\n    @note = Note.find(params[:id])\n  end\nend\n",
        ),
        (
            "app/controllers/admin/notes_controller.rb",
            "class Admin::NotesController < NotesController\nend\n",
        ),
        ("app/views/admin/notes/show.html.erb", "<p><%= @note.body %></p>\n"),
        ("app/views/admin/notes/edit.html.erb", "<p><%= @note.body %></p>\n"),
    ]);
    assert!(found.is_empty(), "{found:#?}");
    assert!(actions_of(&app, "Admin::NotesController").is_empty());
}

fn template_only_roda_app(callback: &str) -> roundhouse::App {
    let parent = format!(
        "class ApplicationController < ActionController::Base\n  {callback}\n  private\n  def require_login\n    head :unauthorized\n  end\nend\n"
    );
    let extra = [
        ("app/controllers/application_controller.rb", parent.as_str()),
        ("app/controllers/notes_controller.rb", "class NotesController < ApplicationController\nend\n"),
        ("app/controllers/public_controller.rb", "class PublicController < ActionController::Base\nend\n"),
        ("app/views/notes/show.html.erb", "<p>PRIVATE CONTENT</p>\n"),
        ("app/views/public/show.html.erb", "<p>PUBLIC CONTENT</p>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get '/notes/:id', to: 'notes#show'\n  get '/public/:id', to: 'public#show'\nend\n"),
    ];
    let tree = BASE.iter().chain(&extra)
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest template-only Roda app")
}

#[test]
fn roda_template_only_actions_do_not_silently_drop_inherited_callbacks() {
    for callback in [
        "before_action :require_login, only: :show",
        "before_action { head :unauthorized }",
        "around_action :require_login",
        "around_action { head :unauthorized }",
        "after_action :require_login",
        "skip_before_action :require_login",
        "skip_around_action :require_login",
        "skip_after_action :require_login",
    ] {
        let app = template_only_roda_app(callback);
        let files = roundhouse::emit::roda::emit(&app);
        let source = &files.iter().find(|f| f.path == PathBuf::from("app.rb")).unwrap().content;
        assert!(source.contains("template-only action callbacks are not converted"), "{callback}: {source}");
    }
    let files = roundhouse::emit::roda::emit(&template_only_roda_app(""));
    let source = &files.iter().find(|f| f.path == PathBuf::from("app.rb")).unwrap().content;
    assert!(!source.contains("template-only action callbacks are not converted"), "{source}");
}

#[test]
#[ignore = "requires the real Roda, Sequel, sqlite3 and render gems"]
fn roda_template_only_protected_request_fails_closed() {
    let app = template_only_roda_app("before_action :require_login, only: :show");
    let scratch = std::env::temp_dir().join(format!("roundhouse-template-only-roda-{}", std::process::id()));
    for file in roundhouse::emit::roda::emit(&app) {
        let path = scratch.join(file.path);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create output parent");
        std::fs::write(path, file.content).expect("write Roda output");
    }
    let output = std::process::Command::new("ruby")
        .args(["-e", r#"require './app'
require 'rack/mock'
requests = Rack::MockRequest.new(App.freeze.app)
protected = requests.get('/notes/1')
raise "protected route answered #{protected.status}" unless protected.status == 501
raise 'private content leaked' if protected.body.include?('PRIVATE CONTENT')
public_page = requests.get('/public/1')
raise "public control answered #{public_page.status}" unless public_page.status == 200
raise 'public control did not render' unless public_page.body.include?('PUBLIC CONTENT')
puts 'protected=501, private content absent; public control=200'
"#])
        .current_dir(&scratch)
        .output()
        .expect("execute emitted Roda app");
    std::fs::remove_dir_all(&scratch).expect("remove Roda output");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
