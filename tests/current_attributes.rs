//! `class Current < ActiveSupport::CurrentAttributes` becomes a plain,
//! statically-resolvable class (`ingest::current_attributes`).
//!
//! Rails stacks three pieces of metaprogramming here — `attribute`
//! defining accessors into a generated module, class-level `Current.user`
//! arriving via `method_missing`, and an app's own writer reaching the
//! generated one through `super`. An emitted tree has none of it, and a
//! statically-resolved target cannot follow `method_missing` at all.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn current() -> String {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        (
            "app/models/current.rb",
            r#"class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user, :request

  delegate :host, :protocol, to: :request, prefix: true, allow_nil: true

  def session=(value)
    super(value)

    if value.present?
      self.user = session.user
    end
  end

  def account
    Account.first
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("current.rb"))
        .map(|f| f.content.clone())
        .expect("current.rb")
}

/// The base is gone — nothing in an emitted tree provides it.
#[test]
fn the_current_attributes_base_is_dropped() {
    let c = current();
    assert!(!c.contains("ActiveSupport::CurrentAttributes"), "{c}");
    assert!(c.contains("class Current\n"), "{c}");
}

/// `attribute :session, :user, :request` becomes real ivar accessors.
#[test]
fn declared_attributes_become_accessors() {
    let c = current();
    for m in ["def user\n    @user", "def user=(value)\n    @user = value", "def request\n    @request"] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}

/// The app writes its OWN `session=`, so only the reader is synthesized —
/// and its `super(value)` becomes the storage write it means, rather than
/// needing a module for `super` to find.
#[test]
fn an_app_written_writer_keeps_its_body_and_loses_super() {
    let c = current();
    assert!(!c.contains("super("), "no super survives:\n{c}");
    assert!(
        c.contains("def session=(value)\n    @session = value"),
        "super became the storage write:\n{c}"
    );
    assert_eq!(c.matches("def session=").count(), 1, "not double-defined:\n{c}");
}

/// `delegate … prefix: true, allow_nil: true` — the nil guard is the
/// whole point of `allow_nil`.
#[test]
fn delegate_expands_with_its_nil_guard() {
    let c = current();
    assert!(c.contains("def request_host"), "prefixed name:\n{c}");
    assert!(c.contains("@request.nil?"), "allow_nil guard:\n{c}");
}

/// The surface app code actually calls. Rails supplies these through
/// `method_missing`; a statically-resolved target needs them real —
/// including for a method the app defined itself (`account`).
#[test]
fn class_level_forwarders_exist_for_every_instance_method() {
    let c = current();
    for m in [
        "def self.user\n    Current.instance.user",
        "def self.user=(value)\n    Current.instance.user = value\n    value",
        "def self.account\n    Current.instance.account",
        "def self.request_host",
        "def self.instance",
        "def self.reset",
    ] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}

/// `attribute :session, :user` on CurrentAttributes is the class-body
/// form apps actually call (`Current.session =`, `Current.user`). The
/// app-written writer still reaches storage through rewritten `super`.
#[test]
fn emitted_current_attributes_session_and_user_run() {
    emit_and_run::real_blog()
        .write(
            "app/models/current.rb",
            r#"class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user

  def session=(value)
    super(value)

    if value.present?
      self.user = session.user
    end
  end
end
"#,
        )
        .run_ruby(
            r#"
session = Object.new
def session.user
  "ada"
end
Current.session = session
raise "user from session writer" unless Current.user == "ada"
raise "session stored" unless Current.session.equal?(session)
Current.reset
raise "reset user" unless Current.user.nil?
puts "current attributes passed"
"#,
        )
        .assert_passes();
}

// Execute the emitted library, not a rewritten Rails receiver or a mock
// Current implementation. The consumer stays outside the ingested tree.
fn execute_current(source: &str, contract: &str) {
    let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", source)]))
        .expect("ingest Current contract");
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = lower.into_iter().chain(roundhouse::analyze::diagnose(&app))
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let files = ruby::emit_library(&app);
    let script = format!("{}\n{contract}", files.iter().filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content.as_str()).collect::<Vec<_>>().join("\n"));
    for (flag, yjit) in [("--disable-yjit", false), ("--yjit", true)] {
        let script = format!("raise 'child YJIT mismatch' unless RubyVM::YJIT.enabled? == {yjit}\n{script}");
        let output = std::process::Command::new("ruby").args([flag, "-e", &script]).output().expect("ruby");
        assert!(output.status.success(), "{flag}: {}\n{}\n{script}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        println!("emitted Current contract: child YJIT={yjit} PASS");
    }
}

const SCOPED_CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user, :trace, :fail_get, :fail_write

  def session=(value)
    trace << ["session=", value]
    super(value)
    self.user = value unless value.nil?
  end

  def user
    trace << ["user", fail_get]
    raise "getter" if fail_get
    super
  end

  def session
    raise "first getter" if fail_get == "session"
    super
  end

  def user=(value)
    trace << ["user=", value]
    super(value)
    raise "writer" if value == fail_write
  end
end

class ScopeProbe
  def self.rhs(value, fail)
    Current.trace << ["rhs", value]
    raise "rhs" if fail
    value
  end

  def self.run(first, second, fail_rhs)
    Current.set(session: rhs(first, false), user: rhs(second, fail_rhs)) do |receiver|
      receiver.trace << ["block", receiver.user]
      "result"
    end
  end

  def self.returning
    Current.set(session: "new") { return "returned" }
    "unreachable"
  end

  def self.breaking
    Current.set(session: "new") { break "broken" }
  end

  def self.nexting
    Current.set(session: "new") { next "nexted" }
  end

  def self.raising
    Current.set(session: "new", user: "explicit") { raise "block" }
  end

  def self.hygiene(__rh_current_instance)
    __current_set_user = "sentinel"
    Current.set(user: "explicit") { [__rh_current_instance, __current_set_user] }
  end
end
"#;

const SEQUENTIAL_SAVE_CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user
  def session=(value)
    super(value)
    self.user = value
  end
end
class ScopeProbe
  def self.run
    Current.set(session: "new", user: Current.user) { Current.user }
  end
end
"#;

const SEQUENTIAL_SAVE_ASSERTIONS: &str = r#"
Current.session = "old"
Current.user = "override"
raise "block must run with pre-evaluated RHS" unless ScopeProbe.run == "override"
raise "sequential save" unless Current.session == "old" && Current.user == "new"
"#;

#[test]
fn current_set_evaluates_rhs_before_writers_and_saves_after_prior_writer_side_effects() {
    execute_current(SEQUENTIAL_SAVE_CURRENT, SEQUENTIAL_SAVE_ASSERTIONS);
}

/// The generated class setter must return its argument after one writer
/// dispatch. An empty array is the RHS Spinel infers as PolyArray when
/// the forwarder ends on the assignment send.
const SETTER_RETURN_CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user, :trace

  def session=(value)
    super(value)
    self.user = value unless value.nil?
  end

  def trace=(value)
    super(value)
    self.user = value.object_id
  end
end
"#;

const SETTER_RETURN_ASSERTIONS: &str = r#"
trace = []
returned = (Current.trace = trace)
raise "setter result" unless returned.equal?(trace) && Current.trace.equal?(trace)
raise "setter dispatch" unless Current.user == trace.object_id
Current.session = "old"
raise "coupled setter" unless Current.user == "old"
"#;

const SCOPED_ASSERTIONS: &str = r#"
Current.trace = []
Current.session = "old"
Current.user = "override"
Current.trace.clear
raise "block result" unless ScopeProbe.run("new", "explicit", false) == "result"
expected = [["rhs", "new"], ["rhs", "explicit"], ["session=", "new"],
  ["user=", "new"], ["user", nil], ["user=", "explicit"], ["user", nil],
  ["block", "explicit"], ["session=", "old"], ["user=", "old"], ["user=", "new"]]
raise "sequential save/set: #{Current.trace.inspect}" unless Current.trace == expected
raise "restore side effect" unless Current.session == "old" && Current.user == "new"
raise "return" unless ScopeProbe.returning == "returned"
raise "break" unless ScopeProbe.breaking == "broken"
raise "next" unless ScopeProbe.nexting == "nexted"
raise "hygiene" unless ScopeProbe.hygiene("argument") == ["argument", "sentinel"]
raise "restore after control flow" unless Current.session == "old" && Current.user == "old"
"#;

#[test]
fn scoped_current_preserves_sequential_save_set_result_receiver_and_control_flow() {
    execute_current(SCOPED_CURRENT, SCOPED_ASSERTIONS);
}

const RESTORE_ASSERTIONS: &str = r#"
def prepare
  Current.fail_get = nil
  Current.fail_write = nil
  Current.trace = []
  Current.session = "old"
  Current.user = "override"
  Current.trace.clear
end
def expect_error(message)
  begin
    yield
  rescue RuntimeError => error
    raise "wrong error: #{error}" unless error.message == message
    return
  end
  raise "missing #{message}"
end
prepare
expect_error("rhs") { ScopeProbe.run("new", "explicit", true) }
raise "RHS changed state" unless Current.trace == [["rhs", "new"], ["rhs", "explicit"]]
raise "RHS state" unless Current.session == "old" && Current.user == "override"

prepare
Current.fail_get = "session"
expect_error("first getter") { ScopeProbe.run("new", "explicit", false) }
raise "first getter changed state" unless Current.trace == [["rhs", "new"], ["rhs", "explicit"]]
Current.fail_get = nil
raise "first getter state" unless Current.session == "old" && Current.user == "override"

prepare
Current.fail_get = true
expect_error("getter") { ScopeProbe.run("new", "explicit", false) }
raise "getter restore" unless Current.trace == [["rhs", "new"], ["rhs", "explicit"],
  ["session=", "new"], ["user=", "new"], ["user", true], ["session=", "old"], ["user=", "old"]]
Current.fail_get = nil
raise "unsaved user restored" unless Current.user == "old"

prepare
Current.fail_write = "explicit"
expect_error("writer") { ScopeProbe.run("new", "explicit", false) }
raise "setter restore" unless Current.trace == [["rhs", "new"], ["rhs", "explicit"],
  ["session=", "new"], ["user=", "new"], ["user", nil], ["user=", "explicit"],
  ["session=", "old"], ["user=", "old"], ["user=", "new"]]
raise "setter state" unless Current.session == "old" && Current.user == "new"

prepare
Current.fail_write = "new"
expect_error("writer") { ScopeProbe.run("new", "explicit", false) }
raise "first setter restore" unless Current.trace == [["rhs", "new"], ["rhs", "explicit"],
  ["session=", "new"], ["user=", "new"], ["session=", "old"], ["user=", "old"]]

prepare
expect_error("block") { ScopeProbe.raising }
raise "block exception state" unless Current.session == "old" && Current.user == "new"

prepare
Current.fail_write = "old"
expect_error("writer") { ScopeProbe.run("new", "explicit", false) }
raise "restore must stop" unless Current.trace.last(2) == [["session=", "old"], ["user=", "old"]]
raise "restore state" unless Current.session == "old" && Current.user == "old"
"#;

#[test]
fn scoped_current_restores_only_saved_slots_in_order_even_when_setup_raises() {
    execute_current(SCOPED_CURRENT, RESTORE_ASSERTIONS);
}

const RESET_CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :user
  def user=(value)
    raise "custom writer" if value.nil?
    super(value)
  end
end
"#;

const RESET_ASSERTIONS: &str = r#"
Current.user = "ada"
instance = Current.instance
instance.instance_variable_set(:@unrelated, 37)
result = Current.reset
raise "instance replaced" unless Current.instance.equal?(instance)
raise "declared storage" unless Current.user.nil?
raise "unrelated state" unless instance.instance_variable_get(:@unrelated) == 37
raise "reset return" unless result == {}
raise "custom writer" unless (Current.user = "ada") == "ada"
"#;

#[test]
fn reset_keeps_the_instance_and_unrelated_state_without_calling_custom_writers() {
    // TracePoint is an external CRuby observation, not part of the
    // emitted/native contract. Keep the constructor-count regression.
    execute_current(RESET_CURRENT, &format!(r#"
Current.user = "ada"
constructions = 0
trace = TracePoint.new(:c_call) do |event|
  constructions += 1 if event.method_id == :new && event.self == Current
end
trace.enable
{RESET_ASSERTIONS}
trace.disable
raise "constructor rerun" unless constructions == 0
"#));
}

const NAMESPACED_CURRENT: &str = r#"
module Dash
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
  class ScopeProbe
    def self.run
      Current.set(user: "outer") do |outer|
        inner = Current.set(user: "inner") { |receiver| receiver.user }
        [inner, outer.user]
      end
    end
  end
end
module Other
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
end
"#;

const NAMESPACED_ASSERTIONS: &str = r#"
Dash::Current.user = "dash"
Other::Current.user = "other"
raise "nested values" unless Dash::ScopeProbe.run == ["inner", "outer"]
raise "nested restore" unless Dash::Current.user == "dash"
raise "namespace isolation" unless Other::Current.user == "other"
dash = Dash::Current.instance
Dash::Current.reset
raise "namespaced reset" unless Dash::Current.instance.equal?(dash) && Dash::Current.user.nil?
raise "other reset" unless Other::Current.user == "other"
"#;

#[test]
fn namespaced_and_nested_current_scopes_do_not_share_storage_or_temps() {
    execute_current(NAMESPACED_CURRENT, NAMESPACED_ASSERTIONS);
}

#[test]
fn current_defaults_and_reset_callbacks_are_errors_not_silently_erased() {
    for declaration in [
        "attribute :user, default: 'guest'", "attribute :user, default: -> { 'guest' }",
        "attribute :user, default: nil", "before_reset { notify }", "after_reset :notify",
        "resets { notify }", "set_callback :reset, :before, :notify",
    ] {
        let source = format!("class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  {declaration}\nend\n");
        let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", &source)])).expect("ingest");
        let lower = roundhouse::session::analyze_and_lower(&mut app);
        let errors: Vec<_> = lower.into_iter().chain(roundhouse::analyze::diagnose(&app))
            .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error).collect();
        assert!(errors.iter().any(|d| d.message.contains("CurrentAttributes") &&
            (d.message.contains("default") || d.message.contains("callback"))), "{declaration}: {errors:?}");
    }
}

const GATED_SCOPE: &str = r#"class Current < ActiveSupport::CurrentAttributes
  attribute :user
end
class ScopeProbe
  def self.run
    Current.set(user: "new") { "body" }
  end
end
"#;

const SCOPE_SHADOWS: &[&str] = &[
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user, :then\nend\n",
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  def then\n    'hijacked'\n  end\nend\n",
    "module Shadow\n  def then\n    'hijacked'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Shadow\nend\n",
    "module Inner\n  def then\n    'hijacked'\n  end\nend\nmodule Outer\n  include Inner\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Outer\nend\n",
    "class Object\n  def then\n    'hijacked'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\nend\n",
    "module Kernel\n  def then\n    'hijacked'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\nend\n",
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include ExternalScope\nend\n",
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  def set(attributes)\n    'custom-set'\n  end\nend\n",
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  def self.set(attributes)\n    'custom-set'\n  end\nend\n",
    "class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  def with(user: nil)\n    'custom-with'\n  end\nend\n",
    "module Shadow\n  def set(attributes)\n    'custom-set'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Shadow\nend\n",
    "module Shadow\n  def marker\n    'first'\n  end\nend\nmodule Shadow\n  def then\n    'hijacked'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Shadow\nend\n",
    "module Shadow\n  def marker\n    'first'\n  end\nend\nmodule Shadow\n  def with(user: nil)\n    'custom-with'\n  end\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Shadow\nend\n",
    "module Inner\n  def set(attributes)\n    'custom-set'\n  end\nend\nmodule Outer\n  def marker\n    'first'\n  end\nend\nmodule Outer\n  include Inner\nend\nclass Current < ActiveSupport::CurrentAttributes\n  attribute :user\n  include Outer\nend\n",
];

#[test]
fn current_scopes_refuse_source_dispatch_overrides_and_reopened_includes() {
    for declaration in SCOPE_SHADOWS {
        let source = format!("{declaration}class ScopeProbe\n  def self.run\n    Current.set(user: 'new') {{ 'body' }}\n  end\nend\n");
        let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", &source)])).expect("ingest");
        let lower = roundhouse::session::analyze_and_lower(&mut app);
        let diagnostic = lower.iter().find(|d| d.severity == roundhouse::diagnostic::Severity::Error
            && d.message.contains("then") && d.message.contains("CurrentAttributes"))
            .unwrap_or_else(|| panic!("source scope override accepted: {source}\n{lower:?}"));
        let source_file = &app.sources[diagnostic.span.file.0 as usize - 1];
        assert_eq!(&source_file.text[diagnostic.span.start as usize..diagnostic.span.end as usize],
            "Current.set(user: 'new') { 'body' }");
    }
}

#[test]
fn current_reset_super_refuses_removing_the_inherited_lifecycle() {
    for (receiver, reopen) in [("reset", false), ("self.reset", false), ("reset", true), ("self.reset", true)] {
        let source = format!("class Current < ActiveSupport::CurrentAttributes\n  attribute :user\n{}  def {receiver}\n    super\n  end\nend\n",
            if reopen { "end\nclass Current\n" } else { "" });
        let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", &source)])).expect("ingest");
        let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
        let error = diagnostics.iter().chain(roundhouse::analyze::diagnose(&app).iter())
            .find(|d| d.severity == roundhouse::diagnostic::Severity::Error
                && d.message.contains("CurrentAttributes reset"))
            .cloned().unwrap_or_else(|| panic!("reset superclass silently removed: {source}\n{diagnostics:?}"));
        let file = &app.sources[error.span.file.0 as usize - 1];
        assert_eq!(&file.text[error.span.start as usize..error.span.end as usize], "super");
    }
}

#[test]
fn current_scope_provenance_keeps_the_original_call_location() {
    fn collect(expr: &roundhouse::expr::Expr, spans: &mut Vec<roundhouse::span::Span>) {
        if expr.decisions & roundhouse::expr::CURRENT_SET_SCOPE != 0 { spans.push(expr.span); }
        expr.node.for_each_child(&mut |child| collect(child, spans));
    }
    let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", GATED_SCOPE)])).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let mut spans = Vec::new();
    for method in app.library_classes.iter().flat_map(|c| &c.methods) {
        collect(&method.body, &mut spans);
    }
    assert_eq!(spans.len(), 1);
    let span = spans[0];
    let source = &app.sources[span.file.0 as usize - 1];
    assert_eq!(source.path, "app/models/current.rb");
    assert_eq!(&source.text[span.start as usize..span.end as usize],
        "Current.set(user: \"new\") { \"body\" }");
}

#[test]
fn unverified_targets_refuse_current_scopes_before_emitting_unknown_then_sends() {
    use roundhouse::project::BuildTarget;
    let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", GATED_SCOPE)])).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for &target in BuildTarget::TRANSPILE {
        // Spinel is admitted only after the ignored native contract below
        // compiles and runs the freshly emitted project. Every other
        // unverified target stays refused.
        if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Futamura | BuildTarget::Spinel) { continue; }
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-api"), target)
        });
        assert!(files.is_err(), "{} emitted an unverified Current scope", target.as_str());
        assert!(diags.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error
            && d.message.contains("CurrentAttributes") && !d.span.is_synthetic()),
            "{}: {diags:?}", target.as_str());
    }
}

/// Fresh compiler emission, not a retained library rewrite. `empty_app`
/// copies the overlay and `run_spinel` calls `project::target_files` for
/// Spinel, then compiles that unchanged tree. The consumer is the same
/// public contract the CRuby tests already run, including the empty-array
/// setter Spinel rejected when the generated forwarder ended on the
/// assignment send.
#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn emitted_current_contracts_execute_in_native_spinel() {
    // Each contract is its own fresh `target_files` tree. Wrapping the
    // shared top-level scripts in one module changes `def` and constant
    // lookup, and a later contract then observes a nil Current.
    let contracts = [
        ("app/models/current.rb", SETTER_RETURN_CURRENT, SETTER_RETURN_ASSERTIONS),
        ("app/models/current.rb", SEQUENTIAL_SAVE_CURRENT, SEQUENTIAL_SAVE_ASSERTIONS),
        ("app/models/current.rb", SCOPED_CURRENT, SCOPED_ASSERTIONS),
        ("app/models/current.rb", SCOPED_CURRENT, RESTORE_ASSERTIONS),
        ("app/models/current.rb", RESET_CURRENT, RESET_ASSERTIONS),
        ("app/models/current.rb", NAMESPACED_CURRENT, NAMESPACED_ASSERTIONS),
    ];
    for (path, source, assertions) in contracts {
        emit_and_run::empty_app()
            .write(path, source)
            .run_spinel(assertions)
            .assert_passes();
    }
}
