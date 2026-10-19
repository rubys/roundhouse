use super::emit_and_run;
use roundhouse::analyze::Analyzer;
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const FILES: &[(&str, &str)] = &[
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    (
        "db/schema.rb",
        r#"ActiveRecord::Schema.define do
  create_table :owners do |t|
    t.string :name, null: false
  end
  create_table :widgets do |t|
    t.integer :owner_id, null: false
    t.integer :status, null: false
    t.string :name, null: false
  end
  create_table :memberships do |t|
    t.integer :owner_id, null: false
    t.integer :widget_id, null: false
  end
  create_table :keyed_widgets, primary_key: :identifier, id: :string do |t|
    t.string :name, null: false
  end
  create_table :labeled_widgets, id: false do |t|
    t.integer :id, null: false
    t.string :identifier, null: false
    t.string :name, null: false
  end
  create_table :ranked_items, id: false do |t|
    t.integer :id, null: false
    t.string :order, null: false
  end
  create_table :flagged_labels, primary_key: :identifier, id: :string do |t|
    t.integer :id, null: false
    t.string :name, null: false
  end
end
"#,
    ),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\nend\n",
    ),
    (
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::Base\nend\n",
    ),
    (
        "app/models/owner.rb",
        r#"class Owner < ApplicationRecord
  has_many :widgets
  has_many :memberships
  has_many :linked_widgets, through: :memberships, source: :widget

  def enum_labels
    widgets.pluck(:status)
  end

  def reader_names
    widgets.pluck(:name)
  end

  def reader_rows
    widgets.pluck(:status, :name)
  end

  def stored_statuses
    linked_widgets.pluck(:status)
  end

  def stored_names
    linked_widgets.pluck(:name)
  end

  def stored_rows
    linked_widgets.pluck(:status, :name)
  end
end
"#,
    ),
    (
        "app/models/widget.rb",
        r#"class Widget < ApplicationRecord
  belongs_to :owner
  enum :status, { active: 0, archived: 1 }

  def name
    7
  end
end
"#,
    ),
    (
        "app/models/membership.rb",
        "class Membership < ApplicationRecord\n  belongs_to :owner\n  belongs_to :widget\nend\n",
    ),
    (
        "app/models/keyed_widget.rb",
        "class KeyedWidget < ApplicationRecord\nend\n",
    ),
    (
        "app/models/labeled_widget.rb",
        "class LabeledWidget < ApplicationRecord\n  self.primary_key = \"identifier\"\nend\n",
    ),
    (
        "app/models/ranked_item.rb",
        "class RankedItem < ApplicationRecord\nend\n",
    ),
    (
        "app/models/flagged_label.rb",
        "class FlaggedLabel < ApplicationRecord\nend\n",
    ),
    (
        "app/models/key_reader.rb",
        r#"class KeyReader < ApplicationRecord
  def self.inlined_ids
    LabeledWidget.ids
  end

  def self.inlined_override
    SpecialWidget.pluck(:name)
  end

  def self.inlined_names
    Widget.where(name: "stored name").pluck(:name)
  end
end
"#,
    ),
    (
        "app/models/special_widget.rb",
        r#"class SpecialWidget < Widget
  def self.pluck(*columns)
    ["override"]
  end
end
"#,
    ),
    (
        "app/models/special_pick.rb",
        r#"class SpecialPick < Widget
  def self.pick(*columns)
    "override"
  end
end
"#,
    ),
    (
        "app/models/projection_probe.rb",
        r#"class ProjectionProbe
  def self.class_names
    Widget.pluck(:name)
  end

  def self.relation_names
    Widget.all.pluck(:name)
  end

  def self.key_ids
    KeyedWidget.pluck(:id)
  end

  def self.relation_key_ids
    KeyedWidget.all.pluck(:id)
  end

  def self.key_rows
    KeyedWidget.pluck(:id, :name)
  end

  def self.relation_key_rows
    KeyedWidget.where(name: "stored key").pluck(:id, :name)
  end

  def self.key_pick
    KeyedWidget.where(name: "stored key").pick(:id, :name)
  end

  def self.overridden_names
    SpecialWidget.pluck(:name)
  end

  def self.overridden_rows
    SpecialWidget.pluck(:id, :name)
  end

  def self.overridden_pick
    SpecialPick.pick(:name)
  end

  def self.relation_special_names
    SpecialWidget.all.pluck(:name)
  end

  def self.class_pick
    Widget.pick(:name)
  end

  def self.label_ids
    LabeledWidget.pluck(:id)
  end

  def self.label_rows
    LabeledWidget.pluck(:id, :name)
  end

  def self.label_all
    LabeledWidget.pluck
  end

  def self.label_keys
    LabeledWidget.ids
  end

  def self.orders
    RankedItem.pluck(:order, :id)
  end
end
"#,
    ),
];

fn parsed() -> roundhouse::App {
    ingest_app_from_tree(
        FILES
            .iter()
            .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
            .collect(),
    )
    .expect("ingest projection app")
}

fn method<'a>(app: &'a roundhouse::App, class: &str, name: &str) -> &'a Expr {
    if let Some(model) = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == class)
    {
        return &model
            .methods()
            .find(|method| method.name.as_str() == name)
            .unwrap()
            .body;
    }
    &app.library_classes
        .iter()
        .find(|model| model.name.0.as_str() == class)
        .unwrap()
        .methods
        .iter()
        .find(|method| method.name.as_str() == name)
        .unwrap()
        .body
}

fn array(elem: Ty) -> Ty {
    Ty::Array {
        elem: std::sync::Arc::new(elem),
    }
}

fn tuple(elems: Vec<Ty>) -> Ty {
    Ty::Tuple {
        elems: elems.into(),
    }
}

fn nilable(value: Ty) -> Ty {
    Ty::Union {
        variants: vec![value, Ty::Nil].into(),
    }
}

#[test]
fn materialized_readers_and_sql_projections_have_different_types() {
    let mut app = parsed();
    Analyzer::new(&app).analyze(&mut app);
    for (class, name, expected) in [
        ("Owner", "enum_labels", array(nilable(Ty::Str))),
        ("Owner", "reader_names", array(Ty::Int)),
        (
            "Owner",
            "reader_rows",
            array(tuple(vec![nilable(Ty::Str), Ty::Int])),
        ),
        ("Owner", "stored_statuses", array(Ty::Int)),
        ("Owner", "stored_names", array(Ty::Str)),
        ("Owner", "stored_rows", array(tuple(vec![Ty::Int, Ty::Str]))),
        ("ProjectionProbe", "class_names", array(Ty::Str)),
        ("ProjectionProbe", "relation_names", array(Ty::Str)),
        ("ProjectionProbe", "overridden_names", array(Ty::Str)),
        ("ProjectionProbe", "overridden_rows", array(Ty::Str)),
        ("ProjectionProbe", "overridden_pick", Ty::Str),
        ("ProjectionProbe", "relation_special_names", array(Ty::Str)),
        ("KeyReader", "inlined_ids", array(Ty::Str)),
        ("KeyReader", "inlined_override", array(Ty::Str)),
        ("ProjectionProbe", "class_pick", nilable(Ty::Str)),
        ("ProjectionProbe", "label_ids", array(Ty::Int)),
        ("ProjectionProbe", "label_keys", array(Ty::Str)),
        ("ProjectionProbe", "label_rows", array(tuple(vec![Ty::Int, Ty::Str]))),
        ("ProjectionProbe", "orders", array(tuple(vec![Ty::Str, Ty::Int]))),
    ] {
        assert_eq!(
            method(&app, class, name).ty.as_ref(),
            Some(&expected),
            "{class}#{name}"
        );
    }
}

#[test]
fn requested_id_alias_types_follow_the_custom_primary_key() {
    let mut app = parsed();
    Analyzer::new(&app).analyze(&mut app);
    for (name, expected) in [
        ("key_ids", array(Ty::Str)),
        ("relation_key_ids", array(Ty::Str)),
        ("key_rows", array(tuple(vec![Ty::Str, Ty::Str]))),
        ("relation_key_rows", array(tuple(vec![Ty::Str, Ty::Str]))),
        (
            "key_pick",
            Ty::Union {
                variants: vec![tuple(vec![Ty::Str, Ty::Str]), Ty::Nil].into(),
            },
        ),
    ] {
        assert_eq!(
            method(&app, "ProjectionProbe", name).ty.as_ref(),
            Some(&expected),
            "{name}"
        );
    }
}

#[test]
fn materialized_map_reads_keep_each_reader_type_and_through_keeps_sql() {
    let mut app = parsed();
    Analyzer::new(&app).analyze(&mut app);
    roundhouse::lower::assoc_pluck::apply_assoc_pluck_lowering(&mut app);
    let ExprNode::Send {
        method: map,
        block: Some(block),
        ..
    } = &*method(&app, "Owner", "reader_rows").node
    else {
        panic!("no map");
    };
    assert_eq!(map.as_str(), "map");
    let ExprNode::Lambda { body, .. } = &*block.node else {
        panic!("no block")
    };
    let ExprNode::Array { elements, .. } = &*body.node else {
        panic!("no tuple")
    };
    assert_eq!(
        elements
            .iter()
            .map(|field| field.ty.clone())
            .collect::<Vec<_>>(),
        vec![Some(nilable(Ty::Str)), Some(Ty::Int)]
    );
    assert_eq!(
        body.ty.as_ref(),
        Some(&tuple(vec![nilable(Ty::Str), Ty::Int]))
    );
    let ExprNode::Send {
        method: terminal, ..
    } = &*method(&app, "Owner", "stored_rows").node
    else {
        panic!("no terminal")
    };
    assert_eq!(terminal.as_str(), "pluck");
}

#[test]
fn emitted_values_match_reader_and_sql_projection_types() {
    let oracle = std::process::Command::new("ruby").args(["-e", r#"
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:keyed_widgets, primary_key: :identifier, id: :string) { |table| table.string :name }
end
class KeyedWidget < ActiveRecord::Base
end
ActiveRecord::Base.connection.execute("INSERT INTO keyed_widgets (identifier, name) VALUES ('key-17', 'stored key')")
key = KeyedWidget.primary_key
p [KeyedWidget.pluck(key), KeyedWidget.pluck(key, :name), KeyedWidget.pick(key, :name)]
"#]).output().expect("run Rails explicit-key oracle");
    assert!(
        oracle.status.success(),
        "Rails explicit-key oracle: {}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let expected = String::from_utf8(oracle.stdout).unwrap();
    let mut app = emit_and_run::empty_app();
    for (path, source) in FILES {
        app = app.write(path, source);
    }
    let script = format!("expected_keys = {}\n", expected.trim())
        + r##"
owner = Owner.create!(name: "owner")
ActiveRecord.adapter.execute_ddl("INSERT INTO widgets (id, owner_id, status, name) VALUES (11, #{owner.id}, 0, 'stored name')")
ActiveRecord.adapter.execute_ddl("INSERT INTO memberships (owner_id, widget_id) VALUES (#{owner.id}, 11)")
ActiveRecord.adapter.execute_ddl("INSERT INTO keyed_widgets (identifier, name) VALUES ('key-17', 'stored key')")
raise "enum reader" unless owner.enum_labels == ["active"]
raise "overridden reader" unless owner.reader_names == [7]
raise "reader rows" unless owner.reader_rows == [["active", 7]]
raise "through enum SQL" unless owner.stored_statuses == [0]
raise "through name SQL" unless owner.stored_names == ["stored name"]
raise "through row SQL" unless owner.stored_rows == [[0, "stored name"]]
raise "class names" unless ProjectionProbe.class_names == ["stored name"]
raise "relation names" unless ProjectionProbe.relation_names == ProjectionProbe.class_names
raise "schema primary key" unless KeyedWidget.primary_key == "identifier"
raise "requested key alias ids" unless ProjectionProbe.key_ids == expected_keys[0]
raise "requested relation key alias ids" unless ProjectionProbe.relation_key_ids == expected_keys[0]
raise "requested key alias rows" unless ProjectionProbe.key_rows == expected_keys[1]
raise "relation key rows" unless ProjectionProbe.relation_key_rows == ProjectionProbe.key_rows
raise "requested key alias pick" unless ProjectionProbe.key_pick == expected_keys[2]
raise "class override" unless ProjectionProbe.overridden_names == ["override"]
raise "class override rows" unless ProjectionProbe.overridden_rows == ["override"]
raise "class pick override" unless ProjectionProbe.overridden_pick == "override"
raise "class pick" unless ProjectionProbe.class_pick == "stored name"
ActiveRecord.adapter.execute_ddl("INSERT INTO labeled_widgets (id, identifier, name) VALUES (4, 'key-4', 'labeled')")
ActiveRecord.adapter.execute_ddl("INSERT INTO ranked_items (id, \"order\") VALUES (1, 'kept')")
raise "id column class" unless ProjectionProbe.label_ids == [4]
raise "id column rows" unless ProjectionProbe.label_rows == [[4, "labeled"]]
raise "bare pluck keeps id" unless ProjectionProbe.label_all == [[4, "key-4", "labeled"]]
raise "ids stays the key" unless ProjectionProbe.label_keys == ["key-4"]
loaded = LabeledWidget.all
loaded_rows = loaded.to_a
raise "id reader stays the column" unless loaded_rows[0].id == 4
raise "loaded ids stays the key" unless loaded.ids == ["key-4"]
ActiveRecord.adapter.execute_ddl("INSERT INTO flagged_labels (id, identifier, name) VALUES (4, 'key-4', 'flagged')")
flagged = FlaggedLabel.all
raise "flagged pluck id" unless flagged.pluck(:id) == [4]
flagged_rows = flagged.to_a
raise "flagged id reader stays the column" unless flagged_rows[0].id == 4
raise "flagged loaded ids stays the key" unless flagged.ids == ["key-4"]
raise "model method ids" unless KeyReader.inlined_ids == ["key-4"]
raise "model method override" unless KeyReader.inlined_override == ["override"]
raise "model method sql pluck" unless KeyReader.inlined_names == ["stored name"]
labeled = ActiveRecord::Relation.new(LabeledWidget)
raise "id column runtime" unless labeled.pluck(:id) == [4]
raise "bare runtime pluck keeps id" unless labeled.pluck == [[4, "key-4", "labeled"]]
raise "reserved column" unless ProjectionProbe.orders == [["kept", 1]]
raise "reserved runtime column" unless ActiveRecord::Relation.new(RankedItem).pluck(:order) == ["kept"]
raise "qualified reserved column" unless ActiveRecord::Relation.new(RankedItem).pluck(:"ranked_items.order") == ["kept"]
relation = ActiveRecord::Relation.new(KeyedWidget)
raise "requested runtime key alias id" unless relation.pluck(:id) == expected_keys[0]
raise "requested runtime key alias row" unless relation.pluck(:id, :name) == expected_keys[1]
raise "requested runtime key alias pick" unless relation.pick(:id, :name) == expected_keys[2]
other = Owner.create!(name: "unknown enum")
ActiveRecord.adapter.execute_ddl("INSERT INTO widgets (id, owner_id, status, name) VALUES (12, #{other.id}, 9, 'unknown enum')")
raise "nil enum reader" unless other.enum_labels == [nil]
puts "Reader, SQL, primary-key and class-override projections passed"
puts "Requested :id alias matches Rails explicit-key projections"
"##;
    app.run_ruby(&script).assert_passes();
}
