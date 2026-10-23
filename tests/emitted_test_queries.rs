//! Queries written only in tests need the same lowering as app methods.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn widgets(model_body: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "app/models/widget.rb",
            &format!("class Widget < ApplicationRecord\n{model_body}\nend\n"),
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.integer "owner_id"
    t.string "name"
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
}

#[test]
fn emitted_test_group_count_returns_a_keyed_hash() {
    // A scope admits scope lowering independently of the test-only-query
    // regression below, so this isolates the grouped terminal rewrite.
    widgets("  scope :by_owner, -> { order(:owner_id) }")
        .write(
            "test/models/counts_test.rb",
            r#"require "test_helper"

class CountsTest < ActiveSupport::TestCase
  def test_counts
    Widget.create!(owner_id: 7, name: "first")
    Widget.create!(owner_id: 7, name: "second")
    Widget.create!(owner_id: 9, name: "third")
    assert_equal({7 => 2, 9 => 1}, Widget.group(:owner_id).count)
    query = Widget.where(name: "first")
    assert_equal({7 => 1}, query.group(:owner_id).count)
    assert_equal({7 => 2, 9 => 1}, Widget.group(:owner_id).order(:owner_id).count)
    assert_equal 3, Widget.count
  end
end
"#,
        )
        .run_test("test/models/counts_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_queries_lower_without_app_scopes_or_queries() {
    let run = widgets("")
        .write(
            "test/models/queries_test.rb",
            r#"require "test_helper"

class QueriesTest < ActiveSupport::TestCase
  def test_ordered_query
    Widget.create!(owner_id: 7, name: "kept")
    Widget.create!(owner_id: 9, name: "other")
    Widget.create!(owner_id: 8, name: "kept")
    assert_equal [8, 7], Widget.order(id: :desc).where(name: "kept").pluck(:owner_id)
    assert_equal [], Widget.order(:id).where(name: "absent").to_a
  end
end
"#,
        )
        .run_test("test/models/queries_test.rb");
    run.assert_passes();
    // CRuby's class-level query delegates can also run the unlowered
    // chain. Require the Relation seed that compiled test consumers need.
    let emitted = std::fs::read_to_string(run.emitted.join("test/models/queries_test.rb"))
        .expect("emitted query test");
    assert!(
        emitted.contains("ActiveRecord::Relation.new(Widget).order"),
        "test-only query did not lower to a Relation: {emitted}"
    );
}

#[test]
fn emitted_test_group_keeps_ordinary_ruby_count_dispatch() {
    widgets("")
        .write(
            "app/lib/grouper.rb",
            r#"class Grouper
  def group(kind)
    [4, 5]
  end
end
"#,
        )
        .write(
            "test/models/ordinary_group_test.rb",
            r#"require "test_helper"

class OrdinaryGroupTest < ActiveSupport::TestCase
  class InnerGrouper
    def group(kind)
      [6, 7, 8]
    end
    def count_groups
      group(:category).count
    end
  end
  def test_ordinary_count
    assert_equal 2, Grouper.new.group(:category).count
    grouper = Grouper.new
    assert_equal 2, grouper.group("category").count
    assert_equal 2, grouper.group(:category).sort.count
    assert_equal [2], [grouper].map { |item| item.group(:category).count }
    grouped = grouper.group(:category)
    assert_equal 2, grouped.count
    assert_equal 1, grouper.group(:category).count(4)
    assert_equal 1, grouper.group(:category).count { |item| item == 5 }
    assert_equal 3, InnerGrouper.new.group(:category).count
    assert_equal 3, InnerGrouper.new.count_groups
    query = Widget.where(name: "absent")
    assert_equal [2], [grouper].map { |query| query.group(:category).count }
    assert_equal({}, query.group(:owner_id).count)
  end
end
"#,
        )
        .run_test("test/models/ordinary_group_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_preserves_override_families_and_implicit_self() {
    // Cover each lowering family, including catalog aliases and the
    // operand shapes used by terminals and bulk insert expansion.
    let methods = [
        "order", "where", "limit", "left_joins", "order!", "all", "group",
        "pluck", "pick", "ids", "maximum", "minimum", "destroy_by", "delete_by",
        "first", "last", "insert_all", "insert_all!",
    ];
    let mut body = String::new();
    let mut assertions = String::new();
    for (index, method) in methods.iter().enumerate() {
        let value = index + 10;
        let incidental = if ["group", "all", "insert_all", "insert_all!"].contains(method) {
            "    ignored = limit(2)\n"
        } else { "" };
        body.push_str(&format!("  def self.{method}(argument = nil)\n{incidental}    [{value}, 2]\n  end\n"));
        let argument = match *method {
            "all" | "ids" => "",
            "where" | "destroy_by" | "delete_by" => "name: \"x\"",
            "limit" | "first" | "last" => "2",
            "insert_all" | "insert_all!" => "[{name: \"x\"}]",
            _ => ":id",
        };
        assertions.push_str(&format!("    assert_equal [{value}, 2], Widget.{method}({argument})\n"));
    }
    body.push_str(r#"  def self.bare_order
    order(:id)
  end
  def self.explicit_order
    self.order(:id)
  end
  def self.bare_all
    all
  end
  def self.constant_order
    Widget.order(:id)
  end
"#);
    assertions.push_str(r#"    assert_equal [10, 2], Widget.bare_order
    assert_equal [10, 2], Widget.explicit_order
    assert_equal [15, 2], Widget.bare_all
    assert_equal [10], Widget.bare_order.first(1)
    assert_equal [10], Widget.constant_order.first(1)
    values = Widget.bare_order
    assert_equal [2], values.last(1)
    assert_equal 2, Widget.group(:id).count
    model = Widget
    assert_equal [10, 2], model.order(:id)
"#);
    widgets(&body)
        .write("test/models/overrides_test.rb", &format!(r#"require "test_helper"
class OverridesTest < ActiveSupport::TestCase
  def test_overrides
{assertions}  end
end
"#))
        .run_test("test/models/overrides_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_preserves_an_inherited_model_override() {
    widgets("")
        .write(
            "app/models/application_record.rb",
            r#"class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
  def self.order(column)
    [6, 2]
  end
end
"#,
        )
        .write(
            "test/models/inherited_override_test.rb",
            r#"require "test_helper"
class InheritedOverrideTest < ActiveSupport::TestCase
  def test_inherited_override
    assert_equal [6, 2], Widget.order(:id)
    assert_equal 2, Widget.order(:id).count
  end
end
"#,
        )
        .run_test("test/models/inherited_override_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_preserves_singleton_method_aliases() {
    widgets(r#"  class << self
    def arrange(column)
      [12, 4]
    end
    alias_method :order, :arrange
    alias group arrange
    alias order! arrange
  end
"#)
        .write("test/models/aliases_test.rb", r#"require "test_helper"
class AliasesTest < ActiveSupport::TestCase
  def test_aliases
    assert_equal [12, 4], Widget.order(:id)
    assert_equal [12, 4], Widget.order!(:id)
    assert_equal [12, 4], Widget.group(:id)
    assert_equal 2, Widget.group(:id).count
  end
end
"#)
        .run_test("test/models/aliases_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_override_can_return_a_query_without_replacing_relation_builtins() {
    widgets(r#"  def self.order(column)
    where(owner_id: 7)
  end
  def self.via_bare
    order(:id)
  end
  def self.via_self
    self.order(:id)
  end
  scope :sorted, -> { order(:id) }
"#)
        .write("test/models/query_override_test.rb", r#"require "test_helper"
class QueryOverrideTest < ActiveSupport::TestCase
  def test_query_override
    Widget.create!(owner_id: 7, name: "first")
    Widget.create!(owner_id: 9, name: "second")
    Widget.create!(owner_id: 7, name: "third")
    assert_equal [7, 7], Widget.order(:id).pluck(:owner_id)
    assert_equal [7, 9, 7], Widget.all.order(:id).pluck(:owner_id)
    assert_equal [7, 7], Widget.via_bare.pluck(:owner_id)
    assert_equal [7, 7], Widget.via_self.pluck(:owner_id)
    assert_equal [], Widget.where(owner_id: 9).via_bare.pluck(:owner_id)
    assert_equal [], Widget.where(owner_id: 9).via_self.pluck(:owner_id)
    assert_equal({7 => 2}, Widget.order(:id).group(:owner_id).count)
    query = Widget.order(:id)
    assert_equal({7 => 2}, query.group(:owner_id).count)
    assert_equal [7, 9, 7], Widget.sorted.pluck(:owner_id)
  end
end
"#)
        .run_test("test/models/query_override_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_association_uses_relation_builders_before_model_overrides() {
    widgets(r#"  def self.order(column)
    where(name: "kept")
  end
"#)
        .write("app/models/owner.rb", "class Owner < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "owners", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "owner_id"
    t.string "name"
  end
end
"#)
        .write("test/models/association_override_test.rb", r#"require "test_helper"
class AssociationOverrideTest < ActiveSupport::TestCase
  def test_association_override
    owner = Owner.create!(name: "first")
    other = Owner.create!(name: "second")
    Widget.create!(owner_id: owner.id, name: "kept")
    Widget.create!(owner_id: owner.id, name: "other")
    Widget.create!(owner_id: other.id, name: "other")
    assert_equal ["kept"], Widget.order(:id).pluck(:name)
    assert_equal ["kept", "other"], owner.widgets.order(:id).pluck(:name)
    query = owner.widgets
    assert_equal ["kept", "other"], query.order(:id).pluck(:name)
  end
end
"#)
        .run_test("test/models/association_override_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_keeps_model_override_dispatch() {
    widgets(
        r#"  def self.order(column)
    [9, 8]
  end
  scope :from_override, -> { order(:id) }
"#,
    )
        .write(
            "test/models/override_test.rb",
            r#"require "test_helper"

class OverrideTest < ActiveSupport::TestCase
  def test_override
    assert_equal [9, 8], Widget.order(:id)
    assert_equal [], Widget.from_override.to_a
    assert_equal({}, Widget.from_override.group(:owner_id).count)
    assert_equal 2, Widget.order(:id).count
    values = Widget.order(:id)
    assert_equal [9, 8], values
    assert_equal [[9, 8]], [1].map { |item| Widget.order(:id) }
  end
end
"#,
        )
        .run_test("test/models/override_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_preserves_extended_module_override_families() {
    let names = ["order", "where", "limit", "left_joins", "order!", "all", "group",
        "pluck", "pick", "ids", "maximum", "minimum", "destroy_by", "delete_by",
        "first", "last", "insert_all", "insert_all!"];
    let mut source = String::from("module QueryOverrides\n");
    let mut assertions = String::new();
    let mut helpers = String::from("  extend QueryOverrides\n");
    for (index, name) in names.iter().enumerate() {
        let value = index + 10;
        source.push_str(&format!("  def {name}(argument = nil)\n    [{value}, 2]\n  end\n"));
        let argument = match *name {
            "all" | "ids" => "",
            "where" | "destroy_by" | "delete_by" => "name: \"x\"",
            "limit" | "first" | "last" => "2",
            "insert_all" | "insert_all!" => "[{name: \"x\"}]",
            _ => ":id",
        };
        assertions.push_str(&format!("    assert_equal [{value}, 2], Widget.{name}({argument})\n"));
        for (spelling, receiver) in [("bare", ""), ("explicit", "self."), ("constant", "Widget.")] {
            helpers.push_str(&format!("  def self.{spelling}_{index}\n    {receiver}{name}({argument})\n  end\n"));
            assertions.push_str(&format!("    assert_equal [{value}, 2], Widget.{spelling}_{index}\n"));
        }
    }
    source.push_str("  alias_method :reorder, :order\n  alias rewhere where\nend\n");
    assertions.push_str(r#"    assert_equal [10, 2], Widget.reorder(:id)
    assert_equal [11, 2], Widget.rewhere(name: "x")
    assert_equal [10, 2], Widget.bare_order
    assert_equal [10, 2], Widget.explicit_order
    assert_equal [16, 2], Widget.bare_group
    assert_equal 2, Widget.bare_group_count
    assert_equal 2, Widget.explicit_group_count
    assert_equal 2, Widget.constant_group_count
    assert_equal 2, Widget.group(:owner_id).count
    model = Widget
    assert_equal [10, 2], model.order(:id)
    assert_equal 2, model.group("owner_id").count
    assert_equal [2], [1].map { |item| Widget.group(:owner_id).count }
"#);
    helpers.push_str(r#"  def self.bare_order
    order(:id)
  end
  def self.explicit_order
    self.order(:id)
  end
  def self.bare_group
    group(:owner_id)
  end
  def self.bare_group_count
    group(:owner_id).count
  end
  def self.explicit_group_count
    self.group(:owner_id).count
  end
  def self.constant_group_count
    Widget.group(:owner_id).count
  end
"#);
    widgets(&helpers)
        .write("app/models/query_overrides.rb", &source)
        .write("test/models/extended_test.rb", &format!(r#"require "test_helper"
class ExtendedTest < ActiveSupport::TestCase
  def test_extended
{assertions}  end
end
"#))
        .run_test("test/models/extended_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_query_preserves_extension_lookup_forms() {
    for (body, parent, module) in [
        ("extend QueryOverrides", "", "module QueryOverrides\n include QueryImplementation\nend"),
        ("class << self\n include QueryOverrides\nend", "", "module QueryOverrides\n include QueryImplementation\nend"),
        ("", "extend QueryOverrides", "module QueryOverrides\n include QueryImplementation\nend"),
        ("extend ::QueryOverrides", "", "module QueryOverrides\n include QueryImplementation\nend"),
        ("extend Extensions::QueryOverrides", "", "module Extensions\n module QueryOverrides\n  include QueryImplementation\n end\nend"),
    ] {
        let run = widgets(body)
            .write("app/models/application_record.rb", &format!("class ApplicationRecord < ActiveRecord::Base\n primary_abstract_class\n {parent}\nend\n"))
            .write("app/models/query_implementation.rb", r#"module QueryImplementation
  def arrange(*columns)
    [9, 8]
  end
  alias_method :order, :arrange
  alias group arrange
end
"#)
            .write("app/models/query_overrides.rb", module)
            .write("test/models/extension_forms_test.rb", r#"require "test_helper"
class ExtensionFormsTest < ActiveSupport::TestCase
  def test_extensions
    assert_equal [9, 8], Widget.order(:id)
    assert_equal 2, Widget.group(:owner_id).count
    assert_equal 2, Widget.group("owner_id").count
    assert_equal 2, Widget.group(:owner_id) { |value| value }.count
    assert_equal 2, Widget.group.count
    assert_equal 2, Widget.group(:owner_id, :name).count
    assert_equal 1, Widget.group(:owner_id).count(9)
    assert_equal 1, Widget.group(:owner_id).count { |value| value == 8 }
    values = Widget.group(:owner_id)
    assert_equal 2, values.count
    model = Widget
    assert_equal 2, model.group(:owner_id).count
    assert_equal [], Widget.all.order(:id).to_a
    query = Widget.where(name: "absent")
    assert_equal({}, query.group(:owner_id).count)
  end
end
"#)
            .run_test("test/models/extension_forms_test.rb");
        run.assert_passes();
    }
}

#[test]
fn emitted_test_extended_query_returns_keep_relation_dispatch() {
    for query in ["Widget.where(owner_id: 7)", "where(owner_id: 7)", "self.where(owner_id: 7)"] {
    widgets("  extend QueryOverrides")
        .write("app/models/query_overrides.rb", &format!("module QueryOverrides\n\
  def order(column)\n    {query}\n  end\n\
  def group(column)\n    {query}\n  end\nend\n"))
        .write("test/models/extended_query_test.rb", r#"require "test_helper"
class ExtendedQueryTest < ActiveSupport::TestCase
  def test_queries
    Widget.create!(owner_id: 7, name: "first")
    Widget.create!(owner_id: 9, name: "other")
    Widget.create!(owner_id: 7, name: "third")
    assert_equal [7, 7], Widget.order(:id).pluck(:owner_id)
    assert_equal [7, 9, 7], Widget.all.order(:id).pluck(:owner_id)
    assert_equal 2, Widget.group(:ignored).count
    assert_equal({7 => 2}, Widget.group(:ignored).group(:owner_id).count)
    query = Widget.group(:ignored)
    assert_equal({7 => 2}, query.group(:owner_id).count)
  end
end
"#)
        .run_test("test/models/extended_query_test.rb")
        .assert_passes();
    }
}

#[test]
fn emitted_test_extension_does_not_import_module_singleton_methods() {
    widgets("  extend QueryOverrides")
        .write("app/models/query_overrides.rb", r#"module QueryOverrides
  def self.order(column)
    [9, 8]
  end
  def self.group(column)
    [9, 8]
  end
end
"#)
        .write("test/models/module_singletons_test.rb", r#"require "test_helper"
class ModuleSingletonsTest < ActiveSupport::TestCase
  def test_queries
    assert_equal [], Widget.order(:id).to_a
    assert_equal({}, Widget.group(:owner_id).count)
  end
end
"#)
        .run_test("test/models/module_singletons_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_instance_mixins_do_not_prove_class_query_dispatch() {
    for (body, parent) in [("include QueryGroups", ""), ("", "include QueryGroups")] {
        widgets(&format!("{body}\n  def groups\n    group(:owner_id).count\n  end\n"))
            .write("app/models/application_record.rb", &format!("class ApplicationRecord < ActiveRecord::Base\n primary_abstract_class\n {parent}\nend\n"))
            .write("app/models/query_groups.rb", r#"module QueryGroups
  def group(column)
    [9, 8]
  end
end
"#)
            .write("test/models/instance_mixin_test.rb", r#"require "test_helper"
class InstanceMixinTest < ActiveSupport::TestCase
  def test_groups
    assert_equal 2, Widget.new.groups
    assert_equal 2, Widget.new.group(:owner_id).count
    assert_equal({}, Widget.group(:owner_id).count)
  end
end
"#)
            .run_test("test/models/instance_mixin_test.rb")
            .assert_passes();
    }
}

#[test]
fn emitted_test_extended_overrides_leave_association_builtins_intact() {
    widgets("  extend QueryOverrides")
        .write("app/models/query_overrides.rb", r#"module QueryOverrides
  def order(column)
    [9, 8]
  end
  def group(column)
    [9, 8]
  end
end
"#)
        .write("app/models/owner.rb", "class Owner < ApplicationRecord\n has_many :widgets\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "owners", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "owner_id"
    t.string "name"
  end
end
"#)
        .write("test/models/extended_association_test.rb", r#"require "test_helper"
class ExtendedAssociationTest < ActiveSupport::TestCase
  def test_queries
    owner = Owner.create!(name: "first")
    other = Owner.create!(name: "second")
    Widget.create!(owner_id: owner.id, name: "kept")
    Widget.create!(owner_id: owner.id, name: "other")
    Widget.create!(owner_id: other.id, name: "other")
    assert_equal [9, 8], Widget.order(:id)
    assert_equal 2, Widget.group(:owner_id).count
    assert_equal ["kept", "other"], owner.widgets.order(:id).pluck(:name)
    assert_equal({owner.id => 2}, owner.widgets.group(:owner_id).count)
    query = owner.widgets
    assert_equal({owner.id => 2}, query.group(:owner_id).count)
    relation = ActiveRecord::Relation.new(Widget)
    assert_equal({owner.id => 2, other.id => 1}, relation.group(:owner_id).count)
  end
end
"#)
        .run_test("test/models/extended_association_test.rb")
        .assert_passes();
}
