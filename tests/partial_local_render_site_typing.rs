//! Partial locals typed from their RENDER SITES, and the ActiveSupport
//! `exclude?` grounding that precise typing exposes.
//!
//! The view lowerer types a partial's declared locals by NAME (`user` →
//! `User`, `stories` → `Array[Story]`). That covers most Rails locals
//! and is wrong for none of them, but it cannot type a local whose name
//! isn't a model — lobsters' `new_message`, bound from a
//! `@new_message = Message.new`. Those emitted `untyped`, and on the
//! strict targets every read off them refused: spinel AOT stopped on
//! `if new_message.mod_note` with "unsupported condition (non-bool)",
//! even though `Message#mod_note` is a declared `attribute :mod_note,
//! :boolean` with a `bool` reader.
//!
//! The analyzer already knew the type — it harvests render-site local
//! types to seed each partial's body typing — so the fix is to record
//! that fact on the App and let the lowerer stamp it into the emitted
//! signature.

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::{apply_exclude_predicate_lowering, lower_view_to_library_class};
use roundhouse::App;

fn app_from(files: Vec<(&str, &str)>) -> App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

const SCHEMA: &str = r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
end
"#;

const MODEL: &str = "class Message < ApplicationRecord\n  attribute :mod_note, :boolean\nend\n";

const CONTROLLER: &str = r#"class MessagesController < ApplicationController
  def index
    @new_message = Message.new
  end
end
"#;

/// `index` renders the partial, passing the typed ivar as a local.
const INDEX: &str =
    "<%= render partial: 'form', locals: { new_message: @new_message, replying: false } %>\n";

/// Strict-locals partial: the header declares the interface exactly, so
/// the first declared local becomes the positional record param.
const PARTIAL: &str = r#"<%# locals: (new_message:, replying:) -%>
<%= new_message.body %>
"#;

fn partial_signature(partial_src: &str) -> String {
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/message.rb", MODEL),
        ("app/controllers/messages_controller.rb", CONTROLLER),
        ("app/views/messages/index.html.erb", INDEX),
        ("app/views/messages/_form.html.erb", partial_src),
    ]);
    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str().contains("_form"))
        .expect("partial ingested");
    let class = lower_view_to_library_class(view, &app);
    let m = class
        .methods
        .iter()
        .find(|m| m.name.as_str() == "form")
        .expect("form method lowered");
    format!("{:?}", m.signature)
}

#[test]
fn a_local_whose_name_is_not_a_model_types_from_the_render_site() {
    let sig = partial_signature(PARTIAL);
    // `new_message` names no model, so the naming convention yields
    // nothing; the render site passes `@new_message` (a `Message.new`).
    assert!(
        sig.contains("Message"),
        "expected `new_message` typed Message from the render site, got:\n{sig}"
    );
    assert!(
        !sig.contains("Untyped"),
        "no param should be left Untyped here, got:\n{sig}"
    );
}

#[test]
fn the_naming_convention_still_wins_where_it_resolves() {
    // A local named for a model keeps the convention type — the
    // render-site fact is consulted only where the convention yields
    // nothing, so one loosely-typed call site can't widen a param that
    // the convention types correctly today.
    let sig = partial_signature(
        "<%# locals: (message:, replying:) -%>\n<%= message.body %>\n",
    );
    assert!(sig.contains("Message"), "got:\n{sig}");
}

// ── exclude? grounding ───────────────────────────────────────────────
// Precise typing is what makes this reachable: on an untyped receiver
// the call passed through as dynamic dispatch, and it only refuses once
// the receiver has a static type.

fn body_after_exclude_lowering(model_src: &str) -> String {
    let mut app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/message.rb", model_src),
    ]);
    apply_exclude_predicate_lowering(&mut app);
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Message")
        .expect("Message ingested");
    format!("{:?}", model.body)
}

#[test]
fn exclude_predicate_grounds_to_negated_include() {
    let body = body_after_exclude_lowering(
        "class Message < ApplicationRecord\n  def hatless?(hats, hat)\n    hats.exclude?(hat)\n  end\nend\n",
    );
    assert!(!body.contains("exclude?"), "exclude? should be gone:\n{body}");
    assert!(body.contains("include?"), "expected include?:\n{body}");
    assert!(body.contains("\"!\""), "expected negation:\n{body}");
}

#[test]
fn an_app_defined_exclude_disables_the_pass() {
    // If the app means something else by the name, the rewrite would be
    // wrong — the pass stands down wholesale rather than guess.
    let body = body_after_exclude_lowering(
        "class Message < ApplicationRecord\n  def exclude?(x)\n    false\n  end\n\n  def hatless?(hats, hat)\n    hats.exclude?(hat)\n  end\nend\n",
    );
    assert!(
        body.contains("exclude?"),
        "app-defined exclude? must be left alone:\n{body}"
    );
}
