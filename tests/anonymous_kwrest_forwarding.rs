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
fn explicit_pairs_before_final_anonymous_forwarding_keep_their_ordered_group() {
    let source = "class Probe; def relay(path, **); target(kind: :get, path: path, **); end; def target(kind:, path:, **); [kind, path]; end; end";
    let app = ingest_app_from_tree(tree(&[("app/services/probe.rb", source)])).expect("ingest mixed anonymous forwarding");
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Probe")
        .expect("Probe class");
    let relay = class.methods.iter().find(|m| m.name.as_str() == "relay").expect("relay method");
    let ExprNode::Send { args, .. } = &*relay.body.node else {
        panic!("expected forwarded call, got {:?}", relay.body.node);
    };
    assert_eq!(args.len(), 1, "one logical keyword group: {args:?}");
    let ExprNode::ForwardKeywordsWithPairs { entries } = &*args[0].node else {
        panic!("expected ordered keyword-forward group, got {:?}", args[0].node);
    };
    let names: Vec<_> = entries
        .iter()
        .map(|(key, _)| match &*key.node {
            ExprNode::Lit { value: roundhouse::expr::Literal::Sym { value } } => value.as_str(),
            other => panic!("expected static symbol key, got {other:?}"),
        })
        .collect();
    assert_eq!(names, ["kind", "path"]);
}

#[test]
fn bare_anonymous_forwarding_does_not_become_a_positional_argument() {
    use roundhouse::ty::Ty;

    let source = r#"
class Probe
  def relay(**)
    target(**)
  end

  def relay_missing_position(**)
    target_with_required(**)
  end

  def target(value = 7, required:, **)
    [value, required]
  end

  def target_with_required(value, required:, **)
    [value, required]
  end
end
"#;
    let mut app = ingest_app_from_tree(tree(&[("app/services/probe.rb", source)])).expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);

    let target = app
        .inferred_method_params
        .get(&(roundhouse::ident::ClassId("Probe".into()), "target".into()))
        .expect("target call site was collected");
    assert!(
        matches!(target.first(), Some(Ty::Var { .. })),
        "bare ** supplies no positional evidence; the runtime regression verifies the default remains 7: {target:?}"
    );

    let target_with_required = app
        .inferred_method_params
        .get(&(roundhouse::ident::ClassId("Probe".into()), "target_with_required".into()))
        .expect("required-position call site was collected");
    assert!(
        matches!(target_with_required.first(), Some(Ty::Var { .. })),
        "bare ** must not satisfy the required positional parameter: {target_with_required:?}"
    );
}

#[test]
fn nonfinal_or_dynamic_mixed_anonymous_keyword_forwarding_stays_unsupported() {
    for call in ["target(**, factor: 11)", "target(**options, **)", "target(factor: 11, **options, **)"] {
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
fn optional_only_keyword_destination_stays_unsupported_without_keyword_rest() {
    use roundhouse::analyze::diagnose;
    use roundhouse::diagnostic::DiagnosticKind;

    let source = "class Probe; def self.call(**); target(**); end; def self.target(enabled: true, token: :missing); [enabled, token]; end; end";
    let mut app = ingest_app_from_tree(tree(&[("app/services/probe.rb", source)])).expect("ingest");
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .collect();
    assert!(errors.iter().any(|d| matches!(
        &d.kind,
        DiagnosticKind::Unsupported { construct, detail, .. }
            if construct.as_str() == "anonymous keyword forwarding"
                && detail.contains("flattened keyword parameters")
    )), "{errors:?}");
}

#[test]
fn anonymous_keywords_and_runtime_guards_are_honest_target_boundaries() {
    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    use roundhouse::project::{BuildTarget, target_files};
    for (source, construct) in [
        ("class Probe; def self.call(**); target(**); end; def self.target(factor:); factor; end; end", "anonymous keyword forwarding"),
        ("class Probe; def self.call(**); target(factor: false, **); end; def self.target(factor:, **); factor; end; end", "anonymous keyword forwarding"),
        ("class Probe; def self.call; defined?(MissingPr197); end; end", "runtime defined? query"),
        ("class Probe; def call; defined?(@@missing); end; end", "runtime defined? query"),
        ("class Probe; def call; @@count ||= 11; @@count; end; end", "class variable write"),
        ("class Probe; def call; @@count &&= 11; @@count; end; end", "class variable write"),
        ("class Probe; def call; @@count += 3; @@count; end; end", "class variable write"),
        ("class Probe; def self.call; @@count; end; end", "class variable read"),
        ("class Probe; @@count = nil; end", "class variable write"),
        ("class Probe; def self.call; $stdout = STDOUT; end; end", "global variable write"),
        ("class Probe; def self.call; $rh_memo ||= []; end; end", "global variable write"),
        ("class Probe; def self.call; $rh_count += 1; end; end", "global variable write"),
    ] {
        let mut app = ingest_app_from_tree(tree(&[("app/services/probe.rb", source)])).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        for target in BuildTarget::ALL.iter().copied().filter(|t| !matches!(t, BuildTarget::Blog)) {
            let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
                target_files(&app, roundhouse::fixtures::real_blog(), target)
            });
            let gates: Vec<_> = diags.iter().filter(|d| matches!(&d.kind,
                DiagnosticKind::Unsupported { construct: name, .. } if name.as_str() == construct)).collect();
            if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby)
                || (matches!(target, BuildTarget::Spinel)
                    && construct == "anonymous keyword forwarding")
            {
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
