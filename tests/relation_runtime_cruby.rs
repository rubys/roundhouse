//! `Relation#load_async` and `Relation#touch_all` against a real emitted
//! CRuby tree and SQLite — the runtime methods lobsters' story page and
//! inbox (`after_action :update_read_at`) reach.
//!
//! Skips when `ruby` with the sqlite3 gem is not on the machine; the CI
//! core job has it.

use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

fn tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-relation-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n    t.datetime \"read_at\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/notes_controller.rb", "class NotesController < ApplicationController\n  def index\n    @notes = Note.all\n  end\nend\n"),
        ("app/views/notes/index.html.erb", "<%= @notes.length %>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :notes, only: [:index]\nend\n"),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let mut app = ingest_app(&dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::target_files(&app, &dir, BuildTarget::Ruby).expect("files");
    let out = dir.join("emitted");
    roundhouse::project::write_to_dir(&files, &out).expect("write");
    out
}

#[test]
fn load_async_and_touch_all_answer_like_rails() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = tree();
    let script = r#"
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
a = Note.new; a.body = "a"; a.save!
b = Note.new; b.body = "b"; b.save!
p ActiveRecord::Relation.new(Note).where(body: "a").load_async.length
p ActiveRecord::Relation.new(Note).where(read_at: nil).where(body: "a").touch_all(:read_at)
p ActiveRecord::Relation.new(Note).where(read_at: nil).length
p Note.find(a.id).read_at.nil?
"#;
    let result = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&out)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "1\n1\n1\nfalse\n");
}
