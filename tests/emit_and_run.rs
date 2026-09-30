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
