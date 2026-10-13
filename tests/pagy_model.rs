//! The pagy gem (43.6.2) in analysis and lowering: what is modeled types
//! and resolves, what is not is refused by name, and nothing is modeled
//! unless `Gemfile.lock` carries the gem. The emitted program's behavior
//! is pinned by `tests/emit_and_run/pagy.rs`, against the real gem.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    pagy (43.6.2)\n      json\n      yaml\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  pagy (~> 43.0)\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :articles do |t|
    t.string :title
  end
end
"#;

const CONCERN: &str = "module Pagination
  extend ActiveSupport::Concern

  module BoundsCheck
    def pagy(collection, **vars)
      pagy, records = super
      raise ActiveRecord::RecordNotFound if pagy.page > pagy.last

      [pagy, records]
    end
  end

  included do
    include Pagy::Method
    prepend BoundsCheck
  end
end
";

const VIEW: &str = "<% if @pagy.last > 1 %>\n  <%== @pagy.series_nav %>\n<% end %>\n<%= @pagy.info_tag %>\n<p><%= @pagy.count %> / <%= @pagy.page %> / <%= @pagy.limit %> / <%= @pagy.next %></p>\n<% @articles.each do |article| %>\n  <%= article.title %>\n<% end %>\n";

fn tree(lock: Option<&str>, extra: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    let mut files: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        ("config/routes.rb".into(), "Rails.application.routes.draw do\n  resources :articles, only: :index\nend\n".into()),
        ("app/models/application_record.rb".into(), "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".into()),
        ("app/models/article.rb".into(), "class Article < ApplicationRecord\nend\n".into()),
        ("app/controllers/application_controller.rb".into(), "class ApplicationController < ActionController::Base\nend\n".into()),
        ("app/views/articles/index.html.erb".into(), VIEW.into()),
    ];
    if let Some(lock) = lock {
        files.push(("Gemfile.lock".into(), lock.into()));
    }
    for (path, content) in extra {
        files.retain(|(p, _)| p != path);
        files.push(((*path).into(), (*content).into()));
    }
    files.into_iter().map(|(p, c)| (p.into(), c.into_bytes())).collect()
}

fn articles_controller(index_body: &str) -> (&'static str, String) {
    (
        "app/controllers/articles_controller.rb",
        format!("class ArticlesController < ApplicationController\n  def index\n    {index_body}\n  end\nend\n"),
    )
}

/// Analysis and lowering error messages for a tree.
fn errors(files: HashMap<PathBuf, Vec<u8>>) -> Vec<String> {
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect()
}

fn with_controller(lock: Option<&str>, index_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let (path, src) = articles_controller(index_body);
    tree(lock, &[(path, src.as_str())])
}

#[test]
fn the_destructured_pair_is_typed_and_the_view_reads_resolve() {
    let errs = errors(with_controller(Some(LOCK), "@pagy, @articles = pagy(Article.order(:id), limit: 3)"));
    assert!(errs.is_empty(), "{errs:?}");
    // And with no options, and from a scope held in a local.
    let errs = errors(with_controller(Some(LOCK), "scope = Article.where.not(title: nil)\n    @pagy, @articles = pagy(scope)"));
    assert!(errs.is_empty(), "{errs:?}");
}

/// A scope method with a path the analysis cannot type (here a class
/// method nothing defines) is a union of a Relation and an open type: still
/// paginated, typed gradually, with the one unknown-method error the cause
/// deserves and no pagy refusal on top of it.
#[test]
fn a_scope_that_is_partly_untyped_is_not_a_second_error() {
    let (path, _) = articles_controller("");
    let src = "class ArticlesController < ApplicationController\n  def index\n    @pagy, @articles = pagy(visible(params[:q]))\n  end\n\n  private\n\n  def visible(q)\n    return Article.none if q.blank?\n    Article.no_such_scope(q)\n  end\nend\n";
    let errs = errors(tree(Some(LOCK), &[(path, src)]));
    assert!(!errs.iter().any(|e| e.contains("pagy")), "{errs:?}");
    assert!(!errs.iter().any(|e| e.contains("@articles has no known type") || e.contains("@pagy has no known type")), "{errs:?}");
}

#[test]
fn without_the_gem_in_the_lock_nothing_is_modeled() {
    let errs = errors(with_controller(None, "@pagy, @articles = pagy(Article.order(:id), limit: 3)"));
    assert!(
        errs.iter().any(|e| e.contains("@pagy has no known type")) && errs.iter().any(|e| e.contains("@articles has no known type")),
        "{errs:?}"
    );
}

#[test]
fn what_the_gem_offers_beyond_the_offset_paginator_is_refused() {
    let cases: [(&str, &str); 8] = [
        ("@pagy, @articles = pagy(:countless, Article.order(:id))", "an explicit paginator argument"),
        ("@pagy, @articles = pagy(:keyset, Article.order(:id))", "an explicit paginator argument"),
        ("@pagy, @articles = pagy(:offset, Article.order(:id))", "an explicit paginator argument"),
        ("@pagy, @articles = pagy(Article.order(:id), page: 2)", "option `page:` is not modeled"),
        ("@pagy, @articles = pagy(Article.order(:id), limit: 5, max_limit: 50)", "option `max_limit:` is not modeled"),
        ("n = params[:per].to_i\n    @pagy, @articles = pagy(Article.order(:id), limit: n)", "`limit:` must be a literal Integer of at least 1"),
        ("@pagy, @articles = pagy(Article.order(:id), limit: 0)", "`limit:` must be a literal Integer of at least 1"),
        ("@pagy, @articles = pagy([1, 2, 3])", "must be an ActiveRecord scope"),
    ];
    for (body, want) in cases {
        let errs = errors(with_controller(Some(LOCK), body));
        assert!(
            errs.iter().any(|e| e.contains("pagy") && e.contains(want)),
            "{body}: wanted an error mentioning {want:?}, got {errs:?}"
        );
    }
}

/// pagy 43.6.2 reads no `items:` option (it is `limit:` now), so
/// `pagy(scope, items: 50)` pages by 20. Modeled as the gem behaves, with a
/// warning: EngineeredAt's Madmin indexes write exactly this.
#[test]
fn items_is_ignored_as_the_gem_ignores_it_and_says_so() {
    let mut app = ingest_app_from_tree(with_controller(Some(LOCK), "@pagy, @articles = pagy(Article.order(:id), items: 50)"))
        .expect("ingest");
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    let all: Vec<_> = diagnose(&app).into_iter().chain(lower).collect();
    assert!(!all.iter().any(|d| d.severity == Severity::Error), "{all:?}");
    assert!(
        all.iter().any(|d| d.severity == Severity::Warning
            && d.message.contains("pagy(..., items:)")
            && d.message.contains("page size stays 20")),
        "{all:?}"
    );
}

#[test]
fn the_pagy_object_offers_what_the_runtime_models_and_nothing_else() {
    let call = "@pagy, @articles = pagy(Article.order(:id), limit: 3)";
    // Not on the object: refused as an unknown method on Pagy.
    for (view_line, method) in [
        ("<%= @pagy.page_url(:next) %>", "page_url"),
        ("<%= @pagy.in %>", "in"),
        ("<%= @pagy.options %>", "options"),
        ("<%== @pagy.series_nav_js %>", "series_nav_js"),
    ] {
        let view = format!("{VIEW}{view_line}\n");
        let (path, src) = articles_controller(call);
        let errs = errors(tree(Some(LOCK), &[(path, src.as_str()), ("app/views/articles/index.html.erb", view.as_str())]));
        assert!(
            errs.iter().any(|e| e.contains(&format!("no known method `{method}` on Pagy"))),
            "{method}: {errs:?}"
        );
    }
    // On the object but taking no arguments here: refused, not ignored.
    for view_line in ["<%== @pagy.series_nav(id: \"nav\") %>", "<%== @pagy.info_tag(item_name: \"posts\") %>"] {
        let view = format!("{VIEW}{view_line}\n");
        let (path, src) = articles_controller(call);
        let errs = errors(tree(Some(LOCK), &[(path, src.as_str()), ("app/views/articles/index.html.erb", view.as_str())]));
        assert!(
            errs.iter().any(|e| e.contains("pagy") && e.contains("takes no arguments here")),
            "{view_line}: {errs:?}"
        );
    }
}

#[test]
fn the_bounds_check_concern_is_admitted_and_wraps_the_gems_pagy() {
    let (path, src) = articles_controller("@pagy, @articles = pagy(Article.order(:id), limit: 3)");
    let files = tree(
        Some(LOCK),
        &[
            (path, src.as_str()),
            ("app/controllers/concerns/pagination.rb", CONCERN),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\n  include Pagination\nend\n",
            ),
        ],
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    let errs: Vec<String> = diagnose(&app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errs.is_empty(), "{errs:?}");
    // The wrapper reached the controller that includes the concern, with
    // its `super` to the gem's `pagy`.
    let application = app.controllers.iter().find(|c| c.name.0.as_str() == "ApplicationController").unwrap();
    let wrapper = application.actions().find(|a| a.name.as_str() == "pagy").expect("pagy spliced into ApplicationController");
    assert!(format!("{:?}", wrapper.body).contains("Super"), "{:?}", wrapper.body);
    // `include Pagy::Method` and `prepend BoundsCheck` were consumed; the
    // block is gone with them.
    let module = app.library_classes.iter().find(|c| c.name.0.as_str() == "Pagination").unwrap();
    assert!(module.unknown_calls.iter().all(|c| !format!("{c:?}").contains("BoundsCheck")), "{:?}", module.unknown_calls);
}

#[test]
fn a_wrapper_that_does_not_hand_back_the_gems_pair_is_refused() {
    let wrappers = [
        // never asks the gem
        "def pagy(collection, **vars)\n      [nil, collection]\n    end",
        // asks, then returns something else
        "def pagy(collection, **vars)\n      pagy, records = super\n      records\n    end",
        // rebinds either name before handing the pair back, however nested
        "def pagy(collection, **vars)\n      pagy, records = super\n      pagy = Pagy.new(0, 1, 1, \"/\", \"\") if collection.nil?\n      [pagy, records]\n    end",
        "def pagy(collection, **vars)\n      pagy, records = super\n      records ||= collection\n      [pagy, records]\n    end",
        "def pagy(collection, **vars)\n      pagy, records = super\n      if collection.nil?\n        records, other = [], 1\n      end\n      [pagy, records]\n    end",
        "def pagy(collection, **vars)\n      pagy, records = super\n      begin\n        raise ArgumentError\n      rescue ArgumentError => records\n        nil\n      end\n      [pagy, records]\n    end",
    ];
    for wrapper in wrappers {
        let concern = format!(
            "module Pagination\n  extend ActiveSupport::Concern\n\n  module BoundsCheck\n    {wrapper}\n  end\n\n  included do\n    prepend BoundsCheck\n  end\nend\n"
        );
        let files = tree(
            Some(LOCK),
            &[
                ("app/controllers/concerns/pagination.rb", concern.as_str()),
                (
                    "app/controllers/application_controller.rb",
                    "class ApplicationController < ActionController::Base\n  include Pagination\nend\n",
                ),
            ],
        );
        let err = ingest_app_from_tree(files).err().expect("refused");
        assert!(err.to_string().contains("overrides the pagy gem's `pagy`"), "{err}");
    }
}

#[test]
fn a_controller_that_defines_pagy_itself_cannot_also_take_the_wrapper() {
    let files = tree(
        Some(LOCK),
        &[
            ("app/controllers/concerns/pagination.rb", CONCERN),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\n  include Pagination\n\n  def pagy(collection, **vars)\n    pagy, records = super\n    [pagy, records]\n  end\nend\n",
            ),
        ],
    );
    let err = ingest_app_from_tree(files).err().expect("refused");
    assert!(err.to_string().contains("defines `pagy` itself"), "{err}");
}

#[test]
fn a_strict_target_ledgers_the_call_instead_of_emitting_it() {
    use roundhouse::project::{target_files, BuildTarget};
    let mut app = ingest_app_from_tree(with_controller(Some(LOCK), "@pagy, @articles = pagy(Article.order(:id), limit: 3)"))
        .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Python, BuildTarget::Typescript, BuildTarget::Go] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, std::path::Path::new("."), target));
        files.expect("target files");
        assert!(
            diags.iter().any(|d| d.severity == Severity::Error
                && matches!(&d.kind, roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "pagy")),
            "{target:?}: {diags:?}"
        );
    }
    // The ruby family and spinel carry the runtime and report nothing.
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, std::path::Path::new("."), target));
        files.expect("target files");
        assert!(
            !diags.iter().any(|d| matches!(&d.kind, roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "pagy")),
            "{target:?}: {diags:?}"
        );
    }
}

#[test]
fn the_runtime_rides_the_tree_only_when_the_lock_has_pagy() {
    use roundhouse::project::{target_files, BuildTarget};
    let has = |lock: Option<&str>| {
        let mut app = ingest_app_from_tree(with_controller(lock, "@articles = Article.order(:id)")).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        let files = target_files(&app, std::path::Path::new("."), BuildTarget::Spinel).expect("target files");
        let aggregator = files.iter().find(|(p, _)| p == "runtime/action_controller.rb").expect("aggregator").1.clone();
        let carries_file = files.iter().any(|(p, _)| p == "runtime/action_controller/pagy.rb");
        (carries_file, aggregator.contains("action_controller/pagy"))
    };
    assert_eq!(has(Some(LOCK)), (true, true));
    assert_eq!(has(None), (false, false));
}

#[test]
fn raw_output_tags_do_not_escape() {
    let compiled = roundhouse::erb::compile_erb("<%== @pagy.series_nav %>|<%= @pagy.info_tag %>|<%== yield %>|<%== render \"x\" %>");
    assert!(compiled.contains("raw((@pagy.series_nav))"), "{compiled}");
    assert!(!compiled.contains("raw((@pagy.info_tag))"), "{compiled}");
    // `yield` and `render` are never escaped by the view lowering, so the
    // raw tag leaves them as they were.
    assert!(!compiled.contains("raw((yield))") && !compiled.contains("raw((render"), "{compiled}");
}

/// The runtime slices with `offset` / `limit`, which a Ruby Array lacks, and
/// pagy 43 paginates an Array only through `pagy(:offset, array)`: a known
/// Array is refused at analysis, where a Relation chain is not.
#[test]
fn a_known_array_is_refused_where_a_relation_chain_is_not() {
    for body in ["@pagy, @articles = pagy(Article.all.to_a)", "@pagy, @articles = pagy([Article.first])"] {
        let errs = errors(with_controller(Some(LOCK), body));
        assert!(errs.iter().any(|e| e.contains("pagy not supported") && e.contains("Array")), "{body}: {errs:?}");
    }
    let errs = errors(with_controller(Some(LOCK), "@pagy, @articles = pagy(Article.where(title: \"x\").order(:id).limit(3))"));
    assert!(errs.is_empty(), "{errs:?}");
}

/// Only a complete bare `yield` / `render` is left unwrapped; anything
/// chained or operated on it is an expression the walker would escape, and
/// `<%==` never escapes.
#[test]
fn raw_output_wraps_everything_but_a_bare_yield_or_render() {
    for bare in ["yield", "yield :slot", "render \"x\"", "render(\"x\", a: 1)", "render partial: \"x\""] {
        let compiled = roundhouse::erb::compile_erb(&format!("<%== {bare} %>"));
        assert!(!compiled.contains("raw(("), "{bare}: {compiled}");
    }
    for chained in ["render(\"x\") + extra", "yield.upcase", "yield + \"!\"", "render(\"x\").strip", "render \"x\" if ok", "yield || render(\"y\")", "renderer", "yield_value"] {
        let compiled = roundhouse::erb::compile_erb(&format!("<%== {chained} %>"));
        assert!(compiled.contains(&format!("raw(({chained}))")), "{chained}: {compiled}");
    }
}

fn wrapper_is_refused(wrapper: &str) -> bool {
    let concern = format!(
        "module Pagination\n  extend ActiveSupport::Concern\n\n  module BoundsCheck\n    {wrapper}\n  end\n\n  included do\n    prepend BoundsCheck\n  end\nend\n"
    );
    let files = tree(
        Some(LOCK),
        &[
            ("app/controllers/concerns/pagination.rb", concern.as_str()),
            ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  include Pagination\nend\n"),
        ],
    );
    match ingest_app_from_tree(files) {
        Err(e) => e.to_string().contains("overrides the pagy gem's `pagy`"),
        Ok(_) => false,
    }
}

#[test]
fn a_wrapper_that_rebinds_or_returns_otherwise_in_any_form_is_refused() {
    let bodies = [
        ("case_in_bind", "case collection\n      in { title: pagy }\n        nil\n      else\n        nil\n      end"),
        ("rightward_bind", "collection => { title: records }"),
        ("predicate_bind", "collection in [pagy, _]"),
        ("splat_multi", "first, *records = [1, 2]"),
        ("nested_multi", "(a, pagy), b = [1, 2], 3"),
        ("opassign_plus", "records += []"),
        ("block_write", "[1].each { |x| pagy = x }"),
        ("while_write", "while true\n        records = nil\n        break\n      end"),
        ("rescue_mod_write", "(pagy = Integer(\"x\")) rescue nil"),
        ("early_return_other_pair", "return [nil, collection] if collection.nil?"),
        ("early_return_other_pair2", "return [pagy, []] if pagy.page > 1"),
    ];
    let mut leaked = vec![];
    for (name, body) in bodies {
        let w = format!("def pagy(collection, **vars)\n      pagy, records = super\n      {body}\n      [pagy, records]\n    end");
        if !wrapper_is_refused(&w) {
            leaked.push(name);
        }
    }
    assert!(leaked.is_empty(), "accepted (should refuse): {leaked:?}");
}

/// `for records in ...` writes the outer local; ingest has no `for` node at
/// all, so the wrapper never reaches the pass-through check: it is an ingest
/// error (a compile-time refusal), not an accepted override.
#[test]
fn a_for_loop_in_the_wrapper_is_an_ingest_refusal() {
    let wrapper = "def pagy(collection, **vars)\n      pagy, records = super\n      for records in [1]\n        nil\n      end\n      [pagy, records]\n    end";
    let concern = format!(
        "module Pagination\n  extend ActiveSupport::Concern\n\n  module BoundsCheck\n    {wrapper}\n  end\n\n  included do\n    prepend BoundsCheck\n  end\nend\n"
    );
    let files = tree(
        Some(LOCK),
        &[
            ("app/controllers/concerns/pagination.rb", concern.as_str()),
            ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  include Pagination\nend\n"),
        ],
    );
    let err = ingest_app_from_tree(files).err().expect("refused").to_string();
    assert!(err.contains("unsupported expression node"), "{err}");
}

#[test]
fn raw_output_wraps_bodies_with_anything_after_the_call() {
    // each must be wrapped in raw((...)): not a complete bare yield/render
    for body in [
        "yield; foo",
        "yield\nfoo",
        "yield # c\nfoo",
        "self.render \"x\"",
        "render(\"x\").to_s",
        "render \"x\" do\n 1\nend.upcase",
    ] {
        let compiled = roundhouse::erb::compile_erb(&format!("<%== {body} %>"));
        assert!(compiled.contains("raw(("), "{body:?}: {compiled}");
    }
}

/// A body that closes the probe's wrapper `def` early and opens another
/// is malformed ERB anyway, but must not be classified as a bare `yield`.
#[test]
fn raw_output_wraps_a_body_that_closes_the_probe_def_early() {
    let compiled = roundhouse::erb::compile_erb("<%== yield\nend\ndef x\nfoo %>");
    assert!(compiled.contains("raw(("), "{compiled}");
}

#[test]
fn a_return_of_exactly_the_pair_is_accepted_and_any_other_exit_refused() {
    let mk = |body: &str| format!("def pagy(collection, **vars)\n      pagy, records = super\n      {body}\n      [pagy, records]\n    end");
    let accepted = [
        "return [pagy, records] if collection.nil?",
        "return pagy, records if collection.nil?",
        "return ([pagy, records]) if collection.nil?",
        "raise ActiveRecord::RecordNotFound if pagy.page > pagy.last",
        "[1].each { |x| next if x }",
        "[1].each { |x| next x }",
        "while true\n        break\n      end",
    ];
    let refused = [
        "return [records, pagy] if collection.nil?",
        "return pagy, records, 1 if collection.nil?",
        "return *[pagy, records] if collection.nil?",
        "return if collection.nil?",
        "return pagy if collection.nil?",
        "return [pagy, records.to_a] if collection.nil?",
        "next [nil, nil] if collection.nil?",
        "next if collection.nil?",
        "x = -> { return [nil, nil] }",
        "begin\n        return [nil, nil]\n      ensure\n        nil\n      end",
        "pagy.page > 1 ? (return [nil, nil]) : nil",
        "collection.nil? && (return [nil, nil])",
    ];
    let bad_acc: Vec<_> = accepted.iter().filter(|b| wrapper_is_refused(&mk(b))).collect();
    let bad_ref: Vec<_> = refused.iter().filter(|b| !wrapper_is_refused(&mk(b))).collect();
    assert!(bad_acc.is_empty() && bad_ref.is_empty(), "wrongly refused: {bad_acc:?}\nwrongly accepted: {bad_ref:?}");
}
