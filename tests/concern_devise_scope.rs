//! A `devise` declaration inside a concern's `included do` makes each
//! includer a Devise scope, exactly like a direct call in the model.
//!
//! Jumpstart Pro's `User::Authenticatable` declares
//! `devise(*[:database_authenticatable, …].compact)` in `included do`;
//! the registry only looked at the model's own body, so `current_user`
//! never typed and every `@account = current_user.accounts.find(…)`
//! controller ivar came out unresolved. The model's (or its included
//! concern's) declaration stays the only fact source.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::DiagnosticKind;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :email\n  end\n  \
    create_table :bookmarks do |t|\n    t.integer :user_id\n    t.string :title\n  end\nend\n";

const CONTROLLER: &str = "class BookmarksController < ApplicationController\n  \
    before_action :authenticate_user!\n  before_action :set_bookmark\n\n  \
    def show\n  end\n\n  private\n\n  def set_bookmark\n    \
    @bookmark = current_user.bookmarks.find(params[:id])\n  end\nend\n";

fn concern(devise: &str) -> String {
    format!(
        "module User::Authenticatable\n  extend ActiveSupport::Concern\n\n  included do\n    \
         {devise}\n    has_many :bookmarks\n  end\nend\n"
    )
}

fn diagnostics(user: &str, concern_src: &str) -> Vec<roundhouse::diagnostic::Diagnostic> {
    diagnostics_with(user, concern_src, &[])
}

fn diagnostics_with(user: &str, concern_src: &str, extra: &[(&str, &str)]) -> Vec<roundhouse::diagnostic::Diagnostic> {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :bookmarks, only: [:show]\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/bookmark.rb", "class Bookmark < ApplicationRecord\n  belongs_to :user\nend\n"),
        ("app/models/user.rb", user),
        ("app/models/user/authenticatable.rb", concern_src),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/bookmarks_controller.rb", CONTROLLER),
        ("app/views/bookmarks/show.html.erb", "<p><%= @bookmark.title %></p>\n"),
    ];
    files.extend_from_slice(extra);
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
}

/// Names of ivars that stayed unresolved, plus `current_user`'s type.
fn analyze(user: &str, concern_src: &str) -> (Vec<String>, String) {
    let diags = diagnostics(user, concern_src);
    let ivars = diags
        .iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::IvarUnresolved { name } => Some(name.as_str().to_string()),
            _ => None,
        })
        .collect();
    let filters = diags
        .iter()
        .filter(|d| d.code() == "undefined_filter_target")
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    (ivars, filters)
}

const USER: &str = "class User < ApplicationRecord\n  include Authenticatable\nend\n";

#[test]
fn devise_in_included_block_types_current_user() {
    let (ivars, filters) = analyze(
        USER,
        &concern("devise :database_authenticatable, :registerable"),
    );
    assert!(ivars.is_empty(), "unresolved ivars: {ivars:?}");
    assert!(filters.is_empty(), "{filters}");
}

#[test]
fn splat_array_devise_form_counts() {
    let (ivars, filters) = analyze(
        USER,
        &concern(
            "devise(*[:database_authenticatable, :registerable, \
             (:omniauthable if defined? OmniAuth)].compact)",
        ),
    );
    assert!(ivars.is_empty(), "unresolved ivars: {ivars:?}");
    assert!(filters.is_empty(), "{filters}");
}

/// The concern declares `devise`, but no model includes it: no scope.
#[test]
fn an_unincluded_concern_creates_no_helpers() {
    let (ivars, filters) = analyze(
        "class User < ApplicationRecord\nend\n",
        &concern("devise :database_authenticatable"),
    );
    assert_eq!(ivars, ["bookmark"], "current_user must stay untyped");
    assert!(filters.contains("authenticate_user!"), "{filters:?}");
}

fn devise_refusals(user: &str, concern_src: &str) -> Vec<String> {
    devise_refusals_with(user, concern_src, &[])
}

fn devise_refusals_with(user: &str, concern_src: &str, extra: &[(&str, &str)]) -> Vec<String> {
    diagnostics_with(user, concern_src, extra)
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "devise"))
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_devise_declaration_is_refused_once_where_it_is_written() {
    let direct = "class User < ApplicationRecord\n  devise :database_authenticatable\nend\n";
    let found = devise_refusals(direct, "module User::Authenticatable\nend\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`current_user`") && found[0].contains("no target ships Devise"), "{found:?}");

    let found = devise_refusals(USER, &concern("devise :database_authenticatable"));
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_concern_two_models_include_is_refused_once_naming_both() {
    let admin = "class Admin < ApplicationRecord\n  include User::Authenticatable\nend\n";
    let found = devise_refusals_with(USER, &concern("devise :database_authenticatable"), &[("app/models/admin.rb", admin)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`User`") && found[0].contains("`Admin`"), "{found:?}");
}
