//! `form_with model: [:mod, tag]` — Rails' scope-prefix array form
//! (namespace symbol(s) + record). The record drives fields and
//! persistence; the symbols only prefix the route helper
//! (`mod_tag_path` / `mod_tags_path`). Before the scoped-record
//! recognizer, the WHOLE array flowed into plain-record handling —
//! `[:mod, tag].persisted?`, `[:mod, tag].id`, and (via the namespaced
//! view dir) a literal slash in the helper name (`RouteHelpers.mod/
//! tag_path`), which reads as division in the emitted Ruby. Found on
//! the lobsters mod forms (spinel-AOT lane, 2026-07-23).

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

fn tag_fixture(view_path: &str, view_src: &str) -> String {
    lower_view(vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :tags do |t|\n    t.string :tag\n  end\nend\n",
        ),
        ("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n"),
        (view_path, view_src),
    ])
}

#[test]
fn scoped_record_form_prefixes_the_helper_and_reads_the_record() {
    let body = tag_fixture(
        "app/views/mod/tags/_form.html.erb",
        "<%= form_with model: [:mod, tag] do |f| %>\n<% end %>\n",
    );
    assert!(
        body.contains("mod_tag_path") && body.contains("mod_tags_path"),
        "scope symbol must prefix member + collection helpers:\n{body}"
    );
    assert!(
        !body.contains("mod/"),
        "no slash may survive into helper names:\n{body}"
    );
    // persisted?/id must read the RECORD, not the array. The array
    // literal would debug-print as an Array node feeding the sends.
    assert!(
        body.contains("persisted?"),
        "action must branch on the record's persistence:\n{body}"
    );
    assert!(
        !body.contains("Array"),
        "the scope array must not survive as a receiver:\n{body}"
    );
}

#[test]
fn namespaced_view_dir_joins_helper_names_with_underscores() {
    // Plain-record form in a namespaced dir: resource_dir is
    // "mod/tags"; the helper must come out `mod_tag_path`, never the
    // slash-carrying `mod/tag_path`.
    let body = tag_fixture(
        "app/views/mod/tags/edit.html.erb",
        "<%= form_with model: tag do |f| %>\n<% end %>\n",
    );
    assert!(
        body.contains("mod_tag_path") && body.contains("mod_tags_path"),
        "namespaced dir must underscore-join helper names:\n{body}"
    );
    assert!(
        !body.contains("mod/"),
        "no slash may survive into helper names:\n{body}"
    );
}
