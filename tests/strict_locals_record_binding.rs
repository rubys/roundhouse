//! Strict-locals record binding: the def side makes the header's FIRST
//! declared local the positional record (`view_to_library` strict
//! override), so the call site must bind that slot by the DECLARED
//! name, not the dir-convention singular. Surfaced by lobsters'
//! `messages/_form` — header `(new_message:, replying:)` vs convention
//! "message": the convention lookup missed, nil-filled the record, and
//! silently dropped the caller's `new_message` local.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit_messages_views() -> Vec<(String, String)> {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :messages do |t|\n    t.string :subject\n  end\nend\n",
        ),
        ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
        (
            "app/views/messages/index.html.erb",
            r#"<%= render :partial => "form", :locals => { :new_message => @new_message, :replying => false } %>
"#,
        ),
        (
            "app/views/messages/_form.html.erb",
            r#"<%# locals: (new_message:, replying: false) -%>
<p><%= new_message.subject %></p>
<% if replying %><span>replying</span><% end %>
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| (f.path.display().to_string(), f.content))
        .collect()
}

#[test]
fn record_binds_by_first_declared_local_not_convention() {
    let views = emit_messages_views();
    let find = |suffix: &str| {
        views
            .iter()
            .find(|(p, _)| p.ends_with(suffix))
            .map(|(_, c)| c.as_str())
            .unwrap_or_else(|| panic!("no emitted view ending in {suffix}: {views:?}"))
    };

    // Def side: first declared local is the positional record.
    let form = find("_form.rb");
    assert!(
        form.contains("def self.form(new_message, replying: false)"),
        "strict header must shape the partial signature:\n{form}"
    );

    // Call side: the record slot binds the DECLARED name from the
    // caller's locals hash — not nil via the missed "message" lookup.
    let index = find("index.rb");
    assert!(
        index.contains("form(new_message"),
        "caller must pass the declared record local positionally:\n{index}"
    );
    assert!(
        !index.contains("form(nil"),
        "record slot must not nil-fill when the declared local is provided:\n{index}"
    );
}
