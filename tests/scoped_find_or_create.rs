//! A scoped find-or-create must retain scalar equality attributes and
//! run initialization before validation, only on the new-record branch.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn scoped_find_or_create_initializes_new_records_before_validation() {
    emit_and_run::real_blog()
        .write(
            "app/models/comment.rb",
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  validates :commenter, presence: true
  validates :body, presence: true

  def self.create_owned
    Comment.where(article_id: 41).find_or_create_by!(commenter: "Alice") do |record|
      record.body = "initialized before validation"
    end
  end

  def self.create_direct_bang
    Comment.find_or_create_by!(article_id: 43, commenter: "Carol", body: "direct valid body")
  end

  def self.create_without_block
    Comment.where(article_id: 44).find_or_create_by!(commenter: "Dave", body: "scoped valid body")
  end

  def self.create_shadowed
    record = Comment.new(body: "outer value")
    Comment.where(article_id: 45).find_or_create_by!(commenter: "Eve") do |record|
      record.body = "shadowed value"
    end
    record
  end

  def self.create_rebound
    Comment.where(article_id: 46).find_or_create_by!(commenter: "Frank") do |record|
      record.body = "original value"
      record = Comment.new(body: "rebound value")
    end
  end

  def self.create_override
    Comment.where(article_id: 41).find_or_create_by!(article_id: 48, commenter: "Heidi", body: "explicit ownership wins")
  end

  def self.create_chained
    Comment.where(article_id: 41).where(article_id: 42).find_or_create_by!(commenter: "Chained", body: "last scope equality wins")
  end

  def self.create_invalid_bang
    Comment.where(article_id: 49).find_or_create_by!(commenter: "Invalid bang")
  end

  def self.create_invalid_non_bang
    Comment.where(article_id: 49).find_or_create_by(commenter: "Invalid plain")
  end

  def self.create_owned_non_bang
    Comment.where(article_id: 42).find_or_create_by(commenter: "Bob") do |record|
      record.body = "initialized before validation"
    end
  end
end
"#,
        )
        .run_ruby(
            r#"
(41..49).each do |id|
  Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (#{id}, 'Owner', 'A sufficiently long body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
end
first = Comment.create_owned
raise "ownership lost" unless first.article_id == 41
raise "block did not initialize" unless first.body == "initialized before validation"
raise "not persisted" unless first.persisted?
first.body = "existing body"
first.save!
again = Comment.create_owned
raise "duplicated existing row" unless again.id == first.id
raise "yielded existing row" unless again.body == "existing body"
plain = Comment.create_owned_non_bang
raise "non-bang ownership lost" unless plain.article_id == 42
raise "non-bang block lost" unless plain.body == "initialized before validation"
raise "non-bang not persisted" unless plain.persisted?
direct = Comment.create_direct_bang
raise "direct bang ownership lost" unless direct.article_id == 43
raise "direct bang not persisted" unless direct.persisted?
scoped = Comment.create_without_block
raise "blockless scope lost" unless scoped.article_id == 44
raise "blockless scope not persisted" unless scoped.persisted?
outer = Comment.create_shadowed
raise "block parameter leaked" unless outer.body == "outer value"
rebound = Comment.create_rebound
raise "saved block rebind instead of original" unless rebound.body == "original value" && rebound.persisted?
overridden = Comment.create_override
raise "explicit conditions did not override scope defaults" unless overridden.article_id == 48 && overridden.persisted?
chained = Comment.create_chained
raise "last scope equality did not win" unless chained.article_id == 42 && chained.persisted?
invalid = Comment.create_invalid_non_bang
raise "plain invalid record was saved" if invalid.persisted?
raise "plain invalid record lost validation errors" if invalid.errors.empty?
begin
  Comment.create_invalid_bang
  raise "bang did not reject invalid record"
rescue ActiveRecord::RecordInvalid
end
puts "PASS scoped find-or-create blocks before validation"
"#,
        )
        .assert_passes();
}

#[test]
fn unsupported_find_or_create_block_control_flow_remains_an_error() {
    for control in [
        "break record",
        "next record",
        "return record",
        "redo",
        "record ||= Comment.new",
        "record, other = [Comment.new, 1]",
    ] {
        let model = format!(
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.unsupported
    Comment.where(article_id: 41).find_or_create_by!(commenter: "Alice") do |record|
      {control}
    end
  end
end
"#
        );
        let run = emit_and_run::real_blog()
            .write("app/models/comment.rb", &model)
            .run_ruby("");
        assert!(
            run.errors
                .iter()
                .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
            "{control} was accepted without executable control-flow lowering: {:?}",
            run.errors
        );
    }
}

#[test]
fn dynamic_relations_and_non_scalar_creation_conditions_remain_errors() {
    for (label, expression) in [
        (
            "primary key",
            "Comment.find_or_create_by!(id: 99, commenter: \"Key\")",
        ),
        (
            "incompatible literal",
            "Comment.where(article_id: \"41\").find_or_create_by!(commenter: \"Cast\")",
        ),
        (
            "OR scope",
            "Comment.where(article_id: 41).or(Comment.where(article_id: 42)).find_or_create_by!(commenter: \"OR\")",
        ),
        (
            "array predicate",
            "Comment.where(article_id: [41, 42]).find_or_create_by!(commenter: \"Array\")",
        ),
        (
            "range predicate",
            "Comment.where(article_id: (41..42)).find_or_create_by!(commenter: \"Range\")",
        ),
        (
            "association",
            "Article.new.comments.find_or_create_by!(commenter: \"Assoc\")",
        ),
        (
            "rest parameter",
            "Comment.where(article_id: 41).find_or_create_by!(commenter: \"Rest\") { |record, *rest| record.body = \"body\" }",
        ),
        (
            "qualified column",
            "Comment.where(\"comments.article_id\" => 41).find_or_create_by!(commenter: \"Qualified\")",
        ),
    ] {
        let model = format!(
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.unsupported
    {expression}
  end
end
"#
        );
        let run = emit_and_run::real_blog()
            .write("app/models/comment.rb", &model)
            .run_ruby("");
        assert!(
            run.errors
                .iter()
                .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
            "{label} was claimed supported: {:?}",
            run.errors
        );
    }
}

#[test]
fn effectful_assignment_targets_remain_errors() {
    let run = emit_and_run::real_blog().write("app/models/comment.rb", r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.unsupported
    rows = []
    rows[Comment.count] = Comment.where(article_id: 41).find_or_create_by!(commenter: "Target", body: "valid")
  end
end
"#).run_ruby("");
    assert!(
        run.errors
            .iter()
            .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
        "effectful assignment receiver/index was reordered: {:?}",
        run.errors
    );
}

#[test]
fn constructor_callbacks_see_scope_and_explicit_conditions() {
    emit_and_run::real_blog().write("app/models/comment.rb", r#"
class Comment < ApplicationRecord
  belongs_to :article
  validates :body, presence: true
  after_initialize do
    self.body = "owner #{article_id}"
  end

  def self.create_derived
    Comment.where(article_id: 41).find_or_create_by!(article_id: 42, commenter: "Callback")
  end
end
"#).run_ruby(r#"
Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (42, 'Owner', 'A sufficiently long body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
record = Comment.create_derived
raise "callback missed initialized attributes" unless record.body == "owner 42"
raise "derived record not saved" unless record.persisted?
puts "PASS constructor receives defaults and explicit attributes before callbacks"
"#).assert_passes();
}

#[test]
fn block_local_assignment_cannot_capture_a_later_implicit_method_call() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/comment.rb",
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.scratch
    "helper"
  end
  def self.unsupported
    Comment.where(article_id: 41).find_or_create_by!(commenter: "Local") do |record|
      scratch = "block local"
      record.body = scratch
    end
    scratch
  end
end
"#,
        )
        .run_ruby("");
    assert!(
        run.errors
            .iter()
            .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
        "block local changed later implicit method dispatch: {:?}",
        run.errors
    );
}

#[test]
fn unlowered_symbol_initialization_callback_remains_an_error() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/comment.rb",
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  after_initialize :derive_body
  def derive_body
    self.body = "owner #{article_id}"
  end
  def self.unsupported
    Comment.where(article_id: 41).find_or_create_by!(commenter: "Callback")
  end
end
"#,
        )
        .run_ruby("");
    assert!(
        run.errors
            .iter()
            .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
        "symbol initialization callback was silently ignored: {:?}",
        run.errors
    );
}

#[test]
fn rescue_exception_bindings_remain_errors() {
    for binding in ["record", "scratch"] {
        let model = format!(
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.unsupported
    Comment.where(article_id: 41).find_or_create_by!(commenter: "Rescue") do |record|
      begin
        raise "after"
      rescue => {binding}
        @seen = {binding}.message
      end
    end
  end
end
"#
        );
        let run = emit_and_run::real_blog()
            .write("app/models/comment.rb", &model)
            .run_ruby("");
        assert!(
            run.errors
                .iter()
                .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
            "exception binding {binding} was changed or leaked: {:?}",
            run.errors
        );
    }
}

#[test]
fn unrepresented_initialization_block_signatures_fail_ingest() {
    for signature in [
        "record; scratch",
        "record, other = 1",
        "record, &callback",
        "record, *",
        "record, *rest, last",
        "record, key: 1",
        "record, **options",
        "(record, other)",
    ] {
        let source = format!(
            "Comment.find_or_create_by!(commenter: \"Signature\") {{ |{signature}| record.body = \"body\" }}"
        );
        let parsed = ruby_prism::parse(source.as_bytes());
        let program = parsed.node();
        let statement = program
            .as_program_node()
            .unwrap()
            .statements()
            .body()
            .iter()
            .next()
            .unwrap();
        let error = roundhouse::ingest::ingest_expr(&statement, "<signature>").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not represented in initialization-block IR"),
            "{signature} lost its declaration: {error}"
        );
    }
    for source in [
        "Comment.find_or_create_by!(commenter: \"Signature\") { |record| record.body = \"body\" }",
        "[1].each { |record| record.to_s }",
    ] {
        let parsed = ruby_prism::parse(source.as_bytes());
        let program = parsed.node();
        let statement = program
            .as_program_node()
            .unwrap()
            .statements()
            .body()
            .iter()
            .next()
            .unwrap();
        assert!(
            roundhouse::ingest::ingest_expr(&statement, "<ordinary block>").is_ok(),
            "ordinary single parameter block was rejected: {source}"
        );
    }
}

#[test]
fn forwarded_initialization_blocks_fail_before_signature_loss() {
    for block in [
        "&lambda { |record, other = (record.body = \"initialized\")| }",
        "&proc { |record, other = (record.body = \"initialized\")| }",
        "&->(record, other = (record.body = \"initialized\")) {}",
    ] {
        for method in ["find_or_create_by", "find_or_create_by!"] {
            let source = format!("Comment.{method}(commenter: \"Forwarded\", {block})");
            let parsed = ruby_prism::parse(source.as_bytes());
            assert!(parsed.errors().next().is_none(), "invalid source: {source}");
            let program = parsed.node();
            let statement = program
                .as_program_node()
                .unwrap()
                .statements()
                .body()
                .iter()
                .next()
                .unwrap();
            let error =
                roundhouse::ingest::ingest_expr(&statement, "<forwarded initializer>").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("forwarded initialization blocks are outside"),
                "{block} bypassed the attached block guard: {error}"
            );
        }
    }
    let parsed = ruby_prism::parse(b"[1].each(&->(record) { record.to_s })");
    let program = parsed.node();
    let statement = program
        .as_program_node()
        .unwrap()
        .statements()
        .body()
        .iter()
        .next()
        .unwrap();
    assert!(roundhouse::ingest::ingest_expr(&statement, "<unrelated forwarded block>").is_ok());
}

#[test]
fn nested_initialization_blocks_remain_errors() {
    for signature in ["record", "index; record"] {
        let model = format!(
            r#"
class Comment < ApplicationRecord
  belongs_to :article
  def self.unsupported
    Comment.where(article_id: 47).find_or_create_by!(commenter: "Nested") do |record|
      [1].each do |{signature}|
        @seen = record.to_s
      end
      record.body = "outer record"
    end
  end
end
"#
        );
        let run = emit_and_run::real_blog()
            .write("app/models/comment.rb", &model)
            .run_ruby("");
        assert!(
            run.errors
                .iter()
                .any(|error| error.contains("scoped find-or-create cannot be lowered safely")),
            "nested signature {signature} could lose binder metadata: {:?}",
            run.errors
        );
    }
}
