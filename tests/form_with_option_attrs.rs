//! form_with's steering options (`method:`, `scope:`) must NOT leak into
//! the open `<form>` tag as HTML attributes. Left in `opts_entries` they
//! rendered `method="#{html_escape(:post)}"` / `scope="#{html_escape(
//! :keybase_proof)}"` — a Symbol into `html_escape`'s String param, which
//! crashes under spinel AOT (`sp_sym` → `const char *`). `method:` is
//! consumed as the form verb (feeds the `_method` override); `scope:` is a
//! field-name prefix option that we drop (fields name bare either way).
//! Regression cover for the two lobsters #2457 sym-attr sites
//! (users/show `form_with model:, method: :post`; keybase
//! `form_with scope: :keybase_proof, url: ...`).

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
fn explicit_method_does_not_leak_as_a_form_attribute() {
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
            "app/views/comments/_form.html.erb",
            "<%= form_with model: comment, method: :post do |f| %>\n<% end %>\n",
        ),
    ]);
    // The open tag carries the builder's single hard-coded `method="post"`
    // (folded into the accept-charset text) and no second attribute.
    assert_eq!(
        body.matches("method=").count(),
        1,
        "explicit `method:` must not render a second `method=` attribute:\n{body}"
    );
    // An explicit `method: :post` wins over the persisted?-derived default
    // — no PATCH ternary feeds the `_method` override.
    assert!(
        !body.contains("patch"),
        "explicit `method: :post` must override the persisted? PATCH default:\n{body}"
    );
}

#[test]
fn scope_option_does_not_leak_as_a_form_attribute() {
    let body = lower_view(vec![(
        "app/views/keybase_proofs/new.html.erb",
        "<%= form_with scope: :keybase_proof, url: keybase_proofs_path do |f| %>\n<% end %>\n",
    )]);
    assert!(
        !body.contains("scope="),
        "`scope:` must not render as a `scope=\"...\"` HTML attribute:\n{body}"
    );
    // And its Symbol value must never reach html_escape (the `:keybase_proof`
    // Sym literal is dropped, not spliced as an attribute value). The route
    // helper `keybase_proofs_path` shares the stem, so match the Sym node.
    assert!(
        !body.contains("Sym { value: Symbol(\"keybase_proof\")"),
        "the `scope:` Symbol value must be dropped, not html_escaped:\n{body}"
    );
}
