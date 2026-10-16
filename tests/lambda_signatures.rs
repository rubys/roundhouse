//! A lambda keeps its whole signature through ingest and Ruby emit:
//! optional positionals, keywords (required and optional) and a keyword
//! rest, named or anonymous. Dropping one is not a degradation: the body
//! still reads the name, so the emitted program raises `NameError`.
//! Behavior on CRuby and Spinel is pinned in `emit_and_run` and
//! `spinel_toolchain`.

use roundhouse::emit::ruby as ruby_emit;
use roundhouse::expr::Expr;
use roundhouse::ingest;

fn ingest_snippet(source: &str) -> Expr {
    let result = ingest::prism::parse(source.as_bytes(), "<input>");
    let root = result.node();
    let program = root.as_program_node().expect("a Ruby program");
    ingest::ingest_expr(&program.statements().as_node(), "<input>").expect("ingest")
}

/// Ruby emit renders the signature as written, and re-ingesting the
/// emitted text gives the same IR.
fn assert_signature_kept(source: &str) {
    let first = ingest_snippet(source);
    let emitted = ruby_emit::emit_expr(&first);
    assert_eq!(emitted.trim(), source, "signature changed");
    assert_eq!(ingest_snippet(&emitted), first, "IR changed across emit and re-ingest:\n{emitted}");
}

#[test]
fn an_optional_positional_is_kept() {
    assert_signature_kept(r##"f = ->(name, size = 18) { "#{name}:#{size}" }"##);
}

#[test]
fn required_and_optional_keywords_are_kept() {
    assert_signature_kept("f = ->(ctx, key:, limit: 10) { ctx[key].first(limit) }");
}

#[test]
fn a_named_keyword_rest_is_kept() {
    assert_signature_kept("f = ->(ctx, **opts) { opts }");
}

#[test]
fn an_anonymous_keyword_rest_is_kept() {
    assert_signature_kept("f = ->(represented:, **) { represented }");
}

#[test]
fn every_kind_together_is_kept_in_ruby_order() {
    assert_signature_kept("f = ->(a, b = 2, *rest, key: 1, **opts) { [a, b, rest, key, opts] }");
}

/// A target whose emitter reads only the required names reports the
/// lambda as unsupported instead of emitting it without the rest.
/// Crystal stands in for the strict targets.
#[test]
fn a_strict_target_refuses_instead_of_dropping() {
    let app = include_str!("support/lambda_signatures.rs");
    assert!(app.contains("label = ->(name, size = 18)"), "contract moved");
    let tree = std::collections::HashMap::from([
        (std::path::PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n".to_vec()),
        (std::path::PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
        (std::path::PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\n  def self.label(name)\n    f = ->(n, size = 18) { \"#{n}:#{size}\" }\n    f.call(name)\n  end\nend\n".to_vec()),
    ]);
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for (target, refused) in [
        (roundhouse::project::BuildTarget::Crystal, true),
        (roundhouse::project::BuildTarget::Spinel, false),
        (roundhouse::project::BuildTarget::Ruby, false),
    ] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&app, std::path::Path::new("."), target)
        });
        let refusal = diags.iter().any(|d| d.message.contains("lambda optional, keyword or keyword-rest parameters"));
        assert_eq!(refusal, refused, "{target:?}: {diags:?}");
    }
}

/// A block with an optional parameter still binds its required ones from
/// the receiver: `[1, 2].map { |n, bonus = 2| n + bonus }` types `n` as the
/// element type; left unbound it was an unresolved type.
#[test]
fn a_block_with_an_optional_parameter_keeps_its_element_type() {
    let tree = std::collections::HashMap::from([
        (std::path::PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n".to_vec()),
        (std::path::PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
        (std::path::PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\n  def self.sums\n    [1, 2].map { |n, bonus = 2| n + bonus }\n  end\nend\n".to_vec()),
    ]);
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut diags = roundhouse::session::analyze_and_lower(&mut app);
    diags.extend(roundhouse::analyze::diagnose(&app));
    let unresolved: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.kind, roundhouse::diagnostic::DiagnosticKind::UnresolvedType { .. }))
        .collect();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
}

/// `**nil` (no keywords accepted) has no representation; dropping it would
/// let the emitted lambda accept keywords the source rejects, so ingest
/// refuses it.
#[test]
fn a_no_keywords_parameter_is_refused() {
    let result = ingest::prism::parse(b"f = ->(a, **nil) { a }", "<input>");
    let program = result.node().as_program_node().expect("a Ruby program");
    assert!(ingest::ingest_expr(&program.statements().as_node(), "<input>").is_err());
}
