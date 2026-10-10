//! Missing methods on an app class can originate in an unmodeled gem
//! mixed into its ancestors. Attribution is a coverage heuristic, not typing.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{
    Analyzer,
    attribution::{attribute_analysis_gaps, attribute_ingest_gaps, attribute_unknown_gems},
    diagnose,
};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::{IngestError, ingest_app_from_tree};
use roundhouse::span::{FileId, Span};
use roundhouse::ty::Ty;
use roundhouse::{App, ClassId, Symbol};

const ALBA_LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    alba (4.0.0)\n    alba-inertia (0.1.4)\n\nDEPENDENCIES\n  alba\n  alba-inertia\n";
const QUARTZ_LOCK: &str =
    "GEM\n  specs:\n    quartz_serializer (1.0.0)\n\nDEPENDENCIES\n  quartz_serializer\n";

fn app(resources: &str, lock: Option<&str>) -> App {
    app_with_consumer(resources, lock, "class ArticlesController < ApplicationController\n  def show\n    render json: {article: ArticleResource.new(Article.find(params[:id])).to_h}\n  end\nend\n")
}

fn app_with_consumer(resources: &str, lock: Option<&str>, consumer: &str) -> App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/articles_controller.rb", consumer),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("app/resources/resources.rb", resources),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    if let Some(lock) = lock {
        tree.insert(PathBuf::from("Gemfile.lock"), lock.as_bytes().to_vec());
    }
    ingest_app_from_tree(tree).expect("synthetic app ingests")
}

fn class(name: &str) -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from(name)),
        args: vec![].into(),
    }
}

fn failed_call(receiver: Ty, method: &str) -> Diagnostic {
    Diagnostic {
        span: Span {
            file: FileId(1),
            start: 3,
            end: 9,
        },
        kind: DiagnosticKind::SendDispatchFailed {
            method: Symbol::from(method),
            recv_ty: receiver,
        },
        severity: Severity::Error,
        message: format!("no known method `{method}`"),
    }
}

#[test]
fn inherited_alba_call_is_attributed_without_claiming_typed_support() {
    let mut app = app_with_consumer(
        "module SerializerBridge\n  include Alba::Resource\n  def local_helper; 1; end\nend\nclass ApplicationResource\n  include SerializerBridge\nend\nclass ArticleResource < ApplicationResource\nend\n",
        Some(ALBA_LOCK),
        "class ArticlesController < ApplicationController\n  def show\n    render json: {article: ArticleResource.new(Article.find(params[:id])).unmodeled_serializer_probe}\n  end\nend\n",
    );
    Analyzer::new(&app).analyze(&mut app);
    let before = app.clone();
    let raw = diagnose(&app);
    // A retained local bridge exercises transitive gem ancestry, while the
    // synthetic method remains unresolved by ordinary dispatch analysis.
    let index = raw.iter().position(|d| matches!(&d.kind,
        DiagnosticKind::SendDispatchFailed { method, recv_ty }
        if method.as_str() == "unmodeled_serializer_probe" && *recv_ty == class("ArticleResource")
    )).expect(&format!("unresolved source call: {raw:?}"));
    let mut attributed = raw.clone();
    let original = attributed[index].clone();
    assert_eq!(original.severity, Severity::Error);
    assert!(!original.span.is_synthetic());
    let source = &app.sources[original.span.file.0 as usize - 1];
    assert!(source.path.ends_with("articles_controller.rb"));
    assert!(source.text[original.span.start as usize..original.span.end as usize]
        .contains("unmodeled_serializer_probe"));
    attribute_unknown_gems(&mut attributed, &app);
    let note = &attributed[index];
    assert_eq!(note.severity, Severity::Info, "{}", note.message);
    assert_eq!(note.span, original.span);
    assert_eq!(note.kind, original.kind);
    assert!(note.message.contains("the `alba` gem"), "{}", note.message);
    assert!(note.message.contains("Alba::Resource"), "{}", note.message);
    assert!(
        note.message.contains("method ownership is unverified"),
        "{}",
        note.message
    );
    assert!(
        !note.message.contains("`alba-inertia` gem"),
        "{}",
        note.message
    );
    assert_eq!(app, before, "attribution must not change types or IR");
    assert_eq!(diagnose(&app), raw, "attribution leaves raw analysis unchanged");
    let once = attributed.clone();
    attribute_unknown_gems(&mut attributed, &app);
    assert_eq!(attributed, once, "attribution is idempotent");
}

#[test]
fn alba_admission_gaps_preserve_errors_and_require_a_resolved_direct_provider() {
    for (lock, attributed) in [
        (Some(ALBA_LOCK), true),
        (None, false),
        (Some("DEPENDENCIES\n  alba\n"), false),
        (Some("GEM\n  specs:\n    alba (4.0.0)\n\nDEPENDENCIES\n  unrelated\n"), false),
        (Some("GEM\n  specs:\n    alba-inertia (0.1.4)\n\nDEPENDENCIES\n  alba-inertia\n"), false),
    ] {
        let mut app = app(
            "class ApplicationResource\n  include Alba::Resource\nend\nclass ArticleResource < ApplicationResource\n  attributes :id, :title\nend\n",
            lock,
        );
        Analyzer::new(&app).analyze(&mut app);
        let before = app.clone();
        let raw = diagnose(&app);
        assert_eq!(raw.len(), 1, "{raw:#?}");
        assert!(matches!(&raw[0].kind, DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "alba_serialization"));
        let mut diags = raw.clone();
        // An unrelated unsupported construct and an unresolved call in the
        // same file must not be suppressed by an analysis-only ledger entry.
        let unrelated = Diagnostic::unsupported(raw[0].span, None, "other_construct", "unmodeled");
        let mut unresolved = failed_call(class("Plain"), "titel");
        unresolved.span = raw[0].span;
        diags.extend([unrelated.clone(), unresolved.clone()]);
        let mut gaps = Vec::new();
        attribute_ingest_gaps(&mut diags, &app, &gaps);
        attribute_analysis_gaps(&mut diags, &app, &mut gaps);
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].kind, raw[0].kind);
        assert_eq!(diags[0].span, raw[0].span);
        assert_eq!(diags[0].message.contains("the `alba` gem"), attributed);
        assert!(!diags[0].message.contains("the `alba-inertia` gem"));
        assert_eq!(diags[1], unrelated);
        assert_eq!(diags[2], unresolved);
        assert_eq!(gaps.len(), 1);
        let IngestError::Unsupported { file, message } = &gaps[0] else { panic!("not a coverage gap") };
        assert_eq!(file, "app/resources/resources.rb");
        assert!(message.starts_with("analysis: 4:1:"), "{message}");
        assert!(message.contains("alba_serialization"), "{message}");
        assert_eq!(message.contains("the `alba` gem"), attributed);
        let once = diags.clone();
        let report = roundhouse::ingest::survey::render_report(&gaps);
        assert_eq!(report.contains("the `alba` gem"), attributed, "{report}");
        attribute_analysis_gaps(&mut diags, &app, &mut gaps);
        assert_eq!(diags, once, "attribution must be idempotent");
        assert_eq!(roundhouse::ingest::survey::render_report(&gaps), report);
        assert_eq!(app, before, "ledger must not change types or IR");
        assert_eq!(diagnose(&app), raw, "raw analysis remains unsupported");
    }
}

#[test]
fn consumed_class_includes_use_original_literal_source_without_guessing() {
    for (body, expected) in [
        ("include QuartzSerializer::Resource", Severity::Info),
        ("module QuartzSerializer; module Resource; end; end; include ::QuartzSerializer::Resource", Severity::Info),
        ("module QuartzSerializer; module Resource; end; end; include QuartzSerializer::Resource", Severity::Error),
        ("include build_namespace::Resource", Severity::Error),
        ("include QuartzSerializer::Resource if enabled?", Severity::Error),
        ("configure do; include QuartzSerializer::Resource; end", Severity::Error),
        ("self.include QuartzSerializer::Resource", Severity::Error),
    ] {
        let mut app = app(&format!("class ArticleResource\n{body}\nend\n"), Some(QUARTZ_LOCK));
        // Simulate a modeled mixin being consumed; this test isolates the
        // original-source evidence. The actual Alba consumer above exercises
        // the real synthesis, without mutating its inferred IR.
        app.library_classes.iter_mut().find(|c| c.name == ClassId(Symbol::from("ArticleResource"))).unwrap().includes.clear();
        let before = app.clone();
        let mut diags = vec![failed_call(class("ArticleResource"), "encode_record")];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags[0].severity, expected, "{body}: {}", diags[0].message);
        assert_eq!(app, before);
    }
    // Restoring a precise source edge must not erase an ambiguous retained
    // IR edge, whose rootedness was lost. Both kinds of evidence stay visible.
    let app = app("class ArticleResource\n module QuartzSerializer; module Resource; end; end\n include ::QuartzSerializer::Resource\nend\n", Some(QUARTZ_LOCK));
    let mut diags = vec![failed_call(class("ArticleResource"), "encode_record")];
    attribute_unknown_gems(&mut diags, &app);
    assert_eq!(diags[0].severity, Severity::Error);
}

#[test]
fn local_mixins_and_inheritance_work_for_an_unlisted_gem_and_method() {
    let app = app(
        "module SerializerBridge\n  include QuartzSerializer::Resource\nend\nclass ApplicationResource\n  include SerializerBridge\nend\nclass ArticleResource < ApplicationResource\nend\n",
        Some(QUARTZ_LOCK),
    );
    // Without gem signatures even a typo could be supplied by that mixin.
    // The note must describe uncertainty, not assert the method exists.
    for method in ["encode_record", "misspelled_encode_record"] {
        let mut diags = vec![failed_call(class("ArticleResource"), method)];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags[0].severity, Severity::Info, "{}", diags[0].message);
        assert!(
            diags[0].message.contains("the `quartz_serializer` gem"),
            "{}",
            diags[0].message
        );
        assert!(diags[0].message.contains("method ownership is unverified"));
    }
}

#[test]
fn models_and_controllers_follow_their_recorded_parents_and_mixins() {
    let tree = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  include QuartzSerializer::Resource\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  include QuartzSerializer::Resource\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show; render plain: 'ok'; end\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("Gemfile.lock", QUARTZ_LOCK),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let app = ingest_app_from_tree(tree).expect("model/controller app ingests");
    assert!(app.models.iter().any(|m| m.name.0.as_str() == "Article"));
    assert!(
        app.controllers
            .iter()
            .any(|c| c.name.0.as_str() == "ArticlesController")
    );
    for receiver in ["Article", "ArticlesController"] {
        let mut diags = vec![failed_call(class(receiver), "encode_record")];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags[0].severity, Severity::Info, "{}", diags[0].message);
        assert!(diags[0].message.contains("the `quartz_serializer` gem"));
    }
}

#[test]
fn census_and_local_definitions_bound_attribution() {
    let resource = "class ArticleResource\n  include Alba::Resource\nend\n";
    for (source, lock) in [
        (resource, None),
        (resource, Some("DEPENDENCIES\n  alba\n")), // not resolved
        (
            resource,
            Some("GEM\n  specs:\n    alba (4.0.0)\n\nDEPENDENCIES\n  unrelated\n"),
        ), // transitive only
        (
            "class ArticleResource\n  include WillPaginate::Collection\nend\n",
            Some("GEM\n  specs:\n    will_paginate (4.0.1)\n\nDEPENDENCIES\n  will_paginate\n"),
        ), // modeled
        ("class ArticleResource\nend\n", Some(ALBA_LOCK)),
        (
            "module Alba::Resource\nend\nclass ArticleResource\n  include Alba::Resource\nend\n",
            Some(ALBA_LOCK),
        ),
        (
            "module Admin\n  module Alba::Resource\n  end\n  class Base\n    include Alba::Resource\n  end\nend\nclass ArticleResource < Admin::Base\nend\n",
            Some(ALBA_LOCK),
        ),
        (
            "class ArticleResource\n  module Alba\n    module Resource\n    end\n  end\n  include Alba::Resource\nend\n",
            Some(ALBA_LOCK),
        ),
    ] {
        let app = app(source, lock);
        let mut diags = vec![failed_call(class("ArticleResource"), "to_h")];
        let before = diags.clone();
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(diags, before, "must not guess: {source}\n{lock:?}");
    }
}

#[test]
fn module_supplements_preserve_reopens_roots_and_declarative_boundaries() {
    let base = "class ArticleResource\n  include SerializerBridge\nend\n";
    for (bridge, expected) in [
        (
            "module SerializerBridge\n  def helper; 1; end\nend\nmodule SerializerBridge\n  include QuartzSerializer::Resource\nend\n",
            Severity::Info,
        ),
        (
            "module SerializerBridge\n  include QuartzSerializer::Resource\nend\nmodule SerializerBridge\n  def helper; 1; end\nend\n",
            Severity::Info,
        ),
        (
            "module SerializerBridge\n  module QuartzSerializer\n    module Resource\n    end\n  end\n  include ::QuartzSerializer::Resource\nend\n",
            Severity::Info,
        ),
        (
            "module SerializerBridge\n  module QuartzSerializer\n    module Resource\n    end\n  end\n  include QuartzSerializer::Resource\nend\n",
            Severity::Error,
        ),
        // A retained module has a lossy IR edge as well as this rooted
        // source edge. Do not discard IR provenance merely because a precise
        // source edge exists: another declaration may have contributed it.
        (
            "module SerializerBridge\n  def helper; 1; end\n  module QuartzSerializer\n    module Resource\n    end\n  end\n  include ::QuartzSerializer::Resource\nend\n",
            Severity::Error,
        ),
        (
            "module SerializerBridge\n  include QuartzSerializer::Resource if enabled?\nend\n",
            Severity::Error,
        ),
        (
            "module SerializerBridge\n  configure do\n    include QuartzSerializer::Resource\n  end\nend\n",
            Severity::Error,
        ),
        (
            "module SerializerBridge\n  def configure\n    include QuartzSerializer::Resource\n  end\nend\n",
            Severity::Error,
        ),
        (
            "module SerializerBridge\n  include build_namespace::Resource\nend\n",
            Severity::Error,
        ),
    ] {
        let app = app(&format!("{base}{bridge}"), Some(QUARTZ_LOCK));
        let mut diags = vec![failed_call(class("ArticleResource"), "encode_record")];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(
            diags[0].severity, expected,
            "{bridge}: {}",
            diags[0].message
        );
    }
}

#[test]
fn ambiguous_gems_are_not_chosen_by_lockfile_or_include_order() {
    for gems in [
        ["acme-core", "acme-telemetry"],
        ["acme-telemetry", "acme-core"],
    ] {
        let lock = format!(
            "GEM\n  specs:\n    {} (1.0.0)\n    {} (1.0.0)\n\nDEPENDENCIES\n  {}\n  {}\n",
            gems[0], gems[1], gems[0], gems[1]
        );
        for includes in [
            "Acme::Resource",
            "AcmeCore::Resource, AcmeTelemetry::Resource",
            "AcmeTelemetry::Resource, AcmeCore::Resource",
        ] {
            let app = app(
                &format!("class ArticleResource\n  include {includes}\nend\n"),
                Some(&lock),
            );
            let mut diags = vec![failed_call(class("ArticleResource"), "encode_record")];
            attribute_unknown_gems(&mut diags, &app);
            assert_eq!(diags[0].severity, Severity::Error, "{}", diags[0].message);
        }
    }
}

#[test]
fn ambiguous_ancestry_cannot_fall_back_to_a_global_method_surface() {
    let app = app(
        "class ArticleResource\n  include Alba::Resource, QuartzSerializer::Resource\nend\n",
        Some(
            "GEM\n  specs:\n    alba (4.0.0)\n    quartz_serializer (1.0.0)\n    paper_trail (16.0.0)\n\nDEPENDENCIES\n  alba\n  quartz_serializer\n  paper_trail\n",
        ),
    );
    let mut diags = vec![failed_call(class("ArticleResource"), "versions")];
    let before = diags.clone();
    attribute_unknown_gems(&mut diags, &app);
    assert_eq!(
        diags, before,
        "PaperTrail's surface cannot resolve conflicting ancestry"
    );
}

#[test]
fn union_arms_must_agree_and_cycles_do_not_hide_reachable_evidence() {
    let app = app(
        "module LoopA\n  include LoopB\nend\nmodule LoopB\n  include LoopA, QuartzSerializer::Resource\nend\nclass ArticleResource\n  include LoopA\nend\nclass OtherResource\n  include QuartzSerializer::Resource\nend\nclass AlbaResource\n  include Alba::Resource\nend\nclass Plain\nend\n",
        Some(
            "GEM\n  specs:\n    alba (4.0.0)\n    quartz_serializer (1.0.0)\n\nDEPENDENCIES\n  alba\n  quartz_serializer\n",
        ),
    );
    for (arms, expected) in [
        (vec![class("ArticleResource")], Severity::Info),
        (
            vec![class("ArticleResource"), class("OtherResource"), Ty::Nil],
            Severity::Info,
        ),
        (
            vec![class("OtherResource"), Ty::Nil, class("ArticleResource")],
            Severity::Info,
        ),
        (
            vec![class("ArticleResource"), class("Plain")],
            Severity::Error,
        ),
        (
            vec![class("Plain"), class("ArticleResource")],
            Severity::Error,
        ),
        (
            vec![class("ArticleResource"), class("AlbaResource")],
            Severity::Error,
        ),
        (
            vec![class("AlbaResource"), class("ArticleResource")],
            Severity::Error,
        ),
        (vec![class("ArticleResource"), Ty::Str], Severity::Error),
    ] {
        let mut diags = vec![failed_call(
            Ty::Union {
                variants: arms.clone().into(),
            },
            "encode_record",
        )];
        attribute_unknown_gems(&mut diags, &app);
        assert_eq!(
            diags[0].severity, expected,
            "{arms:?}: {}",
            diags[0].message
        );
    }
}
