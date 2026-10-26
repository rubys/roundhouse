//! `include?` on an instance of an app class over Hash (discourse's
//! `Enum < Hash`, asked `auto_action_flag_types.include?(name)` in
//! `PostActionCreator`) is Hash#include?, key membership. It used to be
//! refused as Module protocol. A strict emit has no Hash to extend, so
//! there the class itself is reported.

use super::emit_and_run;

const ENUM: &str = r#"class Enum < Hash
  def initialize(*members)
    super({})
    if members[0].is_a?(Hash)
      update Hash[members[0]]
    else
      start = 1
      update Hash[members.zip(start..members.count + start)]
    end
  end

  def valid?(member)
    has_key?(member)
  end
end
"#;

const FLAGS: &str = r#"class FlagTypes
  def self.auto_action?(name)
    Enum.new({ spam: 3, off_topic: 4 }).include?(name)
  end

  def self.positional?(name)
    Enum.new(:a, :b).include?(name)
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/enum.rb", ENUM).write("app/models/flag_types.rb", FLAGS)
}

const ASSERTIONS: &str = r##"got = [FlagTypes.auto_action?(:spam), FlagTypes.auto_action?(:nope), FlagTypes.positional?(:b), FlagTypes.positional?(3)]
raise got.inspect unless got == [true, false, true, false]
puts "hash subclass include passed"
"##;

#[test]
fn hash_subclass_include_runs() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("hash subclass include passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn hash_subclass_include_runs_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("hash subclass include passed"), "{}", run.stdout);
}

#[test]
fn a_builtin_subclass_is_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.iter().any(|e| e.contains("`Enum` subclasses Hash")), "{target:?}: {errors:#?}");
        let (_emitted, errors) = emit_and_run::real_blog()
            .write("test/models/bag_test.rb", "require \"test_helper\"\n\nclass BagTest < ActiveSupport::TestCase\n  class Bag < Hash\n  end\n\n  test \"bag\" do\n    assert Bag.new.empty?\n  end\nend\n")
            .emit(target);
        assert!(errors.iter().any(|e| e.contains("subclasses Hash")), "{target:?} test-local: {errors:#?}");
    }
}
