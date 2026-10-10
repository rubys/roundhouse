//! The static part of Writebook's Positionable concern. Whole ordering,
//! locking and rebalance compatibility are separate runtime obligations.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodReceiver, MethodVisibility, ModelBodyItem};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey};
use roundhouse::project::BuildTarget;
use roundhouse::ty::Ty;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const POSITIONABLE: &str = r#"module Positionable
  extend ActiveSupport::Concern
  class_methods do
    def positioned_within(parent, association:, filter:)
      define_method :positioning_parent do
        send(parent)
      end
      define_method :all_positioned_siblings do
        positioning_parent.send(association).send(filter).positioned
      end
      define_method :other_positioned_siblings do
        all_positioned_siblings.excluding(self)
      end
      private :positioning_parent, :all_positioned_siblings, :other_positioned_siblings
    end
  end
end
"#;

const POSITIONING_CONCERN: &str = r#"module PositioningConcern
  REBALANCE_THRESHOLD = 1e-10
  ELEMENT_GAP = 1

  included do
    scope :positioned, -> { order(:position_score, :id) }
    scope :active, -> { where(active: true) }
    scope :before, ->(other) { positioned.where("position_score < ?", other.position_score) }
    scope :after, ->(other) { positioned.where("position_score > ?", other.position_score) }
    after_save_commit :rebalance_positions, if: :rebalance_required?
  end

  class_methods do
    def positioned_within(parent, association:, filter:)
      define_method :positioning_parent do
        send(parent)
      end
      define_method :all_positioned_siblings do
        positioning_parent.send(association).send(filter).positioned
      end
      define_method :other_positioned_siblings do
        all_positioned_siblings.excluding(self)
      end
      private :positioning_parent, :all_positioned_siblings, :other_positioned_siblings
    end
  end

  def previous
    other_positioned_siblings.before(self).last
  end

  def next
    other_positioned_siblings.after(self).first
  end

  def move_to_position(offset, followed_by: [])
    with_positioning_lock do
      all_to_move = [self, *followed_by]
      before, after = before_and_after_for(offset: offset, moving: all_to_move)
      gap = (after - before) / (all_to_move.count + 1)
      all_to_move.each.with_index(1) do |item, index|
        item.update!(position_score: before + (index * gap))
      end
      remember_to_rebalance_positions if gap < REBALANCE_THRESHOLD
    end
  end

  private
    def before_and_after_for(offset:, moving:)
      other_items = all_positioned_siblings.excluding(moving)
      if offset < 1
        after = all_positioned_siblings.minimum(:position_score) || (2 * ELEMENT_GAP)
        before = after - ELEMENT_GAP
      else
        before, after = other_items.offset(offset - 1).limit(2).pluck(:position_score)
        before ||= all_positioned_siblings.maximum(:position_score)
        after ||= before + (moving.count.succ * ELEMENT_GAP)
      end
      [before, after]
    end

    def remember_to_rebalance_positions
      @rebalance_required = true
    end

    def rebalance_required?
      @rebalance_required
    end

    def rebalance_positions
      with_positioning_lock do
        ordered = all_positioned_siblings.select("row_number() over (order by position_score, id) as new_score, id")
        sql = "update #{self.class.table_name} set position_score = new_score from (#{ordered.to_sql}) as ordered where #{self.class.table_name}.id = ordered.id"
        self.class.connection.execute(sql)
      end
      @rebalance_required = false
    end

    def with_positioning_lock(&block)
      positioning_parent.with_lock(&block)
    end
end
"#;

fn app(concern: &str, leaf_body: &str, other_body: &str) -> roundhouse::App {
    app_with_prefix(concern, "", leaf_body, other_body)
}

fn app_with_prefix(
    concern: &str,
    prefix: &str,
    leaf_body: &str,
    other_body: &str,
) -> roundhouse::App {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :leaves do |t|\n    t.string :title\n  end\n  create_table :books do |t|\n    t.string :title\n  end\nend\n".to_string()),
        ("config/initializers/inflections.rb", "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.irregular \"leaf\", \"leaves\"\nend\n".to_string()),
        ("app/models/concerns/positionable.rb", concern.to_string()),
        ("app/models/leaf.rb", format!("class Leaf < ApplicationRecord\n{prefix}\n  include Positionable\n{leaf_body}\nend\n")),
        ("app/models/book.rb", format!("class Book < ApplicationRecord\n{other_body}\nend\n")),
    ];
    // Boundary tests inspect rejected source bodies as well as generated
    // methods. Preserve an explicit caller's survey collector if present.
    let own_survey = !survey::is_active();
    if own_survey {
        survey::activate();
    }
    let result = ingest_app_from_tree(
        files
            .into_iter()
            .map(|(path, text)| (PathBuf::from(path), text.into_bytes()))
            .collect::<HashMap<_, _>>(),
    );
    if own_survey {
        survey::drain();
    }
    result.expect("survey ingest")
}

fn model<'a>(app: &'a roundhouse::App, name: &str) -> &'a roundhouse::dialect::Model {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == name)
        .unwrap()
}

fn instances<'a>(app: &'a roundhouse::App, name: &str) -> Vec<&'a roundhouse::dialect::MethodDef> {
    model(app, name)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Instance)
        .collect()
}

fn unexpanded(app: &roundhouse::App, name: &str) -> usize {
    model(app, "Leaf")
        .body
        .iter()
        .filter(|item| {
            matches!(item, ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == name))
        })
        .count()
}

#[test]
fn rejected_recognized_macro_is_strict_error_but_survey_retains_source_body() {
    let files = HashMap::from([
        (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table :leaves do |t|; t.string :title; end\nend\n".to_vec()),
        (PathBuf::from("app/models/concerns/positionable.rb"), POSITIONABLE.as_bytes().to_vec()),
        (PathBuf::from("app/models/leaf.rb"), b"class Leaf < ApplicationRecord\n  include Positionable\n  positioned_within :book, association: :leaves\nend\n".to_vec()),
    ]);
    assert!(!survey::is_active());
    let error = ingest_app_from_tree(files.clone())
        .expect_err("missing required filter must refuse strict ingest");
    assert!(
        matches!(error, roundhouse::ingest::IngestError::Unsupported { file, message }
        if file == "app/models/leaf.rb" && message.contains("model macro `positioned_within` not expanded"))
    );
    assert!(!survey::is_active());

    survey::activate();
    let surveyed = ingest_app_from_tree(files).expect("survey must retain the model");
    let gaps = survey::drain();
    assert_eq!(gaps.len(), 1);
    assert!(
        matches!(&gaps[0], roundhouse::ingest::IngestError::Unsupported { file, message }
        if file == "app/models/leaf.rb" && message.contains("model macro `positioned_within` not expanded"))
    );
    assert_eq!(unexpanded(&surveyed, "positioned_within"), 1);
    assert!(instances(&surveyed, "Leaf").is_empty());
    assert!(!survey::is_active());

    let valid = HashMap::from([
        (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table :leaves do |t|; t.string :title; end\nend\n".to_vec()),
        (PathBuf::from("app/models/concerns/positionable.rb"), POSITIONABLE.as_bytes().to_vec()),
        (PathBuf::from("app/models/leaf.rb"), b"class Leaf < ApplicationRecord\n  include Positionable\n  positioned_within :book, association: :leaves, filter: :active\nend\n".to_vec()),
    ]);
    let supported = ingest_app_from_tree(valid).expect("valid strict macro must still expand");
    assert_eq!(unexpanded(&supported, "positioned_within"), 0);
    assert_eq!(instances(&supported, "Leaf").len(), 3);
}

#[test]
fn binds_keywords_by_name_and_keeps_each_includers_capture_and_privacy() {
    let app = app(
        POSITIONABLE,
        "  positioned_within :book, filter: :active, association: :leaves\n  def marker\n    :public_marker\n  end",
        "  include Positionable\n  positioned_within :shelf, association: :books, filter: :published",
    );
    assert_eq!(unexpanded(&app, "positioned_within"), 0);
    for (class, parent, association, filter) in [
        ("Leaf", "book", "leaves", "active"),
        ("Book", "shelf", "books", "published"),
    ] {
        let methods = instances(&app, class);
        for name in [
            "positioning_parent",
            "all_positioned_siblings",
            "other_positioned_siblings",
        ] {
            let method = methods
                .iter()
                .find(|m| m.name.as_str() == name)
                .expect("generated method");
            assert_eq!(method.visibility, MethodVisibility::Private);
            assert!(method.params.is_empty());
        }
        let parent_body = roundhouse::emit::ruby::emit_expr(
            &methods
                .iter()
                .find(|m| m.name.as_str() == "positioning_parent")
                .unwrap()
                .body,
        );
        assert!(
            parent_body.contains(parent) && !parent_body.contains("send("),
            "{class}: {parent_body}"
        );
        let siblings_body = roundhouse::emit::ruby::emit_expr(
            &methods
                .iter()
                .find(|m| m.name.as_str() == "all_positioned_siblings")
                .unwrap()
                .body,
        );
        assert!(
            siblings_body.contains(association)
                && siblings_body.contains(filter)
                && !siblings_body.contains("send("),
            "{class}: {siblings_body}"
        );
    }
    assert_eq!(
        instances(&app, "Leaf")
            .iter()
            .find(|m| m.name.as_str() == "marker")
            .unwrap()
            .visibility,
        MethodVisibility::Public
    );
}

#[test]
fn optional_keywords_bind_by_source_kind_even_when_ingest_flattened_them() {
    let concern = "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install(target: :title)\n      define_method :chosen do\n        send(target)\n      end\n    end\n  end\nend\n";
    for (call, expected) in [("install", "title"), ("install target: :id", "id")] {
        let app = app(concern, call, "");
        let methods = instances(&app, "Leaf");
        let generated = methods
            .iter()
            .find(|m| m.name.as_str() == "chosen")
            .expect("expanded optional keyword");
        let body = roundhouse::emit::ruby::emit_expr(&generated.body);
        assert!(body.contains(expected) && !body.contains("send("), "{body}");
    }
    assert_eq!(unexpanded(&app(concern, "install :id", ""), "install"), 1);
}

#[test]
fn unsupported_captures_and_headers_decline_the_whole_macro() {
    for call in [
        "positioned_within :book, association: :leaves",
        "positioned_within :book, :shelf, association: :leaves, filter: :active",
        "positioned_within :book, association: :leaves, filter: :active, extra: :x",
        "positioned_within choose_parent, association: :leaves, filter: :active",
    ] {
        survey::activate();
        let app = app(POSITIONABLE, call, "");
        let gaps = survey::drain();
        assert_eq!(unexpanded(&app, "positioned_within"), 1, "{call}");
        assert!(instances(&app, "Leaf").is_empty(), "{call}");
        assert!(
            gaps.iter().any(|g| matches!(g,
                roundhouse::ingest::IngestError::Unsupported { file, message }
                if file == "app/models/leaf.rb"
                    && message.contains("model macro `positioned_within` not expanded")
            )),
            "{gaps:?}"
        );
    }
    for replacement in [
        "define_method :all_positioned_siblings do |association|",
        "define_method :all_positioned_siblings do |association = :wrong|",
        "define_method :all_positioned_siblings do |association: :wrong|",
        "define_method :all_positioned_siblings do |; association|",
    ] {
        let concern =
            POSITIONABLE.replace("define_method :all_positioned_siblings do", replacement);
        let app = app(
            &concern,
            "positioned_within :book, association: :leaves, filter: :active",
            "",
        );
        assert_eq!(unexpanded(&app, "positioned_within"), 1, "{replacement}");
        assert!(
            instances(&app, "Leaf").is_empty(),
            "must not retain first helper: {replacement}"
        );
    }
    for replacement in [
        "association = :wrong\n        positioning_parent.send(association).send(filter).positioned",
        "[:wrong].map { |association| positioning_parent.send(association) }",
    ] {
        let concern = POSITIONABLE.replace(
            "positioning_parent.send(association).send(filter).positioned",
            replacement,
        );
        let app = app(
            &concern,
            "positioned_within :book, association: :leaves, filter: :active",
            "",
        );
        assert_eq!(unexpanded(&app, "positioned_within"), 1);
        assert!(instances(&app, "Leaf").is_empty());
    }
    let concern = POSITIONABLE.replace(
        "private :positioning_parent",
        "unmodeled_side_effect\n      private :positioning_parent",
    );
    let app = app(
        &concern,
        "positioned_within :book, association: :leaves, filter: :active",
        "",
    );
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn rejects_redefinitions_and_existing_schema_or_user_method_names() {
    let call = "positioned_within :book, association: :leaves, filter: :active";
    for second in [call, "positioned_within :shelf, association: :books"] {
        let app = app(POSITIONABLE, &format!("{call}\n{second}"), "");
        assert!(instances(&app, "Leaf").is_empty());
        assert_eq!(unexpanded(&app, "positioned_within"), 2);
    }
    let app = app(
        POSITIONABLE,
        &format!("{call}\n  def positioning_parent\n    :own\n  end"),
        "",
    );
    assert_eq!(instances(&app, "Leaf").len(), 1);
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
    let app = self::app(
        &POSITIONABLE.replace("positioning_parent", "title"),
        call,
        "",
    );
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn an_includers_own_class_method_is_not_the_concerns_macro() {
    let app = app(
        POSITIONABLE,
        "positioned_within :book, association: :leaves, filter: :active\n  def self.positioned_within(parent, association:, filter:)\n    :own\n  end",
        "",
    );
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn opaque_second_invocations_and_duplicate_providers_cannot_partially_expand() {
    let call = "positioned_within :book, association: :leaves, filter: :active";
    for second in [
        format!("self.{call}"),
        format!("{call} do\n  :ignored\nend"),
    ] {
        let app = app(POSITIONABLE, &format!("{call}\n{second}"), "");
        assert!(instances(&app, "Leaf").is_empty(), "{second}");
        assert_eq!(unexpanded(&app, "positioned_within"), 2);
    }
    let concern = POSITIONABLE.replace("    def positioned_within", "    def positioned_within(parent, association:, filter:)\n      define_method :positioning_parent do\n        :wrong\n      end\n    end\n    def positioned_within");
    let app = app(&concern, call, "");
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn supplying_include_must_precede_the_call() {
    let call = "positioned_within :book, association: :leaves, filter: :active";
    let app = app_with_prefix(POSITIONABLE, call, "", "");
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn nested_calls_unsigned_accessors_and_overridden_primitives_decline_expansion() {
    let concern = "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install(value)\n      define_method :chosen do\n        value\n      end\n    end\n  end\nend\n";
    for body in [
        "install :first\ninstall :second if true",
        "attribute :chosen, :string\ninstall :first",
        "def self.define_method(name)\n  :ignored\nend\ninstall :first",
    ] {
        let app = app(concern, body, "");
        assert!(instances(&app, "Leaf").is_empty(), "{body}");
        assert!(unexpanded(&app, "install") >= 1, "{body}");
    }
    let concern = concern.replace(
        "    def install",
        "    def define_method(name)\n      :ignored\n    end\n    def install",
    );
    let app = app(&concern, "install :first", "");
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "install"), 1);
}

#[test]
fn erased_splats_in_definition_and_visibility_calls_are_not_specialized() {
    for primitive in [
        "define_method(**name) do\n        :value\n      end",
        "define_method(name) do\n        :value\n      end\n      private(**name)",
        "define_method(name) do\n        send(**name)\n      end",
        "define_method(name) do\n        def self.extra\n          :ok\n        end\n      end",
        "define_method(name) do\n        defined?(name)\n      end",
    ] {
        let concern = format!(
            "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install(name)\n      {primitive}\n    end\n  end\nend\n"
        );
        let app = app(&concern, "install :chosen", "");
        assert!(instances(&app, "Leaf").is_empty(), "{primitive}");
        assert_eq!(unexpanded(&app, "install"), 1);
    }
    // Bound-local definition/visibility names still have a positive path.
    let concern = "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install(name)\n      define_method(name) do\n        :value\n      end\n      protected(name)\n    end\n  end\nend\n";
    let app = app(concern, "install :chosen", "");
    assert_eq!(
        instances(&app, "Leaf")[0].visibility,
        MethodVisibility::Protected
    );
}

#[test]
fn original_signature_and_argument_syntax_are_not_erased_into_support() {
    for (signature, call) in [
        ("(target, *)", "install :title"),
        ("((target))", "install :title"),
        ("(target)", "install(**:title)"),
        ("(target: :title, **)", "install target: :title"),
    ] {
        let concern = format!(
            "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install{signature}\n      define_method :chosen do\n        send(target)\n      end\n    end\n  end\nend\n"
        );
        let app = app(&concern, call, "");
        assert!(instances(&app, "Leaf").is_empty(), "{signature}: {call}");
        assert_eq!(unexpanded(&app, "install"), 1);
    }
}

#[test]
fn lexical_constants_and_framework_api_collisions_stay_unsupported() {
    let call = "positioned_within :book, association: :leaves, filter: :active";
    let concern = POSITIONABLE.replace("send(parent)", "String");
    let app = app(&concern, &format!("String = :shadow\n{call}"), "");
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
    let app = self::app(
        &POSITIONABLE.replace("positioning_parent", "save"),
        call,
        "",
    );
    assert!(instances(&app, "Leaf").is_empty());
    assert_eq!(unexpanded(&app, "positioned_within"), 1);
}

#[test]
fn all_ruby_constructor_hooks_are_implicitly_private() {
    for hook in [
        "initialize",
        "initialize_copy",
        "initialize_dup",
        "initialize_clone",
    ] {
        let concern = format!(
            "module Positionable\n  extend ActiveSupport::Concern\n  class_methods do\n    def install\n      define_method :{hook} do\n        :done\n      end\n    end\n  end\nend\n"
        );
        let app = app(&concern, "install", "");
        if hook == "initialize" {
            // The model already synthesizes initialize: the same
            // collision policy must apply to constructor definitions.
            assert_eq!(unexpanded(&app, "install"), 1);
            continue;
        }
        let method = instances(&app, "Leaf")
            .into_iter()
            .find(|m| m.name.as_str() == hook)
            .unwrap_or_else(|| panic!("missing {hook}"));
        assert_eq!(method.visibility, MethodVisibility::Private, "{hook}");
    }
}

#[test]
fn emitted_helpers_preserve_parent_active_scope_order_exclusion_and_privacy() {
    let run = emit_and_run::real_blog()
        .write("app/models/concerns/positionable.rb", POSITIONABLE)
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.integer \"position_score\", default: 0\n    t.boolean \"active\", default: true")
        .edit("app/models/comment.rb", "  belongs_to :article", r#"  belongs_to :article
  include Positionable
  scope :active, -> { where(active: true) }
  scope :positioned, -> { order(:position_score, :id) }
  positioned_within :article, filter: :active, association: :comments
  def sibling_ids
    other_positioned_siblings.ids
  end
  def sibling_ids_via_local
    siblings = all_positioned_siblings
    siblings.excluding(self).ids
  end
  def parent_id
    positioning_parent.id
  end
  def reflected_sibling_ids
    self.send(:other_positioned_siblings).ids
  end
  def public_reflected_siblings
    self.public_send(:other_positioned_siblings)
  end"#)
        .run_ruby(r#"
a = Article.create!(title: "Macro owner", body: "A sufficiently long article body")
b = Article.create!(title: "Different owner", body: "A sufficiently long article body")
make = ->(owner, score, active) { Comment.create!(article_id: owner.id, commenter: "Macro", body: "Text", position_score: score, active: active) }
later = make.call(a, 30, true)
self_record = make.call(a, 20, true)
earlier = make.call(a, 10, true)
inactive = make.call(a, 5, false)
foreign = make.call(b, 1, true)
raise "parent capture" unless self_record.parent_id == a.id
raise "scope/parent/order/exclusion" unless self_record.sibling_ids == [earlier.id, later.id]
raise "local Relation exclusion" unless self_record.sibling_ids_via_local == [earlier.id, later.id]
raise "compiled reflective wrapper" unless self_record.reflected_sibling_ids == [earlier.id, later.id]
begin
  self_record.public_reflected_siblings
  raise "compiled public_send exposed private helper"
rescue NoMethodError
end
raise "default respond_to?" if self_record.respond_to?(:other_positioned_siblings)
raise "include_private respond_to?" unless self_record.respond_to?(:other_positioned_siblings, true)
raise "send must work" unless self_record.send(:other_positioned_siblings).ids == [earlier.id, later.id]
begin
  self_record.public_send(:other_positioned_siblings)
  raise "public_send exposed private helper"
rescue NoMethodError
end
raise "later public wrapper" unless self_record.respond_to?(:sibling_ids)
puts "macro runtime parity passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("macro runtime parity passed"));
}

#[test]
fn emitted_positioning_runs_reorder_block_move_lock_and_rebalance() {
    let run = emit_and_run::real_blog()
        .write("app/models/concerns/positioning_concern.rb", POSITIONING_CONCERN)
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.float \"position_score\", default: 0.0, null: false\n    t.decimal \"decimal_score\", precision: 8, scale: 3\n    t.integer \"order\"\n    t.date \"archived_on\"\n    t.boolean \"active\", default: true, null: false\n    t.boolean \"featured\"")
        .edit("app/models/comment.rb", "  belongs_to :article", "  belongs_to :article\n  include PositioningConcern\n  positioned_within :article, association: :comments, filter: :active\n\n  def created_at\n    \"display timestamp\"\n  end\n\n  def minimum_created_at_iso8601\n    (Comment.where(article_id: article_id).minimum(:created_at) || Time.current).iso8601\n  end\n\n  def active_extrema_keys\n    Comment.where(article_id: article_id).group(:active).minimum(:position_score).keys\n  end")
        .run_ruby(r#"
article = Article.create!(title: "Positioning owner", body: "A sufficiently long article body")
other_article = Article.create!(title: "Other owner", body: "A sufficiently long article body")
make = ->(owner, score, active = true) { Comment.create!(article_id: owner.id, commenter: "Position", body: "Text", position_score: score, active: active) }
items = 4.times.map { |index| make.call(article, index + 1) }
siblings = ->(owner) { Comment.where(article_id: owner.id) }
raise "initial sibling order" unless siblings.call(article).positioned.ids == items.map(&:id)
raise "default score sequence: #{items.map(&:position_score).inspect}" unless items.map(&:position_score) == [1.0, 2.0, 3.0, 4.0]
inactive = make.call(article, 5, false)
projected = siblings.call(article).active.positioned.select(:id)
raise "minimum ignored the relation scope/projection" unless projected.minimum(:position_score) == 1.0
raise "maximum ignored the relation scope/projection" unless projected.maximum(:position_score) == 4.0
raise "grouped minimum" unless siblings.call(article).group(:article_id).minimum(:position_score) == { article.id => 1.0 }
raise "grouped maximum" unless siblings.call(article).group(:article_id).maximum(:position_score) == { article.id => 5.0 }
raise "composite grouped minimum" unless siblings.call(article).group(:article_id, :commenter).minimum(:position_score) == { [article.id, "Position"] => 1.0 }
raise "grouped having" unless siblings.call(article).group(:article_id).having("MIN(position_score) < 2").minimum(:position_score) == { article.id => 1.0 }
raise "grouped boolean keys" unless siblings.call(article).group(:active).minimum(:position_score) == { true => 1.0, false => 5.0 }
raise "composite boolean keys" unless siblings.call(article).group(:article_id, :active).minimum(:position_score) == { [article.id, true] => 1.0, [article.id, false] => 5.0 }
raise "grouped extrema ignores ordering" unless siblings.call(article).order(:position_score).group(:article_id).minimum(:position_score) == { article.id => 1.0 }
raise "nullable boolean group key" unless siblings.call(article).group(:featured).minimum(:position_score) == { nil => 1.0 }
raise "boolean extrema" unless siblings.call(article).minimum(:active) == false && siblings.call(article).maximum(:active) == true
items[0].update!(decimal_score: 2.125)
inactive.update!(decimal_score: 5.5)
raise "decimal extrema type/value" unless siblings.call(article).minimum(:decimal_score) == 2.125 && siblings.call(article).maximum(:decimal_score) == 5.5
raise "decimal extrema should be Float" unless siblings.call(article).minimum(:decimal_score).is_a?(Float)
raise "grouped decimal extrema type/value" unless siblings.call(article).group(:active).minimum(:decimal_score) == { true => 2.125, false => 5.5 }
items[0].update!(order: 7)
items[1].update!(order: 2)
raise "reserved aggregate column" unless siblings.call(article).minimum(:order) == 2 && siblings.call(article).maximum(:order) == 7
items[0].update!(archived_on: Date.iso8601("2024-03-04"))
items[1].update!(archived_on: Date.iso8601("2024-03-02"))
raise "date extrema type/value" unless siblings.call(article).minimum(:archived_on).iso8601 == "2024-03-02"
date_keys = siblings.call(article).group(:archived_on).minimum(:position_score).keys
raise "grouped date key type: #{date_keys.map { |key| [key, key.class] }.inspect}" unless date_keys.all? { |key| key.nil? || key.is_a?(Date) }
raise "time extrema type" unless siblings.call(article).minimum(:created_at).is_a?(Time)
raise "grouped time key type" unless siblings.call(article).group(:created_at).minimum(:position_score).keys.all? { |key| key.is_a?(Time) }
raise "schema-typed scalar extrema in app source" unless items[0].minimum_created_at_iso8601.is_a?(String)
raise "schema-typed grouped extrema in app source" unless items[0].active_extrema_keys.sort_by(&:to_s) == [false, true]
raise "quoted boolean group" unless siblings.call(article).group('"comments"."active"').minimum(:position_score) == { true => 1.0, false => 5.0 }
having_alias = siblings.call(article).select("COUNT(*) AS n").group(:article_id).having("n > 1")
raise "having select alias" unless having_alias.minimum(:position_score) == { article.id => 1.0 }
raise "aggregate ignores order/limit" unless siblings.call(article).order(:position_score).limit(1).maximum(:position_score) == 5.0
raise "aggregate offset should have no result" unless siblings.call(article).offset(1).minimum(:position_score).nil?
raise "empty minimum should be nil" unless siblings.call(article).where(id: -1).minimum(:position_score).nil?
begin
  siblings.call(article).minimum("position_score) FROM active_record_extreme; DROP TABLE comments; --")
  raise "aggregate accepted an SQL expression as a column"
rescue ArgumentError
end
foreign = make.call(other_article, 1)
items[3].move_to_position(0)
raise "move to first" unless siblings.call(article).active.positioned.ids == [items[3].id, items[0].id, items[1].id, items[2].id]
items[3].move_to_position(99)
raise "move beyond end" unless siblings.call(article).active.positioned.ids == [items[0].id, items[1].id, items[2].id, items[3].id]
items[0].move_to_position(1, [items[1], items[2]])
raise "contiguous block move" unless siblings.call(article).active.positioned.ids == [items[3].id, items[0].id, items[1].id, items[2].id]
raise "neighbors" unless items[1].previous.id == items[0].id && items[1].next.id == items[2].id
items[0].update!(position_score: 1e-11)
items[1].update!(position_score: 2e-11)
items[2].move_to_position(1)
raise "rebalance order" unless siblings.call(article).active.positioned.ids == [items[0].id, items[2].id, items[1].id, items[3].id]
raise "rebalance threshold" unless items[2].send(:rebalance_required?)
items[2].send(:rebalance_positions)
raise "rebalance scores: #{siblings.call(article).active.positioned.pluck(:position_score).inspect}" unless siblings.call(article).active.positioned.pluck(:position_score) == [1.0, 2.0, 3.0, 4.0]
raise "active sibling exclusion" unless siblings.call(article).active.positioned.count == 4 && !siblings.call(article).active.positioned.ids.include?(inactive.id)
raise "owner isolation" unless !siblings.call(article).active.positioned.ids.include?(foreign.id)
puts "positioning semantics passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("positioning semantics passed"));
}

#[test]
fn grouped_extrema_in_a_local_relation_do_not_get_a_scalar_type() {
    let (_, app, _) = emit_and_run::real_blog()
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.float \"position_score\", default: 0.0, null: false\n    t.boolean \"active\", default: true, null: false")
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article",
            "  belongs_to :article\n\n  def grouped_position_score_keys\n    grouped = Comment.where(article_id: article_id).group(:active)\n    grouped.minimum(:position_score).keys\n    Comment.where(article_id: article_id).group(:active).load.minimum(:position_score)\n    Comment.where(article_id: article_id).group(:active).load.maximum(:position_score)\n  end",
        )
        .emit_with_app(BuildTarget::Ruby);

    let method = model(&app, "Comment")
        .methods()
        .find(|method| method.name.as_str() == "grouped_position_score_keys")
        .expect("grouped extrema probe method ingested");
    let mut extrema_types = Vec::new();
    fn collect_extrema_types(expr: &roundhouse::expr::Expr, out: &mut Vec<Option<Ty>>) {
        if matches!(&*expr.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "minimum" | "maximum")) {
            out.push(expr.ty.clone());
        }
        expr.node.for_each_child(&mut |child| collect_extrema_types(child, out));
    }
    collect_extrema_types(&method.body, &mut extrema_types);
    assert_eq!(extrema_types.len(), 3, "expected local, minimum, and maximum calls");
    for ty in extrema_types {
        let has_schema_scalar_type = match &ty {
            Some(Ty::Int | Ty::Float | Ty::Time | Ty::Date | Ty::Str | Ty::Bool) => true,
            Some(Ty::Union { variants }) => variants
                .iter()
                .any(|ty| matches!(ty, Ty::Int | Ty::Float | Ty::Time | Ty::Date | Ty::Str | Ty::Bool)),
            _ => false,
        };
        assert!(
            !has_schema_scalar_type,
            "grouping hidden in a local or unrecognized refiner must not produce a scalar type: {ty:?}"
        );
    }
}
