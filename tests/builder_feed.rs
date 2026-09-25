//! A `.builder` feed end to end: ingested as a view, lowered as
//! `<action>_rss` beside the html template, and reached from the
//! `format.rss` branch of `respond_to`.
//!
//! Current lobsters serves /rss from `home/stories.rss.builder` via
//! `format.rss { render action: "stories", layout: false }`; the 2023
//! snapshot used a format-agnostic `rss.erb`, and `.builder` templates
//! were skipped outright.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const CONTROLLER: &str = r#"class NotesController < ApplicationController
  def index
    @notes = Note.all
    @title = "Notes"
    respond_to do |format|
      format.html { render action: "index" }
      format.rss { render action: "feed", layout: false }
    end
  end
end
"#;

const FEED: &str = r#"xml.instruct! :xml, version: "1.0"
xml.rss version: "2.0" do
  xml.channel do
    xml.title @title
    @notes.each do |note|
      xml.item do
        xml.title note.body
        xml.pubDate note.created_at.rfc822
      end
    end
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n    t.datetime \"created_at\", null: false\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/notes_controller.rb", CONTROLLER),
        ("app/views/notes/index.html.erb", "<%= @notes.length %>\n"),
        ("app/views/notes/feed.rss.builder", FEED),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/notes\" => \"notes#index\"\n  get \"/notes.rss\" => \"notes#index\", :format => \"rss\"\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

#[test]
fn the_rss_branch_renders_the_feed_view_with_its_mime_type() {
    let app = app();
    let controller = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("notes_controller.rb"))
        .map(|f| f.content)
        .expect("controller");
    assert!(
        controller.contains("Views::Notes.feed_rss(@notes, @title"),
        "args in the view's declared order:\n{controller}"
    );
    assert!(controller.contains("application/rss+xml"), "{controller}");
    let views = ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(views.contains("def self.feed_rss(notes, title"), "{views}");
    assert!(views.contains("<?xml version=\\\"1.0\\\" encoding=\\\"UTF-8\\\"?>"), "{views}");
    assert!(views.contains("builder_text"), "{views}");
    // `rfc822` is Rails' `to_fs(:rfc822)` on a UTC TimeWithZone.
    assert!(views.contains("created_at.getutc.strftime(\"%a, %d %b %Y %H:%M:%S %z\")"), "{views}");
}
