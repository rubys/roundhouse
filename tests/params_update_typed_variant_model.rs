//! `record.update!(<x>_params)` when no model is named after `<x>`
//! (#856).
//!
//! The call site was renamed to the typed variant
//! `update_from_<x>_params!` whenever the helper's permit list was
//! recognized, but the model lowering defines that variant only on the
//! model the resource names. A record wrapped under another key
//! (`params.require(:gadget)` updating a `Widget`) got a call to a
//! method nothing defines: `NoMethodError` where Rails runs the update.
//! Now the rename needs a model to define the variant; otherwise the
//! call stays Rails' `update!`, handed the permitted attributes as a
//! Hash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :name\n    t.string :color\n  end\nend\n";

/// Every emitted file whose path contains `needle`, for an `update`
/// action that hands `<key>_params` (permitting `:name`, `:color`) to
/// `@widget.update!`.
fn emitted(target: BuildTarget, key: &str, needle: &str) -> String {
    let helper = format!("params.require(:{key}).permit(:name, :color)");
    emitted_with(target, key, &helper, &[], needle)
}

/// As [`emitted`], with the helper's body spelled out and more
/// controllers in the app.
fn emitted_with(target: BuildTarget, key: &str, helper: &str, extra: &[(&str, &str)], needle: &str) -> String {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def update\n    @widget = Widget.find(params[:id])\n    @widget.update!({key}_params)\n    head :no_content\n  end\n\n  private\n\n  def {key}_params\n    {helper}\n  end\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, source) in extra {
        tree.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets, only: :update\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/widgets_controller.rb"), controller.into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    files
        .into_iter()
        .filter(|(p, _)| p.contains(needle))
        .map(|(p, c)| format!("# {p}\n{c}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_resource_with_no_model_keeps_rails_update() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = emitted(target, "gadget", "widgets_controller");
        assert!(!src.contains("update_from_gadget_params"), "{target:?}:\n{src}");
        assert!(src.contains("@widget.update!(self.gadget_params.to_attrs)"), "{target:?}:\n{src}");
        let params = emitted(target, "gadget", "gadget_params");
        assert!(params.contains("def to_attrs"), "{target:?}: the params class answers the Hash:\n{params}");
    }
}

/// The model the resource names still gets its typed variant.
#[test]
fn the_models_own_resource_is_still_renamed() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = emitted(target, "widget", "widgets_controller");
        assert!(src.contains("@widget.update_from_widget_params!(self.widget_params)"), "{target:?}:\n{src}");
        let model = emitted(target, "widget", "models/widget.rb");
        assert!(model.contains("def update_from_widget_params!"), "{target:?}:\n{model}");
    }
}

/// A helper that answers a Hash built from the params, not the params
/// object: `update!` already gets what Rails' update takes, so the call
/// is left as written (no `to_attrs` on a Hash).
#[test]
fn a_helper_answering_a_hash_is_left_as_written() {
    for helper in [
        "params.require(:gadget).permit(:name, :color).to_h.except(:color)",
        "permitted = params.require(:gadget).permit(:name, :color)\n    permitted.to_h.merge(name: permitted[:name].to_s.strip)",
    ] {
        for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
            let src = emitted_with(target, "gadget", helper, &[], "widgets_controller");
            assert!(src.contains("@widget.update!(self.gadget_params)\n"), "{target:?}:\n{src}");
            assert!(!src.contains("to_attrs"), "{target:?}:\n{src}");
        }
    }
}

/// Another controller (named after the resource, so its list is the
/// canonical `GadgetParams`) permits the same key under the same helper
/// name with a different list, so this controller's list is the
/// controller-qualified class. The call site reads `to_attrs` of the
/// class it holds, and that class is the one that defines it.
#[test]
fn the_class_at_the_call_site_answers_to_attrs() {
    let other = "class Zone::GadgetsController < ApplicationController\n  def create\n    head(gadget_params.to_h.empty? ? :unprocessable_entity : :created)\n  end\n\n  private\n\n  def gadget_params\n    params.require(:gadget).permit(:name)\n  end\nend\n";
    let extra = [("app/controllers/zone/gadgets_controller.rb", other)];
    let helper = "params.require(:gadget).permit(:name, :color)";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = emitted_with(target, "gadget", helper, &extra, "widgets_controller");
        assert!(src.contains("@widget.update!(self.gadget_params.to_attrs)"), "{target:?}:\n{src}");
        let class = src
            .lines()
            .find_map(|l| l.trim().strip_prefix("def gadget_params: () -> "))
            .unwrap_or_else(|| panic!("{target:?}: no helper signature:\n{src}"))
            .to_string();
        assert_ne!(class, "GadgetParams", "{target:?}: this list is the qualified class");
        let file = format!("{}.rb", class.chars().fold(String::new(), |mut out, c| {
            if c.is_uppercase() && !out.is_empty() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            out
        }));
        let params = emitted_with(target, "gadget", helper, &extra, &file);
        assert!(params.contains("def to_attrs"), "{target:?}: {file}:\n{params}");
    }
}
