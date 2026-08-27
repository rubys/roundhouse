//! `render "messages/unrenderable"` in a HELPER body — Rails' shorthand
//! spelling of a partial render, in a module that has no `render`.
//!
//! The library-class partial lowering already claimed the long form
//! (`render partial: "x", locals: {…}`). campfire's
//! `MessagesHelper#message_tag` writes the short one, in the `rescue`
//! that every message row falls back to, and the call emitted bare — a
//! method a module does not have, which is a NoMethodError on CRuby and
//! an unresolved call that stops the spinel build.
//!
//! The second assertion is the reason the binding is not simply nil:
//! the partial's own signature types its record parameter (`(Message
//! message, …)`), so nil there is a seed contradiction — the strict
//! compiler refuses it rather than reinterpreting the value.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#;

fn emitted(helper: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/messages_controller.rb",
            "class MessagesController < ApplicationController\n  def index\n    \
             @messages = Message.all\n  end\nend\n",
        ),
        ("app/helpers/messages_helper.rb", helper),
        (
            "app/views/messages/index.html.erb",
            "<% @messages.each do |message| %><%= message_tag(message) %><% end %>\n",
        ),
        (
            "app/views/messages/_unrenderable.html.erb",
            "<div class=\"message--failed\">Failed to load message content</div>\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :messages\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("messages_helper.rb"))
        .map(|f| f.content.clone())
        .expect("messages_helper.rb")
}

const HELPER: &str = r#"module MessagesHelper
  def message_tag(message)
    tag.div message.body, id: dom_id(message)
  rescue Exception
    render "messages/unrenderable"
  end
end
"#;

#[test]
fn the_shorthand_render_becomes_the_partials_own_call() {
    let src = emitted(HELPER);
    assert!(
        src.contains("Views::Messages.unrenderable("),
        "the shorthand render must reach the lowered partial:\n{src}"
    );
    assert!(
        !src.contains("render \"messages/unrenderable\""),
        "no bare `render` may survive in a module body:\n{src}"
    );
}

/// The record argument is the enclosing method's own same-named
/// parameter, not nil: the partial's signature types it.
#[test]
fn the_record_argument_binds_to_the_method_parameter() {
    let src = emitted(HELPER);
    assert!(
        src.contains("Views::Messages.unrenderable(message"),
        "the record parameter binds to the in-scope `message`:\n{src}"
    );
}
