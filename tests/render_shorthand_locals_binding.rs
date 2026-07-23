//! Shorthand-render locals — `render "message", message: m, is_unread: b`
//! (partial name as a string literal, locals as trailing kwargs). The
//! call-site lowering always parsed this form, but `render_locals_keys`
//! only matched the `partial:`-keyword form, so shorthand locals never
//! reached the partial's def signature and its body read them as
//! unbound frees (lobsters inbox partials — an AOT compile stop).

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

#[test]
fn shorthand_locals_reach_the_partial_signature_and_call_site() {
    let files = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :messages do |t|\n    t.text :body\n  end\nend\n",
        ),
        (
            "app/models/message.rb",
            "class Message < ApplicationRecord\nend\n",
        ),
        (
            "app/views/inbox/all.html.erb",
            "<% messages.each do |message| %>\n<%= render \"message\", message: message, is_unread: true %>\n<% end %>\n",
        ),
        (
            "app/views/inbox/_message.html.erb",
            "<span class=\"<%= 'unread' if is_unread %>\"><%= message.body %></span>\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");

    let partial = app
        .views
        .iter()
        .find(|v| v.name.as_str().contains("_message"))
        .expect("partial ingested");
    let lc = lower_view_to_library_class(partial, &app);
    let m = lc.methods.first().expect("partial method");
    let params: Vec<String> =
        m.params.iter().map(|p| p.name.as_str().to_string()).collect();
    assert!(
        params.iter().any(|p| p == "message") && params.iter().any(|p| p == "is_unread"),
        "shorthand locals must surface as partial params, got {params:?}"
    );

    let caller = app
        .views
        .iter()
        .find(|v| v.name.as_str().contains("all"))
        .expect("caller ingested");
    let lc = lower_view_to_library_class(caller, &app);
    let body = format!("{:?}", lc.methods.first().expect("caller method").body);
    assert!(
        body.contains("is_unread") || body.contains("true"),
        "call site must pass the shorthand locals:\n{body}"
    );
}
