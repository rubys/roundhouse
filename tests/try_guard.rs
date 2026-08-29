//! `recv.try(:m)` guards DEFINEDNESS, not nilness.
//!
//! Ingest used to ground it to `recv && recv.m` — the `&.` desugar. The
//! two agree exactly when the receiver either is nil or DOES define the
//! method. campfire's own `turbo_test_helper` is where they part:
//!
//! ```ruby
//! streambles.collect { |s| s.try(:to_gid_param) || s }.join(":")
//! ```
//!
//! over `[room, :messages]`. `:messages` is not nil and answers no
//! `to_gid_param`, so Rails returns nil and takes the `|| s` arm; the
//! nil guard called it and raised.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn emit(files: &[(&str, &str)]) -> String {
    let mut all: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        // campfire has this file, and its presence is what lets the
        // cover collapse to one arm: `ApplicationRecord` is an APP class
        // there, so it can carry the synthesized `to_gid_param` the
        // narrowing tests for.
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ];
    all.extend_from_slice(files);
    let tree: HashMap<PathBuf, Vec<u8>> = all
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .into_iter()
        .chain(ruby::emit_library(&app))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `to_gid_param` is synthesized onto EVERY model, so the test is the
/// base they share — one `is_a?`, not fifteen.
#[test]
fn a_universal_synthesized_method_narrows_to_the_shared_base() {
    let src = emit(&[(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  def label(s)\n    s.try(:to_gid_param) || s\n  end\nend\n",
    )]);
    assert!(
        src.contains("is_a?(ApplicationRecord)"),
        "every model answers `to_gid_param`, so their base is the test:\n{src}"
    );
    assert!(
        !src.contains("s && s.to_gid_param"),
        "the nil guard is what raised on a Symbol:\n{src}"
    );
}

/// THE PARENTHESES ARE THE FIX AS MUCH AS THE NARROWING IS. A
/// modifier-`if` binds looser than `||`, so `s.to_gid_param if s.is_a?(X)
/// || s` swallows the fallback INTO the condition and answers nil.
#[test]
fn a_conditional_operand_is_parenthesized() {
    let src = emit(&[(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  def label(s)\n    s.try(:to_gid_param) || s\n  end\nend\n",
    )]);
    let line = src
        .lines()
        .find(|l| l.contains("to_gid_param") && l.contains("|| s"))
        .expect("the try site")
        .trim();
    assert!(
        line.starts_with('(') && line.contains(") || s"),
        "without the parens the `|| s` lands inside the condition:\n{line}"
    );
}

/// A method the tree declares on ONE class narrows to that class. Nil
/// still answers nil — `nil.is_a?(X)` is false — so the narrowing does
/// everything the nil guard did.
#[test]
fn a_single_definer_narrows_to_itself() {
    let src = emit(&[(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  def nickname\n    \"nick\"\n  end\n\n  \
         def show(other)\n    other.try(:nickname)\n  end\nend\n",
    )]);
    assert!(
        src.contains("is_a?(User)"),
        "only User answers `nickname`:\n{src}"
    );
}

/// A name no APP class declares may still be a RUNTIME method
/// (`to_param`, `strip`). This pass sees app classes only, so folding to
/// nil would be wrong for exactly those — the nil guard stays.
#[test]
fn an_unknown_name_keeps_the_nil_guard() {
    let src = emit(&[(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  def show(other)\n    other.try(:strip)\n  end\nend\n",
    )]);
    assert!(
        src.contains("other && other.strip"),
        "a runtime method is invisible here; guessing nil would be worse:\n{src}"
    );
}

/// `try!` RAISES where `try` returns nil, so its semantics for a missing
/// method are already the nil guard's. Narrowing it into a silent nil
/// would be the wrong direction.
#[test]
fn try_bang_keeps_the_nil_guard() {
    let src = emit(&[(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  def nickname\n    \"nick\"\n  end\n\n  \
         def show(other)\n    other.try!(:nickname)\n  end\nend\n",
    )]);
    assert!(
        src.contains("other && other.nickname"),
        "try! must not become a silent nil:\n{src}"
    );
}

/// THE DRIFT GUARD on `SYNTHESIZED_ON_EVERY_MODEL`. That list exists
/// because `to_gid_param` is pushed at emit-prep time, after this pass
/// runs — so the pass cannot see it and has to be told. If the synthesis
/// moves or stops, the list silently narrows `try(:to_gid_param)` to
/// classes that no longer answer it.
#[test]
fn the_synthesized_method_list_still_matches_the_synthesizer() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("src/lower/broadcasts.rs"))
        .expect("src/lower/broadcasts.rs");
    assert!(
        src.contains("pub fn push_to_gid_param"),
        "`lower::try_guard::SYNTHESIZED_ON_EVERY_MODEL` names `to_gid_param` because \
         `lower::broadcasts::push_to_gid_param` puts it on every model. That function \
         is gone or renamed — the list is now a claim nothing backs."
    );
}
