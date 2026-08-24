//! A constant declared in a HELPER MODULE or a CONCERN is typed by its
//! value, not by its name.
//!
//! `build_constant_registry` walked models and controllers, whose
//! constants arrive as `Unknown` body items. A library class carries
//! its constants on a FIELD instead, and had no arm — so every constant
//! a concern or helper module declares was invisible to the registry,
//! and a read of one fell to the `Ty::Class { id: ConstName }` fallback:
//! a class named after the constant.
//!
//! That is not a rare corner. campfire keeps `REACTIONS` in
//! `EmojiHelper` and `CONNECTION_TTL` in `Membership::Connectable`, and
//! constants like them produced five of its emit's type errors —
//! `EmojiHelper::REACTIONS.each` asked for `each` on a class, and
//! `count > PAGE_SIZE` compared an Int to one.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :messages do |t|\n    t.string :body\n  end\nend\n";

const HELPER: &str = r#"
module EmojiHelper
  REACTIONS = { "up" => "Thumbs up", "wave" => "Waving hand" }
end
"#;

// The read is a QUALIFIED path from a view — campfire's own shape, and
// the one with no lexical scope to fall back on.
const VIEW: &str = "<% EmojiHelper::REACTIONS.each do |character, title| %>\n\
    <span title=\"<%= title %>\"><%= character %></span>\n\
    <% end %>\n";

const CONTROLLER: &str = r#"
class MessagesController < ApplicationController
  def index
    @messages = Message.all
  end
end
"#;

fn diagnostics() -> Vec<String> {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :messages\nend\n",
        ),
        ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
        ("app/helpers/emoji_helper.rb", HELPER),
        ("app/controllers/messages_controller.rb", CONTROLLER),
        ("app/views/messages/index.html.erb", VIEW),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app).into_iter().map(|d| d.to_string()).collect()
}

#[test]
fn a_helper_module_constant_is_not_read_as_a_class() {
    let ds = diagnostics();
    // The exact failure: `each` dispatched against a class named
    // REACTIONS, because the Hash the constant holds never reached the
    // registry.
    let offenders: Vec<&String> = ds
        .iter()
        .filter(|d| d.starts_with("error") && d.contains("REACTIONS"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a helper module's constant must type as its value: {offenders:?}"
    );
}
