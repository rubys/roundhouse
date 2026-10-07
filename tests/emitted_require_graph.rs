//! Every `require_relative` in an emitted tree must resolve inside it.
//!
//! This is the guard for a defect that recurred five times in different
//! costumes: "which runtime files does target X need" was written down
//! in five places — `project::spinel_files`, both scaffold `main.rb`s,
//! `test/test_helper.rb`, and the two toolchain harnesses — and any two
//! of them could disagree without a single unit test noticing. The miss
//! only ever surfaced at RUN time, as `cannot load such file`, from
//! whichever consumer happened to load the most.
//!
//! A require edge is a fact about the emitted tree, so the tree can be
//! asked directly. No Ruby, no spinel, no fixture boot — just resolve
//! every edge against the file set that ships.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::{
    spinel_base_files, target_files, BuildTarget, RUBY_FAMILY_RUNTIME_CONSTANTS,
};

/// `require_relative "x/y"` occurrences in `content`, as the raw
/// argument text. Deliberately syntactic: a dynamic require is not a
/// static edge and is not this test's business.
fn require_relative_args(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, _) in content.match_indices("require_relative") {
        // Skip commented occurrences. Several runtime files quote their
        // own require line in a header comment (`runtime/base64.rb`,
        // `json.rb`, `gem_facades.rb`), which a naive scan reads as an
        // edge from the file to itself-under-itself.
        let line_start = content[..idx].rfind('\n').map_or(0, |p| p + 1);
        if content[line_start..idx].contains('#') {
            continue;
        }
        let rest = &content[idx + "require_relative".len()..];
        // Skip to the opening quote, but not past end-of-line — a bare
        // `require_relative` inside prose (this file's own comments, the
        // scaffold's) must not swallow the next line's string.
        let Some(line_end) = rest.find('\n') else { continue };
        let line = &rest[..line_end];
        let Some(open) = line.find('"') else { continue };
        let after = &line[open + 1..];
        let Some(close) = after.find('"') else { continue };
        let arg = &after[..close];
        // Interpolated or computed paths are not static edges.
        if arg.contains("#{") {
            continue;
        }
        out.push(arg.to_string());
    }
    out
}

/// Resolve `arg` (as written inside `from`'s directory) to a
/// tree-relative path, collapsing `..` segments.
fn resolve(from: &str, arg: &str) -> Option<String> {
    let base = Path::new(from).parent().unwrap_or(Path::new(""));
    let mut parts: Vec<String> = base
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    for seg in arg.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }
    Some(parts.join("/"))
}

fn assert_require_graph_closed(files: &[(String, String)], label: &str) {
    let present: BTreeSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();

    let mut missing: Vec<String> = Vec::new();
    for (path, content) in files {
        if !path.ends_with(".rb") {
            continue;
        }
        for arg in require_relative_args(content) {
            let Some(target) = resolve(path, &arg) else {
                missing.push(format!("{path}: `{arg}` escapes the tree root"));
                continue;
            };
            let rb = format!("{target}.rb");
            if !present.contains(rb.as_str()) && !present.contains(target.as_str()) {
                missing.push(format!("{path}: require_relative \"{arg}\" → {rb}"));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "{label}: {} unresolved require_relative edge(s) in the emitted tree.\n\
         Every one is a file some consumer will fail to load at run time.\n\n{}",
        missing.len(),
        missing.join("\n"),
    );
}

fn blog() -> (roundhouse::App, PathBuf) {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let mut app = ingest_app(&fixture).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);
    (app, fixture)
}

#[test]
fn ruby_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Ruby).expect("ruby target files");
    assert_require_graph_closed(&files, "ruby");
}

#[test]
fn jruby_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Jruby).expect("jruby target files");
    assert_require_graph_closed(&files, "jruby");
}

#[test]
fn spinel_base_require_graph_is_closed() {
    // The pre-`spin_shape` set — what `make spinel-test` drives, and the
    // one whose gap took `toolchain-spinel` down.
    let (app, fixture) = blog();
    let files = spinel_base_files(&app, &fixture).expect("spinel base files");
    assert_require_graph_closed(&files, "spinel");
}

#[test]
fn spinel_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Spinel).expect("spinel target files");
    assert_require_graph_closed(&files, "spinel (spin shape)");
}

/// A bundled library (`pathname`, `set`, `json`, …) that the tree names
/// but never requires.
///
/// Separate from the `require_relative` graph above and for the same
/// reason: the rule was written down once, inside `spin_shape`, so only
/// the spinel tree got it. Ruby 4.0 autoloads `Set` and `Pathname` and
/// hides the gap; Ruby 3.4 — what the scaffold claims and what
/// `campfire-conformance` runs — raises, and campfire lost two test
/// files to a `Pathname()` in a helper. Every ruby-family target is
/// checked here so the next target to be added is checked too.
fn assert_no_missing_bundled_requires(files: &[(String, String)], label: &str) {
    let gaps = roundhouse::project::missing_bundled_requires(files);
    assert!(
        gaps.is_empty(),
        "{label}: {} file(s) name a bundled-library constant with no require:\n{}",
        gaps.len(),
        gaps.join("\n"),
    );
}

#[test]
fn bundled_requires_are_written_for_every_target() {
    let (app, fixture) = blog();
    for (target, label) in [
        (BuildTarget::Ruby, "ruby"),
        (BuildTarget::Jruby, "jruby"),
        (BuildTarget::Spinel, "spinel"),
    ] {
        let files = target_files(&app, &fixture, target).expect("target files");
        assert_no_missing_bundled_requires(&files, label);
    }
}

#[test]
fn bundled_class_constants_are_ledgered_only_on_targets_without_them() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/controllers/probes_controller.rb", r#"class ProbesController < ActionController::Base
  def index
    [URI::HTTP, URI::InvalidURIError, Net::OpenTimeout, Net::ReadTimeout,
     Net::HTTPRedirection, Net::HTTPOK, StringIO, OpenSSL::OpenSSLError,
     Rails::HTML5::SafeListSanitizer, JSON, JSON::ParserError, Struct, Mutex,
     Queue, SizedQueue, Thread::Queue, Thread::SizedQueue, Thread::Mutex,
     ThreadError, ClosedQueueError, Comparable, Enumerable, JSON::GeneratorError]
  end
end
"#),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [
        BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel,
        BuildTarget::Go, BuildTarget::Rust, BuildTarget::Typescript,
        BuildTarget::TypescriptWorker, BuildTarget::Python, BuildTarget::Elixir,
        BuildTarget::Crystal, BuildTarget::Kotlin, BuildTarget::Swift, BuildTarget::CSharp,
    ] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        files.expect("target files");
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "bundled_constant"
        )).collect();
        if target == BuildTarget::Ruby {
            assert!(gaps.is_empty(), "{target:?}: {gaps:?}");
        } else if target == BuildTarget::Spinel {
            assert_eq!(gaps.len(), 1, "{gaps:?}");
            assert!(gaps[0].message.contains("JSON::GeneratorError"));
            assert_eq!(gaps[0].severity, roundhouse::diagnostic::Severity::Error);
            assert!(!gaps[0].span.is_synthetic(), "{gaps:?}");
        } else if target == BuildTarget::Jruby {
            assert_eq!(gaps.len(), 1, "{gaps:?}");
            assert!(gaps[0].message.contains("Rails::HTML5::SafeListSanitizer"));
            assert_eq!(gaps[0].severity, roundhouse::diagnostic::Severity::Error);
            assert!(!gaps[0].span.is_synthetic(), "{gaps:?}");
        } else {
            assert_eq!(gaps.len(), 23, "{target:?}: {gaps:?}");
            for name in ["URI::HTTP", "URI::InvalidURIError", "Net::OpenTimeout", "Net::ReadTimeout",
                "Net::HTTPRedirection", "Net::HTTPOK", "StringIO", "OpenSSL::OpenSSLError",
                "Rails::HTML5::SafeListSanitizer", "JSON", "JSON::ParserError", "Struct", "Mutex",
                "Queue", "SizedQueue", "Thread::Queue", "Thread::SizedQueue", "Thread::Mutex",
                "ThreadError", "ClosedQueueError", "Comparable", "Enumerable", "JSON::GeneratorError"] {
                let gap = gaps.iter().find(|d| d.message.contains(name)).expect(name);
                assert_eq!(gap.severity, roundhouse::diagnostic::Severity::Error);
                assert!(!gap.span.is_synthetic(), "{gap:?}");
                match &gap.kind {
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { target: Some(name), .. } => assert_eq!(name.as_str(), target.as_str()),
                    other => panic!("wrong target on {other:?}"),
                }
            }
        }
    }
}

/// `check` resolves the exception classes of
/// `runtime/ruby/action_controller/parameter_missing.rb` and
/// `runtime/ruby/action_view/missing_template.rb` for every target, but
/// only the ruby family and spinel ship those files. Each other target
/// reports every reference as an error, so the transpile fails instead
/// of emitting a class that does not exist.
#[test]
fn ruby_family_runtime_constants_are_ledgered_on_other_targets() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :probes, only: %i[index show]\nend\n"),
        ("app/controllers/probes_controller.rb", r#"class ProbesController < ActionController::Base
  def index
    raise ActionController::RoutingError.new("page out of bounds", []) if params[:page] == "0"
    raise ActionController::UnknownFormat.new("no format") if params[:page] == "1"
    raise ActionController::ParameterMissing.new("page") if params[:page] == "2"
    head :ok
  end

  def show
    begin
      head :ok
    rescue ActionController::UnpermittedParameters, ActionView::MissingTemplate
      head :not_found
    end
  end
end
"#),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [
        BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel,
        BuildTarget::Go, BuildTarget::Rust, BuildTarget::Typescript,
        BuildTarget::TypescriptWorker, BuildTarget::Python, BuildTarget::Elixir,
        BuildTarget::Crystal, BuildTarget::Kotlin, BuildTarget::Swift, BuildTarget::CSharp,
    ] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        files.expect("target files");
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "ruby_family_runtime_constant"
        )).collect();
        if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
            assert!(gaps.is_empty(), "{target:?}: {gaps:?}");
            continue;
        }
        assert_eq!(gaps.len(), RUBY_FAMILY_RUNTIME_CONSTANTS.len(), "{target:?}: {gaps:?}");
        let t = target.as_str();
        for name in RUBY_FAMILY_RUNTIME_CONSTANTS {
            let text = format!(
                "ruby_family_runtime_constant not supported ({t}): {name} is not available as a class/module value on {t}"
            );
            let gap = gaps.iter().find(|d| d.message == text).expect(&text);
            assert_eq!(gap.severity, roundhouse::diagnostic::Severity::Error);
            assert!(!gap.span.is_synthetic(), "{gap:?}");
        }
    }
}

/// Both Spinel dispatchers rescue `ActionController::RoutingError`
/// beside `ActiveRecord::RecordNotFound` and answer 404. The CRuby
/// overlay path is covered by
/// `emit_and_run::an_action_that_raises_routing_error_answers_404`; this
/// pins the native Spinel scaffold the same way without needing a
/// Spinel binary in the unit job.
#[test]
fn spinel_dispatchers_rescue_routing_error_as_404() {
    for path in [
        "runtime/spinel/scaffold/main.rb",
        "runtime/spinel/scaffold/ruby_overlay/main.rb",
    ] {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            src.contains("rescue ActiveRecord::RecordNotFound, ActionController::RoutingError"),
            "{path} must rescue RoutingError beside RecordNotFound:\n{src}"
        );
    }
}

/// An app that defines one of these classes itself ships the class, so
/// the reference is not an error on any target.
#[test]
fn an_app_defined_ruby_family_runtime_constant_is_not_ledgered() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :probes, only: %i[index]\nend\n"),
        ("app/models/action_controller/unknown_format.rb", "module ActionController\n  class UnknownFormat < StandardError\n  end\nend\n"),
        ("app/controllers/probes_controller.rb", r#"class ProbesController < ActionController::Base
  def index
    raise ActionController::UnknownFormat.new("no format") if params[:page] == "1"
    head :ok
  end
end
"#),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [BuildTarget::Typescript, BuildTarget::Rust] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        files.expect("target files");
        assert!(!diags.iter().any(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if matches!(construct.as_str(), "bundled_constant" | "ruby_family_runtime_constant")
        )), "{target:?}: {diags:?}");
    }
}

#[test]
fn spinel_json_generator_rescue_does_not_hide_class_values_in_its_branches() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/controllers/probes_controller.rb", r#"class ProbesController < ActionController::Base
  def index(value)
    begin
      JSON.generate(value)
    rescue JSON::GeneratorError
      JSON::GeneratorError
    rescue ::JSON::GeneratorError
      ::JSON::GeneratorError
    else
      JSON::GeneratorError
    ensure
      JSON::GeneratorError
    end
  end
end
"#),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, Path::new("."), target));
        files.expect("target files");
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "bundled_constant"
        )).collect();
        assert_eq!(gaps.len(), if target == BuildTarget::Spinel { 4 } else { 0 }, "{target:?}: {gaps:?}");
        for gap in gaps {
            assert!(gap.message.contains("JSON::GeneratorError"));
            assert_eq!(gap.severity, roundhouse::diagnostic::Severity::Error);
            assert!(!gap.span.is_synthetic(), "{gap:?}");
        }
    }
}

#[test]
fn bundled_constant_gate_follows_aliases_but_not_app_defined_classes() {
    for app_defines_class in [false, true] {
        let source = "class ProbesController < ActionController::Base\n  Timeout = Net::OpenTimeout\n  def index\n    Timeout\n  end\nend\n";
        let mut tree: std::collections::HashMap<_, _> = [
            (PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
            (PathBuf::from("app/controllers/probes_controller.rb"), source.as_bytes().to_vec()),
        ].into_iter().collect();
        if app_defines_class {
            tree.insert(PathBuf::from("app/models/net/open_timeout.rb"), b"module Net\n  class OpenTimeout\n  end\nend\n".to_vec());
        }
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
        let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
        analysis.extend(roundhouse::analyze::diagnose(&app));
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), BuildTarget::Go).expect("target files")
        });
        let rejected = analysis.iter().chain(&diags).any(|d| {
            d.severity == roundhouse::diagnostic::Severity::Error && matches!(
                &d.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if matches!(construct.as_str(), "constant" | "bundled_constant")
            )
        });
        assert_eq!(rejected, !app_defines_class, "app-defined={app_defines_class}: {analysis:?}; {diags:?}");
    }
}

#[test]
fn an_app_model_is_not_mistaken_for_a_bundled_class() {
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :open_timeouts do |t|\n    t.string :name\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/net/open_timeout.rb", "module Net\n  class OpenTimeout < ActiveRecord::Base\n  end\nend\n"),
        ("app/controllers/probes_controller.rb", "class ProbesController < ActionController::Base\n  def index\n    Net::OpenTimeout\n  end\nend\n"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    assert!(app.models.iter().any(|model| model.name.0.as_str() == "Net::OpenTimeout"));
    let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
        target_files(&app, Path::new("."), BuildTarget::Go).expect("target files")
    });
    assert!(!diags.iter().any(|d| matches!(
        &d.kind,
        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "bundled_constant"
    )), "{diags:?}");
}

#[test]
fn mapped_json_receivers_do_not_hide_unsupported_arguments_or_methods() {
    for (body, rejected) in [
        ("JSON.generate(42)", false),
        ("JSON.generate(Net::HTTPOK)", true),
        ("JSON.parse(\"42\")", true),
        ("JSON", true),
    ] {
        let tree = [
            (PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
            (PathBuf::from("app/controllers/probes_controller.rb"), format!("class ProbesController < ActionController::Base\n  def index\n    {body}\n  end\nend\n").into_bytes()),
        ].into_iter().collect();
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), BuildTarget::Go).expect("target files")
        });
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "bundled_constant"
        )).collect();
        assert_eq!(!gaps.is_empty(), rejected, "{body}: {gaps:?}");
        if body == "JSON.generate(Net::HTTPOK)" {
            assert_eq!(gaps.len(), 1, "{gaps:?}");
            assert!(gaps[0].message.contains("Net::HTTPOK"));
        }
    }
}

/// A superclass reference to a ruby-family runtime exception is
/// ledgered on strict targets (parent Const span from ingest).
#[test]
fn a_superclass_ruby_family_runtime_constant_is_ledgered() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :probes, only: %i[index]\nend\n"),
        ("app/models/page_out_of_bounds.rb", "class PageOutOfBounds < ActionController::RoutingError\nend\n"),
        ("app/controllers/probes_controller.rb", "class ProbesController < ActionController::Base\n  def index\n    raise PageOutOfBounds.new(\"page out of bounds\") if params[:page] == \"0\"\n    head :ok\n  end\nend\n"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [BuildTarget::Typescript, BuildTarget::Rust] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "ruby_family_runtime_constant"
        ) && d.message.contains("ActionController::RoutingError")).collect();
        assert_eq!(gaps.len(), 1, "{target:?}: {gaps:?} / {diags:?}");
        assert!(!gaps[0].span.is_synthetic(), "{gaps:?}");
    }
}

/// Test-module inner classes also carry parent Const spans — the gate
/// must scan them, not only app models/controllers/library classes.
#[test]
fn a_test_inner_class_superclass_ruby_family_runtime_constant_is_ledgered() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"probes#index\"\nend\n"),
        ("app/controllers/probes_controller.rb", "class ProbesController < ActionController::Base\n  def index\n    head :ok\n  end\nend\n"),
        ("test/models/page_error_test.rb", "require \"test_helper\"\n\nclass PageErrorTest < ActiveSupport::TestCase\n  class PageOutOfBounds < ActionController::RoutingError\n  end\n\n  test \"placeholder\" do\n    assert true\n  end\nend\n"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    assert!(
        app.test_modules.iter().any(|m| m.inner_classes.iter().any(|c| {
            c.name.0.as_str().contains("PageOutOfBounds")
                && c.parent.as_ref().is_some_and(|p| p.0.as_str() == "ActionController::RoutingError")
                && !c.parent_span.is_synthetic()
        })),
        "expected ingested test inner class with non-synthetic parent_span: {:?}",
        app.test_modules.iter().map(|m| &m.inner_classes).collect::<Vec<_>>(),
    );
    for target in [BuildTarget::Typescript, BuildTarget::Rust] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "ruby_family_runtime_constant"
        ) && d.message.contains("ActionController::RoutingError")).collect();
        assert_eq!(gaps.len(), 1, "{target:?}: {gaps:?} / {diags:?}");
        assert!(!gaps[0].span.is_synthetic(), "{gaps:?}");
    }
}

/// A lowers-added MissingTemplate raise (missing render) is typed so
/// the availability gate sees it on strict targets.
#[test]
fn a_lowering_added_missing_template_is_ledgered_on_strict_targets() {
    let tree = [
        ("config/routes.rb", "Rails.application.routes.draw do
  resources :probes, only: %i[show]
end
"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base
end
"),
        ("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController
  def show
    render :missing
  end
end
"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let mut analysis = roundhouse::session::analyze_and_lower(&mut app);
    analysis.extend(roundhouse::analyze::diagnose(&app));
    assert!(!analysis.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error), "{analysis:?}");
    for target in [BuildTarget::Typescript, BuildTarget::Go] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "ruby_family_runtime_constant"
        ) && d.message.contains("ActionView::MissingTemplate")).collect();
        assert_eq!(gaps.len(), 1, "{target:?}: expected one MissingTemplate ledger, got {diags:?}");
        assert!(!gaps[0].span.is_synthetic(), "{gaps:?}");
    }
}

/// Subclass-template-hook raises (campfire `MessagesController` shape)
/// must reuse the `render` call span so the gate ledgers them — not
/// `Span::synthetic()`, which would skip while emit still throws.
#[test]
fn a_subclass_template_hook_missing_template_is_ledgered_on_strict_targets() {
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do
  create_table \"messages\", force: :cascade do |t|
    t.string \"body\", null: false
  end
end
"),
        ("config/routes.rb", "Rails.application.routes.draw do
  resources :messages, only: %i[update]
  scope path: \":bot_key\", as: :bot, defaults: { format: :json } do
    resources :messages, controller: \"messages/by_bots\", only: %i[update]
  end
end
"),
        ("app/models/message.rb", "class Message < ApplicationRecord
end
"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base
end
"),
        ("app/controllers/messages_controller.rb", "class MessagesController < ApplicationController
  before_action :set_message

  def update
    @message.update!(body: params[:body])

    respond_to do |format|
      format.html { redirect_to message_url(@message) }
      format.json { render :show }
    end
  end

  private
    def set_message
      @message = Message.find(params[:id])
    end
end
"),
        ("app/controllers/messages/by_bots_controller.rb", "class Messages::ByBotsController < MessagesController
end
"),
        ("app/views/messages/by_bots/show.json.jbuilder", "json.id @message.id
json.body @message.body
"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    // Campfire-shaped fixture: jbuilder `@message` may still carry
    // ivar_unresolved residue. This pin is the gate ledger, not clean typing.
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Typescript, BuildTarget::Go] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, Path::new("."), target)
        });
        let gaps: Vec<_> = diags.iter().filter(|d| matches!(
            &d.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "ruby_family_runtime_constant"
        ) && d.message.contains("ActionView::MissingTemplate")).collect();
        assert!(!gaps.is_empty(), "{target:?}: expected subclass-hook MissingTemplate ledger, got {diags:?}");
        assert!(gaps.iter().all(|g| !g.span.is_synthetic()), "{gaps:?}");
    }
}

