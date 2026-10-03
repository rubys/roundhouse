//! Constructs that `check` accepts must run once emitted.
//!
//! See `tests/support/emit_and_run.rs` for the harness and why it
//! exists. The ignored tests below are known places where the two
//! disagree: `check` is clean and the emitted program fails. Each is a
//! complete statement of the fix: make it pass and drop the `#[ignore]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;

/// Class-root query builders still need a Relation when the entire app
/// declares no scope or association. In particular, zero-argument `where`
/// must reach WhereChain rather than Base.where's required argument (#324).
#[test]
fn scope_free_model_query_builders_run() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets", to: "widgets#index"
  get "/named", to: "widgets#named"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def index
    render plain: Widget.order(:name).limit(1).map { |w| w.name }.join(",")
  end
  def named
    render plain: Widget.where.not(name: nil).order(:name).map { |w| w.name }.join(",")
  end
end
"#)
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
Widget.create!(name: "beta")
Widget.create!(name: "alpha")
controller = WidgetsController.new
controller.process_action(:index)
raise "class order lost its relation" unless controller.body == "alpha"
Widget.create!(name: nil)
controller = WidgetsController.new
controller.process_action(:named)
raise "class where.not lost its relation" unless controller.body == "alpha,beta"
puts "scope-free query builders passed"
"#)
        .assert_passes();
}

#[test]
fn finite_concern_class_configuration_runs_without_replaying_rails() {
    for (overlay, assertions) in [
        (class_configuration::overlay(), class_configuration::ASSERTIONS),
        (class_configuration::empty_overlay(), class_configuration::EMPTY_ASSERTIONS),
    ] {
        let run = overlay.run_ruby(assertions);
        run.assert_passes();
        assert!(run.stdout.contains("finite class configuration contract passed"));
    }
}

/// The harness itself: the unedited blog emits and its controller
/// suite, which renders every page, passes.
#[test]
fn the_unedited_blog_runs() {
    emit_and_run::real_blog()
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A builder wrapper's private argument computation belongs to its helper,
/// not to the view into which the form and builder body are spliced.
#[test]
fn a_form_wrapper_keeps_its_private_argument_computation_in_its_owner() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/articles_helper.rb",
            r#"module ArticlesHelper
  def article_form_with!(article, suffix = "!", &)
    form_with model: article, class: "contents", data: private_options(article, suffix), &
  end
  def __rh_form_article_form_with_0; 91; end
  def article_public_form_with(model, &)
    form_with model: model, data: { controller: public_label, label: @article.title }, &
  end
  def public_label; "public-owner"; end
  private
  def private_options(article, suffix)
    article.title = article.title + suffix
    { controller: private_controller, label: article.title }
  end
  def private_controller; "owner-composer"; end
end
"#,
        )
        .write(
            "app/helpers/zzz_helper.rb",
            r#"module ZzzHelper
  def private_options(article, suffix); { controller: "wrong-owner" }; end
  def private_controller; "wrong-controller"; end
end
"#,
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "form_with(model: article, class: \"contents\")",
            "article_form_with!(article)",
        )
        .write(
            "app/views/articles/_public_owner_form.html.erb",
            "<%= article_public_form_with(article) do |form| %><%= form.text_field :title %><% end %>",
        )
        .run_ruby(r#"
article = Article.new(title: "seed", body: "body")
raise "unexpected controller context" unless ActionController::Current.controller.nil?
public_html = Views::Articles.public_owner_form(article)
raise "direct helper ivar lost view binding" unless public_html.include?('data-label="seed"')
raise "public helper lost owner" unless public_html.include?('data-controller="public-owner"')
html = Views::Articles.form(article)
raise "missing form" unless html.include?("<form") && html.include?("</form>")
raise "wrong helper owner" unless html.include?('data-controller="owner-composer"')
raise "argument evaluated incorrectly" unless html.include?('data-label="seed!"') && article.title == "seed!"
raise "builder lost record" unless html.include?('name="article[title]"') && html.include?('value="seed!"')
raise "generated name collision" unless ArticlesHelper.__rh_form_article_form_with_0 == 91
[:private_options, :private_controller].each do |name|
  raise "private helper made public" if ArticlesHelper.respond_to?(name)
  begin
    name == :private_options ? ArticlesHelper.public_send(name, article, "?") : ArticlesHelper.public_send(name)
    raise "public_send exposed private helper"
  rescue NoMethodError
  end
end
puts "form wrapper owner checks passed"
"#)
        .assert_passes();
}

/// Visibility is observable runtime behavior, not just an IR annotation.
/// Cover models, POROs, and a concern's instance and copied class sides.
#[test]
fn local_method_visibility_survives_emission_and_reflective_dispatch() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            r#"class Article < ApplicationRecord
  include VisibilityHelpers
  def visibility_wrapper; visibility_helper; end
  def visibility_helper; 41; end
  private :visibility_helper
  def visibility_later; 42; end
  private
  def self.visibility_public_class; 43; end
  public
  class << self
    def visibility_class_wrapper; visibility_class_helper; end
    private
    def visibility_class_helper; 44; end
  end
  def self.visibility_class_later; 45; end
"#,
        )
        .write(
            "app/lib/visibility_probe.rb",
            r#"class VisibilityProbe
  def initialize; @value = 51; end
  def visibility_wrapper; visibility_helper; end
  private def visibility_helper; @value; end
  def visibility_later; 52; end
  protected def guarded; 53; end
  class << self
    protected
    def guarded; 54; end
  end
end
class PublicInitializeProbe
  def initialize; @value = 71; end
  public :initialize
  def self.initialize; 72; end
end
"#,
        )
        .write(
            "app/models/concerns/visibility_helpers.rb",
            r#"module VisibilityHelpers
  extend ActiveSupport::Concern
  def concern_wrapper; concern_helper; end
  private
  def concern_helper; 61; end
  class_methods do
    def concern_class_wrapper; concern_class_helper; end
    def concern_class_helper; 62; end
    private :concern_class_helper
    def concern_class_later; 63; end
  end
end
"#,
        )
        .run_ruby(r#"
def assert_equal(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
def rejects_public_send(receiver, name)
  begin
    receiver.public_send(name)
  rescue NoMethodError
    return
  end
  raise "public_send reached #{name}"
end
article = Article.new
probe = VisibilityProbe.new
[
  [article, :visibility_wrapper, :visibility_helper, 41],
  [probe, :visibility_wrapper, :visibility_helper, 51],
  [article, :concern_wrapper, :concern_helper, 61],
  [Article, :visibility_class_wrapper, :visibility_class_helper, 44],
  [Article, :concern_class_wrapper, :concern_class_helper, 62]
].each do |receiver, wrapper, helper, expected|
  assert_equal expected, receiver.public_send(wrapper)
  assert_equal expected, receiver.send(helper)
  rejects_public_send receiver, helper
  assert_equal false, receiver.respond_to?(helper)
  assert_equal false, receiver.respond_to?(helper, false)
  assert_equal true, receiver.respond_to?(helper, true)
end
assert_equal 42, article.public_send(:visibility_later)
assert_equal 52, probe.public_send(:visibility_later)
assert_equal 43, Article.public_send(:visibility_public_class)
assert_equal 45, Article.public_send(:visibility_class_later)
assert_equal 63, Article.public_send(:concern_class_later)
assert_equal false, article.respond_to?(:initialize)
assert_equal true, article.respond_to?(:initialize, true)
assert_equal false, probe.respond_to?(:initialize)
rejects_public_send probe, :initialize
assert_equal true, PublicInitializeProbe.new.respond_to?(:initialize)
assert_equal 71, PublicInitializeProbe.new.public_send(:initialize)
assert_equal 72, PublicInitializeProbe.public_send(:initialize)
assert_equal true, VisibilityProbe.protected_instance_methods(false).include?(:guarded)
assert_equal true, VisibilityProbe.singleton_class.protected_instance_methods(false).include?(:guarded)
rejects_public_send probe, :guarded
rejects_public_send VisibilityProbe, :guarded
puts "visibility dispatch checks passed"
"#)
        .assert_passes();
}

/// Module-function promotion retains a public singleton copy; extend self
/// instead retains the original visibility, even across bare markers.
#[test]
fn module_function_and_extend_self_have_distinct_runtime_visibility() {
    emit_and_run::real_blog()
        .write(
            "app/lib/module_visibility.rb",
            r#"module NamedVisibility
  private
  def helper; 81; end
  module_function :helper
end
module BareVisibility
  private
  module_function
  def helper; 82; end
  private :helper
  private def inline_helper; 83; end
  def later_copy; 84; end
  public
  def instance_later; 85; end
end
module ExtendedVisibility
  extend self
  private
  def helper; 91; end
  protected
  def guarded; 92; end
  public
  def later; 93; end
end
"#,
        )
        .run_ruby(r#"
def assert_equal(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
[
  [NamedVisibility, :helper, 81],
  [BareVisibility, :helper, 82],
  [BareVisibility, :inline_helper, 83],
  [BareVisibility, :later_copy, 84],
  [ExtendedVisibility, :later, 93]
].each do |receiver, name, expected|
  assert_equal expected, receiver.public_send(name)
  assert_equal true, receiver.respond_to?(name)
end
assert_equal false, BareVisibility.respond_to?(:instance_later, true)
assert_equal true, BareVisibility.public_instance_methods(false).include?(:instance_later)
[[ExtendedVisibility, :helper, 91], [ExtendedVisibility, :guarded, 92]].each do |receiver, name, expected|
  assert_equal expected, receiver.send(name)
  assert_equal false, receiver.respond_to?(name)
  assert_equal true, receiver.respond_to?(name, true)
  begin
    receiver.public_send(name)
  rescue NoMethodError
    next
  end
  raise "public_send reached #{name}"
end
puts "module visibility checks passed"
"#)
        .assert_passes();
}

/// A test class that a `module` wraps is emitted and runs. Ingest used
/// to read only the top-level classes of a test file. It lost this
/// class, and `check` reported nothing. The emit names the file after
/// the full class name, as it does for `class Models::ArticleTest`.
#[test]
fn a_test_class_inside_a_module_runs() {
    emit_and_run::real_blog()
        .write(
            "test/models/models_article_test.rb",
            r#"require "test_helper"

module Models
  class ArticleTest < ActiveSupport::TestCase
    test "reads a fixture" do
      assert_equal "Getting Started with Rails", articles(:one).title
    end
  end
end
"#,
        )
        .run_test("test/models/models_article_test.rb")
        .assert_passes();
}

/// Source inference must preserve Ruby parameter binding and the existing
/// test lowering; inferred signatures are not permission to rewrite calls.
#[test]
fn original_test_helper_defaults_and_keyword_forwarding_run() {
    let run = emit_and_run::real_blog()
        .write(
            "test/models/source_helper_test.rb",
            r#"require "test_helper"

class SourceHelperTest < ActiveSupport::TestCase
  setup { @prefix = "scope:" }

  test "defaults and forwarding preserve original bindings" do
    assert_equal ["FIRST"], normalized("first")
    assert_equal ["chosen"], normalized("second", "chosen")
    assert_equal ["explicit"], normalized("third", "unused", ["explicit"])
    visited = 0
    ["first", "last"].each do |value|
      values_from(value: value).each do |inner|
        assert_equal @prefix + value, inner
        visited += 1
      end
    end
    assert_equal 4, visited
    assert_equal [13, "forwarded"], forwarding(13, value: "forwarded")
    assert_equal [17, "direct"], target(17, value: "direct")
    assert_equal({ value: "kept" }, whole_hash(19, value: "kept"))
    puts "PASS original source helper assertions"
  end

  def normalized(value, upper = upper_for(value), values = [upper])
    values
  end

  def upper_for(input)
    input.upcase
  end

  def values_from(**details)
    values_for(**details)
  end

  def values_for(value:)
    [@prefix + value, @prefix + value]
  end

  def forwarding(number, **details)
    target(number, **details)
  end

  def target(number, value:)
    [number, value]
  end

  def whole_hash(*numbers, **details)
    details
  end
end
"#,
        )
        .run_test("test/models/source_helper_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("PASS original source helper assertions"));
}

#[test]
fn native_date_column_crud_and_month_shifts() {
    let output = std::process::Command::new("ruby")
        .args(["tests/date_columns_runtime.rb", "native"])
        .output()
        .expect("native Ruby");
    assert!(output.status.success(), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Date CRUD and month shifts OK"));
}

#[test]
fn date_column_crud_and_month_shifts_run() {
    date_blog()
        .run_ruby(include_str!("date_columns_runtime.rb"))
        .assert_passes();
}

#[test]
fn date_storage_and_integer_enum_labels_coexist() {
    date_blog()
        .edit("db/schema.rb", "    t.date \"due_on\"", "    t.date \"due_on\"\n    t.integer \"state\"")
        .edit("app/models/calendar_entry.rb", "class CalendarEntry < ApplicationRecord", "class CalendarEntry < ApplicationRecord\n  enum :state, { draft: 7, published: 42 }")
        .run_ruby(r#"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), state: :published)
entry.reload
raise entry.due_on.inspect unless entry.due_on.class == Date && entry.due_on.iso8601 == "2024-01-31"
raise entry.shifted(2).inspect unless entry.shifted(2).iso8601 == "2024-03-31"
raise entry.state.inspect unless entry.state == "published"
raise entry[:state].inspect unless entry[:state] == "published"
raise entry.attributes.inspect unless entry.attributes["state"] == "published"
raise "enum predicates changed" unless entry.published? && !entry.draft?
stored = CalendarEntry.connection.select_all("SELECT due_on, state FROM calendar_entries WHERE id = #{entry.id}").first
raise stored.inspect unless stored == {"due_on" => "2024-01-31", "state" => 42}
actual = ActionController::JsonRender.encode(entry.as_json(only: [:due_on, :state]))
raise actual.inspect unless actual == '{"due_on":"2024-01-31","state":"published"}'
entry.update!(due_on: nil, state: nil)
entry.reload
raise "nullable readers changed" unless entry.due_on.nil? && entry.state.nil? && entry[:state].nil? && entry.attributes["state"].nil?
stored = CalendarEntry.connection.select_all("SELECT due_on, state FROM calendar_entries WHERE id = #{entry.id}").first
raise stored.inspect unless stored == {"due_on" => nil, "state" => nil}
actual = ActionController::JsonRender.encode(entry.as_json(only: [:due_on, :state]))
raise actual.inspect unless actual == '{"due_on":null,"state":null}'
puts "Date storage and nullable sparse integer enum labels coexist"
"#)
        .assert_passes();
}

fn date_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "  create_table \"articles\"", "  create_table \"calendar_entries\" do |t|\n    t.date \"due_on\"\n    t.datetime \"observed_at\"\n    t.time \"opens_at\"\n  end\n\n  create_table \"articles\"")
        .write("app/models/calendar_entry.rb", include_str!("date_columns_model.rb"))
}

fn date_json_blog() -> emit_and_run::Overlay {
    date_blog()
        .edit("app/models/calendar_entry.rb", "\nend\n", "\n  def as_json(options = {})\n    attrs = [:due_on, :observed_at]\n    json = super(only: attrs)\n    json\n  end\nend\n")
        .write("app/controllers/calendar_entries_controller.rb", "class CalendarEntriesController < ApplicationController\n  def show\n    entry = CalendarEntry.find(params[:id])\n    render json: entry\n  end\nend\n")
        .edit("config/routes.rb", "  resources :articles do", "  resources :calendar_entries, only: [:show]\n  resources :articles do")
}

#[test]
fn specialized_date_json_preserves_dates_and_zoned_timestamps() {
    date_json_blog()
        .run_ruby(r#"
require_relative "app/controllers/calendar_entries_controller"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), observed_at: Time.utc(2024, 1, 31, 23, 47, 19, 123456))
raise "specialization was not exercised" unless entry.respond_to?(:as_json_str)
ActiveSupport.use_zone("Pacific/Auckland") do
  controller = CalendarEntriesController.new
  controller.params = {"id" => entry.id.to_s}
  controller.process_action(:show)
  expected = '{"due_on":"2024-01-31","observed_at":"2024-02-01T12:47:19.123+13:00"}'
  raise controller.body.inspect unless controller.body == expected
  raise controller.content_type.inspect unless controller.content_type == "application/json"
  actual = ActionController::JsonRender.encode(entry.as_json)
  raise actual.inspect unless actual == expected
  entry.update!(due_on: nil)
  controller = CalendarEntriesController.new
  controller.params = {"id" => entry.id.to_s}
  controller.process_action(:show)
  expected = '{"due_on":null,"observed_at":"2024-02-01T12:47:19.123+13:00"}'
  raise controller.body.inspect unless controller.body == expected
  actual = ActionController::JsonRender.encode(entry.as_json)
  raise actual.inspect unless actual == expected
end
puts "Specialized Date JSON and timestamp control OK"
"#)
        .assert_passes();
}

#[test]
fn specialized_date_json_normalizes_unset_nonnullable_storage() {
    date_json_blog()
        .edit("db/schema.rb", "t.date \"due_on\"", "t.date \"due_on\", null: false")
        .run_ruby(r#"
entry = CalendarEntry.new
raise entry.due_on_raw.inspect unless entry.due_on_raw == ""
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry[:due_on].inspect unless entry[:due_on].nil?
raise "unset Date alias changed" unless entry.shifted_attribute(1).nil?
expected = '{"due_on":null,"observed_at":null}'
raise entry.as_json_str.inspect unless entry.as_json_str == expected
actual = ActionController::JsonRender.encode(entry.as_json)
raise actual.inspect unless actual == expected
puts "Unset nonnullable Date JSON is null in both paths"
"#)
        .assert_passes();
}

/// Alba's inherited declarations are executable property reads, not just a
/// return-type assertion. Boot loads the generated classes without Alba.
#[test]
fn alba_inherited_attributes_and_one_nested_resource_run() {
    emit_and_run::real_blog()
        .write("app/lib/alba_resources.rb", include_str!("support/alba.rb"))
        .write("app/controllers/alba_probes_controller.rb", r#"
class AlbaProbesController < ApplicationController
  def index
    author = AlbaAuthor.new(9, "Ada")
    article = AlbaArticle.new(7, "Syn", author)
    render json: ArticleResource.new(article).to_h
  end
end
"#)
        .run_ruby(r#"
expected = {"id" => 7, "title" => "Synthetic", "author" => {"id" => 9, "name" => "Ada"}}
actual = SurveyProbe.call
raise actual.inspect unless actual == expected
raise "external Alba loaded" if defined?(Alba::Resource)
puts "PASS portable Alba source-property contract"
"#)
        .assert_passes();
}

/// A delegated setter going from broken (`def behavior=\n  x.behavior=\n
/// end` — a `def` with no parameter and a bare `x.y=` call, two syntax
/// errors) to working is a claim the emitted program actually runs a
/// SET through it, not just that `check` stays clean (invariant 6). A
/// PORO under `app/lib` (the same shape `procore_os/deprecation.rb`
/// declares: `attr_accessor` on the target, `delegate` for a setter)
/// forwards to another object — reusing `Article`, since it already
/// has a `title` column — and a model test sets through the forwarder
/// and reads the value back off the target.
#[test]
fn a_delegated_setter_forwards_through_to_its_target() {
    emit_and_run::real_blog()
        .write(
            "app/lib/deprecation.rb",
            "class Deprecation\n  attr_accessor :inner\n\n  delegate :title=, to: :inner\nend\n",
        )
        .write(
            "test/models/deprecation_test.rb",
            "require \"test_helper\"\n\n\
             class DeprecationTest < ActiveSupport::TestCase\n  \
               test \"a delegated setter forwards through to its target\" do\n    \
                 d = Deprecation.new\n    \
                 d.inner = Article.new\n    \
                 d.title = \"Reused\"\n    \
                 assert_equal \"Reused\", d.inner.title\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/deprecation_test.rb")
        .assert_passes();
}

/// A lambda-target `before_action` — `before_action -> { … }, only:
/// […]`, no Symbol target and no attached block either — going from
/// "controller class-body macro not recognized" to clean is a claim
/// the emitted dispatcher actually RUNS the lambda's body and applies
/// its `only:` scoping (invariant 6), not just that `check` stops
/// complaining. 233 Procore controllers write exactly this shape for
/// a policy guard closing over a receiverless call
/// (`ensure_permission(documents_policy.configure_tab?)`), not a
/// literal condition inline in the lambda — so this pins that exact
/// pattern: the lambda calls a private predicate method, and the
/// existing "should show article" test is updated to expect the
/// redirect the (permanently failing, for the test) predicate causes.
///
/// NOTE ON SCOPE: a lambda body that reads `params[...]` directly
/// (rather than through a named method) is a NARROWER, separate gap —
/// `rewrite_params`, the pass that turns `params[:x]` into the
/// runtime's string-keyed `@params.fetch("x", ...)`, runs over each
/// action/helper body individually and is never reached by a
/// `PreambleStmt::Block`'s body, which is assembled straight into
/// `process_action` after that pass has already run. None of the 233
/// real sites hit it (`ensure_permission`, `policy.can_view_recycle_bin?`,
/// … all delegate to a named method, whose OWN body goes through the
/// normal per-method pipeline and gets `params` rewritten there); a
/// site that inlined `params[...]` directly in the lambda would not.
/// Recorded here rather than silently left for the next person to
/// rediscover.
#[test]
fn a_lambda_target_before_action_gates_the_action_it_guards() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "before_action :set_article, only: %i[ show edit update destroy ]",
            "before_action :set_article, only: %i[ show edit update destroy ]\n  before_action -> { redirect_to root_path unless allowed_to_view? }, only: [:show]",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  private\n",
            "  private\n\n  def allowed_to_view?\n    false\n  end\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "test \"should show article\" do\n    get article_url(@article)\n    assert_response :success\n    assert_select \"h1\", @article.title\n    assert_select \"h2\", \"Comments\"\n    assert_select \"#comments .p-4\", minimum: 1\n  end",
            "test \"a lambda-target before_action redirects when its guard fails\" do\n    get article_url(@article)\n    assert_redirected_to root_url\n  end",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// An inner class wins over another class with the same last segment.
#[test]
fn a_bare_inner_class_runs_after_resolution() {
    emit_and_run::real_blog()
        .write(
            "app/services/ui/selector.rb",
            "module UI\n  class Selector\n    class Mode\n      def self.value\n        \"selected\"\n      end\n    end\n    def self.value\n      Mode.value\n    end\n  end\n  class Other\n    class Mode\n    end\n  end\nend\n",
        )
        .run_ruby("raise 'wrong inner class' unless UI::Selector.value == 'selected'")
        .assert_passes();
}

/// A library-only path gem is app code, and its consumer must run in the output.
#[test]
fn a_library_only_path_gem_runs_from_an_app_consumer() {
    emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "GEM\n",
            "PATH\n  remote: components/numbers\n  specs:\n    path_numbers (0.1.0)\n\nGEM\n",
        )
        .write(
            "components/numbers/lib/path_number.rb",
            "class PathNumber\n  def self.value\n    41\n  end\nend\n",
        )
        .write(
            "app/services/path_number_consumer.rb",
            "class PathNumberConsumer\n  def self.value\n    PathNumber.value + 1\n  end\nend\n",
        )
        .run_ruby("raise 'wrong path-gem result' unless PathNumberConsumer.value == 42")
        .assert_passes();
}

/// `class UI::ExplicitSelector` does not lexically include `UI`, even
/// though emitted Ruby nests it there. Keep a top-level same-suffix class
/// distinct from the one inside UI after source-backed resolution.
#[test]
fn a_compact_class_uses_its_source_lexical_constant() {
    emit_and_run::real_blog()
        .write(
            "app/services/ui/explicit_selector.rb",
            "class SourceScopeResolution\n  def self.value\n    \"top-level\"\n  end\nend\nmodule UI\n  class SourceScopeResolution\n    def self.value\n      \"nested\"\n    end\n  end\nend\nclass UI::ExplicitSelector\n  def self.value\n    SourceScopeResolution.value\n  end\nend\n",
        )
        .run_ruby(
            "raise 'wrong lexical constant' unless UI::ExplicitSelector.value == 'top-level'",
        )
        .assert_passes();
}

/// Array `find` is not a scalar lookup or a permissive `where(id: ids)`:
/// it raises on a missing scoped row, and preserves requested order unless
/// the relation carries an explicit order. The terminal cannot poison its
/// receiver's WHERE, limit/offset, or loaded-record cache.
#[test]
fn relation_find_with_array_ids_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.find_titles(ids)
    where("title != 'outside'").find(Array(ids)).map(&:title).join("|")
  end"#,
        )
        .run_ruby(
            r##"a = Article.create!(title: "zebra", body: "long enough body")
b = Article.create!(title: "apple", body: "long enough body")
c = Article.create!(title: "outside", body: "long enough body")
raise "requested order" unless Article.find_titles([b.id.to_s, a.id.to_s, b.id.to_s]) == "apple|zebra"
raise "integer ids" unless Article.find_titles([a.id, b.id]) == "zebra|apple"
raise "mixed ids" unless Article.find_titles([b.id.to_s, a.id]) == "apple|zebra"
raise "singleton array" unless Article.find_titles([b.id.to_s]) == "apple"
raise "empty array" unless Article.find_titles([]) == ""
raise "integer casting" unless Article.find_titles(["#{b.id}-slug", a.id.to_s]) == "apple|zebra"
rel = ActiveRecord::Relation.new(Article).where("title != 'outside'")
rel.to_a
raise "loaded scalar" unless rel.find(b.id.to_s).title == "apple"
raise "loaded array" unless rel.find([b.id, a.id]).map(&:title) == ["apple", "zebra"]
begin
  rel.find([a.id, c.id])
  raise "a scoped-out id was accepted"
rescue ActiveRecord::RecordNotFound
end
begin
  rel.find([a.id, 987654321])
  raise "a missing id was accepted"
rescue ActiveRecord::RecordNotFound
end
begin
  rel.find([b.id.to_s, "0#{b.id}"])
  raise "ids were deduplicated after casting"
rescue ActiveRecord::RecordNotFound
end
raise "scope/cache poisoned" unless rel.to_a.map(&:title) == ["zebra", "apple"]
ordered = ActiveRecord::Relation.new(Article).where("title != 'outside'").order(:title)
raise "relation order" unless ordered.find([a.id, b.id]).map(&:title) == ["apple", "zebra"]
offset_only = ActiveRecord::Relation.new(Article).where("title != 'outside'").order(:title).offset(1)
prior_sql = offset_only.to_sql
raise "ordered offset only" unless offset_only.find([a.id, b.id]).map(&:title) == ["zebra"]
raise "offset poisoned" unless offset_only.to_sql == prior_sql
boundary = ActiveRecord::Relation.new(Article).order(:title).offset(2)
raise "offset at size" unless boundary.find([a.id, b.id]) == []
# Writebook's pinned Rails finder raises beyond size (negative expected
# cardinality); newer Rails returns [] here instead.
beyond = ActiveRecord::Relation.new(Article).order(:title).offset(3)
prior_sql = beyond.to_sql
begin
  beyond.find([a.id, b.id])
  raise "pinned Rails beyond-offset behavior changed"
rescue ActiveRecord::RecordNotFound
end
raise "beyond-offset poisoned" unless beyond.to_sql == prior_sql
paged = ActiveRecord::Relation.new(Article).where("title != 'outside'").limit(1).offset(1)
raise "input slicing" unless paged.find([b.id, a.id]).map(&:title) == ["zebra"]
raise "pagination poisoned" unless paged.to_a.map(&:title) == ["apple"]
ordered.limit(1).offset(1)
raise "ordered pagination" unless ordered.find([b.id, a.id]).map(&:title) == ["zebra"]
selected = ActiveRecord::Relation.new(Article).select(:title)
prior_sql = selected.to_sql
raise "projected key" unless selected.find([b.id, a.id]).map(&:id) == [b.id, a.id]
raise "projection poisoned" unless selected.to_sql == prior_sql
puts "ok"
"##,
        )
        .assert_passes();
}

/// The runtime defines this exception in `active_support_ext.rb`.
#[test]
fn framework_exception_resolves_from_real_runtime_source() {
    emit_and_run::real_blog()
        .write(
            "app/services/signature_probe.rb",
            "class SignatureProbe\n  def self.call\n    begin\n      raise ActiveSupport::MessageVerifier::InvalidSignature\n    rescue ActiveSupport::MessageVerifier::InvalidSignature\n      \"handled\"\n    end\n  end\nend\n",
        )
        .run_ruby("raise 'signature error was not caught' unless SignatureProbe.call == 'handled'")
        .assert_passes();
}

/// Rubydex promotes `X = <call>` to a module once code calls a method
/// on `X`. These are still values: `.freeze` and `.map` build a Hash, an
/// Array, and a String, and each read must type and run as that value.
#[test]
fn a_constant_assigned_from_a_call_runs_as_its_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/frozen_table.rb",
            "class FrozenTable\n  STATUSES = { processed: \"processed\" }.freeze\n  NAMES = [\"a\", \"b\"].freeze\n  LABEL = \"label\".freeze\n  DOUBLED = [1, 2].map { |n| n * 2 }\n  def self.summary\n    [STATUSES[:processed].upcase, NAMES.first, LABEL.upcase, DOUBLED.last.to_s].join(\",\")\n  end\nend\n",
        )
        .run_ruby("raise 'frozen constant' unless FrozenTable.summary == 'PROCESSED,a,LABEL,4'")
        .assert_passes();
}

/// Ingest copies a concern's methods into the controller that includes
/// it, and the emitted controller drops the `include`. A class that
/// resolved inside the concern's module must still name that class
/// after the move.
#[test]
fn a_concern_class_reference_survives_the_copy_into_its_controller() {
    emit_and_run::real_blog()
        .write(
            "app/services/price_support.rb",
            "module PriceSupport\n  class RateCalculator\n    def self.value\n      7\n    end\n  end\n\n  def price\n    RateCalculator.value\n  end\nend\n",
        )
        .write(
            "app/controllers/quotes_controller.rb",
            "class QuotesController < ApplicationController\n  include PriceSupport\n\n  def show\n    @value = price\n  end\nend\n",
        )
        .run_ruby(
            "require_relative 'app/controllers/quotes_controller'\nraise 'concern class reference' unless QuotesController.new.price == 7",
        )
        .assert_passes();
}

/// A concern that includes another concern inside its `included do`
/// block: ActiveSupport::Concern runs that block on the includer, so
/// the model gets the inner concern's methods too.
#[test]
fn a_concern_included_from_an_included_block_reaches_the_model() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  def shout\n    title.upcase\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @loudest = @articles.first&.shout\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.shout unless a.shout == \"HI\"",
        )
        .assert_passes();
}

/// Ruby's lookup order for a module included from `included do`: the
/// block runs on the includer after the outer module is appended, so
/// the inner module sits AHEAD of the outer one and its method wins.
#[test]
fn an_include_from_an_included_block_takes_precedence_over_its_concern() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\n\n  def shout\n    \"outer\"\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  def shout\n    \"inner\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.shout unless a.shout == \"inner\"",
        )
        .assert_passes();
}

/// The inner concern's own `included do` runs on the includer too: its
/// scope is declared on the model.
#[test]
fn an_include_from_an_included_block_brings_its_own_included_items() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  included do\n    scope :titled, ->(title) { where(title: title) }\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .run_ruby(
            "Article.create!(title: \"Hi\", body: \"Body text here\")\nraise \"scope missing\" unless Article.titled(\"Hi\").count == 1",
        )
        .assert_passes();
}

/// Integer serialization is not blindly String#to_i: nonnumeric labels
/// must not alias an existing row zero. Invalid IDs still count toward the
/// array finder's required cardinality, except when pagination excludes them.
#[test]
fn relation_find_rejects_nonnumeric_ids_without_aliasing_zero() {
    emit_and_run::real_blog()
        .run_ruby(r#"
Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (0, 'zero', 'long enough body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (31, 'thirty-one', 'long enough body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
rel = ActiveRecord::Relation.new(Article).where("id IN (0, 31)")
prior_sql = rel.to_sql
rel.to_a
raise "valid zero" unless rel.find("0").id == 0
raise "zero prefix" unless rel.find("0x").id == 0
raise "numeric prefix" unless rel.find("31-sarah").id == 31
raise "whitespace/sign" unless rel.find(" \t+31-slug").id == 31
["bogus", "", " ", "+", "-", "٠"].each do |id|
  [id, [id], [id, 31]].each do |input|
    begin
      rel.find(input)
      raise "invalid id aliased a record: #{input.inspect}"
    rescue ActiveRecord::RecordNotFound
    end
  end
end
raise "scope/cache poisoned" unless rel.to_sql == prior_sql && rel.to_a.map(&:id) == [0, 31]
unordered = ActiveRecord::Relation.new(Article).offset(1)
raise "excluded invalid input" unless unordered.find(["bogus", 31]).map(&:id) == [31]
ordered = ActiveRecord::Relation.new(Article).order(:id).limit(1)
raise "ordered limit cardinality" unless ordered.find(["bogus", 31]).map(&:id) == [31]
ordered.offset(1)
begin
  ordered.find(["bogus", 31])
  raise "invalid input dropped from expected size"
rescue ActiveRecord::RecordNotFound
end
puts "ok"
"#)
        .assert_passes();
}

/// A custom String primary key must not take the default Integer cast.
#[test]
fn relation_find_with_string_keys_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"bookmarks\", id: { type: :string, limit: 32 }, primary_key: \"code\", force: :cascade do |t|\n    t.string \"title\", null: false\n    t.integer \"visits\", default: 0\n    t.datetime \"retired_at\"\n    t.index [\"title\"], name: \"index_bookmarks_on_live_title\", unique: true, where: \"(retired_at IS NULL)\"\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
        )
        .write(
            "app/models/bookmark.rb",
            r#"class Bookmark < ApplicationRecord
  self.primary_key = "code"

  def self.find_titles
    where("title != 'outside'").find(["2-apples", "z-9"]).map(&:title).join("|")
  end
end
"#,
        )
        .run_ruby(
            r#"Bookmark.create!(code: "z-9", title: "zebra")
Bookmark.create!(code: "2-apples", title: "apple")
raise "string array cast/order" unless Bookmark.find_titles == "apple|zebra"
rel = ActiveRecord::Relation.new(Bookmark)
raise "string scalar cast" unless rel.find("2-apples").title == "apple"
raise "custom key" unless rel.find(["z-9"]).map(&:id) == ["z-9"]
# The same model must retain both synthesized methods after merging:
# String-key finders and partial-index upsert targets are independent.
Bookmark.upsert_all([{ code: "2-apples", title: "apple", visits: 17 }], unique_by: :title)
raise "partial upsert did not update" unless rel.find("2-apples").visits == 17
raise "partial upsert duplicated the live row" unless Bookmark.where(title: "apple").count == 1
Bookmark.where(code: "2-apples").update_all(retired_at: Time.now)
Bookmark.upsert_all([{ code: "fresh-41", title: "apple", visits: 29 }], unique_by: :title)
raise "partial upsert did not insert past retired row" unless Bookmark.where(title: "apple").count == 2
raise "string keys or partial index lost" unless rel.find(["fresh-41", "2-apples"]).map(&:visits) == [29, 17]
puts "ok"
"#,
        )
        .assert_passes();
}

/// Ingest hoists a file-level constant into the first class of the
/// file, but Ruby declares it on Object. The inferred value must belong
/// to the declaration that the read resolves to.
#[test]
fn a_file_level_constant_runs_from_the_class_below_it() {
    emit_and_run::real_blog()
        .write(
            "app/services/review_probe.rb",
            "ROOT_LIMIT = 7\n\nclass LimitReader\n  def self.value\n    ROOT_LIMIT\n  end\nend\n",
        )
        .run_ruby("raise 'file-level constant' unless LimitReader.value == 7")
        .assert_passes();
}

/// ERB views are not indexed. A qualified class read there must not
/// take the value of an unrelated constant with the same last segment,
/// and a qualified value read takes the value declared at its full name.
#[test]
fn a_qualified_class_in_a_view_ignores_a_same_named_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/archive.rb",
            "module Marker\n  Item = 1\nend\n\nmodule Archive\n  class Item\n    def self.label\n      \"archive\"\n    end\n  end\nend\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>",
            "<% content_for :title, \"Articles\" %>\n<p id=\"archive-label\"><%= Archive::Item.label %></p>\n<p id=\"marker-item\"><%= Marker::Item + 1 %></p>",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1\", \"Articles\"\n    assert_select \"#archive-label\", \"archive\"\n    assert_select \"#marker-item\", \"2\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Each constant in the chain reads the one before it. The value must
/// reach the end of a chain longer than any fixed number of rounds.
#[test]
fn a_long_constant_chain_reaches_its_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/chain.rb",
            "class Chain\n  A = 1\n  B = A\n  C = B\n  D = C\n  E = D\n  F = E\n\n  def self.value\n    F\n  end\nend\n",
        )
        .run_ruby("raise 'constant chain' unless Chain.value == 1")
        .assert_passes();
}

/// Rubydex declares Object, BasicObject, Kernel, Module and Class
/// itself. Those are Ruby's own classes, not unknown constants.
#[test]
fn object_new_runs_as_the_ruby_built_in() {
    emit_and_run::real_blog()
        .write(
            "app/services/object_reader.rb",
            "class ObjectReader\n  def self.value\n    Object.new\n  end\nend\n",
        )
        .write(
            "app/controllers/sentinels_controller.rb",
            "class SentinelsController < ApplicationController\n  def show\n    @sentinel = Object.new\n  end\nend\n",
        )
        .run_ruby("raise 'Object.new' unless ObjectReader.value.instance_of?(Object)")
        .assert_passes();
}

/// Writebook's `pluralize number_with_delimiter(content.split.size), "word"`:
/// keep the formatted label, and recognize a textual one as singular.
#[test]
fn a_formatted_word_count_runs() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/word_counts_helper.rb",
            r#"module WordCountsHelper
  def word_count(content)
    pluralize number_with_delimiter(content.split.size), "word"
  end

  def label(count)
    pluralize count, "word"
  end
end
"#,
        )
        .write(
            "app/views/articles/_word_count.html.erb",
            "<%= pluralize number_with_delimiter(1001), \"word\" %>",
        )
        .run_ruby(
            r#"raise "singular" unless WordCountsHelper.word_count("only") == "1 word"
raise "plural" unless WordCountsHelper.word_count("two words") == "2 words"
raise "delimiter lost" unless WordCountsHelper.word_count((["w"] * 1001).join(" ")) == "1,001 words"
raise "zero" unless WordCountsHelper.word_count("") == "0 words"
raise "text one" unless WordCountsHelper.label("1") == "1 word"
raise "decimal one" unless WordCountsHelper.label("1.00") == "1.00 word"
raise "leading zero" unless WordCountsHelper.label("01") == "01 words"
raise "fraction" unless WordCountsHelper.label("1.01") == "1.01 words"
raise "empty string" unless WordCountsHelper.label("") == " words"
raise "float one" unless WordCountsHelper.label(1.0) == "1.0 word"
raise "view" unless Views::Articles.word_count(nil) == "1,001 words"
puts "ok"
"#,
        )
        .assert_passes();
}

#[test]
fn an_app_pluralize_override_owns_helper_and_view_calls() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/application_helper.rb",
            r#"module ApplicationHelper
  def pluralize(count, word)
    "custom #{count}:#{word}"
  end

  def heading
    pluralize(1, "person")
  end
end
"#,
        )
        .write(
            "app/views/articles/_custom_count.html.erb",
            "<%= pluralize(1, \"person\") %>|<%= \"nested #{pluralize(2, 'person')}\" %>",
        )
        .run_ruby(
            r#"raise "helper override lost" unless ApplicationHelper.heading == "custom 1:person"
raise "view override lost" unless Views::Articles.custom_count(nil) == "custom 1:person|nested custom 2:person"
puts "ok"
"#,
        )
        .assert_passes();
}

/// #139 typed `Model.human_attribute_name` as a String, which took the
/// call from an error to clean, but no runtime defines it, so every
/// page rendering the form raises `undefined method
/// 'human_attribute_name' for class Article`. It belongs once, in
/// `runtime/ruby/active_record/base.rb`, where every target gets it.
#[test]
#[ignore = "check is clean but the emitted view raises NoMethodError: no runtime defines human_attribute_name (#147)"]
fn human_attribute_name_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= Article.human_attribute_name(:title) %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// #140 bound `form_with builder: X`'s block param to `X`, which took a
/// custom builder's own helpers from errors to clean. But the emitted
/// tree cannot load `X` (no runtime `ActionView::Helpers::FormBuilder`
/// to subclass), and the view calls `form.marker_field` on a `form`
/// that no longer exists, because lowering expands the stock builder
/// inline. Passing needs a builder the emitted view can call; until
/// then, the honest state is an error in `check`.
#[test]
#[ignore = "check is clean but the emitted tree fails to load: no runtime FormBuilder, and the inlined form has no builder object (#148)"]
fn a_custom_form_builder_runs() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/custom_form_builder.rb",
            "class CustomFormBuilder < ActionView::Helpers::FormBuilder\n  \
               def marker_field(name)\n    \
                 @template.content_tag(:span, name.to_s, class: \"builder-marker\")\n  \
               end\n\
             end\n",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "form_with(model: article, class: \"contents\")",
            "form_with(model: article, class: \"contents\", builder: CustomFormBuilder)",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= form.marker_field :title %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `enum :x, CONST.map { |v| [v, v.to_s] }.to_h` — Procore's
/// `bid_package.rb` computes an identity string mapping over a
/// constant instead of writing the hash literal out. Pins that the
/// generated predicate and bang-writer methods actually work against a
/// real column, not just that `check` accepts the declaration.
#[test]
fn computed_enum_map_to_h_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            // Anchored on the table, not its columns: Rails 8.1 dumps
            // columns alphabetically, older Rails in creation order.
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"kind\", default: \"post\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  \
             KINDS = %i[post announcement]\n  \
             enum :kind, KINDS.map { |k| [k, k.to_s] }.to_h",
        )
        .write(
            "test/models/article_enum_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleEnumTest < ActiveSupport::TestCase\n  \
               test \"a computed .map{}.to_h enum mapping generates working predicates\" do\n    \
                 article = articles(:one)\n    \
                 assert article.post?\n    \
                 article.announcement!\n    \
                 assert article.announcement?\n    \
                 assert_equal \"announcement\", article.kind\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_enum_test.rb")
        .assert_passes();
}

/// A module constant in another file is the same string-backed enum
/// input as an inline array. Exercise the mapping AND generated methods,
/// including a scope that must exclude the record after a persisted write.
#[test]
fn cross_file_literal_enum_mapping_runs() {
    emit_and_run::real_blog()
        .write(
            "app/services/article_states.rb",
            "module ArticleStates\n  VALUES = %w[draft active archived].freeze\nend\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"state\", default: \"draft\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, ArticleStates::VALUES.index_with(&:itself)",
        )
        .write(
            "test/models/article_cross_file_enum_test.rb",
            r#"require "test_helper"

class ArticleCrossFileEnumTest < ActiveSupport::TestCase
  test "a cross-file literal constant expands to a working string enum" do
    assert_equal({"draft" => "draft", "active" => "active", "archived" => "archived"}, Article.states)
    assert_equal "active", Article.states[:active]
    article = articles(:one)
    other = articles(:two)
    assert article.draft?
    assert_not article.active?
    article.active!
    assert_equal "active", article.reload.state
    assert article.active?
    assert_not article.draft?
    assert_equal article.id, Article.active.first.id
    assert_equal other.id, Article.draft.first.id
    assert_nil Article.archived.first
  end
end
"#,
        )
        .run_test("test/models/article_cross_file_enum_test.rb")
        .assert_passes();
}

/// `def self.included(klass); class << klass; … end; end` — Procore's
/// shared search concerns (`app/concerns/search_engine/indexed.rb` and
/// more) skip `ActiveSupport::Concern` and open the includer's
/// singleton directly from the vanilla `Module#included` hook. Pins
/// that the resulting class method is actually callable on the
/// including model, not just that `check` no longer reports the
/// `SingletonClassNode` it used to.
#[test]
fn included_hook_class_methods_run() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/sluggable.rb",
            "module Sluggable\n  \
               def self.included(klass)\n    \
                 class << klass\n      \
                   def slug_prefix\n        \
                     \"article\"\n      \
                   end\n    \
                 end\n  \
               end\n\
             end\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  include Sluggable\n\n  \
             has_many :comments, dependent: :destroy",
        )
        .write(
            "test/models/article_sluggable_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleSluggableTest < ActiveSupport::TestCase\n  \
               test \"an included-hook class method is callable on the includer\" do\n    \
                 assert_equal \"article\", Article.slug_prefix\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_sluggable_test.rb")
        .assert_passes();
}

/// `scope :x, (lambda do |v| … end)` — Procore's `reports/app/models/
/// report.rb` wraps the spelled-out `lambda`/`proc` scope body in its
/// own parens (`for_tools`, `for_data_sets`, `shared`). Before the fix
/// the parens sat between `parse_scope` and the call it expected, so
/// `check` reported "scope body must be a lambda" — this pins that the
/// emitted scope actually filters, not just that `check` goes quiet.
#[test]
fn parenthesized_lambda_scope_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "validates :body, presence: true, length: { minimum: 10 }",
            "validates :body, presence: true, length: { minimum: 10 }\n\n  \
             scope :with_title, (lambda do |value|\n    \
               where(title: value)\n  \
             end)",
        )
        .write(
            "test/models/article_scope_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleScopeTest < ActiveSupport::TestCase\n  \
               test \"a parenthesized lambda scope filters by title\" do\n    \
                 found = Article.with_title(\"Getting Started with Rails\").first\n    \
                 assert_not_nil found\n    \
                 assert_equal \"Getting Started with Rails\", found.title\n    \
                 assert_nil Article.with_title(\"No Such Title\").first\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_scope_test.rb")
        .assert_passes();
}

/// Invariant 6 pin for transitive filter-target ivar-write discovery
/// (`collect_transitive_filter_ivars` in `src/analyze/mod.rs`): a
/// `before_action` target whose body assigns no ivar itself but calls
/// another private method that does. `set_article` becomes `prepare`,
/// which calls `load_article`, which does the actual
/// `@article = Article.find(...)`, and the `show` view renders
/// `@article.title`.
///
/// `IvarUnresolved` is Error severity (`docs/pipeline/analyze.md`), so
/// this is a genuine before/after, not just an emit-side pin: verified
/// by hand against the pre-change analyzer (`git show
/// origin/main:src/analyze/mod.rs`), this exact edit made `check`
/// report eleven `@article has no known type` errors — `set_article`'s
/// replacement, `prepare`, no longer writes `@article` in its own
/// body, so without this change the write three lines away in
/// `load_article` was invisible. With the change `check` is clean AND
/// the emitted dispatcher actually SEQUENCES `prepare` (which calls
/// `load_article`) before the action body runs — the half of the claim
/// a diagnostic count alone can't make. A chain that silently dropped
/// the transitive write, or that emitted `prepare`'s body without
/// actually invoking `load_article`, would raise `NoMethodError` on
/// `nil.title` when the test hits `GET /articles/:id`.
#[test]
fn a_before_action_that_calls_another_private_method_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "before_action :set_article, only: %i[ show edit update destroy ]",
            "before_action :prepare, only: %i[ show edit update destroy ]",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    # Use callbacks to share common setup or constraints between actions.\n    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            "    # Use callbacks to share common setup or constraints between actions.\n    def prepare\n      load_article\n    end\n\n    def load_article\n      @article = Article.find(params.expect(:id))\n    end\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

const INDEX_VIEW: &str = "app/views/articles/index.html.erb";
const INDEX_HEADING: &str = "<h1 class=\"font-bold text-4xl\">Articles</h1>";
const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";
const INDEX_ASSERTION: &str = "assert_select \"h1\", \"Articles\"\n";

/// Render `calls` on the articles index, then assert `assertions` in
/// the index test. The expected HTML in each caller is Rails' output.
fn on_the_index(overlay: emit_and_run::Overlay, calls: &str, assertions: &str) -> emit_and_run::Run {
    overlay
        .edit(INDEX_VIEW, INDEX_HEADING, &format!("{INDEX_HEADING}\n{calls}"))
        .edit(CONTROLLER_TEST, INDEX_ASSERTION, &format!("{INDEX_ASSERTION}{assertions}"))
        .run_test(CONTROLLER_TEST)
}

/// B1 in NEXUS_BUGS.md: a helper keyword named `class`, read with the
/// `class:` shorthand, emitted a bare `class` and compared it with the
/// String `"nil"`. Rails leaves the attribute out for `class: nil`.
#[test]
fn a_helper_reads_a_reserved_word_keyword_with_the_shorthand_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  \
               def badge(text, class: \"badge\")\n    \
                 tag.span(text, class:)\n  \
               end\n\n  \
               def merged_badge(text, class: \"badge\", **options)\n    \
                 tag.span(text, **options.merge(class:))\n  \
               end\n\
             end\n",
        ),
        "<i id=\"b1-default\"><%= badge(\"hi\") %></i>\n\
         <i id=\"b1-given\"><%= badge(\"hi\", class: \"big\") %></i>\n\
         <i id=\"b1-nil\"><%= badge(\"hi\", class: nil) %></i>\n\
         <i id=\"b1-merged\"><%= merged_badge(\"hi\", class: \"big\", id: \"b\") %></i>\n",
        "    assert_match(/<i id=\"b1-default\"><span class=\"badge\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-given\"><span class=\"big\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-nil\"><span>hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-merged\"><span id=\"b\" class=\"big\">hi<\\/span><\\/i>/, response.body)\n",
    );
    run.assert_passes();
}

/// A String keyword that no caller passes as nil keeps Rails' nil
/// rule: `class: "nil"` renders `class="nil"`. The inquiry lowering
/// read `value.nil?` as `StringInquirer#nil?` and emitted
/// `value == "nil"`, which dropped the attribute.
#[test]
fn a_string_keyword_named_nil_keeps_its_attribute() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  \
               def badge(text, class: \"badge\")\n    \
                 tag.span(text, class: binding.local_variable_get(:class))\n  \
               end\n\
             end\n",
        ),
        "<i id=\"n-literal\"><%= badge(\"hi\", class: \"nil\") %></i>\n\
         <i id=\"n-default\"><%= badge(\"hi\") %></i>\n",
        "    assert_match(/<i id=\"n-literal\"><span class=\"nil\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"n-default\"><span class=\"badge\">hi<\\/span><\\/i>/, response.body)\n",
    );
    run.assert_passes();
}

/// B2 in NEXUS_BUGS.md: strict locals named after reserved words
/// emitted a positional `for` parameter. Nexus reads them with
/// `local_assigns`; the repro reads them with `binding`.
#[test]
fn a_partial_with_reserved_word_strict_locals_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/views/articles/_empty_la.html.erb",
                "<%# locals: (for:, class: \"\") %>\n\
                 <p id=\"b2-la\" class=\"<%= local_assigns[:class] %>\"><%= local_assigns[:for] %></p>\n",
            )
            .write(
                "app/views/articles/_empty_bind.html.erb",
                "<%# locals: (for:, class: \"\") %>\n\
                 <p id=\"b2-bind\" class=\"<%= binding.local_variable_get(:class) %>\"><%= binding.local_variable_get(:for) %></p>\n",
            ),
        "<%= render \"empty_la\", for: Article, class: \"muted\" %>\n\
         <%= render \"empty_la\", for: Article %>\n\
         <%= render \"empty_bind\", for: Article, class: \"muted\" %>\n",
        "    assert_match(/<p id=\"b2-la\" class=\"muted\">Article<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b2-la\" class=\"\">Article<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b2-bind\" class=\"muted\">Article<\\/p>/, response.body)\n",
    );
    run.assert_passes();
}

/// B3 in NEXUS_BUGS.md: `local_assigns[:class]` in a partial without
/// strict locals emitted a positional `class` parameter.
#[test]
fn a_partial_reading_a_reserved_word_local_assign_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/views/articles/_card.html.erb",
            "<div id=\"b3\" class=\"card <%= local_assigns[:class] %>\"><%= title %></div>\n",
        ),
        "<%= render \"card\", title: \"hi\", class: \"wide\" %>\n\
         <%= render \"card\", title: \"hi\" %>\n",
        "    assert_match(/<div id=\"b3\" class=\"card wide\">hi<\\/div>/, response.body)\n    \
             assert_match(/<div id=\"b3\" class=\"card \">hi<\\/div>/, response.body)\n",
    );
    run.assert_passes();
}

/// B4 in NEXUS_BUGS.md: a partial in `app/views/application/` that a
/// view in another directory renders. Rails looks in the view's own
/// directory first, so a same-name partial there wins.
#[test]
fn a_partial_in_the_application_view_directory_runs_from_another_directory() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/views/application/_blank_slate.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-app\"><%= message %></p>\n",
            )
            .write(
                "app/views/application/_shadowed.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-shadow-app\"><%= message %></p>\n",
            )
            .write(
                "app/views/articles/_shadowed.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-shadow-own\"><%= message %></p>\n",
            ),
        "<%= render \"blank_slate\", message: \"No articles\" %>\n\
         <%= render \"shadowed\", message: \"own dir\" %>\n",
        "    assert_match(/<p id=\"b4-app\">No articles<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b4-shadow-own\">own dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-shadow-app/, response.body)\n",
    );
    run.assert_passes();
}

/// B4 in NEXUS_BUGS.md: `render "x", k: v` looks in the view directory
/// of each controller ancestor, nearest first. The `application`
/// directory is only reached when the chain reaches ApplicationController.
#[test]
fn a_partial_in_a_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/base_controller.rb",
                "class BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < BaseController",
            )
            .write(
                "app/views/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-base\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render \"nav\", label: \"parent dir\" %>\n",
        "    assert_match(/<p id=\"b4-base\">parent dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// The `render partial: "x", locals: { ... }` spelling resolves
/// through the parent controller's view directory too.
#[test]
fn a_partial_keyword_in_a_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/base_controller.rb",
                "class BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < BaseController",
            )
            .write(
                "app/views/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-base\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render partial: \"nav\", locals: { label: \"hash form\" } %>\n",
        "    assert_match(/<p id=\"b4-base\">hash form<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// A namespaced parent controller (`Admin::BaseController`) has the
/// view directory `admin/base`, which wins over `application`.
#[test]
fn a_partial_in_a_namespaced_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/admin/base_controller.rb",
                "class Admin::BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < Admin::BaseController",
            )
            .write(
                "app/views/admin/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-admin\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render \"nav\", label: \"admin dir\" %>\n",
        "    assert_match(/<p id=\"b4-admin\">admin dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// Not handed to the csv gem's `headers:`: the lowering writes the header row itself, so the output is held to what CSV.generate answers.
#[test]
fn csv_generate_with_written_headers_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def csv_probe
    CSV.generate(headers: ["id", "title"], write_headers: true) do |csv|
      csv << [1, "x"]
      csv << [2, "y,z"]
    end
  end"##,
        )
        .write(
            "test/models/article_csv_test.rb",
            r#"require "test_helper"

class ArticleCsvTest < ActiveSupport::TestCase
  test "CSV.generate writes its headers as the first row" do
    assert_equal "id,title\n1,x\n2,\"y,z\"\n", Article.new.csv_probe
  end
end
"#,
        )
        .run_test("test/models/article_csv_test.rb")
        .assert_passes();
}

/// Not left on the receiver: no ruby-family runtime ships `ActiveModel::Type::Boolean` or the ActiveSupport key conversions.
#[test]
fn boolean_cast_and_key_conversions_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def as_hash_boolean_probe(flag)
    on = ActiveModel::Type::Boolean.new.cast(flag)
    h = { a: "x", b: "y" }
    [on.inspect, h.stringify_keys.keys.join(","), h.deep_symbolize_keys.keys.join(","), h.symbolize_keys.size, { "c" => 1 }.stringify_keys.keys.first].join("|")
  end"##,
        )
        .write(
            "test/models/article_as_hash_boolean_test.rb",
            r#"require "test_helper"

class ArticleAsHashBooleanTest < ActiveSupport::TestCase
  test "Boolean#cast and the key conversions answer as ActiveModel and ActiveSupport do" do
    article = Article.new
    assert_equal "false|a,b|a,b|2|c", article.as_hash_boolean_probe("off")
    assert_equal "true|a,b|a,b|2|c", article.as_hash_boolean_probe("1")
    assert_equal "nil|a,b|a,b|2|c", article.as_hash_boolean_probe("")
  end
end
"#,
        )
        .run_test("test/models/article_as_hash_boolean_test.rb")
        .assert_passes();
}

/// Not a diagnostic count: each core method's value is held to what CRuby answers.
#[test]
fn core_integer_float_string_array_methods_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def core_surface_probe
    n = 17
    ints = [n.clamp(1, 10), n.div(5), n.modulo(5), n.gcd(4), n.lcm(4), n.pow(2), n.bit_length, n.divmod(5).first]
    f = 7.5
    floats = [n.fdiv(2), f.clamp(1.0, 5.0), f.modulo(2.0), f.fdiv(2)]
    ups = 1.upto(3).map { |i| i * 2 }
    downs = 3.downto(1).map { |i| i }
    steps = 0.step(6, 3).map { |i| i }
    seen = []
    3.times { |i| seen << i }
    labels = %w[a b c].map.with_index { |s, i| "#{i}#{s}" }
    arr = [3, 1, 2]
    arr.sort_by! { |x| -x }
    arr.select! { |x| x > 1 }
    arr.insert(1, 9)
    s = "hello".dup
    s.gsub!("l", "L")
    [ints.join(","), floats.join(","), ups.join(","), downs.join(","), steps.join(","), seen.join(","), labels.join(","), arr.join(","), s, Regexp.escape("a.b")].join("|")
  end"##,
        )
        .write(
            "test/models/article_core_surface_test.rb",
            r#"require "test_helper"

class ArticleCoreSurfaceTest < ActiveSupport::TestCase
  test "core Integer, Float, String and Array methods answer as CRuby does" do
    assert_equal "10,3,2,1,68,289,5,3|8.5,5.0,1.5,3.75|2,4,6|3,2,1|0,3,6|0,1,2|0a,1b,2c|3,9,2|heLLo|a\\.b",
                 Article.new.core_surface_probe
  end
end
"#,
        )
        .run_test("test/models/article_core_surface_test.rb")
        .assert_passes();
}

/// Not only the `check` side: an enum, scope and method a namespaced abstract base declares have to run when another model reaches them.
#[test]
fn an_abstract_base_reaches_its_model() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .write(
            "app/models/base_model/content_base.rb",
            r#"class BaseModel::ContentBase < ApplicationRecord
  self.abstract_class = true
  self.table_name = "articles"
  enum :state, { draft: 0, live: 1 }
  scope :titled, -> { where.not(title: nil) }

  def shout
    title.to_s.upcase
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < BaseModel::ContentBase\n  has_many :comments, dependent: :destroy\n  scope :newest_first, -> { order(id: :desc) }",
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord",
            r#"class Comment < ApplicationRecord
  def base_probe
    a = Article.find(article_id)
    [article.draft?, a.shout, Article.titled.newest_first.to_a.size, Article.newest_first.titled.first.nil?, Article.draft.count].join("|")
  end
"#,
        )
        .write(
            "test/models/comment_abstract_base_test.rb",
            r#"require "test_helper"

class CommentAbstractBaseTest < ActiveSupport::TestCase
  test "another model reaches an abstract base's enum, scope and method" do
    comment = comments(:one)
    title = comment.article.title.to_s.upcase
    assert_equal "true|#{title}|#{Article.count}|false|#{Article.count}", comment.base_probe
  end
end
"#,
        )
        .run_test("test/models/comment_abstract_base_test.rb")
        .assert_passes();
}

/// Not a plain Hash: Rails' mapping reads `statuses[:paid]` as the `"paid"` entry, so the Symbol keys are held to that.
#[test]
fn an_enum_plural_mapping_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  enum :state, { "draft" => 0, "live" => 1, "on hold" => 2 }

  def self.mapping_probe
    [states.keys.join(","), states[:live], self.states["draft"], states.key?(:live), states.fetch(:draft), states.key(1), states.map { |k, v| "#{k}=#{v}" }.join(";")].join("|")
  end

  def live_value
    self.class.states[:live]
  end"##,
        )
        .write(
            "test/models/article_enum_mapping_test.rb",
            r#"require "test_helper"

class ArticleEnumMappingTest < ActiveSupport::TestCase
  test "the plural mapping answers as Rails' indifferent Hash does" do
    assert_equal "draft,live,on hold|1|0|true|0|live|draft=0;live=1;on hold=2", Article.mapping_probe
    assert_equal 1, articles(:one).live_value
    assert_equal 0, Article.states.fetch(:draft)
    assert_equal 2, Article.states[:"on hold"]
  end
end
"#,
        )
        .run_test("test/models/article_enum_mapping_test.rb")
        .assert_passes();
}

/// Not left on the receiver: no ruby-family runtime ships Numeric#to_fs or an errors object with `messages`.
#[test]
fn delimited_numbers_and_errors_messages_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def errors_probe
    bad = Article.new(title: "", body: "short")
    bad.valid?
    good = Article.new(title: "t", body: "long enough body")
    good.valid?
    [bad.errors.messages.key?(:title), bad.errors.messages.key?(:created_at), bad.errors.messages.blank?, bad.errors.messages[:body].join(";"),
     good.errors.messages.blank?, good.errors.messages.present?, 1234567.to_fs(:delimited), 1234.5.to_fs(:delimited), -1234.to_fs(:delimited)].join("|")
  end"##,
        )
        .write(
            "test/models/article_errors_messages_test.rb",
            r#"require "test_helper"

class ArticleErrorsMessagesTest < ActiveSupport::TestCase
  test "errors.messages and to_fs(:delimited) answer as ActiveModel and ActiveSupport do" do
    assert_equal "true|false|false|is too short (minimum is 10 characters)|true|false|1,234,567|1,234.5|-1,234",
                 Article.new.errors_probe
  end
end
"#,
        )
        .run_test("test/models/article_errors_messages_test.rb")
        .assert_passes();
}

/// Not one value per process: a `thread_mattr_accessor` written on one thread reads nil on another, as in Rails.
#[test]
fn a_thread_mattr_accessor_runs_per_thread() {
    emit_and_run::real_blog()
        .write(
            "lib/request_context.rb",
            "module RequestContext\n  thread_mattr_accessor :article\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def remember
    RequestContext.article = self
  end

  def self.remembered_title
    RequestContext.article.title
  end"#,
        )
        .run_ruby(
            r#"article = Article.new(title: "kept", body: "long enough body")
article.remember
raise "the writing thread lost it" unless Article.remembered_title == "kept"
raise "another thread saw it" unless Thread.new { RequestContext.article.nil? }.value
puts "ok"
"#,
        )
        .assert_passes();
}

/// Not a String: `params.expect(article: [...])` answers the permitted hash, so a `merge` onto it has to run as one.
#[test]
fn an_expected_params_hash_merges() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            "    @article = Article.new(article_params.merge(title: \"Merged Title\"))\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"Merged Title\", Article.last.title\n    assert_equal \"A sufficiently long body for validation.\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not read with the Symbol keys the source writes: every hash in a request's params is String-keyed at run time.
#[test]
fn nested_request_params_read_as_the_request_carried_them() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    if params[:extra].present? && params[:extra].key?(:suffix)
      @article.title = @article.title.to_s + " " + params[:extra][:suffix].to_s
    end
    if params[:tags].present?
      params[:tags].each { |t| @article.body = @article.body.to_s + " #" + t.to_s }
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\" }, tags: [\"a\", \"b\"] }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. #a #b\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not left as `permit`/`to_unsafe_h`/`require`: the emitted request's params are plain hashes, which answer none of them.
#[test]
fn nested_request_params_permit_and_require_run() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    if params.to_unsafe_h.key?(:extra)
      @article.title = @article.title.to_s + " " + params[:extra].permit(:suffix)[:suffix].to_s
      @article.body = @article.body.to_s + " " + params[:extra].to_unsafe_h.keys.join(",") + " " + params[:extra].require(:suffix).to_s
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\", other: \"x\" } }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. suffix,other Extra\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not only a `params[...]` chain: a local assigned from one holds the same request value, and reads through it the same way.
#[test]
fn a_local_holding_a_request_params_value_reads_it() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    extra = params[:extra]
    tags = params[:tags]
    if extra.present? && extra.key?(:suffix)
      @article.title = @article.title.to_s + " " + extra[:suffix].to_s
    end
    if tags.present?
      tags.each { |t| @article.body = @article.body.to_s + " #" + t.to_s }
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\" }, tags: [\"a\", \"b\"] }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. #a #b\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Gap F15 took `&method(:name)` from an ingest error (block-argument
/// forms other than `&:symbol`/`&local_var` were unsupported) to
/// clean, emitting `&method(:name)` verbatim (`ExprNode::MethodRef`).
/// Invariant 6: prove the emitted PROGRAM runs it, not just that
/// `check` stays quiet — a PORO under `app/lib` maps an array through
/// a bound-method reference to its own helper, and a model test reads
/// the result back.
#[test]
fn method_ref_block_arg_runs() {
    emit_and_run::real_blog()
        .write(
            "app/lib/doubler.rb",
            "class Doubler\n  \
               def self.doubled(list)\n    \
                 list.map(&method(:double))\n  \
               end\n\n  \
               def self.double(n)\n    \
                 n * 2\n  \
               end\n\
             end\n",
        )
        .write(
            "test/models/doubler_test.rb",
            "require \"test_helper\"\n\n\
             class DoublerTest < ActiveSupport::TestCase\n  \
               test \"&method(:name) as a block argument runs\" do\n    \
                 assert_equal [2, 4, 6], Doubler.doubled([1, 2, 3])\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/doubler_test.rb")
        .assert_passes();
}

/// A clean factory call must construct the receiving T::Struct, not
/// the concern or whichever includer was seen first. Exercise native
/// emitted consumers as well as the objects, independently of the
/// analyzer's inferred return types (invariant 6).
#[test]
fn a_shared_struct_factory_runs_for_both_includers_in_both_orders() {
    let reading = "class Reading < T::Struct\n  include Factory\n  PREFIX = \"local:\"\n  const :label, String\nend\n";
    let packet = "class Packet < T::Struct\n  include Factory\n  const :size, Integer\nend\n";
    for declarations in [format!("{reading}{packet}"), format!("{packet}{reading}")] {
        emit_and_run::real_blog()
            .write("app/services/factory.rb", r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
    def prefix
      "old:"
    end
  end
  def self.prefix
    "initial module:"
  end
end
"#)
            // Reopening a carrier must retain build and take the newer
            // prefix, whose default still belongs to Factory's scope.
            .write("app/services/factory_extension.rb", r#"module Factory
  PREFIX = "reading:"
  module ClassMethods
    def fixed
      Reading.new(label: "fixed").freeze
    end
    def prefix(value = PREFIX)
      value
    end
  end
  def self.prefix
    "module:"
  end
end
"#)
            .write("app/services/values.rb", &declarations)
            .write("app/services/factory_consumer.rb", r#"class FactoryConsumer
  def self.label
    Reading.prefix + Reading.build(label: "sensor").label.upcase
  end
  def self.size
    Packet.build(size: 7).size * 3
  end
end
"#)
            .write("app/controllers/factory_probes_controller.rb", r#"class FactoryProbesController < ApplicationController
  def index
    @label = Reading.build(label: "probe").label
    @size = Packet.build(size: 7).size
    render plain: FactoryConsumer.label
  end
end
"#)
            .run_ruby(r#"
reading = Reading.build(label: "probe")
packet = Packet.build(size: 7)
raise "reading identity" unless reading.class == Reading
raise "reading field" unless reading.label == "probe"
raise "reading freeze" unless reading.frozen?
raise "packet identity" unless packet.class == Packet
raise "packet field" unless packet.size == 7
raise "packet freeze" unless packet.frozen?
raise "label consumer" unless FactoryConsumer.label == "reading:SENSOR"
raise "size consumer" unless FactoryConsumer.size == 21
raise "module singleton" unless Factory.prefix == "module:"
fixed = Packet.fixed
raise "fixed-other identity" unless fixed.class == Reading
raise "fixed-other field" unless fixed.label == "fixed"
raise "fixed-other freeze" unless fixed.frozen?
"#)
            .assert_passes();
    }
}

/// A literal table override must reach the emitted row readers and SQL,
/// not merely quiet the analyzer. Two differently named models share the
/// real articles table via string/symbol declarations; writes through either
/// must be visible to Article, without touching convention-derived decoys.
#[test]
fn explicit_model_table_names_run_against_the_declared_table() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            r#"  create_table "archived_articles" do |t|
    t.string "title"
    t.text "body"
  end
  create_table "ledger_entries" do |t|
    t.string "title"
    t.text "body"
  end
  add_foreign_key "comments", "articles""#,
        )
        .write(
            "app/models/archived_article.rb",
            "class ArchivedArticle < ApplicationRecord\n  self.table_name = \"articles\"\nend\n",
        )
        .write(
            "app/models/ledger/entry.rb",
            r#"module Ledger
  def self.table_name_prefix
    "ledger_"
  end
  class Entry < ApplicationRecord
    self.table_name = :"art\x69cles"
  end
end
"#,
        )
        .write(
            "app/models/decoy/archived_article.rb",
            "class Decoy::ArchivedArticle < ApplicationRecord\nend\n",
        )
        .write(
            "app/models/ledger_entry.rb",
            "class LedgerEntry < ApplicationRecord\nend\n",
        )
        .run_ruby(
            r#"raise ArchivedArticle.table_name.inspect unless ArchivedArticle.table_name == "articles"
raise Ledger::Entry.table_name.inspect unless Ledger::Entry.table_name == "articles"
decoy = Decoy::ArchivedArticle.create!(title: "Conventional decoy", body: "Leave untouched")
prefixed_decoy = LedgerEntry.create!(title: "Prefixed decoy", body: "Leave untouched too")
record = ArchivedArticle.create!(title: "Original", body: "A long enough body")
raise unless Article.find(record.id).title == "Original"
entry = Ledger::Entry.find(record.id)
raise unless entry.title == "Original"
entry.update!(title: "Changed")
raise unless Article.find(record.id).title == "Changed"
entry.destroy!
raise unless Article.find_by(id: record.id).nil?
symbol_record = Ledger::Entry.create!(title: "Symbol-created", body: "A long enough body")
raise unless Article.find(symbol_record.id).title == "Symbol-created"
ArchivedArticle.find(symbol_record.id).update!(title: "String-updated")
raise unless Ledger::Entry.find(symbol_record.id).title == "String-updated"
Ledger::Entry.find(symbol_record.id).destroy!
raise unless Article.find_by(id: symbol_record.id).nil?
raise unless Decoy::ArchivedArticle.count == 1
raise unless Decoy::ArchivedArticle.find(decoy.id).title == "Conventional decoy"
raise unless LedgerEntry.count == 1
raise unless LedgerEntry.find(prefixed_decoy.id).title == "Prefixed decoy"
"#,
        )
        .assert_passes();
}

/// Not only `Model.scope`: a scope the target model inherits from an abstract base answers on an association reaching it too.
#[test]
fn an_inherited_scope_answers_on_an_association() {
    emit_and_run::real_blog()
        .write(
            "app/models/remark_base.rb",
            "class RemarkBase < ApplicationRecord\n  self.abstract_class = true\n  scope :by_alice, -> { where(commenter: \"Alice\") }\nend\n",
        )
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord", "class Comment < RemarkBase")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def alice_count\n    comments.by_alice.count\n  end",
        )
        .write(
            "test/models/article_inherited_scope_test.rb",
            r#"require "test_helper"

class ArticleInheritedScopeTest < ActiveSupport::TestCase
  test "an association answers a scope its model inherits" do
    article = articles(:one)
    before = article.alice_count
    article.comments.create!(commenter: "Alice", body: "A comment from Alice.")
    article.comments.create!(commenter: "Bob", body: "A comment from Bob.")
    assert_equal before + 1, article.alice_count
  end
end
"#,
        )
        .run_test("test/models/article_inherited_scope_test.rb")
        .assert_passes();
}

/// Not only a string default: schema.rb's unquoted `default: true`, `default: 1.5` and `default: -3` reach a new record, and a value the caller passes still wins.
#[test]
fn a_schema_default_that_is_not_a_string_seeds_a_new_record() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.boolean \"visible\", default: true\n    t.boolean \"listed\", default: true, null: false\n    t.float \"score\", default: 1.5\n    t.integer \"rank\", default: 7\n    t.integer \"offset\", default: -3, null: false\n    t.integer \"state\", default: 1, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }",
        )
        .write(
            "test/models/article_default_test.rb",
            r#"require "test_helper"

class ArticleDefaultTest < ActiveSupport::TestCase
  test "an unset column takes its schema default" do
    article = Article.new
    assert_equal true, article.visible
    assert_equal true, article.listed
    assert_equal 1.5, article.score
    assert_equal 7, article.rank
    assert_equal(-3, article.offset)
    assert article.published?
  end

  test "a created record keeps the default" do
    article = Article.create!(title: "Defaults", body: "A body long enough to validate.")
    reloaded = Article.find(article.id)
    assert_equal true, reloaded.visible
    assert_equal 7, reloaded.rank
    assert reloaded.published?
  end

  test "a value the caller passes wins over the default" do
    article = Article.new(visible: false, listed: false, rank: nil, score: nil, state: "draft")
    assert_equal false, article.visible
    assert_equal false, article.listed
    assert_nil article.rank
    assert_nil article.score
    assert article.draft?
  end
end
"#,
        )
        .run_test("test/models/article_default_test.rb")
        .assert_passes();
}

/// Not a NoMethodError: an enum's `not_<label>` scope and `<column>_before_type_cast` exist, as Rails generates them.
#[test]
fn an_enum_negative_scope_and_before_type_cast_run() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false\n    t.string \"tone\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n  enum :tone, { quiet: \"q\", loud: \"l\" }, prefix: true",
        )
        .write(
            "test/models/article_enum_scope_test.rb",
            r#"require "test_helper"

class ArticleEnumScopeTest < ActiveSupport::TestCase
  test "negative scopes" do
    article = Article.create!(title: "Scopes", body: "A body long enough to validate.", state: :published, tone: :loud)
    assert_equal 1, Article.not_draft.where(id: article.id).count
    assert_equal 0, Article.not_published.where(id: article.id).count
    assert_equal 0, Article.not_tone_loud.where(id: article.id).count
  end

  test "the stored value before the label" do
    article = Article.create!(title: "Raw", body: "A body long enough to validate.", state: :published, tone: :loud)
    reloaded = Article.find(article.id)
    assert_equal 1, reloaded.state_before_type_cast
    assert_equal "l", reloaded.tone_before_type_cast
  end
end
"#,
        )
        .run_test("test/models/article_enum_scope_test.rb")
        .assert_passes();
}

/// Not a NoMethodError: `read_attribute`/`write_attribute` are a model's `[]`/`[]=`, inside the model and on a record alike.
#[test]
fn read_and_write_attribute_reach_the_column() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n\n  def shout_title\n    write_attribute(:title, read_attribute(:title).upcase)\n  end",
        )
        .write(
            "test/models/article_attribute_test.rb",
            r#"require "test_helper"

class ArticleAttributeTest < ActiveSupport::TestCase
  test "read_attribute and write_attribute on a record" do
    article = articles(:one)
    article.write_attribute(:state, "published")
    assert_equal "published", article.read_attribute(:state)
    assert_equal "published", article.read_attribute("state")
    article.save!
    assert Article.find(article.id).published?
  end

  test "the bare forms inside the model" do
    article = Article.new(title: "quiet")
    article.shout_title
    assert_equal "QUIET", article.title
  end
end
"#,
        )
        .run_test("test/models/article_attribute_test.rb")
        .assert_passes();
}

/// A schema-less json/jsonb column is decoded at the public attribute
/// boundary and encoded again on assignment. In particular, the three
/// Rails spellings (`record.data`, `record[:data]`, and
/// `read_attribute(:data)`) must not expose SQLite's serialized text.
#[test]
fn json_columns_round_trip_decoded_values() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.json \"metadata\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def metadata_names\n    read_attribute(:metadata).map { |entry| entry[\"name\"] }\n  end",
        )
        .write(
            "test/models/article_json_column_test.rb",
            r#"require "test_helper"

class ArticleJsonColumnTest < ActiveSupport::TestCase
  test "json values are decoded on every public read and encoded on write" do
    value = [{ "name" => "Ada", "enabled" => true }]
    article = Article.create!(title: "JSON", body: "A body long enough to validate.", metadata: value)
    article = Article.find(article.id)

    assert_equal value, article.metadata
    assert_equal value, article[:metadata]
    assert_equal value, article.attributes["metadata"]
    assert_equal ["Ada"], article.metadata_names

    article.write_attribute(:metadata, { "name" => "Grace", "enabled" => false })
    article.save!
    assert_equal({ "name" => "Grace", "enabled" => false }, Article.find(article.id).metadata)
  end
end
"#,
        )
        .run_test("test/models/article_json_column_test.rb")
        .assert_passes();
}

/// Not the column default: `enum …, default:` is the value Rails gives an unset attribute, and a value the caller passes still wins.
#[test]
fn an_enum_default_option_seeds_a_new_record() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"priority\"\n    t.string \"tone\", default: \"quiet\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :priority, { low: 0, high: 1 }, default: :high\n  enum :tone, { quiet: \"quiet\", loud: \"loud\" }, default: :loud",
        )
        .write(
            "test/models/article_enum_default_test.rb",
            r#"require "test_helper"

class ArticleEnumDefaultTest < ActiveSupport::TestCase
  test "an unset enum takes the declared default" do
    article = Article.new
    assert article.high?
    assert article.loud?
  end

  test "a created record keeps it" do
    article = Article.create!(title: "Defaults", body: "A body long enough to validate.")
    reloaded = Article.find(article.id)
    assert_equal "high", reloaded.priority
    assert_equal "loud", reloaded.tone
  end

  test "a value the caller passes wins" do
    article = Article.new(priority: :low, tone: :quiet)
    assert article.low?
    assert article.quiet?
  end
end
"#,
        )
        .run_test("test/models/article_enum_default_test.rb")
        .assert_passes();
}

/// Not stored as 0: a label no mapping names raises ArgumentError as Rails' enum type does, and a string-backed enum reads back its label.
#[test]
fn an_enum_rejects_a_label_it_does_not_name() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false\n    t.integer \"priority\"\n    t.string \"tone\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n  enum :priority, { low: 0, high: 1 }\n  enum :tone, { quiet: \"q\", loud: \"l\" }",
        )
        .write(
            "test/models/article_enum_reject_test.rb",
            r#"require "test_helper"

class ArticleEnumRejectTest < ActiveSupport::TestCase
  test "a label the mapping does not name raises" do
    labels = ["bogus", "Draft", "loud!"]
    article = articles(:one)
    assert_raises(ArgumentError) { article.state = labels[0] }
    assert_raises(ArgumentError) { article.state = labels[1] }
    assert_raises(ArgumentError) { article.update(state: labels[0]) }
    assert_raises(ArgumentError) { article[:state] = labels[0] }
    assert_raises(ArgumentError) { Article.new(state: labels[0]) }
    assert_raises(ArgumentError) { article.tone = labels[2] }
    assert_equal "draft", Article.find(article.id).state
  end

  test "a blank value clears a nullable enum" do
    blank = ""
    article = Article.new(priority: "high")
    article.priority = blank
    assert_nil article.priority
  end

  test "a string-backed enum stores its value and reads its label" do
    article = Article.create!(title: "Tone", body: "A body long enough to validate.", tone: :loud)
    reloaded = Article.find(article.id)
    assert_equal "loud", reloaded.tone
    assert reloaded.loud?
    assert_equal 1, Article.loud.where(id: article.id).count
    assert_equal({ "quiet" => "q", "loud" => "l" }, Article.tones)
  end
end
"#,
        )
        .run_test("test/models/article_enum_reject_test.rb")
        .assert_passes();
}

/// Not `"published".to_i`: an enum column assigned a label at run time stores the label's value, as Rails does.
#[test]
fn an_enum_label_assigned_at_run_time_stores_its_value() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }",
        )
        .write(
            "test/models/article_enum_label_test.rb",
            r#"require "test_helper"

class ArticleEnumLabelTest < ActiveSupport::TestCase
  test "a label reaching the setter, update or []= stores its value" do
    labels = ["published", "draft"]
    article = articles(:one)

    article.state = labels[0]
    article.save!
    assert Article.find(article.id).published?

    article.update(state: labels[1])
    assert Article.find(article.id).draft?

    article[:state] = labels[0]
    article.save!
    assert Article.find(article.id).published?
    assert_equal 0, Article.where(state: :draft).where(id: article.id).count
  end
end
"#,
        )
        .run_test("test/models/article_enum_label_test.rb")
        .assert_passes();
}

/// Not the stored integer: an integer-mapped enum reads back its label, as Rails' reader, `[]` and `attributes` do.
#[test]
fn an_integer_enum_reads_back_its_label() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n\n  def state_label\n    state.humanize\n  end",
        )
        .write(
            "test/models/article_enum_reader_test.rb",
            r#"require "test_helper"

class ArticleEnumReaderTest < ActiveSupport::TestCase
  test "an integer enum reads back its label" do
    article = articles(:one)
    article.update(state: :published)
    reloaded = Article.find(article.id)
    assert_equal "published", reloaded.state
    assert_equal "Published", reloaded.state_label
    assert_equal "published", reloaded[:state]
    assert_equal "published", reloaded.attributes["state"]
    assert reloaded.published?
    assert !reloaded.draft?
    assert reloaded.state == "published"
    assert_equal 1, Article.where(state: :published).where(id: article.id).count
  end
end
"#,
        )
        .run_test("test/models/article_enum_reader_test.rb")
        .assert_passes();
}

/// A Concern's enum must reach a concrete child's readers and query mapping,
/// without replacing that child's own enum or leaking to an unrelated model.
#[test]
fn an_abstract_bases_concern_enum_runs_on_its_child() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|",
            "create_table \"comments\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .write(
            "app/models/concerns/publication_state.rb",
            "module PublicationState\n  extend ActiveSupport::Concern\n  included { enum :state, { draft: 0, live: 3 } }\nend\n",
        )
        .write(
            "app/models/concerns/local_state.rb",
            "module LocalState\n  extend ActiveSupport::Concern\n  included { enum :state, { queued: 2, shipped: 7 } }\nend\n",
        )
        .write(
            "app/models/content_base.rb",
            "class ContentBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  include PublicationState\nend\n",
        )
        .write(
            "app/models/direct_base.rb",
            "class DirectBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  enum :state, { draft: 0, live: 3 }\nend\n",
        )
        .write(
            "app/models/special_article.rb",
            r#"class SpecialArticle < DirectBase
  self.table_name = "articles"
  include LocalState

  def self.shipped_count(id)
    where(state: :shipped).where(id: id).count
  end

  def self.queued_count(id)
    where(state: :queued).where(id: id).count
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            r#"class Article < ContentBase
  def self.live_count(id)
    where(state: :live).where(id: id).count
  end

  def self.draft_count(id)
    where(state: :draft).where(id: id).count
  end"#,
        )
        // Query the app-owned methods: the global lowering deliberately
        // cannot guess a test-side `state` label across conflicting maps.
        .write(
            "test/models/concern_enum_inheritance_test.rb",
            r#"require "test_helper"

class ConcernEnumInheritanceTest < ActiveSupport::TestCase
  test "a base Concern supplies labels and stored query values to its child" do
    article = articles(:one)
    article.update(state: :live)
    reloaded = Article.find(article.id)
    assert_equal "live", reloaded.state
    assert_equal "live", reloaded[:state]
    assert_equal "live", reloaded.attributes["state"]
    assert reloaded.live?
    assert !reloaded.draft?
    assert_equal 1, Article.live_count(article.id)
    assert_equal 1, Article.where(state: 3).where(id: article.id).count
    assert_equal 0, Article.draft_count(article.id)
  end

  test "a child Concern keeps its own asymmetric enum mapping" do
    article = SpecialArticle.create!(title: "Override", body: "Long enough body", state: :shipped)
    reloaded = SpecialArticle.find(article.id)
    assert_equal "shipped", reloaded.state
    assert_equal "shipped", reloaded[:state]
    assert_equal "shipped", reloaded.attributes["state"]
    assert reloaded.shipped?
    assert !reloaded.queued?
    assert_equal 1, SpecialArticle.shipped_count(article.id)
    assert_equal 1, SpecialArticle.where(state: 7).where(id: article.id).count
    assert_equal 0, SpecialArticle.queued_count(article.id)
  end

  test "an unrelated model keeps an ordinary integer reader" do
    comment = Comment.new(state: 3)
    assert_equal 3, comment.state
    assert_equal 3, comment[:state]
    assert_equal 3, comment.attributes["state"]
    assert !comment.respond_to?(:live?)
    assert !comment.respond_to?(:shipped?)
  end
end
"#,
        )
        .run_test("test/models/concern_enum_inheritance_test.rb")
        .assert_passes();
}

/// Not left on the String: `humanize` and `titleize` are ActiveSupport reopens, answered here as ActiveSupport does.
#[test]
fn string_humanize_and_titleize_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.inflections_probe
    ["employee_salary".humanize, "author_id".humanize, "hello-world".titleize, "SSLError".titleize, "raiders_of_the_lost_ark".titleize].join("|")
  end"#,
        )
        .write(
            "test/models/article_inflections_test.rb",
            r#"require "test_helper"

class ArticleInflectionsTest < ActiveSupport::TestCase
  test "humanize and titleize answer as ActiveSupport does" do
    assert_equal "Employee salary|Author|Hello World|Ssl Error|Raiders Of The Lost Ark", Article.inflections_probe
  end
end
"#,
        )
        .run_test("test/models/article_inflections_test.rb")
        .assert_passes();
}

/// The exact inclusion bridge and its carrier can be in different
/// reopenings. Consuming the bridge must follow the carrier's identity,
/// not whether both declarations happened to share a source file.
#[test]
fn a_reopened_factory_carrier_runs_with_its_bridge_in_either_file_order() {
    for (bridge, carrier) in [
        ("app/services/factory.rb", "app/services/factory_extension.rb"),
        ("app/services/factory_bridge.rb", "app/services/factory.rb"),
    ] {
        emit_and_run::real_blog()
            .write(bridge, r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
end
"#)
            .write(carrier, r#"module Factory
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
  end
end
"#)
            .write("app/services/values.rb", r#"class Reading < T::Struct
  include Factory
  const :label, String
end
class Packet < T::Struct
  include Factory
  const :size, Integer
end
"#)
            .write("app/services/split_factory_consumer.rb", r#"class SplitFactoryConsumer
  def self.label
    Reading.build(label: "split").label.upcase
  end
  def self.size
    Packet.build(size: 11).size * 3
  end
end
"#)
            .run_ruby(r#"
reading = Reading.build(label: "split")
packet = Packet.build(size: 11)
raise "split reading identity" unless reading.class == Reading
raise "split reading field" unless reading.label == "split"
raise "split reading freeze" unless reading.frozen?
raise "split packet identity" unless packet.class == Packet
raise "split packet field" unless packet.size == 11
raise "split packet freeze" unless packet.frozen?
raise "split label consumer" unless SplitFactoryConsumer.label == "SPLIT"
raise "split size consumer" unless SplitFactoryConsumer.size == 33
"#)
            .assert_passes();
    }
}

/// `resources :x, only: [] do … end` nests routes under a parent with no
/// routes of its own. Ingest rejected the empty list and dropped the
/// parent with every route nested in it.
#[test]
fn nested_routes_under_an_only_empty_parent_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  resources :articles do\n    resources :comments, only: [:create, :destroy]\n  end\n",
            "  resources :articles\n  resources :articles, only: [] do\n    resources :comments, only: [:create, :destroy]\n  end\n",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// A collection render with `as: :for` emitted a
/// positional `for` param and a `|for|` block param.
#[test]
fn a_collection_render_with_a_reserved_word_as_local_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/views/articles/_row.html.erb",
            "<li class=\"for-row\"><%= binding.local_variable_get(:for).title %></li>\n",
        )
        .write(
            "app/views/articles/_assigns_row.html.erb",
            "<li class=\"for-assigns-row\"><%= local_assigns[:for].title %></li>\n",
        ),
        "<%= render partial: \"row\", collection: @articles, as: :for %>\n\
         <%= render partial: \"assigns_row\", collection: @articles, as: :for %>\n",
        "    assert_select \"li.for-row\", Article.count\n    \
             assert_select \"li.for-assigns-row\", Article.count\n",
    );
    run.assert_passes();
}

/// Not `module ApplicationController`, which cannot load beside the controller's own `class ApplicationController`: a class nested in a controller reopens the controller as a class.
#[test]
fn a_class_nested_in_a_controller_loads() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  class Failure < StandardError\n    attr_reader :status\n\n    def initialize(status)\n      @status = status\n      super(\"failed with #{status}\")\n    end\n  end\n\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  class Missing < StandardError\n  end\n\n",
        )
        .write(
            "test/models/article_nested_class_test.rb",
            r#"require "test_helper"

class ArticleNestedClassTest < ActiveSupport::TestCase
  test "a class nested in a controller is the one the source declared" do
    failure = ApplicationController::Failure.new(404)
    assert_equal 404, failure.status
    assert_equal "failed with 404", failure.message
    assert_kind_of StandardError, ArticlesController::Missing.new
    assert ArticlesController < ApplicationController
  end
end
"#,
        )
        .run_test("test/models/article_nested_class_test.rb")
        .assert_passes();
}

#[test]
fn safe_navigation_comparisons_execute_for_nil_and_string_values() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  def title_long?
    ((title && title.length) || 0) > 1
  end
  def title_short?
    (title&.length || 0) < 1
  end
"#)
        .run_ruby(r#"
article = Article.new(title: "long")
raise "truthy chain" unless article.title_long? && !article.title_short?
article.title = nil
raise "nil chain" unless !article.title_long? && article.title_short?
"#)
        .assert_passes();
}

#[test]
fn forwarded_proc_expressions_execute_once_in_an_emitted_app() {
    emit_and_run::real_blog()
        .write("app/services/block_forward_probe.rb", r#"class BlockForwardProbe
  def initialize
    @calls = 0
    @callback = ->(x) { x * 2 }
  end
  def compute(n)
    @calls += 1
    ->(x) { x + n }
  end
  def run
    doubled = [1, 2].map(&@callback)
    added = [1, 2].map(&compute(3))
    [doubled, added, @calls]
  end
end
"#)
        .run_ruby("raise 'forwarded expression' unless BlockForwardProbe.new.run == [[2, 4], [4, 5], 1]")
        .assert_passes();
}

#[test]
fn typed_instance_keywords_bind_values_and_keep_positional_hashes() {
    emit_and_run::real_blog()
        .write("app/services/keyword_fetcher.rb", r##"
class KeywordFetcher
  def fetch(url, ip: url.upcase)
    "#{url}@#{ip}"
  end

  def merge(url, opts = {})
    "#{url}#{opts}"
  end
end
"##)
        .write("app/services/keyword_locator.rb", r#"
class KeywordLocator
  def locate(url)
    KeywordFetcher.new.fetch(url, ip: "192.0.2.1")
  end

  def merged(url)
    KeywordFetcher.new.merge(url, opts: 1)
  end
end
"#)
        .run_ruby(r#"
locator = KeywordLocator.new
raise "keyword bound to hash" unless locator.locate("host") == "host@192.0.2.1"
raise "positional hash rewritten" unless locator.merged("host") == 'host{opts: 1}'
"#)
        .assert_passes();
}

#[test]
fn multiple_erb_openers_execute_inside_an_output_block() {
    on_the_index(emit_and_run::real_blog(), r#"<span class="multi-opener"><%= capture do %>
<% [1, 2].each do |number|
       unless number.nil? %><%= number %><% end %><% end %><% end %></span>"#,
        "    assert_select \"span.multi-opener\", \"12\"\n")
        .assert_passes();
}

#[test]
fn trailing_erb_comments_execute_without_swallowing_output_terminators() {
    on_the_index(emit_and_run::real_blog(), r#"<span class="commented-title"><%= capture do %>
<% [1, 2].each do |number| %><%= "n: #{number}" #@label %><% end #$numbers %><% end #{capture} %></span>"#,
        "    assert_select \"span.commented-title\", \"n: 1n: 2\"\n")
        .assert_passes();
}

/// Not the scaffold blog's `app/views.rb`, whose requires name views this tree does not have: an app with no views boots and answers a request (#164).
#[test]
fn an_app_with_no_views_boots() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"widgets#index\"\n  resources :widgets, only: :index\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "GET /widgets answered #{status}" unless status == 204
"#,
        )
        .assert_passes();
}

#[test]
fn bundled_uri_and_http_exception_constants_run() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/http_constant_probe.rb",
            r#"class HttpConstantProbe
  def self.http?(url)
    URI.parse(url).is_a?(URI::HTTP)
  end

  def self.invalid_uri
    begin
      URI.parse("https://bad host/")
    rescue URI::InvalidURIError
      "invalid"
    end
  end

  def self.construct
    URI::HTTP.new("http", nil, "example.test", 80, nil, "/", nil, nil, nil).to_s
  end

  def self.invalid_constructor
    begin
      URI::HTTP.new
    rescue ArgumentError
      "arity"
    end
  end

  def self.timeout(kind)
    begin
      if kind == "open"
        raise Net::OpenTimeout
      else
        raise Net::ReadTimeout
      end
    rescue Net::OpenTimeout
      "open"
    rescue Net::ReadTimeout
      "read"
    end
  end
end
"#,
        )
        .run_ruby(
            r#"raise unless HttpConstantProbe.http?("https://example.test/")
raise if HttpConstantProbe.http?("ftp://example.test/")
raise unless HttpConstantProbe.invalid_uri == "invalid"
raise unless HttpConstantProbe.construct == "http://example.test/"
raise unless HttpConstantProbe.invalid_constructor == "arity"
raise unless HttpConstantProbe.timeout("open") == "open"
raise unless HttpConstantProbe.timeout("read") == "read"
"#,
        );
    let probe = std::fs::read_to_string(run.emitted.join("app/models/http_constant_probe.rb")).unwrap();
    assert!(probe.lines().any(|line| line == "require \"uri\""), "{probe}");
    run.assert_passes();
}

#[test]
fn bundled_response_io_json_and_runtime_value_constants_run() {
    emit_and_run::real_blog()
        .write("app/services/bundled_value_probe.rb", r#"class BundledValueProbe
  def self.responses
    [Net::HTTPOK.new("1.1", "200", "OK").is_a?(Net::HTTPOK),
     Net::HTTPRedirection.new("1.1", "302", "Found").is_a?(Net::HTTPRedirection)]
  end
  def self.buffer
    io = StringIO.new
    io << "abc"
    io.string
  end
  def self.encode
    JSON.generate([17, "hello"])
  end
  def self.ssl_error
    begin
      raise OpenSSL::OpenSSLError
    rescue OpenSSL::OpenSSLError
      "ssl"
    end
  end
  def self.attributes
    ActionText::Attachment::ATTRIBUTES.include?("sgid")
  end
  def self.allowed_tags
    Rails::HTML5::SafeListSanitizer.allowed_tags.include?("a")
  end
end
"#)
        .run_ruby(r#"raise unless BundledValueProbe.responses == [true, true]
raise unless BundledValueProbe.buffer == "abc"
raise unless BundledValueProbe.encode == '[17,"hello"]'
raise unless BundledValueProbe.ssl_error == "ssl"
raise unless BundledValueProbe.attributes
raise unless BundledValueProbe.allowed_tags
"#)
        .assert_passes();
}

/// Not `user || raise NotFound` (a syntax error) or `a && self.x = v && b` (assigns `v && b`): a command or a method assignment as an `&&`/`||` operand keeps its parentheses.
#[test]
fn a_command_operand_of_a_boolean_operator_keeps_its_parentheses() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.find_or_fail(id)
    find_by(id: id) || (raise ActiveRecord::RecordNotFound, "no article #{id}")
  end

  def retitle(text, persist)
    text.present? && (self.title = text) && persist && save
  end"#,
        )
        .write(
            "test/models/article_guard_test.rb",
            r#"require "test_helper"

class ArticleGuardTest < ActiveSupport::TestCase
  test "a raise operand runs only when the left operand is nil" do
    article = articles(:one)
    assert_equal article.id, Article.find_or_fail(article.id).id
    assert_raises(ActiveRecord::RecordNotFound) { Article.find_or_fail(-1) }
  end

  test "a setter operand assigns its own argument" do
    article = articles(:one)
    assert_equal false, article.retitle("Retitled", false)
    assert_equal "Retitled", article.title
  end
end
"#,
        )
        .run_test("test/models/article_guard_test.rb")
        .assert_passes();
}

/// Not `out = ""`, a literal spinel freezes: `+""` stays an unfrozen copy that the method can append to.
#[test]
fn a_mutable_string_literal_stays_mutable() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def initials
    out = +""
    title.split.each { |word| out << word[0] }
    out
  end

  def hashtag
    tag = +"#"
    tag << title.downcase.delete(" ")
  end"##,
        )
        .write(
            "test/models/article_mutable_literal_test.rb",
            r##"require "test_helper"

class ArticleMutableLiteralTest < ActiveSupport::TestCase
  test "an unfrozen copy of a literal can be appended to" do
    article = Article.new(title: "Hello World")
    assert_equal "HW", article.initials
    assert_equal "#helloworld", article.hashtag
  end
end
"##,
        )
        .run_test_frozen("test/models/article_mutable_literal_test.rb")
        .assert_passes();
}

/// The key forms Rails' PostgreSQL schema dumper writes run. `id:
/// :serial` stopped ingest ("unsupported type `serial`"), and `id: {
/// type: :string, limit: 32 }` was read as the default key, so the
/// emitted table had an integer autoincrement key where the app keeps
/// string ones.
#[test]
fn postgres_dumped_key_forms_run() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", id: :serial, force: :cascade do |t|",
        )
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"codes\", id: { type: :string, limit: 32 }, force: :cascade do |t|\n    \
             t.string \"label\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/code.rb", "class Code < ApplicationRecord\nend\n")
        .write(
            "test/models/key_forms_test.rb",
            r#"require "test_helper"

class KeyFormsTest < ActiveSupport::TestCase
  test "a serial key is generated" do
    article = Article.create!(title: "Serial", body: "A long enough body")
    assert_kind_of Integer, article.id
    assert_equal "Serial", Article.find(article.id).title
  end

  test "a string key is the one the app supplies" do
    Code.create!(id: "launch-2026", label: "Launch")
    assert_equal "Launch", Code.find("launch-2026").label
  end
end
"#,
        )
        .run_test("test/models/key_forms_test.rb")
        .assert_passes();
}

/// A unique index with `where:` constrains only the rows its predicate
/// selects. Ingest dropped the predicate, so the emitted DDL made the
/// index unique over every row: an archived article's title stayed
/// taken, `create!` raised `RecordNotUnique`, and `check` said nothing.
/// `insert_all`'s guard follows the predicate too: it skips a row only
/// when a row the index covers shares its key (`title` is `null:
/// false`, so the guard reads the index).
#[test]
fn a_partial_unique_index_constrains_only_the_rows_it_selects() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\", null: false\n    \
             t.datetime \"archived_at\"\n    \
             t.index [\"title\"], name: \"index_articles_on_live_title\", unique: true, where: \"(archived_at IS NULL)\"\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.import_titles(first, second, body)
    Article.insert_all([{ title: first, body: body }, { title: second, body: body }])
  end"#,
        )
        .write(
            "test/models/article_live_title_test.rb",
            r#"require "test_helper"

class ArticleLiveTitleTest < ActiveSupport::TestCase
  test "an archived article's title can be reused" do
    old = Article.create!(title: "Reused", body: "A long enough body")
    old.update!(archived_at: Time.now)
    assert Article.create!(title: "Reused", body: "A long enough body").persisted?
  end

  test "two live articles still cannot share a title" do
    Article.create!(title: "Shared", body: "A long enough body")
    assert_raises(ActiveRecord::RecordNotUnique) do
      Article.create!(title: "Shared", body: "A long enough body")
    end
  end

  test "insert_all skips a live duplicate and adds a row only archived ones share" do
    Article.create!(title: "Taken", body: "A long enough body")
    Article.create!(title: "Imported", body: "A long enough body", archived_at: Time.now)
    Article.import_titles("Taken", "Imported", "A long enough body")
    assert_equal 1, Article.where(title: "Taken").count
    assert_equal 1, Article.where(title: "Imported", archived_at: nil).count
  end
end
"#,
        )
        .run_test("test/models/article_live_title_test.rb")
        .assert_passes();
}

/// `upsert_all(unique_by:)` names the index Rails' `InsertAll` picks:
/// the first unique index by name with those columns, in any order. A
/// partial one goes into the conflict target with its `WHERE`, without
/// which SQLite matches no partial index; a full one goes in bare.
#[test]
fn upsert_all_targets_the_unique_index_rails_picks() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            r#"  create_table "slugs", force: :cascade do |t|
    t.string "name", null: false
    t.integer "hits", default: 0, null: false
    t.datetime "retired_at"
    t.index ["name"], name: "index_slugs_on_live_name", unique: true, where: "(retired_at IS NULL)"
  end

  create_table "seats", force: :cascade do |t|
    t.bigint "room_id", null: false
    t.bigint "user_id", null: false
    t.integer "visits", default: 0, null: false
    t.datetime "left_at"
    t.index ["room_id", "user_id"], name: "index_seats_on_live_room_and_user", unique: true, where: "(left_at IS NULL)"
  end

  create_table "tags", force: :cascade do |t|
    t.string "name", null: false
    t.integer "uses", default: 0, null: false
    t.datetime "retired_at"
    t.index ["name"], name: "index_tags_on_name", unique: true
    t.index ["name"], name: "index_tags_on_name_live", unique: true, where: "(retired_at IS NULL)"
  end

  add_foreign_key "comments", "articles""#,
        )
        .write("app/models/slug.rb", "class Slug < ApplicationRecord\nend\n")
        .write("app/models/seat.rb", "class Seat < ApplicationRecord\nend\n")
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .write(
            "test/models/upsert_target_test.rb",
            r#"require "test_helper"

class UpsertTargetTest < ActiveSupport::TestCase
  test "a partial index: updates the live row, then inserts past a retired one" do
    Slug.upsert_all([{ name: "home", hits: 1 }], unique_by: :name)
    Slug.upsert_all([{ name: "home", hits: 2 }], unique_by: :name)
    assert_equal [2], Slug.where(name: "home").pluck(:hits)
    Slug.where(name: "home").update_all(retired_at: Time.now)
    Slug.upsert_all([{ name: "home", hits: 3 }], unique_by: :name)
    assert_equal [3], Slug.where(name: "home", retired_at: nil).pluck(:hits)
    assert_equal 2, Slug.where(name: "home").where.not(retired_at: nil).pluck(:hits).first
  end

  test "a composite partial index: unique_by in either order" do
    Seat.upsert_all([{ room_id: 1, user_id: 7, visits: 1 }], unique_by: [:room_id, :user_id])
    Seat.upsert_all([{ room_id: 1, user_id: 7, visits: 2 }], unique_by: [:user_id, :room_id])
    assert_equal [2], Seat.where(room_id: 1, user_id: 7).pluck(:visits)
  end

  test "a full index first by name: the bare conflict target" do
    Tag.upsert_all([{ name: "rails", uses: 1 }], unique_by: :name)
    Tag.where(name: "rails").update_all(retired_at: Time.now)
    Tag.upsert_all([{ name: "rails", uses: 2 }], unique_by: :name)
    assert_equal [2], Tag.where(name: "rails").pluck(:uses)
  end
end
"#,
        )
        .run_test("test/models/upsert_target_test.rb")
        .assert_passes();
}

/// The blog with its schema as a `db/structure.sql` from pg_dump 18 runs.
/// The dump opens with `\restrict <key>` and closes with `\unrestrict
/// <key>`, psql meta-commands pg_dump writes since 18 and the August 2025
/// minor releases; Rails before 7.2.3 and 8.0.3 keeps them. Ingest read
/// each as the head of an unmodeled statement and stopped. The dump is
/// the blog's `db/schema.rb` loaded into PostgreSQL 18.3 and dumped with
/// Rails' flags (`--schema-only --no-privileges --no-owner`), with the
/// `SET search_path` and `schema_migrations` lines Rails appends.
#[test]
fn a_structure_sql_from_pg_dump_18_runs() {
    emit_and_run::real_blog()
        .remove("db/schema.rb")
        .write("db/structure.sql", include_str!("support/real_blog_structure.sql"))
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// An app that declares no `root` boots and dispatches (#165). `main.rb`
/// composed its route table as `[RouteTable.root] + RouteTable.table`,
/// and the routes emit defines `RouteTable.root` only for a route at
/// `/`, so the first request raised NoMethodError. `/` itself answers
/// 404, as it does in Rails when no route matches.
#[test]
fn an_app_with_no_root_route_dispatches() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :set_widget, only: :show

  def index
    head :no_content
  end

  def show
    head :not_found unless @widget
  end

  private

  def set_widget
    @widget = Widget.find_by(id: params[:id])
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[index show]\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"def get(path)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
{ "/widgets" => 204, "/widgets/1" => 404, "/" => 404 }.each do |path, want|
  got = get(path)
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

#[path = "emit_and_run/concern_accessors.rs"]
mod concern_accessors;

/// A concern split in two, mixed into more than one controller: the
/// inner module calls a method only its includers have (through the
/// outer one). With several includers `self` in the inner module is the
/// module, the bare call fell to `untyped`, and so did everything read
/// from it — the Rails authentication generator's `Current.user` shape.
#[test]
fn a_concern_calls_a_method_every_includer_gets_from_a_sibling() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/featuring.rb",
            r#"module Featuring
  extend ActiveSupport::Concern

  def featured_title
    featured_article&.title
  end
end
"#,
        )
        .write(
            "app/controllers/concerns/browsing.rb",
            r#"module Browsing
  extend ActiveSupport::Concern

  include Featuring

  private

  def featured_article
    Article.order(:id).first
  end
end
"#,
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  include Browsing\n",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "class CommentsController < ApplicationController\n",
            "class CommentsController < ApplicationController\n  include Browsing\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @featured = featured_title.to_s.upcase\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A routed action with a template and no method behind it: Rails runs
/// `show` whether or not `def show` exists, so `before_action
/// :set_article, only: %i[show …]` still feeds `articles/show`. `check`
/// reported every `@article` in that template as having no known type.
#[test]
fn a_template_only_action_is_fed_by_its_before_action() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/1 or /articles/1.json\n  def show\n  end\n\n",
            "",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `case/in` structural pattern matching (#f9): taking `CaseMatchNode`
/// from an ingest error to a typed `CaseMatch` node is a claim the
/// emitted program actually dispatches through it (invariant 6), not
/// just that `check` stops reporting `unsupported expression node:
/// CaseMatchNode`. A PORO under `app/lib` (same placement as the
/// `Deprecation` overlay above) exercises a `Capture` pattern
/// (`Integer => n`) and a plain-class `Value` pattern (`String`) — the
/// two shapes real-blog's own model/controller code never uses, so
/// this is the only thing that runs them through CRuby at all.
#[test]
fn case_in_pattern_matching_runs() {
    emit_and_run::real_blog()
        .write(
            "app/lib/pattern_matcher.rb",
            "class PatternMatcher\n  \
               def self.classify(x)\n    \
                 case x\n    \
                 in Integer => n\n      \
                   n * 2\n    \
                 in String\n      \
                   0\n    \
                 end\n  \
               end\nend\n",
        )
        .write(
            "test/models/pattern_matcher_test.rb",
            "require \"test_helper\"\n\n\
             class PatternMatcherTest < ActiveSupport::TestCase\n  \
               test \"case/in dispatches by pattern and captures a binding\" do\n    \
                 assert_equal 10, PatternMatcher.classify(5)\n    \
                 assert_equal 0, PatternMatcher.classify(\"hi\")\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/pattern_matcher_test.rb")
        .assert_passes();
}

/// `Pathname#join` takes any number of parts, and an app writes
/// `Rails.root.join("source", "posts")` as often as the one-part form.
/// `check` is clean on the call, so the emitted `Rails::AppPath#join`
/// must accept every part, or none, and join them like Pathname does.
#[test]
fn rails_root_join_takes_any_number_of_parts() {
    let run = rails_root_join::overlay().run_ruby(rails_root_join::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root.join contract passed"));
}

/// An Active Job argument serializer extends a Rails base that the
/// runtime does not port. The emit drops the class with a
/// `lower_residue` warning, so the tree still loads. Before, the class
/// was kept, and `app/models.rb` raised `uninitialized constant
/// ActiveJob::Serializers` at boot.
#[test]
fn an_active_job_object_serializer_does_not_stop_the_boot() {
    emit_and_run::real_blog()
        .write(
            "app/serializers/article_serializer.rb",
            r#"class ArticleSerializer < ActiveJob::Serializers::ObjectSerializer
  def klass
    Article
  end

  def serialize(article)
    super("id" => article.id)
  end

  def deserialize(hash)
    Article.find(hash["id"])
  end
end
"#,
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `include ActiveSupport::NumberHelper` in a helper gives it the same
/// number helpers as `ActionView::Helpers::NumberHelper`. No target
/// ships that namespace, so the include must not reach the emitted
/// module. Before, `application_helper.rb` raised `uninitialized
/// constant ActiveSupport::NumberHelper` at boot.
#[test]
fn an_active_support_number_helper_include_does_not_stop_the_boot() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/application_helper.rb",
            r#"module ApplicationHelper
  include ActiveSupport::NumberHelper

  def article_total(count)
    number_with_delimiter(count)
  end
end
"#,
        )
        .run_ruby(
            r#"raise "delimiter" unless ApplicationHelper.article_total(1234567) == "1,234,567"
puts "ok"
"#,
        )
        .assert_passes();
}

/// `javascript_include_tag :application` names the source with a
/// Symbol, as Rails allows. The call is hoisted to a constant, so it
/// runs at load. Before, the runtime called `include?` on the Symbol,
/// and the layout raised `NoMethodError` at boot.
#[test]
fn a_symbol_source_for_javascript_include_tag_renders_a_script_tag() {
    emit_and_run::real_blog()
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= javascript_importmap_tags %>\n",
            "    <%= javascript_importmap_tags %>\n    <%= javascript_include_tag :application %>\n",
        )
        .write(
            "app/views/articles/_scripts.html.erb",
            "<%= javascript_include_tag :admin, defer: true %>",
        )
        .run_ruby(
            r#"html = Views::Articles.scripts(nil)
raise "script tag: #{html}" unless html == %(<script src="/assets/admin.js" defer="defer"></script>)
puts "ok"
"#,
        )
        .assert_passes();
}

/// A Symbol source that a value holds, not a literal, reaches the
/// runtime as a Symbol. Before, the runtime called `include?` on it and
/// raised `NoMethodError`.
#[test]
fn a_symbol_source_in_a_value_for_javascript_include_tag_renders_a_script_tag() {
    emit_and_run::real_blog()
        .write(
            "app/views/articles/_scripts.html.erb",
            "<% source = :admin %><%= javascript_include_tag source %>",
        )
        .run_ruby(
            r#"html = Views::Articles.scripts(nil)
raise "script tag: #{html}" unless html == %(<script src="/assets/admin.js"></script>)
puts "ok"
"#,
        )
        .assert_passes();
}
