//! Dynamic-partial pool dispatch × strict-locals: a controller assigns
//! the options form `@above = {partial: "single_tag", locals: {tag:
//! @tag, related: @related}}` and the view renders `<%= render @above
//! %>`. The pool entry keeps the `locals:` sub-hash, the ivar values
//! fold into the rendering view's closure, and the dispatch arm binds
//! them onto the strict-locals target's declared interface — record
//! (first declared local) positionally, the rest as keywords. Surfaced
//! by lobsters' /t/:tag pages: `_single_tag` declares `(tag:,
//! related:)` (required keywords), so the previous nil-record,
//! no-kwargs arm raised ArgumentError at request time.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit_home() -> (Vec<(String, String)>, Vec<(String, String)>) {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :tags do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n"),
        (
            "app/controllers/home_controller.rb",
            r#"class HomeController < ApplicationController
  def index
    @tag = Tag.first
    @related = Tag.all
    @above = { partial: "single_tag", locals: { tag: @tag, related: @related } }
  end
end
"#,
        ),
        (
            "app/views/home/index.html.erb",
            r#"<%= render @above %>
"#,
        ),
        (
            "app/views/home/_single_tag.html.erb",
            r#"<%# locals: (tag:, related:) -%>
<span><%= tag.name %></span>
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let views = ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| (f.path.display().to_string(), f.content))
        .collect();
    let controllers = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .map(|f| (f.path.display().to_string(), f.content))
        .collect();
    (views, controllers)
}

#[test]
fn pool_arm_binds_strict_locals_from_options_hash() {
    let (views, controllers) = emit_home();
    let find = |files: &[(String, String)], suffix: &str| -> String {
        files
            .iter()
            .find(|(p, _)| p.ends_with(suffix))
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("no emitted file ending in {suffix}: {files:?}"))
    };

    // Def side: strict header shapes the pooled partial's signature.
    let single_tag = find(&views, "_single_tag.rb");
    assert!(
        single_tag.contains("def self.single_tag(tag, related:)"),
        "strict header must shape the pooled partial signature:\n{single_tag}"
    );

    // Dispatch arm: record bound from the entry's locals by declared
    // name, remaining declared locals as keywords — not nil/absent.
    let index = find(&views, "index.rb");
    assert!(
        index.contains("when \"single_tag\""),
        "pool dispatch must case over the assigned name:\n{index}"
    );
    assert!(
        index.contains("Views::Home.single_tag(tag, related: related)"),
        "arm must bind the entry's locals onto the strict interface:\n{index}"
    );
    assert!(
        !index.contains("single_tag(nil"),
        "record slot must not nil-fill when the entry provides it:\n{index}"
    );

    // The locals ivars fold into the view closure, so the controller
    // threads them to the view call.
    let home_controller = find(&controllers, "home_controller.rb");
    assert!(
        home_controller.contains("@related") && home_controller.contains("@tag"),
        "controller must thread the pool-locals ivars into the view call:\n{home_controller}"
    );
}
