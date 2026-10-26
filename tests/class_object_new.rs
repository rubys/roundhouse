//! `k.new` on a class object of unknown identity answers an instance, not a
//! `Class`. Only the constant `Class` builds a class with `new`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

/// The harvested return; `None` when nothing was harvested (emitted as `untyped`).
fn ret_of(source: &str, class: &str, method: &str) -> Option<Ty> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/services/probe.rb"), source.as_bytes().to_vec());
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let owner = app.library_classes.iter().find(|c| c.name.0.as_str() == class).expect("class");
    let def = owner.methods.iter().find(|m| m.name.as_str() == method).expect("method");
    match &def.signature {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    }
}

fn is_class(ty: &Option<Ty>) -> bool {
    matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "Class")
}

/// campfire's `ContentFilters::SanitizeAttributes#sanitizer` is
/// `sanitizer_class.new`, with `sanitizer_class` a `.class` read. Declared
/// `-> Class`, spinel took the sanitizer's own `sanitize` for a call on a
/// class object and refused the build.
#[test]
fn new_on_a_class_valued_method_is_not_a_class() {
    let source = "class Probe\n  def kind\n    Registry.sanitizer.class\n  end\n\n  def build\n    kind.new\n  end\nend\n";
    let kind = ret_of(source, "Probe", "kind");
    assert!(is_class(&kind), "precondition: `.class` reads as Class, got {kind:?}");
    let built = ret_of(source, "Probe", "build");
    assert!(!is_class(&built), "an instance of the class, not a Class: {built:?}");
}

/// The constant itself still builds a class.
#[test]
fn class_new_on_the_constant_is_a_class() {
    let source = "class Probe\n  def build\n    Class.new\n  end\nend\n";
    let built = ret_of(source, "Probe", "build");
    assert!(is_class(&built), "Class.new builds a class: {built:?}");
}
