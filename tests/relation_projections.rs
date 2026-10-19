use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

#[test]
fn literal_columns_type_each_projected_value() {
    let files = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name, null: false\n    t.string :note\n  end\nend\n",
        ),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  def name\n    7\n  end\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ActionController::Base\n  def pairs\n    @value = Widget.pluck(:id, :name, :note)\n  end\n  def first_pair\n    @value = Widget.where(name: \"one\").pick(:id, :name)\n  end\n  def class_first_pair\n    @value = Widget.pick(:id, :name)\n  end\n  def names\n    @value = Widget.where(name: \"one\").pluck(\"name\")\n  end\n  def mapping\n    @value = Widget.where(name: \"one\").pluck(:name, :id).to_h\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, body)| (PathBuf::from(path), body.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let array = |elem| Ty::Array {
        elem: std::sync::Arc::new(elem),
    };
    let tuple = |elems: Vec<_>| Ty::Tuple {
        elems: elems.into(),
    };
    let nilable = |ty| Ty::Union {
        variants: vec![ty, Ty::Nil].into(),
    };
    let expected = [
        (
            "pairs",
            array(tuple(vec![Ty::Int, Ty::Str, nilable(Ty::Str)])),
        ),
        ("first_pair", nilable(tuple(vec![Ty::Int, Ty::Str]))),
        ("class_first_pair", nilable(tuple(vec![Ty::Int, Ty::Str]))),
        ("names", array(Ty::Str)),
        (
            "mapping",
            Ty::Hash {
                key: std::sync::Arc::new(Ty::Str),
                value: std::sync::Arc::new(Ty::Int),
            },
        ),
    ];
    for (name, ty) in expected {
        let action = app.controllers[0]
            .actions()
            .find(|action| action.name.as_str() == name)
            .unwrap();
        assert_eq!(action.body.ty.as_ref(), Some(&ty), "{name}");
    }
}

#[test]
fn source_projection_overrides_keep_their_return_types() {
    let files = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name, null: false\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n  def self.pluck(*columns)\n    []\n  end\n  def self.pick(*columns)\n    7\n  end\nend\n",
        ),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  def self.pick(*columns)\n    \"leaf\"\n  end\nend\n",
        ),
        (
            "app/models/plain.rb",
            "class Plain < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ActionController::Base\n  def rows\n    @value = Plain.pluck(:id, :name)\n  end\n  def ancestor_pick\n    @value = Plain.pick(:name)\n  end\n  def first_row\n    @value = Widget.pick(:id, :name)\n  end\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(path, body)| (PathBuf::from(path), body.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    for (name, expected) in [
        (
            "rows",
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Untyped),
            },
        ),
        ("ancestor_pick", Ty::Int),
        ("first_row", Ty::Str),
    ] {
        let action = app.controllers[0]
            .actions()
            .find(|action| action.name.as_str() == name)
            .unwrap();
        assert_eq!(action.body.ty.as_ref(), Some(&expected), "{name}");
    }
}
