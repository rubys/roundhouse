use super::emit_and_run;

// A plain class beside the blog's models: class and instance methods
// with and without parameters, none of them a framework constant's.
const GREETER: &str = r#"class Greeter
  def self.current
    Greeter.new("real")
  end

  def initialize(name)
    @name = name
  end

  def name
    @name
  end

  def greet(who)
    "hello " + who + " from " + @name
  end
end
"#;

const TEST: &str = r#"require "test_helper"

class GreeterTest < ActiveSupport::TestCase
  test "a class method answers the stubbed value" do
    fake = Greeter.new("fake")
    Greeter.stubs(:current).returns(fake)
    assert_equal "fake", Greeter.current.name
  end

  test "answers depend on the arguments" do
    greeter = Greeter.new("x")
    greeter.stubs(:greet).with("ann").returns("hi ann")
    greeter.stubs(:greet).with("bob").returns("hi bob")
    assert_equal "hi ann", greeter.greet("ann")
    assert_equal "hi bob", greeter.greet("bob")
  end

  test "the newest matching stub answers" do
    greeter = Greeter.new("x")
    greeter.stubs(:greet).with(anything).returns("any")
    greeter.stubs(:greet).with("ann").returns("ann")
    assert_equal "ann", greeter.greet("ann")
    assert_equal "any", greeter.greet("bob")
  end

  test "only the stubbed object answers" do
    greeter = Greeter.new("x")
    other = Greeter.new("y")
    greeter.stubs(:name).returns("stubbed")
    assert_equal "stubbed", greeter.name
    assert_equal "y", other.name
  end

  test "any instance answers" do
    Greeter.any_instance.stubs(:name).returns("everyone")
    assert_equal "everyone", Greeter.new("z").name
  end

  test "an expectation counts its calls" do
    Greeter.expects(:current).twice.returns(Greeter.new("c"))
    Greeter.current
    Greeter.current
  end

  test "a stub from an earlier test is gone" do
    assert_equal "real", Greeter.current.name
  end
end
"#;

fn app(test: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/greeter.rb", GREETER)
        .write("test/models/greeter_test.rb", test)
}

#[test]
fn app_method_answers_run_on_ruby() {
    app(TEST).run_test("test/models/greeter_test.rb").assert_passes();
}

// Each of these must fail: an expectation met once instead of twice, a
// call no stub admits, a stub that leaked into another object.
#[test]
fn app_method_answers_fail_where_mocha_does() {
    for (find, replace) in [
        ("    Greeter.current\n    Greeter.current\n", "    Greeter.current\n"),
        ("assert_equal \"hi bob\", greeter.greet(\"bob\")", "greeter.greet(\"zed\")"),
        ("assert_equal \"y\", other.name", "assert_equal \"stubbed\", other.name"),
    ] {
        let run = app(&TEST.replacen(find, replace, 1)).run_test("test/models/greeter_test.rb");
        assert!(run.errors.is_empty(), "{replace}: {:?}", run.errors);
        assert!(!run.success, "`{replace}` passed:\n{}\n{}", run.stdout, run.stderr);
    }
}

// The point: the same stubs inside the Spinel build.
#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn app_method_answers_run_in_the_spinel_build() {
    let (emitted, errors) = app(TEST).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("spin")
        .args(["test", "test/greeter_test.rb"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn spin");
    std::fs::write(emitted.join("spin-test.stdout"), &output.stdout).unwrap();
    std::fs::write(emitted.join("spin-test.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
