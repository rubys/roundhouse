//! Rails 8.1's exception constants are values, raise arguments, rescue
//! classes and type guards. The emitted runtime must define the same
//! names and parents, including the service/preview/transformer errors.

use super::emit_and_run;

const EXCEPTIONS: &[(&str, &str)] = &[
    ("ActiveStorage::Error", "StandardError"),
    ("ActiveStorage::InvariableError", "ActiveStorage::Error"),
    ("ActiveStorage::UnpreviewableError", "ActiveStorage::Error"),
    ("ActiveStorage::UnrepresentableError", "ActiveStorage::Error"),
    ("ActiveStorage::IntegrityError", "ActiveStorage::Error"),
    ("ActiveStorage::FileNotFoundError", "ActiveStorage::Error"),
    ("ActiveStorage::PreviewError", "ActiveStorage::Error"),
    ("ActiveStorage::InvalidKeyError", "ActiveStorage::Error"),
    ("ActiveStorage::Preview::UnprocessedError", "StandardError"),
    (
        "ActiveStorage::Service::GCSService::MetadataServerError",
        "ActiveStorage::Error",
    ),
    (
        "ActiveStorage::Service::GCSService::MetadataServerNotFoundError",
        "ActiveStorage::Error",
    ),
    (
        "ActiveStorage::Transformers::ImageProcessingTransformer::UnsupportedImageProcessingMethod",
        "StandardError",
    ),
    (
        "ActiveStorage::Transformers::ImageProcessingTransformer::UnsupportedImageProcessingArgument",
        "StandardError",
    ),
];

fn app(methods: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/storage_errors.rb",
            &format!("class StorageErrors < ApplicationRecord\n{methods}\nend\n"),
        )
}

#[test]
fn active_storage_exception_constant_values_keep_their_names_and_parents() {
    let names = EXCEPTIONS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ");
    let expected = EXCEPTIONS
        .iter()
        .map(|(name, parent)| format!("[\"{name}\", \"{parent}\"]"))
        .collect::<Vec<_>>()
        .join(", ");
    app(&format!("  def self.classes\n    [{names}]\n  end"))
        .run_ruby(&format!(
            "actual = StorageErrors.classes.map {{ |klass| [klass.name, klass.superclass.name] }}\n\
             raise actual.inspect unless actual == [{expected}]"
        ))
        .assert_passes();
}

#[test]
fn active_storage_exception_owners_can_be_reopened_with_rails_superclasses() {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write("app/models/storage_owners.rb", r#"module ActiveStorage
  module Transformers
    def self.marker
      "transformers"
    end
    class Transformer
    end
    class ImageProcessingTransformer < Transformer
      def self.marker
        "custom transformer"
      end
    end
  end
  class Service
    class GCSService < Service
      def self.marker
        "custom service"
      end
    end
  end
end
"#)
        .run_ruby(r#"
raise unless ActiveStorage::Transformers.marker == "transformers"
raise unless ActiveStorage::Transformers::ImageProcessingTransformer.marker == "custom transformer"
raise unless ActiveStorage::Service::GCSService.marker == "custom service"
raise unless ActiveStorage::Transformers::ImageProcessingTransformer.superclass == ActiveStorage::Transformers::Transformer
raise unless ActiveStorage::Transformers::Transformer.superclass == Object
raise unless ActiveStorage::Service::GCSService.superclass == ActiveStorage::Service
"#)
        .assert_passes();
}

#[test]
fn active_storage_exception_owner_values_have_the_rails_hierarchy() {
    app("  def self.owners\n    [ActiveStorage::Transformers, ActiveStorage::Transformers::Transformer,\n     ActiveStorage::Transformers::ImageProcessingTransformer, ActiveStorage::Service::GCSService]\n  end")
        .run_ruby(r#"
owners = StorageErrors.owners
raise owners.inspect unless owners.map(&:name) == [
  "ActiveStorage::Transformers", "ActiveStorage::Transformers::Transformer",
  "ActiveStorage::Transformers::ImageProcessingTransformer", "ActiveStorage::Service::GCSService"
]
raise unless owners[0].instance_of?(Module)
raise unless owners[1].superclass == Object
raise unless owners[2].superclass == owners[1]
raise unless owners[3].superclass == ActiveStorage::Service
"#)
        .assert_passes();
}

#[test]
fn active_storage_exception_owners_support_qualified_reopens_and_subclasses() {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write("app/models/storage_owners.rb", r#"class ActiveStorage::Transformers::ImageProcessingTransformer < ::ActiveStorage::Transformers::Transformer
  def self.marker
    "qualified transformer"
  end
end
class ActiveStorage::Service::GCSService < ::ActiveStorage::Service
  def self.marker
    "qualified service"
  end
end
class CustomTransformer < ActiveStorage::Transformers::ImageProcessingTransformer
end
class CustomService < ActiveStorage::Service::GCSService
end
"#)
        .run_ruby(r#"
raise unless CustomTransformer.marker == "qualified transformer"
raise unless CustomService.marker == "qualified service"
raise unless CustomTransformer.superclass.superclass == ActiveStorage::Transformers::Transformer
raise unless CustomService.superclass.superclass == ActiveStorage::Service
TransformerBase = ActiveStorage::Transformers::ImageProcessingTransformer
class AliasedTransformer < TransformerBase
end
ServiceBase = ActiveStorage::Service::GCSService
class AliasedService < ServiceBase
end
raise unless AliasedTransformer.marker == "qualified transformer"
raise unless AliasedService.marker == "qualified service"
raise unless ActiveStorage::Transformers::ImageProcessingTransformer::UnsupportedImageProcessingMethod.superclass == StandardError
raise unless ActiveStorage::Service::GCSService::MetadataServerError.superclass == ActiveStorage::Error
"#)
        .assert_passes();
}

#[test]
fn active_storage_exceptions_in_rescue_have_the_exception_surface() {
    let mut methods = String::new();
    let mut script = String::new();
    for (i, (name, parent)) in EXCEPTIONS.iter().enumerate() {
        methods.push_str(&format!(
            "  def self.rescued_{i}\n    raise ::{name}, \"storage failure\"\n\
             rescue {name} => error\n    error.message.upcase\n  end\n\
             def self.parent_rescue_{i}\n    raise {name}, \"parent rescue\"\n\
             rescue ::{parent} => error\n    error.message.upcase\n  end\n"
        ));
        script.push_str(&format!(
            "raise \"rescue {name}\" unless StorageErrors.rescued_{i} == \"STORAGE FAILURE\"\n\
             raise \"parent {name}\" unless StorageErrors.parent_rescue_{i} == \"PARENT RESCUE\"\n"
        ));
    }
    app(&methods).run_ruby(&script).assert_passes();
}

#[test]
fn active_storage_exceptions_can_be_raised_as_classes_or_instances() {
    let mut methods = String::new();
    let mut script = String::new();
    for (i, (name, _)) in EXCEPTIONS.iter().enumerate() {
        methods.push_str(&format!(
            "  def self.raise_class_{i}\n    raise {name}, \"storage failure\"\n  end\n\
             def self.raise_instance_{i}\n    raise ::{name}.new(\"storage failure\")\n  end\n"
        ));
        for form in ["class", "instance"] {
            script.push_str(&format!(
                "begin\n  StorageErrors.raise_{form}_{i}\nrescue {name} => error\n\
                 raise error.inspect unless error.class == {name} && error.message == \"storage failure\"\n\
                 else\n  raise \"did not raise {name}\"\nend\n"
            ));
        }
    }
    app(&methods).run_ruby(&script).assert_passes();
}

#[test]
fn active_storage_exception_is_a_guards_narrow_to_the_exception_surface() {
    let mut methods = String::new();
    let mut script = String::new();
    for (i, (name, _)) in EXCEPTIONS.iter().enumerate() {
        methods.push_str(&format!(
            "  def self.match_{i}(error)\n    if error.is_a?(::{name})\n\
                 error.message.upcase\n    else\n      \"different\"\n    end\n  end\n"
        ));
        script.push_str(&format!(
            "raise \"match {name}\" unless StorageErrors.match_{i}({name}.new(\"storage failure\")) == \"STORAGE FAILURE\"\n\
             raise \"nonmatch {name}\" unless StorageErrors.match_{i}(StandardError.new) == \"different\"\n"
        ));
    }
    app(&methods).run_ruby(&script).assert_passes();
}

#[test]
fn an_unknown_active_storage_exception_stays_refused() {
    let errors = app("  def self.unknown\n    ActiveStorage::UnknownStorageError\n  end").check_errors();
    assert!(
        errors.iter().any(|e| e.contains("ActiveStorage::UnknownStorageError")),
        "{errors:#?}"
    );
}

#[test]
fn active_storage_exception_methods_are_still_checked() {
    let root = std::env::temp_dir().join(format!("roundhouse-storage-methods-{}", std::process::id()));
    std::fs::create_dir_all(root.join("app/models")).unwrap();
    std::fs::write(
        root.join("app/models/application_record.rb"),
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/models/storage_errors.rb"),
        "class StorageErrors < ApplicationRecord\n  def checked\n    raise \"failure\"\n\
         rescue ActiveStorage::FileNotFoundError => error\n    error.no_such_exception_method\n  end\nend\n",
    )
    .unwrap();
    let mut app = roundhouse::ingest::ingest_app(&root).unwrap();
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == "send_dispatch_failed" && d.message.contains("no_such_exception_method")),
        "{diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|d| d.message.contains("constant not supported")),
        "{diagnostics:#?}"
    );
}

const CONSTRUCTOR_OWNERS: &[&str] = &[
    "ActiveStorage::Transformers::Transformer",
    "ActiveStorage::Transformers::ImageProcessingTransformer",
    "ActiveStorage::Service::GCSService",
];

#[test]
fn active_storage_exception_owner_constructors_stay_refused_in_every_operand_shape() {
    // Each receiver has the same unsupported contract, regardless of the
    // argument packet, the result's use, or how the class value is obtained.
    for owner in CONSTRUCTOR_OWNERS {
        let call = format!("{owner}.new({{ resize_to_limit: [10, 10] }})");
        let forms = [
            format!("::{call}"),
            format!("{owner}.new"),
            format!("{owner}.new({{}})"),
            format!("{owner}.new(public: true)"),
            format!("options = {{ resize_to_limit: [10, 10] }}; {owner}.new(options)"),
            format!("args = [{{ resize_to_limit: [10, 10] }}]; {owner}.new(*args)"),
            format!("options = {{ public: true }}; {owner}.new(**options)"),
            format!("{call} {{ 1 }}"),
            format!("owner = {owner}; owner.new({{}})"),
            "OwnerAlias.new({})".to_string(),
            "NamespaceAlias::OwnerAlias.new({})".to_string(),
            format!("{owner}.freeze.itself.new({{}})"),
            format!("[{owner}].first.new({{}})"),
            format!("{{ owner: {owner} }}[:owner].new({{}})"),
            format!("owner = flag ? {owner} : StandardError; owner.new({{}})"),
            "owner_value.new({})".to_string(),
            format!("construct({owner})"),
            format!("[{call}]"),
            format!("{{ {call} => 1 }}"),
            format!("{{ value: {call} }}"),
            format!("consume({call})"),
            format!("{call}.name"),
            format!("flag ? {call} : nil"),
            format!("flag && {call}"),
            format!("flag || {call}"),
            format!("{call}; 1"),
            format!("{owner}.send(:new, {{}})"),
            format!("{owner}.public_send(\"new\", {{}})"),
            format!("{owner}.__send__(:new, {{}})"),
            format!("{owner}.send(:public_send, :new, {{}})"),
            format!("name = :new; {owner}.public_send(name, {{}})"),
            format!("{owner}.method(:new).call({{}})"),
            format!("constructor = {owner}.public_method(\"new\"); constructor.call({{}})"),
            format!("{owner}.singleton_method(:new).call({{}})"),
            format!("name = :new; constructor = {owner}.method(name); constructor.call({{}})"),
            format!("{owner}.new({call})"),
            format!("{owner}.try(:new, {{}})"),
            format!("{owner}.try!(\"new\", {{}})"),
            format!("{owner}.then {{ |klass| klass.new({{}}) }}"),
            format!("{owner}.tap {{ |klass| klass.new({{}}) }}"),
            format!("[{owner}].map {{ |klass| klass.new({{}}) }}"),
        ];
        for form in forms {
            let methods = format!(
                "  OwnerAlias = {owner}\n  NamespaceAlias = StorageErrors\n\
                 def self.owner_value\n    {owner}\n  end\n\
                 def self.construct(owner)\n    owner.new({{}})\n  end\n\
                 def self.consume(value)\n    value\n  end\n\
                 def self.value(flag = true)\n    {form}\n  end\n"
            );
            let errors = app(&methods).check_errors();
            assert!(
                errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {form}: {errors:#?}"
            );
        }
        for declaration in [
            format!("  Value = {call}"),
            format!("  def self.value(arg = {call})\n    arg\n  end"),
            format!("  def value\n    {call}\n  end"),
        ] {
            let errors = app(&declaration).check_errors();
            assert!(errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {declaration}: {errors:#?}");
        }
    }
    let errors = app("  def self.value\n    ActiveStorage::Transformers.new\n  end").check_errors();
    assert!(errors.iter().any(|e| e.contains("ActiveStorage::Transformers.new lacks")), "{errors:#?}");
}

#[test]
fn active_storage_exception_owner_subclasses_and_empty_reopens_do_not_supply_constructors() {
    for owner in CONSTRUCTOR_OWNERS {
        for expression in ["new({})", "self.new({})", "StorageChild.new({})"] {
            let overlay = app("").write("app/models/storage_child.rb", &format!(
                "class {owner}\nend\n\
                 class StorageParent < {owner}\nend\n\
                 class StorageChild < StorageParent\n\
                   def self.value\n    {expression}\n  end\nend\n"
            ));
            let errors = overlay.check_errors();
            assert!(errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {expression}: {errors:#?}");
        }
    }
}

#[test]
fn active_storage_exception_owner_constructor_refusals_survive_emission() {
    use roundhouse::project::BuildTarget;
    for owner in CONSTRUCTOR_OWNERS {
        for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
            let (_, errors) = app(&format!("  def self.value\n    {owner}.new({{ resize_to_limit: [10, 10] }})\n  end"))
                .emit(target);
            assert!(errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {target:?}: {errors:#?}");
        }
        // The permissive harness still writes output. It must execute an
        // explicit refusal stub instead of the previously accepted arity crash.
        let run = app(&format!("  def self.value\n    {owner}.new({{ resize_to_limit: [10, 10] }})\n  end"))
            .run_ruby("StorageErrors.value");
        assert!(run.errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")));
        assert!(!run.success && run.stderr.contains("ActiveStorage constructor not supported"), "{}", run.stderr);
    }
}

#[test]
fn active_storage_exception_owner_source_initializers_execute() {
    for owner in CONSTRUCTOR_OWNERS {
        app(&format!("  def self.value\n    {owner}.new(7).marker\n  end"))
            .write("app/models/storage_constructor.rb", &format!(
                "class {owner}\n  attr_reader :marker\n\
                   def initialize(marker)\n    @marker = marker\n  end\nend\n"
            ))
            .run_ruby("raise unless StorageErrors.value == 7")
            .assert_passes();
    }
    // Initializers inherited through the newly registered runtime parent,
    // and source subclass/mixin initializers, supply the constructor too.
    app("  def self.value\n    ActiveStorage::Transformers::ImageProcessingTransformer.new(9).marker\n  end")
        .write("app/models/storage_constructor.rb", r#"class ActiveStorage::Transformers::Transformer
  attr_reader :marker
  def initialize(marker)
    @marker = marker
  end
end
"#)
        .run_ruby("raise unless StorageErrors.value == 9")
        .assert_passes();
    for owner in CONSTRUCTOR_OWNERS {
        app("  def self.value\n    StorageChild.new(11).marker\n  end")
            .write("app/models/storage_constructor.rb", &format!(
                "module StorageInitializer\n  attr_reader :marker\n\
                   def initialize(marker)\n    @marker = marker\n  end\nend\n\
                 class StorageParent < {owner}\n  include StorageInitializer\nend\n\
                 class StorageChild < StorageParent\nend\n"
            ))
            .run_ruby("raise unless StorageErrors.value == 11")
            .assert_passes();
    }
}

#[test]
fn active_storage_exception_owner_source_class_constructors_execute() {
    for owner in CONSTRUCTOR_OWNERS.iter().copied().chain(["ActiveStorage::Transformers"]) {
        app(&format!("  def self.value\n    {owner}.new(13)\n  end"))
            .write("app/models/storage_constructor.rb", &format!(
                "{} {owner}\n  def self.new(value)\n    value + 1\n  end\nend\n",
                if owner == "ActiveStorage::Transformers" { "module" } else { "class" }
            ))
            .run_ruby("raise unless StorageErrors.value == 14")
            .assert_passes();
    }
}

#[test]
fn active_storage_exception_owner_constructors_delegating_to_missing_super_stay_refused() {
    for owner in CONSTRUCTOR_OWNERS {
        for method in ["initialize", "self.new"] {
            for delegation in ["super", "super(options)", "if options; super(options); end"] {
                let errors = app("  def self.value\n    StorageChild.new({})\n  end")
                    .write("app/models/storage_child.rb", &format!(
                        "class StorageChild < {owner}\n\
                           def {method}(options)\n    {delegation}\n  end\nend\n"
                    ))
                    .check_errors();
                assert!(errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                    "{owner}: {method}: {delegation}: {errors:#?}");
            }
        }
    }
}

#[test]
fn active_storage_exception_owner_constructor_super_can_reach_a_source_initializer() {
    app("  def self.value\n    ActiveStorage::Transformers::ImageProcessingTransformer.new(17).marker\n  end")
        .write("app/models/storage_constructor.rb", r#"class ActiveStorage::Transformers::Transformer
  attr_reader :marker
  def initialize(marker)
    @marker = marker
  end
end
class ActiveStorage::Transformers::ImageProcessingTransformer
  def initialize(marker)
    super(marker)
  end
  def self.new(marker)
    super(marker)
  end
end
"#)
        .run_ruby("raise unless StorageErrors.value == 17")
        .assert_passes();
}

#[test]
fn active_storage_exception_owner_safe_literal_reflection_still_executes() {
    for owner in CONSTRUCTOR_OWNERS {
        app(&format!("  def self.value\n    [{owner}.send(:name), {owner}.public_send(\"name\"), {owner}.__send__(:name)]\n  end"))
            .run_ruby(&format!("raise unless StorageErrors.value == [\"{owner}\"] * 3"))
            .assert_passes();
    }
}

/// Every owner the namespace registration exposes, the module included.
const REFLECTION_OWNERS: &[(&str, &str)] = &[
    ("ActiveStorage::Transformers", "module"),
    ("ActiveStorage::Transformers::Transformer", "class"),
    ("ActiveStorage::Transformers::ImageProcessingTransformer", "class"),
    ("ActiveStorage::Service::GCSService", "class"),
];

#[test]
fn active_storage_exception_owner_public_send_and_send_with_safe_names_execute() {
    // These checked and ran before constructors were refused: a name that
    // cannot be `new`, through a local bound once, a copy, a conditional,
    // nested reflection and a literal symbol list.
    for (owner, _) in REFLECTION_OWNERS {
        app(&format!(
            "  def self.value(flag = true)\n    name = :name\n    other = name\n    either = flag ? :name : :to_s\n    \
             [{owner}.public_send(name), {owner}.send(name), {owner}.__send__(other), {owner}.public_send(either),\n     \
             {owner}.send(flag ? :name : :to_s), {owner}.public_send(nil || :name), {owner}.send(:public_send, name),\n     \
             {owner}.method(name).call, %i[name to_s].map {{ |m| {owner}.public_send(m) }}.first,\n     \
             [:name].each_with_object([]) {{ |m, out| out << {owner}.send(m) }}.first]\n  end"
        ))
        .run_ruby(&format!(
            "actual = StorageErrors.value\nraise actual.inspect unless actual == [\"{owner}\"] * 10"
        ))
        .assert_passes();
    }
}

#[test]
fn active_storage_exception_owner_source_public_send_and_send_answer_literal_names() {
    // A source class method answers the reflective call in place of
    // Ruby's, so no constructor runs.
    for (owner, kind) in REFLECTION_OWNERS {
        app(&format!(
            "  def self.value\n    [{owner}.public_send(:new, {{}}), {owner}.send(:public_send, :new, {{}}),\n     \
             {owner}.__send__(:public_send, \"new\", {{}})]\n  end"
        ))
        .write("app/models/storage_owner.rb", &format!(
            "{kind} {owner}\n  def self.public_send(name, *args)\n    19\n  end\nend\n"
        ))
        .run_ruby("actual = StorageErrors.value\nraise actual.inspect unless actual == [19, 19, 19]")
        .assert_passes();
        app(&format!("  def self.value\n    [{owner}.send(:new, {{}}), {owner}.public_send(:send, :new, {{}})]\n  end"))
            .write("app/models/storage_owner.rb", &format!(
                "{kind} {owner}\n  class << self\n    def send(name, *args)\n      23\n    end\n  end\nend\n"
            ))
            .run_ruby("actual = StorageErrors.value\nraise actual.inspect unless actual == [23, 23]")
            .assert_passes();
    }
    // Inherited by a Rails subclass, and declared by a source subclass.
    app("  def self.value\n    [ActiveStorage::Transformers::ImageProcessingTransformer.public_send(:new, {}),\n     \
         ActiveStorage::Service::GCSService.send(:new, {}), StorageChild.public_send(:new, {})]\n  end")
        .write("app/models/storage_owner.rb", "class ActiveStorage::Transformers::Transformer\n  \
                def self.public_send(name, *args)\n    19\n  end\nend\n\
                class ActiveStorage::Service\n  def self.send(name, *args)\n    23\n  end\nend\n\
                class StorageChild < ActiveStorage::Service::GCSService\n  \
                def self.public_send(name, *args)\n    29\n  end\nend\n")
        .run_ruby("actual = StorageErrors.value\nraise actual.inspect unless actual == [19, 23, 29]")
        .assert_passes();
}

#[test]
fn active_storage_exception_owner_public_send_and_send_reaching_new_stay_refused() {
    for (owner, kind) in REFLECTION_OWNERS {
        let forms = [
            format!("name = :new\n    {owner}.public_send(name, {{}})"),
            format!("name = :new\n    {owner}.send(name, {{}})"),
            format!("name = :name\n    other = name\n    other = :new\n    {owner}.send(other, {{}})"),
            format!("name = :name\n    name = :new if flag\n    {owner}.public_send(name, {{}})"),
            format!("name = :name\n    reset = -> {{ name = :new }}\n    reset.call\n    {owner}.send(name, {{}})"),
            format!("name = :name\n    [1].each {{ name = :new }}\n    {owner}.public_send(name, {{}})"),
            format!("name = :name\n    binding.local_variable_set(:name, :new)\n    {owner}.send(name, {{}})"),
            format!("{owner}.public_send(selector, {{}})"),
            format!("{owner}.send(flag ? :name : :new, {{}})"),
            format!("name = :public_send\n    {owner}.send(name, :new, {{}})"),
            format!("%i[name new].each {{ |m| {owner}.public_send(m, {{}}) }}"),
            format!("%i[name].each {{ |m| m = :new; {owner}.send(m, {{}}) }}"),
            format!("name = :name\n    [:new].each {{ |name| {owner}.public_send(name, {{}}) }}"),
        ];
        for form in forms {
            let errors = app(&format!("  def self.value(selector = :name, flag = true)\n    {form}\n  end"))
                .check_errors();
            assert!(
                errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {form}: {errors:#?}"
            );
        }
        // Source methods that hand the call back through `super`, answer a
        // different name, or that the emitted direct call never reaches:
        // `try` and an enumerable computed name lower to `recv.new(...)`.
        for (method, body, form) in [
            ("public_send", "super", format!("{owner}.public_send(:new, {{}})")),
            ("send", "19", format!("{owner}.public_send(:new, {{}})")),
            ("public_send", "19", format!("%i[new].each {{ |m| {owner}.public_send(m, {{}}) }}")),
            ("public_send", "19", format!("{owner}.try(:new, {{}})")),
        ] {
            let errors = app(&format!("  def self.value\n    {form}\n  end"))
                .write("app/models/storage_owner.rb", &format!(
                    "{kind} {owner}\n  def self.{method}(name, *args)\n    {body}\n  end\nend\n"
                ))
                .check_errors();
            assert!(
                errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
                "{owner}: {method}: {form}: {errors:#?}"
            );
        }
    }
}

#[test]
fn active_storage_exception_owner_derived_class_constructors_keep_the_refusal() {
    for owner in CONSTRUCTOR_OWNERS {
        let forms = [
            format!("Class.new({owner}).new({{}})"),
            format!("child = Class.new({owner}); child.new({{}})"),
            format!("Class.new({owner}) {{ 1 }}.new({{}})"),
            format!("owner = flag ? {owner} : nil; owner&.new({{}})"),
            format!("[StandardError, {owner}][1].new({{}})"),
            format!("{owner}.ancestors.first.new({{}})"),
            format!("ancestors = {owner}.ancestors; ancestors[0].new({{}})"),
            format!("{owner}.public_send(:ancestors).first.new({{}})"),
            format!("{owner}.method(:ancestors).call.first.new({{}})"),
            format!("{owner}.subclasses.first.new({{}})"),
            format!("children = {owner}.subclasses; children.first.new({{}})"),
            format!("{owner}.method(:itself).call.new({{}})"),
            "StorageChild.superclass.new({})".to_string(),
            "parent = StorageChild.superclass; parent.new({})".to_string(),
            "FactoryChild.new({})".to_string(),
            "derived_class.new({})".to_string(),
            "construct(derived_class)".to_string(),
            format!("parent = flag ? {owner} : StandardError; Class.new(parent).new({{}})"),
            "Class.new(flag ? derived_class : StandardError).new({})".to_string(),
        ];
        for form in forms {
            let errors = app(&format!(
                "  FactoryChild = Class.new({owner})\n\
                 def self.derived_class\n    Class.new({owner})\n  end\n\
                 def self.construct(klass)\n    klass.new({{}})\n  end\n\
                 def self.value(flag = true)\n    {form}\n  end"
            ))
                .write("app/models/storage_child.rb", &format!("class StorageChild < {owner}\nend\n"))
                .check_errors();
            let refused = if form == "FactoryChild.new({})" {
                // This model constant factory already lies outside the indexed
                // class-factory surface. Preserve its existing constant refusal.
                !errors.is_empty()
            } else {
                errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation"))
            };
            assert!(refused, "{owner}: {form}: {errors:#?}");
        }
    }
    for receiver in [
        "ActiveStorage::Transformers::ImageProcessingTransformer.superclass",
        "ActiveStorage::Transformers::ImageProcessingTransformer.method(:superclass).call",
    ] {
        let errors = app(&format!("  def self.value\n    parent = {receiver}\n    parent.new({{}})\n  end"))
            .check_errors();
        assert!(errors.iter().any(|e| e.contains(".new lacks a shared runtime constructor implementation")),
            "{receiver}: {errors:#?}");
    }
}

#[test]
fn active_storage_exception_owner_derived_class_values_still_execute() {
    for owner in CONSTRUCTOR_OWNERS {
        app(&format!("  def self.value\n    [Class.new({owner}), {owner}.ancestors]\n  end"))
            .run_ruby(&format!("child, ancestors = StorageErrors.value\n\
                               raise unless child.superclass == {owner}\n\
                               raise unless ancestors.include?({owner})"))
            .assert_passes();
    }
    app("  def self.value\n    ActiveStorage::Transformers::ImageProcessingTransformer.superclass\n  end")
        .run_ruby("raise unless StorageErrors.value == ActiveStorage::Transformers::Transformer")
        .assert_passes();
}
