//! Three more shapes current lobsters reaches on its benchmark routes.
//!
//! - `Rails.application.credentials.pushover&.api_token` (settings):
//!   Rails' EncryptedConfiguration answers an absent key as nil through
//!   a method; the runtime's (deliberately empty) store is a Hash, so
//!   the read becomes `credentials[:pushover]`.
//! - `@user.notifications.offset(n).limit(25).order(…)` (inbox): an
//!   association read continuing into ANY relation-only chain method is
//!   rooted as a query, not just one that starts with `where`.
//! - `render "_commentbox", locals: {…}` (comment reply): a partial file
//!   rendered as the page — the partial's function with those locals,
//!   and the layout kept (unlike `render partial:`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const CONTROLLER: &str = r#"class NotesController < ApplicationController
  def index
    @user = User.first
    @enabled = Rails.application.credentials.pushover&.api_token.present?
    @notes = @user.notes.offset(0).limit(25).order(id: :desc)
  end

  def show
    note = Note.find(params[:id])
    render "_note", locals: { note: note, extra: 1 }
  end
end
"#;

fn emitted() -> String {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"notes\", force: :cascade do |t|\n    t.integer \"user_id\"\n    t.string \"body\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :notes\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :user\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/notes_controller.rb", CONTROLLER),
        ("app/views/notes/index.html.erb", "<%= @notes.length %>\n"),
        ("app/views/notes/_note.html.erb", "<%= note.body %><%= extra %>\n"),
        ("app/views/layouts/application.html.erb", "<html><body><%= yield %></body></html>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :notes, only: [:index, :show]\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers_with_layout(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("notes_controller.rb"))
        .map(|f| f.content)
        .expect("notes_controller.rb")
}

#[test]
fn a_credentials_key_read_is_an_index() {
    let out = emitted();
    assert!(out.contains("Rails.application.credentials[:pushover]"), "{out}");
    assert!(!out.contains("credentials.pushover"), "{out}");
}

#[test]
fn an_association_read_into_offset_is_a_query() {
    let out = emitted();
    assert!(out.contains("ActiveRecord::Relation.new(Note).where(user_id: @user.id)"), "{out}");
    assert!(!out.contains("@user.notes.offset"), "{out}");
}

#[test]
fn an_underscored_template_renders_the_partial_inside_the_layout() {
    let out = emitted();
    assert!(out.contains("Views::Layouts.application(Views::Notes.note(note"), "{out}");
    assert!(!out.contains("MissingTemplate"), "{out}");
}
