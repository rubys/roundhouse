use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const HELPER: &str = "params.require(:item).permit(:token, :parent_pty_session_id)";

fn files(body: &str, helper: &str) -> Vec<(&'static str, String)> {
    vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :token\n    t.integer :parent_pty_session_id\n  end\nend\n".into()),
        ("app/models/item.rb", "class Item < ApplicationRecord\nend\n".into()),
        ("app/controllers/items_controller.rb", format!(r#"class ItemsController < ActionController::API
  def create
    head :ok
  end

  private

  def lookup(key)
{body}
  end

  def item_params
    {helper}
  end
end
"#)),
        ("config/routes.rb", "Rails.application.routes.draw do\n  post \"/items\", to: \"items#create\"\nend\n".into()),
    ]
}

fn emitted(body: &str, helper: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = files(body, helper).into_iter()
        .map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers(&app).into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("models/item_params.rb"))
        .expect("item params").content
}

#[test]
fn lookup_demand_preserves_factory_projections_and_adds_only_used_membership() {
    let source = emitted("    item_params.key?(key)", HELPER);
    assert!(source.contains("Params.filtered_values(sub,"), "{source}");
    assert!(source.contains("def key?(key)"), "{source}");
    assert!(!source.contains("def include?(key)"), "{source}");
    assert!(source.contains("ItemParams.__rh_param_value(self.__rh_values, key.to_s)"), "{source}");
    assert!(source.contains("Params.scalar_provided(self.__rh_values, \"token\")"), "{source}");
    assert!(source.contains("Params.scalar(self.__rh_values, \"token\")"), "{source}");
    let ordinary = emitted("    item_params", HELPER);
    assert!(!ordinary.contains("__rh_permitted_values"), "{ordinary}");
}

#[test]
fn index_only_classes_keep_legacy_string_projections() {
    let source = emitted("    item_params[\"token\"]", HELPER);
    assert!(!source.contains("__rh_permitted_values"), "{source}");
    assert!(!source.contains("__rh_scalar_values"), "{source}");
    assert!(!source.contains("def key?(key)"), "{source}");
    assert!(!source.contains("def include?(key)"), "{source}");
}

#[test]
fn literal_index_reads_are_not_replaced_by_string_projections() {
    let tree: HashMap<PathBuf, Vec<u8>> = files(
        "    [item_params.key?(:token), item_params[:token], item_params[\"token\"]]", HELPER,
    ).into_iter().map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    let controller = &files.iter().find(|file| file.path.to_string_lossy()
        .ends_with("controllers/items_controller.rb")).expect("controller").content;
    assert!(controller.contains("self.item_params[:token]"), "{controller}");
    assert!(controller.contains("self.item_params[\"token\"]"), "{controller}");
    assert!(!controller.contains("self.item_params.token"), "{controller}");
}

#[test]
fn local_alias_demand_uses_the_binding_at_the_read() {
    let source = emitted("    p = item_params\n    q = p\n    q.include?(key)", HELPER);
    assert!(source.contains("def include?(key)"), "{source}");
    for body in [
        "    p = item_params\n    p = {}\n    p.key?(key)",
        "    if key\n      p = item_params\n    end\n    p.key?(key)",
        "    p = item_params\n    [{}].each do |p|\n      p.key?(key)\n    end",
        "    p = item_params\n    [1, 2].each do |i|\n      p.key?(key)\n      p = {}\n    end",
    ] {
        let source = emitted(body, HELPER);
        assert!(!source.contains("__rh_permitted_values"), "false params proof:\n{source}");
    }
}

#[test]
fn writes_and_merges_keep_the_lookup_boundary() {
    for (body, helper) in [
        ("    item_params.key?(key)", format!("{HELPER}.merge(token: \"server\")")),
        ("    item_params.key?(key)", format!("{HELPER}.reverse_merge(token: \"default\")")),
        ("    p = item_params\n    p.token = \"changed\"\n    p.key?(key)", HELPER.into()),
    ] {
        let source = emitted(body, &helper);
        assert!(!source.contains("__rh_permitted_values"), "unsupported mutable lookup for {helper}, {body}:\n{source}");
        assert!(!source.contains("def key?(key)"), "{source}");
    }
}

#[test]
fn uploaded_file_classes_do_not_use_the_json_scalar_map() {
    let tree: HashMap<PathBuf, Vec<u8>> = files("    item_params.key?(key)", HELPER).into_iter()
        .map(|(path, mut source)| {
            if path == "app/models/item.rb" {
                source = "class Item < ApplicationRecord\n  has_one_attached :token\nend\n".into();
            }
            (path.into(), source.into_bytes())
        }).collect();
    let app = ingest_app_from_tree(tree).expect("ingest file params");
    let mut specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    specs.mark_file_fields(&app.models);
    let classes = roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs);
    let params = classes.iter().find(|class| class.name.0.as_str() == "ItemParams").expect("params");
    assert!(!params.methods.iter().any(|method| method.name.as_str() == "key?"));
    assert!(!params.methods.iter().any(|method| method.name.as_str() == "__rh_permitted_values"));
}

#[test]
fn synthesized_lookup_helpers_have_typed_bodies() {
    fn assert_typed(expr: &roundhouse::expr::Expr) {
        assert!(matches!(expr.ty.as_ref(), Some(ty) if !matches!(ty,
            roundhouse::ty::Ty::Var { .. } | roundhouse::ty::Ty::Untyped)),
            "lookup helper contains an unresolved expression: {expr:?}");
        expr.node.for_each_child(&mut assert_typed);
    }
    let tree: HashMap<PathBuf, Vec<u8>> = files("    item_params.key?(key)", HELPER).into_iter()
        .map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    for ruby in [false, true] {
        let mut specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
        specs.carry_nested(ruby, false);
        let classes = roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs);
        let params = classes.iter().find(|class| class.name.0.as_str() == "ItemParams").expect("params");
        let reader = if ruby { "__rh_values" } else { "__rh_scalar_values" };
        let helpers: Vec<_> = params.methods.iter().filter(|method|
            method.name.as_str() == reader || method.name.as_str() == "__rh_param_value").collect();
        assert_eq!(helpers.len(), 2);
        for helper in helpers { assert_typed(&helper.body); }
    }
}

#[test]
fn emitted_lookup_matches_rails_presence_values_keys_and_fallbacks() {
    let body = r#"    permitted = item_params
    aliased = permitted
    [aliased.key?(key), aliased.include?(key), aliased[key],
     aliased.key?(key) ? aliased[key] : "saved",
     aliased.include?("parent_pty_session_id"), item_params[:token], item_params["token"]]
"#;
    let script = r#"
cases = {
  "absent" => {}, "nil" => {"token" => nil}, "false" => {"token" => false},
  "true" => {"token" => true}, "zero" => {"token" => 0}, "float" => {"token" => 2.5},
  "empty" => {"token" => ""}, "string" => {"token" => "value"},
  "hash" => {"token" => {"nested" => "rejected"}}, "array" => {"token" => ["rejected"]},
  "parent_nil" => {"parent_pty_session_id" => nil},
  "parent_empty" => {"parent_pty_session_id" => ""},
  "parent_false" => {"parent_pty_session_id" => false},
  "parent_zero" => {"parent_pty_session_id" => 0}
}
rows = []
cases.each do |label, values|
  raw = {"item" => {"unpermitted" => "outside"}.merge(values)}
  keys = [:token, "token", :parent_pty_session_id, "parent_pty_session_id", :unknown, "unknown", :unpermitted]
  keys.each do |key|
    rows << [label, key.to_s, probe(raw, key)]
  end
end
puts JSON.generate(rows)
"#;
    let emitted_probe = r#"
require_relative "app/controllers/items_controller"
def probe(raw, key)
  controller = ItemsController.new
  controller.params = raw
  controller.send(:lookup, key)
end
"#;
    let mut overlay = emit_and_run::empty_app();
    for (path, content) in files(body, HELPER) { overlay = overlay.write(path, &content); }
    let run = overlay.run_ruby(&format!("require 'json'\n{emitted_probe}\n{script}"));
    run.assert_passes();
    assert_eq!(run.stdout.trim(), include_str!("params_vectors/scalar_lookup.json").trim());
}
