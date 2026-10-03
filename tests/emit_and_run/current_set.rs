//! Generated-project proof for literal Current.set scopes.
//! Expected values are the Rails `Object#with` contract; the emitted
//! project does not load ActiveSupport.

#[test]
fn literal_current_set_runs_in_the_generated_ruby_project() {
    const CURRENT: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :user, :account, :mode
  TRACE = ["seed"]
  def user=(value)
    TRACE << "user"
    super(value)
    self.account = "side" if value == 83 && (mode == :side || mode == :read)
    raise "restore" if value == 47 && mode == :restore
  end
  def account=(value)
    TRACE << "account"
    super(value)
    @observed_account = value
    raise "setup" if value == "bad"
  end
  def account
    raise "read" if user == 83 && mode == :read
    @observed_account
  end
end
"#;
    const PROBE: &str = r#"
class RoundhouseCurrentSetScope1
  def self.marker
    "app-owned"
  end
end
class ScopeProbe
  RoundhouseCurrentSetScope2 = 191
  def self.seed
    Current.mode = :none
    Current.user = 47
    Current.account = "Z"
  end
  def self.expression
    ::Current.set(user: 83) { 31 }
  end
  def self.argument
    [::Current.set(user: 83) { Current.user }, Current.user]
  end
  def self.body
    seen = 0
    ::Current.set(user: 83) { seen = Current.user }
    seen
  end
  def self.shadow
    context = 7
    result = ::Current.set(user: 83) { |context| context.user }
    [result, context]
  end
  def self.two_params
    extra = "outer"
    result = ::Current.set(user: 83) { |context, extra| [context.user, extra] }
    [result, extra]
  end
  def self.next_value
    ::Current.set(user: 83) { next 41; raise "unreachable" }
  end
  def self.break_value
    ::Current.set(user: 83) { break 59; raise "unreachable" }
  end
  def self.method_return
    ::Current.set(user: 83) { return [31, Current.user] }
    raise "unreachable"
  end
  def self.guarded_return(flag)
    ::Current.set(user: 83) { return if flag; 31 }
    99
  end
  def self.nested_guarded_return
    ::Current.set(user: 83) { if true; return if true; 31; end }
    99
  end
  def self.nested_block_guarded_return
    ::Current.set(user: 83) do
      ::Current.set(user: 109) { return if true; 31 }
      99
    end
    131
  end
  def self.nil_value
    ::Current.set(user: nil) { |context| context.user }
  end
  def self.identity
    ::Current.set(user: 83) { |context| context.equal?(Current.instance) }
  end
  def self.nested
    seen = [Current.user]
    ::Current.set(user: 83) do
      seen << Current.user
      ::Current.set(user: 109) { seen << Current.user }
      seen << Current.user
    end
    seen
  end
  def self.nested_exit
    result = ::Current.set(user: 83) do
      ::Current.set(user: 109) { break 67 }
      Current.user
    end
    result
  end
  def self.nested_each
    seen = []
    result = ::Current.set(user: 83) do
      [2, 5].each { |n| seen << n + Current.user }
      seen
    end
    [result, Current.user]
  end
  def self.downstream
    ::Current.set(user: 83) { 29 }.positive?
  end
  def self.ordered
    ::Current.set(user: first_rhs, account: second_rhs) { |context| [context.user, context.account] }
  end
  def self.first_rhs
    Current::TRACE << "first"
    83
  end
  def self.second_rhs
    Current::TRACE << "second"
    "C"
  end
  def self.rhs_failure
    ::Current.set(user: first_rhs, account: raising_rhs) { raise "body" }
  end
  def self.raising_rhs
    Current::TRACE << "rhs-error"
    raise "rhs"
  end
  def self.writer_effect
    ::Current.set(user: 83, account: "C") { |context| [context.user, context.account] }
  end
  def self.setup_failure
    ::Current.set(user: 83, account: "bad") { raise "body" }
  end
  def self.reader_failure
    ::Current.set(user: 83, account: "C") { raise "body" }
  end
  def self.restore_failure
    ::Current.set(user: 83, account: "C") { 31 }
  end
  def self.body_failure
    ::Current.set(user: 83) { raise "body" }
  end
  def self.hygiene
    __context = 7
    __value0 = 11
    __previous0 = 13
    __saved0 = 17
    result = ::Current.set(user: 83) { [__context, __value0, __previous0, __saved0] }
    [result, RoundhouseCurrentSetScope1.marker, RoundhouseCurrentSetScope2]
  end
end
module ScopeContext
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
end
module OtherScopeContext
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
end
class AScopedBootProbe
  VALUE = ::Current.set(user: 83) { 31 }
end
class ABootNamespaceProbe
  VALUE = ::ScopeContext::Current.set(user: 181) { 41 }
end
class NamespaceProbe
  def self.seed
    ScopeContext::Current.user = 151
    OtherScopeContext::Current.user = 229
  end
  def self.run
    ::ScopeContext::Current.set(user: 181) { |context| context.user }
  end
  def self.other
    ::OtherScopeContext::Current.set(user: 257) { |context| context.user }
  end
end
"#;
    // Independent literal expectations derived from Rails Object#with,
    // not generated output. Execute these against pinned Rails first.
    const ASSERTIONS: &str = r##"
def check(name, expected, actual)
  raise "#{name}: expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
def fresh
  ScopeProbe.seed
  Current::TRACE.clear
end
def error_from
  yield
  "did not raise"
rescue RuntimeError => error
  error.message
end
check("eager root and namespaced dependency", [31, nil, 41, nil], [AScopedBootProbe::VALUE, ::Current.user, ABootNamespaceProbe::VALUE, ::ScopeContext::Current.user])
fresh; check("expression", [31, 47], [ScopeProbe.expression, Current.user])
fresh; check("argument", [83, 47], ScopeProbe.argument)
fresh; check("body", [83, 47], [ScopeProbe.body, Current.user])
fresh; check("shadow", [[83, 7], 47], [ScopeProbe.shadow, Current.user])
fresh; check("extra required nil", [[[83, nil], "outer"], 47], [ScopeProbe.two_params, Current.user])
fresh; check("next", [41, 47], [ScopeProbe.next_value, Current.user])
fresh; check("break", [59, 47], [ScopeProbe.break_value, Current.user])
fresh; check("method return", [[31, 83], 47], [ScopeProbe.method_return, Current.user])
fresh; check("taken bare-return guard", [nil, 47], [ScopeProbe.guarded_return(true), Current.user])
fresh; check("untaken bare-return guard", [99, 47], [ScopeProbe.guarded_return(false), Current.user])
fresh; check("nested bare-return guard", [nil, 47], [ScopeProbe.nested_guarded_return, Current.user])
fresh; check("nested block bare-return guard", [nil, 47], [ScopeProbe.nested_block_guarded_return, Current.user])
fresh; check("nil", [nil, 47], [ScopeProbe.nil_value, Current.user])
fresh; check("current instance identity", [true, 47], [ScopeProbe.identity, Current.user])
fresh; check("nested", [[47, 83, 109, 83], 47], [ScopeProbe.nested, Current.user])
fresh; check("nested break ownership", [83, 47], [ScopeProbe.nested_exit, Current.user])
fresh; check("nested each", [[[85, 88], 47], 47], [ScopeProbe.nested_each, Current.user])
fresh; check("downstream", true, ScopeProbe.downstream)
fresh; check("hygiene", [[[7, 11, 13, 17], "app-owned", 191], 47], [ScopeProbe.hygiene, Current.user])
fresh; check("real body raise", ["body", 47], [error_from { ScopeProbe.body_failure }, Current.user])
fresh; check("RHS failure", ["rhs", 47, "Z", ["first", "rhs-error"]], [error_from { ScopeProbe.rhs_failure }, Current.user, Current.account, Current::TRACE.dup])
fresh; Current.mode = :side
check("writer side effect and sequential save", [[83, "C"], 47, "side"], [ScopeProbe.writer_effect, Current.user, Current.account])
fresh; Current.mode = :side
check("partial writer failure", ["setup", 47, "side", ["user", "account", "account", "user", "account"]], [error_from { ScopeProbe.setup_failure }, Current.user, Current.account, Current::TRACE.dup])
fresh; Current.mode = :read
check("partial reader failure", ["read", 47, "side", ["user", "account", "user"]], [error_from { ScopeProbe.reader_failure }, Current.user, Current.account, Current::TRACE.dup])
fresh; Current.mode = :restore
check("restore stops in insertion order", ["restore", 47, "C", ["user", "account", "user"]], [error_from { ScopeProbe.restore_failure }, Current.user, Current.account, Current::TRACE.dup])
fresh
class << Current
  alias_method :original_instance_for_probe, :instance
  def instance
    Current::TRACE << "instance"
    original_instance_for_probe
  end
end
inside = ScopeProbe.ordered
check("RHS before instance capture", [83, "C"], inside)
check("once and source order", ["first", "second", "instance", "user", "account", "user", "account"], Current::TRACE)
NamespaceProbe.seed
check("namespace and distinct context", [181, 151, 257, 229, 47], [NamespaceProbe.run, ScopeContext::Current.user, NamespaceProbe.other, OtherScopeContext::Current.user, Current.user])
puts "29 Current.set generated-project assertions passed"
"##;
    let run = super::emit_and_run::real_blog()
        .write("app/models/current.rb", CURRENT)
        .write("app/lib/scope_probe.rb", PROBE)
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    let models = std::fs::read_to_string(run.emitted.join("app/models.rb"))
        .expect("project models aggregator");
    assert!(
        models.contains("roundhouse_current_set_scope"),
        "generated scopes must load through the aggregator: {models}"
    );
    assert!(
        models.contains("scope_context/current"),
        "namespaced Current must load through its project path: {models}"
    );
    assert_eq!(
        run.stdout.trim(),
        "29 Current.set generated-project assertions passed"
    );
    println!("{}", run.stdout.trim());
}

#[test]
fn current_set_own_initializer_has_an_honest_output_boundary() {
    const CURRENT: &str = "class Current < ActiveSupport::CurrentAttributes\n attribute :user\n VALUE = ::Current.set(user: 83) { 31 }\nend\n";
    let mut app = roundhouse::ingest::ingest_app_from_tree(std::collections::HashMap::from([(
        std::path::PathBuf::from("app/models/current.rb"),
        CURRENT.as_bytes().to_vec(),
    )]))
    .unwrap();
    for stage in ["raw", "post-lowering"] {
        let before = app.clone();
        let error = roundhouse::project::target_files(
            &app,
            std::path::Path::new("/nonexistent-current-set-fixture"),
            roundhouse::project::BuildTarget::Ruby,
        )
        .expect_err(stage);
        assert!(
            error.contains("class-body initializers") && error.contains("app/models/current.rb"),
            "{error}"
        );
        assert_eq!(app, before, "refusal retains the original initializer");
        roundhouse::session::analyze_and_lower(&mut app);
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.message.contains("class-body initializers"))
        );
        assert!(app.library_classes.iter().all(|class| !matches!(
            class.origin,
            Some(roundhouse::dialect::LibraryClassOrigin::CurrentSet { .. })
        )));
    }
}

#[test]
fn current_set_relative_qualified_receiver_keeps_distinct_identities() {
    const CURRENT: &str = "module Tenant\n class Current < ActiveSupport::CurrentAttributes\n attribute :user\n end\nend\nmodule Outer\n module Tenant\n class Current < ActiveSupport::CurrentAttributes\n attribute :user\n end\n end\nend\n";
    const PROBE: &str = "module Outer\n class Probe\n def self.run\n Tenant::Current.set(user: 181) { [::Tenant::Current.user, ::Outer::Tenant::Current.user] }\n end\n end\nend\n";
    const ASSERTION: &str = "::Tenant::Current.user = 47\n::Outer::Tenant::Current.user = 83\nactual = [Outer::Probe.run, ::Tenant::Current.user, ::Outer::Tenant::Current.user]\nraise actual.inspect unless actual == [[47, 181], 47, 83]";
    let app = roundhouse::ingest::ingest_app_from_tree(std::collections::HashMap::from([
        (
            std::path::PathBuf::from("app/models/current.rb"),
            CURRENT.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/lib/probe.rb"),
            PROBE.as_bytes().to_vec(),
        ),
    ]))
    .unwrap();
    let before = app.clone();
    roundhouse::lower::current_set::guard_output(&app, "ruby")
        .expect("Tenant::Current inside Outer is Outer::Tenant::Current");
    assert_eq!(app, before, "preflight analyzes a clone");
    // The two reads in the block keep their distinct identities.
    super::emit_and_run::real_blog()
        .write("app/models/current.rb", CURRENT)
        .write("app/lib/probe.rb", PROBE)
        .run_ruby(ASSERTION)
        .assert_passes();
}

#[test]
fn current_set_helper_names_do_not_capture_test_owned_declarations() {
    const CURRENT: &str =
        "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n";
    const TEST: &str = r#"require "test_helper"
class RoundhouseCurrentSetScope3 < ActiveSupport::TestCase
  RoundhouseCurrentSetScope1 = 191
  class RoundhouseCurrentSetScope2
    def self.marker
      37
    end
  end
  test "scope avoids the test declarations" do
    assert_equal 31, ::Current.set(user: 83) { 31 }
    assert_nil Current.user
  end
end
"#;
    let run = super::emit_and_run::real_blog()
        .write("app/models/current.rb", CURRENT)
        .write("test/models/current_scope_test.rb", TEST)
        .run_test("test/models/roundhouse_current_set_scope3_test.rb");
    run.assert_passes();
    assert!(
        run.emitted
            .join("app/models/roundhouse_current_set_scope4.rb")
            .exists(),
        "declaration keys, inner names and test-module names must all be reserved"
    );
}
