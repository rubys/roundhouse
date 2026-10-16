//! `<params-helper>.merge!(k: v)` converts through `to_attrs`, as
//! `merge` does. It stayed on the typed `WidgetParams`, which defines no
//! `merge!` (`NoMethodError` at request time).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :name\n    t.integer :status\n  end\nend\n";

/// The emitted controller and params class for one action body.
fn emit(action: &str) -> String {
    let controller = format!(
        "class WidgetsController < ApplicationController\n{action}\n\n  private\n\n  def widget_params\n    params.require(:widget).permit(:name)\n  end\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets, only: %i[create update]\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/widgets_controller.rb"), controller.into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), BuildTarget::Spinel).expect("emit");
    files
        .into_iter()
        .filter(|(p, _)| p.contains("widgets_controller") || p.ends_with("widget_params.rb"))
        .map(|(_, c)| c)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn merge_bang_on_a_params_helper_converts_through_to_attrs() {
    let src = emit("  def create\n    @widget = Widget.create(widget_params.merge!(status: 1))\n    head :created\n  end");
    assert!(src.contains("self.widget_params.to_attrs.merge!("), "{src}");
    // The conversion demands the method on the params class.
    assert!(src.contains("def to_attrs\n"), "{src}");
}

#[test]
fn merge_bang_into_update_converts_the_same_way() {
    let src = emit(
        "  def update\n    @widget = Widget.find(params[:id])\n    @widget.update(widget_params.merge!(status: 1))\n    head :ok\n  end",
    );
    assert!(src.contains("@widget.update(self.widget_params.to_attrs.merge!(status: 1))"), "{src}");
}
