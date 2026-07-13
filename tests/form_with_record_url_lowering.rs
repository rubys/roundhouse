//! `form_with url: <bare record>` resolves its action at COMPILE time
//! (`route_helperize`'s model-gated branch): Rails' `url_for(record)`
//! semantics — member path when persisted (record rides whole, so a
//! custom `to_param` shapes the segment), collection path when new,
//! POST either way. The typed replacement for the runtime `url_for`
//! fallback, whose `is_a?`-dispatch body is CRuby-overlay-only and
//! refused under spinel AOT (lobsters' _commentbox). Non-model
//! barewords keep the url_for fallback.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_view(files: Vec<(&str, &str)>) -> String {
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
fn record_url_resolves_to_persisted_ternary() {
    let body = lower_view(vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :comments do |t|\n    t.text :comment\n  end\nend\n",
        ),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\nend\n",
        ),
        (
            "app/views/comments/_commentbox.html.erb",
            "<%= form_with url: comment do |f| %>\n<% end %>\n",
        ),
    ]);
    assert!(
        body.contains("persisted?"),
        "record url must branch on persistence:\n{body}"
    );
    assert!(
        body.contains("comment_path"),
        "persisted arm must use the member path helper:\n{body}"
    );
    assert!(
        body.contains("comments_path"),
        "new-record arm must use the collection path helper:\n{body}"
    );
    assert!(
        !body.contains("url_for"),
        "the typed resolution must replace the runtime url_for:\n{body}"
    );
}

#[test]
fn non_model_bareword_keeps_url_for_fallback() {
    let body = lower_view(vec![(
        "app/views/searches/_box.html.erb",
        "<%= form_with url: destination do |f| %>\n<% end %>\n",
    )]);
    assert!(
        body.contains("url_for"),
        "a bareword that is no model must keep the runtime fallback:\n{body}"
    );
}
