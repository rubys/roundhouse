use super::emit_and_run;

const SOURCE: &str = r#"class Presenter
  attr_reader :label, :scope, :now

  def initialize(label:, scope: nil, now: Time.now)
    @label = label
    @scope = scope
    @now = now
  end

  def summary
    @label
  end
end

class PresenterCaller
  def self.build
    Presenter.new(label: "first", scope: nil)
  end

  def self.replace_summary
    Presenter.define_method(:summary) { "replacement" }
  end
end
"#;

#[test]
fn emitted_constructor_runs_when_a_test_redefines_an_unrelated_method() {
    emit_and_run::real_blog()
        .write("app/presenters/presenter.rb", SOURCE)
        .write(
            "test/models/presenter_test.rb",
            r#"require "test_helper"

class PresenterTest < ActiveSupport::TestCase
  test "summary can be replaced" do
    Presenter.define_method(:summary) { "replacement" }
    assert_equal "first", PresenterCaller.build.label
  end
end
"#,
        )
        .run_ruby(
            r#"
presenter = PresenterCaller.build
raise "required keyword lost" unless presenter.label == "first"
raise "optional keyword lost" unless presenter.scope.nil?
raise "callee default lost" unless presenter.now.is_a?(Time)
"#,
        )
        .assert_passes();
}
