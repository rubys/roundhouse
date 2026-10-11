//! Local, statically known visibility survives lexical flattening and Ruby
//! family emission. This does not claim strict-target dispatch equivalence.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodDef, MethodReceiver, MethodVisibility, ModelBodyItem};
use roundhouse::ingest::{
    ingest_app_from_tree, ingest_library_class, ingest_library_classes, ingest_model,
};
use roundhouse::schema::Schema;

fn methods(source: &str, model: bool) -> Vec<MethodDef> {
    if model {
        ingest_model(
            source.as_bytes(),
            "thing.rb",
            &Schema::default(),
            &Default::default(),
        )
        .expect("model ingest")
        .expect("model")
        .body
        .into_iter()
        .filter_map(|item| match item {
            ModelBodyItem::Method { method, .. } => Some(method),
            _ => None,
        })
        .collect()
    } else {
        ingest_library_class(source.as_bytes(), "thing.rb")
            .expect("library ingest")
            .expect("class")
            .methods
    }
}

fn visibility(methods: &[MethodDef], name: &str, receiver: MethodReceiver) -> MethodVisibility {
    methods
        .iter()
        .rev()
        .find(|m| m.name.as_str() == name && m.receiver == receiver)
        .expect(name)
        .visibility
}

#[test]
fn instance_and_singleton_defaults_are_lexical_not_sticky() {
    use MethodReceiver::{Class, Instance};
    use MethodVisibility::{Private, Protected, Public};
    let source = r#"class Thing < ApplicationRecord
  private
  def hidden; 11; end
  def self.visible; 12; end
  class << self
    def first; 13; end
    protected
    def guarded; 14; end
    private
    def hidden; 15; end
  end
  def still_hidden; 16; end
  class << self
    def fresh; 17; end
    private :first
  end
  public
  def later; 18; end
  def initialize; @value = 19; end
end"#;
    for model in [false, true] {
        let methods = methods(source, model);
        for (name, receiver, want) in [
            ("hidden", Instance, Private),
            ("visible", Class, Public),
            ("first", Class, Private),
            ("guarded", Class, Protected),
            ("hidden", Class, Private),
            ("still_hidden", Instance, Private),
            ("fresh", Class, Public),
            ("later", Instance, Public),
            ("initialize", Instance, Private),
        ] {
            assert_eq!(
                visibility(&methods, name, receiver),
                want,
                "model={model} {name}"
            );
        }
    }
}

#[test]
fn repeated_private_hooks_keep_duplicate_evidence_but_named_changes_still_error() {
    for name in ["initialize", "initialize_copy", "initialize_dup", "initialize_clone"] {
        let source = format!("class Thing\n  def {name}; 11; end\n  def {name}; 29; end\nend");
        let definitions = methods(&source, false);
        assert_eq!(definitions.len(), 2, "duplicate evidence lost: {name}");
        assert!(definitions.iter().all(|m| m.visibility == MethodVisibility::Private));

        let changed = source.replace(
            &format!("  def {name}; 29; end"),
            &format!("  public :{name}\n  def {name}; 29; end"),
        );
        assert!(ingest_library_class(changed.as_bytes(), "thing.rb").is_err(), "{changed}");
    }
}

#[test]
fn named_and_inline_changes_do_not_change_the_next_definition() {
    use MethodReceiver::{Class, Instance};
    use MethodVisibility::{Private, Protected, Public};
    let source = r#"class Thing < ApplicationRecord
  def helper; 21; end
  private :helper
  def later; 22; end
  protected def guarded; 23; end
  def last; 24; end
  def self.hidden; 25; end
  private_class_method :hidden
  private_class_method def self.inline_hidden; 26; end
  def self.visible; 27; end
  public_class_method :hidden
  public :helper
end"#;
    for model in [false, true] {
        let methods = methods(source, model);
        for (name, receiver, want) in [
            ("helper", Instance, Public),
            ("later", Instance, Public),
            ("guarded", Instance, Protected),
            ("last", Instance, Public),
            ("hidden", Class, Public),
            ("inline_hidden", Class, Private),
            ("visible", Class, Public),
        ] {
            assert_eq!(
                visibility(&methods, name, receiver),
                want,
                "model={model} {name}"
            );
        }
    }
}

#[test]
fn private_class_method_follows_module_function() {
    for source in [
        "module BulkWrite\n  module_function\n  def build_row\n    1\n  end\n  private_class_method :build_row\nend\n",
        "module BulkWrite\n  def build_row\n    1\n  end\n  module_function :build_row\n  private_class_method :build_row\nend\n",
        "module BulkWrite\n  module_function\n  def build_row\n    1\n  end\n  def other\n    2\n  end\n  private_class_method :build_row, :other\nend\n",
    ] {
        let classes = ingest_library_classes(source.as_bytes(), "thing.rb").expect(source);
        let methods = &classes[0].methods;
        assert_eq!(
            visibility(methods, "build_row", MethodReceiver::Class),
            MethodVisibility::Private,
            "{source}"
        );
        if source.contains("def other") {
            assert_eq!(
                visibility(methods, "other", MethodReceiver::Class),
                MethodVisibility::Private,
                "{source}"
            );
        }
    }
    let restored = "module BulkWrite\n  module_function\n  def build_row\n    1\n  end\n  private_class_method :build_row\n  public_class_method :build_row\nend\n";
    let restored_methods = &ingest_library_classes(restored.as_bytes(), "thing.rb")
        .expect("public_class_method")[0]
        .methods;
    assert_eq!(
        visibility(restored_methods, "build_row", MethodReceiver::Class),
        MethodVisibility::Public
    );
    let undefined = "module BulkWrite\n  module_function\n  def build_row\n    1\n  end\n  private_class_method :missing\nend\n";
    assert!(
        ingest_library_classes(undefined.as_bytes(), "thing.rb").is_err(),
        "a name module_function never copied stays an error"
    );
}

#[test]
fn public_ends_module_function_copy_tracking() {
    let source = "module BulkWrite\n  module_function\n  def build_row\n    1\n  end\n  public\n  def later\n    2\n  end\n  private_class_method :later\nend\n";
    assert!(
        ingest_library_classes(source.as_bytes(), "thing.rb").is_err(),
        "public ends module_function, so later is not a class method"
    );
}

#[test]
fn private_class_method_accepts_several_defined_class_methods() {
    let source = "class Probe\n  def self.a; 1; end\n  def self.b; 2; end\n  def self.c; 3; end\n  private_class_method :a, :b, :c\n  public_class_method :b\nend\n";
    let methods = methods(source, false);
    assert_eq!(visibility(&methods, "a", MethodReceiver::Class), MethodVisibility::Private);
    assert_eq!(visibility(&methods, "b", MethodReceiver::Class), MethodVisibility::Public);
    assert_eq!(visibility(&methods, "c", MethodReceiver::Class), MethodVisibility::Private);
}

#[test]
fn a_modifier_if_is_not_a_conditional_visibility_declaration() {
    // `return value if value.is_a?(Kind)` is one expression. A visibility
    // marker inside `if` / `unless` / `else` is still dynamic.
    for (body, accepted) in [
        ("def parse(value)\n    return value if value.is_a?(String)\n    value\n  end", true),
        ("def parse(value)\n    return value unless value.nil?\n    value\n  end", true),
        ("def self.helper; 1; end\n  private_class_method :helper", true),
        ("def self.helper; 1; end\n  private_class_method :helper, :other", false),
        ("def parse(value)\n    if value\n      private :parse\n    end\n    value\n  end", true),
        ("def parse(value)\n    unless value\n      private :parse\n    else\n      value\n    end\n  end", true),
        ("if ready\n    private\n    def hidden; 1; end\n  end", false),
        // A neighboring `def` is what makes the module surface. Without
        // it, an accessor inside `if` would be dropped with the module
        // and the error would never be raised.
        ("def present; 1; end\n  if enabled\n    attr_reader :token\n  end", false),
        ("def present; 1; end\n  unless enabled\n    attr_writer :token\n  else\n    attr_accessor :other\n  end", false),
        ("def present; 1; end\n  if enabled\n    cattr_reader :domain\n  end", false),
        ("def present; 1; end\n  mattr_accessor :label", true),
        ("return value if value", true),
        ("unless ready\n    def hidden; 1; end\n  else\n    private :helper\n  end\n  def helper; 1; end", false),
        ("def self.helper; 1; end\n  class << self\n    private_class_method :helper\n  end", false),
    ] {
        let source = format!("module Toolkit\n  {body}\nend");
        let result = ingest_library_classes(source.as_bytes(), "toolkit.rb");
        if result.is_ok() != accepted {
            panic!("body={body}\nresult={result:?}");
        }
        if accepted && body.contains("mattr_accessor :label") {
            let classes = result.expect("bare accessor");
            assert!(
                classes[0].methods.iter().any(|m| m.name.as_str() == "label"),
                "a bare accessor is kept; {body}"
            );
        } else if accepted && body.contains("private_class_method :helper") && !body.contains(",") {
            let classes = result.expect("private_class_method");
            let methods = &classes
                .iter()
                .find(|c| c.methods.iter().any(|m| m.name.as_str() == "helper"))
                .expect("helper class")
                .methods;
            assert_eq!(
                visibility(&methods, "helper", MethodReceiver::Class),
                MethodVisibility::Private,
                "{body}"
            );
        }
    }
}

#[test]
fn forward_inherited_dynamic_and_ambiguous_changes_stay_errors() {
    for body in [
        "private :later\n def later; 1; end",
        "private :inherited",
        "private_class_method :later\n def self.later; 1; end",
        "private_class_method :inherited",
        "def same; 1; end\n def same; 2; end\n private :same",
        "def same; 1; end\n private :same\n def same; 2; end",
        "private\n def same; 1; end\n public\n def same; 2; end",
        "def self.only; 1; end\n private :only",
        "def only; 1; end\n private_class_method :only",
        "def helper; 1; end\n private name",
        "def self.helper; 1; end\n private def self.other; 2; end",
        "def helper; 1; end\n private :helper if condition",
        "if condition\n def helper; 1; end\n end\n private :helper",
        "def helper; 1; end\n class << self; private :helper; end",
    ] {
        let source = format!("class Thing < ApplicationRecord\n{body}\nend");
        assert!(
            ingest_model(
                source.as_bytes(),
                "thing.rb",
                &Schema::default(),
                &Default::default()
            )
            .is_err(),
            "model accepted {body}"
        );
        assert!(
            ingest_library_class(source.as_bytes(), "thing.rb").is_err(),
            "library accepted {body}"
        );
    }
}

#[test]
fn ordinary_method_bodies_are_not_outer_visibility_declarations() {
    let source = "class Thing < ApplicationRecord\n def macro; private :generated; end\n def later; 1; end\nend";
    for model in [false, true] {
        let methods = methods(source, model);
        assert_eq!(
            visibility(&methods, "later", MethodReceiver::Instance),
            MethodVisibility::Public
        );
    }
}

#[test]
fn a_concern_carrier_is_not_the_modules_own_singleton_scope() {
    for carrier in ["class_methods do", "module ClassMethods"] {
        let source = format!(
            "module Helpers\n def self.helper; 1; end\n {carrier}\n private :helper\n end\nend"
        );
        assert!(
            ingest_library_classes(source.as_bytes(), "helpers.rb").is_err(),
            "{source}"
        );
        let source = format!(
            "module Helpers\n {carrier}\n def helper; 1; end\n end\n private_class_method :helper\nend"
        );
        assert!(
            ingest_library_classes(source.as_bytes(), "helpers.rb").is_err(),
            "{source}"
        );
    }
}

#[test]
fn second_singleton_level_inside_carriers_remains_unsupported() {
    for carrier in [
        "class << self",
        "class_methods do",
        "module ClassMethods",
        "def self.included(base); class << base",
    ] {
        let end = if carrier.starts_with("def") { "end; end" } else { "end" };
        for declaration in [
            "def helper; 1; end; private_class_method :helper",
            "private_class_method def helper; 1; end",
            "def helper; 1; end; public_class_method :helper",
            "public_class_method def helper; 1; end",
            "private; module_function; def helper; 1; end",
            "private; def helper; 1; end; module_function :helper",
        ] {
            let source = format!(
                "class Thing < ApplicationRecord\n{carrier}\n{declaration}\n{end}\nend"
            );
            let error = ingest_library_class(source.as_bytes(), "thing.rb").unwrap_err();
            assert!(error.to_string().contains("nested singleton"), "{source}: {error}");
            let error = ingest_model(
                source.as_bytes(),
                "thing.rb",
                &Schema::default(),
                &Default::default(),
            ).unwrap_err();
            assert!(error.to_string().contains("nested singleton"), "{source}: {error}");
        }
    }
}

#[test]
fn nested_classes_and_modules_have_independent_defaults_and_names() {
    let source = r#"class Thing < ApplicationRecord
  private
  def same; 1; end
  class Child
    def same; 2; end
    protected def guarded; 3; end
  end
  module Helpers
    def same; 4; end
  end
end"#;
    let classes = ingest_library_classes(source.as_bytes(), "thing.rb").unwrap();
    let child = &classes
        .iter()
        .find(|c| c.name.0.as_str() == "Thing::Child")
        .unwrap()
        .methods;
    let helpers = &classes
        .iter()
        .find(|c| c.name.0.as_str() == "Thing::Helpers")
        .unwrap()
        .methods;
    assert_eq!(
        visibility(child, "same", MethodReceiver::Instance),
        MethodVisibility::Public
    );
    assert_eq!(
        visibility(child, "guarded", MethodReceiver::Instance),
        MethodVisibility::Protected
    );
    assert_eq!(
        visibility(helpers, "same", MethodReceiver::Instance),
        MethodVisibility::Public
    );
    for model in [false, true] {
        assert_eq!(
            visibility(&methods(source, model), "same", MethodReceiver::Instance),
            MethodVisibility::Private
        );
    }
}

#[test]
fn nonpublic_model_accessors_are_not_silently_accepted_without_method_metadata() {
    for body in [
        "private\n attr_reader :value",
        "attr_reader :value\n private :value",
    ] {
        let source = format!("class Thing < ApplicationRecord\n{body}\nend");
        assert!(
            ingest_model(
                source.as_bytes(),
                "thing.rb",
                &Schema::default(),
                &Default::default()
            )
            .is_err(),
            "{source}"
        );
    }
}

/// A model aliases a private def under `private` with `alias_method`. The
/// alias is a copy of the def with the original's visibility, as Ruby gives it:
/// `T.private_instance_methods(false)` is `[:hidden, :hidden_too]`, the
/// public ones `[:shown, :shown_too]`.
#[test]
fn a_model_alias_method_of_a_local_def_copies_it_with_the_originals_visibility() {
    let source = "class Thing < ApplicationRecord\n def shown; 1; end\n private\n def hidden; 2; end\n alias_method :hidden_too, :hidden\n alias_method :shown_too, :shown\nend";
    let methods = methods(source, true);
    for (name, want) in [
        ("hidden", MethodVisibility::Private),
        ("hidden_too", MethodVisibility::Private),
        ("shown", MethodVisibility::Public),
        ("shown_too", MethodVisibility::Public),
    ] {
        assert_eq!(visibility(&methods, name, MethodReceiver::Instance), want, "{name}");
    }
    let copy = methods.iter().find(|m| m.name.as_str() == "hidden_too").unwrap();
    let original = methods.iter().find(|m| m.name.as_str() == "hidden").unwrap();
    assert_eq!(copy.body, original.body);
}

/// Ruby's alias of a def that calls `super` reaches the original name's
/// super method; a renamed copy would reach its own, so it is not made.
#[test]
fn a_model_alias_method_of_a_super_calling_def_is_not_copied() {
    let source = "class Thing < ApplicationRecord\n private\n def save; super; end\n alias_method :store, :save\nend";
    assert!(
        ingest_model(source.as_bytes(), "thing.rb", &Schema::default(), &Default::default()).is_err(),
        "{source}"
    );
}

#[test]
fn a_custom_definition_can_unambiguously_override_an_accessor() {
    let source =
        "class Thing < ApplicationRecord\n attr_reader :value\n private\n def value; 7; end\nend";
    for model in [false, true] {
        assert_eq!(
            visibility(&methods(source, model), "value", MethodReceiver::Instance),
            MethodVisibility::Private
        );
    }
}

#[test]
fn accessors_and_aliases_capture_visibility_at_their_own_position() {
    let source = r#"class Thing
  attr_reader :value
  alias_method :early, :value
  private :value
  alias_method :late, :value
  protected
  attr_accessor :guarded
  public
  attr_reader :later
end"#;
    let methods = methods(source, false);
    for (name, want) in [
        ("value", MethodVisibility::Private),
        ("early", MethodVisibility::Public),
        ("late", MethodVisibility::Private),
        ("guarded", MethodVisibility::Protected),
        ("guarded=", MethodVisibility::Protected),
        ("later", MethodVisibility::Public),
    ] {
        assert_eq!(visibility(&methods, name, MethodReceiver::Instance), want);
    }
}

#[test]
fn serde_defaults_old_ir_to_public_and_round_trips_each_visibility() {
    let mut method = methods("class Thing; def helper; 1; end; end", false).remove(0);
    let mut old = serde_json::to_value(&method).unwrap();
    old.as_object_mut().unwrap().remove("visibility");
    let decoded: MethodDef = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.visibility, MethodVisibility::Public);
    for visibility in [
        MethodVisibility::Public,
        MethodVisibility::Protected,
        MethodVisibility::Private,
    ] {
        method.visibility = visibility;
        assert_eq!(
            serde_json::from_str::<MethodDef>(&serde_json::to_string(&method).unwrap()).unwrap(),
            method
        );
    }
}

#[test]
fn concerns_keep_method_visibility_through_splicing_and_lowering() {
    for carrier in ["class_methods do", "module ClassMethods"] {
        let concern = format!(
            r#"module Helpers
  extend ActiveSupport::Concern
  private
  def instance_helper; 31; end
  {carrier}
    def class_wrapper; class_helper; end
    private def class_helper; 32; end
    protected
    def guarded; 33; end
  end
end"#
        );
        let tree: HashMap<PathBuf, Vec<u8>> = [
            ("db/schema.rb", "ActiveRecord::Schema.define do; create_table :things do |t|; t.string :name; end; end"),
            ("app/models/thing.rb", "class Thing < ApplicationRecord; include Helpers; def later; 34; end; end"),
            ("app/models/concerns/helpers.rb", concern.as_str()),
        ].into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest app");
        let model = &app.models[0];
        let methods: Vec<_> = model
            .body
            .iter()
            .filter_map(|i| match i {
                ModelBodyItem::Method { method, .. } => Some(method.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            visibility(&methods, "class_helper", MethodReceiver::Class),
            MethodVisibility::Private
        );
        assert_eq!(
            visibility(&methods, "class_wrapper", MethodReceiver::Class),
            MethodVisibility::Public
        );
        roundhouse::session::analyze_and_lower(&mut app);
        let output = roundhouse::emit::ruby::emit_lowered_models(&app);
        let model = &output
            .iter()
            .find(|f| f.path.ends_with("thing.rb"))
            .unwrap()
            .content;
        assert!(
            model.contains("end\n  private_class_method :class_helper"),
            "{model}"
        );
        assert!(model.contains("protected :guarded"), "{model}");
        assert!(model.contains("def later"), "{model}");
        assert!(!model.contains("\n  private\n"), "{model}");
        let libs = ingest_library_classes(concern.as_bytes(), "helpers.rb").unwrap();
        assert!(
            libs[0]
                .unknown_calls
                .iter()
                .all(|c| !roundhouse::emit::ruby::emit_expr(c).starts_with("private"))
        );
    }
}

/// A concern's `ClassMethods` carrier written with `class << self; def
/// extended(base)` hooks that singleton is the carrier module's own, not the
/// includers' class side the carrier is flattened into, so the refusal says so.
#[test]
fn a_class_methods_carrier_singleton_is_refused_by_name() {
    let source = "module Sharded\n  extend ActiveSupport::Concern\n\n  module ClassMethods\n    class << self\n      def extended(base)\n        base\n      end\n    end\n\n    def sharded?\n      true\n    end\n  end\nend\n";
    let error = ingest_library_classes(source.as_bytes(), "sharded.rb").unwrap_err();
    assert!(
        error.to_string().contains(
            "`class << self` in a concern's `ClassMethods` defines the carrier module's own singleton methods"
        ),
        "{error}"
    );
}
