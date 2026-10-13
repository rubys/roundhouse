//! `<params helper>[:field].humanize` (and the other String
//! inflections) in a controller action.
//!
//! The inflection grounding (`x.humanize` -> `ActiveSupport.humanize(x)`)
//! needs a String receiver. When it runs, `label_params[:note]` is still
//! an untyped `[]` read, so it declines; the typed-params rewrite turns
//! the read into `label_params.note` (a String) afterwards, and nothing
//! grounded the call again. The emitted `.note.humanize` raised
//! `undefined method 'humanize' for an instance of String` on every
//! target whose String has no ActiveSupport extensions (Spinel).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :note\n  end\nend\n";

/// The emitted `widgets_controller` (Ruby and RBS) for a `label` action
/// that writes `label_params[:note].<call>` to the record.
fn controller_for(target: BuildTarget, call: &str) -> String {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def label\n    @widget = Widget.find(params[:id])\n    @widget.update!(note: label_params[:note].{call})\n    render json: {{ note: @widget.note }}\n  end\n\n  private\n\n  def label_params\n    params.require(:label).permit(:note)\n  end\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  put \"widgets/:id/label\", to: \"widgets#label\"\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/widgets_controller.rb"), controller.into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    files
        .into_iter()
        .filter(|(p, _)| p.contains("widgets_controller"))
        .map(|(_, c)| c)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn an_inflection_on_a_permitted_field_is_grounded() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        for name in ["humanize", "titleize", "underscore"] {
            let src = controller_for(target, name);
            assert!(
                src.contains(&format!("ActiveSupport.{name}(self.label_params.note)")),
                "{target:?} {name}:\n{src}"
            );
            assert!(!src.contains(&format!(".note.{name}")), "{target:?} {name}:\n{src}");
        }
    }
}

/// An argument after the receiver is kept, and a call the grounding does
/// not cover is left as it was.
#[test]
fn arguments_are_kept_and_other_calls_are_left_alone() {
    let src = controller_for(BuildTarget::Spinel, "remove(\"_\")");
    assert!(src.contains("ActiveSupport.remove(self.label_params.note, \"_\")"), "{src}");
    let src = controller_for(BuildTarget::Spinel, "upcase");
    assert!(src.contains("self.label_params.note.upcase"), "{src}");
}
