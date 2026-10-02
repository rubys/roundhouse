//! A bounded Alba 4 source-property contract, distinct from hash-backed input.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;

const SOURCE: &str = include_str!("support/alba.rb");
const CONTRACT: &str = r#"
expected = {"id" => 7, "title" => "Synthetic", "author" => {"id" => 9, "name" => "Ada"}}
actual = SurveyProbe.call
raise "inherited/nested serialization: #{actual.inspect}" unless actual == expected && actual.inspect == expected.inspect
puts "PASS portable Alba source-property contract"
"#;

fn assert_typed(expr: &roundhouse::Expr) {
    assert!(
        expr.ty.as_ref().is_some_and(|ty| !ty.is_unknown()),
        "unresolved: {expr:#?}"
    );
    expr.node.for_each_child(&mut assert_typed);
}

fn app(source: &str) -> roundhouse::App {
    ingest_app_from_tree(HashMap::from([
        (
            PathBuf::from("app/lib/resources.rb"),
            source.as_bytes().to_vec(),
        ),
        (
            PathBuf::from("app/controllers/application_controller.rb"),
            b"class ApplicationController < ActionController::Base\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  root 'probes#index'\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/views/probes/index.html.erb"),
            b"<p>Probe</p>".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/probes_controller.rb"),
            br#"
class ProbesController < ActionController::Base
  def index
    author = AlbaAuthor.new(9, "Ada")
    article = AlbaArticle.new(7, "Syn", author)
    render json: ArticleResource.new(article).to_h
  end
end
"#
            .to_vec(),
        ),
    ]))
    .expect("ingest")
}

#[test]
fn consumer_and_serializer_bodies_are_inferred_not_stamped() {
    let mut app = app(SOURCE);
    Analyzer::new(&app).analyze(&mut app);
    let mut diags = diagnose(&app);
    let raw = diags.clone();
    let mut gaps = Vec::new();
    roundhouse::analyze::attribution::attribute_analysis_gaps(&mut diags, &app, &mut gaps);
    assert!(gaps.is_empty(), "admitted declarations are not gaps: {gaps:?}");
    assert_eq!(diags, raw);
    let errors: Vec<_> = diags
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    for name in ["ArticleResource", "AuthorResource"] {
        let class = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == name)
            .unwrap();
        let method = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "to_h")
            .unwrap();
        assert_typed(&method.body);
        assert!(class.unknown_calls.is_empty());
        assert!(
            !class
                .includes
                .iter()
                .any(|i| i.0.as_str() == "Alba::Resource")
        );
    }
}

#[test]
fn generated_ruby_runs_the_literal_contract_without_alba_or_class_preloads() {
    let mut app = app(SOURCE);
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    assert!(residue.is_empty(), "{residue:#?}");
    let dir = std::env::temp_dir().join(format!("roundhouse-alba-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(
            &app,
            std::path::Path::new("fixtures/tiny-blog"),
            roundhouse::project::BuildTarget::Ruby,
        )
    });
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    println!("Ruby: 0 lower_residue, 0 emit diagnostics");
    let files = files.unwrap();
    roundhouse::project::write_to_dir(&files, &dir).unwrap();
    let output = Command::new("ruby").args(["-e", &format!(
        "require './main'; {CONTRACT}; raise 'external Alba loaded' if defined?(Alba::Resource)"
    )]).env("SECRET_KEY_BASE", "public-synthetic-alba-test")
        .current_dir(&dir).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "PASS portable Alba source-property contract"
    );
}

#[test]
fn unsupported_declarations_are_rejected_not_silently_partially_lowered() {
    for declaration in [
        "attributes :title, if: :visible?",
        "attribute(:title) { 'invented' }",
        "attributes *NAMES",
        "one :author, resource: UnknownResource",
        "one :author, {resource: AuthorResource}",
        "one :author, resource: AuthorResource, if: :visible?",
        "one(:author, resource: AuthorResource) { |x| x }",
        "many :authors, resource: AuthorResource",
        "unknown_macro :title",
        "if true; attributes :title; end",
        "def initialize(object); @object = object; end",
        "def to_h; {}; end",
        "one :author, resource: (raise('receiver evaluated'))::AuthorResource",
    ] {
        let source = SOURCE.replace("attributes :title", declaration);
        let result = ingest_app_from_tree(HashMap::from([(
            PathBuf::from("app/lib/resources.rb"),
            source.into_bytes(),
        )]));
        assert!(
            result.is_err(),
            "accepted unsupported declaration: {declaration}"
        );
    }
}

#[test]
fn hidden_reopens_cannot_disappear_from_the_contract() {
    for fragment in [
        "class ArticleResource; attributes :extra; end",
        "if true; class ArticleResource; attributes :extra; end; end",
        "1.times do; class ArticleResource; attributes :extra; end; end",
        "ArticleResource.class_eval do; def to_h; {'changed' => 123}; end; end",
        "ArticleResource.define_method(:to_h) { {'changed' => 123} }",
        "class << ArticleResource; def new(object); object; end; end",
        "ArticleResource = AlbaArticle",
        "def ArticleResource.new(object); raise 'changed constructor'; end",
        "$resource = ArticleResource; $resource.class_eval { def to_h; {'altered' => true}; end }",
        "@resource = ArticleResource; @resource.class_eval { def to_h; {'altered' => true}; end }",
        "class AliasHolder; @@resource = ArticleResource; end",
        "$resource ||= ArticleResource; $resource.class_eval { def to_h; {}; end }",
        "@resource ||= ArticleResource; @resource.class_eval { def to_h; {}; end }",
        "class AliasHolder; @@resource ||= ArticleResource; end",
        "$resource &&= ArticleResource",
        "resources = [ArticleResource]; resources.first.class_eval { def to_h; {}; end }",
    ] {
        let result = ingest_app_from_tree(HashMap::from([
            (
                PathBuf::from("app/lib/resources.rb"),
                SOURCE.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/reopen.rb"),
                fragment.as_bytes().to_vec(),
            ),
        ]));
        assert!(result.is_err(), "accepted a resource fragment: {fragment}");
    }
}

#[test]
fn unsafe_inputs_are_rejected_at_each_site_even_beside_a_valid_constructor() {
    for input in [
        "{}",
        "nil",
        "[]",
        "missing_object",
        "ExternalInput.object",
        "AlbaArticle",
        "AlbaArticle.new(7, 'Syn', AlbaAuthor)",
        "AlbaArticle.new((author = AlbaAuthor; 7), 'Syn', author)",
    ] {
        for unsafe_first in [true, false] {
            let good = "ArticleResource.new(article).to_h";
            let unsafe_call = format!("ArticleResource.new({input}).to_h");
            let calls = if unsafe_first {
                format!("{unsafe_call}\n    {good}")
            } else {
                format!("{good}\n    {unsafe_call}")
            };
            let mut app = app(&SOURCE.replace(good, &calls));
            Analyzer::new(&app).analyze(&mut app);
            let errors: Vec<_> = diagnose(&app)
                .into_iter()
                .filter(|d| d.severity == Severity::Error && d.code() == "unsupported")
                .collect();
            assert!(
                !errors.is_empty(),
                "unsafe input {input}, first={unsafe_first} was hidden by a valid call"
            );
            assert!(errors.iter().all(|d| !d.span.is_synthetic()), "{errors:#?}");
        }
    }
}

#[test]
fn a_missing_property_is_not_library_only_clean_support() {
    let mut app = app(&SOURCE.replace("def title\n", "def unused_title\n"));
    Analyzer::new(&app).analyze(&mut app);
    assert!(
        diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error && d.code() == "unsupported")
    );
}

#[test]
fn class_alias_reassignments_cannot_be_laundered_by_source_order() {
    let good = "ArticleResource.new(article).to_h";
    let bad = "ArticleResource.new(candidate).to_h";
    for statements in [
        format!("candidate = AlbaArticle\n{bad}\n{good}"),
        format!("{good}\ncandidate = AlbaArticle\n{bad}"),
        format!("candidate = article\ncandidate = AlbaArticle\n{bad}\n{good}"),
        format!("candidate = AlbaArticle\n{bad}\ncandidate = article\n{good}"),
        format!(
            "candidate = article\nArticleResource.new((candidate = AlbaArticle; candidate)).to_h\n{good}"
        ),
        format!(
            "candidate = AlbaArticle\nif false\n candidate = article\nelse\n {bad}\nend\n{good}"
        ),
    ] {
        let mut app = app(&SOURCE.replace(good, &statements));
        Analyzer::new(&app).analyze(&mut app);
        assert!(
            diagnose(&app).iter().any(|d| d.code() == "unsupported"),
            "accepted {statements}"
        );
    }
    let mut app = app(&SOURCE.replace(good, &format!("candidate = article\n{bad}")));
    Analyzer::new(&app).analyze(&mut app);
    assert!(
        diagnose(&app).iter().all(|d| d.severity != Severity::Error),
        "valid instance alias refused"
    );
}

#[test]
fn constructor_reader_evidence_requires_unmodified_required_parameters() {
    for (before, after) in [
        (
            "@author = author",
            "author = AlbaAuthor\n    @author = author",
        ),
        (
            "@author = author",
            "@author = author\n    @author ||= AlbaAuthor",
        ),
        (
            "@author = author",
            "@author = author\n    @author, ignored = AlbaAuthor, 1",
        ),
        (
            "initialize(id, title, author)",
            "initialize(id, title, author = AlbaAuthor)",
        ),
        (
            "class BaseResource",
            "class AlbaArticle; def initialize(id, title, author); @other = id; end; end\nclass BaseResource",
        ),
        (
            "  def author\n",
            "  def initialize(id, title, author); @other = id; end\n\n  def author\n",
        ),
        (
            "class BaseResource",
            "class AlbaArticle; def author; AlbaAuthor; end; end\nclass BaseResource",
        ),
    ] {
        let mut app = app(&SOURCE.replace(before, after));
        Analyzer::new(&app).analyze(&mut app);
        assert!(
            diagnose(&app).iter().any(|d| d.code() == "unsupported"),
            "accepted unproven reader: {after}"
        );
    }
    let source = SOURCE.replace(
        "AlbaArticle.new(7, \"Syn\", author)",
        "AlbaArticle.new(7, \"Syn\")",
    );
    let mut app = app(&source);
    Analyzer::new(&app).analyze(&mut app);
    assert!(diagnose(&app).iter().any(|d| d.code() == "unsupported"));
}

#[test]
fn unused_resources_and_unrelated_library_diagnostics_are_unchanged() {
    let source = SOURCE.split("class SurveyProbe").next().unwrap().to_owned()
        + "\nclass Unrelated; def call; UnknownVendor.unknown; end; end\n";
    let mut app = ingest_app_from_tree(HashMap::from([(
        PathBuf::from("app/lib/resources.rb"),
        source.into_bytes(),
    )]))
    .unwrap();
    Analyzer::new(&app).analyze(&mut app);
    assert!(diagnose(&app).is_empty());
}

#[test]
fn nullable_and_false_properties_remain_outside_the_named_contract() {
    for expression in ["nil", "false", "[9]", "@author", "AlbaAuthor"] {
        let source = SOURCE.replace("@author || raise(\"author required\")", expression);
        let mut app = app(&source);
        Analyzer::new(&app).analyze(&mut app);
        assert!(
            diagnose(&app).iter().any(|d| d.code() == "unsupported"),
            "accepted {expression}"
        );
    }
}

#[test]
fn plain_attributes_cannot_impersonate_the_declared_nested_resource() {
    for value in ["{'invented' => 'value'}", "(@id == 7 ? 'text' : 9)"] {
        let source = SOURCE.replace("title + \"thetic\"", value);
        let mut app = app(&source);
        Analyzer::new(&app).analyze(&mut app);
        assert!(
            diagnose(&app).iter().any(|d| d.code() == "unsupported"),
            "accepted {value}"
        );
    }
}

#[test]
fn view_only_constructors_are_checked_without_another_callers_evidence() {
    let source = SOURCE.split("class SurveyProbe").next().unwrap();
    for input in [
        "nil",
        "{}",
        "[]",
        "unknown_source",
        "AlbaArticle.new(7, 'Syn', AlbaAuthor.new(9, 'Ada'))",
    ] {
        let mut tree = HashMap::from([
            (
                PathBuf::from("app/lib/resources.rb"),
                source.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/views/probes/index.html.erb"),
                format!("<% ArticleResource.new({input}).to_h %><p>Probe</p>").into_bytes(),
            ),
        ]);
        // Include a normal view controller, but no serializer invocation there.
        tree.insert(
            PathBuf::from("app/controllers/probes_controller.rb"),
            b"class ProbesController < ActionController::Base; def index; end; end".to_vec(),
        );
        let mut app = ingest_app_from_tree(tree).unwrap();
        Analyzer::new(&app).analyze(&mut app);
        let unsupported = diagnose(&app).iter().any(|d| d.code() == "unsupported");
        assert_eq!(
            unsupported,
            !input.starts_with("AlbaArticle.new"),
            "view input {input}"
        );
    }
}

#[test]
fn dynamic_constant_parents_cannot_be_erased_before_admission() {
    for (literal, dynamic) in [
        (
            "resource: AuthorResource",
            "resource: (raise('evaluated'))::AuthorResource",
        ),
        ("< BaseResource", "< (raise('evaluated'))::BaseResource"),
        (
            "class ArticleResource",
            "class (raise('evaluated'))::ArticleResource",
        ),
        (
            "include Alba::Resource",
            "include (raise('evaluated'))::Alba::Resource",
        ),
    ] {
        let result = ingest_app_from_tree(HashMap::from([(
            PathBuf::from("app/lib/resources.rb"),
            SOURCE.replace(literal, dynamic).into_bytes(),
        )]));
        assert!(result.is_err(), "accepted dynamic identity {dynamic}");
    }
}

#[test]
fn alba_origin_does_not_change_emitted_paths_imports_or_loading() {
    let mut tagged = app(SOURCE);
    roundhouse::session::analyze_and_lower(&mut tagged);
    let mut untagged = tagged.clone();
    for class in &mut untagged.library_classes {
        if matches!(
            class.origin,
            Some(roundhouse::dialect::LibraryClassOrigin::AlbaResource { .. })
        ) {
            class.origin = None;
        }
    }
    for target in roundhouse::project::BuildTarget::ALL {
        // Emitters maintain thread-local accessor registries. Isolate both
        // runs so stale entries from the first emit cannot affect the second.
        let emit = |app: roundhouse::App| {
            let target = *target;
            std::thread::spawn(move || {
                roundhouse::project::target_files(
                    &app,
                    std::path::Path::new("fixtures/tiny-blog"),
                    target,
                )
                .unwrap()
            })
            .join()
            .unwrap()
        };
        let actual = emit(tagged.clone());
        let expected = emit(untagged.clone());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.0, expected.0, "{target:?}");
            assert_eq!(actual.1, expected.1, "{target:?}: {}", actual.0);
        }
        assert_eq!(actual.len(), expected.len(), "{target:?}");
    }
}

/// Actual native module/program contract, not Rails/server boot or original
/// Alba gem compilation. CI/dev toolchain lane: SPINEL=/path/to/spinel cargo
/// test --test alba generated_spinel -- --ignored --nocapture.
#[test]
#[ignore = "requires the real Spinel compiler and C toolchain"]
fn generated_spinel_compiles_and_runs_the_same_literal_contract() {
    let mut app = app(SOURCE);
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    assert!(residue.is_empty(), "{residue:#?}");
    let dir = std::env::temp_dir().join(format!("roundhouse-alba-spinel-{}", std::process::id()));
    let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(
            &app,
            std::path::Path::new("fixtures/tiny-blog"),
            roundhouse::project::BuildTarget::Spinel,
        )
    });
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    println!("Spinel: 0 lower_residue, 0 emit diagnostics");
    let files = files.unwrap();
    roundhouse::project::write_to_dir(&files, &dir).unwrap();
    std::fs::write(
        dir.join("contract.rb"),
        format!("require_relative 'app/models'\n{CONTRACT}"),
    )
    .unwrap();
    let output = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .args(["--require-gate", "contract.rb", "-o", "contract"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compile: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(dir.join("contract")).output().unwrap();
    assert!(
        output.status.success(),
        "run: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "PASS portable Alba source-property contract"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
