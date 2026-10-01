//! Constructs that `check` accepts must run once emitted.
//!
//! See `tests/support/emit_and_run.rs` for the harness and why it
//! exists. The ignored tests below are known places where the two
//! disagree: `check` is clean and the emitted program fails. Each is a
//! complete statement of the fix: make it pass and drop the `#[ignore]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The harness itself: the unedited blog emits and its controller
/// suite, which renders every page, passes.
#[test]
fn the_unedited_blog_runs() {
    emit_and_run::real_blog()
        .run_test("test/controllers/articles_controller_test.rb")
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

/// A custom String primary key must not take the default Integer cast.
#[test]
fn relation_find_with_string_keys_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"bookmarks\", id: :string, primary_key: \"code\", force: :cascade do |t|\n    t.string \"title\"\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
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
puts "ok"
"#,
        )
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
