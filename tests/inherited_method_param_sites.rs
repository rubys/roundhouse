//! A receiverless call is keyed to the class it is written in, and the
//! `def` it reaches may sit on an ancestor. `inherited_param_owner`
//! resolves each call site to the class that defines the method, in
//! Ruby's lookup order, so a base class's method that only subclasses
//! call gets its parameter types.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"email\", null: false\n    t.string \"type\"\n  end\nend\n";
const APP_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\nend\n";
const USER: &str = "class User < ApplicationRecord\nend\n";
const APP_CTRL: &str = "class ApplicationController < ActionController::Base\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  post \"/api/auth\", to: \"api/auths#create\"\nend\n";

/// Every unresolved or gradual position in the app built from the
/// common files plus `files`.
fn unknown_positions(files: &[(&str, &str)]) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
        ("app/models/user.rb", USER),
        ("app/controllers/application_controller.rb", APP_CTRL),
        ("config/routes.rb", ROUTES),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    tree.extend(files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())));
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::UnresolvedType { .. }
                    | DiagnosticKind::GradualUntyped { .. }
                    | DiagnosticKind::SendDispatchFailed { .. }
            )
        })
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_parent_controllers_method_takes_its_type_from_subclass_call_sites() {
    let unknown = unknown_positions(&[
        (
            "app/controllers/api/base_controller.rb",
            "class Api::BaseController < ApplicationController\n  private\n\n  def sign_in_and_render(user)\n    render json: {email: user.email}\n  end\nend\n",
        ),
        (
            "app/controllers/api/auths_controller.rb",
            "class Api::AuthsController < Api::BaseController\n  def create\n    sign_in_and_render(User.find(params[:id]))\n  end\nend\n",
        ),
    ]);
    assert!(unknown.is_empty(), "{}", unknown.join("\n"));
}

/// The keywords of a subclass call bind to the ancestor's keyword
/// parameters by name; the kwargs Hash is not its first parameter.
#[test]
fn a_subclass_call_with_keywords_types_the_parents_keyword_parameters() {
    let unknown = unknown_positions(&[
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def greeting(to:, note:)\n    to.email + note.upcase\n  end\nend\n",
        ),
        (
            "app/models/admin.rb",
            "class Admin < User\n  def hello\n    greeting(to: self, note: \"hi\")\n  end\nend\n",
        ),
    ]);
    assert!(unknown.is_empty(), "{}", unknown.join("\n"));
}

/// The nested declaration style records the parent as written
/// (`BaseController`); the walk qualifies it against the child's
/// enclosing namespace.
#[test]
fn a_lexically_named_parent_is_found() {
    let unknown = unknown_positions(&[
        (
            "app/controllers/api/base_controller.rb",
            "module Api\n  class BaseController < ApplicationController\n    private\n\n    def sign_in_and_render(user)\n      render json: {email: user.email}\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/api/auths_controller.rb",
            "module Api\n  class AuthsController < BaseController\n    def create\n      sign_in_and_render(User.find(params[:id]))\n    end\n  end\nend\n",
        ),
    ]);
    assert!(
        !unknown.iter().any(|m| m.contains("`user`") || m.contains("`email`")),
        "{}",
        unknown.join("\n")
    );
}

/// Ruby looks in a class's included modules before its parent. A call
/// that reaches a module's method is not evidence for a same-named
/// method on the parent, whose parameter stays unresolved here.
#[test]
fn a_call_that_reaches_an_included_module_does_not_type_the_parent() {
    let unknown = unknown_positions(&[
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def describe(thing)\n    thing.email\n  end\nend\n",
        ),
        (
            "app/models/concerns/describable.rb",
            "module Describable\n  extend ActiveSupport::Concern\n\n  def describe(thing)\n    thing.email\n  end\nend\n",
        ),
        (
            "app/models/admin.rb",
            "class Admin < User\n  include Describable\n\n  def label\n    describe(self)\n  end\nend\n",
        ),
    ]);
    assert!(
        unknown.iter().any(|m| m.contains("`thing`")),
        "User#describe has no caller, so `thing` must stay unresolved:\n{}",
        unknown.join("\n")
    );
}
