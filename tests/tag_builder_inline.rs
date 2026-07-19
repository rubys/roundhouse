//! ActionView's dynamic tag builder `<%= tag.<element>(opts) do ...
//! inner... %>` inline-expands to open/walk/close HTML accumulation
//! (lobsters' stories/_form `tag.details`). Left unrecognized it fell to
//! the generic block fallback, which rebuilt `tag.<element> do ... end`
//! verbatim wrapped in html_escape — an unresolved `tag.details` becomes
//! an `sp_raise_nomethod` token under spinel AOT. A bare `tag` receiver
//! that is actually a local (a Tag model) must NOT be caught.

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
fn tag_builder_expands_to_open_walk_close() {
    let body = lower_view(vec![(
        "app/views/stories/_form.html.erb",
        "<%= tag.details class: \"boxline actions\", open: shown ? true : nil do %>\n<p>hi</p>\n<% end %>\n",
    )]);
    // Open tag with the element name and a `class=` attribute; the body
    // splices; the close tag follows. No raw `tag.details` survives.
    assert!(
        body.contains("<details"),
        "must emit the open <details tag:\n{body}"
    );
    assert!(
        body.contains("</details>"),
        "must emit the close </details> tag:\n{body}"
    );
    assert!(
        body.contains("class="),
        "class: opt must render as an HTML attribute:\n{body}"
    );
    // The `open:` boolean attribute renders bare (` open`) when truthy.
    assert!(
        body.contains(" open"),
        "open: boolean attribute must render bare when truthy:\n{body}"
    );
    // The verbatim builder call must be gone (no html_escape((tag...))).
    assert!(
        !body.contains("Symbol(\"details\")"),
        "the raw tag.details send must not survive:\n{body}"
    );
}

#[test]
fn tag_local_model_is_not_treated_as_the_builder() {
    // `tag` here is a block-local Tag model, not the ActionView builder.
    // `tag.hotness_mod` has no block and must pass through untouched.
    let body = lower_view(vec![(
        "app/views/tags/_row.html.erb",
        "<% tags.each do |tag| %><%= tag.hotness_mod %><% end %>\n",
    )]);
    assert!(
        body.contains("hotness_mod"),
        "a Tag-model method call must survive as an ordinary send:\n{body}"
    );
}
