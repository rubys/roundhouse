//! Core container tests preserve known element types and the recursive request
//! value contract; an arbitrary untyped input must keep its gradual escape.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{BodyTyper, ClassInfo, Ctx};
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ident::{ClassId, Symbol, TyVar, VarId};
use roundhouse::span::Span;
use roundhouse::ty::Ty;

fn param_value() -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from("Roundhouse::ParamValue")),
        args: vec![],
    }
}

fn narrowed_branches(input: Ty, path: &[&str], predicate: &str) -> (Ty, Ty) {
    let variable = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from("value"),
            },
        )
    };
    let mut expression = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(variable()),
                    method: Symbol::from(predicate),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Const {
                            path: path.iter().map(|part| Symbol::from(*part)).collect(),
                        },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
            then_branch: variable(),
            else_branch: variable(),
        },
    );
    let classes: HashMap<ClassId, ClassInfo> = HashMap::new();
    let mut context = Ctx::default();
    context.local_bindings.insert(Symbol::from("value"), input);
    BodyTyper::new(&classes).analyze_expr(&mut expression, &context);
    let ExprNode::If { then_branch, else_branch, .. } = &*expression.node else {
        unreachable!()
    };
    (
        then_branch.ty.clone().expect("typed true branch"),
        else_branch.ty.clone().expect("typed false branch"),
    )
}

fn narrowed(input: Ty, path: &[&str]) -> Ty {
    narrowed_branches(input, path, "is_a?").0
}

#[test]
fn core_array_and_hash_tests_retain_concrete_and_recursive_members() {
    let array = Ty::Array {
        elem: Box::new(Ty::Str),
    };
    let hash = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Int),
    };
    assert_eq!(narrowed(array.clone(), &["Array"]), array);
    assert_eq!(
        narrowed(
            Ty::Union {
                variants: vec![array.clone(), Ty::Nil]
            },
            &["", "Array"]
        ),
        array
    );
    assert_eq!(narrowed(hash.clone(), &["Hash"]), hash);
    assert_eq!(
        narrowed(
            Ty::Union {
                variants: vec![hash.clone(), Ty::Str]
            },
            &["Hash"]
        ),
        hash
    );
    assert_eq!(
        narrowed(param_value(), &["Array"]),
        Ty::Array {
            elem: Box::new(param_value())
        }
    );
    assert_eq!(
        narrowed(
            Ty::Union {
                variants: vec![param_value(), Ty::Nil]
            },
            &["Hash"]
        ),
        Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(param_value())
        }
    );
}

#[test]
fn open_container_members_stay_unresolved_beside_recursive_values() {
    let open_array = Ty::Array { elem: Box::new(Ty::Var { var: TyVar(0) }) };
    let open_hash = Ty::Hash {
        key: Box::new(Ty::Var { var: TyVar(1) }),
        value: Box::new(Ty::Var { var: TyVar(2) }),
    };
    assert_eq!(narrowed(open_array.clone(), &["Array"]), open_array);
    assert_eq!(narrowed(open_hash.clone(), &["Hash"]), open_hash);
    fn assert_contains(ty: &Ty, member: &Ty) {
        assert!(
            ty == member || matches!(ty, Ty::Union { variants } if variants.contains(member)),
            "member {member:?} was erased from {ty:?}",
        );
    }
    for variants in [
        vec![param_value(), open_array.clone()],
        vec![open_array, param_value()],
    ] {
        let Ty::Array { elem } = narrowed(Ty::Union { variants }, &["Array"]) else {
            panic!("expected narrowed Array");
        };
        assert_contains(&elem, &Ty::Var { var: TyVar(0) });
        assert_contains(&elem, &param_value());
    }
    for variants in [
        vec![param_value(), open_hash.clone()],
        vec![open_hash, param_value()],
    ] {
        let Ty::Hash { key, value } = narrowed(Ty::Union { variants }, &["Hash"]) else {
            panic!("expected narrowed Hash");
        };
        assert_contains(&key, &Ty::Var { var: TyVar(1) });
        assert_contains(&key, &Ty::Str);
        assert_contains(&value, &Ty::Var { var: TyVar(2) });
        assert_contains(&value, &param_value());
    }
}

#[test]
fn false_branches_remove_matching_parameterized_containers() {
    let array = Ty::Array { elem: Box::new(Ty::Str) };
    let hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Int) };
    for predicate in ["is_a?", "kind_of?", "instance_of?"] {
        assert_eq!(
            narrowed_branches(
                Ty::Union { variants: vec![array.clone(), hash.clone(), Ty::Nil] },
                &["Array"],
                predicate,
            ),
            (array.clone(), Ty::Union { variants: vec![hash.clone(), Ty::Nil] }),
        );
        assert_eq!(
            narrowed_branches(
                Ty::Union { variants: vec![array.clone(), hash.clone()] },
                &["Hash"],
                predicate,
            ),
            (hash.clone(), array.clone()),
        );
    }
}

#[test]
fn unknown_values_and_custom_container_names_do_not_gain_fake_member_types() {
    let gradual_array = Ty::Array {
        elem: Box::new(Ty::Untyped),
    };
    let gradual_hash = Ty::Hash {
        key: Box::new(Ty::Untyped),
        value: Box::new(Ty::Untyped),
    };
    assert_eq!(narrowed(Ty::Untyped, &["Array"]), gradual_array);
    assert_eq!(narrowed(Ty::Untyped, &["Hash"]), gradual_hash);
    let mixed = narrowed(
        Ty::Union {
            variants: vec![
                Ty::Array {
                    elem: Box::new(Ty::Str),
                },
                Ty::Untyped,
            ],
        },
        &["Array"],
    );
    assert!(
        format!("{mixed:?}").contains("Untyped"),
        "unknown variant was discarded: {mixed:?}"
    );
    let recursive_and_gradual = narrowed(
        Ty::Union {
            variants: vec![param_value(), gradual_array],
        },
        &["Array"],
    );
    assert!(format!("{recursive_and_gradual:?}").contains("Untyped"));
    assert_eq!(
        narrowed(param_value(), &["Other", "Array"]),
        Ty::Class {
            id: ClassId(Symbol::from("Other::Array")),
            args: vec![]
        }
    );
    assert_eq!(
        narrowed(param_value(), &["Other", "Hash"]),
        Ty::Class {
            id: ClassId(Symbol::from("Other::Hash")),
            args: vec![]
        }
    );
}

#[test]
fn recursive_request_container_blocks_retain_the_member_contract() {
    let methods = roundhouse::runtime_src::parse_methods_with_rbs(
        r#"module RecursiveProbe
  def self.array(value)
    if value.is_a?(Array)
      value.map { |item| item }
    else
      raise("not an Array")
    end
  end
  def self.hash(value)
    if value.is_a?(Hash)
      value.map { |key, item| item }
    else
      raise("not a Hash")
    end
  end
end
"#,
        r#"module RecursiveProbe
  def self.array: (Roundhouse::ParamValue) -> Array[Roundhouse::ParamValue]
  def self.hash: (Roundhouse::ParamValue) -> Array[Roundhouse::ParamValue]
end
"#,
    )
    .expect("recursive request accessor parses");
    let mut reads = 0;
    fn visit(expr: &Expr, reads: &mut usize) {
        if matches!(&*expr.node, ExprNode::Var { name, .. } if name.as_str() == "item") {
            assert_eq!(expr.ty, Some(param_value()), "item lost its contract: {expr:?}");
            *reads += 1;
        }
        expr.node.for_each_child(&mut |child| visit(child, reads));
    }
    for method in &methods {
        visit(&method.body, &mut reads);
    }
    assert!(reads >= 2, "recursive item reads were not inspected");
}

#[test]
fn arbitrary_runtime_array_items_remain_gradual_in_source_diagnostics() {
    let ruby = "class Probe < ApplicationRecord\n def first_item(value)\n if value.is_a?(Array)\n value.first\n else\n nil\n end\n end\nend";
    let rbs = "class Probe\n def first_item: (untyped value) -> untyped\nend";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n create_table :probes do |t|; t.string :name; end\nend"),
        ("app/models/probe.rb", ruby),
    ].into_iter().map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    // Use the runtime body typer and the standard diagnostic walker.
    let methods = roundhouse::runtime_src::parse_methods_with_rbs(ruby, rbs).unwrap();
    app.models[0].methods_mut().next().unwrap().body = methods[0].body.clone();
    fn assert_first_is_gradual(expr: &Expr, found: &mut bool) {
        if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "first") {
            assert!(
                format!("{:?}", expr.ty).contains("Untyped"),
                "Array member became falsely concrete: {expr:?}"
            );
            *found = true;
        }
        expr.node
            .for_each_child(&mut |child| assert_first_is_gradual(child, found));
    }
    let mut found = false;
    assert_first_is_gradual(&methods[0].body, &mut found);
    assert!(found, "Array member read was not inspected");
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::GradualUntyped { .. }
        )),
        "an arbitrary Array's member read must remain gradual: {diagnostics:?}"
    );
}

#[test]
fn source_resolved_unqualified_containers_keep_their_shadowing_class() {
    let source = r#"module ContainerNamespace
  class Array
  end
  class Hash
  end
  class Probe
    def self.array
      value = Array.new
      if value.is_a?(Array)
        value
      else
        nil
      end
    end
    def self.hash
      value = Hash.new
      if value.is_a?(Hash)
        value
      else
        nil
      end
    end
  end
end
"#;
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/services/container_namespace.rb", source),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest namespaced classes");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let probe = app.library_classes.iter()
        .find(|class| class.name.0.as_str() == "ContainerNamespace::Probe")
        .expect("namespaced probe");
    fn inspect(expr: &Expr, expected: &Ty, hits: &mut usize) {
        if let ExprNode::If { cond, then_branch, .. } = &*expr.node {
            let ExprNode::Send { args, .. } = &*cond.node else {
                panic!("expected class check");
            };
            assert_eq!(args[0].ty.as_ref(), Some(expected), "source class was not resolved");
            assert_eq!(then_branch.ty.as_ref(), Some(expected), "shadowed class became a core container");
            *hits += 1;
        }
        expr.node.for_each_child(&mut |child| inspect(child, expected, hits));
    }
    for (method, class) in [("array", "Array"), ("hash", "Hash")] {
        let body = &probe.methods.iter().find(|entry| entry.name.as_str() == method)
            .expect("probe method").body;
        let expected = Ty::Class {
            id: ClassId(Symbol::from(format!("ContainerNamespace::{class}"))),
            args: vec![],
        };
        let mut hits = 0;
        inspect(body, &expected, &mut hits);
        assert_eq!(hits, 1, "class check was not inspected");
    }
}
