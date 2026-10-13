use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::App;
use roundhouse::ingest::ingest_library_classes;

#[test]
fn top_level_constructor_refinements_apply_to_later_class_methods() {
    let source = r#"
module TopLevelConstructorRefinement
  refine Class do
    def new(label:)
      label
    end
  end
end

if true
  using TopLevelConstructorRefinement
end

class TopLevelRefinedTarget
  def initialize(label: "default")
    @label = label
  end
end

class TopLevelRefinedCaller
  def self.build
    TopLevelRefinedTarget.new(label: "refined")
  end
end
"#;
    let tree = [(
        PathBuf::from("app/services/top_level_refinement.rb"),
        source.as_bytes().to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest app");
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call = "TopLevelRefinedTarget.new(label: \"refined\")";
    let start = source.find(call).expect("constructor call") as u32;
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                        if construct.as_str() == "constructor keyword arguments"
                            && detail.contains("lexical refinement")
                )
        }),
        "a top-level `using` activation must keep later class method calls on the Ruby `new` lookup path: {diagnostics:#?}"
    );
}

#[test]
fn reopened_non_constructor_mixins_do_not_poison_struct_constructor_lookup() {
    let mut app = App::new();
    for source in [
        r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
end
"#,
        r#"module Factory
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
    def fixed
      Reading.new(label: "fixed").freeze
    end
  end
end
"#,
        r#"class Reading < T::Struct
  include Factory
  const :label, String
end
"#,
    ] {
        let classes = ingest_library_classes(source.as_bytes(), "reopened_struct_factory.rb")
            .expect("ingest");
        app.library_classes.extend(classes);
    }
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diagnostics.iter().all(|diagnostic| {
            !matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        }),
        "a complete reopened class-method carrier must not make an unrelated struct constructor unverified: {diagnostics:#?}"
    );
}

#[test]
fn directly_included_unmodeled_modules_make_constructor_lookup_unknown() {
    let source = r#"
class IncludedLookupTarget
  include ExternalConstructorHooks

  def initialize(label: "default")
    @label = label
  end
end

class IncludedLookupCaller
  def self.build
    IncludedLookupTarget.new(label: "value")
  end
end
"#;
    let classes = ingest_library_classes(source.as_bytes(), "unknown_include.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call_start = source
        .find("IncludedLookupTarget.new(label: \"value\")")
        .expect("constructor call") as u32;
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == call_start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported {
                        construct,
                        detail,
                        ..
                    } if construct.as_str() == "constructor keyword arguments"
                        && detail.contains("unmodeled constructor lookup")
                )
        }),
        "a directly included, unmodeled module may override class-side `new` or `initialize`: {diagnostics:#?}"
    );
}

#[test]
fn nested_keyword_splats_are_preserved_for_constructor_refusal() {
    let source = r#"
module Outer
  class Item
    def initialize(label: "default")
      @label = label
    end
  end

  def self.build(options)
    Item.new(**options)
  end
end
"#;
    let classes = ingest_library_classes(source.as_bytes(), "nested_splat.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call = "Item.new(**options)";
    let start = source.find(call).expect("nested splat call") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "keyword splat into source keyword parameters"
                )
        }),
        "without source scope provenance, a nested constructor packet must remain refused: {lower_diagnostics:#?}"
    );
}

#[test]
fn constructor_hooks_that_mutate_the_receivers_lookup_remain_unmodeled() {
    let source = r#"
module IncludedNewHook
  def self.included(base)
    base.define_singleton_method(:new) { |**options| options }
  end
end

class IncludedHookItem
  include IncludedNewHook

  def initialize(label: "default")
    @label = label
  end
end

class InheritedNewHook
  def self.inherited(child)
    child.define_singleton_method(:new) { |**options| options }
  end
end

class InheritedHookItem < InheritedNewHook
  def initialize(label: "default")
    @label = label
  end
end

module InstallingNewHook
  def self.extended(base)
    base.define_singleton_method(:new) { |**options| options }
  end
end

module DelegatingNewHook
  def self.extended(base)
    base.extend(InstallingNewHook)
  end
end

module DelegatedOpaqueNewHook
  def self.extended(base)
    install_lookup_override(base)
  end

  def self.install_lookup_override(base)
    base.singleton_class.prepend(InstallingNewHook)
  end
end

class DelegatedHookItem
  extend DelegatingNewHook

  def initialize(label: "default")
    @label = label
  end
end

class OpaqueDelegatedHookItem
  extend DelegatedOpaqueNewHook

  def initialize(label: "default")
    @label = label
  end
end

module UnrelatedIncludedHook
  def self.included(_base)
    Object.define_singleton_method(:new) { |**options| options }
  end
end

class UnrelatedIncludedHookItem
  include UnrelatedIncludedHook

  def initialize(label: "default")
    @label = label
  end
end

class ConstructorHookCalls
  def self.build
    [
      IncludedHookItem.new(**{ label: "included" }),
      InheritedHookItem.new(**{ label: "inherited" }),
      DelegatedHookItem.new(**{ label: "delegated" }),
      OpaqueDelegatedHookItem.new(**{ label: "opaque delegated" }),
      UnrelatedIncludedHookItem.new(label: "still lowered"),
    ]
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "constructor_hooks.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    for call in [
        "IncludedHookItem.new(**{ label: \"included\" })",
        "InheritedHookItem.new(**{ label: \"inherited\" })",
        "DelegatedHookItem.new(**{ label: \"delegated\" })",
        "OpaqueDelegatedHookItem.new(**{ label: \"opaque delegated\" })",
    ] {
        let start = source.find(call).expect("hook-mutated constructor call") as u32;
        assert!(
            lower_diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start == start
                    && matches!(
                        &diagnostic.kind,
                        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                            if matches!( construct.as_str(), "constructor keyword arguments"
                                | "keyword splat into source keyword parameters")
                    )
            }),
            "constructor lookup changed by a lifecycle hook must remain fail-closed: {lower_diagnostics:#?}"
        );
    }
    let unrelated_call = "UnrelatedIncludedHookItem.new(label: \"still lowered\")";
    let start = source.find(unrelated_call).expect("unrelated hook call") as u32;
    assert!(
        lower_diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a lifecycle hook mutating an unrelated receiver must not poison the includer's constructor: {lower_diagnostics:#?}"
    );
}

#[test]
fn explicit_constructor_lookup_mutations_are_attributed_to_their_receiver() {
    let source = r#"
module CustomNewMethods
  def new(label:)
    label
  end
end

class ExplicitlyExtendedItem
  def initialize(label: "default")
    @label = label
  end
end

class ReceiverMutationCaller
  def self.install
    ExplicitlyExtendedItem.public_send(:extend, CustomNewMethods)
  end

  def self.build
    ExplicitlyExtendedItem.new(label: "custom new")
  end
end

class UnrelatedMutationCaller
  def self.mutate(other)
    other.extend(CustomNewMethods)
  end

  def self.build
    ExplicitlyExtendedItem.new(label: "still custom new")
  end
end

"#;
    let classes = ingest_library_classes(source.as_bytes(), "explicit_constructor_mutation.rb")
        .expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call = "ExplicitlyExtendedItem.new(label: \"custom new\")";
    let start = source.find(call).expect("constructor call") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "an explicit constant receiver's constructor mutation must mark that receiver unmodeled: {lower_diagnostics:#?}"
    );
}

#[test]
fn mixin_operations_only_invalidate_the_lookup_chain_they_change() {
    let source = r#"
module InstanceNewOnly
  def new(label:)
    label
  end
end

module InstanceInitializeOnly
  def initialize(label:)
    @label = label
  end
end

module NestedCustomNew
  def new(label:)
    label
  end
end

module NewWrapper
  include NestedCustomNew

  def marker
    :wrapper
  end
end

module OmittedNewWrapper
  include NestedCustomNew
end

module OmittedNewBridge
  include NestedCustomNew
end

module WrapperWithMissingBridge
  include OmittedNewBridge

  def marker
    :wrapper
  end
end

class IncludesInstanceNew
  include InstanceNewOnly

  def initialize(label: "default")
    @label = label
  end
end

class ExtendsInstanceInitialize
  extend InstanceInitializeOnly

  def initialize(label: "default")
    @label = label
  end
end

class SingletonIncludesWrappedNew
  class << self
    include NewWrapper
  end

  def initialize(label: "default")
    @label = label
  end
end

class SingletonIncludesOmittedWrapper
  class << self
    include OmittedNewWrapper
  end

  def initialize(label: "default")
    @label = label
  end
end

class SingletonIncludesWrapperWithMissingBridge
  class << self
    include WrapperWithMissingBridge
  end

  def initialize(label: "default")
    @label = label
  end
end

class SingletonDynamicallyIncludesNew
  class << self
    send(:include, NestedCustomNew)
  end

  def initialize(label: "default")
    @label = label
  end
end

class HarmlessMixinCalls
  def self.build
    [
      IncludesInstanceNew.new(label: "included"),
      ExtendsInstanceInitialize.new(label: "extended"),
      SingletonIncludesWrappedNew.new(label: "wrapped"),
      SingletonIncludesOmittedWrapper.new(label: "omitted wrapper"),
      SingletonIncludesWrapperWithMissingBridge.new(label: "missing bridge"),
      SingletonDynamicallyIncludesNew.new(label: "dynamic include"),
    ]
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "mixin_lookup_sides.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    for call in [
        "IncludesInstanceNew.new(label: \"included\")",
        "ExtendsInstanceInitialize.new(label: \"extended\")",
    ] {
        let start = source.find(call).expect("mixin constructor call") as u32;
        assert!(
            lower_diagnostics.iter().all(|diagnostic| {
                diagnostic.span.start != start
                    || !matches!(
                        &diagnostic.kind,
                        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                            if construct.as_str() == "constructor keyword arguments"
                    )
            }),
            "a mixin that changes only the other constructor lookup chain must not block lowering: {lower_diagnostics:#?}"
        );
    }
    let wrapped_call = "SingletonIncludesWrappedNew.new(label: \"wrapped\")";
    let wrapped_start = source
        .find(wrapped_call)
        .expect("wrapped custom constructor") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == wrapped_start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                        if construct.as_str() == "constructor keyword arguments"
                            && detail.contains("unmodeled constructor lookup")
                )
        }),
        "a singleton mixin that inherits a custom `new` must preserve the refusal: {lower_diagnostics:#?}"
    );
    let omitted_call = "SingletonIncludesOmittedWrapper.new(label: \"omitted wrapper\")";
    let omitted_start = source.find(omitted_call).expect("omitted wrapper call") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == omitted_start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                        if construct.as_str() == "constructor keyword arguments"
                            && detail.contains("unmodeled constructor lookup")
                )
        }),
        "an omitted include-only mixin is unknown lookup, not proof of a harmless constructor: {lower_diagnostics:#?}"
    );
    for call in [
        "SingletonIncludesWrapperWithMissingBridge.new(label: \"missing bridge\")",
        "SingletonDynamicallyIncludesNew.new(label: \"dynamic include\")",
    ] {
        let start = source.find(call).expect("constructor call") as u32;
        assert!(
            lower_diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start == start
                    && matches!(
                        &diagnostic.kind,
                        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                            if construct.as_str() == "constructor keyword arguments"
                                && detail.contains("unmodeled constructor lookup")
                    )
            }),
            "{call} must be refused when an indirect or dynamic singleton mixin changes `new`: {lower_diagnostics:#?}"
        );
    }
}

#[test]
fn classes_without_constructor_contracts_still_make_short_names_ambiguous() {
    let factory_source = "class ItemFactory\n  def self.build\n    Item.new(label: \"model attribute\")\n  end\nend\n";
    let files = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :label\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/item.rb",
            "class Item < ApplicationRecord\nend\n",
        ),
        (
            "app/services/admin_item.rb",
            "module Admin\n  class Item\n    def initialize(label: \"default\")\n      @label = label\n    end\n  end\nend\n",
        ),
        ("app/services/item_factory.rb", factory_source),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest app");
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call_start = factory_source
        .find("Item.new(label: \"model attribute\")")
        .expect("model constructor call") as u32;
    assert!(
        lower_diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != call_start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "the unqualified model constructor must not be rebound to Admin::Item slots: {lower_diagnostics:#?}"
    );
    let emitted = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|file| file.path.to_string_lossy().ends_with(".rb"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !emitted.contains("Admin::Item.new"),
        "an unqualified model constructor must not be rebound to the same-named namespaced class:\n{emitted}"
    );
}

#[test]
fn unresolved_ambiguous_constructor_names_are_refused() {
    let source = r#"
module Outer
  class Item
    def initialize(label: "outer")
      @label = label
    end
  end
end

module Admin
  class Item
    def initialize(label: "admin")
      @label = label
    end
  end
end

class ItemFactory
  def self.build
    Item.new(label: "ambiguous")
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "ambiguous_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let start = source.find("Item.new(label: \"ambiguous\")").expect("call") as u32;
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                        if construct.as_str() == "constructor keyword arguments"
                            && detail.contains("unqualified class name is ambiguous")
                )
        }),
        "an unresolved short name with competing constructor owners must not silently retain unsafe keyword binding: {diagnostics:#?}"
    );
}

#[test]
fn unresolved_mixin_without_a_source_constructor_keeps_the_call_verbatim() {
    let source = r#"
class MixinOnlyConstructor
  include UnresolvedConstructorMixin
end

class MixinOnlyCaller
  def self.build
    MixinOnlyConstructor.new(label: "value", step: 2) { |row| row }
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "mixin_only_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        !diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "nothing in source was flattened, so the call keeps its Ruby binding: {diagnostics:#?}"
    );
    let emitted = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        emitted.contains("MixinOnlyConstructor.new(label: \"value\", step: 2)"),
        "{emitted}"
    );
    assert!(
        !emitted.contains("constructor keyword arguments not supported"),
        "{emitted}"
    );
}
