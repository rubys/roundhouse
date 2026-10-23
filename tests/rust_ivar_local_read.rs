//! A local bound to an instance variable must not move the field out
//! of `self` in the Rust target.
//!
//! Ruby's `owner = @widget` copies a reference and leaves `@widget`
//! set. The Rust controller method takes `&mut self`, so a bare
//! `let owner = self.widget;` of a non-Copy model is E0507 ("cannot
//! move out of `self.widget` which is behind a mutable reference").
//! The association-build lowering binds the owner this way
//! (`rh_build_owner_N = @widget`), so every `@x = @widget.gadgets.build(...)`
//! hit it, including real-blog's comments controller.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_controller() -> String {
    let files: Vec<(&str, &str)> = vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table :widgets do |t|
    t.string :name
    t.integer :count
  end
  create_table :gadgets do |t|
    t.integer :widget_id
    t.string :label
  end
end
"#),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :gadgets\nend\n"),
        ("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  belongs_to :widget\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets do\n    resources :gadgets, only: [:create]\n  end\nend\n"),
        ("app/controllers/gadgets_controller.rb", r#"class GadgetsController < ActionController::API
  def create
    @widget = Widget.find(params[:widget_id])
    @count = 1
    copy = @widget
    tally = @count
    @gadget = @widget.gadgets.build(gadget_params)
    if @gadget.save && copy.id == @widget.id && tally == 1
      head :created
    else
      head :unprocessable_entity
    end
  end

  private

  def gadget_params
    params.require(:gadget).permit(:label)
  end
end
"#),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("controllers/gadgets_controller.rs"))
        .expect("emitted gadgets controller")
        .content
}

#[test]
fn local_bound_to_a_model_ivar_copies_instead_of_moving_out_of_self() {
    let src = emitted_controller();
    assert!(
        src.contains("let mut copy = self.widget.clone();") || src.contains("let copy = self.widget.clone();"),
        "a local bound to a model ivar must clone the field:\n{src}"
    );
    assert!(
        src.contains("rh_build_owner_0 = self.widget.clone();"),
        "the association build's owner temp must clone the field:\n{src}"
    );
    assert!(
        !src.contains("= self.widget;"),
        "no binding may move the model field out of self:\n{src}"
    );
}

#[test]
fn local_bound_to_a_copy_ivar_reads_it_directly() {
    let src = emitted_controller();
    assert!(
        src.contains("tally = self.count;"),
        "an Integer ivar is Copy and needs no clone:\n{src}"
    );
}
