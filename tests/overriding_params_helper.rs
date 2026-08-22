//! A subclass that overrides `<x>_params` still yields the params class.
//!
//! campfire's `Messages::ByBotsController#message_params` overrides its
//! parent's with a shape the permit recognizer cannot read — one branch
//! is `params.permit(:attachment)` with no `require`, the other a bare
//! `{ body: body }` returned from inside a block. No spec was
//! recognized, so the method emitted VERBATIM and BOTH branches were
//! broken: one called `permit` on a Hash, the other handed a Hash to the
//! parent's injected `.to_attrs`. The Hash branch just failed first.
//!
//! The class comes from the PARENT's helper of the same name, which is
//! what Rails means too: whatever consumes it expects the same thing no
//! matter which subclass supplied it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :body\n    t.string :tag\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :notes\n  \
    namespace :notes do\n    resources :bots, only: [ :create ]\n  end\nend\n";

const PARENT: &str = "class NotesController < ApplicationController\n  \
    def create\n    @note = Note.create!(note_params)\n  end\n\n  \
    private\n    def note_params\n      params.require(:note).permit(:body, :tag)\n    end\nend\n";

fn emit(subclass: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/note.rb"),
        b"class Note < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("config/routes.rb"), ROUTES.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/notes_controller.rb"),
        PARENT.as_bytes().to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/notes/bots_controller.rb"),
        subclass.as_bytes().to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers_with_layout(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("bots_controller.rb"))
        .expect("bots_controller.rb")
        .content
}

/// A bare Hash literal in return position becomes the params object,
/// with each written key marked provided BY BEING WRITTEN.
#[test]
fn a_hash_literal_becomes_the_parents_params_class() {
    let src = emit(
        "class Notes::BotsController < NotesController\n  \
           private\n    def note_params\n      { body: \"hi\" }\n    end\nend\n",
    );
    assert!(src.contains("NoteParams.new"), "constructs the class:\n{src}");
    assert!(src.contains("body_provided = true"), "written means provided:\n{src}");
    assert!(!src.contains("{ body:"), "no bare Hash may survive:\n{src}");
}

/// A `permit` with no `require` reads the TOP level — `from_raw` could
/// not serve it, since that factory opens by digging into the resource
/// sub-hash.
#[test]
fn a_bare_permit_reads_the_top_level_params() {
    let src = emit(
        "class Notes::BotsController < NotesController\n  \
           private\n    def note_params\n      params.permit(:body)\n    end\nend\n",
    );
    assert!(src.contains("NoteParams.new"), "{src}");
    assert!(
        src.contains(r#"Params.provided(@params, "body")"#),
        "reads presence off the flat params:\n{src}"
    );
    assert!(!src.contains("from_raw"), "from_raw would dig into :note:\n{src}");
}

/// Only the permitted key is set. A shared flat factory would widen
/// every such call to the class's full field list; this keeps
/// `permit(:body)` meaning body and nothing else.
#[test]
fn a_bare_permit_sets_only_what_it_names() {
    let src = emit(
        "class Notes::BotsController < NotesController\n  \
           private\n    def note_params\n      params.permit(:body)\n    end\nend\n",
    );
    assert!(!src.contains("tag_provided"), "tag was not permitted here:\n{src}");
}

/// DECLINES on a key the parent never permitted. That is a different
/// list, and quietly dropping the extra would be mass assignment by
/// omission.
#[test]
fn an_unknown_key_declines() {
    let src = emit(
        "class Notes::BotsController < NotesController\n  \
           private\n    def note_params\n      { body: \"hi\", secret: \"x\" }\n    end\nend\n",
    );
    assert!(!src.contains("NoteParams.new"), "must decline whole:\n{src}");
}

/// A subclass with its OWN full permit chain is untouched — it has a
/// spec of its own and needs nothing from its parent.
#[test]
fn a_subclass_with_its_own_permit_is_unchanged() {
    let src = emit(
        "class Notes::BotsController < NotesController\n  \
           private\n    def note_params\n      params.require(:note).permit(:body)\n    end\nend\n",
    );
    assert!(src.contains("from_raw"), "keeps the recognized shape:\n{src}");
}
