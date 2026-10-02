//! Runtime RBS block contracts survive return-only registry seeds.
use std::collections::HashMap;
use std::process::Command;

use roundhouse::analyze::ClassInfo;
use roundhouse::dialect::MethodDef;
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::runtime_src::parse_methods_with_rbs_in_ctx;
use roundhouse::ty::Ty;

#[path = "support/runtime_block_signature.rs"]
mod runtime_block_signature;

use runtime_block_signature::{RBS, RUBY};

fn method<'a>(methods: &'a [MethodDef], name: &str) -> &'a MethodDef {
    methods.iter().find(|m| m.name.as_str() == name).unwrap()
}

fn assert_no_inference_gaps(expr: &Expr) {
    assert!(
        !matches!(expr.ty, None | Some(Ty::Var { .. })),
        "inference gap: {expr:?}"
    );
    expr.node.for_each_child(&mut assert_no_inference_gaps);
}

fn local_types(expr: &Expr, name: &str) -> Vec<Ty> {
    let mut types = Vec::new();
    if matches!(&*expr.node, ExprNode::Var { name: local, .. } if local.as_str() == name) {
        types.push(expr.ty.clone().expect("local type"));
    }
    expr.node
        .for_each_child(&mut |child| types.extend(local_types(child, name)));
    types
}

fn assert_local_type(expr: &Expr, name: &str, expected: Ty) {
    let types = local_types(expr, name);
    assert!(!types.is_empty(), "missing local {name}");
    assert!(types.iter().all(|ty| *ty == expected), "{name}: {types:?}");
}

fn assert_emitted_ruby(methods: &[MethodDef], class: &str, assertion: &str) {
    let defs: String = methods
        .iter()
        .map(roundhouse::emit::ruby::emit_method)
        .collect();
    let emitted = format!("class {class}\n{defs}\nend\n{assertion}\n");
    let output = Command::new("ruby")
        .arg("-e")
        .arg(&emitted)
        .output()
        .expect("ruby");
    assert!(
        output.status.success(),
        "emitted:\n{emitted}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn single_array_block_signature_binds_elements_without_mutating_the_registry() {
    let batch_id = ClassId(Symbol::from("Batch"));
    let mut batch = ClassInfo::default();
    batch.instance_methods.insert(Symbol::from("rows"), Ty::Nil);
    let classes = HashMap::from([(batch_id.clone(), batch)]);
    let methods = parse_methods_with_rbs_in_ctx(RUBY, RBS, &classes).expect("runtime parses");
    let consume = method(&methods, "consume");
    assert_eq!(consume.body.ty, Some(Ty::Int));
    assert_no_inference_gaps(&consume.body);
    assert_local_type(
        &consume.body,
        "items",
        Ty::Array {
            elem: Box::new(Ty::Int),
        },
    );
    assert_local_type(&consume.body, "item", Ty::Int);
    assert_eq!(
        classes[&batch_id].instance_methods[&Symbol::from("rows")],
        Ty::Nil
    );
    assert_emitted_ruby(
        &methods,
        "Batch",
        "raise 'wrong sum' unless Batch.new.consume == 3",
    );
}

#[test]
fn class_side_blocks_keep_their_yield_type() {
    let ruby = r#"
class ClassBatch
  def self.rows
    yield [3, 4]
    nil
  end

  def self.consume
    total = 0
    rows { |items| items.each { |item| total = total + item } }
    total
  end
end
"#;
    let rbs = r#"
class ClassBatch
  def self.rows: () { (Array[Integer]) -> void } -> nil
  def self.consume: () -> Integer
end
"#;
    let batch_id = ClassId(Symbol::from("ClassBatch"));
    let mut batch = ClassInfo::default();
    batch.class_methods.insert(Symbol::from("rows"), Ty::Nil);
    let classes = HashMap::from([(batch_id.clone(), batch)]);
    let methods = parse_methods_with_rbs_in_ctx(ruby, rbs, &classes).expect("runtime parses");
    let consume = method(&methods, "consume");
    assert_eq!(consume.body.ty, Some(Ty::Int));
    assert_no_inference_gaps(&consume.body);
    assert_local_type(
        &consume.body,
        "items",
        Ty::Array {
            elem: Box::new(Ty::Int),
        },
    );
    assert_local_type(&consume.body, "item", Ty::Int);
    assert_eq!(
        classes[&batch_id].class_methods[&Symbol::from("rows")],
        Ty::Nil
    );
    assert_emitted_ruby(
        &methods,
        "ClassBatch",
        "raise 'wrong sum' unless ClassBatch.consume == 7",
    );
}

#[test]
fn zero_and_multiple_yield_parameters_keep_the_declared_block_arity() {
    let ruby = r#"
class Arity
  def none
    yield
    nil
  end

  def pair
    yield 3, "four"
    nil
  end

  def consume
    none { 1.to_s }
    pair { |number, word| number.to_s + word }
  end
end
"#;
    let rbs = r#"
class Arity
  def none: () { () -> void } -> nil
  def pair: () { (Integer, String) -> void } -> nil
  def consume: () -> nil
end
"#;
    let methods =
        parse_methods_with_rbs_in_ctx(ruby, rbs, &HashMap::new()).expect("runtime parses");
    let consume = method(&methods, "consume");
    assert_eq!(consume.body.ty, Some(Ty::Nil));
    assert_no_inference_gaps(&consume.body);
    assert_local_type(&consume.body, "number", Ty::Int);
    assert_local_type(&consume.body, "word", Ty::Str);
    assert_emitted_ruby(
        &methods,
        "Arity",
        "raise 'wrong return' unless Arity.new.consume.nil?",
    );
}

#[test]
fn non_block_methods_keep_cross_file_return_types() {
    let ruby = r#"
class RegistryProbe
  def rows
    yield [1]
    nil
  end

  def existing
    1
  end

  def consume
    rows { |items| items.each { |item| item.to_s } }
    value = existing
    foreign = Foreign.answer
    value + foreign.to_s
  end
end
"#;
    let rbs = r#"
class RegistryProbe
  def rows: () { (Array[Integer]) -> void } -> nil
  def existing: () -> Integer
  def consume: () -> String
end
"#;
    let probe_id = ClassId(Symbol::from("RegistryProbe"));
    let foreign_id = ClassId(Symbol::from("Foreign"));
    let parent = ClassId(Symbol::from("Parent"));
    let mut probe = ClassInfo::default();
    probe.instance_methods.insert(Symbol::from("rows"), Ty::Nil);
    probe
        .instance_methods
        .insert(Symbol::from("existing"), Ty::Str);
    probe.parent = Some(parent.clone());
    let mut foreign = ClassInfo::default();
    foreign
        .class_methods
        .insert(Symbol::from("answer"), Ty::Bool);
    let classes = HashMap::from([(probe_id.clone(), probe), (foreign_id.clone(), foreign)]);
    let methods = parse_methods_with_rbs_in_ctx(ruby, rbs, &classes).expect("runtime parses");
    let consume = method(&methods, "consume");
    assert_eq!(consume.body.ty, Some(Ty::Str));
    assert_no_inference_gaps(&consume.body);
    assert_local_type(&consume.body, "value", Ty::Str);
    assert_local_type(&consume.body, "foreign", Ty::Bool);
    assert_eq!(
        classes[&probe_id].instance_methods[&Symbol::from("existing")],
        Ty::Str
    );
    assert_eq!(classes[&probe_id].parent, Some(parent));
    assert_eq!(
        classes[&foreign_id].class_methods[&Symbol::from("answer")],
        Ty::Bool
    );
}
