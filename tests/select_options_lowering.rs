//! Compile-time select-option rendering (`form_builder::
//! emit_select_options`) — replaces the runtime `select_options_for`
//! seam (a CRuby-overlay `is_a?`-walk, the shape the typed runtime
//! refuses) with per-shape expansion. Byte-contract matches the
//! overlay: options concatenated, `<option[ selected="selected"]
//! value="V">TEXT</option>`, to_s selection compare. Surfaced by
//! lobsters' hat pickers refusing under spinel AOT.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_view(view_src: &str) -> String {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.integer :mailing_list_mode\n  end\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/views/settings/_form.html.erb", view_src),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let view = app.views.first().expect("view ingested");
    let lc = lower_view_to_library_class(view, &app);
    format!("{:?}", lc.methods.first().expect("view method").body)
}

#[test]
fn literal_pairs_expand_to_static_options_with_selected_ternary() {
    let body = lower_view(
        r#"<%= form_with model: user do |f| %>
  <%= f.select :mailing_list_mode, [ [ "No e-mails", 0 ], [ "Both", 1 ] ] %>
<% end %>
"#,
    );
    assert!(
        !body.contains("select_options_for"),
        "literal pairs must not use the runtime seam:\n{body}"
    );
    for expected in ["No e-mails", "Both", "selected=", "value=\\\"0\\\""] {
        assert!(
            body.contains(expected),
            "static option expansion missing {expected}:\n{body}"
        );
    }
}

#[test]
fn collection_form_expands_to_static_reader_loop() {
    let body = lower_view(
        r#"<%= form_with model: user do |f| %>
  <%= f.select "hat_id", options_from_collection_for_select(user.wearable_hats, "id", "hat", user.mailing_list_mode), :include_blank => true %>
<% end %>
"#,
    );
    assert!(
        !body.contains("select_options_for"),
        "collection form must not use the runtime seam:\n{body}"
    );
    for expected in ["_options_hat_id", "each", "label="] {
        assert!(
            body.contains(expected),
            "collection loop missing {expected}:\n{body}"
        );
    }
    assert!(
        !body.contains("include_blank"),
        "include_blank is behavior, not an attribute:\n{body}"
    );
}
