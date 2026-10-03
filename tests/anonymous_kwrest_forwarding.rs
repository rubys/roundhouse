//! Anonymous keyword forwarding retains provenance, not a capturable local.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::Param;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

#[test]
fn anonymous_kwrest_param_and_forward_keep_the_native_contract() {
    let app = ingest_app_from_tree(tree(&[(
        "app/services/widget.rb",
        "class Widget\n  def build\n    other\n  end\n\n  def other(**opts)\n    opts\n  end\nend\n",
    )]))
    .expect("ingest");
    // Sanity: the fixture ingests at all (a canary the anonymous form
    // alone wouldn't need, but keeps this test from silently no-op'ing
    // if `Widget` stops resolving under a future refactor).
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Widget"));

    let app = ingest_app_from_tree(tree(&[(
        "app/services/anon_widget.rb",
        "class AnonWidget\n  def build(**)\n    other(**)\n  end\nend\n",
    )]))
    .expect("ingest anonymous forwarding");
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "AnonWidget")
        .expect("AnonWidget class");
    let build = class.methods.iter().find(|m| m.name.as_str() == "build").expect("build method");

    // The declaration has no binding; it must never capture a user's local.
    let kwrest = build
        .params
        .iter()
        .find(|p: &&Param| p.keyword && p.rest)
        .unwrap_or_else(|| panic!("expected anonymous kwrest, got {:?}", build.params));
    assert_eq!(kwrest.name.as_str(), "");
    assert!(!kwrest.from_kwrest);

    // The call packet is not a value read from a synthesized variable.
    match &*build.body.node {
        ExprNode::Send { args, .. } => {
            assert_eq!(args.len(), 1);
            assert!(matches!(&*args[0].node, ExprNode::ForwardKeywords));
        }
        other => panic!("expected a Send body, got {other:?}"),
    }
}

#[test]
fn mixed_anonymous_keywords_keep_the_existing_unsupported_diagnostic() {
    for call in ["target(factor: 11, **)", "target(**, factor: 11)", "target(**options, **)"] {
        let source = format!("class Probe; def call(options, **); {call}; end; end");
        let parsed = ruby_prism::parse(source.as_bytes());
        assert_eq!(parsed.errors().count(), 0, "legal Ruby control: {source}");
        let err = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
            .expect_err("mixed keyword forwarding remains unsupported");
        assert!(matches!(err, roundhouse::ingest::IngestError::Unsupported { message, .. }
            if message == "anonymous `**` keyword forwarding not yet supported"));
    }
}

#[test]
fn anonymous_keywords_and_runtime_guards_are_honest_target_boundaries() {
    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    use roundhouse::project::{BuildTarget, target_files};
    for (source, construct) in [
        ("class Probe; def self.call(**); target(**); end; def self.target(factor:); factor; end; end", "anonymous keyword forwarding"),
        ("class Probe; def self.call; defined?(MissingPr197); end; end", "runtime defined? query"),
        ("class Probe; def call; defined?(@@missing); end; end", "runtime defined? query"),
        ("class Probe; def call; @@count ||= 11; @@count; end; end", "class variable write"),
        ("class Probe; def call; @@count &&= 11; @@count; end; end", "class variable write"),
        ("class Probe; def call; @@count += 3; @@count; end; end", "class variable write"),
        ("class Probe; def self.call; @@count; end; end", "class variable read"),
        ("class Probe; @@count = nil; end", "class variable write"),
    ] {
        let mut app = ingest_app_from_tree(tree(&[("app/services/probe.rb", source)])).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        for target in BuildTarget::ALL.iter().copied().filter(|t| !matches!(t, BuildTarget::Blog)) {
            let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
                target_files(&app, roundhouse::fixtures::real_blog(), target)
            });
            let gates: Vec<_> = diags.iter().filter(|d| matches!(&d.kind,
                DiagnosticKind::Unsupported { construct: name, .. } if name.as_str() == construct)).collect();
            if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
                assert!(gates.is_empty(), "{target:?}: {diags:?}");
            } else {
                assert!(!gates.is_empty(), "{target:?}: {construct}: {diags:?}");
                assert!(gates.iter().all(|d| d.severity == Severity::Error && !d.span.is_synthetic()));
            }
        }
    }
}

#[test]
fn native_initializers_in_test_inner_classes_keep_the_target_boundary() {
    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    use roundhouse::project::{BuildTarget, target_files};
    let mut app = ingest_app_from_tree(tree(&[(
        "test/models/probe_test.rb",
        "class ProbeTest < ActiveSupport::TestCase\n  class Counter\n    @@count = nil\n  end\n  def test_probe\n    assert_equal 1, 1\n  end\nend\n",
    )])).unwrap();
    assert_eq!(app.test_modules[0].inner_classes[0].class_ivar_initializers.len(), 1);
    roundhouse::session::analyze_and_lower(&mut app);
    for target in BuildTarget::ALL.iter().copied().filter(|t| !matches!(t, BuildTarget::Blog)) {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, roundhouse::fixtures::real_blog(), target)
        });
        let gates: Vec<_> = diags.iter().filter(|d| matches!(&d.kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "class variable write")).collect();
        if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby) {
            assert!(gates.is_empty(), "{target:?}: {diags:?}");
        } else {
            assert!(!gates.is_empty(), "{target:?}: {diags:?}");
            assert!(gates.iter().all(|d| d.severity == Severity::Error && !d.span.is_synthetic()));
        }
    }
}
