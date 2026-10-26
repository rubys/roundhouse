//! A helper of a locked gem roundhouse does not model (pundit's
//! `authorize`, doorkeeper's `doorkeeper_authorize!`) reaches no method
//! on any target, so each call or filter naming it is refused where it is
//! written. What it feeds (`@articles` in the view) stays an
//! attributed note; an app's own method of the same name is not refused.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    doorkeeper (5.8.0)\n    pundit (2.5.2)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  doorkeeper\n  pundit\n";

const CONTROLLER: &str = "class ArticlesController < ApplicationController\n  \
    before_action :doorkeeper_authorize!\n\n  \
    def index\n    @articles = policy_scope(Article)\n  end\n\n  \
    def show\n    @article = Article.find(params[:id])\n    authorize @article\n  end\nend\n";

fn analyzed(application_controller: &str) -> (roundhouse::App, Vec<Diagnostic>) {
    let files = [
        ("Gemfile.lock", LOCK),
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: [:index, :show]\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", application_controller),
        ("app/controllers/articles_controller.rb", CONTROLLER),
        ("app/views/articles/index.html.erb", "<% @articles.each do |article| %><p><%= article.title %></p><% end %>\n"),
        ("app/views/articles/show.html.erb", "<p><%= @article.title %></p>\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let diags = diagnose(&app);
    (app, diags)
}

fn refused(diags: &[Diagnostic]) -> Vec<String> {
    let mut out: Vec<String> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .filter_map(|d| match &d.kind {
            DiagnosticKind::Unsupported { construct, detail, .. } => {
                Some(format!("{}: {}", construct.as_str(), detail.split('`').nth(1).unwrap_or("")))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out
}

#[test]
fn each_unshipped_gem_helper_is_refused_where_it_is_written() {
    let (_, diags) = analyzed("class ApplicationController < ActionController::Base\nend\n");
    assert_eq!(
        refused(&diags),
        ["doorkeeper: doorkeeper_authorize!", "pundit: authorize", "pundit: policy_scope"],
        "{diags:#?}"
    );
    assert!(
        !diags.iter().any(|d| d.code() == "undefined_filter_target"),
        "the filter's own error is the refusal, not a second one: {diags:#?}"
    );
}

#[test]
fn what_a_refused_helper_feeds_stays_a_note() {
    let (app, mut diags) = analyzed("class ApplicationController < ActionController::Base\nend\n");
    roundhouse::analyze::attribution::attribute_unknown_gems(&mut diags, &app);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).map(|d| d.code()).collect();
    assert_eq!(errors, ["unsupported"; 3], "{diags:#?}");
    assert!(
        diags.iter().any(|d| matches!(&d.kind, DiagnosticKind::IvarUnresolved { name } if name.as_str() == "@articles" || name.as_str() == "articles")
            && d.severity == Severity::Info),
        "{diags:#?}"
    );
}

#[test]
fn an_app_method_of_the_same_name_is_not_refused() {
    let (_, diags) = analyzed(
        "class ApplicationController < ActionController::Base\n  \
         def doorkeeper_authorize!\n    true\n  end\n\n  \
         def authorize(record)\n    record\n  end\n\n  \
         def policy_scope(scope)\n    scope.all\n  end\nend\n",
    );
    assert_eq!(refused(&diags), Vec::<String>::new(), "{diags:#?}");
}

#[test]
fn an_app_method_whose_answer_is_untyped_is_not_refused() {
    let (_, diags) = analyzed(
        "class ApplicationController < ActionController::Base\n  \
         def doorkeeper_authorize!\n    true\n  end\n\n  \
         def authorize(record)\n    unknown_helper(record)\n  end\n\n  \
         def policy_scope(scope)\n    unknown_helper(scope)\n  end\nend\n",
    );
    assert_eq!(refused(&diags), Vec::<String>::new(), "{diags:#?}");
}

#[test]
fn a_parameter_default_and_an_explicit_self_call_are_refused_where_written() {
    let (app, diags) = analyzed(
        "class ApplicationController < ActionController::Base\n  \
         private\n\n  \
         def scoped(rel = policy_scope(Article))\n    rel\n  end\n\n  \
         def check(record)\n    self.authorize(record)\n  end\nend\n",
    );
    let at = |line: &str| -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.render(&app.sources))
            .filter(|r| r.contains(&format!("application_controller.rb:{line}:")))
            .collect()
    };
    let default_site = at("4");
    assert!(default_site.len() == 1 && default_site[0].contains("`policy_scope` is a helper of the `pundit` gem"), "{default_site:#?}");
    let self_call = at("9");
    assert!(self_call.len() == 1 && self_call[0].contains("`authorize` is a helper of the `pundit` gem"), "the refusal replaces the dispatch error: {self_call:#?}");
}

#[test]
fn a_same_named_method_elsewhere_does_not_hide_the_refusal() {
    let (_, diags) = analyzed(
        "class ApplicationController < ActionController::Base\nend\n\n\
         class Gatekeeper\n  def authorize(record)\n    record\n  end\nend\n",
    );
    assert!(refused(&diags).contains(&"pundit: authorize".to_string()), "{:?}", refused(&diags));
}
