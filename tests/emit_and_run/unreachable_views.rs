//! A template no request reaches has its errors reported as warnings
//! (lobsters' `mod/notes/index`, whose ivars no action sets); it is still
//! emitted. One an action, a route, a render or an inheriting controller
//! reaches keeps its errors.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/notes_controller.rb", "class NotesController < ApplicationController\n  def create\n    head :ok\n  end\n\n  def shown\n    render :rendered\n  end\nend\n")
        .write("app/controllers/archived_notes_controller.rb", "class ArchivedNotesController < ::NotesController\n  def inherited\n  end\nend\n")
        .write("app/views/notes/orphan.html.erb", "<p><%= @missing.title %></p>\n")
        .write("app/views/notes/rendered.html.erb", "<p>rendered</p>\n")
        .write("app/views/notes/routed.html.erb", "<p>routed</p>\n")
        .write("app/views/notes/inherited.html.erb", "<p>inherited</p>\n")
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", "Rails.application.routes.draw do\n  get \"notes/routed\", to: \"notes#routed\"\n")
}

#[test]
fn an_unreachable_template_s_errors_are_warnings() {
    let warnings = app().ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("`notes/orphan` is rendered by no action, route or render call")), "{warnings:#?}");
    for kept in ["notes/rendered", "notes/routed", "notes/inherited"] {
        assert!(!warnings.iter().any(|w| w.contains(&format!("`{kept}` is rendered by no action"))), "{kept}: {warnings:#?}");
    }
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("@missing")), "{errors:#?}");
    // The same read in a template an action renders stays an error.
    let (_emitted, errors) = app()
        .write("app/views/notes/rendered.html.erb", "<p><%= @missing.title %></p>\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("@missing")), "{errors:#?}");
}

#[test]
fn the_app_with_it_still_boots() {
    app().run_ruby("raise \"kept\" unless defined?(Views::Notes) && Views::Notes.respond_to?(:rendered) && Views::Notes.respond_to?(:orphan)\nputs \"boots\"\n").assert_passes();
}
