use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{Analyzer, Severity, diagnose};
use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ty::Ty;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn ingest(source: &str, action: &str, extra: Option<&str>) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        (PathBuf::from("app/services/owner.rb"), source.as_bytes().to_vec()),
        (PathBuf::from("app/controllers/probe_controller.rb"), format!(
            "class ProbeController < ApplicationController\n  def index\n    @result = {action}\n  end\nend\n"
        ).into_bytes()),
    ].into_iter().collect();
    if let Some(extra) = extra {
        tree.insert(
            PathBuf::from("app/services/other.rb"),
            extra.as_bytes().to_vec(),
        );
    }
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

fn analyzed(mut app: roundhouse::App, source: bool) -> (roundhouse::App, Analyzer) {
    let mut analyzer = if source {
        Analyzer::for_source(&app)
    } else {
        Analyzer::new(&app)
    };
    analyzer.analyze(&mut app);
    (app, analyzer)
}

fn errors(app: &roundhouse::App) -> Vec<roundhouse::diagnostic::Diagnostic> {
    diagnose(app)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect()
}

fn send_type(expr: &Expr, method: &str) -> Option<Ty> {
    if matches!(&*expr.node, ExprNode::Send { method: found, .. } if found.as_str() == method) {
        return expr.ty.clone();
    }
    let mut result = None;
    expr.node.for_each_child(&mut |child| {
        if result.is_none() {
            result = send_type(child, method);
        }
    });
    result
}

#[test]
fn retained_factories_have_exact_identity_parent_and_constructor() {
    let app = ingest(
        "class Owner\n  Plain = Class.new\n  Failure = Class.new(StandardError)\n  Empty = ::Class.new(Object) {}\nend\n",
        "Owner::Failure.new(\"message\").message",
        None,
    );
    let (main, main_analyzer) = analyzed(app.clone(), false);
    let (source, analyzer) = analyzed(app, true);
    // A blockless one-argument factory is a real class at ingest. No-arg
    // and empty-block factories stay check-only.
    assert!(
        main_analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Failure")))
    );
    assert!(
        !main_analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Plain")))
    );
    assert!(
        !main_analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Empty")))
    );
    assert!(errors(&main).is_empty(), "{:?}", errors(&main));
    assert!(errors(&source).is_empty(), "{:?}", errors(&source));
    for (name, parent) in [
        ("Plain", "Object"),
        ("Failure", "StandardError"),
        ("Empty", "Object"),
    ] {
        let id = ClassId(Symbol::from(format!("Owner::{name}")));
        assert_eq!(
            analyzer.class_registry()[&id].parent,
            Some(ClassId(Symbol::from(parent)))
        );
    }
    let action = source.controllers[0].actions().next().unwrap();
    assert_eq!(
        send_type(&action.body, "new"),
        Some(Ty::Class {
            id: ClassId(Symbol::from("Owner::Failure")),
            args: vec![].into()
        })
    );
    assert_eq!(send_type(&action.body, "message"), Some(Ty::Str));
}

#[test]
fn known_parent_methods_are_inherited_without_lifting_declarations() {
    let raw = ingest(
        "class Parent\n def code = 42\nend\nclass Owner\n Child = Class.new(Parent) {}\nend",
        "Owner::Child.new.code",
        None,
    );
    let (main, _) = analyzed(raw.clone(), false);
    assert!(!errors(&main).is_empty());
    let (source, analyzer) = analyzed(raw, true);
    assert!(errors(&source).is_empty(), "{:?}", errors(&source));
    let child = &analyzer.class_registry()[&ClassId(Symbol::from("Owner::Child"))];
    assert_eq!(child.parent, Some(ClassId(Symbol::from("Parent"))));
    assert!(child.instance_methods.is_empty());
    assert_eq!(
        send_type(
            &source.controllers[0].actions().next().unwrap().body,
            "code"
        ),
        Some(Ty::Int)
    );
}

#[test]
fn raise_and_rescue_share_the_existing_exception_surface() {
    let app = ingest(
        "class Owner\n  Failure = Class.new(StandardError)\n  Child = Class.new(Failure)\n  def self.run\n    raise Child, \"narrow\"\n  rescue Failure => error\n    error.message\n  end\nend\n",
        "Owner.run",
        None,
    );
    let (app, analyzer) = analyzed(app, true);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let owner = &analyzer.class_registry()[&ClassId(Symbol::from("Owner"))];
    let returned = owner.class_methods.get(&Symbol::from("run")).unwrap();
    let return_type = match returned {
        Ty::Fn { ret, .. } => ret.as_ref(),
        ty => ty,
    };
    assert_eq!(return_type, &Ty::Str);
    assert_eq!(
        analyzer.class_registry()[&ClassId(Symbol::from("Owner::Child"))].parent,
        Some(ClassId(Symbol::from("Owner::Failure")))
    );
}

#[test]
fn excluded_shapes_preserve_main_analysis_in_full() {
    for (source, extra) in [
        (
            "class Owner\n Failure = Class.new { def code = 1 }\nend",
            None,
        ),
        ("class Owner\n Failure = Class.new(parent)\nend", None),
        (
            "class Owner\n Failure = Class.new\n Failure = Class.new\nend",
            None,
        ),
        (
            "class Owner\n Failure = Class.new\nend",
            Some("class Owner::Failure\n def code = 1\nend"),
        ),
        (
            "class Owner\n Failure = Class.new\n Failure &&= nil\nend",
            None,
        ),
        (
            "class Owner\n Failure = Class.new\nend",
            Some("Owner::Failure += 1"),
        ),
        (
            "class Owner\n Failure = Class.new\n Failure ||= nil\nend",
            None,
        ),
        (
            "class Owner\n Failure = Class.new\nend",
            Some("class << Owner::Failure\n def code = 1\nend"),
        ),
        (
            "class Owner\n Failure = Class.new\nend",
            Some("def (Owner::Failure).code = 1"),
        ),
        (
            "module Override\n def new = Object\nend\nClass.prepend(Override)\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "class Owner\n Class = Object\n Failure = Class.new\nend",
            None,
        ),
        (
            "class Class; end\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "def Class.new = Object\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "Class.instance_eval { def new = Object }\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "Class.send(:define_method, :new) { Object }\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "def (Class).new = Object\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "class << (Class)\n def new = Object\nend\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "Core = Class\nclass << Core\n def new = Object\nend\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        ("class Owner\n Failure = Class.new(Math)\nend", None),
        (
            "class Module\n def self.new(*) = Object\nend\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "class Object\n def self.new(*) = 7\nend\nclass Owner\n Failure = Class.new(BasicObject)\nend",
            None,
        ),
        (
            "Object.const_set(:Class, Object)\nclass Owner\n Failure = Class.new\nend",
            None,
        ),
        (
            "module Parent; end\nclass Owner\n Failure = Class.new(Parent)\nend",
            None,
        ),
        ("class Owner\n Failure = Class.new(Class)\nend", None),
        (
            "class Owner\n Parent = 1\n Failure = Class.new(Parent)\nend",
            None,
        ),
        ("Failure = Class.new\nclass Owner; end", None),
        ("class Owner\n Failure ||= Class.new\nend", None),
        ("class Owner\n Failure = Class.new { |klass| }\nend", None),
        (
            "class Owner\n Failure = Class.new\nend\nOwner.const_set(:Failure, nil)",
            None,
        ),
    ] {
        let raw = ingest(source, "Owner::Failure", extra);
        let (main, main_analyzer) = analyzed(raw.clone(), false);
        let (source_app, analyzer) = analyzed(raw, true);
        assert_eq!(source_app, main, "{source}\n{extra:?}");
        let id = ClassId(Symbol::from("Owner::Failure"));
        assert_eq!(
            analyzer.class_registry().contains_key(&id),
            main_analyzer.class_registry().contains_key(&id),
            "{source}"
        );
    }
}

#[test]
fn newly_exposed_errors_are_kept_without_rolling_back_other_facts() {
    let raw = ingest(
        "class Owner\n Failure = Class.new(StandardError) {}\n Useful = Class.new(StandardError) {}\nend",
        "[Owner::Failure.new + 1, Owner::Useful.new.message]",
        None,
    );
    let (main, _) = analyzed(raw.clone(), false);
    let (source, analyzer) = analyzed(raw, true);
    assert!(
        errors(&main)
            .iter()
            .any(|error| error.code() == "unsupported")
    );
    assert!(
        errors(&source)
            .iter()
            .any(|error| error.code() == "incompatible_binop")
    );
    for name in ["Failure", "Useful"] {
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from(format!("Owner::{name}"))))
        );
    }
    assert_eq!(
        send_type(
            &source.controllers[0].actions().next().unwrap().body,
            "message"
        ),
        Some(Ty::Str)
    );
}

#[test]
fn unrelated_reflection_and_aliases_do_not_veto_a_factory() {
    for extra in [
        "class Other\n def self.probe(receiver, name)\n receiver.define_method(name) {}\n receiver.public_send(name)\n eval(name)\n end\nend",
        "class Other; end\nOther.class_eval('def code = 1')",
        "class Other; end\nOther.const_set(:Child, nil)",
        "class Other\n Child = 1\n Child &&= nil\nend",
        "Alias = Owner::Child",
        "Other = Object\nOther &&= nil",
        "Other = Class\nOther &&= nil",
        "Alias = Owner::Child\nAlias &&= nil",
        "module Unrelated\n Other = Object\nend\nUnrelated::Other &&= nil",
    ] {
        let raw = ingest(
            "class Owner\n Child = Class.new(StandardError)\nend",
            "Owner::Child.new.message",
            Some(extra),
        );
        let (source, analyzer) = analyzed(raw, true);
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{extra}"
        );
        assert_eq!(
            send_type(
                &source.controllers[0].actions().next().unwrap().body,
                "message"
            ),
            Some(Ty::Str),
            "{extra}"
        );
    }
}

#[test]
fn literal_mutations_are_scoped_to_the_factory_and_known_ancestry() {
    for extra in [
        "Owner::Child.class_eval('def code = 1')",
        "Owner::Child.public_send(method_name)",
        "Owner::Child.singleton_class.define_method(:code) { 7 }",
        "Alias = Owner::Child.singleton_class\nAlias.define_method(:code) { 7 }",
        "Parent.define_method(:code) { 7 }",
        "Parent.send(:define_method, :code) { 7 }",
        "Grandparent.module_eval { def code = 7 }",
        "Class.public_send(method_name)",
        "Class.singleton_class.define_method(:new) { Object }",
        "Owner.remove_const('Child')",
        "Owner.send(:remove_const, :Child)",
        "Alias = Parent\nAlias.class_eval('def code = 7')",
        "Alias = Owner\nAlias::Child &&= nil",
    ] {
        let raw = ingest(
            "class Grandparent; end\nclass Parent < Grandparent; end\nclass Owner\n Child = Class.new(Parent) {}\n Useful = Class.new(StandardError) {}\nend",
            "Owner::Child",
            Some(extra),
        );
        let (_, analyzer) = analyzed(raw, true);
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{extra}"
        );
        if !extra.starts_with("Class.") {
            assert!(
                analyzer
                    .class_registry()
                    .contains_key(&ClassId(Symbol::from("Owner::Useful"))),
                "{extra}"
            );
        }
    }
}

#[test]
fn superclass_facts_follow_main_for_constructors_and_external_parents() {
    for (parent, action) in [
        ("class Parent\n def self.new = 7\nend", "Owner::Child.new"),
        (
            "class Parent\n class << self\n def answer = 7\n end\nend",
            "Owner::Child.answer",
        ),
        (
            "class Parent < External::Base; end",
            "Owner::Child.new.unmodeled",
        ),
        ("", "Owner::Child.new.unmodeled"),
    ] {
        let parent_name = if parent.is_empty() {
            "External::Base"
        } else {
            "Parent"
        };
        let factory = format!("{parent}\nclass Owner\n Child = Class.new({parent_name})\nend");
        let ordinary = format!("{parent}\nclass Owner\n class Child < {parent_name}; end\nend");
        let (factory_app, factory_analyzer) = analyzed(ingest(&factory, action, None), true);
        let (ordinary_app, ordinary_analyzer) = analyzed(ingest(&ordinary, action, None), false);
        let id = ClassId(Symbol::from("Owner::Child"));
        assert_eq!(
            factory_analyzer.class_registry()[&id].parent,
            ordinary_analyzer.class_registry()[&id].parent,
            "{parent}"
        );
        assert_eq!(errors(&factory_app), errors(&ordinary_app), "{parent}");
        assert_eq!(
            factory_app.controllers[0].actions().next().unwrap().body.ty,
            ordinary_app.controllers[0]
                .actions()
                .next()
                .unwrap()
                .body
                .ty,
            "{parent}"
        );
    }
}

#[test]
fn inherited_hooks_exclude_factories_through_the_entire_ancestry() {
    for (declarations, parent, extra) in [
        (
            "class Parent; def self.inherited(child); child.name.split(\"::\"); end; end",
            "Parent",
            None,
        ),
        (
            "class Parent; NAMES = []; def self.inherited(subclass); NAMES << subclass.name; end; end",
            "Parent",
            None,
        ),
        (
            "class Parent; def self.inherited(child) = nil; end",
            "Parent",
            None,
        ),
        (
            "class Grandparent; def self.inherited(child) = nil; end; class Parent < Grandparent; end",
            "Parent",
            None,
        ),
        (
            "class Grandparent; class << self; def inherited(child) = nil; end; end; class Parent < Grandparent; end",
            "Parent",
            None,
        ),
        (
            "class Parent; end",
            "Parent",
            Some("class Parent; def self.inherited(child) = nil; end"),
        ),
        (
            "class Object; def self.inherited(child) = nil; end",
            "Object",
            None,
        ),
        (
            "class Grandparent; def self.inherited(child) = nil; end; class Owner; Parent = Class.new(Grandparent) {}; end",
            "Owner::Parent",
            None,
        ),
        // An included module's own hook can give its includer one.
        (
            "module Tracking; def self.included(base) = base.define_singleton_method(:inherited) { |child| child.name.split(\"::\") }; end; class Parent; include Tracking; end",
            "Parent",
            None,
        ),
        (
            "module Tracking; class << self; def append_features(base) = super; end; end; class Grandparent; include Tracking; end; class Parent < Grandparent; end",
            "Parent",
            None,
        ),
    ] {
        let factory = format!("{declarations}\nclass Owner; Child = Class.new({parent}) {{}}; end");
        let raw = ingest(&factory, "Owner::Child.new", extra);
        let (main, _) = analyzed(raw.clone(), false);
        let (source, analyzer) = analyzed(raw, true);
        assert_eq!(source, main, "{declarations}");
        assert!(!errors(&source).is_empty(), "{declarations}");
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{declarations}"
        );
    }
    let (source, analyzer) = analyzed(
        ingest(
            "class Owner; Child = Class.new; end",
            "Owner::Child.new",
            None,
        ),
        true,
    );
    assert!(errors(&source).is_empty());
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child")))
    );
}

#[test]
fn singleton_namespace_mutations_exclude_factories() {
    // A normal class can inherit a singleton's lexical class without an alias.
    let declarations = r#"
class Container
  class << self
    class Grandparent < StandardError; end
    Grandparent.define_singleton_method(:inherited) do |child|
      child.define_singleton_method(:new) { 7 } if child.name.nil?
    end
    class ::Parent < Grandparent; end
  end
end
class Owner
  Child = Class.new(Parent) {}
  Useful = Class.new(StandardError) {}
end
"#;
    let raw = ingest(declarations, "Owner::Child.new.message", None);
    let (main, _) = analyzed(raw.clone(), false);
    let (source, analyzer) = analyzed(raw, true);
    assert!(
        !analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child"))),
        "the singleton ancestor's inherited mutation must exclude the factory"
    );
    assert!(!errors(&source).is_empty());
    assert_eq!(errors(&source), errors(&main));
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Useful")))
    );

    let output = std::process::Command::new("ruby")
        .args([
            "-e",
            &format!("{declarations}\np Owner::Child.new\nOwner::Child.new.message"),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("NoMethodError"));
}

#[test]
fn ruby_inherited_hooks_observe_anonymous_factories_and_named_headers() {
    for (declarations, action) in [
        (
            "class Parent; def self.inherited(child); child.name.split(\"::\"); end; end",
            "Owner::Child.new",
        ),
        (
            "class Parent < StandardError; def self.inherited(child); child.define_singleton_method(:new) { 7 } if child.name.nil?; end; end",
            "Owner::Child.new.message",
        ),
        (
            "module Tracking; def self.included(base) = base.define_singleton_method(:inherited) { |child| child.name.split(\"::\") }; end; class Parent; include Tracking; end",
            "Owner::Child.new",
        ),
    ] {
        let factory =
            format!("{declarations}\nclass Owner; Child = Class.new(Parent) {{}}; end\n{action}");
        let ordinary =
            format!("{declarations}\nclass Owner; class Child < Parent; end; end\n{action}");
        let (source, analyzer) = analyzed(ingest(&factory, action, None), true);
        assert!(!errors(&source).is_empty());
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child")))
        );
        let (main, _) = analyzed(ingest(&ordinary, action, None), false);
        assert!(errors(&main).is_empty(), "{:?}", errors(&main));
        for (program, success) in [(&factory, false), (&ordinary, true)] {
            let output = std::process::Command::new("ruby")
                .args(["-e", program])
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                success,
                "{program}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    let registry =
        "class Parent; NAMES = []; def self.inherited(subclass); NAMES << subclass.name; end; end";
    for (declaration, expected) in [
        ("Child = Class.new(Parent)", "[nil]\n"),
        ("class Child < Parent; end", "[\"Owner::Child\"]\n"),
    ] {
        let program = format!("{registry}\nclass Owner; {declaration}; end\np Parent::NAMES");
        let output = std::process::Command::new("ruby")
            .args(["-e", &program])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
}

// Main treats core operations as built-ins rather than modeling every way an
// application can redefine them. These constructor redefinitions share that
// core-method-model limit; unlike superclass_limits, they are not equivalent
// runtime behavior of a factory and an ordinary class header.
const CORE_CONSTRUCTOR_LIMITS: &[&str] = &[
    "Core, Other = Class; Core.define_singleton_method(:new) { |*| 7 }",
    "Core, Other = *[Class, nil]; Core.define_singleton_method(:new) { |*| 7 }",
    "Class.method(:define_singleton_method).to_proc.call(:new) { 7 }",
    "Class.method(:define_singleton_method).curry.call(:new) { 7 }",
    "Module.instance_method(:define_method).bind_call(Class.singleton_class, :new) { 7 }",
    "module Replacement; refine Class do; def new(*) = 7; end; end; using Replacement",
    r#"class Module; class << self; undef :"n#{"ew"}"; end; end"#,
];

#[test]
fn core_constructor_redefinitions_remain_known_limits() {
    for mutation in CORE_CONSTRUCTOR_LIMITS {
        let factory = format!("{mutation}\nclass Owner; Child = Class.new(Object) {{}}; end");
        let raw = ingest(&factory, "Owner::Child.new", None);
        let (main, _) = analyzed(raw.clone(), false);
        assert!(!errors(&main).is_empty(), "{mutation}");
        let (source, analyzer) = analyzed(raw, true);
        assert!(
            errors(&source).is_empty(),
            "{mutation}: {:?}",
            errors(&source)
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{mutation}"
        );
        let ordinary = format!("{mutation}\nclass Owner; class Child < Object; end; end");
        let raw = ingest(&ordinary, "Owner::Child.new", None);
        let (main, _) = analyzed(raw.clone(), false);
        let (source, _) = analyzed(raw, true);
        assert!(errors(&main).is_empty(), "{mutation}: {:?}", errors(&main));
        assert_eq!(source, main, "{mutation}");
    }
}

#[test]
fn ruby_constructor_redefinitions_expose_the_known_limits() {
    for mutation in CORE_CONSTRUCTOR_LIMITS {
        for (declaration, success) in [
            ("Child = Class.new(Object)", false),
            ("class Child < Object; end", true),
        ] {
            let program = format!("{mutation}\nclass Owner; {declaration}; end\nOwner::Child.new");
            let output = std::process::Command::new("ruby")
                .args(["-e", &program])
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                success,
                "{program}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn lexical_const_set_of_class_shadows_only_later_factories() {
    for shadow in [
        "const_set(:Class, Object)",
        "self.const_set(:Class, Object)",
        "send(:const_set, :Class, Object)",
    ] {
        let refused = format!("class Owner\n  {shadow}\n  Failure = Class.new\nend");
        let (source, analyzer) = analyzed(ingest(&refused, "Owner::Failure.new", None), true);
        assert!(
            !errors(&source).is_empty(),
            "{shadow}: {:?}",
            errors(&source)
        );
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Failure"))),
            "{shadow}"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &format!("{refused}\nOwner::Failure.new")])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{shadow}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("NoMethodError"),
            "{shadow}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let reopened = "class Owner; end\nOwner.const_set(:Class, Object)\nclass Owner\n  Failure = Class.new\nend";
    let (source, analyzer) = analyzed(ingest(reopened, "Owner::Failure.new", None), true);
    assert!(!errors(&source).is_empty(), "{:?}", errors(&source));
    assert!(
        !analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Failure")))
    );

    // A shadow after the call does not rewrite the class already created.
    let kept = "class Owner\n  Failure = Class.new\nend\nOwner.const_set(:Class, Object)";
    let (source, analyzer) = analyzed(ingest(kept, "Owner::Failure.new", None), true);
    assert!(errors(&source).is_empty(), "{:?}", errors(&source));
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Failure")))
    );

    // The receiver may be an alias of the namespace. Ruby still defines
    // Owner::Class, so a later factory in Owner must not see ::Class.
    for (alias, shadow) in [
        ("Alias = Owner", "Alias.const_set(:Class, Object)"),
        ("Core, Other = Owner, nil", "Core.const_set(:Class, Object)"),
        ("Alias ||= Owner", "Alias.const_set(:Class, Object)"),
    ] {
        let refused =
            format!("class Owner; end\n{alias}\n{shadow}\nclass Owner\n  Failure = Class.new\nend");
        let (source, analyzer) = analyzed(ingest(&refused, "Owner::Failure.new", None), true);
        assert!(
            !errors(&source).is_empty(),
            "{alias}: {:?}",
            errors(&source)
        );
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Failure"))),
            "{alias}"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &format!("{refused}\nOwner::Failure.new")])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{alias}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("NoMethodError"),
            "{alias}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn analysis_facts_never_enter_the_emission_pipeline() {
    let raw = ingest(
        "class Owner\n  Plain = Class.new\nend",
        "Owner::Plain.new",
        None,
    );
    let mut emitted = raw.clone();
    roundhouse::session::analyze_and_lower(&mut emitted);
    assert!(
        !Analyzer::new(&raw)
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Plain")))
    );
    assert!(
        errors(&emitted)
            .iter()
            .any(|diagnostic| diagnostic.code() == "unsupported")
    );
    let (source, analyzer) = analyzed(raw, true);
    assert!(errors(&source).is_empty());
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Plain")))
    );
}

/// The checker only registers identities that main's emitted declarations
/// actually define. In-app calls still go through the unchanged emission gate.
#[test]
fn retained_declarations_execute_with_their_original_runtime_contract() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/retained_factories.rb",
            r#"class RetainedParent
  def code = 42
end
class RetainedFactories
  Plain = Class.new
  Failure = Class.new(StandardError)
  Empty = ::Class.new(Object) {}
  Child = Class.new(RetainedParent)
end
"#,
        )
        .run_ruby(
            r#"raise "default parent" unless RetainedFactories::Plain.superclass == Object
raise "explicit parent" unless RetainedFactories::Empty.superclass == Object
raise "inherited method" unless RetainedFactories::Child.new.code == 42
raise "constructor" unless RetainedFactories::Failure.new("narrow").message == "narrow"
begin
  raise RetainedFactories::Failure, "raised"
rescue RetainedFactories::Failure => error
  raise "rescue" unless error.message == "raised"
end
puts "retained Class.new declarations passed"
"#,
        );
    run.assert_passes();
    assert!(
        run.stdout
            .contains("retained Class.new declarations passed")
    );
}

#[test]
fn factory_ancestry_mutations_are_independent_of_file_registration_order() {
    for (owner, extra) in [
        (
            "class Owner\n Child = Class.new(Middle) {}\n Useful = Class.new(StandardError) {}\nend",
            "class Parent; end\nclass Owner\n Middle = Class.new(Parent) {}\nend\nParent.class_eval('def code = 7')",
        ),
        (
            "class Parent; end\nclass Owner\n Middle = Class.new(Parent) {}\n Child = Class.new(Middle) {}\n Useful = Class.new(StandardError) {}\nend",
            "Parent.class_eval('def code = 7')",
        ),
    ] {
        let (_, analyzer) = analyzed(ingest(owner, "Owner::Child", Some(extra)), true);
        for name in ["Child", "Middle"] {
            assert!(
                !analyzer
                    .class_registry()
                    .contains_key(&ClassId(Symbol::from(format!("Owner::{name}"))))
            );
        }
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Useful")))
        );
    }
}

#[test]
fn literal_mutations_of_an_included_ancestor_are_scoped() {
    let (_, analyzer) = analyzed(
        ingest(
            "module Shared; end\nclass Parent\n include Shared\nend\nclass Owner\n Child = Class.new(Parent) {}\n Useful = Class.new(StandardError) {}\nend",
            "Owner::Child",
            Some("Shared.module_eval('def code = 7')"),
        ),
        true,
    );
    assert!(
        !analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child")))
    );
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Useful")))
    );
}

#[test]
fn known_ancestors_are_checked_past_an_incomplete_source_index() {
    for mutation in [
        "Owner::Grandparent.class_eval('def code = 7')",
        "Owner::Shared.module_eval('def code = 7')",
    ] {
        let (_, analyzer) = analyzed(
            ingest(
                "class Owner\n module Shared; end\n class Grandparent < External::Base; end\n class Parent < Grandparent\n include Shared\n end\n Child = Class.new(Parent) {}\n Useful = Class.new(StandardError) {}\nend",
                "Owner::Child",
                Some(mutation),
            ),
            true,
        );
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{mutation}"
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Useful"))),
            "{mutation}"
        );
    }
}

#[test]
fn factory_aliases_follow_main_class_object_evidence() {
    let (factory, _) = analyzed(
        ingest(
            "class Owner\n Child = Class.new(StandardError)\n Alias = Child\n def self.pick = Alias.new.message\nend",
            "Owner.pick",
            None,
        ),
        true,
    );
    let (ordinary, _) = analyzed(
        ingest(
            "class Owner\n class Child < StandardError; end\n Alias = Child\n def self.pick = Alias.new.message\nend",
            "Owner.pick",
            None,
        ),
        false,
    );
    assert!(errors(&ordinary).is_empty(), "{:?}", errors(&ordinary));
    assert!(errors(&factory).is_empty(), "{:?}", errors(&factory));
    assert_eq!(
        factory.controllers[0].actions().next().unwrap().body.ty,
        ordinary.controllers[0].actions().next().unwrap().body.ty
    );
}

#[test]
fn core_singleton_new_override_changes_the_factory_but_not_a_class_keyword() {
    let output = std::process::Command::new("ruby")
        .args([
            "-e",
            r#"
class Module
  def self.new(*) = :replacement
end
puts Class.new(Object).class
class Ordinary < Object; end
puts Ordinary.superclass
"#,
        ])
        .output()
        .expect("Ruby constructor control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Symbol\nObject\n");
}

#[test]
fn positional_struct_constructor_facts_follow_the_ordinary_class_header() {
    let parent = "class Parent < Struct.new(:code); end";
    let factory = format!("{parent}\nclass Owner\n Child = Class.new(Parent)\nend");
    let ordinary = format!("{parent}\nclass Owner\n class Child < Parent; end\nend");
    let action = "Owner::Child.new(code: 1).code";
    let (factory_app, factory_analyzer) = analyzed(ingest(&factory, action, None), true);
    let (ordinary_app, ordinary_analyzer) = analyzed(ingest(&ordinary, action, None), false);
    let id = ClassId(Symbol::from("Owner::Child"));
    assert!(ordinary_analyzer.class_registry()[&id].positional_struct);
    assert!(factory_analyzer.class_registry()[&id].positional_struct);
    assert_eq!(errors(&factory_app), errors(&ordinary_app));
}

#[test]
fn opening_an_alias_body_mutates_only_the_related_factory() {
    for extra in [
        "Alias = Owner::Child\nclass Alias\n def code = 7\nend",
        "Alias = Parent\nclass Alias\n def code = 7\nend",
    ] {
        let (_, analyzer) = analyzed(
            ingest(
                "class Parent; end\nclass Owner\n Child = Class.new(Parent) {}\n Useful = Class.new(StandardError) {}\nend",
                "Owner::Child",
                Some(extra),
            ),
            true,
        );
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{extra}"
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Useful"))),
            "{extra}"
        );
    }
}

#[test]
fn constructor_lookup_mutations_refuse_factories() {
    for mutation in [
        "class Module\n class << self\n def replacement(*) = 7\n alias new replacement\n end\nend",
        "class Module\n class << self\n undef new\n end\nend",
        "class << Module\n def replacement(*) = 7\n alias new replacement\nend",
        "class << Module\n undef new\nend",
        "Class.private_class_method(:new)",
        "Class.send(:private_class_method, :new)",
        "class Module\n class << self\n private :new\n end\nend",
        "module Replacement\n def new(*) = 7\nend\nclass Module\n class << self\n include Replacement\n end\nend",
        "module Replacement\n def new(*) = 7\nend\nclass << Module\n include Replacement\nend",
        "class Module\n class << self\n attr_reader :new\n end\nend",
        "class Module\n def new(*) = 7\nend",
        "Class.send(:send, :define_singleton_method, :new) { 7 }",
        "Class.__send__(:send, :__send__, :define_singleton_method, :new) { 7 }",
        "Class.method(:define_singleton_method).call(:new) { 7 }",
        "Class.method(:send).call(:send, :define_singleton_method, :new) { 7 }",
        "Class.public_method(:define_singleton_method)[:new] { 7 }",
        "Alias = Module.singleton_class\nAlias.send(:send, :alias_method, :new, :name)",
    ] {
        let raw = ingest(
            "class Owner\n Child = Class.new\nend",
            "Owner::Child.new",
            Some(mutation),
        );
        let (main, _) = analyzed(raw.clone(), false);
        let (source, analyzer) = analyzed(raw, true);
        assert!(!errors(&source).is_empty(), "{mutation}");
        assert_eq!(source, main, "{mutation}");
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{mutation}"
        );
    }
}

#[test]
fn qualified_constructor_body_owners_use_resolved_identity() {
    for mutation in [
        "class Object::Module; def self.new(*) = 7; end",
        "class ::Object::Module; def self.new(*) = 7; end",
        "module Space; class Object::Module; def self.new(*) = 7; end; end",
        "Core = Object; class Core::Module; def self.new(*) = 7; end",
        "Core = Object; module Space; class Core::Module; def self.new(*) = 7; end; end",
        "Core = Module; class Core; def self.new(*) = 7; end",
        "class Object::Module; class << self; undef new; end; end",
        "class Object::Module; class << self; attr_reader :new; end; end",
    ] {
        assert_refused_factory(mutation, "Object");
        let ordinary = format!("{mutation}\nclass Owner; class Child < Object; end; end");
        let raw = ingest(&ordinary, "Owner::Child.new", None);
        let (main, _) = analyzed(raw.clone(), false);
        let (source, _) = analyzed(raw, true);
        assert!(errors(&main).is_empty(), "{mutation}: {:?}", errors(&main));
        assert_eq!(source, main, "{mutation}");
    }
}

fn assert_refused_factory(declarations: &str, parent: &str) {
    let raw = ingest(
        &format!("{declarations}\nclass Owner; Child = Class.new({parent}) {{}}; end"),
        "Owner::Child.new",
        None,
    );
    let (main, _) = analyzed(raw.clone(), false);
    let (source, analyzer) = analyzed(raw, true);
    assert!(!errors(&main).is_empty(), "{declarations}");
    assert_eq!(source, main, "{declarations}");
    assert!(
        !analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child"))),
        "{declarations}"
    );
}

// Mixins into a constructor ancestor of a module that defines no
// construction method (`new`, `allocate`, `initialize`, `inherited`) and no
// mixin hook. action_policy runs the last one, guarded for JRuby.
const MIXINS_WITHOUT_CONSTRUCTION: &[&str] = &[
    "module Ext\n def namespace = name\nend\nModule.include(Ext)",
    "module Ext\n def namespace = name\nend\n::Module.include(Ext)",
    "module Ext\n def namespace = name\nend\nModule.send(:include, Ext)",
    "module Ext\n def namespace = name\nend\nModule.public_send(:include, Ext)",
    "module Ext\n def namespace = name\nend\nModule.method(:include).call(Ext)",
    "module Ext\n def namespace = name\nend\nModule.prepend(Ext)",
    "module Ext\n def namespace = name\nend\nModule.extend(Ext)",
    "module Ext\n def namespace = name\nend\nModule.singleton_class.include(Ext)",
    "module Ext\n def namespace = name\nend\nclass Module\n include Ext\nend",
    "module Ext\n def namespace = name\nend\nClass.include(Ext)",
    "module Ext\n def namespace = name\nend\nClass.prepend(Ext)",
    "module Ext\n def namespace = name\nend\nObject.include(Ext)",
    "module Ext\n def namespace = name\nend\nBasicObject.include(Ext)",
    "module Base\n def label = 1\nend\nmodule Ext\n include Base\nend\nModule.include(Ext)",
    "module Outer\n module Ext\n def namespace = name\n end\n ::Module.include(Ext) if RUBY_PLATFORM.match?(/java/i)\nend",
];

#[test]
fn mixins_without_construction_methods_keep_factories() {
    for &extra in MIXINS_WITHOUT_CONSTRUCTION {
        let raw = ingest(
            "class Owner\n Child = Class.new(StandardError) {}\nend",
            "Owner::Child.new.message",
            Some(extra),
        );
        let (source, analyzer) = analyzed(raw, true);
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{extra}"
        );
        assert_eq!(
            send_type(
                &source.controllers[0].actions().next().unwrap().body,
                "message"
            ),
            Some(Ty::Str),
            "{extra}"
        );
    }
}

#[test]
fn a_dead_jruby_mixin_keeps_model_factory_constants() {
    let boom_errors = |ext: &str| {
        let app = ingest_files(&[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define(version: 1) do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\nend\n",
            ),
            (
                "config/routes.rb",
                "Rails.application.routes.draw do\nend\n",
            ),
            (
                "app/models/application_record.rb",
                "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
            ),
            (
                "app/models/article.rb",
                "class Article < ApplicationRecord\n  Boom = Class.new(StandardError)\n\n  def explode!\n    raise Boom, \"no\"\n  end\nend\n",
            ),
            (
                "lib/namespace.rb",
                &format!(
                    "module Namespace\n  module Ext\n{ext}\n  end\n\n  ::Module.include(Ext) if RUBY_PLATFORM.match?(/java/i)\nend\n"
                ),
            ),
        ]);
        let (app, _) = analyzed(app, true);
        errors(&app)
            .into_iter()
            .filter(|error| error.to_string().contains("Boom"))
            .count()
    };
    assert_eq!(boom_errors("    def namespace = name"), 0);
    assert_ne!(boom_errors("    def new(*) = 7"), 0);
}

#[test]
fn mixins_that_may_change_construction_refuse_factories() {
    for declarations in [
        // Each construction method, and its other spellings.
        "module Ext\n def new(*) = 7\nend\nModule.include(Ext)",
        "module Ext\n def allocate = 7\nend\nModule.include(Ext)",
        "module Ext\n def initialize(*) = nil\nend\nClass.prepend(Ext)",
        "module Ext\n def inherited(child) = nil\nend\nObject.extend(Ext)",
        "module Ext\n def build = 7\n alias new build\nend\nModule.extend(Ext)",
        "module Ext\n attr_reader :new\nend\nModule.extend(Ext)",
        "module Ext\n def new(*) = 7\nend\n::Module.include(Ext)",
        "module Ext\n def new(*) = 7\nend\nModule.send(:include, Ext)",
        "module Ext\n def new(*) = 7\nend\nModule.method(:include).call(Ext)",
        "module Ext\n def new(*) = 7\nend\nclass Module\n include Ext\nend",
        "module Ext\n def new(*) = 7\nend\nClass.include(Ext)",
        "module Ext\n def new(*) = 7\nend\nObject.include(Ext)",
        // A hook of the module runs when it is mixed in.
        "module Ext\n def self.included(base) = base.singleton_class.define_method(:new) { |*| 7 }\nend\nModule.include(Ext)",
        "module Ext\n class << self\n def extended(base) = nil\n end\nend\nModule.extend(Ext)",
        "module Hooks\n def included(base) = nil\nend\nmodule Ext\n extend Hooks\nend\nModule.include(Ext)",
        "class Module\n def included(base) = nil\nend\nmodule Ext; end\nModule.include(Ext)",
        // A module it includes, a later opening, or a mutation elsewhere.
        "module Base\n def new(*) = 7\nend\nmodule Ext\n include Base\nend\nModule.extend(Ext)",
        "module Ext; end\nModule.include(Ext)\nmodule Ext\n def inherited(child) = nil\nend",
        "module Ext; end\nExt.define_method(:new) { |*| 7 }\nModule.include(Ext)",
        // No source body shows the module's methods.
        "Module.include(External::Ext)",
        "Ext = Module.new { def new(*) = 7 }\nModule.include(Ext)",
        "ext = Module.new { def new(*) = 7 }\nModule.include(ext)",
        "Module.include(*[Comparable])",
        // A reopening shows only part of a core or framework module.
        "module Kernel\n def namespace = name\nend\nModule.include(Kernel)",
        "module ActiveRecord\n module SignedId\n def namespace = name\n end\nend\nObject.include(ActiveRecord::SignedId)",
        "module ActiveRecord\n module SignedId\n def namespace = name\n end\nend\nmodule Ext\n include ActiveRecord::SignedId\nend\nModule.include(Ext)",
    ] {
        assert_refused_factory(declarations, "Object");
    }
}

#[test]
fn ruby_keeps_class_new_under_mixins_without_construction_methods() {
    for mixin in MIXINS_WITHOUT_CONSTRUCTION {
        let program = format!(
            "{mixin}\nclass Owner; Child = Class.new(StandardError) {{}}; end\nraise 'changed' unless Owner::Child.superclass == StandardError && Owner::Child.new('x').message == 'x'\nputs 'kept'"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &program])
            .output()
            .expect("Ruby mixin control");
        assert!(
            output.status.success(),
            "{mixin}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "kept\n", "{mixin}");
    }
    for mixin in [
        "module Ext\n def new(*) = 7\nend\nClass.prepend(Ext)",
        "module Ext\n def new(*) = 7\nend\nModule.extend(Ext)",
        "module Ext\n def inherited(child) = raise('hook')\nend\nObject.extend(Ext)",
        "module Ext\n def self.included(base) = base.singleton_class.define_method(:new) { |*| 7 }\nend\nModule.include(Ext)",
        "module Base\n def new(*) = 7\nend\nmodule Ext\n include Base\nend\nModule.extend(Ext)",
    ] {
        let program = format!(
            "{mixin}\nbegin\n class Owner; Child = Class.new(StandardError) {{}}; end\n puts Owner::Child.is_a?(Class) ? 'kept' : 'changed'\nrescue RuntimeError\n puts 'changed'\nend"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &program])
            .output()
            .expect("Ruby mixin control");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "changed\n",
            "{mixin}"
        );
    }
}

#[test]
fn parallel_assignments_follow_literal_constructor_aliases() {
    for declarations in [
        "Core, Other = Class, nil; Core.define_singleton_method(:new) { 7 }",
        "Other, Core = nil, Class; Core.define_singleton_method(:new) { 7 }",
        "::Core, Other = Class, nil; Core.define_singleton_method(:new) { 7 }",
        "(Core, Other), Tail = [Class, nil], nil; Core.define_singleton_method(:new) { 7 }",
        "*Rest, Core = [nil, Class]; Core.define_singleton_method(:new) { 7 }",
        "module Space; end; Space::Core, Other = Class, nil; Space::Core.define_singleton_method(:new) { 7 }",
    ] {
        assert_refused_factory(declarations, "Object");
    }
    // Rebinding an alias changes its binding, not the old class it named.
    for extra in [
        "Alias = Owner::Child; Alias, Other = 7, nil",
        "Alias = Class; Alias, Other = 7, nil",
        "class Other; end; Other, Tail = 7, nil",
    ] {
        let (_, analyzer) = analyzed(
            ingest(
                "class Owner; Child = Class.new(StandardError); end",
                "Owner::Child.new.message",
                Some(extra),
            ),
            true,
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{extra}"
        );
    }
}

#[test]
fn reflective_method_lookup_composes_with_every_literal_invocation() {
    for mutation in [
        "Class.send(:method, :define_singleton_method).call(:new) { 7 }",
        "Class.public_send(:public_method, :define_singleton_method)[:new] { 7 }",
        "Class.__send__(:send, :method, :define_singleton_method).call(:new) { 7 }",
        "Class.method(:define_singleton_method).===(:new) { 7 }",
        "Class.send(:method, :define_singleton_method).===(:new) { 7 }",
        "(Class.send(:method, :define_singleton_method)).call(:new) { 7 }",
        "Class.method(:define_singleton_method).send(:call, :new) { 7 }",
        "Class.send(:method, :send).call(:define_singleton_method, :new) { 7 }",
    ] {
        assert_refused_factory(mutation, "Object");
        let ordinary = format!("{mutation}\nclass Owner; class Child < Object; end; end");
        let (app, _) = analyzed(ingest(&ordinary, "Owner::Child.new", None), true);
        assert!(errors(&app).is_empty(), "{mutation}: {:?}", errors(&app));
    }
    let (_, analyzer) = analyzed(
        ingest(
            "class Other; end; class Owner; Child = Class.new(StandardError); end",
            "Owner::Child.new.message",
            Some("Other.send(:method, :define_singleton_method).===(:new) { 7 }"),
        ),
        true,
    );
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child")))
    );
}

#[test]
fn ruby_rejects_review_counterexamples_while_ordinary_headers_survive_constructor_changes() {
    for mutation in [
        "Core ||= Class; Core.define_singleton_method(:new) { 7 }",
        "module Space; end; Space::Core ||= Class; Space::Core.define_singleton_method(:new) { 7 }",
        "Core = String; Core &&= Class; Core.define_singleton_method(:new) { 7 }",
        "Core = Inner = Class; Core.define_singleton_method(:new) { 7 }",
        "Class.singleton_class.attr(:new)",
        "Class.singleton_class.send(:attr, :new)",
        "class Module; class << self; def replacement(*) = 7; alias new replacement; end; end",
        "class Module; class << self; undef new; end; end",
        "Class.private_class_method(:new)",
        "Class.method(:define_singleton_method).call(:new) { 7 }",
        "class Object::Module; def self.new(*) = 7; end",
        "class ::Object::Module; def self.new(*) = 7; end",
        "module Space; class Object::Module; def self.new(*) = 7; end; end",
        "Core = Object; class Core::Module; def self.new(*) = 7; end",
        "Core = Module; class Core; def self.new(*) = 7; end",
        "Core, Other = Class, nil; Core.define_singleton_method(:new) { 7 }",
        "(Core, Other), Tail = [Class, nil], nil; Core.define_singleton_method(:new) { 7 }",
        "Class.send(:method, :define_singleton_method).call(:new) { 7 }",
        "Class.method(:define_singleton_method).===(:new) { 7 }",
        "Class.public_send(:public_method, :define_singleton_method)[:new] { 7 }",
        "Class.__send__(:send, :method, :define_singleton_method).call(:new) { 7 }",
        "Class.method(:define_singleton_method).send(:call, :new) { 7 }",
        "module Replacement; def new(*) = 7; end; class Module; class << self; include Replacement; end; end",
    ] {
        let script = format!(
            r#"
{mutation}
begin
  class Owner; Child = Class.new; end
  Owner::Child.new
  raise "factory should fail"
rescue NoMethodError
end
class Ordinary < Object; end
raise "ordinary class" unless Ordinary.new.is_a?(Ordinary)
puts "refused factory and valid ordinary class"
"#
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &script])
            .output()
            .expect("Ruby refusal control");
        assert!(
            output.status.success(),
            "{mutation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "refused factory and valid ordinary class\n"
        );
    }
    for parent in [
        "Float::INFINITY",
        "File::RDONLY",
        "Encoding::UTF_8",
        "RUBY_VERSION",
        "File::Constants",
        "Class.new { break 7 }",
        "Class.new { break Enumerable }",
    ] {
        let script = format!(
            "begin\n Class.new({parent})\n raise 'expected invalid superclass'\nrescue TypeError\n puts 'refused superclass'\nend"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &script])
            .output()
            .expect("Ruby superclass control");
        assert!(
            output.status.success(),
            "{parent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "refused superclass\n"
        );
    }
    for assignment in ["Parent, Other = 7, nil", "::Parent, Other = 7, nil"] {
        let script = format!(
            "class Parent; end\n{assignment}\nbegin\n Class.new(Parent)\n raise 'expected invalid superclass'\nrescue TypeError\n puts 'refused superclass'\nend"
        );
        let output = std::process::Command::new("ruby")
            .args(["-e", &script])
            .output()
            .expect("Ruby parallel superclass control");
        assert!(
            output.status.success(),
            "{assignment}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "refused superclass\n"
        );
    }
}

#[test]
fn conditional_and_chained_aliases_track_constructor_mutations() {
    for mutation in [
        "Core ||= Class; Core.define_singleton_method(:new) { 7 }",
        "module Space; end; Space::Core ||= Class; Space::Core.define_singleton_method(:new) { 7 }",
        "Core = String; Core &&= Class; Core.define_singleton_method(:new) { 7 }",
        "module Space; Core = String; end; Space::Core &&= Class; Space::Core.define_singleton_method(:new) { 7 }",
        "Core = Inner = Class; Core.define_singleton_method(:new) { 7 }",
        "Core = (Inner = Class); Core.define_singleton_method(:new) { 7 }",
        "Core = (Inner ||= Class); Core.define_singleton_method(:new) { 7 }",
        "module Space; end; Space::Core = Inner = Class; Space::Core.define_singleton_method(:new) { 7 }",
    ] {
        assert_refused_factory(mutation, "Object");
        let ordinary = format!("{mutation}\nclass Owner; class Child < Object; end; end");
        let raw = ingest(&ordinary, "Owner::Child.new", None);
        let (main, _) = analyzed(raw.clone(), false);
        let (source, _) = analyzed(raw, true);
        assert!(errors(&main).is_empty(), "{mutation}: {:?}", errors(&main));
        assert_eq!(source, main, "{mutation}");
    }
    let (_, analyzer) = analyzed(
        ingest(
            "class Other; end; class Owner; Child = Class.new(StandardError); end",
            "Owner::Child.new.message",
            Some("Core ||= Other; Core.define_singleton_method(:new) { 7 }"),
        ),
        true,
    );
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Owner::Child")))
    );
}

#[test]
fn attr_on_constructor_lookup_path_refuses_factories() {
    for mutation in [
        "Class.singleton_class.attr(:new)",
        "Class.singleton_class.send(:attr, :new)",
        "Class.singleton_class.__send__(:send, :attr, :new)",
        "class Module; class << self; attr :new; end; end",
    ] {
        assert_refused_factory(mutation, "Object");
        let raw = ingest(
            &format!("{mutation}\nclass Owner; class Child < Object; end; end"),
            "Owner::Child.new",
            None,
        );
        let (main, _) = analyzed(raw.clone(), false);
        let (source, _) = analyzed(raw, true);
        assert!(errors(&main).is_empty(), "{mutation}: {:?}", errors(&main));
        assert_eq!(source, main, "{mutation}");
    }
    for declaration in [
        "class Parent; attr :code; end",
        "class Parent; end; Class.singleton_class.attr(:unrelated)",
        "class Parent; end; Module.singleton_class.attr(:unrelated, false)",
    ] {
        let (_, analyzer) = analyzed(
            ingest(
                &format!("{declaration}\nclass Owner; Child = Class.new(Parent); end"),
                "Owner::Child.new",
                None,
            ),
            true,
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{declaration}"
        );
    }
}

#[test]
fn superclass_limits_remain_at_ordinary_class_parity() {
    // These programs fail on CRuby. Main accepts the equivalent ordinary
    // Child header with the same parent expression and preceding mutation.
    // Refusing them here would impose a stronger class model on Class.new.
    for (declarations, parent) in [
        ("", "Float::INFINITY"),
        ("", "File::RDONLY"),
        ("", "Encoding::UTF_8"),
        ("", "RUBY_VERSION"),
        ("", "File::Constants"),
        ("class Owner; Parent = Class.new { break 7 }; end", "Parent"),
        (
            "class Owner; Parent = Class.new { break Enumerable }; end",
            "Parent",
        ),
        ("class Parent; end; Parent, Other = 7, nil", "Parent"),
        ("class Parent; end; ::Parent, Other = 7, nil", "Parent"),
        (
            "class Parent; end; (Parent, Other), Tail = [7, nil], nil",
            "Parent",
        ),
        ("class Parent; end; *Rest, Parent = [nil, 7]", "Parent"),
    ] {
        let ordinary = format!("{declarations}\nclass Owner; class Child < {parent}; end; end");
        let (main, _) = analyzed(ingest(&ordinary, "Owner::Child.new", None), false);
        assert!(
            errors(&main).is_empty(),
            "{declarations}, {parent}: {:?}",
            errors(&main)
        );
        let factory = format!("{declarations}\nclass Owner; Child = Class.new({parent}); end");
        let (source, analyzer) = analyzed(ingest(&factory, "Owner::Child.new", None), true);
        assert!(
            errors(&source).is_empty(),
            "{declarations}, {parent}: {:?}",
            errors(&source)
        );
        assert!(
            analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("Owner::Child"))),
            "{declarations}, {parent}"
        );
    }
}

fn ingest_files(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

fn class_method_return(analyzer: &Analyzer, class: &str, method: &str) -> Ty {
    match &analyzer.class_registry()[&ClassId(Symbol::from(class))].class_methods[&Symbol::from(method)] {
        Ty::Fn { ret, .. } => ret.as_ref().clone(),
        ty => ty.clone(),
    }
}

#[test]
fn file_level_factories_are_top_level_classes() {
    const READER: &str = "class Probe\n  def self.value = ExampleError\nend\n";
    for files in [
        // Ahead of a class in the same file: not that class's constant.
        vec![(
            "app/models/probe.rb",
            "TIMEOUT = 5\nExampleError = Class.new(StandardError)\nclass Probe\n  def self.value = ExampleError\n  def self.timeout = TIMEOUT\nend\n",
        )],
        // A file of its own under app/models: not a model.
        vec![
            ("app/models/example_error.rb", "ExampleError = Class.new(StandardError)\n"),
            ("app/models/probe.rb", READER),
        ],
        // A support directory, read from a module's rescue and a class body.
        vec![
            ("app/errors/example_errors.rb", "BaseError = Class.new(StandardError)\nExampleError = Class.new(BaseError)\n"),
            (
                "app/services/limiter.rb",
                "module Limiter\n  def self.check(n)\n    raise ExampleError, \"too many\" if n > 3\n    \"ok\"\n  rescue BaseError => e\n    e.message\n  end\nend\n",
            ),
            ("app/models/probe.rb", "class Probe\n  ERRORS = [BaseError, ExampleError].freeze\n  def self.value = ERRORS\nend\n"),
        ],
    ] {
        let (app, analyzer) = analyzed(ingest_files(&files), true);
        assert!(errors(&app).is_empty(), "{files:?}: {:?}", errors(&app));
        let class = &analyzer.class_registry()[&ClassId(Symbol::from("ExampleError"))];
        assert!(class.parent.is_some(), "{files:?}");
        assert!(
            !analyzer.class_registry().contains_key(&ClassId(Symbol::from("Probe::ExampleError"))),
            "{files:?}"
        );
    }
}

#[test]
fn a_factory_block_of_class_body_statements_gives_the_class_its_methods() {
    const BLOCK: &str = "Class.new(StandardError) do\n  attr_reader :details\n  def code = 42\n  private\n  def hidden = 1\nend";
    for (files, id) in [
        (
            vec![(
                "app/models/probe.rb",
                "class Probe\n  ExampleError = Class.new(StandardError) do\n    def code = 42\n  end\n  def self.value = ExampleError.new(\"x\").code\nend\n",
            )],
            "Probe::ExampleError",
        ),
        (
            vec![
                ("app/models/example_error.rb", "ExampleError = Class.new(StandardError) do\n  def code = 42\nend\n"),
                ("app/models/probe.rb", "class Probe\n  def self.value = ExampleError.new(\"x\").code\nend\n"),
            ],
            "ExampleError",
        ),
    ] {
        let (app, analyzer) = analyzed(ingest_files(&files), true);
        assert!(errors(&app).is_empty(), "{files:?}: {:?}", errors(&app));
        let class = &analyzer.class_registry()[&ClassId(Symbol::from(id))];
        assert_eq!(class.parent, Some(ClassId(Symbol::from("StandardError"))));
        assert!(class.instance_methods.contains_key(&Symbol::from("code")), "{id}");
        assert_eq!(class_method_return(&analyzer, "Probe", "value"), Ty::Int, "{id}");
    }
    let files = [
        ("app/models/example_error.rb", format!("ExampleError = {BLOCK}\n")),
        ("app/models/probe.rb", "class Probe\n  def self.value = ExampleError.new(\"x\").code\nend\n".to_string()),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    let (app, analyzer) = analyzed(ingest_files(&files), true);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let class = &app.library_classes.iter().find(|class| class.name.0.as_str() == "ExampleError").unwrap();
    let visibility = |name: &str| class.methods.iter().find(|method| method.name.as_str() == name).map(|method| method.visibility);
    assert_eq!(visibility("details"), Some(roundhouse::dialect::MethodVisibility::Public));
    assert_eq!(visibility("hidden"), Some(roundhouse::dialect::MethodVisibility::Private));
    assert_eq!(class_method_return(&analyzer, "Probe", "value"), Ty::Int);
}

#[test]
fn other_file_level_factories_stay_unresolved_constants() {
    // A parent the source does not name, or a block whose constant is the
    // enclosing scope's, is not the class a `class` header would make.
    for factory in [
        "ExampleError = Class.new(Object.const_get(:StandardError))",
        "ExampleError = Class.new(StandardError) do\n  CODE = 42\nend",
    ] {
        let files = [("app/models/probe.rb", format!("{factory}\nclass Probe\n  def self.value = ExampleError\nend\n"))];
        let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
        let (app, analyzer) = analyzed(ingest_files(&files), true);
        assert!(
            errors(&app).iter().any(|error| error.code() == "unsupported"),
            "{factory}: {:?}",
            errors(&app)
        );
        assert!(!analyzer.class_registry().contains_key(&ClassId(Symbol::from("ExampleError"))), "{factory}");
    }
}

#[test]
fn file_level_constants_hoist_onto_a_class_new_only_file() {
    // A library file of only Class.new factories still needs sibling
    // constants on Object; hoist them onto the first factory.
    let files = [(
        "app/models/limit_errors.rb",
        "TIMEOUT = 5\nBaseError = Class.new(StandardError)\nLimitError = Class.new(BaseError) do\n  def self.code = TIMEOUT\nend\n",
    )];
    let (app, analyzer) = analyzed(ingest_files(&files), true);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let base = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "BaseError")
        .expect("BaseError");
    assert!(
        base.constants.iter().any(|(name, _)| name.as_str() == "TIMEOUT"),
        "TIMEOUT missing on BaseError: {:?}",
        base.constants.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(class_method_return(&analyzer, "LimitError", "code"), Ty::Int);
}

#[test]
fn file_level_constants_refuse_a_sibling_factory_reader() {
    // Hoisting TIMEOUT onto A leaves B's bare read unreachable at runtime.
    let source = "TIMEOUT = 5\nA = Class.new(StandardError)\nB = Class.new(StandardError) do\n  def self.t = TIMEOUT\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
    assert!(
        !app.library_classes.iter().any(|class| class.name.0.as_str() == "B"),
        "refused file must not keep the sibling factory: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn file_level_constants_refuse_an_unrelated_host_reader() {
    let source = "TIMEOUT = 5\nclass Foo; end\nE = Class.new(StandardError) do\n  def self.t = TIMEOUT\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_value_naming_a_later_class() {
    let source = "BaseError = Class.new(StandardError)\nLimitError = Class.new(BaseError)\nALL = [LimitError]\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("defined after host")),
        "{gaps:?}"
    );
}


#[test]
fn file_level_constants_refuse_a_sibling_param_default_reader() {
    // A default is evaluated in the callee's scope, not the host's.
    let source = "TIMEOUT = 5\nA = Class.new(StandardError)\nB = Class.new(StandardError) do\n  def self.t(x = TIMEOUT) = x\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_sibling_rooted_const_reader() {
    // `::TIMEOUT` is Object::TIMEOUT; after hoist onto A it is gone.
    let source = "TIMEOUT = 5\nA = Class.new(StandardError)\nB = Class.new(StandardError) do\n  def self.t = ::TIMEOUT\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_host_rooted_const_reader() {
    // Even the host cannot keep Object::TIMEOUT after the hoist.
    let source = "TIMEOUT = 5\nA = Class.new(StandardError) do\n  def self.t = ::TIMEOUT\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_rooted_value() {
    let source = "TIMEOUT = 5\nA = Class.new(StandardError)\nALL = [::TIMEOUT]\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("roots `::")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_qualified_later_class_value() {
    let source = "class BaseError; end\nclass LimitError < BaseError; TOKEN = 1; end\nALL = [LimitError::TOKEN]\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("defined after host")),
        "{gaps:?}"
    );
}

#[test]
fn file_level_constants_refuse_a_qualified_hoisted_reader() {
    let source = "CONFIG = { PI: 3 }\nA = Class.new(StandardError) do\n  def self.t = CONFIG::PI\nend\nB = Class.new(StandardError) do\n  def self.t = CONFIG::PI\nend\n";
    roundhouse::ingest::survey::activate();
    let tree: HashMap<PathBuf, Vec<u8>> =
        [(PathBuf::from("app/models/errors.rb"), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
    let _app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("cannot see it after hoisting")),
        "{gaps:?}"
    );
}

#[test]
fn a_file_level_factory_follows_its_in_file_parent_class() {
    // `NotFound = Class.new(AppError)` after `class AppError` must not
    // prepend the factory ahead of AppError: Ruby resolves the superclass
    // while defining NotFound.
    let files = [(
        "app/models/errors.rb",
        "class AppError < StandardError; end\nNotFound = Class.new(AppError)\n",
    )];
    let (app, _) = analyzed(ingest_files(&files), true);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let names: Vec<&str> = app.library_classes.iter().map(|class| class.name.0.as_str()).collect();
    let app_error = names.iter().position(|name| *name == "AppError").expect("AppError");
    let not_found = names.iter().position(|name| *name == "NotFound").expect("NotFound");
    assert!(app_error < not_found, "expected AppError before NotFound, got {names:?}");
}

#[test]
fn a_nested_factory_is_the_sibling_of_its_ordinary_parent() {
    let files = [(
        "app/models/owner.rb",
        "class Owner\n  class Parent; end\n  Child = Class.new(Parent)\nend\n",
    )];
    let (app, analyzer) = analyzed(ingest_files(&files), true);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(app.library_classes.iter().any(|class| class.name.0.as_str() == "Owner::Child"));
    assert!(app.library_classes.iter().any(|class| class.name.0.as_str() == "Owner::Parent"));
    let child = &analyzer.class_registry()[&ClassId(Symbol::from("Owner::Child"))];
    assert_eq!(child.parent, Some(ClassId(Symbol::from("Owner::Parent"))));
}
