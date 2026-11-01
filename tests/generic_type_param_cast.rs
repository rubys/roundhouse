//! An inline `#:` cast names a type parameter of its generic method or
//! class.
//!
//! RBS declares method type parameters in the signature
//! (`#: [U] (Array[U]) -> U?`) and class type parameters on a `#: [K, V]`
//! line above the class. Within those bodies `U` is a type variable,
//! not a constant. The signature already read it that way, but the
//! inline casts in the body (`result = nil #: U?`) were parsed alone and
//! refused as `U has no indexed declaration` (about 30 sites in a large app).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const GEN_BOX: &str = r#"
class GenBox
  #: [U] (Array[U]) -> U?
  def first_of(items)
    result = nil #: U?
    items.each do |item|
      result = item #: U
    end
    result
  end

  #: [out U, unchecked in V < Comparable[V]]
  class Acc
    #: (Array[untyped], untyped) -> void
    def initialize(items, err)
      @items = items #: Array[U]
      @err = err #: V
    end
  end

  #: (Integer) -> Integer
  def outside(x)
    y = x #: U
    y
  end
end
"#;

fn errors() -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/gen_box.rb", GEN_BOX),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|e| e.contains("has no indexed declaration"))
        .collect()
}

#[test]
fn method_and_class_type_parameters_are_in_scope_for_body_casts() {
    let errors = errors();
    assert!(!errors.iter().any(|e| e.contains("V has no")), "{errors:?}");
    let u: Vec<&String> = errors.iter().filter(|e| e.contains("U has no")).collect();
    assert_eq!(u.len(), 1, "{errors:?}");
}

/// Outside its method a type parameter's name is an ordinary constant.
#[test]
fn a_type_parameter_does_not_leak_past_its_method() {
    let errors = errors();
    assert!(errors.iter().any(|e| e.contains("U has no")), "{errors:?}");
}
