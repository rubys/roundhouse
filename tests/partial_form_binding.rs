//! Cross-partial FormBuilder binding (`view_to_library::
//! partial_form_bindings`): a partial receiving a form builder as a
//! LOCAL (`render partial: "stories/form", locals: { story: @story,
//! f: f }` from inside `form_with model: @story do |f|`) re-derives
//! the binding at compile time. The form local drops out of the
//! partial's interface entirely (no FormBuilder object exists at
//! runtime — passing `f` was a NameError waiting to happen), `f.*`
//! calls inline exactly as in the defining template, `f.object` reads
//! substitute to the record local, `defined?(f)` folds to true, and
//! inference is TRANSITIVE (a bound partial forwarding its form local
//! binds the next). Surfaced by lobsters' stories/_form: every `f.*`
//! call fell to the escape path under spinel AOT.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_stories_views() -> Vec<(String, String)> {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :stories do |t|\n    t.string :title\n  end\nend\n",
        ),
        ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
        (
            "app/views/stories/new.html.erb",
            r#"<%= form_with model: @story do |f| %>
  <%= render :partial => "stories/form", :locals => { :story => @story, :f => f } %>
<% end %>
"#,
        ),
        (
            "app/views/stories/_form.html.erb",
            r#"<%= render :partial => "stories/form_errors", :locals => { :f => f, :story => f.object } %>
<%= f.label :title, "Title:" %>
<%= f.text_field :title %>
"#,
        ),
        (
            "app/views/stories/_form_errors.html.erb",
            r#"<% if defined?(f) %>
  <%= f.hidden_field :seen_previous %>
<% end %>
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    app.views
        .iter()
        .map(|v| {
            let lc = lower_view_to_library_class(v, &app);
            let m = lc.methods.first().expect("view method");
            (
                format!("{}({:?})", m.name.as_str(), m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()),
                format!("{:?}", m.body),
            )
        })
        .collect()
}

#[test]
fn form_local_binds_transitively_and_drops_from_interfaces() {
    let views = lower_stories_views();
    let find = |name: &str| {
        views
            .iter()
            .find(|(sig, _)| sig.starts_with(name))
            .unwrap_or_else(|| panic!("view {name} lowered: {views:?}"))
    };

    // The direct partial: no `f` param, builder calls inlined.
    let (sig, body) = find("form(");
    assert!(!sig.contains("\"f\""), "form local must drop from the partial interface: {sig}");
    assert!(
        body.contains("<label") && body.contains("story[title]"),
        "f.label / f.text_field must inline under the story binding:\n{body}"
    );

    // The forwarded partial: bound transitively, defined?(f) folded so
    // the guarded hidden field renders.
    let (sig, body) = find("form_errors(");
    assert!(!sig.contains("\"f\""), "forwarded form local must drop too: {sig}");
    assert!(
        body.contains("seen_previous") && body.contains("hidden"),
        "the defined?-guarded f.hidden_field must inline:\n{body}"
    );
    assert!(
        !body.contains("defined?"),
        "defined?(f) must fold to true in a bound partial:\n{body}"
    );
}

/// The SHORTHAND render — `render "bots/form", form: form, bot: @bot`
/// — is the same call to Rails as `render partial:, locals:`, and it
/// must derive the same binding. `render_locals_keys` (which builds the
/// partial's SIGNATURE) has always read both spellings; the binding
/// derivation read only the keyword one, so a shorthand-rendered
/// partial got its form local FILTERED OUT of the signature and kept
/// its `form.*` sends. In the emitted module a bare `form` is not a
/// local at all — it resolves to the module's OWN `form` function (the
/// partial is `_form.html.erb`), so campfire's bots page died on
/// `wrong number of arguments (given 0, expected 1..4)` rather than on
/// anything that named a form.
fn lower_bots_views() -> Vec<(String, String)> {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :bots do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("app/models/bot.rb", "class Bot < ApplicationRecord\nend\n"),
        (
            "app/views/bots/new.html.erb",
            r#"<%= form_with model: @bot do |form| %>
  <%= render "bots/form", form: form, bot: @bot %>
<% end %>
"#,
        ),
        (
            "app/views/bots/_form.html.erb",
            r#"<%= form.text_field :name, class: "input" %>
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    app.views
        .iter()
        .map(|v| {
            let lc = lower_view_to_library_class(v, &app);
            let m = lc.methods.first().expect("view method");
            (
                format!("{}({:?})", m.name.as_str(), m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()),
                format!("{:?}", m.body),
            )
        })
        .collect()
}

#[test]
fn shorthand_render_binds_the_form_local_too() {
    let views = lower_bots_views();
    let (sig, body) = views
        .iter()
        .find(|(sig, _)| sig.starts_with("form("))
        .unwrap_or_else(|| panic!("bots/_form lowered: {views:?}"));

    assert!(
        !sig.contains("\"form\""),
        "the form local must drop from a shorthand-rendered partial's interface: {sig}"
    );
    assert!(
        body.contains("bot[name]"),
        "form.text_field must inline under the bot binding:\n{body}"
    );
}
