//! An RBI declares a gem's classes; it does not run them. A target that
//! loads no Ruby gems refuses an unmodeled gem's constants whether or not
//! an RBI declares them, while `check`, `ruby` and `jruby` keep the RBI's
//! types (#1217).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{self, BuildTarget};
use roundhouse::App;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    redcarpet (3.6.1)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  redcarpet (= 3.6.1)\n";
const RBI_PATH: &str = "sorbet/rbi/gems/redcarpet@3.6.1.rbi";
const RBI: &str = "\
# typed: true

module Redcarpet; end

class Redcarpet::Markdown
  sig { params(renderer: T.untyped, extensions: T.untyped).void }
  def initialize(renderer, extensions = T.unsafe(nil)); end

  sig { params(text: String).returns(String) }
  def render(text); end
end

module Redcarpet::Render; end

class Redcarpet::Render::HTML; end
";
const RENDERER: &str = "class Renderer\n  def self.html(text)\n    Redcarpet::Markdown.new(Redcarpet::Render::HTML).render(text)\n  end\nend\n";

fn app(files: &[(&str, &str)]) -> App {
    let tree: HashMap<PathBuf, Vec<u8>> = [("Gemfile.lock", LOCK)].iter().chain(files)
        .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest probe app");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn errors(app: &App) -> Vec<String> {
    diagnose(app).into_iter().filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.message).collect()
}

/// The target's `native_extension_gem` refusals, or `None` when it emits.
fn refusals(app: &App, target: BuildTarget) -> Option<Vec<String>> {
    let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        project::target_files(app, Path::new("."), target)
    });
    result.is_err().then(|| diagnostics.into_iter().filter(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "native_extension_gem"))
        .map(|diagnostic| diagnostic.message).collect())
}

fn emitted(app: &App, target: BuildTarget, name: &str) -> String {
    let files = project::target_files(app, Path::new("."), target)
        .unwrap_or_else(|error| panic!("{target:?} refused: {error}"));
    files.into_iter().find(|(path, _)| path == name).map(|(_, text)| text)
        .unwrap_or_else(|| panic!("{target:?} emitted no {name}"))
}

#[test]
fn without_an_rbi_the_constants_are_check_errors() {
    assert_eq!(errors(&app(&[("app/models/renderer.rb", RENDERER)])), [
        "constant not supported (all targets): Redcarpet::Markdown",
        "constant not supported (all targets): Redcarpet::Render::HTML",
    ]);
}

#[test]
fn an_rbi_types_the_constants_but_targets_without_gems_refuse_them() {
    let app = app(&[("app/models/renderer.rb", RENDERER), (RBI_PATH, RBI)]);
    assert_eq!(errors(&app), Vec::<String>::new());
    for target in [BuildTarget::Spinel, BuildTarget::Typescript] {
        let refused = refusals(&app, target).unwrap_or_else(|| panic!("{target:?} emitted Redcarpet"));
        assert_eq!(refused, [
            format!("native_extension_gem not supported ({}): Redcarpet::Markdown is provided by gem redcarpet; this target does not load native Ruby gems", target.as_str()),
            format!("native_extension_gem not supported ({}): Redcarpet::Render::HTML is provided by gem redcarpet; this target does not load native Ruby gems", target.as_str()),
        ], "{target:?}");
    }
    assert!(emitted(&app, BuildTarget::Ruby, "Gemfile").contains("gem \"redcarpet\", \"= 3.6.1\""));
    assert_eq!(refusals(&app, BuildTarget::Jruby), None);
}

#[test]
fn check_keeps_the_rbi_types() {
    let action = "class ReportController < ApplicationController\n  def show\n    @html = Redcarpet::Markdown.new(Redcarpet::Render::HTML).render(\"*hi*\").not_a_string_method\n  end\nend\n";
    let app = app(&[
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/report_controller.rb", action),
        ("config/routes.rb", "Rails.application.routes.draw { get '/report', to: 'report#show' }\n"),
        (RBI_PATH, RBI),
    ]);
    let diagnostics = diagnose(&app);
    assert!(diagnostics.iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::SendDispatchFailed { method, recv_ty }
            if method.as_str() == "not_a_string_method" && *recv_ty == roundhouse::Ty::Str)), "{diagnostics:#?}");
}

#[test]
fn an_app_that_defines_the_classes_is_not_refused() {
    let own = "module Redcarpet\n  module Render\n    class HTML\n    end\n  end\n\n  class Markdown\n    def initialize(renderer)\n      @renderer = renderer\n    end\n\n    def render(text)\n      text\n    end\n  end\nend\n";
    let app = app(&[("app/models/renderer.rb", RENDERER), ("app/models/redcarpet.rb", own), (RBI_PATH, RBI)]);
    assert_eq!(refusals(&app, BuildTarget::Spinel).unwrap_or_default(), Vec::<String>::new());
}

#[test]
fn an_rbi_that_reopens_a_class_outside_the_gem_does_not_claim_it() {
    let rbi = format!("{RBI}\nclass Digest::SHA256\n  sig {{ returns(String) }}\n  def self.gem_patch; end\nend\n");
    let source = "class Token\n  def self.value\n    Digest::SHA256.hexdigest(\"x\")\n  end\nend\n";
    let app = app(&[("app/models/token.rb", source), (RBI_PATH, &rbi)]);
    let refused = refusals(&app, BuildTarget::Spinel).unwrap_or_default();
    assert!(!refused.iter().any(|message| message.contains("Digest")), "{refused:#?}");
}
