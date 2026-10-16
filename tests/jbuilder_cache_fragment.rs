//! `json.cache! record do … end` around a whole jbuilder template.
//!
//! ```ruby
//! json.cache! widget do
//!   json.(widget, :id)
//!   json.name widget.name
//! end
//! ```
//!
//! Jbuilder's fragment cache: when the controller caches, the template's
//! JSON is read from (and on a miss written to) the store under
//! `jbuilder/` + the controller's combined key for the template and the
//! record, through `JbuilderTemplate`'s `_read_fragment_cache` /
//! `_write_fragment_cache` — which campfire's `BoundedJbuilderFragments`
//! prepends onto. It used to be read transparently (the block rendered
//! every time), which campfire's fragment test observed as no cache hits.
//! A `cache!` beside other pairs stays transparent.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(partial: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    render :show\n  end\nend\n",
        ),
        ("app/views/widgets/show.json.jbuilder", "json.partial! @widget\n"),
        ("app/views/widgets/_widget.json.jbuilder", Box::leak(partial.to_string().into_boxed_str())),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_jbuilder_views(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets/_widget_json.rb"))
        .map(|f| f.content)
        .expect("the partial is emitted")
}

#[test]
fn a_whole_template_cache_reads_and_writes_the_fragment() {
    let src = emitted("json.cache! widget do\n  json.(widget, :id)\n  json.name widget.name\nend\n");
    assert!(src.contains("if ActionView::ViewHelpers.jbuilder_caching?"), "{src}");
    assert!(
        src.contains("ActionView::ViewHelpers.jbuilder_cache_key(\"widgets/_widget\", widget)"),
        "{src}"
    );
    assert!(src.contains("ActionView::ViewHelpers.jbuilder_fragment_read(__jb_key0)"), "{src}");
    assert!(src.contains("ActionView::ViewHelpers.jbuilder_fragment_write(__jb_key0, __jb_io0)"), "{src}");
    // The uncached arm renders the same object straight into `io`.
    assert_eq!(src.matches("\"\\\"name\\\":\"").count(), 2, "{src}");
}

#[test]
fn a_cache_beside_other_pairs_stays_transparent() {
    let src = emitted("json.id widget.id\njson.cache! widget do\n  json.name widget.name\nend\n");
    assert!(!src.contains("jbuilder_fragment_read"), "{src}");
    assert!(!src.contains("cache!"), "{src}");
    assert!(src.contains("\"\\\"name\\\":\""), "{src}");
}
