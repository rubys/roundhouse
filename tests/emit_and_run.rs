//! Constructs that `check` accepts must run once emitted.
//!
//! See `tests/support/emit_and_run.rs` for the harness and why it
//! exists. The ignored tests below are known places where the two
//! disagree: `check` is clean and the emitted program fails. Each is a
//! complete statement of the fix: make it pass and drop the `#[ignore]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/class_attribute.rs"]
mod class_attribute;
#[path = "emit_and_run/integer_query_find_by.rs"]
mod integer_query_find_by;
#[path = "support/lambda_signatures.rs"]
mod lambda_signatures_contract;
#[path = "emit_and_run/lambda_signatures.rs"]
mod lambda_signatures;
#[path = "emit_and_run/strong_params.rs"]
mod strong_params;
#[path = "emit_and_run/params_wrapper.rs"]
mod params_wrapper;

#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/runtime_block_signature.rs"]
mod runtime_block_signature;
#[path = "support/data_factory.rs"]
mod data_factory;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;
#[path = "support/cable_broadcast_json.rs"]
mod cable_broadcast_json_contract;
#[path = "emit_and_run/cable_broadcast_json.rs"]
mod cable_broadcast_json;
#[path = "support/anonymous_keywords.rs"]
mod anonymous_keywords;
#[path = "support/delegate_association.rs"]
mod delegate_association;
#[path = "support/io_process_constants.rs"]
mod io_process_constants_contract;
#[path = "emit_and_run/io_process_constants.rs"]
mod io_process_constants;
#[path = "emit_and_run/helper_anonymous_rest.rs"]
mod helper_anonymous_rest;
#[path = "emit_and_run/controller_response_body.rs"]
mod controller_response_body;
#[path = "emit_and_run/render_to_string_partial_ivar.rs"]
mod render_to_string_partial_ivar;
#[path = "emit_and_run/request_optional_port.rs"]
mod request_optional_port;
#[path = "emit_and_run/controller_url_helpers.rs"]
mod controller_url_helpers;
#[path = "emit_and_run/controller_super_ivars.rs"]
mod controller_super_ivars;
#[path = "emit_and_run/assoc_pluck_typed.rs"]
mod assoc_pluck_typed;
#[path = "emit_and_run/action_controller_head.rs"]
mod action_controller_head;
#[path = "emit_and_run/sti_global_id.rs"]
mod sti_global_id;
#[path = "support/campfire_caches.rs"]
mod campfire_caches_contract;
#[path = "emit_and_run/campfire_caches.rs"]
mod campfire_caches;
#[path = "emit_and_run/action_text_markdown.rs"]
mod action_text_markdown;
#[path = "emit_and_run/ordinalize.rs"]
mod ordinalize;

/// A generated text column on the real-blog Article model exercises the
/// schema-to-runtime path together with Rails-style symbol callbacks. The
/// create callbacks must see the value returned by the database, while an
/// update remains stale until the model is explicitly reloaded.
#[test]
fn generated_article_text_runs_through_callbacks_and_reload() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n",
            "    t.string \"title\"\n    t.text \"body\"\n    t.virtual \"display_text\", type: :string, as: \"coalesce(title, '') || ' / ' || coalesce(body, '')\", stored: true\n    t.datetime \"created_at\", null: false\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\nend\n",
            concat!(
                "  validates :body, presence: true, length: { minimum: 10 }\n\n",
                "  after_create :capture_generated_create\n",
                "  after_update :capture_generated_update\n",
                "  after_save :capture_generated_save\n\n",
                "  def capture_generated_create\n    @generated_after_create = display_text\n  end\n\n",
                "  def capture_generated_update\n    @generated_after_update = display_text\n  end\n\n",
                "  def capture_generated_save\n    @generated_after_save = display_text\n  end\n\n",
                "  def generated_after_create\n    @generated_after_create\n  end\n\n",
                "  def generated_after_update\n    @generated_after_update\n  end\n\n",
                "  def generated_after_save\n    @generated_after_save\n  end\n",
                "end\n",
            ),
        )
        .write(
            "test/models/generated_column_article_test.rb",
            r#"require "test_helper"

class GeneratedColumnArticleTest < ActiveSupport::TestCase
  test "generated text hydrates before create callbacks and reload refreshes updates" do
    article = Article.create!(title: "Generated title", body: "A sufficiently long article body.")
    original = "Generated title / A sufficiently long article body."

    assert_equal original, article.display_text
    assert_equal original, article.generated_after_create
    assert_equal original, article.generated_after_save

    article.update!(title: "Updated title")
    assert_equal original, article.display_text
    assert_equal original, article.generated_after_update
    assert_equal original, article.generated_after_save
    assert_equal "Updated title / A sufficiently long article body.", article.reload.display_text
  end
end
"#,
        )
        .run_test("test/models/generated_column_article_test.rb")
        .assert_passes();
}

/// The same anonymous keyword packet survives defaulting, local-name
/// collisions, and a virtual override in emitted CRuby. Effectful input
/// values also stay left-to-right and run once.
#[test]
fn anonymous_keyword_forwarding_runs_without_capturing_or_reordering_values() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/keyword_forwarder.rb",
            anonymous_keywords::SOURCE,
        )
        .run_ruby(anonymous_keywords::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("anonymous keyword forwarding contract passed"));
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/keyword_forwarder.rb"))
        .expect("emitted keyword forwarding class");
    assert!(emitted.contains("class KeywordForwarder"), "{emitted}");
    assert!(emitted.contains("request(kind: :get, path: path, **)"), "{emitted}");
}

#[test]
fn model_concern_delegate_through_belongs_to_runs() {
    let run = delegate_association::overlay().run_ruby(delegate_association::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("model concern delegate passed"));
}

#[test]
fn active_support_many_block_and_squish_bang_run_in_emitted_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/active_support_probe.rb",
            r#"class ActiveSupportProbe
  def self.many_matches
    seen = []
    zero = [1, 2, 3].many? { |n| n > 9 }
    one = [1, 2, 3].many? { |n| n == 2 }
    multiple = [1, 2, 3, 4].many? { |n| seen << n; n > 1 }
    [zero, one, multiple, seen.length]
  end

  def self.squish_bang
    text = "\u00a0foo\u2003bar\u2028".dup
    text.squish!
  end
end
"#,
        )
        .run_ruby(
            r#"raise "many? block zero/one/multiple or short circuit failed" unless ActiveSupportProbe.many_matches == [false, false, true, 3]
raise "squish! did not match Unicode whitespace semantics" unless ActiveSupportProbe.squish_bang == "foo bar"
puts "ActiveSupport core extensions emitted Ruby passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("ActiveSupport core extensions emitted Ruby passed"));
}

#[test]
fn later_delegate_replaces_an_earlier_handwritten_method() {
    let run = delegate_association::overlay()
        .write(
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :article

  def article_body
    "handwritten"
  end

  delegate :body, to: :article, prefix: true
end
"#,
        )
        .run_ruby(
            r#"article = Article.create!(title: "Association title", body: "article body")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
raise "later delegate did not replace the earlier method" unless comment.article_body == "article body"
puts "later delegate ordering passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("later delegate ordering passed"));
}

/// `to_sql` renders a relation as SQL another query can embed: the
/// subquery runs, and selects exactly the commented article.
#[test]
fn a_relations_to_sql_runs_as_a_subquery() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.commented
    where(\"articles.id IN (#{Comment.select(:article_id).to_sql})\")
  end
",
        )
        .run_ruby(
            r#"commented = Article.create!(title: "Commented", body: "A sufficiently long body.")
Article.create!(title: "Quiet", body: "A sufficiently long body.")
Comment.create!(article: commented, commenter: "Reader", body: "Comment body")
sql = Article.where(title: "Quiet").to_sql
raise "to_sql: #{sql}" unless sql.start_with?("SELECT") && sql.include?("articles")
ids = Article.commented.map(&:id)
raise "subquery: #{ids.inspect}" unless ids == [commented.id]
puts "to_sql subquery passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("to_sql subquery passed"));
}

/// A has_many reader followed by `to_sql` is rooted as the association's
/// relation: the reader alone answers an Array, which has no `to_sql`,
/// and `check` was clean while the emitted method raised NoMethodError.
#[test]
fn a_has_many_readers_to_sql_is_the_scoped_query() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def comment_sql
    comments.to_sql
  end
",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
sql = a.comment_sql
raise "comment_sql: #{sql}" unless sql.include?("comments") && sql.include?("article_id = #{a.id}")
puts "has_many to_sql passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("has_many to_sql passed"));
}

/// `maximum` / `minimum` on a model class and on a has_many reader (of a
/// possibly-nil owner too) run as SQL extrema of the column's type, nil
/// over no rows: the reader alone answers an Array, which has neither.
fn extrema_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.newest_id
    Article.maximum(:id)
  end

  def last_commenter
    comments.maximum(:commenter)
  end

  def first_comment_id
    self.comments.minimum(:id)
  end

  def self.first_articles_last_comment_id
    Article.first.comments.maximum(:id)
  end
",
    )
}

const EXTREMA_ASSERTIONS: &str = r#"Article.delete_all
raise "empty newest_id: #{Article.newest_id.inspect}" unless Article.newest_id.nil?
a = Article.create!(title: "One", body: "A sufficiently long body.")
b = Article.create!(title: "Two", body: "A sufficiently long body.")
raise "newest_id: #{Article.newest_id.inspect}" unless Article.newest_id == b.id
raise "empty last_commenter: #{a.last_commenter.inspect}" unless a.last_commenter.nil?
c1 = Comment.create!(article_id: a.id, commenter: "Ann", body: "first comment")
Comment.create!(article_id: a.id, commenter: "Zed", body: "second comment")
Comment.create!(article_id: b.id, commenter: "Zoe", body: "other article")
raise "last_commenter: #{a.last_commenter.inspect}" unless a.last_commenter == "Zed"
raise "first_comment_id: #{a.first_comment_id.inspect}" unless a.first_comment_id == c1.id
raise "first_articles_last_comment_id: #{Article.first_articles_last_comment_id.inspect}" unless Article.first_articles_last_comment_id == Comment.where(article_id: a.id).last.id
puts "extrema passed"
"#;

#[test]
fn extrema_run_on_a_model_and_its_has_many_reader() {
    let run = extrema_app().run_ruby(EXTREMA_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("extrema passed"));
}

/// With a date column the Date package loads and an aggregate over it
/// answers a Date, on Spinel as on CRuby.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn date_extrema_answer_a_date_on_spinel() {
    let overlay = emit_and_run::real_blog()
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.date \"archived_on\"")
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  belongs_to :article\n\n  def self.first_archived_on\n    Comment.minimum(:archived_on)\n  end\n",
        );
    let script = r#"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
raise "empty: #{Comment.first_archived_on.inspect}" unless Comment.first_archived_on.nil?
a = Article.create!(title: "One", body: "A sufficiently long body.")
Comment.create!(article_id: a.id, commenter: "Ann", body: "later", archived_on: Date.iso8601("2024-03-04"))
Comment.create!(article_id: a.id, commenter: "Bob", body: "earlier", archived_on: Date.iso8601("2024-03-02"))
raise "first_archived_on: #{Comment.first_archived_on.iso8601}" unless Comment.first_archived_on.iso8601 == "2024-03-02"
puts "date extrema passed"
"#;
    overlay.run_spinel(script).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn extrema_run_on_spinel_without_date_columns() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{EXTREMA_ASSERTIONS}"
    );
    extrema_app().run_spinel(&script).assert_passes();
}

/// The has_many readers `lower::scope_chain` does not seed — one off a
/// call with arguments, or off a local holding the reader — stay
/// unresolved: their Array has no `maximum`.
#[test]
fn extrema_on_an_unseeded_reader_stay_errors() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.by_title_last_comment_id(title)
    Article.find_by!(title: title).comments.maximum(:id)
  end

  def held_last_comment_id
    held = comments
    held.maximum(:id)
  end
",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert_eq!(
        errors.iter().filter(|e| e.contains("`maximum`")).count(),
        2,
        "both unseeded readers keep their error: {errors:?}"
    );
}

/// `in_batches` with a block hands each batch as a relation; without
/// one, `update_all` and `touch_all` reach every row.
#[test]
fn in_batches_runs_with_and_without_a_block() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  scope :batched, -> { in_batches }

  def self.batch_total
    total = 0
    in_batches(of: 1) { |batch| total += batch.count }
    total
  end

  def self.rename_all(title)
    in_batches.update_all(title: title)
  end

  def self.touch_everything
    in_batches(order: :desc).touch_all
  end

  def touch_comments
    comments.in_batches.touch_all
  end
",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
Article.create!(title: "Two", body: "A sufficiently long body.")
Comment.create!(article: a, commenter: "Reader", body: "Comment body")
raise "batch_total: #{Article.batch_total}" unless Article.batch_total == 2
Article.rename_all("Renamed")
raise "rename_all" unless Article.all.map(&:title).uniq == ["Renamed"]
raise "touch_everything" unless Article.touch_everything == 2
raise "touch_comments" unless a.touch_comments == 1
raise "batched scope" unless Article.batched.where(title: "Renamed").count == 2
puts "in_batches passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("in_batches passed"));
}

/// `Rails.public_path` runs as `Rails.root` does: the runtime's AppPath
/// answers `join` and `to_s`.
#[test]
fn rails_public_path_joins() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.logo_path
    Rails.public_path.join(\"images\", \"logo.png\").to_s
  end
",
        )
        .run_ruby(
            r#"raise "logo_path: #{Article.logo_path}" unless Article.logo_path == "public/images/logo.png"
puts "public_path passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("public_path passed"));
}

/// A nested `transaction(requires_new: true)` is a savepoint: a Rollback
/// or a rescued exception from it undoes only its own writes, and the
/// outer transaction commits the rest; without an error it answers the
/// block's value.
#[test]
fn requires_new_transactions_roll_back_to_their_savepoint() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.add(title)
    Article.create!(title: title, body: \"A sufficiently long body.\")
  end

  def self.savepoints
    Article.transaction do
      add(\"outer\")
      Article.transaction(requires_new: true) do
        add(\"rolled-back\")
        raise ActiveRecord::Rollback
      end
      begin
        Article.transaction(requires_new: true) do
          add(\"raised\")
          raise ArgumentError, \"inner\"
        end
      rescue ArgumentError
        nil
      end
      Article.transaction(requires_new: true) do
        add(\"kept\")
        \"kept\"
      end
    end
  end
",
        )
        .run_ruby(
            r#"Article.delete_all
raise "savepoints: #{Article.savepoints.inspect}" unless Article.savepoints == "kept"
titles = Article.all.map(&:title).sort
raise "titles: #{titles.inspect}" unless titles == ["kept", "outer"]
puts "savepoints passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("savepoints passed"));
}

/// `rescue URI::Error` catches `URI.parse`'s `URI::InvalidURIError`, and
/// `SocketError` resolves where the app names it without a require (Rails
/// has loaded socket); both lanes define the classes.
fn uri_and_socket_errors_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.parsed?(url)
    URI.parse(url)
    true
  rescue URI::Error
    false
  end

  def self.socket_failure
    raise SocketError, \"unreachable\"
  rescue SocketError => e
    e.message
  end
",
    ).edit(
        "app/models/comment.rb",
        "  validates :body, presence: true\n",
        "  validates :body, presence: true

  def self.contained?
    yield
    false
  rescue ArgumentError,SocketError
    true
  end
",
    )
}

const URI_AND_SOCKET_ASSERTIONS: &str = r#"raise "parsed good" unless Article.parsed?("https://example.com/a")
raise "parsed bad" if Article.parsed?("http://bad uri")
raise "socket: #{Article.socket_failure}" unless Article.socket_failure == "unreachable"
raise "contained" unless Comment.contained? { raise ArgumentError, "x" }
raise "uncontained" if Comment.contained? { 1 }
puts "uri and socket errors passed"
"#;

#[test]
fn uri_and_socket_errors_are_rescued() {
    let run = uri_and_socket_errors_app().run_ruby(URI_AND_SOCKET_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("uri and socket errors passed"));
}

/// The emitted model requires what it rescues, rather than relying on
/// another runtime file to have loaded socket first.
#[test]
fn rescued_socket_and_uri_errors_bring_their_requires() {
    let (emitted, errors) = uri_and_socket_errors_app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let model = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("emitted model");
    for line in ["require \"socket\"", "require \"uri\""] {
        assert!(model.lines().any(|l| l.trim() == line), "missing {line}:\n{model}");
    }
    let comment = std::fs::read_to_string(emitted.join("app/models/comment.rb")).expect("emitted model");
    assert!(comment.lines().any(|l| l.trim() == "require \"socket\""), "rescue A,SocketError:\n{comment}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn uri_and_socket_errors_are_rescued_on_spinel() {
    uri_and_socket_errors_app().run_spinel(URI_AND_SOCKET_ASSERTIONS).assert_passes();
}

/// A Sidekiq worker's class-side entries run its `perform` inline, as an
/// ActiveJob's `perform_later` does: `include Sidekiq::Job` (or `Worker`)
/// and `sidekiq_options` leave the emitted class, `perform_in` /
/// `perform_at` drop their delay, and a subclass of a worker is one too.
#[test]
fn sidekiq_workers_perform_inline() {
    let run = emit_and_run::real_blog()
        .write(
            "app/workers/title_worker.rb",
            "class TitleWorker
  include Sidekiq::Job
  sidekiq_options queue: :low_priority, retry: 3

  def perform(title)
    Article.create!(title: title, body: \"A sufficiently long body.\")
  end
end
",
        )
        .write(
            "app/workers/loud_title_worker.rb",
            "class LoudTitleWorker < TitleWorker
  def perform(title)
    Article.create!(title: title.upcase, body: \"A sufficiently long body.\")
  end
end
",
        )
        .write(
            "app/workers/legacy_title_worker.rb",
            "class LegacyTitleWorker
  include Sidekiq::Worker

  def perform(title)
    Article.create!(title: title + \"?\", body: \"A sufficiently long body.\")
  end
end
",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.enqueue_all
    TitleWorker.perform_async(\"now\")
    TitleWorker.perform_in(5.minutes, \"later\")
    LoudTitleWorker.perform_async(\"loud\")
    LegacyTitleWorker.perform_at(1.hour.from_now, \"legacy\")
  end
",
        )
        .run_ruby(
            r#"Article.enqueue_all
titles = Article.all.map(&:title).sort
raise "titles: #{titles.inspect}" unless titles == ["LOUD", "later", "legacy?", "now"]
puts "sidekiq passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("sidekiq passed"));
}

/// A class object and its instances that define the same names: each
/// side's call types and runs as that side's method.
#[test]
fn same_named_class_and_instance_methods_run_on_their_own_side() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/greeter.rb",
            "class Greeter
  def self.call(name)
    new(name).call
  end

  def initialize(name)
    @name = name
  end

  def call
    \"hello \" + @name
  end
end
",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.headline
    [:class]
  end

  def headline
    title.to_s
  end

  def shout
    headline.upcase
  end
",
        )
        .run_ruby(
            "raise \"class call\" unless Greeter.call(\"a\") == \"hello a\"
raise \"instance call\" unless Greeter.new(\"b\").call == \"hello b\"
raise \"class headline\" unless Article.headline == [:class]
raise \"instance headline\" unless Article.new(title: \"hi\").shout == \"HI\"
puts \"same-named sides passed\"
",
        );
    run.assert_passes();
    assert!(run.stdout.contains("same-named sides passed"));
}

#[path = "support/engine_mount.rs"]
mod engine_mount;

#[test]
fn literal_isolated_engine_mount_runs_over_cruby_http() {
    let run = engine_mount::overlay().run_ruby(engine_mount::CRUBY_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("literal engine mount CRuby HTTP contract passed"));
}

#[test]
fn engine_helper_calls_are_explicit_instead_of_using_host_helpers() {
    let controller = r#"module Catalog
  class ProductsController < ActionController::Base
    def index
      render plain: products_path
    end
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/products_controller.rb", controller)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "vendor/catalog/app/controllers/catalog/products_controller.rb", "products_path"),
        "engine helper collision was not explicit:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn reflective_engine_helpers_and_proxy_calls_are_explicit() {
    let engine_controller = r#"module Catalog
  class ProductsController < ActionController::Base
    def reflective_symbol
      send(:products_path)
    end

    def reflective_string
      public_send("items_path")
    end
  end
end
"#;
    let host_controller = r#"class HostController < ApplicationController
  def host_products_reflection
    public_send(:products_path)
  end

  def reflective_mount_helper
    self.send(:catalog_path)
  end

  def reflective_symbol_proxy
    catalog.send(:products_path)
  end

  def reflective_string_proxy
    self.catalog.public_send("items_path")
  end

  def reflective_private_proxy
    self.catalog.__send__("products_path")
  end

  def reflective_proxy_then_direct_helper
    public_send(:catalog).items_path
  end

  def reflective_proxy_and_helper_chain
    self.public_send("catalog").public_send(:items_path)
  end

  def reflective_send_proxy_and_helper
    send(:catalog).send("products_path")
  end
end
"#;
    let engine_routes = "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\n  get \"/items\", to: \"products#index\", as: :items\nend\n";
    let host_routes = "Rails.application.routes.draw do\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/host/products\", to: \"host#reflective_symbol_proxy\", as: :products\nend\n";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/products_controller.rb", engine_controller)
        .write("vendor/catalog/config/routes.rb", engine_routes)
        .write("app/controllers/host_controller.rb", host_controller)
        .write("config/routes.rb", host_routes)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    for (path, needle) in [
        ("vendor/catalog/app/controllers/catalog/products_controller.rb", "send(:products_path)"),
        ("vendor/catalog/app/controllers/catalog/products_controller.rb", "public_send(\"items_path\")"),
        ("app/controllers/host_controller.rb", "catalog.send(:products_path)"),
        ("app/controllers/host_controller.rb", "self.catalog.public_send(\"items_path\")"),
        ("app/controllers/host_controller.rb", "self.catalog.__send__(\"products_path\")"),
        ("app/controllers/host_controller.rb", "public_send(:catalog).items_path"),
        ("app/controllers/host_controller.rb", "self.public_send(\"catalog\").public_send(:items_path)"),
        ("app/controllers/host_controller.rb", "send(:catalog).send(\"products_path\")"),
        ("app/controllers/host_controller.rb", "self.send(:catalog_path)"),
    ] {
        assert!(
            engine_mount::has_error_on_source_line(&app, &errors, path, needle),
            "reflective helper use {needle} in {path} was not a located mount error:\n{}",
            engine_mount::describe_errors(&app, &errors)
        );
    }
    assert!(
        !engine_mount::has_error_on_source_line(
            &app,
            &errors,
            "app/controllers/host_controller.rb",
            "public_send(:products_path)"
        ),
        "host-authored products_path reflection was rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn host_engine_helper_proxy_calls_are_explicit() {
    let host = r#"class HostController < ApplicationController
  def index
    render plain: "host root"
  end

  def root_helper
    render plain: root_path
  end

  def products_helper
    render plain: products_path
  end

  def engine_proxy
    render plain: catalog.products_path
  end

  def self_engine_proxy
    render plain: self.catalog.items_path
  end

  def after
    render plain: "host after"
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("app/controllers/host_controller.rb", host)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"host#index\"\n  get \"/root-helper\", to: \"host#root_helper\"\n  get \"/products-helper\", to: \"host#products_helper\"\n  get \"/engine-proxy\", to: \"host#engine_proxy\"\n  get \"/self-engine-proxy\", to: \"host#self_engine_proxy\"\n  get \"/products\", to: \"products#index\", as: :products\n  get \"/catalog/products\", to: \"products#index\", as: :host_catalog_products\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/catalog/items\", to: \"items#index\", as: :host_catalog_items\n  get \"/after\", to: \"host#after\"\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "app/controllers/host_controller.rb", "products_path"),
        "mounted engine proxy calls were not explicit:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    assert!(
        engine_mount::has_error_on_source_line(&app, &errors, "app/controllers/host_controller.rb", "self.catalog.items_path"),
        "self.catalog.items_path receiver was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn host_root_helper_is_kept_while_engine_root_helper_is_an_error() {
    let home = r#"module Catalog
  class HomeController < ActionController::Base
    def index
      render plain: root_path
    end
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/home_controller.rb", home)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "vendor/catalog/app/controllers/catalog/home_controller.rb", "root_path"),
        "engine root helper did not retain an explicit error:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    assert!(
        !engine_mount::has_error_in_source(&app, &errors, "app/controllers/host_controller.rb", "root_path"),
        "the legitimate host root_path call was rejected: {errors:?}"
    );
}

#[test]
fn isolated_engine_paths_do_not_create_or_shadow_host_helpers() {
    let host = r#"class HostController < ApplicationController
  def index
    render plain: "host root"
  end

  def fake_engine_helper
    render plain: "host action"
  end
end
"#;
    let (emitted, app, errors) = engine_mount::overlay()
        .write("app/controllers/host_controller.rb", host)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"host#index\"\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/host/catalog/products\", to: \"host#fake_engine_helper\", as: :catalog_products\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.is_empty(),
        "literal engine mount reported unexpected errors:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    let helpers = std::fs::read_to_string(emitted.join("app/route_helpers.rb")).expect("route helpers");
    assert_eq!(
        helpers.matches("def self.catalog_products_path").count(),
        1,
        "the host-authored route helper must remain available exactly once: {helpers}"
    );
    assert!(
        helpers.contains("\"/host/catalog/products\""),
        "catalog_products_path must retain the host route's path: {helpers}"
    );
    assert!(
        !helpers.contains("def self.catalog_items_path"),
        "the isolated engine's catalog_items_path leaked into host helpers: {helpers}"
    );
}

#[test]
fn helper_proxy_uses_in_library_jbuilder_and_configured_test_sources_are_explicit() {
    let sources = [
        (
            "lib/engine_proxy_reference.rb",
            "class EngineProxyReference\n  def self.path\n    catalog.products_path\n  end\nend\n",
        ),
        (
            "app/views/host/index.json.jbuilder",
            "json.url catalog.products_path\n",
        ),
        (
            "spec/engine_proxy_reference_spec.rb",
            "class EngineProxyReferenceSpec\n  def path\n    catalog.products_path\n  end\nend\n",
        ),
        (
            "vendor/catalog/lib/catalog/path_reference.rb",
            r#"module Catalog
  class PathReference
    def path
      products_path
    end

    def self.engine_home
      products_path
    end

    class Engine
      def self.reference
        "plain nested helper class"
      end
    end
  end
end
"#,
        ),
    ];
    for (path, source) in sources {
        let mut overlay = engine_mount::overlay().write(path, source);
        if path.starts_with("spec/") {
            overlay = overlay.write("roundhouse.yml", "test_paths:\n  - spec\n");
        }
        let (_emitted, app, errors) = overlay.emit_with_app(roundhouse::project::BuildTarget::Ruby);
        assert!(
            engine_mount::has_error_in_source(&app, &errors, path, "products_path"),
            "route helper call in {path} was not an explicit mount gap:\n{}",
            engine_mount::describe_errors(&app, &errors)
        );
    }
}

#[test]
fn engine_haml_helper_calls_keep_their_source_boundary() {
    let path = "vendor/catalog/app/views/home/index.html.haml";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write(path, "= root_path\n")
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, path, "root_path"),
        "bare engine helper in HAML was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn dynamic_engine_route_targets_keep_their_source_boundary() {
    let path = "vendor/catalog/config/routes.rb";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write(
            path,
            "Catalog::Engine.routes.draw do\n  root to: redirect(root_path)\n  get \"/products\", to: \"products#index\"\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, path, "dynamic targets"),
        "dynamic engine route target was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[path = "emit_and_run/integer_query_exists.rs"]
mod integer_query_exists;

#[test]
fn critic_corrections_preserve_class_objects_reflection_and_operators() {
    emit_and_run::real_blog()
        .write("app/helpers/protocol_control.rb", r#"class FirstOperand
  def +(other)
    other + 41
  end
end
class SecondOperand
  def +(other)
    other + 42
  end
end
class ProtocolControl
  def initialize
    @value = 7
  end
  def self.implicit_eval
    class_eval { 31 }
  end
  def union_operator_control(flag)
    value = flag ? FirstOperand.new : SecondOperand.new
    value + 1
  end
  def reflection
    instance_variable_get(:@value)
  end
  def include?(value)
    value == 3
  end
  def +(other)
    @value + other
  end
def reflective_override(value)
  value
end
def override_control
  ProtocolControl.new.reflective_override("ok").upcase
end
def operator_control
    ProtocolControl.new + 2
  end
  def install
    klass = ProtocolControl
    alias_klass = klass
    alias_klass.define_method(:installed) { 19 }
    ProtocolControl.new.installed
  end
end
"#)
        .run_ruby(r#"
control = ProtocolControl.new
raise "implicit class identity lost" unless ProtocolControl.implicit_eval == 31
raise "first union operator lost" unless control.union_operator_control(true) == 42
raise "second union operator lost" unless control.union_operator_control(false) == 43
raise "reflection changed" unless control.reflection == 7
raise "override changed" unless control.include?(3)
raise "app return inference changed" unless control.override_control == "OK"
raise "operator changed" unless control.operator_control == 9
raise "class alias identity lost" unless control.install == 19
raise "JSON support changed" unless control.to_json.is_a?(String)
puts "critic positive controls passed"
"#)
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_generated_model_narrowing() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  def critic_item\n    self\n  end")
        .write("app/views/articles/_critic_narrow.html.erb", "<% case article.critic_item %>\n<% when Article %>\n<%= link_to 'narrowed', article.critic_item %>\n<% end %>\n")
        .run_ruby("article = Article.new(id: 7, title: 'narrowed title'); raise 'model narrowing changed' unless Views::Articles.critic_narrow(article).include?('/articles/7')")
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_native_module_callback_identity() {
    emit_and_run::real_blog()
        .write("app/models/concerns/native_hook.rb", "module NativeHook\n  def self.included(base)\n    base.define_method(:hook_value) { 23 }\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  include NativeHook")
        .run_ruby("raise 'native callback identity lost' unless Article.new.hook_value == 23")
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_errors_message_projections() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  def critic_title_messages\n    errors[:title]\n  end\n  def critic_full_messages\n    errors.full_messages\n  end")
        .run_ruby(r#"
article = Article.new
article.valid?
raise "message indexing changed" unless article.critic_title_messages.include?("can't be blank")
raise "full messages changed" unless article.critic_full_messages.any?
puts "error message runtime controls passed"
"#)
        .assert_passes();
}

#[test]
fn defined_method_operands_are_not_invoked() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/helpers/defined_probe.rb", r#"class DefinedProbe
  def present
    raise "defined? invoked its operand"
  end

  def present_definition
    defined?(self.present)
  end

  def missing_definition
    defined?(self.missing)
  end
end
"#)
        .run_ruby(r#"
probe = DefinedProbe.new
raise "existing method lost" unless probe.present_definition == "method"
raise "missing method admitted" unless probe.missing_definition.nil?
puts "defined? did not invoke either operand"
"#)
        .assert_passes();
}

/// Build each query case independently: declaring a model class method
/// must not accidentally open the old gate for the order/where.not cases.
fn scope_free_query_app(action: &str) -> emit_and_run::Overlay {
    let (model, query) = match action {
        "index" => ("class Widget < ApplicationRecord\nend\n", "Widget.order(:name).limit(1)"),
        "named" => ("class Widget < ApplicationRecord\nend\n", "Widget.where.not(name: nil).order(:name)"),
        "recent" => (
            "class Widget < ApplicationRecord\n  def self.recent\n    order(:name).limit(1)\n  end\nend\n",
            "Widget.recent",
        ),
        _ => panic!("unknown scope-free query action: {action}"),
    };
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", model)
        .write("config/routes.rb", &format!(
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#{action}\"\nend\n"
        ))
        .write("app/controllers/widgets_controller.rb", &format!(
            "class WidgetsController < ApplicationController\n  def {action}\n    render plain: {query}.map {{ |w| w.name }}.join(\",\")\n  end\nend\n"
        ))
}

/// Exercise one action independently, so an order failure cannot mask
/// where.not or the model class method's implicit-self query root.
fn scope_free_query_assertions(action: &str, expected: &str) -> String {
    let nullable_row = if action == "named" { "Widget.create!(name: nil)" } else { "" };
    format!(r#"
require_relative "app/controllers/widgets_controller"
Widget.create!(name: "beta")
Widget.create!(name: "alpha")
{nullable_row}
controller = WidgetsController.new
controller.process_action(:{action})
raise "{action} lost its relation: #{{controller.body}}" unless controller.body == "{expected}"
puts "scope-free {action} passed"
"#)
}

/// Class-root order needs a Relation even without any declared scope.
#[test]
fn scope_free_model_order_runs() {
    scope_free_query_app("index")
        .run_ruby(&scope_free_query_assertions("index", "alpha"))
        .assert_passes();
}

/// Zero-argument where reaches WhereChain independently of the order case.
#[test]
fn scope_free_model_where_not_runs() {
    scope_free_query_app("named")
        .run_ruby(&scope_free_query_assertions("named", "alpha,beta"))
        .assert_passes();
}

/// A model class method's bare order root remains supported without named
/// scopes; this control is independent of other class-root query demands.
#[test]
fn scope_free_model_bare_root_class_method_runs() {
    scope_free_query_app("recent")
        .run_ruby(&scope_free_query_assertions("recent", "alpha"))
        .assert_passes();
}

/// Compile and execute the same three controller actions after booting
/// their emitted in-memory SQLite database.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn scope_free_model_query_builders_run_on_spinel() {
    for (action, expected) in [("index", "alpha"), ("named", "alpha,beta"), ("recent", "alpha")] {
        let script = format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
            scope_free_query_assertions(action, expected)
        );
        scope_free_query_app(action).run_spinel(&script).assert_passes();
    }
}

/// The runtime-provided cable mount survives strict analysis and Rack dispatch.
#[test]
fn builtin_cable_mount_runs_with_supported_sibling_routes() {
    emit_and_run::real_blog()
        .write("app/channels/application_cable/connection.rb", "module ApplicationCable\n  class Connection < ActionCable::Connection::Base\n    def connect\n      reject_unauthorized_connection\n    end\n  end\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ActionController::Base\n  def index\n    render plain: 'widgets'\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get '/widgets', to: 'widgets#index'\n  mount ActionCable.server => '/cable'\nend\n")
        .run_ruby(r##"
require "rack"
require "rack/mock"
# As in overlay_cable_identity, leave the unused reactor dependencies empty.
# The actual app authorization must refuse before any reactor API is called.
%w[nio websocket/driver].each { |feature| $LOADED_FEATURES << "#{feature}.rb" }
module NIO; end
module WebSocket; end
builder = Rack::Builder.new
builder.instance_eval(File.read("config.ru"), "config.ru")
client = Rack::MockRequest.new(builder.to_app)
widgets = client.get("/widgets")
raise "sibling route failed: #{widgets.status} #{widgets.body}" unless widgets.status == 200 && widgets.body == "widgets"
cable = client.get("/cable", "HTTP_HOST" => "example.test", "HTTP_ORIGIN" => "http://example.test")
raise "cable endpoint disappeared: #{cable.status} #{cable.body}" unless cable.status == 401 && cable.body == "Unauthorized\n"
puts "runtime cable endpoint preserved"
"##)
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

/// A Concern macro that writes a `class_attribute` runs when the
/// includer loads, rather than being evaluated at compile time.
#[test]
fn concern_class_attribute_macros_run_at_class_load() {
    let run = class_attribute::overlay().run_ruby(class_attribute::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute contract passed"));
}

/// An explicit nil on a subclass is its value; unset reads the parent's.
#[test]
fn concern_class_attribute_set_to_nil_is_not_unset() {
    let run = class_attribute::nil_overlay().run_ruby(class_attribute::NIL_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute nil contract passed"));
}

/// The same Concern types without a diagnostic of any severity: the
/// macro parameters from the class-body calls (a subclass's included),
/// the helper's keywords through `**options`, `Array(...)`'s elements,
/// and the attribute from the values its methods store.
#[test]
fn concern_class_attribute_macros_are_fully_typed() {
    let controllers = [
        ("probe_controller.rb", "class ProbeController < ApplicationController\n  include PreloadableConfigurationConcern\n  preload_site_configs %w[a b], only: :show\nend\n"),
        ("own_controller.rb", "class OwnController < ProbeController\n  preload_feature_flags %w[f], only: %i[index show]\nend\n"),
        ("inherit_controller.rb", "class InheritController < ProbeController\n  def show\n    render plain: self.class._preload_definitions.length.to_s\n  end\nend\n"),
    ];
    let tree = [
        ("app/controllers/concerns/preloadable_configuration_concern.rb".to_string(), class_attribute::CONCERN.to_string()),
        ("app/controllers/application_controller.rb".to_string(), "class ApplicationController < ActionController::Base\nend\n".to_string()),
    ]
    .into_iter()
    .chain(controllers.iter().map(|(f, s)| (format!("app/controllers/{f}"), s.to_string())))
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let concern: Vec<String> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| {
            (d.span.file.0 as usize)
                .checked_sub(1)
                .and_then(|i| app.sources.get(i))
                .is_some_and(|f| f.path.ends_with("preloadable_configuration_concern.rb"))
        })
        .map(|d| d.message)
        .collect();
    assert!(concern.is_empty(), "concern diagnostics: {concern:#?}");
    // The signature declares the parameter's own type, not the slot's.
    let probe = app.controllers.iter().find(|c| c.name.0.as_str() == "ProbeController").unwrap();
    let preload = probe.class_methods().find(|m| m.name.as_str() == "preload_site_configs").unwrap();
    let Some(roundhouse::ty::Ty::Fn { params, .. }) = &preload.signature else {
        panic!("unsigned: {:?}", preload.signature)
    };
    assert_eq!(
        params[0].ty,
        roundhouse::ty::Ty::Array { elem: Box::new(roundhouse::ty::Ty::Str) },
        "codes"
    );
    // `only: nil` is nil when absent, which the signature has to say.
    let add = probe.class_methods().find(|m| m.name.as_str() == "add_preload_definition").unwrap();
    let Some(roundhouse::ty::Ty::Fn { params, .. }) = &add.signature else {
        panic!("unsigned: {:?}", add.signature)
    };
    let only = params.iter().find(|p| p.name.as_str() == "only").expect("only");
    assert!(
        matches!(&only.ty, roundhouse::ty::Ty::Union { variants } if variants.contains(&roundhouse::ty::Ty::Nil)),
        "only: {:?}",
        only.ty
    );
}

/// The harness itself: the unedited blog emits and its controller
/// suite, which renders every page, passes.
/// `reorder`, `skip_preloading!` and `preload_associations`, the three
/// Relation methods campfire's message paging (basecamp/once-campfire#292)
/// leans on. Preloading is observed by deleting the parts after loading:
/// only a widget whose parts were preloaded still sees them.
fn relation_paging_hooks_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    reordered = Widget.order(:name).reorder(name: :desc).map { |w| w.name }.join(",")

    relation = Widget.includes(:parts).order(:name)
    widgets = relation.skip_preloading!.to_a
    relation.preload_associations(widgets.first(1))
    Part.delete_all
    counts = widgets.map { |w| w.parts.size }.join(",")

    render plain: "#{reordered}|#{counts}"
  end
end
"##)
}

fn relation_paging_hooks_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"
alpha = Widget.create!(name: "alpha")
beta = Widget.create!(name: "beta")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: beta, label: "b1")
controller = WidgetsController.new
controller.process_action(:index)
raise "paging hooks: #{controller.body}" unless controller.body == "beta,alpha|1,0"
puts "relation paging hooks passed"
"##
}

#[test]
fn relation_reorder_and_deferred_preloading_run() {
    relation_paging_hooks_app()
        .run_ruby(relation_paging_hooks_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn relation_reorder_and_deferred_preloading_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        relation_paging_hooks_assertions()
    );
    relation_paging_hooks_app().run_spinel(&script).assert_passes();
}

/// A class method runs against the relation it is reached from: called at
/// implicit self inside a scope body (basecamp/once-campfire#292's
/// `scope :last_page, -> { last_page_of(PAGE_SIZE) }`), and called on a
/// relation no syntactic channel recognizes, here a through-association
/// (campfire's `Current.user.reachable_messages.search(q)
/// .last_page_of_matches(n)`). `a0` sorts first in the whole table but
/// belongs to the other widget and the other shop, so an unscoped page
/// answers it.
fn class_method_relation_scope_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "shops", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "shop_id"
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/shop.rb", "class Shop < ApplicationRecord\n  has_many :widgets\n  has_many :shop_parts, through: :widgets, source: :parts\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :shop\n  has_many :parts\nend\n")
        .write("app/models/part.rb", r#"class Part < ApplicationRecord
  belongs_to :widget

  scope :by_label, -> { order(:label) }
  scope :first_label, -> { labels_page(1) }

  def self.labels_page(size)
    by_label.limit(size).pluck(:label)
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/parts\", to: \"parts#index\"\nend\n")
        .write("app/controllers/parts_controller.rb", r##"class PartsController < ApplicationController
  def index
    widget = Widget.find_by(name: "alpha")
    shop = Shop.find_by(name: "north")
    render plain: "#{widget.parts.first_label.join(",")}|#{shop.shop_parts.where.not(label: nil).labels_page(1).join(",")}"
  end
end
"##)
}

fn class_method_relation_scope_assertions() -> &'static str {
    r##"
require_relative "app/controllers/parts_controller"
north = Shop.create!(name: "north")
south = Shop.create!(name: "south")
alpha = Widget.create!(name: "alpha", shop: north)
beta = Widget.create!(name: "beta", shop: south)
Part.create!(widget: alpha, label: "a2")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: beta, label: "a0")
controller = PartsController.new
controller.process_action(:index)
raise "class method lost its relation: #{controller.body}" unless controller.body == "a1|a1"
puts "class method relation scope passed"
"##
}

#[test]
fn class_methods_run_against_the_relation_they_are_reached_from() {
    class_method_relation_scope_app()
        .run_ruby(class_method_relation_scope_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn class_methods_run_against_the_relation_they_are_reached_from_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        class_method_relation_scope_assertions()
    );
    class_method_relation_scope_app().run_spinel(&script).assert_passes();
}

/// `relation.public_send(direction, size)` where `direction` is a
/// parameter every caller passes as a Symbol literal — campfire's
/// `Page.load(relation, :first | :last, size)` after
/// basecamp/once-campfire#292. The literals are the name set, so the
/// send grounds into a static dispatch, and the counted `last(n)` lands
/// on the runtime's `last_n` because the receiver is a typed Relation.
fn param_selector_dispatch_app() -> emit_and_run::Overlay {
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
        .write("app/models/pager.rb", r#"class Pager
  def self.load(relation, direction, size)
    relation.public_send(direction, size).map { |widget| widget.name }
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    oldest = Pager.load(Widget.order(:name), :first, 2).join(",")
    newest = Pager.load(Widget.order(:name), :last, 1).join(",")
    render plain: "#{oldest}|#{newest}"
  end
end
"##)
}

fn param_selector_dispatch_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"
%w[ beta alpha gamma ].each { |name| Widget.create!(name: name) }
controller = WidgetsController.new
controller.process_action(:index)
raise "selector dispatch: #{controller.body}" unless controller.body == "alpha,beta|gamma"
puts "param selector dispatch passed"
"##
}

#[test]
fn a_send_whose_selector_every_caller_names_dispatches_statically() {
    param_selector_dispatch_app()
        .run_ruby(param_selector_dispatch_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_send_whose_selector_every_caller_names_dispatches_statically_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        param_selector_dispatch_assertions()
    );
    param_selector_dispatch_app().run_spinel(&script).assert_passes();
}

/// assert_select attribute operators (`$=`, `^=`, `*=`) and `:not([…])`,
/// and assert_response's failure message. Rails 8.2-era tests write all
/// of them: basecamp/once-campfire#301 checks `img[src*='install-edge']`,
/// #303 checks `input[type=checkbox]:not([checked])` and passes a message
/// to assert_response.
#[test]
fn assert_select_attribute_operators_and_negation_run() {
    emit_and_run::real_blog()
        .write(
            "test/controllers/article_selectors_controller_test.rb",
            r#"require "test_helper"

class ArticleSelectorsControllerTest < ActionDispatch::IntegrationTest
  test "attribute operators, negation and a response message" do
    article = Article.create!(title: "Selectors", body: "Body text here")
    get article_url(article)
    assert_response :success, "the article page"
    assert_select "a[href$='/edit']"
    assert_select "a[href^='/articles/']"
    assert_select "a[href*='articles']"
    assert_select "h1[class]:not([hidden])"
  end
end
"#,
        )
        .run_test("test/controllers/article_selectors_controller_test.rb")
        .assert_passes();
}

/// `redirect(path: ...)` is Rails' options form, and unlike the
/// positional `redirect("/x")` it keeps the request's query string: an
/// empty query leaves the path alone, a path that already has a `?` is
/// joined with `&`, and the query goes ahead of a fragment. The last two
/// diverge from Rails on purpose, since Rails builds `/articles?sort=new?page=2`
/// and `/articles#top?page=2` (see `synthesize_redirect_controller`).
/// A `%{id}` is decoded by the router and path-escaped again, in both
/// redirect forms, as Rails does: an encoded `#` or `?` stays in the
/// path, `%25` survives, and a non-ASCII or control sequence the router
/// leaves encoded is not escaped twice. Each expectation is Rails 8.1.4's.
/// Jumpstart Pro routes its Devise-era `/users/sign_in` this way.
#[test]
fn a_path_option_redirect_keeps_the_request_query() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/posts\", to: redirect(path: \"/articles\")\n  get \"/filtered\", to: redirect(path: \"/articles?sort=new\")\n  get \"/step\", to: redirect(path: \"/articles#top\")\n  get \"/old/:id\", to: redirect(path: \"/articles/%{id}\", status: 302)\n  get \"/legacy/:id\", to: redirect(\"/articles/%{id}\")\n",
        )
        .write(
            "test/controllers/path_redirects_controller_test.rb",
            r#"require "test_helper"

class PathRedirectsControllerTest < ActionDispatch::IntegrationTest
  test "the options form keeps the query" do
    get "/posts"
    assert_response 301
    assert_redirected_to "/articles"
    get "/posts?page=2"
    assert_redirected_to "/articles?page=2"
    get "/filtered?page=2"
    assert_redirected_to "/articles?sort=new&page=2"
    get "/step?page=2"
    assert_redirected_to "/articles?page=2#top"
    get "/filtered"
    assert_redirected_to "/articles?sort=new"
    get "/old/7?page=2"
    assert_response 302
    assert_redirected_to "/articles/7?page=2"
    get "/old/a%23top?page=2"
    assert_redirected_to "/articles/a%23top?page=2"
    get "/legacy/a%20b%3Fc"
    assert_redirected_to "/articles/a%20b%3Fc"
    get "/old/jos%C3%A9"
    assert_redirected_to "/articles/jos%C3%A9"
    get "/old/100%25"
    assert_redirected_to "/articles/100%25"
    get "/legacy/a%2Fb%7e"
    assert_redirected_to "/articles/a/b~"
    get "/old/x%0Ay"
    assert_redirected_to "/articles/x%0Ay"
  end
end
"#,
        )
        .run_test("test/controllers/path_redirects_controller_test.rb")
        .assert_passes();
}

/// A path parameter is percent-decoded the way Rails' router decodes it
/// (`CGI.unescapeURIComponent`): `%20` is a space, `%2F`/`%2f` a slash
/// inside the one segment, `%25` a percent, and a `+` stays a `+`. A
/// glob capture is decoded too. Every expectation is what Rails 8.1.4
/// answers for the same request.
#[test]
fn a_path_parameter_is_percent_decoded() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/echo/:id\", to: \"echoes#show\"\n  get \"/files/*rest\", to: \"echoes#glob\"\n",
        )
        .write(
            "app/controllers/echoes_controller.rb",
            "class EchoesController < ApplicationController\n  def show\n    render plain: params[:id]\n  end\n\n  def glob\n    render plain: params[:rest]\n  end\nend\n",
        )
        .write(
            "test/controllers/echoes_controller_test.rb",
            r#"require "test_helper"

class EchoesControllerTest < ActionDispatch::IntegrationTest
  test "path parameters are percent-decoded" do
    get "/echo/a%20b+c"
    assert_equal "a b+c", response.body
    get "/echo/a%2Fb"
    assert_equal "a/b", response.body
    get "/echo/a%2fb%3F%23"
    assert_equal "a/b?#", response.body
    get "/echo/100%25"
    assert_equal "100%", response.body
    get "/echo/plain"
    assert_equal "plain", response.body
    get "/files/a%20b/c%2Fd"
    assert_equal "a b/c/d", response.body
  end
end
"#,
        )
        .run_test("test/controllers/echoes_controller_test.rb")
        .assert_passes();
}

/// `Date.parse(params[:from])` types the way a String argument does: a
/// request parameter is a union whose other arms (nil, an Array, nested
/// params) raise in Rails as well. The calendar sends after it run too:
/// a week that starts on a named day, and `in_time_zone` with a zone
/// that may be nil. Expected values are Rails 8.1's.
#[test]
fn date_parse_of_a_request_parameter_runs() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/week\", to: \"weeks#show\"\n",
        )
        .write(
            "app/controllers/weeks_controller.rb",
            r#"class WeeksController < ApplicationController
  def show
    from = Date.parse(params[:from])
    zone = Article.new.title
    at = Time.utc(2024, 2, 15, 10, 0, 0)
    render plain: [
      from.iso8601, from.beginning_of_month.iso8601, Date.parse(params.require(:from)).year,
      from.beginning_of_week.iso8601, from.beginning_of_week(:sunday).iso8601, from.end_of_week(:sunday).iso8601,
      at.beginning_of_week(:sunday).day, at.end_of_week(:wednesday).day,
      from.in_time_zone(zone).strftime("%H:%M")
    ].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/weeks_controller_test.rb",
            r#"require "test_helper"

class WeeksControllerTest < ActionDispatch::IntegrationTest
  test "a request parameter parses as a date" do
    get "/week", params: { from: "2024-02-15" }
    assert_equal "2024-02-15 2024-02-01 2024 2024-02-12 2024-02-11 2024-02-17 11 20 00:00", response.body
  end
end
"#,
        )
        .run_test("test/controllers/weeks_controller_test.rb")
        .assert_passes();
}

/// `Result = Struct.new(…, keyword_init: true)` in a class body is the
/// class it defines: its members, their nil default, and the methods
/// its block adds. Expected values are Ruby's own.
#[test]
fn a_struct_constant_runs_as_its_class() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/render\", to: \"renders#show\"\n",
        )
        .write(
            "app/services/renderer.rb",
            r#"class Renderer
  Result = Struct.new(:title, :html, keyword_init: true)
  Pair = Struct.new(:left, :right, keyword_init: true) do
    def total
      left + right
    end
  end

  def call(text)
    Result.new(title: text, html: "<p>#{text}</p>")
  end

  def pair
    Pair.new(left: 1, right: 2)
  end

  def half
    Pair.new(left: 3)
  end
end
"#,
        )
        .write(
            "app/controllers/renders_controller.rb",
            r#"class RendersController < ApplicationController
  def show
    result = Renderer.new.call(params[:text].to_s)
    result.title = result.title.upcase
    pair = Renderer.new.pair
    render plain: [result.title, result.html, pair.total, pair.left, Renderer.new.half.right.nil?].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/renders_controller_test.rb",
            r#"require "test_helper"

class RendersControllerTest < ActionDispatch::IntegrationTest
  test "struct constants build their classes" do
    get "/render", params: { text: "hi" }
    assert_equal "HI <p>hi</p> 3 1 true", response.body
  end
end
"#,
        )
        .run_test("test/controllers/renders_controller_test.rb")
        .assert_passes();
}

/// `Float#to_d` / `Integer#to_d` build a BigDecimal without
/// `bigdecimal/util`, and the decimal arithmetic after them runs:
/// Float and Integer operands, comparison, `round`, and `floor`/`ceil`
/// with and without digits. `0.1 + 0.2` keeps `Float#to_d`'s 16 digits,
/// and exponent-form floats (`Float#to_s`'s `1.0e-05`) convert as well.
/// Expected values are CRuby 3.4's with bigdecimal 4.1.
#[test]
fn float_to_d_and_decimal_arithmetic_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/decimal\", to: \"decimals#show\"\n",
        )
        .write(
            "app/controllers/decimals_controller.rb",
            r#"class DecimalsController < ApplicationController
  def show
    f = params[:f].to_s.to_f
    i = params[:i].to_s.to_i
    g = params[:a].to_s.to_f + params[:b].to_s.to_f
    a = BigDecimal("1.555")
    price = f.to_d * i.to_d
    render plain: [
      price.to_s, (price - a).to_s, (a / 7).floor(2).to_s, a.round(2).to_s, a.round, a.floor,
      a.ceil(1).to_s, (f - a).to_s, (i * a).to_s, a.to_f, price > a, g.to_d.to_s, (-a).abs.to_s, a.round(0),
      a.to_d.to_s,
      params[:exps].to_s.split(",").map { |e| e.to_f.to_d.to_s }.join(",")
    ].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/decimals_controller_test.rb",
            r#"require "test_helper"

class DecimalsControllerTest < ActionDispatch::IntegrationTest
  test "decimal arithmetic matches CRuby" do
    get "/decimal", params: { f: "2.5", i: "3", a: "0.1", b: "0.2", exps: "1.0e-5,1e20,-1.0e-10,1.2345678901234567e20,-1.2345678901234567e-20" }
    assert_equal "0.75e1 0.5945e1 0.22e0 0.156e1 2 1 0.16e1 0.945e0 0.4665e1 1.555 true 0.3e0 0.1555e1 2 0.1555e1 " \
      "0.1e-4,0.1e21,-0.1e-9,0.1234567890123456e21,-0.1234567890123456e-19", response.body
  end
end
"#,
        )
        .run_test("test/controllers/decimals_controller_test.rb")
        .assert_passes();
}

/// An Integer range's Enumerable calls (`map`, `filter_map`, `select`,
/// `reduce`, `flat_map`, `each_with_object`) run through `to_a`, on an
/// inclusive and an exclusive range and a constant bound.
#[test]
fn integer_range_enumerable_calls_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/months\", to: \"months#show\"\n",
        )
        .write(
            "app/controllers/months_controller.rb",
            r#"class MonthsController < ApplicationController
  MAX = 3

  def show
    a = (1..12).map { |m| "month#{m}" }
    b = (1..MAX).filter_map { |i| i.even? ? i * 2 : nil }
    c = (1..5).select { |i| i > 2 }
    d = (1..4).reduce(0) { |s, i| s + i }
    e = (1..3).flat_map { |i| [i, i] }
    f = (1..3).each_with_object({}) { |i, h| h[i] = i * i }
    g = (2...5).map { |i| i * 10 }
    render plain: [a.first, a.size, b.sum, c.size, d, e.size, f[3], g.join("-")].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/months_controller_test.rb",
            r#"require "test_helper"

class MonthsControllerTest < ActionDispatch::IntegrationTest
  test "range enumerable calls" do
    get "/months"
    assert_equal "month1 12 4 3 10 6 9 20-30-40", response.body
  end
end
"#,
        )
        .run_test("test/controllers/months_controller_test.rb")
        .assert_passes();
}

/// The range reaches those calls as an Array, the shape every target
/// answers: the emitted Ruby reads `(1..n).to_a.filter_map`, and a call
/// that answers its receiver (`each_with_index`) is left on the range.
#[test]
fn integer_range_enumerable_calls_go_through_to_a() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "app/models/calendar.rb",
            "class Calendar\n  def self.even_doubles(n)\n    (1..n).filter_map { |i| i.even? ? i * 2 : nil }\n  end\n\n  def self.walk\n    (1..3).each_with_index { |i, at| i + at }\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let source = walk_files(&emitted)
        .into_iter()
        .find(|text| text.contains("def self.even_doubles"))
        .expect("Calendar emitted");
    assert!(source.contains("(1..n).to_a.filter_map"), "{source}");
    assert!(source.contains("(1..3).each_with_index"), "{source}");
}

/// A test body takes the same rewrites as app code: the analyzer types
/// `(1..3).filter_map` and `2.5.to_d` there too, so without them the
/// emitted test would call a range method or a `bigdecimal/util`
/// reopen no target provides.
#[test]
fn range_and_to_d_rewrites_reach_test_bodies() {
    emit_and_run::real_blog()
        .write(
            "test/models/rewrites_in_tests_test.rb",
            r#"require "test_helper"

class RewritesInTestsTest < ActiveSupport::TestCase
  test "range and decimal calls in a test body" do
    assert_equal [4], (1..3).filter_map { |i| i.even? ? i * 2 : nil }
    assert_equal "0.25e1", 2.5.to_d.to_s
  end
end
"#,
        )
        .run_test("test/models/rewrites_in_tests_test.rb")
        .assert_passes();
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "test/models/rewrites_in_tests_test.rb",
            "require \"test_helper\"\n\nclass RewritesInTestsTest < ActiveSupport::TestCase\n  test \"range\" do\n    assert_equal [4], (1..3).filter_map { |i| i.even? ? i * 2 : nil }\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let test = std::fs::read_to_string(emitted.join("test/models/rewrites_in_tests_test.rb")).expect("emitted test");
    assert!(test.contains("(1..3).to_a.filter_map"), "{test}");
}

fn walk_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.extend(std::fs::read_to_string(&path).ok());
        }
    }
    out
}

/// Rails' own exception classes the app raises answer the status
/// Rails' rescue_responses give them, through the production
/// dispatcher: `ActionController::BadRequest` 400,
/// `ActiveRecord::RecordNotSaved` / `RecordInvalid` /
/// `ActionController::InvalidAuthenticityToken` 422. A rescued
/// `ActiveRecord::ReadOnlyRecord` carries its message.
#[test]
fn rails_exception_classes_answer_their_status() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/fail/:kind\", to: \"failures#show\"\n",
        )
        .write(
            "app/controllers/failures_controller.rb",
            r#"class FailuresController < ApplicationController
  def show
    case params[:kind]
    when "bad" then raise ActionController::BadRequest, "nope"
    when "not_saved" then raise ActiveRecord::RecordNotSaved.new("not saved", Article.new)
    when "invalid" then Article.new(title: "").save!
    when "token" then raise ActionController::InvalidAuthenticityToken
    when "readonly"
      begin
        raise ActiveRecord::ReadOnlyRecord, "frozen"
      rescue ActiveRecord::ReadOnlyRecord => e
        render plain: e.message
      end
    else
      render plain: "ok"
    end
  end
end
"#,
        )
        .run_ruby(
            r##"def call(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  [status, body.respond_to?(:join) ? body.join : body.to_s]
end
{
  "/fail/bad" => 400,
  "/fail/not_saved" => 422,
  "/fail/invalid" => 422,
  "/fail/token" => 422,
  "/fail/none" => 200,
  "/fail/readonly" => 200,
}.each do |path, want|
  got, = call(path)
  raise "#{path} answered #{got}, want #{want}" unless got == want
end
_, body = call("/fail/readonly")
raise "readonly body #{body.inspect}" unless body.include?("frozen")
"##,
        )
        .assert_passes();
}

/// `ActiveRecord::Type::Boolean` casts as `ActiveModel::Type::Boolean`
/// does (its parent), and `deserialize` casts the same way for a
/// boolean. `ActiveSupport::StringInquirer.new(x)` is `x.inquiry`: a
/// method returning it answers `<label>?` as a comparison with the
/// label. Expected values are Rails 8.1's.
#[test]
fn ar_boolean_type_and_string_inquirer_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/flags\", to: \"flags#show\"\n",
        )
        .write(
            "app/services/deploy_env.rb",
            r#"class DeployEnv
  def self.current(name)
    ActiveSupport::StringInquirer.new(name.presence || "development")
  end

  def self.live?(name)
    current(name).production?
  end
end
"#,
        )
        .write(
            "app/controllers/flags_controller.rb",
            r#"class FlagsController < ApplicationController
  def show
    cast = ActiveRecord::Type::Boolean.new.cast(params[:a])
    stored = ActiveRecord::Type::Boolean.new.deserialize(params[:b])
    blank = ActiveRecord::Type::Boolean.new.cast(params[:c])
    render plain: [cast, stored, blank.nil?, DeployEnv.live?(params[:env].to_s), DeployEnv.live?(""), DeployEnv.current("").development?].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/flags_controller_test.rb",
            r#"require "test_helper"

class FlagsControllerTest < ActionDispatch::IntegrationTest
  test "boolean casts and inquirer predicates" do
    get "/flags", params: { a: "0", b: "t", c: "", env: "production" }
    assert_equal "false true true true false true", response.body
  end
end
"#,
        )
        .run_test("test/controllers/flags_controller_test.rb")
        .assert_passes();
}

/// `ActiveRecord::Associations::Preloader.new(records:, associations:).call`
/// is the model's own `preload_associations`, over an Array of one model
/// or a relation. The association reads after it answer what they
/// would have without the preload.
#[test]
fn associations_preloader_call_runs() {
    emit_and_run::real_blog()
        .write(
            "app/models/comment_digest.rb",
            r#"class CommentDigest
  def self.titles
    comments = Comment.order(:id).to_a
    ActiveRecord::Associations::Preloader.new(records: comments, associations: :article).call
    ActiveRecord::Associations::Preloader.new(records: Article.all, associations: [:comments]).call
    comments.map { |c| c.article.title }.join(",") + " " + Article.order(:id).map { |a| a.comments.size }.join(",")
  end
end
"#,
        )
        .run_ruby(
            r##"a = Article.create!(title: "First", body: "A body long enough")
b = Article.create!(title: "Second", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
Comment.create!(article_id: b.id, commenter: "z", body: "three")
got = CommentDigest.titles
raise "got #{got.inspect}" unless got == "First,Second,Second 1,2"
"##,
        )
        .assert_passes();
}

/// A job `perform_later` enqueues under the test adapter is held, not
/// dropped, and a blockless `perform_enqueued_jobs only:` runs it
/// (basecamp/once-campfire#296's tests). Its broadcast is JSON encoded
/// once with `ActiveSupport::JSON.encode` and sent `coder: nil`
/// (#292), so the test pubsub hands it back decoded exactly once.
#[test]
fn held_jobs_run_on_demand_and_pre_encoded_broadcasts_arrive_once() {
    emit_and_run::real_blog()
        .write(
            "app/jobs/notice_job.rb",
            r#"class NoticeJob < ApplicationJob
  def perform(article)
    ActionCable.server.broadcast "notices", ActiveSupport::JSON.encode(articleId: article.id, title: article.title), coder: nil
  end
end
"#,
        )
        .write(
            "test/models/notice_job_test.rb",
            r#"require "test_helper"

class NoticeJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test "a held job runs when performed, and broadcasts pre-encoded JSON" do
    article = Article.create!(title: "Held", body: "Body text here")
    NoticeJob.perform_later(article)
    assert_enqueued_with job: NoticeJob
    assert_equal 0, ActionCable.server.pubsub.broadcasts("notices").size

    perform_enqueued_jobs only: NoticeJob

    notices = ActionCable.server.pubsub.broadcasts("notices").map { |broadcast| JSON.parse(broadcast) }
    assert_equal [ { "articleId" => article.id, "title" => "Held" } ], notices
  end
end
"#,
        )
        .run_test("test/models/notice_job_test.rb")
        .assert_passes();
}

/// rack-test's `Rack::Test::UploadedFile` built from a StringIO, as
/// campfire's undecodable-image test builds one from half a WebP
/// (basecamp/once-campfire#311). It is the `ActionDispatch::Http::UploadedFile`
/// a controller's params read takes. Undefined, it was a constant error
/// on CRuby and, on Spinel, a refusal that kept the whole test file
/// from compiling.
#[test]
fn rack_test_uploaded_file_from_a_stringio_runs() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_upload_test.rb",
            r#"require "test_helper"

class ArticleUploadTest < ActiveSupport::TestCase
  test "a StringIO upload is the file params carry" do
    upload = Rack::Test::UploadedFile.new(StringIO.new("RIFF half"), "image/webp", original_filename: "broken.webp")
    assert_equal "broken.webp", upload.original_filename
    assert_equal "image/webp", upload.content_type
    assert_equal "RIFF half", upload.read
    assert_equal 9, upload.size
    assert upload.is_a?(ActionDispatch::Http::UploadedFile)
  end
end
"#,
        )
        .run_test("test/models/article_upload_test.rb")
        .assert_passes();
}

/// Named binds in `where` and `having` — `:size` used twice and an Array
/// bound into `IN (:labels)` — in the shape of campfire's direct-room
/// lookup (basecamp/once-campfire#310). Unbound, the placeholders reached
/// SQLite as NULL and the lookup found nothing; campfire then created a
/// second room on every Ping. Plus `relation.to_set`.
fn named_binds_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  has_many :parts

  def self.with_exactly(labels)
    joins(:parts).group(:id)
      .having("COUNT(*) = :size AND COUNT(CASE WHEN parts.label IN (:labels) THEN 1 END) = :size", size: labels.size, labels: labels)
      .first
  end
end
"#)
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    exact = Widget.with_exactly(%w[ a1 a2 ])
    named = Widget.where("name = :name", name: "beta").first
    set = Widget.where(name: %w[ alpha beta ]).to_set
    render plain: "#{exact&.name}|#{named&.name}|#{set.size}|#{set.include?(named)}"
  end
end
"##)
}

fn named_binds_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
alpha = Widget.create!(name: "alpha")
beta = Widget.create!(name: "beta")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: alpha, label: "a2")
Part.create!(widget: beta, label: "a1")
controller = WidgetsController.new
controller.process_action(:index)
raise "named binds: #{controller.body}" unless controller.body == "alpha|beta|2|true"
puts "named binds passed"
"#
}

#[test]
fn named_binds_reach_the_query() {
    named_binds_app().run_ruby(named_binds_assertions()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn named_binds_reach_the_query_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        named_binds_assertions()
    );
    named_binds_app().run_spinel(&script).assert_passes();
}

/// `sanitize_sql_array` is the documented array-form entry point (#400).
fn sanitize_sql_array_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def self.quoted(value)
    ActiveRecord::Base.sanitize_sql_array(["SELECT ? AS v", value])
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def index
    render plain: Widget.quoted(1)
  end
end
"##,
        )
}

#[test]
fn sanitize_sql_array_is_supported() {
    sanitize_sql_array_app()
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"
sql = Widget.quoted(1)
raise "sanitize_sql_array: #{sql.inspect}" unless sql == "SELECT 1 AS v"
controller = WidgetsController.new
controller.process_action(:index)
raise "controller: #{controller.body}" unless controller.body == "SELECT 1 AS v"
puts "sanitize_sql_array passed"
"#,
        )
        .assert_passes();
}

/// Relation#ids must preserve uuid / named string keys (#310).
fn relation_ids_uuid_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, force: :cascade do |t|
    t.string "name"
    t.boolean "active", default: true
  end
end
"#,
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widget_ids\", to: \"widgets#ids\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def ids
    render plain: Widget.where(active: true).ids.join(",")
  end
end
"##,
        )
}

#[test]
fn relation_ids_preserves_uuid_keys() {
    relation_ids_uuid_app()
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"
uid = "44444444-4444-4444-8444-444444444441"
Widget.create!(id: uid, name: "a", active: true)
controller = WidgetsController.new
controller.process_action(:ids)
raise "uuid ids: #{controller.body.inspect}" unless controller.body == uid
puts "relation ids uuid passed"
"#,
        )
        .assert_passes();
}

/// Writebook-shaped `ActionText::Markdown < Record` under `module ActionText`
/// in `lib/` is an ordinary model (table `action_text_markdowns`, attr
/// `content`). Storage-only — does not claim `has_markdown`.
#[test]
fn action_text_markdown_saves_and_reloads_content() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content"
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    belongs_to :record, polymorphic: true
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
m = ActionText::Markdown.new
m.content = "# Hello"
m.name = "body"
m.record_type = "Article"
m.record_id = 1
m.save!
reloaded = ActionText::Markdown.find(m.id)
raise "content lost: #{reloaded.content.inspect}" unless reloaded.content == "# Hello"
raise "name lost: #{reloaded.name.inspect}" unless reloaded.name == "body"
puts "action_text markdown storage passed"
"##,
        )
        .assert_passes();
}

/// The pinned Writebook Page tests exercise this exact behavior: a new
/// Page's `markable` method returns the raw Markdown stored by `has_markdown`.
/// Keep the source-shaped `ActionText::Markdown < Record` declaration here so
/// the emitted model resolves the Action Text base against shared runtime code.
#[test]
fn writebook_page_markable_runs_against_emitted_markdown_runtime() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "pages", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    belongs_to :record, polymorphic: true
  end
end
"#,
        )
        .write(
            "app/models/page.rb",
            r#"class Page < ApplicationRecord
  has_markdown :body

  def markable
    body.content.to_s
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r###"
page = Page.new(body: "## Markdown Content\n\nWith **bold** text.")
raise "raw markdown changed: #{page.markable.inspect}" unless page.markable == "## Markdown Content\n\nWith **bold** text."
empty = Page.new(body: "")
raise "empty markdown changed: #{empty.markable.inspect}" unless empty.markable == ""
puts "Writebook Page markable behavior passed"
"###,
        )
        .assert_passes();
}

/// A nested include file must load after the parent class is established,
/// so its namespace can reopen the intended class rather than a placeholder.
#[test]
fn nested_included_module_loads_inside_its_parent_class() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :action_text_markdowns do |t|\n    t.text :content\n  end\nend\n",
        )
        .write(
            "app/models/action_text/markdown.rb",
            r#"module ActionText
  class Markdown < ApplicationRecord
    include ActionText::Markdown::Uploads
  end
end
"#,
        )
        .write(
            "app/models/action_text/markdown/uploads.rb",
            r#"module ActionText
  class Markdown < ApplicationRecord
    module Uploads
      def nested_upload_module_loaded?
        true
      end
    end
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(r#"
raise "nested module was not included" unless ActionText::Markdown.new.nested_upload_module_loaded?
puts "nested include namespace passed"
"#)
        .assert_passes();
}

/// Named plain-text association (`has_markdown :body`): assign through
/// the owner, autosave on save, reload scoped by owner/name. Abstract
/// overlay — Writebook `Page#body` is extra fixture coverage only.
#[test]
fn named_plain_text_attr_assign_save_reload() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_markdown :body\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
a = Article.new
a.title = "Hello"
a.body = "# Title\n\nParagraph"
a.save!
reloaded = Article.find(a.id)
raise "body missing" unless reloaded.body
raise "content lost: #{reloaded.body.content.inspect}" unless reloaded.body.content == "# Title\n\nParagraph"
raise "name wrong: #{reloaded.body.name.inspect}" unless reloaded.body.name == "body"
raise "record_type wrong: #{reloaded.body.record_type.inspect}" unless reloaded.body.record_type == "Article"
raise "record_id wrong: #{reloaded.body.record_id.inspect}" unless reloaded.body.record_id == reloaded.id
# Ordinary autosave includes blank content (unlike RichText blank suppression).
b = Article.create!(title: "Empty")
b.body = ""
b.save!
blank = Article.find(b.id)
raise "blank content not saved: #{blank.body.content.inspect}" unless blank.body.content == ""
raise "predicate false on blank row" unless blank.body?
puts "named plain text attr assign/save/reload passed"
"##,
        )
        .assert_passes();
}

/// `delegated_type` singular reader (`entry.page`) must stay a record
/// reader at runtime — not collide with Relation pagination `page` —
/// and compose with a plain-text attr on the delegated target.
#[test]
fn delegated_type_singular_reader_plain_text_body_runs() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "entries", force: :cascade do |t|
    t.string "entryable_type", null: false
    t.integer "entryable_id", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "pages", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "sections", force: :cascade do |t|
    t.text "body"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "app/models/page.rb",
            "class Page < ApplicationRecord\n  has_markdown :body\n\n  def searchable_content\n    body.content\n  end\nend\n",
        )
        .write(
            "app/models/section.rb",
            "class Section < ApplicationRecord\n  def searchable_content\n    body\n  end\nend\n",
        )
        .write(
            "app/models/entry.rb",
            r#"class Entry < ApplicationRecord
  delegated_type :entryable, types: %w[ Page Section ]
  delegate :searchable_content, to: :entryable
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
page = Page.new
page.body = "# Hello"
page.save!
entry = Entry.create!(entryable: page)
raise "page? false" unless entry.page?
raise "page reader nil" unless entry.page
raise "body content lost: #{entry.page.body.content.inspect}" unless entry.page.body.content == "# Hello"
# Zero-arg `page` on a record is the delegated_type reader, not pagination.
raise "page reader must be Page, got #{entry.page.class}" unless entry.page.is_a?(Page)
raise "delegated method lost" unless entry.searchable_content == "# Hello"
section = Section.create!(body: "Section content")
section_entry = Entry.create!(entryable: section)
raise "delegated method lost on second type" unless section_entry.searchable_content == "Section content"
puts "delegated_type singular reader plain text body passed"
"##,
        )
        .assert_passes();
}

/// Rails 7.2's query assertions and the notification they are built on,
/// over the runtime's statement capture, with `connection.select_rows`
/// answering Arrays: campfire's tests count queries, assert none match a
/// pattern, and read an `EXPLAIN QUERY PLAN` through a `->(*, payload)`
/// callback (basecamp/once-campfire#295, #304, #310, #312).
#[test]
fn query_assertions_and_sql_notifications_run() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_queries_test.rb",
            r#"require "test_helper"

class ArticleQueriesTest < ActiveSupport::TestCase
  test "query assertions count, match and explain" do
    article = Article.create!(title: "Counted", body: "Body text here")

    found = assert_queries_count(1) { Article.find(article.id) }
    assert_equal "Counted", found.title
    assert_no_queries { found.title }
    assert_no_queries_match(/comments/) { Article.find(article.id) }
    assert_queries_match(/articles/) { Article.where(title: "Counted").to_a }

    statements = []
    callback = ->(*, payload) { statements << payload[:sql] }
    ActiveSupport::Notifications.subscribed(callback, "sql.active_record") do
      Article.where(title: "Counted").to_a
    end
    assert_equal 1, statements.size
    # Rails' quoting, which apps' tests filter statements by
    # (`start_with?(%(SELECT "messages"))`, basecamp/once-campfire#312).
    assert statements.first.start_with?(%(SELECT "articles")), statements.first
    plan = Article.connection.select_rows("EXPLAIN QUERY PLAN #{statements.first}").map(&:last).join(" | ")
    assert_match(/articles/, plan)
  end
end
"#,
        )
        .run_test("test/models/article_queries_test.rb")
        .assert_passes();
}

/// `offset(n).exists?` asks for a row past the first n (campfire's
/// `paged?`, basecamp/once-campfire#297) where a COUNT ignored the offset;
/// and the caching knobs campfire's messages caching test turns
/// (`Rails.cache =`, `ActiveSupport::Cache::MemoryStore.new`,
/// `ActionView::PartialRenderer.collection_cache`, a controller's
/// `cache_store` and `perform_caching`).
#[test]
fn offset_exists_and_caching_knobs_run() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_paging_test.rb",
            r#"require "test_helper"

class ArticlePagingTest < ActiveSupport::TestCase
  test "a row past the offset, and the caching knobs" do
    Article.delete_all
    2.times { |i| Article.create!(title: "Paged #{i}", body: "Body text here") }
    assert Article.offset(1).exists?
    assert_not Article.offset(2).exists?

    store = ActiveSupport::Cache::MemoryStore.new
    Rails.cache = store
    ActionView::PartialRenderer.collection_cache = Rails.cache
    ArticlesController.cache_store = Rails.cache
    ArticlesController.perform_caching = true
    assert_same store, Rails.cache
    assert_same store, ActionView::PartialRenderer.collection_cache
    assert_same store, ArticlesController.cache_store
    assert ArticlesController.perform_caching
  end
end
"#,
        )
        .run_test("test/models/article_paging_test.rb")
        .assert_passes();
}

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

/// An app with no jobs still runs its tests. The test helper switches
/// `ActiveJob` to enqueue at load, so it needs the runtime even when no
/// app file names `ActiveJob`.
#[test]
fn an_app_without_jobs_runs_its_tests() {
    emit_and_run::real_blog()
        .remove("app/jobs/application_job.rb")
        .run_test("test/models/article_test.rb")
        .assert_passes();
}

/// `thread_state` replaces the job queue methods with locked, per-thread
/// versions. Boot loads `active_job` first, so a job class that loads it
/// again later does not put the unlocked versions back.
#[test]
fn the_job_queue_keeps_its_thread_safe_methods() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"%i[drain pending_count performed enqueue_locked record_performed_for_tests].each do |m|
  file = ActiveJob.method(m).source_location[0]
  raise "ActiveJob.#{m} comes from #{file}" unless file.end_with?("runtime/thread_state.rb")
end
# The serving drain wraps these two, and calls the locked ones above.
%i[enqueue record_performed].each do |m|
  file = ActiveJob.method(m).source_location[0]
  raise "ActiveJob.#{m} comes from #{file}" unless file.end_with?("runtime/active_job_cruby.rb")
end
puts "ok"
"#,
        )
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

/// ActiveSupport Date calendar: constructors, date-preserving edges, and
/// Date→Time / Integer→Time zone conversions must both type-clean and run.
#[test]
fn activesupport_date_calendar_runs() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "events", force: :cascade do |t|
    t.date "due_on"
  end
end
"#,
        )
        .write(
            "app/models/event.rb",
            r#"class Event < ApplicationRecord
  def month_span
    due_on.beginning_of_month..due_on.end_of_month
  end

  def prior_day
    due_on.yesterday
  end

  def zoned
    due_on.in_time_zone("UTC")
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/probe\", to: \"probe#show\"\nend\n",
        )
        .write(
            "app/controllers/probe_controller.rb",
            r#"class ProbeController < ApplicationController
  def show
    event = Event.create!(due_on: Date.new(2024, 1, 31))
    cur = Date.current
    yday = Date.yesterday
    span = event.month_span
    prior = event.prior_day
    # Non-nilable Date literal: column readers are Date? and binop gate
    # refuses Date? + Integer (see #394); day arithmetic on a known Date
    # still grounds through date_days_since.
    shifted = Date.new(2024, 1, 31) + 2
    zoned = event.zoned
    # Today must not be past? (Rails Date#past? is self < Date.current).
    today_past = Date.current.past?
    old_past = Date.new(2020, 1, 1).past?
    epoch = 1_704_067_200.in_time_zone("UTC")
    render plain: [
      cur.class.name,
      yday.class.name,
      span.begin.iso8601,
      span.end.iso8601,
      prior.iso8601,
      shifted.iso8601,
      zoned.year,
      today_past,
      old_past,
      epoch.year
    ].join(",")
  end
end
"#,
        )
        .run_ruby(
            r#"
require_relative "app/controllers/probe_controller"
controller = ProbeController.new
controller.process_action(:show)
parts = controller.body.split(",")
raise "Date.current class: #{parts[0]}" unless parts[0] == "Date"
raise "Date.yesterday class: #{parts[1]}" unless parts[1] == "Date"
raise "beginning_of_month: #{parts[2]}" unless parts[2] == "2024-01-01"
raise "end_of_month: #{parts[3]}" unless parts[3] == "2024-01-31"
raise "yesterday: #{parts[4]}" unless parts[4] == "2024-01-30"
raise "Date+2: #{parts[5]}" unless parts[5] == "2024-02-02"
raise "in_time_zone year: #{parts[6]}" unless parts[6] == "2024"
raise "today.past?: #{parts[7]}" unless parts[7] == "false"
raise "old.past?: #{parts[8]}" unless parts[8] == "true"
raise "Integer#in_time_zone year: #{parts[9]}" unless parts[9] == "2024"
puts "ActiveSupport Date calendar OK"
"#,
        )
        .assert_passes();
}

/// Spinel has no native `Date#+`; grounding must carry constructors,
/// day arithmetic, and calendar-day `past?` — CRuby stdlib can mask that.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn activesupport_date_calendar_runs_on_spinel() {
    date_blog()
        .edit(
            "app/models/calendar_entry.rb",
            "\nend\n",
            "\n  def prior_day\n    due_on.yesterday\n  end\n\n  def self.probe\n    entry = create!(due_on: Date.new(2024, 1, 31))\n    [\n      Date.current.class.name,\n      entry.due_on.beginning_of_month.iso8601,\n      entry.due_on.end_of_month.iso8601,\n      entry.prior_day.iso8601,\n      (Date.new(2024, 1, 31) + 2).iso8601,\n      Date.current.past?,\n      Date.new(2020, 1, 1).past?,\n    ]\n  end\nend\n",
        )
        .run_spinel(
            r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
parts = CalendarEntry.probe
raise "Date.current class: #{parts[0]}" unless parts[0] == "Date"
raise "beginning_of_month: #{parts[1]}" unless parts[1] == "2024-01-01"
raise "end_of_month: #{parts[2]}" unless parts[2] == "2024-01-31"
raise "yesterday: #{parts[3]}" unless parts[3] == "2024-01-30"
raise "Date+2: #{parts[4]}" unless parts[4] == "2024-02-02"
raise "today.past?: #{parts[5]}" unless parts[5] == false
raise "old.past?: #{parts[6]}" unless parts[6] == true
puts "ActiveSupport Date calendar OK on Spinel"
"#,
        )
        .assert_passes();
}

/// ActiveSupport's Date calendar extensions and `Date.current`, which
/// reads today in the app's zone rather than the host's.
#[test]
fn date_calendar_extensions_and_current_run() {
    date_blog()
        .edit(
            "app/models/calendar_entry.rb",
            "\nend\n",
            "\n  def month_span\n    [due_on.beginning_of_month, due_on.end_of_month]\n  end\n\n  def day_edges\n    [due_on.beginning_of_day, due_on.end_of_day]\n  end\n\n  def self.current_day\n    Date.current\n  end\nend\n",
        )
        .run_ruby(r#"
entry = CalendarEntry.create!(due_on: Date.new(2024, 2, 10))
first, last = entry.month_span
raise first.inspect unless first == Date.new(2024, 2, 1)
raise last.inspect unless last == Date.new(2024, 2, 29)
ActiveSupport.use_zone("Asia/Tokyo") do
  b, e = entry.day_edges
  raise b.inspect unless [b.year, b.month, b.day, b.hour, b.min, b.sec] == [2024, 2, 10, 0, 0, 0]
  raise e.inspect unless [e.year, e.month, e.day, e.hour, e.min, e.sec] == [2024, 2, 10, 23, 59, 59]
  raise b.utc_offset.inspect unless b.utc_offset == 9 * 3600
end
east = ActiveSupport.use_zone("Pacific/Kiritimati") { CalendarEntry.current_day }
west = ActiveSupport.use_zone("Pacific/Pago_Pago") { CalendarEntry.current_day }
raise [east, west].inspect unless east.is_a?(Date) && east > west
"#)
        .assert_passes();
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

/// A jbuilder view over a date column renders what Rails 8.1 + jbuilder
/// 2.15 render: the ISO date, or `null` — both through `json.extract!`
/// and a bare `json.key record.col` pair. The view used to send the
/// date's stored text through `encode_datetime`, which quoted the ""
/// an unset nonnullable slot holds (`"due_on":""`). A timestamp column
/// in the same view keeps its `encode_datetime` route.
#[test]
fn jbuilder_date_column_renders_the_iso_date_or_null() {
    date_blog()
        .edit("db/schema.rb", "t.date \"due_on\"", "t.date \"due_on\", null: false")
        .write("app/controllers/calendar_entries_controller.rb", "class CalendarEntriesController < ApplicationController\n  def show\n    @calendar_entry = CalendarEntry.find(params[:id])\n  end\n\n  def fresh\n    @calendar_entry = CalendarEntry.new\n    render :show\n  end\nend\n")
        .write("app/views/calendar_entries/show.json.jbuilder", "json.extract! @calendar_entry, :due_on, :observed_at\njson.due @calendar_entry.due_on\n")
        .edit("config/routes.rb", "  resources :articles do", "  resources :calendar_entries, only: [:show] do\n    get :fresh, on: :collection\n  end\n  resources :articles do")
        .run_ruby(r#"
require_relative "app/controllers/calendar_entries_controller"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), observed_at: Time.utc(2024, 1, 31, 23, 47, 19, 123456))
controller = CalendarEntriesController.new
controller.params = {"id" => entry.id.to_s}
controller.process_action(:show)
expected = '{"due_on":"2024-01-31","observed_at":"2024-01-31T23:47:19.123Z","due":"2024-01-31"}'
raise controller.body.inspect unless controller.body == expected
controller = CalendarEntriesController.new
controller.params = {}
controller.process_action(:fresh)
expected = '{"due_on":null,"observed_at":null,"due":null}'
raise controller.body.inspect unless controller.body == expected
puts "jbuilder Date JSON is the ISO date or null"
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

/// `Relation#last_n` is Rails' SQL tail (`ORDER BY … DESC LIMIT n`, then
/// reverse), not `to_a.last(n)` over the whole history. Campfire's
/// `ordered.last(PAGE_SIZE)` is every room page.
#[test]
fn relation_last_n_limits_in_sql() {
    emit_and_run::real_blog()
        .run_ruby(
            r##"
seen = []
orig = Db.method(:prepare)
Db.define_singleton_method(:prepare) do |sql|
  seen << sql
  orig.call(sql)
end

Article.create!(title: "tail-a", body: "long enough body")
Article.create!(title: "tail-b", body: "long enough body")
Article.create!(title: "tail-c", body: "long enough body")
Article.create!(title: "tail-d", body: "long enough body")
Article.create!(title: "tail-e", body: "long enough body")

rel = ActiveRecord::Relation.new(Article).where("title LIKE 'tail-%'").order(:title)
prior = rel.to_sql
seen.clear
titles = rel.last_n(2).map(&:title)
raise "tail in relation order: #{titles.inspect}" unless titles == ["tail-d", "tail-e"]
raise "last_n poisoned the chain: #{rel.to_sql}" unless rel.to_sql == prior
sql = seen.find { |s| s.include?("FROM articles") && s.include?("LIMIT") }
raise "last_n did not LIMIT in SQL: #{seen.inspect}" if sql.nil?
raise "reversed order missing: #{sql}" unless sql.upcase.include?("TITLE DESC")
raise "LIMIT 2 missing: #{sql}" unless sql.include?("LIMIT 2")
raise "count poisoned" unless rel.count == 5

raise "bare last" unless rel.last.title == "tail-e"
raise "last poisoned the chain" unless rel.to_sql == prior

rel.to_a
seen.clear
loaded = rel.last_n(2).map(&:title)
raise "loaded tail: #{loaded.inspect}" unless loaded == ["tail-d", "tail-e"]
raise "loaded last_n re-queried: #{seen.inspect}" if seen.any? { |s| s.include?("FROM articles") && s.include?("LIMIT") }

off = ActiveRecord::Relation.new(Article).where("title LIKE 'tail-%'").order(:title).offset(1)
raise "offset tail" unless off.last_n(2).map(&:title) == ["tail-d", "tail-e"]

rel = ActiveRecord::Relation.new(Article)
raise "one col" unless rel.reverse_order_term("title DESC") == "title ASC"
raise "hash join" unless rel.reverse_order_term("a ASC, b DESC") == "a DESC, b ASC"
raise "raw pair" unless rel.reverse_order_term("created_at DESC, id DESC") == "created_at ASC, id ASC"
raise "bare" unless rel.reverse_order_term("title") == "title DESC"
puts "ok"
"##,
        )
        .assert_passes();
}

/// Overlay for the `cached: true` collection-cache gate probes.
fn cached_collection_probe() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  def self.render_count
    @render_count || 0
  end
  def self.reset_render_count
    @render_count = 0
  end
  def self.bump_render
    @render_count = render_count + 1
  end
  def bump_render
    Article.bump_render
    title
  end
"#,
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n\n  def probe\n    @articles = Article.order(:title)\n  end\n",
        )
        .write(
            "app/views/articles/_probe_row.html.erb",
            "<% probe_row.bump_render %><i><%= probe_row.title %></i>\n",
        )
        .write(
            "app/views/articles/probe.html.erb",
            "<%= render partial: \"articles/probe_row\", collection: @articles, cached: true %>\n",
        )
}

/// `n` rows, `second` bumps on the second render (`0` = store hit).
fn assert_cached_collection_probe(n: i64, second: i64) {
    cached_collection_probe()
        .run_ruby(&format!(
            r#"
Article.delete_all
{n}.times {{ |i| Article.create!(title: "row-#{{i}}", body: "long enough body") }}
rows = ActiveRecord::Relation.new(Article).to_a.sort_by {{ |a| a.title }}
ActionController::Base.perform_caching = true
ActionController::Current.controller = ActionController::Base.new
Article.reset_render_count
a = Views::Articles.probe(rows)
raise "first #{{Article.render_count}}: #{{a}}" unless Article.render_count == {n}
Article.reset_render_count
b = Views::Articles.probe(rows)
raise "second #{{Article.render_count}}: #{{b}}" unless Article.render_count == {second}
raise "html drifted #{{a.inspect}} vs #{{b.inspect}}" unless a == b
puts "ok"
"#
        ))
        .assert_passes();
}

/// `cached: true` on a collection render is one store read of the
/// concatenated partials. A second render of the same records must not
/// run the inner fragment bodies. Needs more than
/// `MAX_UNCACHED_COLLECTION_LENGTH` rows — at or below that the cost
/// gate skips the store.
#[test]
fn cached_true_collection_skips_partial_bodies_on_hit() {
    let n = roundhouse::lower::MAX_UNCACHED_COLLECTION_LENGTH + 1;
    assert_cached_collection_probe(n, 0);
}

/// Small `cached: true` collections skip the store: key-build + read
/// would cost more than rendering (Campfire sidebar after #488).
#[test]
fn cached_true_small_collection_skips_the_store() {
    assert_cached_collection_probe(2, 2);
}

/// The exclusive gate: length == MAX is still uncached.
#[test]
fn cached_true_collection_at_gate_skips_the_store() {
    let n = roundhouse::lower::MAX_UNCACHED_COLLECTION_LENGTH;
    assert_cached_collection_probe(n, n);
}

/// `rel.more_than?(n)` is `SELECT 1 LIMIT 1 OFFSET n` with the same
/// FROM/JOIN/WHERE as COUNT, and the relation is not mutated. Campfire's
/// `Message.paged?` is `count > PAGE_SIZE` rewritten to this method.
#[test]
fn relation_more_than_probes_offset_without_count() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_more_than_test.rb",
            r#"require "test_helper"

class ArticleMoreThanTest < ActiveSupport::TestCase
  test "more_than? offsets without COUNT or mutating the relation" do
    Article.delete_all
    3.times { |i| Article.create!(title: "more-#{i}", body: "Body text here") }
    rel = Article.where("title LIKE 'more-%'")
    prior = rel.to_sql
    statements = []
    callback = ->(*, payload) { statements << payload[:sql] }
    ActiveSupport::Notifications.subscribed(callback, "sql.active_record") do
      assert rel.more_than?(2)
      assert_not rel.more_than?(3)
    end
    assert_equal prior, rel.to_sql
    sql = statements.find { |s| s.include?("OFFSET 2") }
    assert sql, statements.inspect
    assert_no_match(/COUNT/i, sql)
    assert_match(/LIMIT 1/, sql)
  end
end
"#,
        )
        .run_test("test/models/article_more_than_test.rb")
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

/// An action whose whole body is a call to a rendering helper defined
/// on a PARENT controller. The default response appended to the action
/// must be guarded by `performed?`, as it is for a helper on the
/// action's own controller, or it overwrites what the helper rendered.
#[test]
fn an_action_responding_through_an_inherited_helper_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    render_title(Article.find(params[:id]))\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through the base controller's helper\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// The same, one call further: the action calls an inherited helper
/// that delegates to the inherited helper that renders.
#[test]
fn an_action_responding_through_a_delegating_inherited_helper_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def report(article)\n    render_title(article)\n  end\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    report(Article.find(params[:id]))\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through a delegating helper\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// The same when the nearest helper is an override that reaches the
/// rendering one through `super`.
#[test]
fn an_action_responding_through_an_override_calling_super_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    render_title(Article.find(params[:id]))\n  end\n\n  private\n\n  def render_title(article)\n    super\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through super\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// `render_code(size: 2, **opts)` into `def render_code(size:, color:
/// "black")`: a keyword bundle splatted AFTER a literal keyword, in a
/// receiverless call. Ingest desugars it to `{ size: 2 }.merge(opts)`;
/// `kwsplat` recovers the keywords, with the literal as the default
/// the bundle is read against — Ruby lets the later `**` win.
#[test]
fn a_keyword_bundle_after_a_literal_keyword_reaches_a_keyword_callee() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def code_svg(**opts)\n    render_code(size: 2, **opts)\n  end\n\n  def render_code(size:, color: \"black\")\n    \"#{title}:#{size}:#{color}\"\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.code_svg(color: \"red\") unless a.code_svg(color: \"red\") == \"Hi:2:red\"\nraise a.code_svg unless a.code_svg == \"Hi:2:black\"\nraise a.code_svg(size: 9) unless a.code_svg(size: 9) == \"Hi:9:black\"",
        )
        .assert_passes();
}

/// The same call where the callee comes from an included concern: the
/// receiverless send resolves through the model's includes.
#[test]
fn a_keyword_bundle_reaches_a_keyword_callee_from_an_included_concern() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/coded.rb",
            "module Coded\n  extend ActiveSupport::Concern\n\n  def render_code(size:, color: \"black\")\n    \"#{size}:#{color}\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Coded\n\n  def code_svg(**opts)\n    render_code(size: 2, **opts)\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.code_svg(color: \"red\") unless a.code_svg(color: \"red\") == \"2:red\"\nraise a.code_svg unless a.code_svg == \"2:black\"",
        )
        .assert_passes();
}

/// real-blog's Article with two token purposes declared from a concern's
/// `included do`: `:share` (a day's expiry, the title as its value) and
/// `:plain` (the id alone, never expiring).
fn article_with_shareable_tokens() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/shareable.rb",
            "module Shareable\n  extend ActiveSupport::Concern\n\n  included do\n    generates_token_for :share, expires_in: 1.day do\n      title\n    end\n    generates_token_for :plain\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Shareable\n",
        )
}

/// `generates_token_for :purpose, expires_in: D do <value> end` (Rails
/// 7.1), synthesized over `ActiveRecord::TokenFor`: a token finds its
/// record, stops verifying when the block's value changes (Rails'
/// contract), rejects tampering and a wrong purpose, the bang form
/// raises InvalidSignature as Rails' does, and the declaration is
/// honored from a concern's `included do` too.
#[test]
fn a_generated_token_finds_its_record_until_its_value_changes() {
    article_with_shareable_tokens()
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\nraise \"find\" unless Article.find_by_token_for(:share, token)&.id == a.id\nraise \"bang\" unless Article.find_by_token_for!(:share, token).id == a.id\nraise \"purpose\" unless Article.find_by_token_for(:plain, token).nil?\nraise \"tamper\" unless Article.find_by_token_for(:share, token + \"x\").nil?\nplain = a.generate_token_for(:plain)\nraise \"plain\" unless Article.find_by_token_for(:plain, plain)&.id == a.id\na.update!(title: \"Changed\")\nraise \"stale\" unless Article.find_by_token_for(:share, token).nil?\nraise \"plain survives\" unless Article.find_by_token_for(:plain, plain)&.id == a.id\nbegin\n  Article.find_by_token_for!(:share, token)\n  raise \"no raise\"\nrescue ActiveSupport::MessageVerifier::InvalidSignature\nend\nputs \"PASS token_for\"",
        )
        .assert_passes();
}

/// A block value goes into the payload as the JSON Rails' `as_json`
/// writes for its type: an Integer is a number (`[1,1]`) and a boolean
/// is `true`/`false` (`[1,false]`), not a quoted string. Both tokens
/// were minted by Rails 8.1.4 (`SECRET_KEY_BASE=test-secret`) and are
/// unexpiring, so the emitted app must mint the same bytes and accept
/// Rails' own.
#[test]
fn a_non_string_token_value_is_the_json_rails_writes() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :counted do\n    id\n  end\n  generates_token_for :flagged do\n    title.nil?\n  end\n",
        )
        .run_ruby(
            r#"
Rails.secret_key_base = "test-secret"
counted = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsMV0sInB1ciI6IkFydGljbGVcbmNvdW50ZWRcbiJ9fQ==--d02f8c1104bf97b778253f734d7156c48eb98e88"
flagged = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsZmFsc2VdLCJwdXIiOiJBcnRpY2xlXG5mbGFnZ2VkXG4ifX0=--fb27c1ecd775cb15bc0966020341a5c07c7ab57e"
a = Article.create!(title: "Hello", body: "Body text here")
raise "expected the first row" unless a.id == 1
raise "integer value differs from rails" unless a.generate_token_for(:counted) == counted
raise "boolean value differs from rails" unless a.generate_token_for(:flagged) == flagged
raise "rails integer token rejected" unless Article.find_by_token_for(:counted, counted)&.id == 1
raise "rails boolean token rejected" unless Article.find_by_token_for(:flagged, flagged)&.id == 1
puts "PASS typed values"
"#,
        )
        .assert_passes();
}

/// A multi-statement block body must still synthesize: the typed
/// payload writers parenthesize the emitted expression so a Seq (or a
/// modifier-`if`) is legal as a call argument. Without that, prism
/// rejects the synthesized source, methods stay typed but undefined
/// (invariant 6), and `generate_token_for` raises NoMethodError.
#[test]
fn a_multi_statement_token_block_still_synthesizes() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :share do\n    t = title\n    t\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\nraise \"find\" unless Article.find_by_token_for(:share, token)&.id == a.id\na.update!(title: \"Changed\")\nraise \"stale\" unless Article.find_by_token_for(:share, token).nil?\nputs \"PASS multi-statement token block\"",
        )
        .assert_passes();
}

/// Declaring a purpose twice keeps the last declaration, as Rails'
/// `token_definitions.merge` does: the token carries the second block's
/// value, so changing the first block's value leaves it valid.
#[test]
fn a_redeclared_token_purpose_uses_the_last_declaration() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :share do\n    title\n  end\n  generates_token_for :share, expires_in: 1.hour do\n    body\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\na.update!(title: \"Changed\")\nraise \"first declaration used\" unless Article.find_by_token_for(:share, token)&.id == a.id\na.update!(body: \"Other body text\")\nraise \"last declaration ignored\" unless Article.find_by_token_for(:share, token).nil?\nputs \"PASS redeclared\"",
        )
        .assert_passes();
}

/// The tokens are Rails' own: ones minted by Rails 8.1.4 for the same
/// declarations verify here, with `SECRET_KEY_BASE=test-secret` and the
/// clock at 2100-01-01 so the day's expiry is still ahead:
///
///   {"_rails":{"data":[1,"Hello"],"exp":"2100-01-02T00:00:00.000Z",
///    "pur":"Article\nshare\n86400"}}
///   {"_rails":{"data":[1],"pur":"Article\nplain\n"}}
///
/// and a token minted here is the bytes Rails mints for the same record
/// at the same instant, since the purpose, payload and absent `exp` are
/// all Rails' choices.
#[test]
fn a_generated_token_minted_by_rails_verifies() {
    article_with_shareable_tokens()
        .run_ruby(
            r#"
Rails.secret_key_base = "test-secret"
share = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsIkhlbGxvIl0sImV4cCI6IjIxMDAtMDEtMDJUMDA6MDA6MDAuMDAwWiIsInB1ciI6IkFydGljbGVcbnNoYXJlXG44NjQwMCJ9fQ==--b944fbcc044254e17f4d952e9a1c1e63ec185093"
plain = "eyJfcmFpbHMiOnsiZGF0YSI6WzFdLCJwdXIiOiJBcnRpY2xlXG5wbGFpblxuIn19--5a8e42fe42d686af11a02e972f7e5e1c88bb57db"
a = Article.create!(title: "Hello", body: "Body text here")
raise "expected the first row" unless a.id == 1
raise "rails share token rejected" unless Article.find_by_token_for(:share, share)&.id == 1
raise "rails plain token rejected" unless Article.find_by_token_for!(:plain, plain).id == 1
raise "unexpiring token differs from rails" unless a.generate_token_for(:plain) == plain
a.update!(title: "Changed")
raise "stale rails token accepted" unless Article.find_by_token_for(:share, share).nil?
puts "PASS rails tokens"
"#,
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

/// `Rails.application.routes.recognize_path` in a helper. `check` was
/// clean, and the emitted app raised NameError on `#routes`. Each
/// expected value is the output of Rails 7.2.4 for the same routes.
/// Rails returns Symbol keys with String values, puts `:format` last,
/// and raises `ActionController::RoutingError` when no route matches.
/// Rails takes a full URL, and normalizes any other path: one leading
/// slash, no doubled slashes and no trailing slash. The error message
/// shows the normalized path.
/// The `:method` of the second argument is the verb, a Symbol or a
/// String in any case, and GET is the default. A verb that Rails does
/// not accept raises `ActionController::UnknownHttpMethod`, which is not
/// a `RoutingError`. The helper rescues it as a `StandardError`: app
/// code that names the class gets a `check` error.
/// The emitted view does not escape the result of a helper call, and
/// Rails does. The `(&quot;|")` alternative in the message check only
/// tolerates that gap, which exists before this test.
#[test]
fn a_helper_recognizes_a_path_with_the_application_routes() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/articles_helper.rb",
            r##"module ArticlesHelper
  def route_of(path, environment = {})
    recognized = Rails.application.routes.recognize_path(path, environment)
    recognized.map { |key, value| "#{key}=#{value}" }.join(" ")
  rescue ActionController::RoutingError => e
    "none: #{e.message}"
  rescue StandardError => e
    "#{e.class.name}: #{e.message}"
  end
end
"##,
        ),
        r#"<i id="rp-index"><%= route_of("/articles") %></i>
<i id="rp-show"><%= route_of("/articles/7?tab=comments") %></i>
<i id="rp-fragment"><%= route_of("/articles/7#comments") %></i>
<i id="rp-root"><%= route_of("/") %></i>
<i id="rp-json"><%= route_of("/articles/7.json") %></i>
<i id="rp-missing"><%= route_of("/nowhere") %></i>
<i id="rp-get"><%= route_of("/articles", method: :get) %></i>
<i id="rp-post"><%= route_of("/articles", method: :post) %></i>
<i id="rp-string"><%= route_of("/articles/7/comments/3", method: "Delete") %></i>
<i id="rp-no-verb"><%= route_of("/articles", method: :put) %></i>
<i id="rp-direct"><%= Rails.application.routes.recognize_path("/articles", method: :post)[:action] %></i>
<i id="rp-head"><%= route_of("/articles/7", method: :head) %></i>
<i id="rp-any"><%= route_of("/articles", method: :any) %></i>
<i id="rp-foo"><%= route_of("/articles", method: :foo) %></i>
<i id="rp-url"><%= route_of("http://example.com/articles/7") %></i>
<i id="rp-url-port"><%= route_of("https://example.com:3000/articles?x=1") %></i>
<i id="rp-relative"><%= route_of("articles/7") %></i>
<i id="rp-relative-miss"><%= route_of("nowhere") %></i>
<i id="rp-relative-url-query"><%= route_of("articles/7?next=http://x") %></i>
<i id="rp-doubled"><%= route_of("/articles//7") %></i>
<i id="rp-trailing-miss"><%= route_of("/nowhere/") %></i>
<i id="rp-bad-uri"><%= route_of("/articles/a b") %></i>
<i id="rp-url-trailing"><%= route_of("http://example.com/articles/7/") %></i>
<i id="rp-percent"><%= route_of("/nowhere/%7e") %></i>
<i id="rp-bad-uri-first"><%= route_of("/a b", method: :foo) %></i>
<i id="rp-same"><%= Rails.application.routes.recognize_path("/articles/7.json") == { controller: "articles", action: "show", id: "7", format: "json" } %></i>
"#,
        r#"    assert_match(/<i id="rp-index">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-show">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-fragment">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-root">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-json">controller=articles action=show id=7 format=json<\/i>/, response.body)
    assert_match(/<i id="rp-missing">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-get">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-post">controller=articles action=create<\/i>/, response.body)
    assert_match(/<i id="rp-string">controller=comments action=destroy article_id=7 id=3<\/i>/, response.body)
    assert_match(/<i id="rp-no-verb">none: No route matches (&quot;|")\/articles(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-direct">create<\/i>/, response.body)
    assert_match(/<i id="rp-head">controller=articles action=show id=7<\/i>/, response.body)
    assert_includes(response.body, '<i id="rp-any">ActionController::UnknownHttpMethod: ANY, accepted HTTP methods are OPTIONS, GET, HEAD, POST, PUT, DELETE, TRACE, CONNECT, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, VERSION-CONTROL, REPORT, CHECKOUT, CHECKIN, UNCHECKOUT, MKWORKSPACE, UPDATE, LABEL, MERGE, BASELINE-CONTROL, MKACTIVITY, ORDERPATCH, ACL, SEARCH, MKCALENDAR, and PATCH</i>')
    assert_match(/<i id="rp-foo">ActionController::UnknownHttpMethod: FOO, accepted HTTP methods are OPTIONS, GET, /, response.body)
    assert_match(/<i id="rp-url">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-url-port">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-relative">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-relative-miss">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-relative-url-query">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-doubled">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-trailing-miss">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-bad-uri">none: bad URI \(is not URI\?\): (&quot;|")\/articles\/a b(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-url-trailing">none: No route matches (&quot;|")http:\/\/example.com\/articles\/7\/(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-percent">none: No route matches (&quot;|")\/nowhere\/%7E(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-bad-uri-first">none: bad URI \(is not URI\?\): (&quot;|")\/a b(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-same">true<\/i>/, response.body)
"#,
    );
    run.assert_passes();
}

/// `recognize_path` gives the Rails controller path, not the flat
/// router name, for a namespaced controller, for the health controller
/// and for the Active Storage controllers. It skips a redirect route and
/// tries the later routes, as Rails does. The app has no root route, so
/// `/` matches no route. Each expected value is the output of Rails
/// 7.2.4 for the same routes.
#[test]
fn a_helper_recognizes_controller_paths_and_skips_redirect_routes() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .edit("config/routes.rb", "  root \"articles#index\"\n", "")
            .edit(
                "config/routes.rb",
                "  resources :articles do",
                "  namespace :admin do\n    resources :articles, only: [:index]\n  end\n  \
                 get \"old\", to: redirect(\"/articles\")\n  \
                 get \"moved\", to: redirect(\"/articles\")\n  \
                 get \"moved\", to: \"articles#index\"\n  \
                 get \"up\" => \"rails/health#show\"\n  \
                 resources :articles do",
            )
            .write(
                "app/controllers/admin/articles_controller.rb",
                "class Admin::ArticlesController < ApplicationController\n  def index\n    @articles = Article.all\n  end\nend\n",
            )
            .write("app/views/admin/articles/index.html.erb", "<p>admin</p>\n")
            .write(
                "app/helpers/articles_helper.rb",
                r##"module ArticlesHelper
  def route_of(path)
    recognized = Rails.application.routes.recognize_path(path)
    recognized.map { |key, value| "#{key}=#{value}" }.join(" ")
  rescue ActionController::RoutingError => e
    "none: #{e.message}"
  end
end
"##,
            ),
        r#"<i id="rp-namespaced"><%= route_of("/admin/articles") %></i>
<i id="rp-redirect"><%= route_of("/old") %></i>
<i id="rp-after-redirect"><%= route_of("/moved") %></i>
<i id="rp-health"><%= route_of("/up") %></i>
<i id="rp-disk"><%= route_of("/rails/active_storage/disk/k/x.png") %></i>
<i id="rp-blob"><%= route_of("/rails/active_storage/blobs/redirect/abc/x.png") %></i>
<i id="rp-no-root"><%= route_of("/") %></i>
"#,
        r#"    assert_match(/<i id="rp-namespaced">controller=admin\/articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-redirect">none: No route matches (&quot;|")\/old(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-after-redirect">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-health">controller=rails\/health action=show<\/i>/, response.body)
    assert_match(/<i id="rp-disk">controller=active_storage\/disk action=show encoded_key=k filename=x format=png<\/i>/, response.body)
    assert_match(/<i id="rp-blob">controller=active_storage\/blobs\/redirect action=show signed_id=abc filename=x format=png<\/i>/, response.body)
    assert_match(/<i id="rp-no-root">none: No route matches (&quot;|")\/(&quot;|")<\/i>/, response.body)
"#,
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

/// `Hash#to_query` is the scalar query string Rails builds: symbol or
/// string keys, a nil value with no `=`, and insertion order. Nested
/// hashes stay on the ruby-family reopen.
#[test]
fn a_hash_to_query_renders_symbol_and_string_keys() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def query_probe
    [
      { name: "Ada", role: nil }.to_query,
      { "name" => "Ada", "role" => "editor" }.to_query,
      {}.to_query
    ].join("|")
  end
"#,
        )
        .write(
            "test/models/article_hash_query_test.rb",
            r#"require "test_helper"

class ArticleHashQueryTest < ActiveSupport::TestCase
  test "Hash#to_query renders symbol keys, string keys, and a nil value" do
    assert_equal "name=Ada&role|name=Ada&role=editor|", Article.new.query_probe
  end
end
"#,
        )
        .run_test("test/models/article_hash_query_test.rb")
        .assert_passes();
}

/// `Array.wrap` is ActiveSupport's class method: nil is empty, an array
/// stays an array, and a scalar becomes a one-element array.
#[test]
fn array_wrap_keeps_nil_an_array_and_a_scalar_distinct() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def wrapped_nil
    Array.wrap(nil).map { |item| item.to_s }.join(",")
  end

  def wrapped_array
    Array.wrap(%w[a b]).map { |item| item.to_s }.join(",")
  end

  def wrapped_string
    Array.wrap("solo").map { |item| item.to_s }.join(",")
  end

  def wrapped_integer
    Array.wrap(7).map { |item| item.to_s }.join(",")
  end

  # One caller passes an array, the default is nil, so the parameter
  # is `Array | Nil`. Folding that to one shape would nest the array
  # or wrap nil. The call stays and answers both.
  def wrapped_either(value = nil)
    Array.wrap(value).map { |item| item.to_s }.join(",")
  end

  def either_from_array
    wrapped_either(%w[a b])
  end
"#,
        )
        .write(
            "test/models/article_array_wrap_test.rb",
            r#"require "test_helper"

class ArticleArrayWrapTest < ActiveSupport::TestCase
  test "Array.wrap keeps nil, an array, and a scalar distinct" do
    article = Article.new
    assert_equal "", article.wrapped_nil
    assert_equal "a,b", article.wrapped_array
    assert_equal "solo", article.wrapped_string
    assert_equal "7", article.wrapped_integer
    assert_equal "", article.wrapped_either
    assert_equal "", article.wrapped_either(nil)
    assert_equal "a,b", article.wrapped_either(%w[a b])
  end
end
"#,
        )
        .run_test("test/models/article_array_wrap_test.rb")
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

/// A `T::Struct` nested in a controller concern must lower and run:
/// keyword construction, readers, and a writable `prop`. Taking that
/// nested class as the controller used to drop the declarations as
/// unrecognized macros and skip the concern's module path entirely.
#[test]
fn a_t_struct_nested_in_a_controller_concern_runs() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/window_settings.rb",
            concat!(
                "module WindowSettings\n",
                "  extend ActiveSupport::Concern\n",
                "\n",
                "  class Span < T::Struct\n",
                "    const :from_date, String\n",
                "    const :to_date, String\n",
                "    prop :label, String, default: \"window\"\n",
                "  end\n",
                "end\n",
            ),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  include WindowSettings\n",
        )
        .run_ruby(
            concat!(
                "span = WindowSettings::Span.new(from_date: \"2026-01-01\", to_date: \"2026-01-31\")\n",
                "raise \"from\" unless span.from_date == \"2026-01-01\"\n",
                "raise \"to\" unless span.to_date == \"2026-01-31\"\n",
                "raise \"default\" unless span.label == \"window\"\n",
                "span.label = \"quarter\"\n",
                "raise \"prop\" unless span.label == \"quarter\"\n",
            ),
        )
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
fn enum_negative_scopes_and_stored_values_run() {
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
    assert_equal 1, ActiveRecord.adapter.find("articles", reloaded.id)["state"]
    assert_equal "l", ActiveRecord.adapter.find("articles", reloaded.id)["tone"]
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

/// `cookies.permanent` in each spelling Rails accepts, over one plain
/// write as the control. The permanent jar was the identity until
/// 2026-10-06, so these went out with no Expires and ended with the
/// browser session: campfire's sign-in did not survive a restart.
fn permanent_cookie_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/visits_controller.rb",
            r#"class VisitsController < ApplicationController
  def index
    cookies.permanent[:last_room] = 7
    cookies.signed.permanent[:session_token] = { value: "tok", httponly: true, same_site: :lax }
    cookies.permanent.signed[:remember] = "me"
    cookies[:plain] = "p"
    head :no_content
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :visits, only: :index\nend\n",
        )
        .write("app/models/visit.rb", "class Visit < ApplicationRecord\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"visits\", force: :cascade do |t|\n    t.string \"room\"\n  end\nend\n",
        )
}

#[test]
fn permanent_cookies_go_out_with_an_expiry() {
    permanent_cookie_app()
        .run_ruby(
            r##"status, headers, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/visits", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "GET /visits answered #{status}" unless status == 204
lines = headers["set-cookie"] || []
year = (Time.now.utc.year + 20).to_s
%w[last_room session_token remember].each do |name|
  line = lines.find { |l| l.start_with?("#{name}=") } or raise "no Set-Cookie for #{name}: #{lines.inspect}"
  raise "#{name} has no twenty-year Expires: #{line}" unless line =~ /; Expires=\w{3}, \d{2} \w{3} #{year} \d{2}:\d{2}:\d{2} GMT/
end
plain = lines.find { |l| l.start_with?("plain=") } or raise "no Set-Cookie for plain: #{lines.inspect}"
raise "a plain cookie must stay a session cookie: #{plain}" if plain.include?("Expires")
session = lines.find { |l| l.start_with?("session_token=") }
raise "options still apply under permanent: #{session}" unless session.include?("SameSite=Lax") && session.include?("HttpOnly")
puts "permanent cookies passed"
"##,
        )
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn permanent_cookies_record_an_expiry_on_spinel() {
    permanent_cookie_app()
        .run_spinel(
            r##"controller = VisitsController.new
controller.process_action(:index)
jar = controller.cookies
year = (Time.now.utc.year + 20).to_s
["last_room", "session_token", "remember"].each do |name|
  exp = jar.flag_expires(name)
  raise "#{name} has no twenty-year expiry: #{exp.inspect}" unless exp.split(" ")[3] == year && exp.end_with?(" GMT")
end
raise "a plain cookie must stay a session cookie" unless jar.flag_expires("plain") == ""
raise "the signed permanent value must round-trip" unless jar.signed[:session_token] == "tok"
puts "permanent cookies passed"
"##,
        )
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

  def self.https?(url)
    URI.parse(url).is_a?(URI::HTTPS)
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

  def self.construct_https
    URI::HTTPS.new("https", nil, "example.test", 443, nil, "/", nil, nil, nil).to_s
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
raise unless HttpConstantProbe.https?("https://example.test/")
raise if HttpConstantProbe.https?("http://example.test/")
raise unless HttpConstantProbe.invalid_uri == "invalid"
raise unless HttpConstantProbe.construct == "http://example.test/"
raise unless HttpConstantProbe.construct_https == "https://example.test/"
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

/// `raise ActionController::RoutingError` in an action answers 404, as
/// Rails' `ActionDispatch::ExceptionWrapper` maps it to `:not_found`. A
/// Rails app raises it for a page number out of bounds. Rails' second
/// constructor argument, `failures`, is optional. The raise is in
/// `index`, which has no `before_action`, so a `RecordNotFound` from
/// `set_article` cannot answer the 404 in its place.
#[test]
fn an_action_that_raises_routing_error_answers_404() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n",
            "  def index\n    raise ActionController::RoutingError.new(\"page out of bounds\") if params[:page] == \"0\"\n    raise ActionController::RoutingError.new(\"No route matches\", []) if params[:page] == \"-1\"\n",
        )
        .run_ruby(
            r#"def get(path, query)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  status
end
{ "page=0" => 404, "page=-1" => 404, "page=1" => 200, "" => 200 }.each do |query, want|
  got = get("/articles", query)
  raise "GET /articles?#{query} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

#[path = "emit_and_run/concern_accessors.rs"]
mod concern_accessors;

#[path = "emit_and_run/action_not_found.rs"]
mod action_not_found;

#[path = "emit_and_run/finder_miss_readers.rs"]
mod finder_miss_readers;

#[path = "emit_and_run/finder_miss_messages.rs"]
mod finder_miss_messages;

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

/// Gap #18.1: Tim Tischler's trailing keyword-hash enum runtime pin.
#[test]
fn enum_keyword_hash_mapping_predicate_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"kind\", default: \"kind\", null: false\n    t.integer \"priority\", default: 17, null: false",
        )
        .edit(
            "app/models/article.rb",
            "has_many :comments, dependent: :destroy\n",
            "has_many :comments, dependent: :destroy\n\n  enum :kind, kind: 'kind', other: 'other'\n  enum :priority, pending: 17, priority: 41\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n  <p id=\"kind-predicate\"><%= @article.kind? %></p>\n  <p id=\"priority-predicate\"><%= @article.priority? %></p>",
        )
        .run_ruby(r#"
article = Article.create!(title: "Enum control", body: "A sufficiently long body")
raise "enum default predicate is false" unless article.kind?
article.kind = "other"
raise "enum predicate ignored its value" if article.kind?
raise "bare integer mapping lost stored values" unless Article.priorities == {"pending" => 17, "priority" => 41}
raise "integer enum default label is wrong" unless article.pending?
raise "column predicate shadowed enum comparison" if article.priority?
article.priority = 41
raise "integer label predicate is false" unless article.priority?
raise "integer label predicate ignored its value" if article.pending?
"#)
        .assert_passes();
}

#[test]
fn anonymous_keywords_run_without_capturing_user_bindings() {
    const METHODS: &str = r#"
  def self.pr197_forward(label, __fwd_kwargs, **)
    [label - __fwd_kwargs, pr197_sink(**)]
  end
  def self.pr197_sink(factor:, offset: 2)
    factor * 3 + offset
  end
  def self.pr197_local_collision(**)
    __fwd_kwargs = {factor: 99}
    pr197_sink(**)
  end
  def pr197_instance(label, __fwd_kwargs, **)
    [label - __fwd_kwargs, pr197_instance_sink(**)]
  end
  def pr197_instance_sink(factor:, offset: 2)
    factor * 3 + offset
  end
"#;
    let library = format!("class KeywordProbe\n{METHODS}end\n");
    emit_and_run::real_blog()
        .write("app/services/keyword_probe.rb", &library)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{METHODS}"))
        .run_ruby(r#"
[Article, KeywordProbe].each do |owner|
  raise "keyword packet captured a local" unless owner.pr197_local_collision(factor: 7) == 23
  raise "class keyword packet captured a positional" unless owner.pr197_forward(11, 4, factor: 7, offset: 5) == [7, 26]
  raise "class keyword default lost" unless owner.pr197_forward(11, 4, factor: 7) == [7, 23]
  instance = owner.new
  raise "instance keyword packet captured a positional" unless instance.pr197_instance(11, 4, factor: 7, offset: 5) == [7, 26]
  raise "instance keyword default lost" unless instance.pr197_instance(11, 4, factor: 7) == [7, 23]
  begin
    owner.pr197_forward(11, 4)
    raise "missing required keyword accepted"
  rescue ArgumentError
  end
end
"#).assert_passes();
}

#[test]
fn anonymous_keywords_forward_empty_and_false_values_through_super() {
    emit_and_run::real_blog()
        .write("app/services/keyword_parent.rb", r#"class KeywordParent
  def call(factor: false, offset: nil, **)
    [factor, offset]
  end
end
class KeywordChild < KeywordParent
  def call(**)
    super(**)
  end
end
"#)
        .run_ruby(r#"
probe = KeywordChild.new
raise "empty keyword packet changed defaults" unless probe.call == [false, nil]
raise "false or nil keyword was dropped" unless probe.call(factor: nil, offset: false) == [nil, false]
"#).assert_passes();
}

#[test]
fn destructuring_preserves_user_bindings_and_expression_values() {
    const TEMPLATE: &str = r#"class HygieneProbe
  def self.targets
    a, *TARGET, c = [11, 22, 33]
    [a, TARGET, c]
  end
  def self.scope(PARAM)
    a, *middle, c = [11, 22, 33]
    [a, middle, c, PARAM]
  end
  def self.expression
    (a, *middle, c = [11, 22, 33, 44])
  end
  def self.instance_targets
    @a, *@middle, @c = [*[11, 22], 33, 44]
    [@a, @middle, @c]
  end
end
"#;
    // Deliberately collide with both span-derived stems. Parameter names
    // change later offsets, so settle the source before ingesting it.
    let mut target = "__target".to_string();
    let mut param = "__param".to_string();
    let source = loop {
        let source = TEMPLATE.replace("TARGET", &target).replace("PARAM", &param);
        let next_target = format!("__mw_{}", source.find("a, *").unwrap());
        let next_param = format!("__mw_{}", source.find("a, *middle").unwrap());
        if target == next_target && param == next_param { break source }
        target = next_target;
        param = next_param;
    };
    const ASSERTIONS: &str = r#"
raise "temporary captured rest target" unless HygieneProbe.targets == [11, [22], 33]
raise "temporary captured a parameter" unless HygieneProbe.scope(41) == [11, [22], 33, 41]
raise "assignment expression lost RHS" unless HygieneProbe.expression == [11, 22, 33, 44]
raise "ivar targets or array splat changed" unless HygieneProbe.instance_targets == [11, [22, 33], 44]
"#;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{source}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog().write("app/services/hygiene_probe.rb", &source)
        .run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn post_rest_destructuring_handles_short_arrays_and_evaluates_once() {
    const SOURCE: &str = r#"class DestructureProbe
  def self.first_value
    @calls ||= 0
    @calls = @calls + 1
    11
  end
  def self.short
    a, *b, c, d = [first_value, 22]
    [a, b, c, d, @calls]
  end
  def self.empty
    a, *b, c, d = []
    [a, b, c, d]
  end
  def self.one
    a, *b, c, d = [11]
    [a, b, c, d]
  end
  def self.exact
    a, *b, c, d = [11, 22, 33]
    [a, b, c, d]
  end
  def self.long
    a, *b, c, d = [11, 22, 33, 44, 55]
    [a, b, c, d]
  end
  def self.discard
    a, *, c, d = [11, 22]
    [a, c, d]
  end
end
"#;
    const ASSERTIONS: &str = r##"
expected = {short: [11, [], 22, nil, 1], empty: [nil, [], nil, nil], one: [11, [], nil, nil], exact: [11, [], 22, 33], long: [11, [22, 33], 44, 55], discard: [11, 22, nil]}
expected.each do |method, want|
  got = DestructureProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/services/destructure_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn class_variable_compound_writes_share_the_read_storage() {
    emit_and_run::real_blog()
        .write("app/services/counter_probe.rb", r#"class CounterProbe
  @@count = nil
  def next_value
    @@count ||= 11
    @@count = @@count + 3
    @@count
  end
  def operators
    @@count += 7
    @@count -= 3
    @@count &&= @@count + 2
    @@count
  end
  def skip
    @@count = false
    @@count &&= explode
    @@count
  end
  def explode
    raise "short circuit evaluated RHS"
  end
  def self.current
    @@count
  end
end
class CounterChild < CounterProbe
end
"#)
        .run_ruby(r#"
raise "native nil initializer was dropped" unless CounterProbe.current.nil? && CounterChild.current.nil?
raise "compound write and read used different storage" unless CounterChild.new.next_value == 14
raise "class reader used per-class storage" unless CounterProbe.current == 14 && CounterChild.current == 14
raise "class variable storage split across inheritance" unless CounterProbe.new.next_value == 17
raise "class reader lost the shared update" unless CounterProbe.current == 17 && CounterChild.current == 17
raise "compound operators changed" unless CounterChild.new.operators == 23
raise "operator storage split across inheritance" unless CounterProbe.current == 23
raise "false RHS was evaluated" unless CounterProbe.new.skip == false
raise "shared false storage lost" unless CounterChild.current == false
"#).assert_passes();
}

#[test]
fn defined_guards_and_source_literals_run_after_emission() {
    emit_and_run::real_blog()
        .write("app/services/guard_probe.rb", r#"class GuardProbe
  VALUE = 11
  def self.constants
    [defined?(GuardProbe), defined?(GuardProbe::VALUE), defined?(MissingPr197), defined?(GuardProbe::MissingPr197)]
  end
  def self.uninvoked
    raise "defined? invoked its terminal method"
  end
  def self.calls
    defined?(self.uninvoked)
  end
  def self.location
    [__FILE__, __LINE__]
  end
  def self.value
    11
  end
  def self.predicates
    [defined?(self.uninvoked.nil?), defined?(self.value.nil?), defined?(self.value.present?)]
  end
  def self.simple
    [defined?(self), defined?(nil), defined?(true), defined?(false), defined?(17), defined?(MissingOuterPr197::Inner)]
  end
  def classvars
    before = defined?(@@value)
    @@value = nil
    [before, defined?(@@value)]
  end
  def visible
    11
  end
  private
  def hidden
    raise "private query invoked method"
  end
end
class GuardChild < GuardProbe
  def self.calls
    defined?(super)
  end
  def queries
    [defined?(self.visible), defined?(self.hidden)]
  end
end
"#)
        .run_ruby(r#"
raise "constant guard changed" unless GuardProbe.constants == ["constant", "constant", nil, nil]
raise "method guard changed" unless GuardProbe.calls == "method"
raise "super guard changed" unless GuardChild.calls == "super"
raise "source identity changed" unless GuardProbe.location == [File.expand_path("app/services/guard_probe.rb", Dir.pwd), 13]
raise "predicate query was lowered or evaluated as a normal call" unless GuardProbe.predicates == [nil, "method", "method"]
raise "static descriptors became booleans" unless GuardProbe.simple == ["self", "nil", "true", "false", "expression", nil]
raise "nil class variable was confused with absence" unless GuardProbe.new.classvars == [nil, "class variable"]
raise "inherited or private method query changed" unless GuardChild.new.queries == ["method", nil]
"#).assert_passes();
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

#[test]
fn array_and_hash_checks_preserve_members() {
    emit_and_run::real_blog()
        .write("app/lib/container_narrowing_probe.rb", r##"class ContainerNarrowingProbe
  def self.array_members
    value = ["alpha", "beta"]
    if value.is_a?(Array)
      value.map { |item| item.upcase }
    else
      raise("not an Array")
    end
  end
  def self.hash_members
    value = {"answer" => 41}
    if value.is_a?(Hash)
      value.map { |key, item| "#{key.upcase}=#{item + 1}" }
    else
      raise("not a Hash")
    end
  end
  def self.nested_members
    value = [{"name" => "alpha"}, {"name" => "beta"}]
    if value.is_a?(Array)
      value.map do |item|
        if item.is_a?(Hash)
          item.fetch("name").upcase
        else
          raise("not a Hash")
        end
      end
    else
      raise("not an Array")
    end
  end
end
"##)
        .run_ruby(r#"
raise "Array members changed" unless ContainerNarrowingProbe.array_members == ["ALPHA", "BETA"]
raise "Hash members changed" unless ContainerNarrowingProbe.hash_members == ["ANSWER=42"]
raise "nested members changed" unless ContainerNarrowingProbe.nested_members == ["ALPHA", "BETA"]
"#)
        .assert_passes();
}

const ARTICLES_CONTROLLER: &str = "app/controllers/articles_controller.rb";
const TRACK_FILTER: &str = "  def track\n    \
                              @tracked = %w[index show].include?(action_name)\n    \
                              @action = action_name\n  \
                            end\n";
const TRACKED_CALLS: &str = "<p id=\"tracked\"><%= @tracked %> <%= @action %></p>\n";
const TRACKED_ASSERTION: &str = "    assert_match(/<p id=\"tracked\">true index<\\/p>/, response.body)\n";

/// A filter reads `action_name`. Rails gives the name of the action as a
/// String. The emitted filter body raised NameError, because only the
/// dispatcher had `action_name`, and the dispatcher gave a Symbol.
#[test]
fn a_before_action_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .edit(
                ARTICLES_CONTROLLER,
                "  before_action :set_article,",
                "  before_action :track\n  before_action :set_article,",
            )
            .edit(ARTICLES_CONTROLLER, "  private\n", &format!("  private\n{TRACK_FILTER}\n")),
        TRACKED_CALLS,
        TRACKED_ASSERTION,
    )
    .assert_passes();
}

/// The same filter from a concern that the controller includes.
#[test]
fn a_before_action_from_a_concern_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/concerns/tracks_action.rb",
                &format!(
                    "module TracksAction\n  \
                       extend ActiveSupport::Concern\n\n  \
                       included do\n    \
                         before_action :track\n  \
                       end\n\n  \
                       private\n\n{TRACK_FILTER}end\n"
                ),
            )
            .edit(
                ARTICLES_CONTROLLER,
                "class ArticlesController < ApplicationController\n",
                "class ArticlesController < ApplicationController\n  include TracksAction\n",
            ),
        TRACKED_CALLS,
        TRACKED_ASSERTION,
    )
    .assert_passes();
}

/// A private method reads `self.action_name`, and the action calls it.
#[test]
fn a_private_method_reads_self_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .edit(
                ARTICLES_CONTROLLER,
                "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
                "    @articles = Article.includes(:comments).order(created_at: :desc)\n    \
                   @label = label\n",
            )
            .edit(
                ARTICLES_CONTROLLER,
                "  private\n",
                "  private\n  def label\n    self.action_name\n  end\n\n",
            ),
        "<p id=\"label\"><%= @label %></p>\n",
        "    assert_match(/<p id=\"label\">index<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// A block filter and a lambda filter read `action_name`. Their bodies run
/// inside the dispatcher, so a bare `action_name` read the dispatcher's
/// Symbol, and `self.action_name` read `""`.
#[test]
fn block_and_lambda_filters_read_action_name() {
    on_the_index(
        emit_and_run::real_blog().edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  before_action { @bare = action_name }\n  \
               before_action -> { @own = self.action_name }\n  \
               before_action :set_article,",
        ),
        "<p id=\"names\"><%= @bare.inspect %> <%= @own.inspect %></p>\n",
        "    assert_match(/<p id=\"names\">&quot;index&quot; &quot;index&quot;<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// A block `after_action` reads `action_name`. It runs inside the
/// dispatcher too, so it read the dispatcher's Symbol.
#[test]
fn a_block_after_action_reads_action_name() {
    emit_and_run::real_blog()
        .edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  after_action only: :index do\n    \
                 raise \"after_action read #{action_name.inspect}\" unless action_name == \"index\"\n  \
               end\n  \
               before_action :set_article,",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A `rescue_from` block reads `action_name`. The dispatcher runs the
/// handler, so it read the dispatcher's Symbol.
#[test]
fn a_rescue_from_block_reads_action_name() {
    emit_and_run::real_blog()
        .edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  rescue_from ActiveRecord::RecordNotFound do\n    \
                 redirect_to articles_path, notice: \"missing #{action_name.inspect}\"\n  \
               end\n  \
               before_action :set_article,",
        )
        .edit(
            CONTROLLER_TEST,
            INDEX_ASSERTION,
            &format!(
                "{INDEX_ASSERTION}    get \"/articles/0\"\n    \
                   follow_redirect!\n    \
                   assert_match(/id=\"notice\">missing &quot;show&quot;</, response.body)\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// An inherited filter has an `if:` lambda that reads `action_name`. The
/// dispatcher runs the guard, so it compared the dispatcher's Symbol with
/// a String, and the filter never ran.
#[test]
fn an_inherited_filter_guard_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog().edit(
            "app/controllers/application_controller.rb",
            "  allow_browser versions: :modern\n",
            "  allow_browser versions: :modern\n  \
               before_action :mark, if: -> { action_name == \"index\" }\n\n  \
               private\n\n  \
               def mark\n    \
                 @marked = true\n  \
               end\n",
        ),
        "<p id=\"marked\"><%= @marked.inspect %></p>\n",
        "    assert_match(/<p id=\"marked\">true<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// An inherited filter and an inlined filter read `action_name`, for the
/// target that runs the dispatcher natively.
fn action_name_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  \
               before_action :track\n\n  \
               private\n\n  \
               def track\n    \
                 @tracked = %w[index show].include?(action_name)\n  \
               end\n\
             end\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  \
               before_action :name_it\n\n  \
               def index\n    \
                 render plain: \"#{@tracked} #{@name}\"\n  \
               end\n\n  \
               private\n\n  \
               def name_it\n    \
                 @name = action_name\n  \
               end\n\
             end\n",
        )
}

const ACTION_NAME_ASSERTIONS: &str = r#"
require_relative "app/controllers/widgets_controller"
controller = WidgetsController.new
controller.process_action(:index)
raise "action_name: #{controller.body}" unless controller.body == "true index"
puts "action_name passed"
"#;

#[test]
fn filters_read_action_name_as_a_string() {
    action_name_app().run_ruby(ACTION_NAME_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn filters_read_action_name_as_a_string_on_spinel() {
    action_name_app().run_spinel(ACTION_NAME_ASSERTIONS).assert_passes();
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

/// Campfire's `TimeLimitedVideoPreviewer` subclasses Rails'
/// `ActiveStorage::Previewer::VideoPreviewer`. Before the nested class
/// existed in the runtime, `app/models.rb` raised
/// `uninitialized constant ActiveStorage::Previewer::VideoPreviewer`
/// at boot. The capture body (Timeout / IO.popen) is a separate ledger
/// entry; this pins the constant that inheritance needs to load.
#[test]
fn time_limited_video_previewer_subclass_boots() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/time_limited_video_previewer.rb",
            r#"# Campfire's inheritance shape (capture body omitted — Timeout/IO are
# not yet modeled). The NameError at boot was the missing superclass.
class TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
  TIME_LIMIT = 10
end
"#,
        )
        .write(
            "config/initializers/extensions.rb",
            r##"%w[ rails_ext ].each do |extensions_dir|
  Dir["#{Rails.root}/lib/#{extensions_dir}/*"].each { |path| require "#{extensions_dir}/#{File.basename(path)}" }
end
"##,
        )
        .run_ruby(
            r#"raise "missing nested class" unless defined?(ActiveStorage::Previewer::VideoPreviewer)
raise "subclass missing" unless defined?(TimeLimitedVideoPreviewer)
raise "wrong parent" unless TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
raise "wrong grandparent" unless TimeLimitedVideoPreviewer < ActiveStorage::Previewer
raise "TIME_LIMIT" unless TimeLimitedVideoPreviewer::TIME_LIMIT == 10
raise "PreviewError missing" unless defined?(ActiveStorage::PreviewError)
raise "Error base missing" unless defined?(ActiveStorage::Error)
raise "PreviewError parent" unless ActiveStorage::PreviewError < ActiveStorage::Error
raise "Error parent" unless ActiveStorage::Error < StandardError
puts "time_limited_video_previewer boot ok"
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

/// A Stimulus `data:` hash has `controller:` and `action:` keys, as
/// `url_for` options do. Before, the lowerer read it as `url_for`
/// options and wrote a route helper named after its keys. A dashed key
/// made that name a syntax error, and the app did not load.
#[test]
fn a_stimulus_data_hash_on_link_to_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= link_to "Open", articles_path, data: { controller: "menu", action: "menu#open", "menu-id-value": 1 } %>"#,
        &[r#"href="/articles""#, ">Open</a>"],
    );
}

/// `form_with` takes the `data:` hash for its `<form>` in `html:`, one
/// level deeper.
#[test]
fn a_stimulus_data_hash_in_form_with_html_options_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= form_with url: articles_path, html: { data: { controller: "menu", action: "menu#open", "menu-id-value": 1 } } do |f| %><%= f.submit "Go" %><% end %>"#,
        &[r#"action="/articles""#, r#"value="Go""#],
    );
}

/// A `data:` hash that a local holds is not a literal at the call.
#[test]
fn a_stimulus_data_hash_in_a_local_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<% d = { controller: "menu", action: "menu#open", "menu-id-value": 1 } %><%= link_to "Open", articles_path, data: d %>"#,
        &[r#"href="/articles""#, ">Open</a>"],
    );
}

/// A `data:` hash with a `.merge` on it is a call, not a Hash literal.
#[test]
fn a_merged_stimulus_data_hash_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= link_to "Open", articles_path, data: { controller: "menu", action: "menu#open", "menu-id-value": 1 }.merge(turbo: false) %>"#,
        &[r#"href="/articles""#, r#"data-turbo="false""#, ">Open</a>"],
    );
}

/// Renders `erb` as a partial, and expects the Stimulus attributes and
/// each of `parts` in the HTML.
fn a_stimulus_data_hash_renders(erb: &str, parts: &[&str]) {
    let stimulus = [r#"data-controller="menu""#, r#"data-action="menu#open""#, r#"data-menu-id-value="1""#];
    let expected: Vec<String> = stimulus
        .iter()
        .chain(parts)
        .map(|p| format!("{p:?}"))
        .collect();
    emit_and_run::real_blog()
        .write("app/views/articles/_menu.html.erb", erb)
        .run_ruby(&format!(
            r#"html = Views::Articles.menu(nil)
[{}].each do |part|
  raise "missing #{{part}}: #{{html}}" unless html.include?(part)
end
puts "ok"
"#,
            expected.join(", ")
        ))
        .assert_passes();
}

/// `t.integer …, limit: 8` is a `bigint` now (the width Rails creates),
/// where it was an `integer`. On SQLite both are INTEGER and both type
/// as `Integer`, so the emitted program must keep a value past 32 bits
/// through a save and a reload, as it did before.
#[test]
fn an_eight_byte_integer_column_keeps_a_value_past_32_bits() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"views\", limit: 8, default: 0, null: false",
        )
        .write(
            "test/models/article_views_test.rb",
            r#"require "test_helper"

class ArticleViewsTest < ActiveSupport::TestCase
  test "a value past 32 bits survives a reload" do
    article = Article.create!(title: "Popular", body: "A long enough body", views: 5_000_000_000)
    assert_equal 5_000_000_000, Article.find(article.id).views
  end
end
"#,
        )
        .run_test("test/models/article_views_test.rb")
        .assert_passes();
}

#[test]
fn literal_data_factories_and_aliases_check_cleanly_and_execute() {
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::ty::Ty;

    let run = emit_and_run::real_blog()
        .write("app/lib/factory_examples.rb", data_factory::DECLARATIONS)
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @declared = FactoryExamples::First::Result.new("first", 1.0, false)
    @alias = FactoryExamples::First::ChainedAlias.new(name: nil, score: 0.0, enabled: true)
    @second = FactoryExamples::Second::Result.new(name: "second")
    @empty = FactoryExamples::Empty::Result.new
  end
end
"#)
        .run_ruby(r#"
first = FactoryExamples::First.build
aliased = FactoryExamples::First.aliased
qualified = FactoryExamples::First.qualified
second = FactoryExamples::Second.build
empty = FactoryExamples::Empty.build
raise "wrong declared class" unless first.class == FactoryExamples::First::Result
raise "alias created another class" unless aliased.class == first.class && FactoryExamples::First::Alias == first.class && FactoryExamples::First::ChainedAlias == first.class
raise "member values changed" unless first.name == "first" && first.score == 0.8 && first.enabled == false
raise "nil/zero/true changed" unless aliased.name.nil? && aliased.score == 0.0 && aliased.enabled == true
raise "positional constructor changed" unless qualified.name == "qualified" && qualified.score == 1.0 && qualified.enabled == false
raise "same-named factory crossed owners" unless second.class == FactoryExamples::Second::Result && second.name == "second" && second.class != first.class
raise "empty factory changed" unless empty.class == FactoryExamples::Empty::Result && empty.members.empty?
raise "Data lost immutability" unless first.frozen? && !first.respond_to?(:name=)
puts "Data factory identity, aliases, constructors, values and immutability passed"
"#);
    run.assert_passes();
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/factory_examples/first.rb")).unwrap();
    assert!(emitted.contains("Data.define"), "{emitted}");
    for (owner, stem, readers) in [
        ("FactoryExamples::First", "first", vec!["name", "score", "enabled"]),
        ("FactoryExamples::Second", "second", vec!["name"]),
        ("FactoryExamples::Empty", "empty", vec![]),
    ] {
        let sidecar = std::fs::read_to_string(run.emitted.join(
            format!("sig/app/models/factory_examples/{stem}.rbs")
        )).unwrap();
        assert!(sidecar.contains("class Result < ::Data"), "{sidecar}");
        assert!(!sidecar.contains("class Alias") && !sidecar.contains("class ChainedAlias"), "{sidecar}");
        let signatures = roundhouse::rbs::parse_app_signatures(&sidecar).expect("emitted RBS parses");
        let id = ClassId(Symbol::from(format!("{owner}::Result")));
        let factory = signatures.get(&id).expect("factory return type is declared");
        assert!(factory.contains_key(&Symbol::from("new")), "{sidecar}");
        for reader in readers {
            let signature = &factory[&Symbol::from(reader)];
            assert!(matches!(signature, Ty::Fn { ret, .. } if **ret == Ty::Untyped), "{sidecar}");
            assert!(!factory.contains_key(&Symbol::from(format!("{reader}="))), "{sidecar}");
        }
        let owner_methods = &signatures[&ClassId(Symbol::from(owner))];
        let Ty::Fn { ret, .. } = &owner_methods[&Symbol::from("build")] else {
            panic!("build has no function signature: {sidecar}");
        };
        assert_eq!(ret.as_ref(), &Ty::Class { id, args: vec![] }, "{sidecar}");
    }
}

/// Execute emitted Ruby to check keyword/positional construction, `with`, method
/// visibility, aliases, and lexical constants. A preceding `defined?` also catches
/// factory declarations hoisted ahead of their original source position.
#[test]
fn custom_data_factories_preserve_methods_initializers_and_scope() {
    let run = emit_and_run::real_blog()
        .write("app/lib/factory_examples.rb", data_factory::CUSTOM_DECLARATIONS)
        .edit("app/lib/factory_examples.rb", "    PREFIX = \"order\"", "    PREFIX = \"order\"\n    BEFORE = defined?(State)")
        .run_ruby(&format!("{}\nraise 'declaration was hoisted' unless FactoryExamples::Stateful::BEFORE.nil?", data_factory::CUSTOM_ASSERTIONS));
    run.assert_passes();
    assert!(run.stdout.contains("custom Data factory contract passed"));
}

/// A file in `app/models/<model>/` often reopens the model only to
/// hold a nested class. That reopen is a namespace, so the model keeps
/// its own file. Before, the reopen became a library class whose file
/// was the model's file, so the emit wrote the nested class over the
/// model, and `Article.find` raised `NoMethodError`.
#[test]
fn a_model_reopened_to_hold_a_nested_class_keeps_its_model() {
    a_reopen_at_keeps_the_model("app/models/article/summary.rb");
}

/// The same reopen outside `app/models`. The ingest reads these
/// folders later, and the reopen must not write over the model there
/// either.
#[test]
fn a_model_reopened_in_app_services_keeps_its_model() {
    a_reopen_at_keeps_the_model("app/services/article/summary.rb");
}

#[test]
fn a_model_reopened_in_lib_keeps_its_model() {
    a_reopen_at_keeps_the_model("lib/article/summary.rb");
}

fn a_reopen_at_keeps_the_model(path: &str) {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  DRAFT = \"draft\".freeze\n\n  def summary\n    Summary.new(self)\n  end\n",
        )
        .write(
            path,
            r##"class Article
  class Summary
    def initialize(article)
      @article = article
    end

    def text
      "#{@article.title} (#{Article::DRAFT})"
    end
  end
end
"##,
        )
        .write(
            "test/models/article_summary_test.rb",
            r##"require "test_helper"

class ArticleSummaryTest < ActiveSupport::TestCase
  test "the model and its nested class both load" do
    article = Article.find(articles(:one).id)
    assert_equal "#{article.title} (draft)", article.summary.text
    assert Article < ApplicationRecord
  end
end
"##,
        )
        .run_test("test/models/article_summary_test.rb")
        .assert_passes();
}

/// lobsters keeps its site settings in a top-level
/// `class << Rails.application` block. Rubydex records no constant
/// references inside a singleton class opened on an expression, so the
/// `ENV` and `Rails` written there have no source answer; they must still
/// resolve as the modeled constants, not emit as refusal stubs.
#[test]
fn rails_application_singleton_settings_read_env_and_rails() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            r#"module TestApp
  class Application < Rails::Application
  end
end

class << Rails.application
  def open_signups?
    ENV["OPEN_SIGNUPS"] == "true"
  end

  def domain
    "example.test"
  end

  def root_url
    "https://#{Rails.application.domain}/"
  end
end
"#,
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"ENV["OPEN_SIGNUPS"] = "true"
raise "ENV read: open_signups? must be true" unless Rails.application.open_signups?
ENV["OPEN_SIGNUPS"] = "false"
raise "ENV read: open_signups? must be false" if Rails.application.open_signups?
url = Rails.application.root_url
raise "Rails.application chain: #{url.inspect}" unless url == "https://example.test/"
"#,
        );
    run.assert_passes();
}

/// `if:` / `unless:` guards a callback. Ingest used to reject the
/// declaration outright, so the callback was silently dropped and ran in
/// no circumstance. A zero-arity lambda body (`if: -> { color.blank? }`)
/// is spliced as the guard; a Symbol (`unless: :loud?`) is the predicate
/// call, negated. This runs the emitted program to prove the callback
/// fires exactly when Rails would.
#[test]
fn a_conditional_callback_runs_only_when_its_condition_holds() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "module TestApp\n  class Application < Rails::Application\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  before_validation :assign_color, on: :create, if: -> { color.blank? }
  before_save :shout, unless: :loud?

  private
    def assign_color
      self.color = "default"
    end

    def loud?
      self.name == "LOUD"
    end

    def shout
      self.name = self.name.to_s.upcase
    end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"color\"\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"quiet = Widget.create!(name: "quiet")
raise "if: true must run the callback: #{quiet.color.inspect}" unless quiet.color == "default"
raise "unless: true (not loud?) must run the callback: #{quiet.name.inspect}" unless quiet.name == "QUIET"
loud = Widget.create!(name: "LOUD", color: "red")
raise "if: false must skip the callback: #{loud.color.inspect}" unless loud.color == "red"
raise "unless: false (loud?) must skip the callback: #{loud.name.inspect}" unless loud.name == "LOUD"
"#,
        );
    run.assert_passes();
}

/// Both `if:` and `unless:` on one callback: Rails runs it only when the
/// `if:` holds AND the `unless:` does not, so the two guards compose as
/// `if` and `!unless`. The emitted program must honour all four
/// combinations.
#[test]
fn a_callback_with_both_if_and_unless_requires_both() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "module TestApp\n  class Application < Rails::Application\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  before_save :stamp, if: :ready?, unless: :blocked?

  def ready?
    self.color.present?
  end

  def blocked?
    self.name == "blocked"
  end

  def stamp
    self.name = "STAMPED"
  end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"color\"\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"not_ready = Widget.create!(name: "x")
raise "if: false must skip: #{not_ready.name.inspect}" unless not_ready.name == "x"
blocked = Widget.create!(color: "red", name: "blocked")
raise "unless: true must skip: #{blocked.name.inspect}" unless blocked.name == "blocked"
runs = Widget.create!(color: "red", name: "x")
raise "if: true and unless: false must run: #{runs.name.inspect}" unless runs.name == "STAMPED"
"#,
        );
    run.assert_passes();
}
/// A predicate the app defines on `String` is that method, not an
/// inquirer comparison against its own name.
#[test]
fn a_string_predicate_the_app_defines_is_not_folded_as_an_inquiry() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/string.rb",
            "class String\n  def shout?\n    self == upcase\n  end\n\n  def self.special?\n    true\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def shouting?\n    title.to_s.shout?\n  end\n\n  def shouting_inquirer?\n    title.to_s.inquiry.shout?\n  end\n\n  def class_side_inquirer?\n    title.to_s.inquiry.special?\n  end",
        )
        .run_ruby(
            r#"
raise "folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting?
raise "answers true for everything" if Article.new(title: "quiet", body: "b").shouting?
raise "inquirer folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting_inquirer?
raise "inquirer answers true for everything" if Article.new(title: "quiet", body: "b").shouting_inquirer?
raise "class-side predicate blocked the fold" unless Article.new(title: "special", body: "b").class_side_inquirer?
raise "class-side fold answers true for everything" if Article.new(title: "quiet", body: "b").class_side_inquirer?
"#,
        )
        .assert_passes();
}

/// The same through a module the app includes into `String`.
#[test]
fn a_string_predicate_from_an_included_module_is_not_folded_as_an_inquiry() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/string.rb",
            "module Shouting\n  def shout?\n    self == upcase\n  end\nend\n\nclass String\n  include Shouting\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def shouting?\n    title.to_s.shout?\n  end",
        )
        .run_ruby(
            r#"
raise "folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting?
raise "answers true for everything" if Article.new(title: "quiet", body: "b").shouting?
"#,
        )
        .assert_passes();
}

/// `includes(:comments)` distributes each parent's children by binary
/// search over the children's foreign keys, sorted by the preload query
/// (`ActiveRecord.lower_bound`), where it used to scan every child per
/// parent — O(N * M), a million comparisons at 1,000 x 1,000, which put
/// the emitted index behind Rails' keyed preloader
/// (koduki/example-rails-aot). The comment-byte gates are blind to
/// grouping, so this renders each article's preloaded comments by body:
/// inserts interleaved across articles, a parent with no children, and
/// a run at the end of the sorted list all have to land, in insertion
/// order within each article.
#[test]
fn includes_distributes_each_parents_children_in_order() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_article.html.erb",
            "(<%= pluralize(article.comments.size, \"comment\") %>)",
            "(<%= pluralize(article.comments.size, \"comment\") %>)<i class=\"pc\"><%= article.title %>=<%= article.comments.map(&:body).join(\",\") %></i>",
        )
        .write(
            "test/controllers/articles_preload_controller_test.rb",
            r#"require "test_helper"

class ArticlesPreloadControllerTest < ActionDispatch::IntegrationTest
  test "includes distributes each article's comments" do
    a = Article.create!(title: "Alpha", body: "A sufficiently long body for validation.")
    b = Article.create!(title: "Beta", body: "A sufficiently long body for validation.")
    c = Article.create!(title: "Gamma", body: "A sufficiently long body for validation.")
    Article.create!(title: "Delta", body: "A sufficiently long body for validation.")
    Comment.create!(article_id: c.id, commenter: "x", body: "c1")
    Comment.create!(article_id: a.id, commenter: "x", body: "a1")
    Comment.create!(article_id: c.id, commenter: "x", body: "c2")
    Comment.create!(article_id: b.id, commenter: "x", body: "b1")
    Comment.create!(article_id: a.id, commenter: "x", body: "a2")
    Comment.create!(article_id: c.id, commenter: "x", body: "c3")
    get articles_url
    assert_response :success
    assert_match(/<i class="pc">Alpha=a1,a2<\/i>/, response.body)
    assert_match(/<i class="pc">Beta=b1<\/i>/, response.body)
    assert_match(/<i class="pc">Gamma=c1,c2,c3<\/i>/, response.body)
    assert_match(/<i class="pc">Delta=<\/i>/, response.body)
  end
end
"#,
        )
        .run_test("test/controllers/articles_preload_controller_test.rb")
        .assert_passes();
}

/// A rooted `class_name:` names the top-level class (chatwoot's
/// `has_many :portals, class_name: "::Portal"`).
#[test]
fn a_rooted_class_name_association_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  has_many :rooted_comments, class_name: \"::Comment\"\n\n  def first_rooted_body\n    rooted_comments.first.body\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Rooted", body: "Body text here")
Comment.create!(article_id: article.id, commenter: "Ann", body: "First remark")
raise "rooted association count" unless article.rooted_comments.count == 1
raise "rooted association read" unless article.first_rooted_body == "First remark"
"#,
        )
        .assert_passes();
}

/// Interface keys belong to `as:`, even when the Concern name matches it.
#[test]
fn a_polymorphic_inverse_from_a_concern_runs() {
    assert_polymorphic_inverse_from_a_concern_runs("Notifiable");
}

/// A different Concern name must preserve the same id/type interface.
#[test]
fn a_polymorphic_inverse_from_a_differently_named_concern_runs() {
    assert_polymorphic_inverse_from_a_concern_runs("NotificationOwner");
}

fn assert_polymorphic_inverse_from_a_concern_runs(concern: &str) {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\", force: :cascade do |t|",
            "  create_table \"notifications\", force: :cascade do |t|\n    t.integer \"notifiable_id\"\n    t.string \"notifiable_type\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|",
        )
        .write(
            "app/models/notification.rb",
            "class Notification < ApplicationRecord\n  belongs_to :notifiable, polymorphic: true\n  def owner_title\n    notifiable.title\n  end\nend\n",
        )
        .write(
            "app/models/concerns/notifiable.rb",
            &format!("module {concern}\n  extend ActiveSupport::Concern\n  included do\n    has_many :notifications, as: :notifiable\n    has_many :explicit_notifications, class_name: \"Notification\", as: :notifiable, foreign_key: :notifiable_id\n    has_one :first_notification, class_name: \"Notification\", as: :notifiable\n    has_one :last_notification, class_name: \"Notification\", as: :notifiable, foreign_key: :notifiable_id\n  end\nend\n"),
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n  include {concern}\n"),
        )
        .run_ruby(
            r#"article = Article.create!(title: "Owner", body: "Body text here")
note = Notification.create!(notifiable_id: article.id, notifiable_type: "Article")
reloaded = Notification.find(note.id)
raise "owner id changed" unless reloaded.notifiable_id == article.id
raise "owner type changed" unless reloaded.notifiable_type == "Article"
raise "polymorphic read" unless reloaded.owner_title == "Owner"
raise "inverse read" unless article.notifications.count == 1
raise "explicit inverse read" unless article.explicit_notifications.count == 1
raise "default singular inverse read" unless article.first_notification.owner_title == "Owner"
raise "explicit singular inverse read" unless article.last_notification.owner_title == "Owner"
"#,
        )
        .assert_passes();
}

/// An explicit `foreign_key:` is the key, even when its name matches
/// the Concern-derived default. Only a defaulted key is rehomed.
#[test]
fn an_explicit_key_named_like_its_concern_is_kept() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\", force: :cascade do |t|",
            "  create_table \"remarks\", force: :cascade do |t|\n    t.integer \"remarkable_id\"\n    t.string \"body\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|",
        )
        .write(
            "app/models/remark.rb",
            "class Remark < ApplicationRecord\n  belongs_to :article, foreign_key: :remarkable_id\nend\n",
        )
        .write(
            "app/models/concerns/remarkable.rb",
            "module Remarkable\n  extend ActiveSupport::Concern\n  included do\n    has_many :remarks, foreign_key: :remarkable_id\n    has_one :first_remark, class_name: \"Remark\", foreign_key: :remarkable_id\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Remarkable\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Owner", body: "Body text here")
Remark.create!(remarkable_id: article.id, body: "hi")
raise "inverse read" unless article.remarks.count == 1
raise "singular inverse read" unless article.first_remark.body == "hi"
"#,
        )
        .assert_passes();
}

/// Not `super: no superclass method 'password='`: a model's own password writer that calls `super` reaches `has_secure_password`'s writer, as the macro's module method does in Rails.
#[test]
fn a_password_writer_that_calls_super_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"password_digest\"\n    t.string \"recovery_password_digest\"",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|",
            "create_table \"comments\", force: :cascade do |t|\n    t.string \"password_digest\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  has_secure_password
  has_secure_password :recovery_password

  def password=(value)
    @password_supplied = true
    super(value == "" ? nil : value)
  end

  def password_supplied?
    @password_supplied == true
  end

  def seed_plaintext(value)
    @password = value
    @recovery_password = value
  end

  def recovery_password=(value)
    @recovery_password_supplied = true
    super
  end

  def recovery_password_supplied?
    @recovery_password_supplied == true
  end

  def recovery_plaintext
    @recovery_password
  end"#,
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord",
            r#"class Comment < ApplicationRecord
  has_secure_password

  def password=(value)
    @password_supplied = true
    super
  end

  def password_supplied?
    @password_supplied == true
  end

  def seed_plaintext(value)
    @password = value
  end"#,
        )
        .write(
            "test/models/article_password_writer_test.rb",
            r#"require "test_helper"

class ArticlePasswordWriterTest < ActiveSupport::TestCase
  # Each assignment ends in nil, which the macro's writer stores
  # without reaching bcrypt (CI's unit job does not install it).
  test "super(x) passes x to the macro's writer" do
    article = articles(:one)
    article.seed_plaintext("seeded")
    assert !article.password_supplied?
    article.password = ""
    assert article.password_supplied?
    assert_nil article.password
  end

  test "a bare super passes the writer's own argument" do
    comment = comments(:one)
    comment.seed_plaintext("seeded")
    assert !comment.password_supplied?
    comment.password = nil
    assert comment.password_supplied?
    assert_nil comment.password
  end

  test "each secure-password attribute reaches its own macro writer" do
    article = articles(:one)
    article.seed_plaintext("seeded")
    article.password = ""
    assert article.password_supplied?
    assert_nil article.password
    assert_equal "seeded", article.recovery_plaintext
    assert !article.recovery_password_supplied?
    article.recovery_password = nil
    assert article.recovery_password_supplied?
    assert_nil article.recovery_plaintext
    assert_nil article.password
  end
end
"#,
        )
        .run_test("test/models/article_password_writer_test.rb")
        .assert_passes();
}

#[path = "emit_and_run/relation_finders.rs"]
mod relation_finders;
#[path = "emit_and_run/attach_hash.rs"]
mod attach_hash;
#[path = "emit_and_run/many_attached.rs"]
mod many_attached;

/// A controller under `ActionController::API`, the base `rails new
/// --api` writes, dispatches (#163). The runtime defined only `Base`,
/// so the ruby tree raised NameError loading ApplicationController, and
/// every request to the spinel binary answered 500 (`undefined method
/// 'params='`). The `before_action` reads `params[:id]`, which is the
/// writer the spinel dispatcher failed on.
#[test]
fn an_action_controller_api_controller_dispatches() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
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
            "Rails.application.routes.draw do\n  root \"widgets#index\"\n  resources :widgets, only: %i[index show]\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"widget = Widget.create!(name: "a")
def get(path)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
{ "/widgets" => 204, "/widgets/#{widget.id}" => 204, "/widgets/#{widget.id + 1}" => 404 }.each do |path, want|
  got = get(path)
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

#[path = "emit_and_run/string_bytes.rs"]
mod string_bytes;

/// A Slim view is ingested rather than skipped, so `check` going quiet on
/// it is a claim the emitted page renders. Swap the blog's index for a
/// Slim twin that exercises the grammar (shortcuts merging with a
/// `class=`, Ruby and boolean attributes, `tag: child` nesting, output
/// and code lines with a block, `|` text, comments) and assert on the
/// rendered markup, not just the status.
#[test]
fn a_slim_view_renders() {
    let slim = r#"= turbo_stream_from "articles"
- content_for :title, "Articles"
- turbo_exempts_page_from_cache

/ never rendered
.w-full
  - if notice.present?
    p.py-2#notice = notice
  .flex.justify-between
    h1.font-bold.text-4xl Articles
    = link_to "New article", new_article_path, class: "rounded-md"
  #articles.min-w-full class="space-y-5" data-count=@articles.size
    - if @articles.any?
      = render @articles
    - else
      p.text-center No articles found.
  ul.slim-list(data-kind="list" hidden)
    - @articles.each do |article|
      li: a href=article_path(article) = article.title
  p.slim-text
    | plain text
"#;
    emit_and_run::real_blog()
        .remove("app/views/articles/index.html.erb")
        .write("app/views/articles/index.html.slim", slim)
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1.font-bold\", \"Articles\"\n    \
             assert_select \"#articles.min-w-full.space-y-5[data-count]\"\n    \
             assert_select \"ul.slim-list[data-kind=list][hidden] li a\", minimum: 1\n    \
             assert_select \"p.slim-text\", \"plain text\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A view directory with a hyphen (`product-item/`) is legal in Rails and
/// common in real apps, but `Views::Product-item` and a `product-item`
/// parameter are not Ruby. The emitted partial must load and render.
#[test]
fn a_hyphenated_view_directory_renders() {
    emit_and_run::real_blog()
        .write(
            "app/views/note-card/_note.html.erb",
            "<aside class=\"note-card\"><%= note %></aside>\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<div class=\"w-full\">\n",
            "<div class=\"w-full\">\n  <%= render \"note-card/note\", note: \"hyphen ok\" %>\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1\", \"Articles\"\n    assert_select \"aside.note-card\", \"hyphen ok\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Rails' own guards against a request-steered header, ahead of the
/// server's (which drops any header holding a control character):
/// `redirect_to` deletes CR and LF from the location
/// (`_compute_redirect_to_location`, actionpack 8.1), and Active
/// Storage serves only `inline` or `attachment`, whatever disposition a
/// URL asks for (`content_disposition_with`, activestorage 8.1) — the
/// blob redirect route takes it from a query param and signs it into
/// the disk URL whose Content-Disposition it becomes.
fn header_values_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
  end
  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
  end
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  has_one_attached :file\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/bounce\", to: \"docs#bounce\"\nend\n")
        .write("app/controllers/docs_controller.rb", r#"class DocsController < ApplicationController
  def bounce
    redirect_to params[:back]
  end
end
"#)
}

#[test]
fn request_steered_header_values_stay_one_line() {
    header_values_app()
        .run_ruby(r#"
require_relative "app/controllers/docs_controller"
controller = DocsController.new
controller.params = { "back" => "/next\r\nSet-Cookie: pwned=1" }
controller.process_action(:bounce)
location = controller.location.to_s
raise "CR/LF reached the Location: #{location.inspect}" if location.include?("\r") || location.include?("\n")
raise "the rest of the location is kept, as Rails keeps it: #{location.inspect}" unless location == "/nextSet-Cookie: pwned=1"

asked = ActiveStorage::DiskKey.decode(ActiveStorage::DiskKey.encode("k", "attachment\r\nSet-Cookie: pwned=1"))
raise "an unknown disposition was signed as asked: #{asked.inspect}" unless asked == ["k", "inline"]
kept = ActiveStorage::DiskKey.decode(ActiveStorage::DiskKey.encode("k", "attachment"))
raise "attachment is a disposition: #{kept.inspect}" unless kept == ["k", "attachment"]
puts "header values passed"
"#)
        .assert_passes();
}

fn query_value_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  protect_from_forgery with: :exception\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/limited", to: "widgets#limited"
  get "/paged", to: "widgets#paged"
  get "/sorted", to: "widgets#sorted"
  get "/sorted_by", to: "widgets#sorted_by"
  get "/sorted_str", to: "widgets#sorted_str"
  get "/bounce", to: "widgets#bounce"
  post "/touch", to: "widgets#touch"
  get "/headed", to: "widgets#headed"
  get "/tails", to: "widgets#tails"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def limited
    render plain: Widget.order(:name).limit(params[:n]).map { |w| w.name }.join(",")
  end

  def paged
    render plain: Widget.order(:name).offset(params[:skip]).map { |w| w.name }.join(",")
  end

  def sorted
    render plain: Widget.order(name: params[:dir]).map { |w| w.name }.join(",")
  end

  def sorted_by
    render plain: Widget.order(params[:sort] => :asc).map { |w| w.name }.join(",")
  end

  def sorted_str
    render plain: Widget.order(params[:sort]).map { |w| w.name }.join(",")
  end

  def bounce
    redirect_to params[:back]
  end

  def touch
    render plain: "ok"
  end

  def headed
    head :created, location: params[:back]
  end

  def tails
    render plain: Widget.order(:name).first(1).map { |w| w.name }.join(",") + Widget.order(:name).last(1).map { |w| w.name }.join(",")
  end
end
"#)
}

fn query_value_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
Widget.create!(name: "beta")
Widget.create!(name: "alpha")
Widget.create!(name: "gamma")

def run(action, params)
  controller = WidgetsController.new
  controller.request_method = "GET"
  controller.params = params
  controller.process_action(action)
  controller.body
end

def rejected(action, params)
  "ran: " + run(action, params)
rescue ArgumentError
  "rejected"
end

got = run(:limited, { "n" => "2" })
raise "a numeric String limit is Rails' Integer(): #{got}" unless got == "alpha,beta"
got = rejected(:limited, { "n" => "(SELECT COUNT(*) FROM widgets)" })
raise "LIMIT took SQL: #{got}" unless got == "rejected"

got = run(:paged, { "skip" => "1" })
raise "a numeric String offset is Rails' to_i: #{got}" unless got == "beta,gamma"
got = run(:paged, { "skip" => "(SELECT 2)" })
raise "OFFSET took SQL: #{got}" unless got == "alpha,beta,gamma"
got = run(:paged, { "skip" => "1; SELECT 1" })
raise "OFFSET to_i prefix: #{got}" unless got == "beta,gamma"

got = run(:sorted, { "dir" => "desc" })
raise "a String direction: #{got}" unless got == "gamma,beta,alpha"
got = rejected(:sorted, { "dir" => "asc, (SELECT 1)" })
raise "ORDER direction took SQL: #{got}" unless got == "rejected"

got = run(:sorted_by, { "sort" => "name" })
raise "a String column key: #{got}" unless got == "alpha,beta,gamma"
got = rejected(:sorted_by, { "sort" => "(SELECT 1)" })
raise "ORDER column took SQL: #{got}" unless got == "rejected"
got = run(:sorted_by, { "sort" => "widgets.name" })
raise "table.col hash key: #{got}" unless got == "alpha,beta,gamma"

got = run(:sorted_str, { "sort" => "name desc" })
raise "string order: #{got}" unless got == "gamma,beta,alpha"
got = rejected(:sorted_str, { "sort" => "id DESC, (SELECT 1)" })
raise "string ORDER took SQL: #{got}" unless got == "rejected"
got = Widget.all.order("LOWER(name)").map { |w| w.name }.join(",")
raise "LOWER(name) order: #{got}" unless got == "alpha,beta,gamma"
got = Widget.all.order("RANDOM()").map { |w| w.name }.length
raise "RANDOM() order rejected" unless got == 3
got = rejected(:sorted_str, { "sort" => "SLEEP()" })
raise "SLEEP() order: #{got}" unless got == "rejected"
got = rejected(:sorted_str, { "sort" => "LOWER(name); SELECT 1" })
raise "LOWER plus splice: #{got}" unless got == "rejected"

rel = Widget.all.order(:name)
begin
  rel.last_n("(SELECT 1)")
  raise "last_n accepted SQL"
rescue ArgumentError
  got = rel.order(:name).map { |w| w.name }.join(",")
  raise "last_n mutated orders: #{got}" unless got == "alpha,beta,gamma"
end
got = Widget.all.order(:name).first_n("2").map { |w| w.name }.join(",")
raise "first_n string: #{got}" unless got == "alpha,beta"
got = Widget.all.order(:name).limit(2.9).map { |w| w.name }.join(",")
raise "float limit truncate: #{got}" unless got == "alpha,beta"

puts "query values passed"
"#
}

#[test]
fn query_params_are_values_not_sql() {
    query_value_app()
        .run_ruby(query_value_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn query_params_are_values_not_sql_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        query_value_assertions()
    );
    query_value_app().run_spinel(&script).assert_passes();
}

#[test]
fn request_steered_head_and_headers_stay_one_line() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
controller = WidgetsController.new
controller.params = { "back" => "/next\r\nSet-Cookie: pwned=1" }
controller.process_action(:headed)
location = controller.location.to_s
raise "head location kept CR/LF: #{location.inspect}" if location.include?("\r") || location.include?("\n")

controller = WidgetsController.new
controller.headers["X-Link"] = "a\r\nSet-Cookie: pwned=1"
raise "CR/LF header was stored" unless controller.headers["X-Link"].nil?

controller.headers["X-Ok"] = "one-line"
raise "legal header dropped" unless controller.headers["X-Ok"] == "one-line"

controller.headers["X-Rev"] = nil
raise "nil header write stored a value" unless controller.headers["X-Rev"].nil?
raise "nil header wiped a sibling" unless controller.headers["X-Ok"] == "one-line"
puts "head and headers passed"
"#)
        .assert_passes();
}

#[test]
fn redirect_to_rejects_an_unvalidated_host() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "http://evil.example/" }
begin
  controller.process_action(:bounce)
  raise "open redirect ran: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/home" }
controller.process_action(:bounce)
raise "relative redirect lost: #{controller.location.inspect}" unless controller.location == "/home"

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "http://app.example/ok" }
controller.process_action(:bounce)
raise "same-host absolute refused: #{controller.location.inspect}" unless controller.location == "http://app.example/ok"

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "evil.example")
controller.request_method = "GET"
controller.session[:return_to_after_authenticating] = controller.request.url
controller.params = { "back" => controller.session[:return_to_after_authenticating] }
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
begin
  controller.process_action(:bounce)
  raise "spoofed request.url honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/\\evil.example" }
begin
  controller.process_action(:bounce)
  raise "backslash host honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "///evil.example" }
begin
  controller.process_action(:bounce)
  raise "triple-slash honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/\t/evil.example" }
begin
  controller.process_action(:bounce)
  raise "tab host honored: #{controller.location.inspect}"
rescue ArgumentError
end
puts "open redirect passed"
"#)
        .assert_passes();
}

#[test]
fn csrf_rejects_a_post_without_the_session_token() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = true
controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "empty CSRF ran: #{controller.status}" unless controller.status == 422

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "GET")
controller.request = req
ActionController::Current.request = req
controller.request_method = "GET"
masked = ActionView::ViewHelpers.form_authenticity_token
secret = controller.session[:_csrf_token].to_s
raise "session secret not minted: #{secret.inspect}" if secret.empty?
raise "token was the raw secret: #{masked.inspect}" if masked == secret
raise "masked token did not verify" unless ActionController::AuthenticityToken.valid?(masked, secret)

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = { "authenticity_token" => masked }
controller.process_action(:touch)
raise "matching masked CSRF failed: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "ok"

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = { "authenticity_token" => secret }
controller.process_action(:touch)
raise "unmasked session token failed: #{controller.status}" unless controller.status == 200

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST", "HTTP_X_CSRF_TOKEN" => masked)
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = {}
controller.process_action(:touch)
raise "X-CSRF-Token header failed: #{controller.status}" unless controller.status == 200 && controller.body == "ok"
puts "csrf passed"
"#)
        .assert_passes();
}

#[test]
fn csrf_implicit_default_and_skip_before_action_run() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  post \"/touch\", to: \"widgets#touch\"\n  post \"/open\", to: \"open#touch\"\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n  def touch\n    render plain: \"ok\"\n  end\nend\n")
        .write("app/controllers/open_controller.rb", "class OpenController < ApplicationController\n  skip_before_action :verify_authenticity_token\n  def touch\n    render plain: \"open\"\n  end\nend\n")
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
require_relative "app/controllers/open_controller"
ActionController::Base.allow_forgery_protection = true
controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "implicit CSRF ran: #{controller.status}" unless controller.status == 422

controller = OpenController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "skip_before_action did not skip: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "open"
puts "csrf skip passed"
"#)
        .assert_passes();
}

/// A prior explicit include is a no-op when an included block repeats it.
/// The Concern itself never gains the nested module as an ancestor.
#[test]
fn a_repeated_included_block_include_preserves_host_and_concern_ancestry() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n  included do\n    include Signing::Codes\n  end\n  def shout\n    \"outer\"\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n  def shout\n    \"inner\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing::Codes\n  include Signing\n",
        )
        .run_ruby(
            r#"raise "concern ancestry changed" if Signing.ancestors.include?(Signing::Codes)
a = Article.new(title: "Hi", body: "Body text here")
raise "repeated include changed precedence" unless a.shout == "outer"
raise "host ancestry changed" unless Article.ancestors.index(Signing) < Article.ancestors.index(Signing::Codes)
"#,
        )
        .assert_passes();
}

#[test]
fn a_shared_factory_respects_an_overridden_constructor() {
    emit_and_run::real_blog()
        .write("app/services/custom_factory.rb", r#"module CustomFactory
  class_methods do
    def build
      new
    end
  end
end
class FactoryReading < T::Struct
  const :label, String
end
class FactoryPacket
  include CustomFactory
  def self.new
    FactoryReading.new(label: "custom")
  end
end
class FactoryConsumer
  def self.label
    FactoryPacket.build.label.upcase
  end
end
"#)
        .run_ruby(r#"
raise "constructor identity" unless FactoryPacket.build.class == FactoryReading
raise "constructor consumer" unless FactoryConsumer.label == "CUSTOM"
"#)
        .assert_passes();
}

/// Unlike a hash pattern with keys, `{}` requires the hash to be empty.
/// A bare `is_a?(Hash)` check silently chose the wrong case arm for
/// every nonempty hash. `**` explicitly permits the remaining keys.
#[test]
fn an_empty_hash_pattern_rejects_extra_keys() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/hash_pattern_probe.rb",
            r#"class HashPatternProbe
  #: (Hash[Symbol, Integer]) -> bool
  def self.empty_match(value)
    case value
    in {}
      true
    else
      false
    end
  end

  #: (Hash[Symbol, Integer]) -> bool
  def self.open_match(value)
    case value
    in { ** }
      true
    else
      false
    end
  end

  #: (Hash[Symbol, Integer]) -> bool
  def self.key_match(value)
    case value
    in { x: 1 }
      true
    else
      false
    end
  end
end
"#,
        )
        .run_ruby(
            r#"raise "empty hash did not match" unless HashPatternProbe.empty_match({})
raise "nonempty hash incorrectly matched {}" if HashPatternProbe.empty_match({ x: 1 })
raise "open hash pattern rejected extra keys" unless HashPatternProbe.open_match({ x: 1 })
raise "keyed pattern rejected extra keys" unless HashPatternProbe.key_match({ x: 1, y: 2 })
raise "keyed pattern accepted a missing key" if HashPatternProbe.key_match({ y: 2 })
"#,
        )
        .assert_passes();
}

#[test]
fn model_rest_and_block_parameters_run_with_their_source_arity() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  def tagged(*labels)
    labels.join(",")
  end
  def pair(first, *rest, last)
    [first, rest.join(","), last].join("|")
  end
  def each_title(&blk)
    [title, "tail"].each(&blk)
  end
  def forward_titles(...)
    each_title(...)
  end
  def both(*args, **opts)
    [args.join(","), opts[:tag]].join("|")
  end
"#)
        .write("app/services/rest_control.rb", r#"class RestControl
  def tagged(*labels)
    labels.join(",")
  end
end
"#)
        .run_ruby(r#"
article = Article.new(title: "source")
control = RestControl.new
raise "model rest" unless article.tagged("x", "y") == "x,y"
raise "empty rest" unless article.tagged == ""
raise "library control" unless control.tagged("x", "y") == article.tagged("x", "y")
raise "post parameter" unless article.pair("head", "a", "b", "last") == "head|a,b|last"
raise "empty post rest" unless article.pair("head", "last") == "head||last"
seen = []
result = article.each_title { |value| seen << value.upcase }
raise "block values" unless seen == ["SOURCE", "TAIL"]
raise "block return" unless result == ["source", "tail"]
forwarded = []
article.forward_titles { |value| forwarded << value.upcase }
raise "forwarded block preservation" unless forwarded == seen
raise "rest with keywords" unless article.both("x", "y", tag: "z") == "x,y|z"
raise "empty positional rest" unless article.both(tag: "z") == "|z"
"#).assert_passes();
}

#[test]
fn duplicate_route_only_options_use_the_last_value_at_runtime() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "resources :articles do",
            "resources :articles, only: [], only: [:index, :show, :new, :create, :edit, :update, :destroy] do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn duplicate_route_except_options_use_the_last_value_at_runtime() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "resources :articles do",
            "resources :articles, except: [:show], except: [] do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn rubydex_qualified_value_constants_survive_shared_lowerings() {
    emit_and_run::real_blog()
        .write("app/services/collection_constants.rb", r#"
class CollectionConstants
  WORDS = ["a", "bb"]
  LENGTHS = WORDS.index_by(&:length)
  def self.values
    [LENGTHS[2], "a".in?(WORDS)]
  end
end
"#)
        .run_ruby("raise 'qualified lowered constants' unless CollectionConstants.values == ['bb', true]")
        .assert_passes();
}

/// `invisible_captcha only: :create` → before_action that heads :ok when
/// a honeypot field is filled (invariant 6: the survey gap closing is a
/// claim the emitted gate runs).
#[test]
fn invisible_captcha_blocks_spam_posts() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  protect_from_forgery with: :exception\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n")
        .write("app/models/user.rb", "class User < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  post \"/users\", to: \"users#create\"\nend\n")
        .write(
            "app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  invisible_captcha only: :create\n\n  def create\n    render plain: \"created\"\n  end\nend\n",
        )
        .run_ruby(r#"
require_relative "app/controllers/users_controller"
ActionController::Base.allow_forgery_protection = false
controller = UsersController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = { "subtitle" => "http://spam.example" }
controller.process_action(:create)
raise "honeypot did not block: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body.to_s.empty?
controller = UsersController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = { "email" => "ok@example.com" }
controller.process_action(:create)
raise "clean post failed: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "created"
puts "invisible_captcha passed"
"#)
        .assert_passes();
}

/// `impersonates :user` wraps `current_user` and exposes pretender's
/// impersonate / stop helpers (invariant 6).
#[test]
fn impersonates_switches_current_user() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  def current_user\n    User.find_by(id: session[:signed_in_user_id])\n  end\n\n  impersonates :user\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n")
        .write("app/models/user.rb", "class User < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/who\", to: \"who#show\"\nend\n")
        .write(
            "app/controllers/who_controller.rb",
            "class WhoController < ApplicationController\n  def show\n    render plain: [true_user&.email, current_user&.email].join(\",\")\n  end\nend\n",
        )
        .run_ruby(r#"
require_relative "app/controllers/who_controller"
admin = User.create!(email: "admin@example.com")
other = User.create!(email: "other@example.com")
controller = WhoController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request = req
ActionController::Current.request = req
controller.session[:signed_in_user_id] = admin.id
raise "baseline true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "admin@example.com"
controller.impersonate_user(other)
raise "impersonating true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "other@example.com"
controller.stop_impersonating_user
raise "stopped true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "admin@example.com"
puts "impersonates passed"
"#)
        .assert_passes();
}

/// Controller ivars written through `instance_variable_set` with a
/// statically resolvable name (literal, `controller_name`, or a helper
/// that inflects `self.class`) reach the template, and a template that
/// assigns an ivar reaches the layout — the same view context Rails
/// uses. Overlay is abstract; the forcing fixture is Writebook.
fn record_ivar_set_overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            r#"    def set_article
      instance_variable_set "@#{instance_name}", Article.find(params.expect(:id))
    end

    def instance_name
      controller_record_name.underscore
    end

    def controller_record_name
      self.class.to_s.remove("Controller").demodulize.singularize
    end
"#,
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<% content_for :title, \"Showing article\" %>\n",
            "<% @section_class = \"reading\" %>\n<% content_for :title, \"Showing article\" %>\n",
        )
        .edit(
            "app/views/layouts/application.html.erb",
            "<main class=\"container mx-auto mt-28 px-5 flex flex-col\">",
            "<main class=\"container mx-auto mt-28 px-5 flex flex-col <%= @section_class %>\">",
        )
}

#[test]
fn instance_variable_set_and_view_assigned_layout_ivar_run() {
    record_ivar_set_overlay()
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn controller_name_instance_variable_set_runs_on_show() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            "    def set_article\n      instance_variable_set(\"@#{controller_name.singularize}\", Article.find(params.expect(:id)))\n    end\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn campfire_capture_stdlib_consts_run() {
    // Campfire tip's TimeLimitedVideoPreviewer#capture and web-push pool
    // name IO / Timeout / Process. Registering them clears check errors;
    // this pin proves the emitted Ruby actually runs those Consts
    // (invariant 6) — without claiming the full ActiveStorage capture
    // path (#557 lands VideoPreviewer separately).
    emit_and_run::real_blog()
        .write(
            "app/models/capture_stdlib_probe.rb",
            r#"class CaptureStdlibProbe
  def self.exercise
    timed_out = false
    begin
      Timeout.timeout(0.05) { sleep 1 }
    rescue Timeout::Error
      timed_out = true
    end
    raise "Timeout.timeout did not fire" unless timed_out

    pid = Process.pid
    raise "Process.pid" unless pid.is_a?(Integer) && pid > 0

    clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    raise "CLOCK_MONOTONIC" unless clock.is_a?(Float)

    out = IO.popen(["echo", "hi"], in: IO::NULL, err: IO::NULL) { |io| io.read }
    raise "IO.popen" unless out.to_s.include?("hi")

    killed = false
    IO.popen(["sleep", "30"]) do |io|
      Process.kill(:KILL, io.pid)
      killed = true
    end
    raise "Process.kill" unless killed

    src = StringIO.new("xy")
    dst = StringIO.new
    n = IO.copy_stream(src, dst)
    raise "IO.copy_stream" unless n == 2 && dst.string == "xy"

    rescued = false
    begin
      raise SystemCallError, "x"
    rescue SystemCallError
      rescued = true
    end
    raise "SystemCallError" unless rescued

    rescued = false
    begin
      raise OpenSSL::SSL::SSLError, "x"
    rescue OpenSSL::SSL::SSLError
      rescued = true
    end
    raise "OpenSSL::SSL::SSLError" unless rescued

    # Vips::Error is registered for Campfire's attachment rescue; the
    # constant is supplied by ruby-vips at runtime, not by this probe.

    "ok"
  end
end
"#,
        )
        .run_ruby(
            r#"
raise "probe" unless CaptureStdlibProbe.exercise == "ok"
"#,
        )
        .assert_passes();
}

#[test]
fn campfire_video_preview_config_runs() {
    // Campfire tip initializer sets video_preview_arguments (gte(t,5))
    // and swaps previewers VideoPreviewer → TimeLimitedVideoPreviewer.
    // Suite asserts ActiveStorage.previewers / video_preview_arguments;
    // poster reads the vf filter from the same config.
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/time_limited_video_previewer.rb",
            r#"class TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
  TIME_LIMIT = 10
end
"#,
        )
        .write(
            "config/initializers/extensions.rb",
            r#"Dir[Rails.root.join("lib/rails_ext/*.rb")].sort.each { |f| require f }
"#,
        )
        .write(
            "config/initializers/active_storage.rb",
            r#"require "rails_ext/time_limited_video_previewer"

Rails.application.configure do
  config.active_storage.video_preview_arguments =
    "-vf 'select=eq(n\\,0)+eq(key\\,1)+gt(scene\\,0.015)+gte(t\\,5),loop=loop=-1:size=2,trim=start_frame=1'" \
    " -frames:v 1 -f image2"

  config.active_storage.previewers = config.active_storage.previewers.map do |previewer|
    previewer == ActiveStorage::Previewer::VideoPreviewer ? TimeLimitedVideoPreviewer : previewer
  end
end
"#,
        )
        .run_ruby(
            r#"
raise "args" unless ActiveStorage.video_preview_arguments.include?("gte(t\\,5)")
raise "filter" unless ActiveStorage.video_preview_vf_filter.include?("gte(t\\,5)")
raise "previewers include" unless ActiveStorage.previewers.include?(TimeLimitedVideoPreviewer)
raise "previewers exclude" if ActiveStorage.previewers.include?(ActiveStorage::Previewer::VideoPreviewer)

# `-vf` must match as a whole option, not a prefix of `-vframes`.
def ActiveStorage.video_preview_arguments
  "-vframes 1 -vf 'scale=320:240' -f image2"
end
raise "vf vs vframes" unless ActiveStorage.video_preview_vf_filter == "scale=320:240"
"#,
        )
        .assert_passes();
}

#[test]
fn an_rbs_array_block_runs_after_app_emission() {
    emit_and_run::real_blog()
        .write("app/lib/batch.rb", runtime_block_signature::RUBY)
        .write("sig/batch.rbs", runtime_block_signature::RBS)
        .run_ruby("raise 'wrong sum' unless Batch.new.consume == 3")
        .assert_passes();
}

/// `pairs.to_h` with no block reads each element as a [key, value] pair; the
/// ivar rewritten through it keeps its String keys and Integer values.
#[test]
fn an_ivar_rewritten_through_sort_by_to_h_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def word_counts\n    @word_counts = {}\n    body.split.each do |w|\n      @word_counts[w] = 0 if @word_counts[w].nil?\n      @word_counts[w] += 1\n    end\n    @word_counts = @word_counts.sort_by { |k, v| [-v, k] }.to_h\n  end\n\n  def top_word\n    word_counts.keys.first\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Counts", body: "b a b c b a")
counts = article.word_counts
raise counts.inspect unless counts == { "b" => 3, "a" => 2, "c" => 1 }
raise article.top_word.inspect unless article.top_word == "b"
"#,
        )
        .assert_passes();
}

/// A class an initializer defines and the app reads only through `[]`
/// (forem's `ApplicationConfig["KEY"]`) is the app's, as `X.` and `X::` are.
#[test]
fn an_initializer_class_read_through_brackets_runs() {
    emit_and_run::real_blog()
        .write(
            "config/initializers/app_settings.rb",
            "class AppSettings\n  DEFAULTS = { \"BANNER\" => \"Welcome\" }.freeze\n\n  def self.[](key)\n    DEFAULTS.fetch(key, \"\")\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def banner\n    \"#{AppSettings[\"BANNER\"]}: #{title}\"\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Brackets", body: "Body text here")
raise article.banner.inspect unless article.banner == "Welcome: Brackets"
"#,
        )
        .assert_passes();
}

/// Array `&` and `|` are set intersection and union in Ruby. The
/// typed targets used to print their native operators: bitwise on
/// TypeScript (two arrays coerce to `0`), a `TypeError` on Python
/// lists, and a `.&(…)` method call that Rust cannot parse.
fn array_set_operators_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.common_ids\n    [1, 2, 2, 3] & [2, 3, 4]\n  end\n\n  def self.either_ids\n    [3, 1, 1] | [2, 1]\n  end\n",
        )
        .write(
            "test/models/set_ops_test.rb",
            "require \"test_helper\"\n\nclass SetOpsTest < ActiveSupport::TestCase\n  test \"array set operators\" do\n    assert_equal [2, 3], Article.common_ids\n    assert_equal [3, 1, 2], Article.either_ids\n  end\nend\n",
        )
}

#[test]
fn array_set_operators_run() {
    array_set_operators_app()
        .run_test("test/models/set_ops_test.rb")
        .assert_passes();
}

#[test]
fn array_set_operators_are_not_native_infix_on_typed_targets() {
    use roundhouse::project::BuildTarget;
    for (target, file, intersect, union) in [
        (
            BuildTarget::Typescript,
            "app/models/article.ts",
            "((__l, __r) => [...new Set([...__l])].filter(x => __r.includes(x)))([1, 2, 2, 3], [2, 3, 4])",
            "[...new Set([...[3, 1, 1], ...[2, 1]])]",
        ),
        (
            BuildTarget::Python,
            "app/v2/models.py",
            "(lambda __l, __r, __eq: [x for i, x in enumerate(__l) if any(__eq(x, y) for y in __r) and not any(__eq(x, y) for y in __l[:i])])([1, 2, 2, 3], [2, 3, 4], lambda a, b: type(a) is type(b) and a == b)",
            "(lambda __a, __eq: [x for i, x in enumerate(__a) if not any(__eq(x, y) for y in __a[:i])])([*[3, 1, 1], *[2, 1]], lambda a, b: type(a) is type(b) and a == b)",
        ),
        (
            BuildTarget::Rust,
            "src/models/article.rs",
            "if __rhs.contains(x) && !__out.contains(x)",
            "for x in __lhs.iter().chain(__rhs.iter())",
        ),
    ] {
        let (tree, errors) = array_set_operators_app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let src = std::fs::read_to_string(tree.join(file))
            .unwrap_or_else(|e| panic!("{target:?}: read {file}: {e}"));
        assert!(src.contains(intersect), "{target:?} `&`:\n{src}");
        assert!(src.contains(union), "{target:?} `|`:\n{src}");
    }
}

/// `Model.delete_all` returns the affected-row count, as Rails does —
/// the class form and the scoped `Relation` form alike. The class form
/// used to type as `Int` while running to `nil`.
#[test]
fn delete_all_returns_affected_row_count() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_delete_all_test.rb",
            r#"require "test_helper"

class ArticleDeleteAllTest < ActiveSupport::TestCase
  test "delete_all returns the number of rows deleted" do
    Comment.delete_all
    Article.delete_all
    3.times { |i| Article.create!(title: "gone-#{i}", body: "Body text here") }
    Article.create!(title: "kept", body: "Body text here")
    assert_equal 3, Article.where("title LIKE 'gone-%'").delete_all
    assert_equal 1, Article.delete_all
    assert_equal 0, Article.delete_all
  end
end
"#,
        )
        .run_test("test/models/article_delete_all_test.rb")
        .assert_passes();
}

/// Destructured block parameters and nested multi-write targets bind
/// every name. campfire's `|(host, secure, origin), index|` was emitted
/// as `|index|`, and its body read three names nothing bound.
#[test]
fn nested_destructuring_binds_every_name() {
    const SOURCE: &str = r#"class NestedDestructureProbe
  def self.block_params
    out = []
    [[1, 2, 3], [4, 5, 6]].each_with_index do |(a, b, c), index|
      out << [a, b, c, index]
    end
    out
  end
  def self.deep_block
    [[1, [2, 3]]].map { |(a, (b, c))| [a, b, c] }
  end
  def self.lambda_param
    adder = ->((a, b)) { a + b }
    adder.call([20, 22])
  end
  def self.multi_write
    _, (_, removed) = [1, [2, 3]]
    removed
  end
  def self.deep_multi_write
    a, (b, (c, d)) = [1, [2, [3, 4]]]
    [a, b, c, d]
  end
  def self.expression
    (a, (b, c) = [1, [2, 3]])
  end
end
"#;
    const ASSERTIONS: &str = r##"
expected = {block_params: [[1, 2, 3, 0], [4, 5, 6, 1]], deep_block: [[1, 2, 3]], lambda_param: 42, multi_write: 3, deep_multi_write: [1, 2, 3, 4], expression: [1, [2, 3]]}
expected.each do |method, want|
  got = NestedDestructureProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/services/nested_destructure_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}

/// `recv.m(**payload, badge: b)` where nothing types `recv`: the app's
/// one `m` takes `**rest`, so the `**` the ingest desugar erased is put
/// back (campfire's `WebPush::Pool#deliver_later`). Passed positionally
/// it is Ruby 3's `wrong number of arguments (given 1, expected 0)`.
#[test]
fn a_keyword_splat_to_an_untyped_receiver_keeps_its_double_splat() {
    emit_and_run::real_blog()
        // A MODEL method, as campfire's `Push::Subscription#notification`
        // is: models keep `badge:` a keyword beside `**params`.
        .edit("app/models/comment.rb", "  validates :commenter", r#"  def kwsplat_probe_note(badge: 0, **params)
    KwSplatNote.new(**params, badge: badge)
  end

  validates :commenter"#)
        .write("app/services/kwsplat_probe.rb", r#"class KwSplatNote
  attr_reader :title, :badge

  def initialize(title:, badge:)
    @title, @badge = title, badge
  end
end

class KwSplatCaller
  def self.call(items, payload)
    items.map { |item| item.kwsplat_probe_note(**payload, badge: 3) }
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Splat title", body: "A sufficiently long article body.")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = KwSplatCaller.call([comment], { title: "t" }).map { |n| [n.title, n.badge] }
raise "keyword splat lost: #{got.inspect}" unless got == [["t", 3]]
"#)
        .assert_passes();
}

/// Rails' `association(:name).loaded?` on a belongs_to: false until the
/// reader runs, true after — the question campfire's presentation tests
/// ask of a page of messages.
#[test]
fn association_loaded_answers_for_a_belongs_to() {
    emit_and_run::real_blog()
        .write("app/services/loaded_probe.rb", r#"class LoadedProbe
  def self.flags(comment)
    before = comment.association(:article).loaded?
    comment.article
    [before, comment.association(:article).loaded?]
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Loaded title", body: "A sufficiently long article body.")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = LoadedProbe.flags(Comment.find(comment.id))
raise "association(:article).loaded? answered #{got.inspect}" unless got == [false, true]
"#)
        .assert_passes();
}

/// `owner.<has_many>.reload` reads the rows again (campfire's rooms
/// test: `assert_empty room.memberships.reload`).
#[test]
fn has_many_reload_reads_the_rows_again() {
    emit_and_run::real_blog()
        .write("app/services/reload_probe.rb", r#"class ReloadProbe
  def self.counts(article)
    before = article.comments.size
    Comment.create!(article_id: article.id, commenter: "Late", body: "Arrived later")
    [before, article.comments.reload.size]
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Reload title", body: "A sufficiently long article body.")
Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = ReloadProbe.counts(Article.find(article.id))
raise "reload answered #{got.inspect}" unless got == [1, 2]
"#)
        .assert_passes();
}

/// `reorder(Arel.sql("+articles.id"))` keeps its fragment: Rails takes an
/// `Arel.sql` literal past the column-name check (campfire's
/// `reorder(Arel.sql("+messages.created_at"))`, SQLite's index-skipping
/// unary plus), while a bare String with the same text is refused.
#[test]
fn an_arel_sql_order_fragment_passes_the_column_check() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  scope :plus_ordered, -> { order(:title).reorder(Arel.sql("+articles.id")) }
"#)
        .run_ruby(r#"
Article.create!(title: "Second", body: "A sufficiently long article body.")
Article.create!(title: "First", body: "A sufficiently long article body.")
got = Article.plus_ordered.map(&:title)
raise "Arel.sql order answered #{got.inspect}" unless got == ["Second", "First"]
begin
  Article.plus_ordered.reorder("+articles.id").to_a
  raise "a bare String fragment passed the column check"
rescue ArgumentError
end
"#)
        .assert_passes();
}

/// A concern's `before_update -> { … }` lambda callback, Rails'
/// `attachment_changes`, and commit callbacks that run after `after_save`
/// — campfire's `Message::Searchable` reindexes a message whose file was
/// replaced: the lambda notes `attachment_changes.key?("attachment")`,
/// and the `after_update_commit` reads the NEW file's name, which only
/// exists once the save's own `after_save` has attached it. The lambda
/// was dropped from the concern, `attachment_changes` didn't exist, and
/// the commit hooks fired before `after_save`.
#[test]
fn a_replaced_attachment_is_seen_by_the_commit_callback() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  include Tracked\n  has_one_attached :file\nend\n")
        .write("app/models/doc/tracked.rb", r#"module Doc::Tracked
  extend ActiveSupport::Concern

  included do
    before_update -> { @file_replaced = attachment_changes.key?("file") }
    after_update_commit :note_file, if: :file_replaced?
  end

  def seen
    @seen
  end

  private
    def file_replaced?
      @file_replaced
    end

    def note_file
      @seen = file.filename.to_s
    end
end
"#)
        .run_ruby(r#"
upload = ->(name) { { io: StringIO.new("bytes"), filename: name, content_type: "text/plain" } }
doc = Doc.create!(name: "first", file: upload.("a.txt"))
doc.update!(name: "renamed")
raise "a rename noted a file: #{doc.seen.inspect}" unless doc.seen.nil?
doc.update!(file: upload.("b.txt"))
raise "the commit callback saw #{doc.seen.inspect}" unless doc.seen == "b.txt"
raise "the change outlived the save: #{doc.attachment_changes.inspect}" unless doc.attachment_changes.empty?
"#)
        .assert_passes();
}

/// Three things Rails' test environment does that campfire's tests lean
/// on, given on the CRuby tree's test support:
/// - `allow_forgery_protection = false` (Rails' generated test.rb) means
///   forms carry no authenticity token, so a cached fragment and a fresh
///   render of it are the same bytes (campfire's messages caching test);
/// - `ActiveSupport::Notifications.subscribe(regexp)` hears the fragment
///   cache's reads and writes, keys in Rails' shape, until unsubscribed;
/// - `freeze_time` stops `Time.current` on the instant records are
///   stamped with (campfire's user test compares the two).
#[test]
fn rails_test_environment_tokens_cache_events_and_clock() {
    emit_and_run::real_blog()
        .edit("app/views/articles/_article.html.erb", "<div id=\"<%= dom_id(article) %>\"", "<% cache article do %>\n<div id=\"<%= dom_id(article) %>\"")
        .edit("app/views/articles/_article.html.erb", "  </div>\n</div>\n", "  </div>\n</div>\n<% end %>\n")
        .write(
            "test/controllers/test_environments_controller_test.rb",
            r#"require "test_helper"

class TestEnvironmentsControllerTest < ActionDispatch::IntegrationTest
  test "forms carry no token while forgery protection is off" do
    get new_article_url
    assert_select "form"
    assert_select "input[name='authenticity_token']", count: 0
    get articles_url
    assert_select "form.button_to"
    assert_select "input[name='authenticity_token']", count: 0
  end

  test "a subscriber hears fragment cache reads and writes until it unsubscribes" do
    keys = []
    subscriber = ActiveSupport::Notifications.subscribe(/\Acache_(read|write)\.active_support\z/) do |*, payload|
      keys << payload[:key]
    end
    previous_caching = ActionController::Base.perform_caching
    ActionController::Base.perform_caching = true
    begin
      get articles_url
      assert keys.any? { |key| key.include?("articles/_article/articles/") }, keys.inspect
      heard = keys.size
      ActiveSupport::Notifications.unsubscribe(subscriber)
      get articles_url
      assert_equal heard, keys.size
    ensure
      ActionController::Base.perform_caching = previous_caching
      ActiveSupport::Notifications.unsubscribe(subscriber)
    end
  end

  test "freeze_time stops Time.current on the instant records are stamped with" do
    freeze_time
    article = Article.create!(title: "Frozen", body: "A sufficiently long article body.")
    assert_equal Time.current, article.created_at
    assert_equal Time.current, Article.find(article.id).updated_at
  end
end
"#,
        )
        .run_test("test/controllers/test_environments_controller_test.rb")
        .assert_passes();
}

/// A Duration handed to the stdlib as a timeout, and `Timeout.timeout`
/// answering its block's value — campfire's unfurl (`DEADLINE =
/// 10.seconds` around the fetch, `open_timeout: 7.seconds` into
/// `Net::HTTP.start`). The Duration did no arithmetic, so
/// `TCPSocket.new(connect_timeout:)` could not convert it and every fetch failed;
/// and `Timeout.timeout { record }` typed Untyped, so `render json:`
/// wrote the record's `inspect` instead of its JSON.
#[test]
fn a_duration_times_out_the_stdlib_and_a_timed_block_keeps_its_type() {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  get \"/timed/:id\", to: \"articles#timed\"\n  get \"/twice\", to: \"articles#twice\"\n  resources :articles do\n")
        .edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", r#"class ArticlesController < ApplicationController
  LIMIT = 2.seconds

  def timed
    card = Timeout.timeout(LIMIT) { Card.new(title: Article.find(params[:id]).title) }
    render json: card
  end

  def twice
    first = Article.first
    again = Article.first
    render plain: (first.id == again.id).to_s
  end

"#)
        .write("app/models/card.rb", "class Card\n  include ActiveModel::Model\n\n  attr_accessor :title\nend\n")
        .write(
            "test/controllers/timed_articles_controller_test.rb",
            r#"require "test_helper"

require "active_record/testing/query_assertions"

class TimedArticlesControllerTest < ActionDispatch::IntegrationTest
  include ActiveRecord::Assertions::QueryAssertions

  test "a request replays an identical query, as Rails' query cache does" do
    assert_queries_count(1) { get "/twice" }
    assert_equal "true", response.body
    assert_queries_count(2) { Article.first; Article.first }
  end

  test "the timed lookup renders the model's JSON" do
    article = articles(:one)
    get "/timed/#{article.id}"
    assert_response :success
    assert_equal article.title, JSON.parse(response.body)["title"]
  end

  test "duration works with socket and stdlib timeout APIs" do
    server = TCPServer.new("127.0.0.1", 0)
    socket = TCPSocket.new("127.0.0.1", server.addr[1], connect_timeout: 1.second)
    socket.close
    server.close
    assert_nil IO.select(nil, nil, nil, 0.01.seconds)
    assert_equal :ok, Timeout.timeout(1.second) { :ok }
    http = Net::HTTP.new("127.0.0.1", 80)
    http.open_timeout = 1.second
    assert_equal 1.0, http.open_timeout
    assert_equal 1.5, (2.seconds - 0.5).to_f
    assert_equal 1.5, (2.seconds - 0.5.seconds).to_f
    assert_operator 1.second, :<, 2
  end
end
"#,
        )
        .run_test("test/controllers/timed_articles_controller_test.rb")
        .assert_passes();
}

/// Destroying a record takes its attachments with it, as `dependent:`
/// says: Rails' default `:purge_later` deletes the join row and purges
/// the blob in an `ActiveStorage::PurgeJob` (run inline here, with no
/// queue adapter), and `dependent: false` leaves both. Without it
/// campfire's `Room#destroy_one_message_at_a_time` left every message's
/// attachment row and file behind.
#[test]
fn destroying_an_owner_purges_its_attachments_as_dependent_says() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  has_one_attached :file\n  has_one_attached :keep, dependent: false\nend\n")
        .run_ruby(r#"
upload = ->(name) { { io: StringIO.new("bytes"), filename: name, content_type: "text/plain" } }
count = ->(table) { ActiveRecord::Base.connection.select_value("SELECT count(*) FROM #{table}") }
doc = Doc.create!(name: "first", file: upload.("a.txt"), keep: upload.("b.txt"))
raise "attached #{count.("active_storage_attachments")}" unless count.("active_storage_attachments") == 2
doc.destroy
raise "rows left: #{count.("active_storage_attachments")}" unless count.("active_storage_attachments") == 1
raise "blobs left: #{count.("active_storage_blobs")}" unless count.("active_storage_blobs") == 1
raise "PurgeJob not recorded" unless ActiveJob.performed.include?("ActiveStorage::PurgeJob")
"#)
        .assert_passes();
}

/// Fixtures load the way Rails' `insert_fixtures_set` loads them: raw
/// rows, no validations and no callbacks. A callback that raises would
/// abort the load if it ran; campfire's Message `after_create_commit`
/// marked memberships unread at load time, so the fixture users began
/// with unread rooms Rails never gives them.
#[test]
fn fixture_rows_load_without_running_callbacks() {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", "class Comment < ApplicationRecord\n  before_save { raise \"a callback ran for a fixture row\" }\n  after_create_commit { raise \"a commit callback ran for a fixture row\" }\n")
        .write(
            "test/models/fixture_load_test.rb",
            r#"require "test_helper"

class FixtureLoadTest < ActiveSupport::TestCase
  test "every comment fixture is in the table, stamped" do
    assert_operator Comment.count, :>, 0
    assert Comment.all.all? { |comment| comment.created_at && comment.updated_at }
  end
end
"#,
        )
        .run_test("test/models/fixture_load_test.rb")
        .assert_passes();
}

/// `owner.assoc.create!(attributes)` in a test, with the attributes in a
/// local: the test-side association rewrite kept only a LITERAL hash and
/// replaced anything else with the foreign key alone, so campfire's
/// `rooms(:pets).messages.create!(attributes)` saved a message with no
/// creator ("Validation failed: Creator must exist"). The value now
/// merges the key in, and the association's key wins over the caller's,
/// as in Rails.
#[test]
fn an_association_create_keeps_attributes_held_in_a_local() {
    emit_and_run::real_blog()
        .write(
            "test/models/assoc_create_test.rb",
            r#"require "test_helper"

class AssocCreateTest < ActiveSupport::TestCase
  test "attributes in a local reach the record" do
    attributes = { commenter: "Reader", body: "Comment body" }
    comment = articles(:one).comments.create!(attributes)
    assert_equal "Reader", comment.commenter
    assert_equal articles(:one).id, comment.article_id
  end

  test "the association's key wins over the caller's" do
    comment = articles(:one).comments.create!(commenter: "Lit", body: "Literal body", article_id: articles(:two).id)
    assert_equal articles(:one).id, comment.article_id
  end
end
"#,
        )
        .run_test("test/models/assoc_create_test.rb")
        .assert_passes();
}

/// Active Storage's "was it made?" questions, as campfire's
/// presentation asks them so a view never makes a preview: a variant is
/// `processed?` once its record exists, and its `image` is that record's
/// (both looked up, not made — asked of a file that cannot be decoded,
/// `image` used to raise the decode error again);
/// `preview(:poster)` names a variant the owner's `has_one_attached`
/// block declares; and `url_for` of a Preview held in a typed local is
/// the preview's representation URL, not the object.
#[test]
fn a_variant_is_processed_once_its_record_exists_and_a_preview_resolves_its_name() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", r#"class Doc < ApplicationRecord
  has_one_attached :file do |attachable|
    attachable.variant :thumb, resize_to_limit: [ 10, 10 ]
    attachable.variant :poster, format: :webp, resize_to_limit: [ 10, 10 ]
  end

  def poster_url
    poster = file.preview(:poster)
    ActionView::ViewHelpers.url_for(poster)
  end
end
"#)
        .run_ruby(r#"
doc = Doc.create!(name: "first", file: { io: StringIO.new("bytes"), filename: "a.mov", content_type: "video/quicktime" })
thumb = doc.file.representation(:thumb)
raise "processed before any record" if thumb.processed?
raise "image made a variant of bytes that are no image" unless thumb.image.nil?
raise "variant records: #{ActiveStorage::VariantRecord.count}" unless ActiveStorage::VariantRecord.count == 0
image = ActiveStorage::Blob.create_and_upload!("png", "a.png", "image/png")
connection = ActiveRecord::Base.connection
connection.execute("INSERT INTO active_storage_variant_records (blob_id, variation_digest) VALUES (#{doc.file.blob.id}, '#{thumb.variation.digest}')")
record_id = connection.select_value("SELECT max(id) FROM active_storage_variant_records")
connection.execute("INSERT INTO active_storage_attachments (name, record_type, record_id, blob_id, created_at) VALUES ('image', 'ActiveStorage::VariantRecord', #{record_id}, #{image.id}, '2026-01-01')")
raise "not processed once its record exists" unless doc.file.representation(:thumb).processed?
raise "variant records: #{ActiveStorage::VariantRecord.count}" unless ActiveStorage::VariantRecord.count == 1
raise "no image once its record exists" if doc.file.representation(:thumb).image.nil?
preview = doc.file.preview(:poster)
raise "preview named #{preview.variation&.name.inspect}" unless preview.variation.name == "poster"
url = doc.poster_url
raise "url_for(preview) answered #{url.inspect}" unless url.is_a?(String) && url.include?("/representations/")
"#)
        .assert_passes();
}

/// `insert_all!` and `insert_all` are one raw INSERT per row in Rails:
/// no validations, no callbacks, timestamps filled. `insert_all!`
/// raises on a duplicate and answers an `ActiveRecord::Result` of the
/// RETURNING columns, whose `rows` are Arrays of values (campfire's
/// search test reads new ids with `.rows.flatten`); `insert_all` skips
/// a duplicate. Both used to run the save callbacks, and `insert_all!`
/// was undefined.
#[test]
fn bulk_inserts_skip_callbacks_and_insert_all_bang_returns_its_rows() {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", r#"class Comment < ApplicationRecord
  after_create_commit { raise "a callback ran for a bulk insert" }

  def self.bulk_ids(article, names)
    rows = names.map { |name| { article_id: article.id, commenter: name, body: "Bulk body" } }
    Comment.insert_all!(rows, returning: %w[ id commenter ]).rows
  end

  def self.bulk_skipping(article, names)
    rows = names.map { |name| { article_id: article.id, commenter: name, body: "Bulk body" } }
    Comment.insert_all(rows)
  end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Bulk", body: "A sufficiently long article body.")
rows = Comment.bulk_ids(article, ["a", "b"])
raise "rows #{rows.inspect}" unless rows.map(&:last) == ["a", "b"] && rows.all? { |r| r.first.is_a?(Integer) }
raise "timestamps" unless Comment.where(article_id: article.id).all? { |c| c.created_at && c.updated_at }
Comment.bulk_skipping(article, ["c"])
raise "count #{Comment.where(article_id: article.id).count}" unless Comment.where(article_id: article.id).count == 3
"#)
        .assert_passes();
}

/// campfire's `MessagesController#create` from 2c53c46 on: the
/// controller renders the message once, hands the markup to a concern
/// method by keyword (`broadcast_create(html:)`), which broadcasts it
/// with `**(html ? { html: html } : {})`, and the turbo-stream view
/// appends the same String (`turbo_stream.append target, @html`). Its
/// cache layer folds `I18n.locale` into keys and copies response headers
/// by downcased name (`response.headers.slice(*CACHE_HEADERS)`).
///
/// Before: the keyword reached the flattened `broadcast_create(html =
/// nil)` as a Hash, `broadcast_append_to` with that option was left
/// unlowered (undefined on the record), the view looked for a
/// `message_htmls/_message_html` partial, `I18n` was undefined, and
/// `headers["ETag"]` / `headers["etag"]` were two headers.
#[test]
fn rendered_markup_reaches_the_broadcast_and_the_stream_as_given() {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  post \"/articles/:article_id/comments/:id/announce\", to: \"comments#announce\", as: :announce_article_comment\n  resources :articles do\n")
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  include Announces\n  belongs_to :article\n")
        .write("app/models/comment/announces.rb", r##"module Comment::Announces
  def announce(html: nil)
    broadcast_append_to article, :announcements, target: [ article, :announcements ], **(html ? { html: html } : {})
  end
end
"##)
        .edit("app/controllers/comments_controller.rb", "  def destroy\n", r##"  def announce
    @comment = @article.comments.find(params[:id])
    @comment_html = render_to_string partial: "comments/comment", formats: :html, locals: { comment: @comment }
    @comment.announce(html: @comment_html)
    response.headers["X-Probe"] = "first"
    response.headers["x-probe"] = "second"
    response.headers["X-Gone"] = "soon"
    response.headers.delete("x-gone")
    response.headers["X-Copied"] = response.headers.slice("X-PROBE", "x-absent").map { |name, value| "#{name}=#{value}" }.join(",")
    response.headers["X-Locale"] = I18n.locale.to_s
  end

  def destroy
"##)
        .write("app/views/comments/announce.turbo_stream.erb", "<%= turbo_stream.append dom_id(@comment.article, :announcements), @comment_html %>\n")
        .write("test/controllers/announcements_controller_test.rb", r##"require "test_helper"

class AnnouncementsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = Article.create!(title: "Announced", body: "A sufficiently long article body.")
    @comment = @article.comments.create!(commenter: "Ann", body: "Said <b>once</b>")
    @stream = "#{@article.to_gid_param}:announcements"
  end

  test "the rendered comment is broadcast and streamed as given" do
    post announce_article_comment_url(@article, @comment, format: :turbo_stream)
    assert_response :success
    assert_includes response.body, %(<turbo-stream action="append" target="announcements_article_#{@article.id}">)
    assert_includes response.body, %(<div id="comment_#{@comment.id}")
    assert_includes response.body, "Said &lt;b&gt;once&lt;/b&gt;"
    assert_not_includes response.body, "&lt;div"

    sent = ActionCable.server.pubsub.broadcasts(@stream).map { |broadcast| JSON.parse(broadcast) }
    assert_equal 1, sent.size
    assert_includes sent.first, %(<div id="comment_#{@comment.id}")
    assert_not_includes sent.first, "&lt;div"
  end

  test "header names match without regard to case" do
    post announce_article_comment_url(@article, @comment, format: :turbo_stream)
    assert_equal "second", response.headers["X-Probe"]
    assert_nil response.headers["X-Gone"]
    assert_equal "x-probe=second", response.headers["X-Copied"]
    assert_equal "en", response.headers["X-Locale"]
  end

  test "markup handed in replaces the partial, and without it the partial renders" do
    @comment.announce(html: "<p>handed in</p>")
    @comment.announce
    sent = ActionCable.server.pubsub.broadcasts(@stream).map { |broadcast| JSON.parse(broadcast) }
    assert_equal 2, sent.size
    assert_includes sent.first, "<p>handed in</p>"
    assert_includes sent.last, %(<div id="comment_#{@comment.id}")
  end
end
"##)
        .run_test("test/controllers/announcements_controller_test.rb")
        .assert_passes();
}

/// Framework surface campfire main's tests and cache keys reach, each
/// with Rails' behaviour:
/// - `request.format.to_s` is the negotiated Mime string;
/// - `Rails.application.env_config` keeps what a test sets in it;
/// - a helper test's `controller` is a test controller, whose
///   `perform_caching` is false in the test environment;
/// - `update_columns` and `delete` skip callbacks;
/// - `raise ActiveRecord::Rollback` undoes the transaction quietly;
/// - `clear_enqueued_jobs`, `assert_nothing_raised`, `assert_dom_equal`.
fn rails_surface_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  get \"/format\", to: \"articles#format_probe\"\n  resources :articles do\n")
        .edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", r##"class ArticlesController < ApplicationController
  def format_probe
    render plain: request.format.to_s
  end

"##)
        .write("app/helpers/articles_helper.rb", r##"module ArticlesHelper
  def caching_label
    controller.perform_caching ? "cached" : "fresh"
  end
end
"##)
        .write("app/jobs/touch_job.rb", "class TouchJob < ApplicationJob\n  def perform(article)\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  SAVED = []\n  after_save { SAVED << title }\n\n")
        .write("test/helpers/articles_helper_test.rb", r##"require "test_helper"

class ArticlesHelperTest < ActionView::TestCase
  test "a helper reads the test controller" do
    assert_equal "fresh", caching_label
  end

  test "markup compares up to attribute order and the whitespace between tags" do
    assert_dom_equal %(<p class="a" id="b">x</p>), %(<p id="b"  class="a">x</p>\n)
    assert_nothing_raised { Article.count }
  end
end
"##)
        .write("test/controllers/format_probes_controller_test.rb", r##"require "test_helper"

class FormatProbesControllerTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  test "the request format is its Mime string" do
    get "/format"
    assert_equal "text/html", response.body
  end

  test "env_config keeps what a test sets" do
    previous = Rails.application.env_config["action_dispatch.show_exceptions"]
    Rails.application.env_config["action_dispatch.show_exceptions"] = :rescuable
    assert_equal :rescuable, Rails.application.env_config["action_dispatch.show_exceptions"]
  ensure
    Rails.application.env_config["action_dispatch.show_exceptions"] = previous
  end

  test "update_columns and delete skip callbacks" do
    article = Article.create!(title: "Columns", body: "A sufficiently long article body.")
    saved = Article::SAVED.size
    article.update_columns(title: "Renamed")
    assert_equal saved, Article::SAVED.size
    assert_equal "Renamed", Article.find(article.id).title
    article.delete
    assert_nil Article.find_by(id: article.id)
  end

  test "a rollback undoes the transaction without raising" do
    count = Article.count
    result = Article.transaction do
      Article.create!(title: "Gone", body: "A sufficiently long article body.")
      raise ActiveRecord::Rollback
    end
    assert_nil result
    assert_equal count, Article.count
  end

  test "clear_enqueued_jobs forgets what was enqueued" do
    article = Article.create!(title: "Queued", body: "A sufficiently long article body.")
    TouchJob.perform_later(article)
    assert_enqueued_jobs 1, only: TouchJob
    clear_enqueued_jobs
    assert_no_enqueued_jobs only: TouchJob
  end
end
"##)
}

#[test]
fn rails_surface_for_cache_keys_rollbacks_and_jobs() {
    rails_surface_app()
        .run_test("test/controllers/format_probes_controller_test.rb")
        .assert_passes();
}

#[test]
fn a_helper_test_has_a_test_controller_and_dom_assertions() {
    rails_surface_app()
        .run_test("test/helpers/articles_helper_test.rb")
        .assert_passes();
}

/// Three ingest/emit shapes campfire main's web push pool reaches:
/// - `@idle[address].pop&.first` evaluates the receiver ONCE (the `&.`
///   desugar read it twice, popping two entries);
/// - classes nested in a class under a runtime namespace
///   (`class WebPush::Connections` with `class HTTP < Net::HTTP; include
///   Stages; end`) are emitted, a module a sibling includes first;
/// - each nested class keeps its own name inside the parent.
#[test]
fn a_safe_navigated_call_runs_once_and_nested_classes_survive_a_compact_parent() {
    emit_and_run::real_blog()
        .write("lib/web_push/probe_pool.rb", r##"class WebPush::ProbePool
  class Lost < StandardError; end

  module Stages
    def stage
      :checking
    end
  end

  class HTTP < Net::HTTP
    include Stages
  end

  def initialize
    @idle = { "a" => [ [ :first, 1 ], [ :second, 2 ] ] }
  end

  def checkout(address)
    @idle[address].pop&.first
  end

  def left(address)
    @idle[address].size
  end

  def connection
    HTTP.new("example.com", 443)
  end
end
"##)
        .run_ruby(r##"
pool = WebPush::ProbePool.new
taken = pool.checkout("a")
raise "took #{taken.inspect}" unless taken == :second
raise "popped #{2 - pool.left("a")}" unless pool.left("a") == 1
raise "no stage" unless pool.connection.stage == :checking
raise "HTTP is #{WebPush::ProbePool::HTTP.superclass}" unless WebPush::ProbePool::HTTP.superclass == Net::HTTP
raise "Lost" unless WebPush::ProbePool::Lost.ancestors.include?(StandardError)
"##)
        .assert_passes();
}

/// `def m(...)` that only forwards to `super`, where `super` is Net::HTTP's
/// private `begin_transport(req)` / `connect` (campfire's
/// `WebPush::Connections::Stages`): the overrides take those signatures
/// (`lower::known_super_forwarding`), the spinel emit has no refusal left,
/// and on both lanes the hooks run in CRuby's order around two requests
/// over a connection the server closes after each.
fn super_forwarding_transport_hooks_app() -> emit_and_run::Overlay {
    // Top level, not nested in a compact parent: a class nested in
    // `class A::B` is undefined on spinel (matz/spinel#8369), which is
    // campfire's own spelling and not what this pins.
    emit_and_run::real_blog().write("lib/probe_stages.rb", r##"module ProbeStages
  attr_reader :stage

  def trace
    @trace ||= []
  end

  private
    def begin_transport(...)
      trace << "begin"
      @stage = :checking
      super.tap { @stage = :sent }
    end

    def connect(...)
      trace << "connect"
      @stage = :connecting if @stage == :checking
      super
    end
end

class ProbeHTTP < Net::HTTP
  include ProbeStages
end
"##)
}

const SUPER_FORWARDING_SCRIPT: &str = r##"
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
t = Thread.new do
  2.times do |i|
    c = server.accept
    while (line = c.gets)
      break if line.strip.empty?
    end
    c.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nr#{i}")
    c.close
  end
end
http = ProbeHTTP.new("127.0.0.1", port)
http.start
bodies = [http.get("/").body, http.get("/").body]
http.finish
t.join
raise "bodies #{bodies.inspect}" unless bodies == ["r0", "r1"]
raise "stage #{http.stage.inspect}" unless http.stage == :sent
raise "trace #{http.trace.inspect}" unless http.trace == ["connect", "begin", "begin", "connect"]
"##;

#[test]
fn forwarding_into_net_http_transport_hooks_runs_on_ruby() {
    super_forwarding_transport_hooks_app().run_ruby(SUPER_FORWARDING_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn forwarding_into_net_http_transport_hooks_runs_on_spinel() {
    super_forwarding_transport_hooks_app().run_spinel(SUPER_FORWARDING_SCRIPT).assert_passes();
}

/// A `test/test_helpers/` module that one test class includes itself
/// (campfire's `include PushServiceTestHelper`), carried whole into the
/// test's file: its nested class, its module methods, a constructor
/// whose keywords stay keywords (`Server.new(**options)`), a duration in
/// a module method, and `assert_not_kind_of`.
#[test]
fn a_test_helper_module_one_test_includes_is_carried_whole() {
    emit_and_run::real_blog()
        .write("test/test_helpers/probe_server_helper.rb", r##"module ProbeServerHelper
  NAME = "probe"

  class Server
    attr_reader :status, :label

    def initialize(status: "201 Created", label: -> { NAME })
      @status, @label = status, label.call
    end
  end

  class << self
    def started_at
      1.minute.ago
    end
  end

  private
    def with_server(**options)
      yield Server.new(**options)
    end
end
"##)
        .write("test/models/probe_server_test.rb", r##"require "test_helper"

class ProbeServerTest < ActiveSupport::TestCase
  include ProbeServerHelper

  test "the helper's class takes its keywords" do
    with_server(status: "500 Oops") do |server|
      assert_equal "500 Oops", server.status
      assert_equal "probe", server.label
      assert_not_kind_of String, server
    end
    with_server do |server|
      assert_equal "201 Created", server.status
    end
  end

  test "a module method reads a duration" do
    assert ProbeServerHelper.started_at < Time.now
  end
end
"##)
        .run_test("test/models/probe_server_test.rb")
        .assert_passes();
}

/// campfire main keeps `rooms.messages_count` with SQLite triggers it
/// installs itself, and repairs the count once fixtures are loaded:
/// - a `load_fixtures` override on the app's `ActiveSupport::TestCase`
///   (`fixtures = super; …; fixtures`) runs its statements after the
///   fixtures, ahead of the tests' own setup;
/// - `connection.data_source_exists?` / `column_exists?` read SQLite's
///   catalog, `raw_connection.transaction(:immediate)` is a write
///   transaction below ActiveRecord, `connection_pool.release_connection`
///   answers;
/// - `clear_query_caches_for_current_thread` leaves the cache on.
#[test]
fn a_fixture_hook_and_trigger_maintenance_run_as_rails_runs_them() {
    emit_and_run::real_blog()
        .edit("test/test_helper.rb", "    # Add more helper methods to be used by all tests here...\n", r##"    def load_fixtures(config)
      fixtures = super
      Article::Counting.ensure!
      fixtures
    end
"##)
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.integer \"comments_total\", default: 0, null: false\n")
        .write("app/models/article/counting.rb", r##"class Article::Counting
  TRIGGER = "comments_ai_articles_total"

  def self.ensure!(connection = ActiveRecord::Base.connection)
    return unless connection.adapter_name.match?(/sqlite/i)
    return unless connection.data_source_exists?(:articles)
    return unless connection.column_exists?(:articles, :comments_total)
    return if installed?(connection)

    connection.raw_connection.transaction(:immediate) do
      connection.execute("UPDATE articles SET comments_total = (SELECT COUNT(*) FROM comments WHERE comments.article_id = articles.id)")
      connection.execute("CREATE TRIGGER #{TRIGGER} AFTER INSERT ON comments BEGIN UPDATE articles SET comments_total = comments_total + 1 WHERE id = NEW.article_id; END")
    end
  end

  def self.installed?(connection = ActiveRecord::Base.connection)
    connection.select_value("SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = #{connection.quote(TRIGGER)}").present?
  end
end
"##)
        .write("test/models/article_counting_test.rb", r##"require "test_helper"

class ArticleCountingTest < ActiveSupport::TestCase
  test "the fixture hook installed the trigger and backfilled the counts" do
    assert Article::Counting.installed?
    article = articles(:one)
    assert_equal article.comments.count, article.comments_total
    article.comments.create!(commenter: "Ann", body: "Counted by the trigger")
    assert_equal article.comments.count, article.reload.comments_total
  end

  test "schema questions read the catalog" do
    connection = ActiveRecord::Base.connection
    assert connection.data_source_exists?(:comments)
    assert_not connection.data_source_exists?(:nothing_here)
    assert connection.column_exists?(:articles, :comments_total)
    assert_not connection.column_exists?(:articles, :missing)
    assert_nil ActiveRecord::Base.connection_pool.release_connection
    assert_nil ActiveRecord::Base.clear_query_caches_for_current_thread
  end

  test "a raw write transaction rolls back when its block raises" do
    connection = ActiveRecord::Base.connection
    count = Article.count
    assert_raises(RuntimeError) do
      connection.raw_connection.transaction(:immediate) do
        connection.execute("DELETE FROM articles")
        raise "abandon"
      end
    end
    assert_equal count, Article.count
  end
end
"##)
        .run_test("test/models/article_counting_test.rb")
        .assert_passes();
}

/// campfire main's header-only forgery protection, from the test side:
/// - an app helper overriding `token_tag` to answer "" leaves every form
///   without a token field (Rails' form helpers build it through that);
/// - a POST carrying `_method: "delete"` is routed as the DELETE it says
///   (Rack's MethodOverride, as both production dispatchers apply it);
/// - the integration session's `reset!` starts a fresh one.
#[test]
fn a_blank_token_tag_override_method_override_and_session_reset() {
    emit_and_run::real_blog()
        .write("app/helpers/application_helper.rb", r##"module ApplicationHelper
  private
    def token_tag(*)
      ""
    end
end
"##)
        .write("test/controllers/token_free_forms_controller_test.rb", r##"require "test_helper"

class TokenFreeFormsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @previous = ActionController::Base.allow_forgery_protection
    ActionController::Base.allow_forgery_protection = true
  end

  teardown do
    ActionController::Base.allow_forgery_protection = @previous
  end

  test "forms carry no token field" do
    get new_article_url
    assert_response :success
    assert_select "form"
    assert_select "input[name='authenticity_token']", count: 0
  end

  test "a POST with _method is the verb it names" do
    ActionController::Base.allow_forgery_protection = false
    article = Article.create!(title: "Overridden", body: "A sufficiently long article body.")
    assert_difference "Article.count", -1 do
      post article_url(article), params: { _method: "delete" }
    end
  end

  test "reset! starts a fresh session" do
    https!
    host! "blog.example.test"
    get articles_url
    reset!
    assert_not https?
    assert_equal "www.example.com", host
    assert_nil response
  end
end
"##)
        .run_test("test/controllers/token_free_forms_controller_test.rb")
        .assert_passes();
}

/// A bounded `ActiveSupport::Cache::MemoryStore` keeps an html-safe
/// String (a SafeBuffer) as such, as Rails' store does. campfire main
/// caches each text message's rendered body this way (`auto_link`
/// answers a SafeBuffer); the store refused anything but an exact
/// String, so with caching on every message body rendered empty.
#[test]
fn a_memory_store_keeps_an_html_safe_string() {
    emit_and_run::real_blog()
        .run_ruby(r##"
store = ActiveSupport::Cache::MemoryStore.new(size: 4096)
body = store.fetch("presentation") { "<b>hi</b>".html_safe }
raise "fetch answered #{body.inspect}" unless body == "<b>hi</b>" && body.html_safe?
again = store.read("presentation")
raise "read answered #{again.inspect}" unless again == "<b>hi</b>" && again.html_safe?
raise "the cached copy is the caller's object" if again.equal?(body)
"##)
        .assert_passes();
}

/// A model-only app (no controllers) must boot: `config/routes.rb`
/// requires `application_controller` only when the app defines one, the
/// same defined-only rule the per-controller requires follow.
#[test]
fn a_model_only_app_boots_without_an_application_controller() {
    emit_and_run::empty_app()
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets" do |t|
    t.integer "owner_id", null: false
    t.boolean "enabled", default: false, null: false
    t.string "name", null: false
  end
end
"#)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :name, presence: true\nend\n")
        .write("test/fixtures/widgets.yml", "on:\n  owner_id: 7\n  name: a\n  enabled: true\n\noff:\n  owner_id: 8\n  name: b\n  enabled: false\n")
        .write("test/models/widget_test.rb", r#"require "test_helper"

class WidgetTest < ActiveSupport::TestCase
  test "enabled defaults to false" do
    refute Widget.new(owner_id: 1).enabled
  end

  test "requires name" do
    record = Widget.new(owner_id: 1, name: nil)
    refute_predicate record, :valid?
    assert_includes record.errors[:name], "can't be blank"
  end

  test "can query by owner_id and enabled status" do
    record = widgets(:on)
    assert Widget.exists?(owner_id: record.owner_id, enabled: true)
    refute Widget.exists?(owner_id: record.owner_id, enabled: false)
  end
end
"#)
        .run_test("test/models/widget_test.rb")
        .assert_passes();
}

/// An app with nothing in it but what the emitted test boot needs: an
/// application controller, an empty route table and a schema.
fn bare_test_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n")
}

/// A gem keeps its tests at `test/` root over a base its helper
/// defines (`class GemCompat::TestCase < ActiveSupport::TestCase`
/// in test/test_helper.rb). With `test_paths: [test]` the helper was
/// read a second time as a test file, and the emitted subclass never
/// loaded its base: NameError on `GemCompat` before any test ran.
#[test]
fn a_test_case_base_defined_in_the_helper_is_loaded_by_its_subclasses() {
    bare_test_app()
        .write("app/.keep", "")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/fragments.rb", "module Fragments\n  def self.random\n    \"RANDOM()\"\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support\"\n\nmodule GemCompat\n  class TestCase < ActiveSupport::TestCase\n  end\nend\n")
        .write("test/fragments_test.rb", "require \"test_helper\"\n\nclass FragmentsTest < GemCompat::TestCase\n  test \"random\" do\n    assert_equal \"RANDOM()\", Fragments.random\n  end\nend\n")
        .run_test("test/models/fragments_test.rb")
        .assert_passes();
}

/// A helper that defines only a plain class with a superclass (a logger
/// subclass the tests share) is not a test-case base: no test file, no
/// autorun driver for zero tests.
#[test]
fn a_plain_class_in_the_helper_is_not_read_as_a_test_case() {
    let (emitted, errors) = bare_test_app()
        .write("app/.keep", "")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/fragments.rb", "module Fragments\n  def self.random\n    \"RANDOM()\"\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support\"\nrequire \"logger\"\n\nclass RecordingLogger < Logger\nend\n")
        .write("test/fragments_test.rb", "require \"test_helper\"\n\nclass FragmentsTest < ActiveSupport::TestCase\n  test \"random\" do\n    assert_equal \"RANDOM()\", Fragments.random\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "got {errors:?}");
    let emitted_tests: Vec<String> = ["test", "test/models"]
        .iter()
        .filter_map(|d| std::fs::read_dir(emitted.join(d)).ok())
        .flat_map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    assert!(emitted_tests.iter().any(|f| f == "fragments_test.rb"), "got {emitted_tests:?}");
    assert!(emitted_tests.iter().all(|f| !f.contains("recording_logger")), "got {emitted_tests:?}");
}
