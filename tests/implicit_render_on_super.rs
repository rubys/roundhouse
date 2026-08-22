//! An action a subclass reaches with `super` renders in the DISPATCHER.
//!
//! Rails runs the default render AFTER the action returns —
//! `send_action`, then `default_render` unless `performed?`. We inlined
//! it at the end of the action body instead, which is the same thing
//! until a subclass writes
//!
//! ```text
//! def create
//!   super          # parent body runs...
//!   head :created  # ...and THIS is the response
//! end
//! ```
//!
//! There the inlined version fires the PARENT's default render while
//! `super` is still on the stack, before the subclass can respond at
//! all. campfire's `Messages::ByBotsController#create` is exactly that
//! shape, and every bot message died on `ActionView::MissingTemplate`
//! for a request the subclass was about to answer with `head :created`.
//!
//! Demand-gated: a controller nobody subclasses this way keeps a
//! byte-identical body and dispatcher, which the last test here pins.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :notes\n  \
    namespace :notes do\n    resources :bots, only: [ :create ]\n  end\nend\n";

fn emit(files: Vec<(&str, &str)>) -> HashMap<String, String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/note.rb"),
        b"class Note < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("config/routes.rb"), ROUTES.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/views/notes/create.turbo_stream.erb"),
        b"<div>made</div>\n".to_vec(),
    );
    for (p, c) in files {
        tree.insert(PathBuf::from(p), c.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers_with_layout(&app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect()
}

const PARENT: (&str, &str) = (
    "app/controllers/notes_controller.rb",
    "class NotesController < ApplicationController\n  \
       def create\n    @note = Note.create!(body: \"x\")\n  end\nend\n",
);

const SUBCLASS_WITH_SUPER: (&str, &str) = (
    "app/controllers/notes/bots_controller.rb",
    "class Notes::BotsController < NotesController\n  \
       def create\n    super\n    head :created\n  end\nend\n",
);

fn file<'a>(files: &'a HashMap<String, String>, needle: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(needle))
        .unwrap_or_else(|| {
            panic!("no {needle} in {:?}", files.keys().collect::<Vec<_>>())
        })
        .1
        .as_str()
}

/// The parent's BODY loses the tail and its DISPATCHER gains it.
#[test]
fn a_supered_action_renders_from_the_dispatcher() {
    let files = emit(vec![PARENT, SUBCLASS_WITH_SUPER]);
    let parent = file(&files, "notes_controller.rb");
    let (dispatcher, body) = parent
        .split_once("def create")
        .expect("a create method");
    assert!(
        !body.contains("MissingTemplate") && !body.contains("request_format =="),
        "the action body must not carry the default render:\n{body}"
    );
    assert!(
        dispatcher.contains("self.create"),
        "dispatcher dispatches: {dispatcher}"
    );
    assert!(
        dispatcher.contains("performed?") && dispatcher.contains("request_format =="),
        "the dispatcher carries the guarded default render:\n{dispatcher}"
    );
}

/// The tail is LOWERED, not a bare `render :create`. It is synthesized
/// into the body so it rides the render rewrite, then split back off —
/// building it in the dispatcher directly emitted a symbol-form render
/// that resolves to nothing.
#[test]
fn the_moved_tail_is_a_views_call_not_a_symbol_render() {
    let files = emit(vec![PARENT, SUBCLASS_WITH_SUPER]);
    let parent = file(&files, "notes_controller.rb");
    assert!(
        parent.contains("Views::Notes.create_turbo_stream("),
        "the moved tail must be bound to its view:\n{parent}"
    );
    assert!(
        !parent.contains("render(:create"),
        "no symbol-form render may survive:\n{parent}"
    );
}

/// The SUBCLASS is untouched: its own body terminates in `head`, so
/// there was never a default render to move.
#[test]
fn the_subclass_dispatcher_gains_nothing() {
    let files = emit(vec![PARENT, SUBCLASS_WITH_SUPER]);
    let sub = file(&files, "bots_controller.rb");
    assert!(
        !sub.contains("request_format =="),
        "subclass needs no default render:\n{sub}"
    );
    assert!(sub.contains("head(:created"), "{sub}");
}

/// DEMAND GATE. With no `super` anywhere, the parent keeps the tail in
/// its body and the dispatcher stays a bare dispatch — byte-identical
/// to what it emitted before this pass existed.
#[test]
fn a_controller_nobody_supers_is_unchanged() {
    let files = emit(vec![PARENT]);
    let parent = file(&files, "notes_controller.rb");
    let (dispatcher, body) = parent.split_once("def create").expect("a create method");
    assert!(
        body.contains("request_format =="),
        "the tail stays in the body:\n{body}"
    );
    assert!(
        !dispatcher.contains("performed?") || !dispatcher.contains("request_format =="),
        "the dispatcher must not grow a render:\n{dispatcher}"
    );
}
