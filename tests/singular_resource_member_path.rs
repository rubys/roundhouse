//! A SINGULAR Rails resource (`resource :account`) has no `:id` segment,
//! so its member route helper takes NO argument.
//!
//! Every record-in-URL position — `form_with model: @account`,
//! `link_to text, account`, an association-reader URL — built
//! `RouteHelpers.<singular>_path(record.id)` on the assumption that a
//! member helper has a segment to fill. Rails tolerates the extra
//! argument (its non-optimized `url_for` has nowhere to put it and
//! answers "/account"); a GENERATED function does not, and campfire's
//! account page died with `wrong number of arguments (given 1,
//! expected 0)` on the FORM TAG, three lowerings deep into a page whose
//! actual subject was pagination.
//!
//! The arity is read off the LOWERED route helpers — the same functions
//! the emit ships — rather than re-derived from the route table, so the
//! two cannot drift.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_view(files: Vec<(&str, &str)>, stem: &str) -> String {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str().contains(stem))
        .unwrap_or_else(|| panic!("view {stem} ingested"));
    let lc = lower_view_to_library_class(view, &app);
    format!("{:?}", lc.methods.first().expect("view method").body)
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :accounts do |t|
    t.string :name
  end
  create_table :articles do |t|
    t.string :title
  end
end
"#;

/// `resource :account` (singular) beside `resources :articles` (plural),
/// so one fixture proves both directions of the same rule.
const ROUTES: &str = r#"Rails.application.routes.draw do
  resource :account
  resources :articles
end
"#;

fn files(view_path: &'static str, view: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (view_path, view),
    ]
}

#[test]
fn a_singular_resource_form_action_takes_no_id() {
    let body = lower_view(
        files(
            "app/views/accounts/edit.html.erb",
            "<%= form_with model: @account do |f| %>\n<% end %>\n",
        ),
        "accounts/edit",
    );
    assert!(
        body.contains("account_path"),
        "the member helper still names the action:\n{body}"
    );
    assert!(
        !body.contains("\"id\""),
        "no `.id` argument — `account_path` takes none:\n{body}"
    );
}

/// The other half of the rule, and the reason it is a lookup rather
/// than a special case: a plural resource's member helper DOES take
/// the segment, and must keep getting it.
#[test]
fn a_plural_resource_form_action_still_takes_the_id() {
    let body = lower_view(
        files(
            "app/views/articles/edit.html.erb",
            "<%= form_with model: @article do |f| %>\n<% end %>\n",
        ),
        "articles/edit",
    );
    assert!(
        body.contains("article_path"),
        "the member helper names the action:\n{body}"
    );
    assert!(
        body.contains("\"id\""),
        "the member helper's `:id` segment is still filled:\n{body}"
    );
}

/// The bare-record URL position (`link_to text, account`) goes through
/// the same helper, so it answers the same way.
#[test]
fn a_bare_record_url_drops_the_id_for_a_singular_resource() {
    let body = lower_view(
        files(
            "app/views/accounts/show.html.erb",
            "<%= link_to \"Settings\", @account %>\n",
        ),
        "accounts/show",
    );
    assert!(
        body.contains("account_path"),
        "the record resolves through its member helper:\n{body}"
    );
    assert!(
        !body.contains("\"id\""),
        "no `.id` argument — `account_path` takes none:\n{body}"
    );
}
