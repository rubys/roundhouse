//! Full forwarding has a positional/keyword/block contract, not a naming convention.
//! Native controls deliberately distinguish all three from plausible miscompiles.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;

const LIBRARY: &str = r#"
class RequiredSink
  def combine(first, second, factor:, offset: 0, &blk)
    result = (first - second) * factor + offset
    blk ? blk.call(result) : result
  end
end
class NoKeywordSink
  def combine(first, second, &blk)
    result = first - second
    blk ? blk.call(result) : result
  end
end
class ShapeSink
  def combine(first, options, factor:)
    yield(first * factor + options[:factor])
  end
end
class AnonymousKeywordSink
  def combine(first, __fwd_kwargs, **)
    first - __fwd_kwargs
  end
end
class DiscardedKeywordSink
  def combine(first, second, *rest, **ignored)
    first - second + rest.length
  end
end
class Forwarder
  def call(...)
    helper = RequiredSink.new
    helper.combine(...)
  end
  def empty(...)
    helper = NoKeywordSink.new
    helper.combine(...)
  end
  def shapes(...)
    helper = ShapeSink.new
    helper.combine(...)
  end
  def discard(...)
    11
  end
  def anonymous_keywords(...)
    helper = AnonymousKeywordSink.new
    helper.combine(...)
  end
  def discarded_keywords(...)
    helper = DiscardedKeywordSink.new
    helper.combine(...)
  end
  def self.call(...)
    target(...)
  end
  def self.target(first, second, factor:)
    yield((first - second) * factor)
  end
  def leading(__fwd_args, ...)
    helper = RequiredSink.new
    helper.combine(__fwd_args, ...)
  end
  def collisions(...)
    __fwd_args = [1, 2]
    __fwd_kwargs = {factor: 99}
    __fwd_blk = nil
    helper = RequiredSink.new
    helper.combine(...)
  end
end
class Bridge
  def call(...)
    helper = Forwarder.new
    helper.call(...)
  end
end
class Child < RequiredSink
  def combine(...)
    super(...)
  end
end
class ImplicitChild < RequiredSink
  def combine(...)
    super
  end
end
class Inherited < Forwarder
end
class Ordinary
  def call(__fwd_args, __fwd_kwargs, &block)
    helper = RequiredSink.new
    begin
      helper.combine(*__fwd_args, __fwd_kwargs, &block)
    rescue ArgumentError
      "rejected"
    end
  end
end
class CompiledEntry
  def self.call
    helper = Forwarder.new
    helper.call(11, 4, factor: 3) { |r| r * 2 + 1 }
  end
end
class NamedNew
  def initialize(factor: 2)
    @factor = factor
  end
  def new(first, second, factor:)
    (first - second) * factor
  end
  def relay(...)
    new(...)
  end
end
"#;

const CONCERN: &str = r#"
module ForwardingConcern
  extend ActiveSupport::Concern
  def forwarded_concern(...)
    helper = RequiredSink.new
    helper.combine(...)
  end
  class_methods do
    def forwarded_class(...)
      helper = RequiredSink.new
      helper.combine(...)
    end
  end
end
"#;

const MODEL_METHODS: &str = r#"
  include ForwardingConcern
  def forwarded_model(...)
    helper = RequiredSink.new
    helper.combine(...)
  end
  def discard_model_keywords(first, __fwd_kwargs, **)
    first - __fwd_kwargs
  end
  def named_model(__fwd_args, *__fwd_kwargs, last, &__blk)
    result = __fwd_args - last + __fwd_kwargs.length
    __blk ? __blk.call(result) : result
  end
  def kwargs_model(*values, **__fwd_kwargs, &__fwd_blk)
    result = values.length + __fwd_kwargs.length
    __fwd_blk ? __fwd_blk.call(result) : result
  end
"#;

const ASSERTIONS: &str = r#"
def expect_value(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
def expect_argument_error
  raised = false
  begin
    yield
  rescue ArgumentError
    raised = true
  end
  raise "expected ArgumentError" unless raised
end
f = Forwarder.new
expect_value(23, f.call(11, 4, factor: 3) { |r| r + 2 })
expect_value(43, f.call(11, 4, factor: 3) { |r| r * 2 + 1 })
expect_value(21, f.call(11, 4, factor: 3))
expect_value(28, f.call(11, 4, factor: 3, offset: 5) { |r| r + 2 })
expect_value(9, f.empty(11, 4) { |r| r + 2 })
expect_value(9, f.empty(11, 4, **{}) { |r| r + 2 })
expect_value(75, f.shapes(11, {factor: 4}, factor: 3) { |r| r * 2 + 1 })
expect_value(11, f.discard(1, 2, 3, factor: 9) { raise "unused block" })
expect_value(7, f.anonymous_keywords(11, 4))
expect_value(7, f.anonymous_keywords(11, 4, factor: 3, extra: 9))
expect_value(9, f.discarded_keywords(11, 4, 8, 10, factor: 3))
expect_value(23, Forwarder.call(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, f.leading(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, f.collisions(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Bridge.new.call(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Child.new.combine(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, ImplicitChild.new.combine(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Inherited.new.call(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Article.new.forwarded_model(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Article.new.forwarded_concern(11, 4, factor: 3) { |r| r + 2 })
expect_value(23, Article.forwarded_class(11, 4, factor: 3) { |r| r + 2 })
expect_value(7, Article.new.discard_model_keywords(11, 4, factor: 3, extra: 9))
expect_value(11, Article.new.named_model(11, 8, 10, 4) { |r| r + 2 })
expect_value(6, Article.new.kwargs_model(11, 4, factor: 3, extra: 9) { |r| r + 2 })
expect_value("rejected", Ordinary.new.call([11, 4], {factor: 3}) { |r| r + 2 })
expect_value(43, CompiledEntry.call)
expect_value(21, NamedNew.new.relay(11, 4, factor: 3))
expect_argument_error { f.call(11, 4) }
expect_argument_error { f.call(11, 4, factor: 3, unknown: 7) }
expect_argument_error { f.call(11, factor: 3) }
expect_argument_error { f.call(11, 4, 9, factor: 3) }
puts "forwarding contract passed"
"#;

#[test]
fn native_and_emitted_forwarding_preserve_arguments_keywords_and_block() {
    let native = Command::new("ruby")
        .args(["-ractive_support/concern", "-e"])
        .arg(format!(
            "{LIBRARY}\n{CONCERN}\nclass Article\n{MODEL_METHODS}\nend\n{ASSERTIONS}"
        ))
        .output()
        .expect("native Ruby control");
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "forwarding contract passed\n"
    );

    let run = emit_and_run::real_blog()
        .write("app/lib/forwarder.rb", LIBRARY)
        .write("app/models/concerns/forwarding_concern.rb", CONCERN)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{MODEL_METHODS}\n"),
        )
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert_eq!(run.stdout, "forwarding contract passed\n");
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/forwarder.rb")).unwrap();
    assert!(emitted.contains("def call(...)"), "{emitted}");
    assert!(emitted.contains("helper.combine(...)"), "{emitted}");
    assert!(!emitted.contains("def call(*__fwd_args"), "{emitted}");
    let rbs = std::fs::read_to_string(run.emitted.join("sig/app/models/forwarder.rbs")).unwrap();
    assert!(
        rbs.contains(
            "def call: (*untyped, **untyped) ?{ (*untyped, **untyped) -> untyped } -> untyped"
        ),
        "{rbs}"
    );
    assert!(!rbs.contains("untyped ..."), "{rbs}");
    ruby_rbs::node::parse(&rbs).expect("forwarding sidecar parses as RBS");
    let anonymous =
        std::fs::read_to_string(run.emitted.join("app/models/anonymous_keyword_sink.rb")).unwrap();
    assert!(
        anonymous.contains("def combine(first, __fwd_kwargs, **)"),
        "{anonymous}"
    );
    let anonymous_rbs = std::fs::read_to_string(
        run.emitted
            .join("sig/app/models/anonymous_keyword_sink.rbs"),
    )
    .unwrap();
    ruby_rbs::node::parse(&anonymous_rbs)
        .expect("anonymous keyword declaration sidecar parses as RBS");
}

fn analyzed(source: &str) -> roundhouse::App {
    let mut app = ingest_app_from_tree(HashMap::from([(
        PathBuf::from("app/lib/probe.rb"),
        source.as_bytes().to_vec(),
    )]))
    .expect("ingest synthetic forwarding source");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn forwarding_markers_are_not_values_or_gradual_escapes() {
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::expr::ExprNode;

    let sink = "class Sink; def self.target(a,b); a-b; end; end";
    let methods = "def relay(...); Sink.target(...); end; def opaque; yield; end";
    let mut app = ingest_app_from_tree(HashMap::from([
        (PathBuf::from("app/lib/sink.rb"), sink.as_bytes().to_vec()),
        (PathBuf::from("app/models/article.rb"), format!("class Article < ApplicationRecord; {methods}; end").into_bytes()),
    ])).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let mut pending: Vec<_> = app.models[0].methods().map(|m| &m.body).collect();
    let mut packets = Vec::new();
    let mut yields = Vec::new();
    while let Some(expr) = pending.pop() {
        match &*expr.node {
            ExprNode::ForwardArgs => packets.push(expr.span),
            ExprNode::Yield { .. } => yields.push(expr.span),
            _ => {}
        }
        expr.node.for_each_child(&mut |child| pending.push(child));
    }
    assert_eq!(packets.len(), 1);
    assert_eq!(yields.len(), 1);
    let diags = diagnose(&app);
    let gradual: Vec<_> = diags.iter().filter(|d| matches!(d.kind, DiagnosticKind::GradualUntyped { .. })).collect();
    assert!(!gradual.iter().any(|d| packets.contains(&d.span)), "{diags:?}");
    assert!(gradual.iter().any(|d| yields.contains(&d.span)), "{diags:?}");

    let script = "puts Article.new.relay(11,3)";
    native_result(&format!("{sink}; class Article; {methods}; end"), script, "8\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/sink.rb", sink)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n {methods}\n"))
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "8\n");
}

#[test]
fn flattened_optional_keyword_callee_is_honestly_unsupported() {
    let source = r#"
class Sink
  def call(first, second, factor: 2)
    (first - second) * factor
  end
end
class Probe
  def call(...)
    helper = Sink.new
    helper.call(...)
  end
end
"#;
    let app = analyzed(source);
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| d.message.contains("forwarding") && d.message.contains("flattened")),
        "{errors:?}"
    );
}

#[test]
fn forwarding_declaration_and_call_round_trip_as_a_contract() {
    let source = "class Probe\n  def call(__fwd_args, ...)\n    target(__fwd_args, ...)\n  end\n  def target(first, second, factor:)\n    yield((first - second) * factor)\n  end\nend\n";
    let app = analyzed(source);
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Probe")
        .unwrap();
    let emitted = format!(
        "class Probe\n{}end\n",
        class
            .methods
            .iter()
            .map(roundhouse::emit::ruby::emit_method)
            .collect::<String>()
    );
    let again = analyzed(&emitted);
    let second = again
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Probe")
        .unwrap();
    assert_eq!(
        format!(
            "class Probe\n{}end\n",
            second
                .methods
                .iter()
                .map(roundhouse::emit::ruby::emit_method)
                .collect::<String>()
        ),
        emitted
    );
    assert!(emitted.contains("def call(__fwd_args, ...)"), "{emitted}");
    assert!(emitted.contains("target(__fwd_args, ...)"), "{emitted}");
}

#[test]
fn mixed_anonymous_keyword_pairs_round_trip_as_an_ordered_group() {
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::expr::ExprNode;

    let source = "class Probe\n  def relay(path, **)\n    target(kind: :get, path: path, **)\n  end\n  def target(kind:, path:, **)\n    [kind, path]\n  end\nend\n";
    let group_names = |app: &roundhouse::App| {
        let class = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == "Probe")
            .expect("Probe class");
        let relay = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "relay")
            .expect("relay method");
        let ExprNode::Send { args, .. } = &*relay.body.node else {
            panic!("expected forwarded call, got {:?}", relay.body.node);
        };
        assert_eq!(args.len(), 1, "the keyword group is one call argument");
        let ExprNode::ForwardKeywordsWithPairs { entries } = &*args[0].node else {
            panic!("expected ordered anonymous keyword group, got {:?}", args[0].node);
        };
        entries
            .iter()
            .map(|(key, _)| match &*key.node {
                ExprNode::Lit { value: roundhouse::expr::Literal::Sym { value } } => value.as_str().to_string(),
                other => panic!("expected static symbol key, got {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    let emit = |app: &roundhouse::App| {
        let class = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == "Probe")
            .expect("Probe class");
        format!(
            "class Probe\n{}end\n",
            class.methods.iter().map(roundhouse::emit::ruby::emit_method).collect::<String>()
        )
    };
    let app = analyzed(source);
    assert_eq!(group_names(&app), vec![String::from("kind"), String::from("path")]);
    let errors = diagnose(&app);
    assert!(
        !errors.iter().any(|d| matches!(
            &d.kind,
            DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "full argument forwarding"
        )),
        "anonymous keyword forwarding must not be diagnosed as full `...` forwarding: {errors:?}"
    );
    let first = emit(&app);
    let again = analyzed(&first);
    assert_eq!(group_names(&again), group_names(&app), "re-ingest must preserve the explicit pair group and its order");
    let second = emit(&again);
    assert_eq!(first, second, "mixed anonymous keyword forwarding must reach a Ruby IR fixed point");
    assert!(first.contains("target(kind: :get, path: path, **)"), "{first}");
}

#[test]
fn flattened_and_unknown_contracts_remain_errors_through_lowering() {
    for source in [
        "class Probe\n def call(...)\n target(...)\n end\n def target(a, b, factor: 2)\n (a-b)*factor\n end\nend",
        "class Probe\n def call(...)\n self.target(...)\n end\n def target(a, b, **kw)\n (a-b)*kw[:factor]\n end\nend",
        "class Parent\n def call(a, b, factor: 2)\n (a-b)*factor\n end\nend\nclass Child < Parent\n def call(...)\n super\n end\nend",
        "module Mixed\n def call(a, b, factor: 2)\n (a-b)*factor\n end\nend\nclass Parent\n def call(a,b,factor:)\n (a-b)*factor\n end\nend\nclass Child < Parent\n include Mixed\n def call(...)\n super(...)\n end\nend",
        "class Probe\n def call(...)\n absent(...)\n end\nend",
    ] {
        let native = Command::new("ruby")
            .args(["-c", "-e", source])
            .output()
            .unwrap();
        assert!(native.status.success(), "native legal source: {source}");
        let mut app = analyzed(source);
        let lower = roundhouse::session::analyze_and_lower(&mut app);
        let errors: Vec<_> = diagnose(&app)
            .into_iter()
            .chain(lower)
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.iter().any(|d| d.message.contains("forwarding")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn keyword_target_gates_distinguish_ordinary_super_from_full_forwarding() {
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::project::{BuildTarget, target_files};

    for (source, expected) in [
        ("class Probe < StandardError; def initialize; options={}; super(**options); end; end",
         "keyword splat in ordinary super"),
        ("class Probe; def self.call(...); 11; end; def self.run; options={}; call(**options); end; end",
         "keyword splat into full argument forwarding"),
    ] {
        let app = analyzed(source);
        for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Spinel, BuildTarget::Roda] {
            let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
                target_files(&app, roundhouse::fixtures::real_blog(), target)
            });
            let gates: Vec<_> = diagnostics.iter().filter(|d| {
                matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "keyword splat in ordinary super"
                        || construct.as_str() == "keyword splat into full argument forwarding")
            }).collect();
            assert_eq!(gates.len(), 1, "{target:?}: {source}: {diagnostics:?}");
            let gate = gates[0];
            assert_eq!(gate.severity, Severity::Error);
            assert!(!gate.span.is_synthetic());
            assert!(matches!(&gate.kind, DiagnosticKind::Unsupported { construct, target: Some(name), .. }
                if construct.as_str() == expected && name.as_str() == target.as_str()), "{gate:?}");
            if expected == "keyword splat in ordinary super" {
                assert!(gate.message.contains("argument ABI"), "{gate:?}");
            }
        }
    }
}

#[test]
fn source_archive_copies_unrepresented_formals_without_a_transpile_error() {
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::project::{BuildTarget, target_files};

    for formal in ["(a,b)", "**nil"] {
        let source = format!("class Probe; def call({formal}); 7; end; end\n");
        let app = analyzed(&source);
        let fixture = std::env::temp_dir().join(format!("roundhouse-source-archive-{}-{}", std::process::id(), formal.len()));
        std::fs::create_dir(&fixture).unwrap();
        std::fs::write(fixture.join("probe.rb"), &source).unwrap();
        for target in [BuildTarget::Blog, BuildTarget::Ruby] {
            let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
                target_files(&app, &fixture, target)
            });
            let formal_errors: Vec<_> = diagnostics.iter().filter(|d| {
                matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "parameter declaration")
            }).collect();
            assert_eq!(formal_errors.len(), usize::from(target == BuildTarget::Ruby), "{target:?}: {diagnostics:?}");
            if target == BuildTarget::Blog {
                let files = files.expect("source archive");
                assert!(files.iter().any(|(path, text)| path == "probe.rb" && text == &source), "{files:?}");
            } else {
                assert_eq!(formal_errors[0].severity, Severity::Error);
            }
        }
        std::fs::remove_dir_all(&fixture).unwrap();
    }
}

#[test]
fn declaration_only_forwarders_are_gated_on_unverified_targets() {
    use roundhouse::project::{BuildTarget, target_files};
    let app = analyzed("class Probe\n def call(...)\n 11\n end\nend");
    for target in [
        BuildTarget::Rust,
        BuildTarget::Typescript,
        BuildTarget::Spinel,
        BuildTarget::Roda,
    ] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, roundhouse::fixtures::real_blog(), target)
        });
        files.expect("project assembly");
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains("forwarding")),
            "{target:?}: {diags:?}"
        );
    }
}

/// A controller method keeps its `...` / `**` (`Action::anonymous_formal`);
/// a test entrypoint, which nothing calls with arguments, still refuses.
#[test]
fn controller_declarations_are_kept_and_test_entry_declarations_are_rejected() {
    use roundhouse::dialect::AnonymousFormal;
    for (formal, kept) in [("...", AnonymousFormal::Forwarding), ("**", AnonymousFormal::KeywordRest)] {
        let source = format!("class ProbeController < ApplicationController\n def call({formal})\n 11\n end\nend");
        let controller = roundhouse::ingest::ingest_controller(source.as_bytes(), "probe_controller.rb")
            .expect("a controller method keeps its anonymous formal")
            .expect("one controller");
        let call = controller.actions().find(|a| a.name.as_str() == "call").expect("call");
        assert_eq!(call.anonymous_formal, Some(kept), "{formal}");
        for name in ["setup", "test_forwarding"] {
            let source = format!("class ProbeTest < ActiveSupport::TestCase\n def {name}({formal})\n 11\n end\nend");
            let err = roundhouse::ingest::ingest_test_file(source.as_bytes(), "probe_test.rb")
                .expect_err("test entrypoint forwarding is outside this slice");
            assert!(err.to_string().contains("forwarding declaration"));
        }
    }
}

#[test]
fn anonymous_keyword_forwarding_refuses_unverified_keyword_abis() {
    for source in [
        "class Probe; def call(__fwd_kwargs, **); missing(__fwd_kwargs, **); end; end",
        "class Probe; def target(**options); options; end; def call(**); target(**); end; end",
        "class Probe; def target; 7; end; def call(**); target(**); end; end",
    ] {
        let mut app = analyzed(source);
        let lower = roundhouse::session::analyze_and_lower(&mut app);
        let errors: Vec<_> = diagnose(&app).into_iter().chain(lower)
            .filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.iter().any(|d| d.message.contains("keyword forwarding")), "{source}: {errors:?}");
    }
}

#[test]
fn unrepresented_formals_never_admit_a_forwarding_contract() {
    for (source, call, expected) in [
        (
            "class Probe; def target((a,b)); 7; end; def call(...); target(...); end; end",
            "Probe.new.call([11,4])",
            "7",
        ),
        (
            "class Probe; def target(**nil); 7; end; def call(...); target(...); end; end",
            "Probe.new.call",
            "7",
        ),
        (
            "class Probe; def call((a,b),...); target(...); end; def target(first,factor:); first*factor; end; end",
            "Probe.new.call([11,4],7,factor:3)",
            "21",
        ),
    ] {
        let native = Command::new("ruby")
            .args(["-e", &format!("{source}; puts {call}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), expected);
        let mut app = analyzed(source);
        let lowering = roundhouse::session::analyze_and_lower(&mut app);
        let errors: Vec<_> = diagnose(&app)
            .into_iter()
            .chain(lowering)
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("parameter declaration")),
            "{source}: {errors:?}"
        );
    }
}

/// An unforwarded anonymous `*` binds to a generated name (see
/// `ingest::forwarding`), so it is a represented formal: a `...` caller
/// forwards into it and the emitted program answers what Ruby answers.
#[test]
fn an_unforwarded_anonymous_rest_is_a_represented_formal() {
    let source = "class Probe; def target(*); 7; end; def call(...); target(...); end; end";
    let script = "puts Probe.new.call(1); puts Probe.new.target; puts Probe.new.target(1, 2, 3)";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "7\n7\n7\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "7\n7\n7\n");
}

/// A nested `def` binds its own parameters, so a bare `*` inside it
/// forwards its own rest and does not make the outer `*` forwarded.
#[test]
fn a_nested_def_forwarding_its_own_rest_leaves_the_outer_rest_represented() {
    let source = "class Probe; def target(*); def nested(*) = inner(*); 7; end; def inner(*a) = a.size; end";
    let script = "puts Probe.new.target; puts Probe.new.target(1, 2, 3)";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "7\n7\n");
    let errors: Vec<_> = diagnose(&analyzed(source))
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "7\n7\n");
}

/// A body that forwards the anonymous rest (`g(*)`) keeps the recorded
/// fact: the bare splat would ingest as `*nil`, and forwarding it is not
/// modeled yet.
#[test]
fn a_forwarded_anonymous_rest_stays_unsupported() {
    let source = "class Probe; def target(*); inner(*); end; def inner(*a); a.size; end; end";
    let app = analyzed(source);
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.iter().any(|d| d.message.contains("anonymous positional rest")),
        "{errors:?}"
    );
}

#[test]
fn custom_new_is_not_admitted_as_the_initializer_contract() {
    let source = "class Factory; def self.new(factor: 2); factor; end; def initialize; end; end; class Probe; def call(...); Factory.new(...); end; end";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; puts Probe.new.call(factor: 3)")])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
    let errors: Vec<_> = diagnose(&analyzed(source))
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.iter().any(|d| d.message.contains("flattened")),
        "{errors:?}"
    );
}

#[test]
fn anonymous_keyword_rest_keeps_ordinary_and_forwarded_optional_keywords() {
    let source = "class Probe; def self.scale(n, factor: 2, **); n*factor; end; def self.run; Probe.scale(7, factor: 3); end; def self.call(...); scale(...); end; end";
    let script = "raise unless Probe.run == 21; raise unless Probe.call(7,factor:3) == 21; raise unless Probe.call(7) == 14; puts 'keyword identity passed'";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "keyword identity passed\n");
}

#[test]
fn forwarded_operator_and_index_calls_keep_explicit_send_syntax() {
    let source = r#"
class Sink
  def [](first, second, factor:)
    yield((first-second)*factor)
  end
  def ==(first)
    first+2
  end
end
class Probe
  def index(...)
    Sink.new.[](...)
  end
  def equal(...)
    Sink.new.==(...)
  end
end
"#;
    let script = "raise unless Probe.new.index(11,4,factor:3) { |r| r*2+1 } == 43; raise unless Probe.new.equal(11) == 13; puts 'operator forwarding passed'";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "operator forwarding passed\n");
}

#[test]
fn model_formal_rejection_survives_body_rewriting() {
    let sink = "class Sink; def target(n,factor:); n*factor; end; end";
    let method = "def relay((a,b),...); helper=Sink.new; helper.target(...); end";
    let mut app = ingest_app_from_tree(HashMap::from([
        (PathBuf::from("app/lib/sink.rb"), sink.as_bytes().to_vec()),
        (
            PathBuf::from("app/models/article.rb"),
            format!("class Article < ApplicationRecord; {method}; end").into_bytes(),
        ),
    ]))
    .unwrap();
    let expected = Some(roundhouse::dialect::UnsupportedFormal::Destructured);
    assert_eq!(
        app.models[0]
            .methods()
            .find(|m| m.name.as_str() == "relay")
            .unwrap()
            .unsupported_formals,
        expected
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let declaration = app.models[0]
        .methods()
        .find(|m| m.name.as_str() == "relay")
        .unwrap();
    assert_eq!(declaration.unsupported_formals, expected);
    assert!(
        declaration.body.diagnostic.is_none(),
        "formal provenance is not a body annotation"
    );
    let (_, emission) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(
            &app,
            roundhouse::fixtures::real_blog(),
            roundhouse::project::BuildTarget::Ruby,
        )
    });
    assert!(
        emission
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("parameter declaration")),
        "{emission:?}"
    );
    let native = Command::new("ruby")
        .args([
            "-e",
            &format!(
                "{sink}; class Article; {method}; end; puts Article.new.relay([11,4],7,factor:3)"
            ),
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "21\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/sink.rb", sink)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n {method}\n"),
        )
        .run_ruby("puts Article.new.relay([11,4],7,factor:3)");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("parameter declaration")),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn a_class_valued_local_is_not_assumed_to_be_an_instance() {
    let source = "class Sink; def target(factor:); factor; end; def self.target(factor: 2); factor; end; end; class Probe; def call(...); receiver=Sink; receiver.target(...); end; end";
    let script = "puts Probe.new.call(factor:3).inspect";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    assert!(
        run.errors.iter().any(|e| e.contains("forwarding")),
        "expected native3; errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn keyword_normalization_uses_the_effective_last_class_definition() {
    let source = "class Probe; def self.call(factor: 2); factor; end; def self.call(...); target(...); end; def self.target(factor:); factor; end; def self.run; Probe.call(factor:3); end; end";
    let script = "raise unless Probe.run == 3; puts 'effective definition passed'";
    let native = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(native.status.success());
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "effective definition passed\n");
}

#[test]
fn runtime_source_retains_unrepresented_declaration_facts() {
    use roundhouse::dialect::UnsupportedFormal;
    for (source, expected) in [
        ("def call(*); 7; end", UnsupportedFormal::AnonymousRest),
        ("def call(**nil); 7; end", UnsupportedFormal::NoKeywords),
    ] {
        let methods = roundhouse::runtime_src::parse_methods(source).unwrap();
        assert_eq!(methods[0].unsupported_formals, Some(expected));
    }
}

#[test]
fn source_adapter_and_association_extension_retain_formal_facts() {
    use roundhouse::dialect::UnsupportedFormal;
    let source = roundhouse::ingest::ingest_library_class(
        b"class Probe; def self.call((a,b)); 7; end; end",
        "probe.rb",
    )
    .unwrap()
    .unwrap();
    let functions = roundhouse::lower::view_to_library::flatten_lcs_to_functions(&[source]);
    let restored = roundhouse::lower::module_funcs_to_library_class("Probe", &functions);
    assert_eq!(
        restored.methods[0].unsupported_formals,
        Some(UnsupportedFormal::Destructured)
    );

    let app = ingest_app_from_tree(HashMap::from([
        (PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord; has_many :comments do; def unpack((a,b)); 7; end; end; end".to_vec()),
        (PathBuf::from("app/models/comment.rb"), b"class Comment < ApplicationRecord; end".to_vec()),
    ])).unwrap();
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
    let method = lowered
        .methods
        .iter()
        .find(|m| m.name.as_str() == "comments_unpack")
        .unwrap();
    assert_eq!(
        method.unsupported_formals,
        Some(UnsupportedFormal::Destructured)
    );
    let (_, emitted) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(
            &app,
            roundhouse::fixtures::real_blog(),
            roundhouse::project::BuildTarget::Ruby,
        )
    });
    assert!(
        emitted
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("parameter declaration")),
        "{emitted:?}"
    );
}

fn native_result(source: &str, script: &str, expected: &str) {
    let output = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn anonymous_block_and_virtual_destination_losses_are_refused() {
    for (source, script, expected, refusal) in [
        (
            "class Probe; def self.leaf; yield; end; def self.target(&); __blk=7; leaf(&); end; def self.call(...); target(...); end; end",
            "puts Probe.call {23}",
            "23\n",
            "anonymous block",
        ),
        (
            "class Parent; def call(...); target(...); end; def target(a,b,factor:); (a-b)*factor; end; end; class Child < Parent; def target(a,b,factor:2); (a-b)*factor; end; end",
            "puts Child.new.call(11,4,factor:3)",
            "21\n",
            "subclass contract",
        ),
        (
            "module Relay; def call(...); target(...); end; def target(a,b,factor:); (a-b)*factor; end; end; class Child; include Relay; def target(a,b,factor:2); (a-b)*factor; end; end",
            "puts Child.new.call(11,4,factor:3)",
            "21\n",
            "subclass contract",
        ),
    ] {
        native_result(source, script, expected);
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", source)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains(refusal)),
            "{refusal}: {:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn anonymous_model_block_destination_stays_refused_after_parameter_retention() {
    let methods = "def self.leaf; yield; end; def self.target(&); __blk=7; leaf(&); end; def self.call(...); target(...); end";
    let script = "puts Article.call {23}";
    native_result(&format!("class Article; {methods}; end"), script, "23\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{methods}\n"),
        )
        .run_ruby(script);
    assert!(
        run.errors.iter().any(|e| e.contains("anonymous block")),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn named_block_spelling_and_no_forwarding_anonymous_block_keep_legacy_behavior() {
    let source = "class Probe; def self.leaf; yield; end; def self.named(&__blk); leaf(&__blk); end; def self.call(...); named(...); end; def self.ordinary(&); leaf(&); end; end";
    let script = "puts Probe.call {23}; puts Probe.ordinary {43}";
    native_result(source, script, "23\n43\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "23\n43\n");
}

#[test]
fn anonymous_block_source_fact_survives_adapters_extensions_and_runtime_ingest() {
    let class = roundhouse::ingest::ingest_library_class(
        b"class Probe; def self.call(&); 7; end; end",
        "probe.rb",
    )
    .unwrap()
    .unwrap();
    assert!(class.methods[0].has_anonymous_block);
    let functions = roundhouse::lower::view_to_library::flatten_lcs_to_functions(&[class]);
    assert!(functions[0].has_anonymous_block);
    let restored = roundhouse::lower::module_funcs_to_library_class("Probe", &functions);
    assert!(restored.methods[0].has_anonymous_block);
    let model = roundhouse::ingest::ingest_model(
        b"class Article < ApplicationRecord; has_many :comments do; def call(&); 7; end; end; end",
        "article.rb",
        &Default::default(),
        &Default::default(),
    )
    .unwrap()
    .unwrap();
    let lowered = roundhouse::lower::lower_model_to_library_class(&model, &Default::default());
    assert!(
        lowered
            .methods
            .iter()
            .find(|m| m.name.as_str() == "comments_call")
            .unwrap()
            .has_anonymous_block
    );
    assert!(
        roundhouse::runtime_src::parse_methods("def call(&); 7; end").unwrap()[0]
            .has_anonymous_block
    );
    assert!(
        !roundhouse::runtime_src::parse_methods("def call(&__blk); 7; end").unwrap()[0]
            .has_anonymous_block
    );
}

#[test]
fn association_lexical_contract_is_not_guessed_from_the_model_owner() {
    native_result(
        "module Extension; def target(a,b,*tail); a-b; end; def call(...); target(...); end; end; class Proxy; include Extension; end",
        "puts Proxy.new.call(11,4,9)",
        "7\n",
    );
    let methods = "def target(...); 695; end; has_many :comments do; def target(a,b,*tail); a-b; end; def call(...); target(...); end; end";
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{methods}\n"),
        )
        .run_ruby("puts Article.new.comments_call(11,4,9)");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("declaration cannot be verified")),
        "{:?}",
        run.errors
    );
}

#[test]
fn runtime_full_declarations_refuse_both_entry_paths_and_project_legacy_keywords() {
    assert!(
        roundhouse::runtime_src::parse_methods("def call(...); 7; end")
            .unwrap_err()
            .contains("full forwarding")
    );
    assert!(
        roundhouse::runtime_src::parse_library_with_rbs(
            b"class Probe; def call(...); 7; end; end",
            "class Probe\n def call: (*untyped) -> Integer\nend",
            "probe.rb"
        )
        .unwrap_err()
        .contains("full forwarding")
    );
    let method = roundhouse::runtime_src::parse_methods("def call(kw); target(**kw); end")
        .unwrap()
        .remove(0);
    let emitted = roundhouse::emit::ruby::emit_method(&method);
    assert!(emitted.contains("target(kw)"), "{emitted}");
    assert!(!emitted.contains("**kw"), "{emitted}");
}

#[test]
fn extend_module_super_forwarding_is_not_residual() {
    // Campfire WebPush::Connections::Stages: module extended onto Net::HTTP,
    // `super(...)` lands outside app source. Must not residual tip check.
    use std::collections::HashMap;
    use std::path::PathBuf;
    let files: [(&str, &str); 3] = [
        (
            "app/models/pool.rb",
            r#"class Pool
  module Stages
    def begin_transport(...)
      super
    end
    def connect(...)
      super(...)
    end
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = roundhouse::analyze::diagnose(&app);
    let forwards = diags
        .iter()
        .filter(|d| d.to_string().contains("full argument forwarding"))
        .map(|d| d.to_string())
        .collect::<Vec<_>>();
    assert!(
        forwards.is_empty(),
        "extend-module super(...) must not residual: {forwards:?}"
    );
}
