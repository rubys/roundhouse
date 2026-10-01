//! The static part of Writebook's Positionable concern. Whole ordering,
//! locking and rebalance compatibility are separate runtime obligations.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodReceiver, MethodVisibility, ModelBodyItem};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey};

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
    ingest_app_from_tree(
        files
            .into_iter()
            .map(|(path, text)| (PathBuf::from(path), text.into_bytes()))
            .collect::<HashMap<_, _>>(),
    )
    .expect("ingest")
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
fn binds_keywords_by_name_and_keeps_each_includers_capture_and_privacy() {
    let app = app(
        POSITIONABLE,
        "  positioned_within :book, filter: :active, association: :leaves\n  def marker\n    :public_marker\n  end",
        "  include Positionable\n  positioned_within :shelf, association: :books, filter: :published",
    );
    assert_eq!(unexpanded(&app, "positioned_within"), 0);
    for (class, parent, association, filter) in [
        ("Leaf", ":book", ":leaves", ":active"),
        ("Book", ":shelf", ":books", ":published"),
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
        assert!(parent_body.contains(parent), "{class}: {parent_body}");
        let siblings_body = roundhouse::emit::ruby::emit_expr(
            &methods
                .iter()
                .find(|m| m.name.as_str() == "all_positioned_siblings")
                .unwrap()
                .body,
        );
        assert!(
            siblings_body.contains(association) && siblings_body.contains(filter),
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
    for (call, expected) in [("install", ":title"), ("install target: :id", ":id")] {
        let app = app(concern, call, "");
        let methods = instances(&app, "Leaf");
        let generated = methods
            .iter()
            .find(|m| m.name.as_str() == "chosen")
            .expect("expanded optional keyword");
        assert!(roundhouse::emit::ruby::emit_expr(&generated.body).contains(expected));
    }
    assert_eq!(unexpanded(&app(concern, "install :id", ""), "install"), 1);
}

#[test]
fn unsupported_captures_and_headers_decline_the_whole_macro() {
    for call in [
        "positioned_within :book, association: :leaves",
        "positioned_within :book, :shelf, association: :leaves, filter: :active",
        "positioned_within :book, association: :leaves, filter: :active, extra: :x",
        "positioned_within :book, association: 'leaves', filter: :active",
        "positioned_within choose_parent, association: :leaves, filter: :active",
    ] {
        survey::activate();
        let app = app(POSITIONABLE, call, "");
        let gaps = survey::drain();
        assert_eq!(unexpanded(&app, "positioned_within"), 1, "{call}");
        assert!(instances(&app, "Leaf").is_empty(), "{call}");
        assert!(
            gaps.iter().any(|g| g
                .to_string()
                .contains("model macro `positioned_within` not expanded")),
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
