use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn files(helper: &str) -> Vec<(&'static str, String)> {
    vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :body\n  end\n  create_table :assignments do |t|\n    t.integer :start_within_seconds\n  end\nend\n".into()),
        ("app/models/item.rb", "class Item < ApplicationRecord\nend\n".into()),
        ("app/models/assignment.rb", "class Assignment < ApplicationRecord\nend\n".into()),
        ("config/routes.rb", "Rails.application.routes.draw do\n  post \"/items\", to: \"items#create\"\nend\n".into()),
        ("app/controllers/items_controller.rb", format!(r#"class ItemsController < ActionController::API
  def create
    head :ok
  end

  private

  def snapshot
    permitted = item_params
    aliased = permitted
    before = permitted.to_h
    excluded = permitted.except(:body, "assignment")
    compacted = permitted.compact
    present = permitted.key?(:assignment)
    indexed = permitted[:assignment]
    removed = permitted.delete(:assignment)
    again = permitted.delete("assignment")
    [before, excluded.to_h, compacted.to_h, present, indexed, removed, again,
     permitted.to_h, aliased.to_h, permitted.include?("assignment")]
  end

  def lookup
    permitted = item_params
    removed = permitted.delete(:assignment)
    aliased = removed
    aliased&.[](:start_within_seconds)
  end

  def attributes
    permitted = item_params
    permitted.delete(:assignment)
    permitted.to_attrs
  end

  def encoded
    permitted = item_params
    permitted.delete(:body)
    permitted.as_json
  end

  def item_params
    {helper}
  end
end
"#)),
    ]
}

fn overlay(helper: &str) -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::empty_app();
    for (path, source) in files(helper) { overlay = overlay.write(path, &source); }
    overlay
}

fn emitted(helper: &str) -> Vec<roundhouse::emit::EmittedFile> {
    let tree: HashMap<PathBuf, Vec<u8>> = files(helper).into_iter()
        .map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    ruby::emit_lowered_controllers(&app)
}

const PERMIT: &str = "params.require(:item).permit(:body, assignment: [:start_within_seconds])";
const EXPECT: &str = "params.expect(item: [:body, { assignment: [:start_within_seconds] }])";

#[test]
fn finite_nested_delete_is_backed_by_the_same_filtered_value_map() {
    let files = emitted(PERMIT);
    let params = &files.iter().find(|f| f.path.to_string_lossy()
        .ends_with("models/item_params.rb")).expect("params").content;
    assert!(params.contains("Params.filtered_values(sub,"), "{params}");
    assert!(params.contains("def delete(key)"), "{params}");
    assert!(params.contains("removed = values.delete(key.to_s)"), "{params}");
    assert!(params.contains("Params.scalar_provided("), "{params}");
    assert!(params.contains("def except(*keys)"), "{params}");
    assert!(params.contains("def compact"), "{params}");
    let controller = &files.iter().find(|f| f.path.to_string_lossy()
        .ends_with("controllers/items_controller.rb")).expect("controller").content;
    assert!(controller.contains("Params.nested_lookup(aliased,"), "{controller}");
}

#[test]
fn distinct_child_filters_and_permit_expect_forms_get_distinct_classes() {
    let mut source = files(PERMIT);
    let controller = source.iter_mut().find(|(path, _)| *path == "app/controllers/items_controller.rb").unwrap();
    controller.1 = controller.1.replace("  def item_params", &format!(r#"  def other_params
    params.require(:item).permit(:body, assignment: [:other])
  end

  def expected_params
    {EXPECT}
  end

  def item_params"#));
    let tree: HashMap<PathBuf, Vec<u8>> = source.into_iter()
        .map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    assert_eq!(specs.iter().count(), 3);
    let names: std::collections::HashSet<_> = specs.iter().map(|spec| spec.class_id.clone()).collect();
    assert_eq!(names.len(), 3);
}

#[test]
fn reassigned_conditional_and_shadowed_values_are_not_nested_params_proofs() {
    let original = "    permitted = item_params\n    removed = permitted.delete(:assignment)\n    aliased = removed\n    aliased&.[](:start_within_seconds)";
    for replacement in [
        "    removed = item_params.delete(:assignment)\n    removed = { start_within_seconds: 33 }\n    removed[:start_within_seconds]",
        "    if params[:choose]\n      removed = item_params.delete(:assignment)\n    end\n    removed[:start_within_seconds]",
        "    removed = item_params.delete(:assignment)\n    [{}].each do |removed|\n      removed[:start_within_seconds]\n    end",
        "    removed = item_params.delete(:assignment)\n    [1, 2].each do |index|\n      removed[:start_within_seconds]\n      removed = {}\n    end",
    ] {
        let mut source = files(PERMIT);
        let controller = source.iter_mut().find(|(path, _)| *path == "app/controllers/items_controller.rb").unwrap();
        assert!(controller.1.contains(original));
        controller.1 = controller.1.replace(original, replacement);
        let tree = source.into_iter().map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        let files = ruby::emit_lowered_controllers(&app);
        let controller = &files.iter().find(|file| file.path.to_string_lossy()
            .ends_with("controllers/items_controller.rb")).expect("controller").content;
        assert!(!controller.contains("Params.nested_lookup("), "false nested proof:\n{controller}");
    }
}

#[test]
fn unsupported_filters_and_block_delete_keep_an_explicit_boundary() {
    for (helper, block) in [
        ("params.require(:item).permit(:body, assignment: [:start_within_seconds, tags: []])", false),
        ("params.require(:item).permit(:body, assignment: { details: [:start_within_seconds] })", false),
        (PERMIT, true),
    ] {
        let mut source = files(helper);
        if block {
            let controller = source.iter_mut().find(|(path, _)| *path == "app/controllers/items_controller.rb").unwrap();
            controller.1 = controller.1.replace("removed = permitted.delete(:assignment)",
                "removed = permitted.delete(:assignment) { |key| key }");
        }
        let tree = source.into_iter().map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect();
        let app = ingest_app_from_tree(tree).expect("ingest");
        let (classes, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
            roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs)
        });
        assert!(diagnostics.iter().any(|d| d.message.contains("unsupported")), "{diagnostics:#?}");
        assert!(!classes.iter().flat_map(|class| &class.methods)
            .any(|method| method.name.as_str() == "delete"));
    }
}

#[test]
fn nested_lookup_preserves_nil_false_and_numbers_through_a_deleted_value_alias() {
    let run = overlay(PERMIT).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
rows = []
[nil, false, 0, 33, "33", ""].each do |value|
  controller = ItemsController.new
  controller.params = {"item" => {"body" => "kept", "assignment" => {"start_within_seconds" => value}}}
  rows << controller.send(:lookup)
end
missing = ItemsController.new
missing.params = {"item" => {"body" => "kept"}}
rows << missing.send(:lookup)
puts JSON.generate(rows)
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), r#"[null,false,0,33,"33","",null]"#);
}

#[test]
fn a_symbol_lookup_on_a_deleted_array_retains_the_array_index_error() {
    let run = overlay(PERMIT).run_ruby(r#"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"body" => "kept", "assignment" => [{"start_within_seconds" => 33}]}}
begin
  controller.send(:lookup)
  raise "Array accepted a Symbol index"
rescue TypeError
  puts "TypeError"
end
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "TypeError");
}

#[test]
fn deletion_updates_factory_projections_and_serialization_without_changing_a_copy() {
    let run = overlay(PERMIT).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"body" => "kept", "assignment" => {"start_within_seconds" => 33}}}
params = controller.send(:item_params)
copied = params.except(:assignment)
deleted = params.delete("body")
raise "deleted value" unless deleted == "kept"
raise "stale projection" unless params.body == "" && !params.body_provided
raise "copy mutated" unless copied.body == "kept" && copied.body_provided
params.delete(:assignment)
item = Item.from_params(params)
item.save!
raise "deleted field resurrected" unless item.body.nil?
original = Item.from_params(copied)
original.save!
raise "copy projection" unless original.body == "kept"
raise "to_attrs stale" unless controller.send(:attributes) == {body: "kept"}
raise "as_json stale" unless controller.send(:encoded) == {"assignment" => {"start_within_seconds" => 33}}
puts "projections, copies, to_attrs and as_json"
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "projections, copies, to_attrs and as_json");
}

#[test]
fn deleted_nested_values_feed_a_bounded_assignment_constructor_probe() {
    let run = overlay(PERMIT).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
cases = {
  "absent" => {}, "nil_parent" => {"assignment" => nil},
  "empty_child" => {"assignment" => {}},
  "nil_child" => {"assignment" => {"start_within_seconds" => nil}},
  "blank_child" => {"assignment" => {"start_within_seconds" => ""}},
  "int_child" => {"assignment" => {"start_within_seconds" => 33}},
  "string_child" => {"assignment" => {"start_within_seconds" => "33"}}
}
rows = cases.map do |label, values|
  controller = ItemsController.new
  controller.params = {"item" => {"body" => "kept"}.merge(values)}
  params = controller.send(:item_params)
  removed = params.delete(:assignment)
  raise "remaining nested key" if params.key?(:assignment)
  value = controller.send(:lookup)
  if removed
    assignment = Assignment.new(start_within_seconds: value.nil? ? nil : value.to_i)
    assignment.save!
    [label, value, Assignment.find(assignment.id).start_within_seconds]
  else
    [label, value, "not created"]
  end
end
puts JSON.generate(rows)
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), r#"[["absent",null,"not created"],["nil_parent",null,"not created"],["empty_child",null,null],["nil_child",null,null],["blank_child","",0],["int_child",33,33],["string_child","33",33]]"#);
}

#[test]
fn nested_model_assignment_reads_remaining_values_through_the_public_writer() {
    let run = overlay(PERMIT).write("app/models/item.rb", r#"class Item < ApplicationRecord
  def assignment=(value)
    self.body = value["start_within_seconds"].to_s
  end
end
"#).run_ruby(r#"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"body" => "kept", "assignment" => {"start_within_seconds" => 33}}}
params = controller.send(:item_params)
params.delete(:body)
with_nested = Item.from_params(params)
with_nested.save!
raise "nested writer did not receive its value" unless Item.find(with_nested.id).body == "33"
params = controller.send(:item_params)
params.delete(:assignment)
without_nested = Item.from_params(params)
without_nested.save!
raise "deleted nested value reached the writer" unless Item.find(without_nested.id).body == "kept"
puts "nested writer observes deletion"
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "nested writer observes deletion");
}

#[test]
fn carried_filter_forms_keep_their_values_and_live_projections_after_deletion() {
    for (helper, raw, expected) in [
        ("params.require(:item).permit(:body, assignment: [[:start_within_seconds]])",
         r#"[{"start_within_seconds":33,"outside":"rejected"}]"#,
         r#"[{"start_within_seconds":33}]"#),
        ("params.expect(item: [:body, { assignment: [[:start_within_seconds]] }])",
         r#"[{"start_within_seconds":33,"outside":"rejected"}]"#,
         r#"[{"start_within_seconds":33}]"#),
        ("params.require(:item).permit(:body, assignment: {})",
         r#"{"deeper":{"start_within_seconds":33}}"#,
         r#"{"deeper":{"start_within_seconds":33}}"#),
    ] {
        let run = overlay(helper).run_ruby(&format!(r#"
require "json"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {{"item" => {{"body" => "kept", "assignment" => JSON.parse({raw:?})}}}}
params = controller.send(:item_params)
copy = params.except(:body)
params.delete(:body)
expected = JSON.parse({expected:?})
raise "carried field lost" unless params.assignment == expected && params.assignment_provided
raise "serialized field lost" unless params.to_h == {{"assignment" => expected}}
raise "copy lost field" unless copy.to_h == params.to_h
raise "wrong deleted value" unless params.delete(:assignment) == expected
raise "stale nested projection" unless params.assignment.nil? && !params.assignment_provided
raise "copy changed" unless copy.assignment == expected && copy.assignment_provided
puts "carried value preserved"
"#));
        run.assert_passes();
        assert_eq!(run.stdout.trim(), "carried value preserved");
    }
}

#[test]
fn scalar_array_lookup_keeps_values_while_factory_projection_keeps_its_strings() {
    let run = overlay("params.require(:item).permit(:body, assignment: [])").run_ruby(r#"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"body" => "kept", "assignment" => [33, false, nil, "text"]}}
params = controller.send(:item_params)
raise "lookup changed scalar types" unless params[:assignment] == [33, false, nil, "text"]
raise "factory projection changed" unless params.assignment == ["33", "false", "", "text"]
copy = params.except(:body)
raise "delete changed scalar types" unless params.delete(:assignment) == [33, false, nil, "text"]
raise "stale list projection" unless params.assignment == [] && !params.assignment_provided
raise "copy changed" unless copy.assignment == ["33", "false", "", "text"]
puts "scalar list values and projections preserved"
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "scalar list values and projections preserved");
}

#[test]
fn serialized_nested_keys_follow_the_permit_list_order() {
    for helper in [
        "params.require(:item).permit(:body, z: [:value], a: [:value])",
        "params.require(:item).permit(:a, :body, z: [:value], a: [:value])",
    ] {
    let controller = r#"class ItemsController < ActionController::API
  def create
    head :ok
  end

  private

  def snapshot
    permitted = item_params
    permitted.delete(:unused)
    [permitted.to_h, permitted.except(:unused).to_h, permitted.compact.to_h,
     permitted.to_attrs, permitted.as_json]
  end

  def item_params
    params.require(:item).permit(:body, z: [:value], a: [:value])
  end
end
"#.replace("params.require(:item).permit(:body, z: [:value], a: [:value])", helper);
    let run = overlay(PERMIT).write("app/controllers/items_controller.rb", &controller).run_ruby(r#"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"a" => {"value" => 2}, "z" => {"value" => 1}, "body" => "kept"}}
controller.send(:snapshot).each do |values|
  raise "filter order changed" unless values.keys.map(&:to_s) == ["body", "z", "a"]
end
puts "nested filter order preserved"
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "nested filter order preserved");
    }
}

#[test]
fn deleted_nested_indices_match_recorded_rails_without_coercing_other_keys() {
    let controller = include_str!("params_vectors/nested_delete_indices.rb");
    let run = overlay(PERMIT).write("app/controllers/items_controller.rb", controller).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {
  "items" => [{"name" => "first", "outside" => "rejected"}, {"name" => "last"}],
  "labels" => [33, false], "settings" => {"0" => "zero", "value" => 44, "deeper" => {"0" => "deep zero", "value" => 55}}
}}
puts JSON.generate(controller.send(:indices))
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), include_str!("params_vectors/nested_delete_indices.json").trim());
}

#[test]
fn deleted_nested_runtime_keys_match_recorded_rails_for_arrays_and_hashes() {
    let controller = include_str!("params_vectors/nested_delete_indices.rb");
    let run = overlay(PERMIT).write("app/controllers/items_controller.rb", controller).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {
  "items" => [{"name" => "first", "outside" => "rejected"}, {"name" => "last"}],
  "labels" => [33, false], "settings" => {"0" => "zero", "value" => 44, "deeper" => {"0" => "deep zero", "value" => 55}}
}}
rows = [0, "0", :"0", "value", :value, nil, false, 0..0].map do |key|
  values = [:array_index, :scalar_array_index, :unknown_index].map do |method|
    begin
      controller.send(method, key)
    rescue TypeError => error
      ["error", error.class.name]
    end
  end
  [key.class.name, key.to_s, values]
end
puts JSON.generate(rows)
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), include_str!("params_vectors/nested_delete_runtime_keys.json").trim());
}

#[test]
fn typed_param_value_indices_normalize_only_runtime_string_and_symbol_keys() {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ident::{ClassId, Symbol, VarId};
    use roundhouse::span::Span;
    use roundhouse::ty::Ty;

    let span = Span::synthetic();
    let mut value = Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("value") });
    value.ty = Some(Ty::Class { id: ClassId(Symbol::from("Roundhouse::ParamValue")), args: vec![].into() });
    let key = Expr::new(span, ExprNode::Var { id: VarId(1), name: Symbol::from("key") });
    let read = Expr::new(span, ExprNode::Send {
        recv: Some(value), method: Symbol::from("[]"), args: vec![key],
        block: None, parenthesized: false,
    });
    let expression = ruby::emit_expr(&read);
    let run = overlay(PERMIT).run_ruby(&format!(r#"
require "json"
rows = [0, "0", :"0", "value", :value, nil, false, 0..0].map do |key|
  values = [[{{"name" => "first"}}, {{"name" => "last"}}], {{"0" => "zero", "value" => 44}}].map do |value|
    begin
      {expression}
    rescue TypeError => error
      ["error", error.class.name]
    end
  end
  [key.class.name, key.to_s, values]
end
puts JSON.generate(rows)
"#));
    run.assert_passes();
    let actual: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
    let recorded: serde_json::Value = serde_json::from_str(include_str!("params_vectors/nested_delete_runtime_keys.json")).unwrap();
    let expected: Vec<_> = recorded.as_array().unwrap().iter().map(|row|
        serde_json::json!([row[0], row[1], [row[2][0], row[2][2]]])).collect();
    assert_eq!(actual, serde_json::json!(expected));
}

#[test]
fn multiparameter_parts_remain_in_the_map_and_drive_live_factory_projections() {
    let run = overlay(PERMIT).run_ruby(r#"
require "json"
require_relative "app/controllers/items_controller"
def snapshot(params)
  item = Item.from_params(params)
  item.save!
  [params.to_h, Item.find(item.id).body, params.key?(:body), params.key?("body(1)")]
end
controller = ItemsController.new
controller.params = {"item" => {"body(1)" => "first", "body(2i)" => "7"}}
params = controller.send(:item_params)
raise "missing multiparameter metadata" unless params.rh_multiparameter_fields == ["body"] && !params.rh_multiparameter_unsupported
copied = params.except("body(1)")
compacted = params.compact
rows = [snapshot(params), snapshot(copied), snapshot(compacted)]
rows << [params.delete("body(1)"), snapshot(params)]
rows << [params.delete("body(2i)"), snapshot(params)]
raise "stale multiparameter metadata" unless params.rh_multiparameter_fields.empty? && !params.rh_multiparameter_unsupported && !params.body_provided
raise "copy metadata changed" unless compacted.rh_multiparameter_fields == ["body"]
rows << snapshot(compacted)
puts JSON.generate(rows)
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), include_str!("params_vectors/multiparameter_delete.json").trim());
}

#[test]
fn unsupported_multiparameter_parts_remain_refused_until_removed() {
    let run = overlay(PERMIT).run_ruby(r#"
require_relative "app/controllers/items_controller"
{"body(1)" => "say \"hi\"", "body(1f)" => "1.5", "body(2)" => nil, "body(3)" => false}.each do |key, value|
  controller = ItemsController.new
  controller.params = {"item" => {key => value}}
  params = controller.send(:item_params)
  raise "part lost" unless params.key?(key) && params.to_h == {key => value}
  raise "unsupported part accepted" unless params.rh_multiparameter_fields == ["body"] && params.rh_multiparameter_unsupported
  begin
    Item.from_params(params)
    raise "model accepted unsupported part"
  rescue RuntimeError => error
    raise unless error.message.include?("not modeled")
  end
  copied = params.except(key)
  raise "copy metadata stale" unless copied.rh_multiparameter_fields.empty? && !copied.rh_multiparameter_unsupported
  Item.from_params(copied)
  compacted = params.compact
  raise "compact metadata stale" unless compacted.rh_multiparameter_unsupported == !value.nil?
  raise "wrong part returned" unless params.delete(key) == value
  raise "delete metadata stale" unless params.rh_multiparameter_fields.empty? && !params.rh_multiparameter_unsupported
  Item.from_params(params)
end
puts "unsupported multiparameter parts remain guarded until removed"
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "unsupported multiparameter parts remain guarded until removed");
}

#[test]
fn generated_value_methods_have_fully_typed_bodies() {
    fn assert_typed(expr: &roundhouse::expr::Expr) {
        assert!(matches!(expr.ty.as_ref(), Some(ty) if !matches!(ty,
            roundhouse::ty::Ty::Var { .. } | roundhouse::ty::Ty::Untyped)), "{expr:#?}");
        expr.node.for_each_child(&mut assert_typed);
    }
    let tree = files(PERMIT).into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let mut specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    specs.carry_nested(true, false);
    for class in roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs) {
        for method in class.methods {
            if method.name.as_str() != "from_raw" { assert_typed(&method.body); }
        }
    }
}

#[test]
fn filtered_nested_values_delete_copies_and_serialization_match_recorded_rails() {
    let cases = include_str!("params_vectors/nested_delete_cases.json");
    for (helper, expected) in [(PERMIT, include_str!("params_vectors/nested_delete_permit.json")),
                              (EXPECT, include_str!("params_vectors/nested_delete_expect.json"))] {
        let run = overlay(helper).run_ruby(&format!(r#"
require "json"
require_relative "app/controllers/items_controller"
rows = JSON.parse({cases:?}).map do |label, values|
  controller = ItemsController.new
  controller.params = {{"item" => {{"body" => "kept", "outside" => "rejected"}}.merge(values)}}
  [label, controller.send(:snapshot)]
end
puts JSON.generate(rows)
"#));
        run.assert_passes();
        assert_eq!(run.stdout.trim(), expected.trim());
    }
}
