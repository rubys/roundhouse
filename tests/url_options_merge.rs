//! `link_to text, {controller:, action:, page:}.merge(extra)`.
//!
//! Current lobsters carries its last-read marker across pages by merging
//! `@next_page_params || {}` into the url-options hash. The literal keys
//! resolve through the generated `path_for_controller_action_page`
//! lookup, as before; the merged keys are runtime, and Rails renders a
//! key that is no route segment as the query string, so they go through
//! `RouteHelpers.query_suffix`. Before, the merge hid the literal and
//! the whole hash reached `link_to` — a NoMethodError at render.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn merged_url_options_resolve_with_a_query_suffix() {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/controllers/notes_controller.rb", "class NotesController < ApplicationController\n  def index\n    @page = 1\n    @extra = {marker: 5}\n  end\nend\n"),
        ("app/views/notes/index.html.erb", "<%= link_to \"next\", { controller: controller_name, action: action_name, page: @page + 1 }.merge(@extra || {}), rel: \"next\" %>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/notes\" => \"notes#index\"\n  get \"/notes/page/:page\" => \"notes#index\"\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let views = ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(views.contains("RouteHelpers.path_for_controller_action_page("), "{views}");
    assert!(views.contains("+ RouteHelpers.query_suffix("), "{views}");
    let helpers = roundhouse::lower::lower_routes_to_library_functions(&app);
    assert!(
        helpers.iter().any(|f| f.name.as_str() == "query_suffix"),
        "the helper is generated: {:?}",
        helpers.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
    );
}
