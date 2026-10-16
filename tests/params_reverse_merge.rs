//! `<permit-chain>.reverse_merge(k: v)` as a params helper's return value.
//!
//! Ingest spells it `{ k: v }.merge(<permit-chain>)`, so the chain is the
//! ARGUMENT of `merge`. The helper was still typed `WidgetParams` (its
//! permit list is found inside), while the body rewrote to
//! `Hash#merge(WidgetParams)`: a `TypeError` at request time, and the
//! defaults never reached the record. It now lowers like the merge form,
//! with Rails' reverse_merge rule: a default for a permitted key applies
//! only when the request did not provide that key; a default for any
//! other key is always assigned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :name\n    t.string :kind\n    t.integer :status\n  end\nend\n";

/// The emitted `widgets_controller` (Ruby and RBS) for a `create` action
/// that hands `widget_params` to `Widget.create`.
fn controller_for(target: BuildTarget, helper_body: &str) -> String {
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def create\n    @widget = Widget.create(widget_params)\n    head :created\n  end\n\n  private\n\n  def widget_params\n    {helper_body}\n  end\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets, only: :create\nend\n".to_vec(),
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
fn a_default_outside_the_permit_list_is_always_assigned() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = controller_for(target, "params.require(:widget).permit(:name, :kind).reverse_merge(status: 0)");
        assert!(
            src.contains(r#"_p = WidgetParams.from_raw(Params.require_present(@params, "widget"))"#),
            "{target:?}:\n{src}"
        );
        assert!(src.contains("_p.status = \"0\"\n    _p.status_provided = true\n    _p\n"), "{target:?}:\n{src}");
        assert!(!src.contains(".merge("), "{target:?}:\n{src}");
    }
}

#[test]
fn a_default_inside_the_permit_list_yields_to_the_request() {
    let src = controller_for(
        BuildTarget::Spinel,
        "params.require(:widget).permit(:name, :kind).reverse_merge(kind: \"basic\", status: 0)",
    );
    assert!(
        src.contains("if !(_p.kind_provided)\n      _p.kind = \"basic\"\n      _p.kind_provided = true\n    end\n"),
        "{src}"
    );
    assert!(src.contains("    _p.status = \"0\"\n"), "{src}");
}

/// The helper's signature and its call site agree with the body: one
/// typed class, reached through the typed factory.
#[test]
fn the_helper_and_its_call_site_use_the_typed_class() {
    let src = controller_for(BuildTarget::Spinel, "params.require(:widget).permit(:name).reverse_merge(status: 0)");
    assert!(src.contains("def widget_params: () -> WidgetParams"), "{src}");
    assert!(src.contains("Widget.create_from_params(self.widget_params)"), "{src}");
}

#[test]
fn the_expect_form_keeps_its_refusal() {
    let src = controller_for(BuildTarget::Spinel, "params.expect(widget: [:name]).reverse_merge(status: 0)");
    assert!(
        src.contains(r#"_p = WidgetParams.from_raw(Params.expect_present(@params, "widget", ["name"], [], [], []))"#),
        "{src}"
    );
    assert!(src.contains("_p.status = \"0\""), "{src}");
}

/// `return <chain>.reverse_merge(...)`: the statements replace the
/// `return`, which moves onto the final `_p`.
#[test]
fn an_explicit_return_is_rewritten_too() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = controller_for(target, "return params.require(:widget).permit(:name).reverse_merge(status: 0)");
        assert!(
            src.contains(r#"_p = WidgetParams.from_raw(Params.require_present(@params, "widget"))"#),
            "{target:?}:\n{src}"
        );
        assert!(src.contains("_p.status = \"0\"\n    _p.status_provided = true\n    return _p\n"), "{target:?}:\n{src}");
        assert!(!src.contains(".merge("), "{target:?}:\n{src}");
    }
}
