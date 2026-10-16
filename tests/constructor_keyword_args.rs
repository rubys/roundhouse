//! A keyword passed to `.new` must bind to its source `initialize` slot
//! after optional keywords are flattened for the emitted Ruby ABI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

static RUN: AtomicUsize = AtomicUsize::new(0);

const SOURCE: &str = r#"
class Tally
  attr_reader :label, :step

  def initialize(label, step: 1)
    @label = label
    @step = step
  end
end

class ChildTally < Tally
end

class Notice
  attr_reader :message, :hint

  def initialize(message, hint: nil)
    @message = message
    @hint = hint
  end
end

class CustomConstructor
  def self.new(step:)
    step
  end

  def initialize(step: 1)
    @step = step
  end
end

class KeywordRestCustomConstructor
  def self.new(**options)
    options
  end
end

class StringAliasedCustomNew
  class << self
    def factory(other:)
      other
    end

    alias_method "new", "factory"
  end
end

module UnrelatedConstructorExtension
end

class UnrelatedReceiverMutation
  attr_reader :label

  def initialize(label: "default")
    @label = label
  end

  def self.mutate(other)
    other.extend(UnrelatedConstructorExtension)
  end
end

class ParentCustomConstructor
  def self.new(label: "parent default")
    "parent:#{label}"
  end

  def initialize(label:)
    @label = label
  end
end

class ChildCustomConstructor < ParentCustomConstructor
end

module ExtendedConstructor
  def new(label: "extended default")
    "extended:#{label}"
  end
end

class ExtendedCustomConstructor
  extend ExtendedConstructor

  def initialize(label: "default")
    @label = label
  end
end

module ReceiverExtendedConstructorMethods
  def new(label:)
    "receiver-extended:#{label}"
  end
end

class ReceiverExtendedConstructor
  self.extend ReceiverExtendedConstructorMethods

  def initialize(label: "default")
    @label = label
  end
end

class ClassEvalConstructor
  def initialize(label: "default")
    @label = label
  end

  class_eval "def self.new(label:); label; end"
end

class HookConstructor
  def initialize(label: "default")
    @label = label
  end

  def self.method_added(_name)
  end
end

class OverrideInitialize < Tally
  attr_reader :name

  def initialize(name:)
    @name = name
  end
end

class PositionalHashConstructor
  attr_reader :options

  def initialize(options = {})
    @options = options
  end
end

class UnrelatedDslConstructor
  attr_reader :label

  validates :label, presence: true

  def initialize(label: "default")
    @label = label
  end
end

class ConstructorOrder
  def initialize(first: nil, second: nil)
    @first = first
    @second = second
  end

  def self.tick(value)
    $constructor_events << value
    value
  end
end

class ContextDefaultConstructor
  def initialize(first: next_value, second: nil)
    @first = first
    @second = second
  end

  def next_value
    "contextual default"
  end
end

class ConstantDefaultConstructor
  LABEL = "class label"

  def initialize(label: LABEL, step: 1)
    @label = label
    @step = step
  end
end

class RegexDefaultConstructor
  def initialize(pattern: /café/, step: 1)
    @pattern = pattern
    @step = step
  end
end

module PrependedInitializer
  def initialize(label: "prepended")
    @label = "prepended:#{label}"
  end
end

class PrependedConstructor
  def initialize(label: "own")
    @label = "own:#{label}"
  end

  prepend PrependedInitializer
end

module SingletonConstructorMethods
  def new(label:)
    "singleton:#{label}"
  end
end

class SingletonIncludedConstructor
  class << self
    include SingletonConstructorMethods
  end

  def initialize(label: "default")
    @label = label
  end
end

module SingletonPrependedNew
  def new(label:)
    "prepended singleton:#{label}"
  end
end

class SingletonPrependedConstructor
  class << self
    prepend SingletonPrependedNew
  end

  def initialize(label: "default")
    @label = label
  end
end

class OtherConstructorContext
  LABEL = "caller label"

  def self.build
    ConstantDefaultConstructor.new(step: 2)
  end
end

class ConstructorCalls
  def self.build
    [
      Tally.new("widgets", step: 2),
      ChildTally.new("child", step: 3),
      Notice.new("low stock", hint: "reorder"),
      Tally.new("default"),
      OverrideInitialize.new(name: "child override"),
      PositionalHashConstructor.new(options: "kept as Hash"),
      UnrelatedDslConstructor.new(label: "still lowered"),
    ]
  end

  def self.custom_new
    CustomConstructor.new(step: 2)
  end

  def self.keyword_rest_custom_new
    KeywordRestCustomConstructor.new(**{ label: "x" })
  end

  def self.unrelated_receiver_mutation
    UnrelatedReceiverMutation.new(label: "accepted")
  end

  def self.inherited_custom_new
    ChildCustomConstructor.new(label: "y")
  end

  def self.extended_custom_new
    ExtendedCustomConstructor.new(label: "z")
  end
end

class UnsafeConstructorCalls
  def self.build
    [
      ConstructorOrder.new(second: tick("second"), first: tick("first")),
      ConstructorOrder.new(first: tick("first duplicate"), first: tick("last duplicate")),
      Tally.new("unknown keyword", unsupported: tick("unsupported")),
      ContextDefaultConstructor.new(second: 2),
      Tally.new(*["splat"], step: 1),
      Tally.new("splat", **{ step: 2 }),
      Tally.new(**{ label: "keyword splat" }),
      ExtendedCustomConstructor.new(**{ label: "splat" }),
      StringAliasedCustomNew.new(label: "x"),
      OtherConstructorContext.build,
      RegexDefaultConstructor.new(step: 2),
      PrependedConstructor.new(label: "z"),
      SingletonIncludedConstructor.new(label: "z"),
      SingletonPrependedConstructor.new(label: "z"),
      ReceiverExtendedConstructor.new(label: "z"),
      ClassEvalConstructor.new(label: "z"),
      HookConstructor.new(label: "z"),
    ]
  end

  def self.tick(value)
    $constructor_events << value
    value
  end
end
"#;

fn emitted() -> Vec<roundhouse::emit::EmittedFile> {
    let classes = ingest_library_classes(SOURCE.as_bytes(), "constructors.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    roundhouse::session::analyze_and_lower(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|file| {
            file.path
                .extension()
                .is_some_and(|extension| extension == "rb")
        })
        .collect()
}

#[test]
fn constructor_keywords_bind_to_the_flattened_initialize_slots() {
    let emitted = emitted();
    let source = emitted
        .iter()
        .map(|file| file.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        source.contains("Tally.new(\"widgets\", 2)"),
        "the constructor keyword should move to initialize's `step` slot:\n{source}"
    );
    assert!(
        source.contains("Notice.new(\"low stock\", \"reorder\")"),
        "the constructor keyword should move to initialize's `hint` slot:\n{source}"
    );
    assert!(
        source.contains("ChildTally.new(\"child\", 3)"),
        "an inherited initialize contract should apply to the child constructor:\n{source}"
    );
    assert!(
        source.contains("KeywordRestCustomConstructor.new(**{ label: \"x\" })"),
        "keyword-rest custom `new` must retain its keyword-splat call ABI:\n{source}"
    );

    let dir = std::env::temp_dir().join(format!(
        "roundhouse-constructor-keywords-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for file in &emitted {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().expect("emitted file parent")).expect("mkdir");
        std::fs::write(path, &file.content).expect("write emitted source");
    }
    let requires = emitted
        .iter()
        .map(|file| format!("require_relative {:?}", file.path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");
    let script = format!(
        "{}\n{}",
        requires,
        [
        "$constructor_events = []",
        "results = ConstructorCalls.build",
        "tally, child, notice, default_tally = results",
        "raise 'wrong tally label' unless tally.label == 'widgets'",
        "raise 'keyword Hash bound to step' unless tally.step == 2",
        "raise 'wrong child step' unless child.step == 3",
        "raise 'wrong notice message' unless notice.message == 'low stock'",
        "raise 'keyword Hash bound to hint' unless notice.hint == 'reorder'",
        "raise 'constructor default changed' unless default_tally.step == 1",
        "raise 'child initialize override was ignored' unless results[4].name == 'child override'",
        "raise 'positional Hash contract changed' unless results[5].options == { options: 'kept as Hash' }",
        "raise 'unrelated class DSL blocked constructor lowering' unless results[6].label == 'still lowered'",
        "raise 'custom class-side new changed' unless ConstructorCalls.custom_new == 2",
        "raise 'custom new keyword-rest lost its keyword Hash' unless ConstructorCalls.keyword_rest_custom_new == { label: 'x' }",
        "raise 'child inherited custom new changed' unless ConstructorCalls.inherited_custom_new == 'parent:y'",
        ]
        .join("\n")
    );
    let output = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&dir)
        .output()
        .expect("run Ruby");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "emitted constructor behavior failed:\n{}\n{}\n{source}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unsafe_keyword_order_and_duplicate_keys_are_rejected() {
    let classes = ingest_library_classes(SOURCE.as_bytes(), "constructors.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    let assert_refused_at = |call: &str, detail: Option<&str>| {
        let start = SOURCE
            .find(call)
            .unwrap_or_else(|| panic!("missing source call {call:?}")) as u32;
        assert!(
            lower_diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start == start
                    && matches!(
                        &diagnostic.kind,
                        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail: actual, .. }
                            if construct.as_str() == "constructor keyword arguments"
                                && detail.is_none_or(|expected| actual.contains(expected))
                    )
            }),
            "expected a refusal diagnostic for {call:?}: {lower_diagnostics:#?}"
        );
    };
    for call in [
        "ConstructorOrder.new(second: tick(\"second\"), first: tick(\"first\"))",
        "ConstructorOrder.new(first: tick(\"first duplicate\"), first: tick(\"last duplicate\"))",
        "Tally.new(\"unknown keyword\", unsupported: tick(\"unsupported\"))",
        "ContextDefaultConstructor.new(second: 2)",
        "Tally.new(*[\"splat\"], step: 1)",
        "Tally.new(\"splat\", **{ step: 2 })",
        "Tally.new(**{ label: \"keyword splat\" })",
        "ExtendedCustomConstructor.new(**{ label: \"splat\" })",
        "ConstantDefaultConstructor.new(step: 2)",
        "RegexDefaultConstructor.new(step: 2)",
    ] {
        assert_refused_at(call, None);
    }
    for call in [
        "PrependedConstructor.new(label: \"z\")",
        "SingletonIncludedConstructor.new(label: \"z\")",
        "SingletonPrependedConstructor.new(label: \"z\")",
        "ReceiverExtendedConstructor.new(label: \"z\")",
        "ClassEvalConstructor.new(label: \"z\")",
        "HookConstructor.new(label: \"z\")",
        "ExtendedCustomConstructor.new(label: \"z\")",
    ] {
        assert_refused_at(call, Some("unmodeled constructor lookup"));
    }
    assert!(
        diagnostics.iter().all(|diagnostic| {
            !matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        }),
        "lowering refusals are already returned directly and must not duplicate expression annotations: {diagnostics:#?}"
    );
    let emitted_files = emit_library(&app)
        .into_iter()
        .filter(|file| {
            file.path
                .extension()
                .is_some_and(|extension| extension == "rb")
        })
        .collect::<Vec<_>>();
    let emitted = emitted_files
        .iter()
        .map(|file| file.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        emitted.contains("constructor keyword arguments"),
        "the unsafe constant default must emit a refusal stub, not a caller-scoped constant: {emitted}"
    );
    assert!(
        !emitted.contains("ReceiverExtendedConstructor.new(label: \"z\")"),
        "a receiver-extended constructor is refused because emitted lookup does not retain `extend`: {emitted}"
    );
}

#[test]
fn custom_class_side_new_methods_are_not_treated_as_initialize_forwarders() {
    let emitted = emitted()
        .into_iter()
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        emitted.contains("CustomConstructor.new(step: 2)"),
        "custom `.new` keeps its class-method keyword contract:\n{emitted}"
    );
    assert!(
        emitted.contains("ChildCustomConstructor.new(\"y\")"),
        "an inherited custom `new` uses its flattened keyword slot:\n{emitted}"
    );
    assert!(
        !emitted.contains("ExtendedCustomConstructor.new(\"z\")"),
        "an `extend`-provided constructor is refused because emitted class lookup does not retain `extend`:\n{emitted}"
    );
}

#[test]
fn string_aliases_and_unrelated_receivers_are_classified_conservatively() {
    let classes = ingest_library_classes(SOURCE.as_bytes(), "constructors.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let assert_constructor_refused = |call: &str| {
        let start = SOURCE.find(call).expect("source call") as u32;
        assert!(
            lower_diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start == start
                    && matches!(
                        &diagnostic.kind,
                        roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                            if construct.as_str() == "constructor keyword arguments"
                    )
            }),
            "expected constructor refusal at {call:?}: {lower_diagnostics:#?}"
        );
    };
    assert_constructor_refused("StringAliasedCustomNew.new(label: \"x\")");

    let unrelated_call = "UnrelatedReceiverMutation.new(label: \"accepted\")";
    let start = SOURCE.find(unrelated_call).unwrap_or_else(|| {
        panic!("expected {unrelated_call:?} to be present in the constructor fixture")
    }) as u32;
    assert!(
        lower_diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a method mutating an unrelated receiver must not disable the class's constructor contract"
    );
    assert!(
        emit_library(&app).into_iter().any(|file| {
            file.content
                .contains("UnrelatedReceiverMutation.new(\"accepted\")")
        }),
        "the known initialize keyword should lower to its positional slot"
    );
}

#[test]
fn unknown_constructor_lookup_is_reported_without_flattened_slots_elsewhere() {
    let source = r#"
class HookOnlyConstructor
  def self.method_added(_name)
  end

  def initialize(label:)
    @label = label
  end

  def self.build
    HookOnlyConstructor.new(label: "z")
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "hook_only_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                if construct.as_str() == "constructor keyword arguments"
                    && detail.contains("unmodeled constructor lookup")
        )),
        "an uncertain new lookup must be refused even if no initialize slot was flattened: {lower_diagnostics:#?}"
    );
}

#[test]
fn lookup_hooks_override_even_a_declared_custom_new_contract() {
    let source = r#"
class HookedCustomConstructor
  def self.new(label: "default")
    label
  end

  def self.method_added(_name)
  end

  def self.build
    HookedCustomConstructor.new(label: "z")
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "hooked_custom_new.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                if construct.as_str() == "constructor keyword arguments"
                    && detail.contains("unmodeled constructor lookup")
        )),
        "a method_added hook may replace an apparently declared class-side `new`: {diagnostics:#?}"
    );
    let emitted = emit_library(&app)
        .into_iter()
        .filter(|file| {
            file.path
                .extension()
                .is_some_and(|extension| extension == "rb")
        })
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(emitted.contains("constructor keyword arguments"));
}

#[test]
fn a_custom_new_added_by_reopening_is_not_missed() {
    let source = r#"
class ReopenedConstructor
  def initialize(label: "default")
    @label = label
  end

  def self.build
    ReopenedConstructor.new(label: "z")
  end
end

class ReopenedConstructor
  def self.new(label:)
    "custom:#{label}"
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "reopened_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "an unverified reopened class hierarchy must not infer default Class#new dispatch"
    );
}

#[test]
fn removing_or_undefining_initialize_blocks_stale_constructor_contracts() {
    for mutation in ["remove_method :initialize", "undef_method :initialize"] {
        let source = format!(
            r#"
class ParentLookup
  def initialize(label: "parent", step: 1)
    @label = label
    @step = step
  end
end

class ChildLookup < ParentLookup
  def initialize(step: 2, label: "child")
    @label = label
    @step = step
  end

  {mutation}

  def self.build
    ChildLookup.new(step: 3, label: "x")
  end
end
"#
        );
        let classes =
            ingest_library_classes(source.as_bytes(), "mutated_initialize.rb").expect("ingest");
        let mut app = App::new();
        app.library_classes.extend(classes);
        let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
        assert!(
            lower_diagnostics.iter().any(|diagnostic| matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                    if construct.as_str() == "constructor keyword arguments"
                        && detail.contains("unmodeled constructor lookup")
            )),
            "{mutation} must prevent trusting the syntactically present but removed initializer"
        );
        let emitted = emit_library(&app)
            .into_iter()
            .filter(|file| {
                file.path
                    .extension()
                    .is_some_and(|extension| extension == "rb")
            })
            .map(|file| file.content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            emitted.contains("constructor keyword arguments"),
            "{mutation} must leave a refusal stub instead of emitting a stale positional binding"
        );
    }
}

#[test]
fn lexical_constructor_refinements_are_not_rewritten_as_initialize_calls() {
    let source = r#"
class RefinedTarget
  def initialize(label: "default")
    @label = label
  end
end

module ConstructorRefinement
  refine RefinedTarget.singleton_class do
    def new(label:)
      "refined:#{label}"
    end
  end
end

class RefinedCaller
  BEFORE = RefinedTarget.new(label: "before constant")

  def self.before_refinement
    RefinedTarget.new(label: "before")
  end

  using ConstructorRefinement

  AFTER = RefinedTarget.new(**{ label: "after constant" })

  def self.build
    RefinedTarget.new(label: "z")
  end
end

module OuterRefinementScope
  using ConstructorRefinement

  def self.marker
    :outer
  end

  class NestedCaller
    def self.build
      RefinedTarget.new(label: "nested scope")
    end
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "refined_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let lower_diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "a using/refinement scope can replace `.new`, so the call cannot be rebound to initialize"
    );
    let before_call = "RefinedTarget.new(label: \"before\")";
    let before_start = source.find(before_call).expect("pre-refinement call") as u32;
    assert!(
        lower_diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != before_start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a method defined before `using` is outside the refinement scope"
    );
    let before_constant = "RefinedTarget.new(label: \"before constant\")";
    let before_constant_start = source
        .find(before_constant)
        .expect("pre-refinement constant") as u32;
    assert!(
        lower_diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != before_constant_start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a constant initializer before `using` is outside the refinement scope"
    );
    let after_constant = "RefinedTarget.new(**{ label: \"after constant\" })";
    let after_constant_start = source
        .find(after_constant)
        .expect("post-refinement constant") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == after_constant_start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a constructor splat in the active refinement scope must be preserved and refused"
    );
    let nested_call = "RefinedTarget.new(label: \"nested scope\")";
    let nested_start = source.find(nested_call).expect("nested refinement call") as u32;
    assert!(
        lower_diagnostics.iter().any(|diagnostic| {
            diagnostic.span.start == nested_start
                && matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a refinement activated in an enclosing lexical scope applies to nested class methods"
    );
}

#[test]
fn an_unqualified_nested_constructor_uses_its_flattened_slots() {
    let source = r#"
module Outer
  class Item
    attr_reader :label

    def initialize(label: "default")
      @label = label
    end
  end

  def self.build
    Item.new(label: "nested")
  end
end
"#;
    let classes =
        ingest_library_classes(source.as_bytes(), "nested_constructor.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let emitted_files = emit_library(&app)
        .into_iter()
        .filter(|file| {
            file.path
                .extension()
                .is_some_and(|extension| extension == "rb")
        })
        .collect::<Vec<_>>();
    let emitted = emitted_files
        .iter()
        .map(|file| file.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        emitted.contains("Item.new(\"nested\")"),
        "a lexically resolved nested class call should use the flattened initialize slot: {emitted}"
    );
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-nested-constructor-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for file in &emitted_files {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().expect("emitted file parent")).expect("mkdir");
        std::fs::write(path, &file.content).expect("write emitted file");
    }
    let requires = emitted_files
        .iter()
        .map(|file| format!("require_relative {:?}", file.path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");
    let output = Command::new("ruby")
        .arg("-e")
        .arg(format!(
            "{requires}\nraise 'nested constructor changed' unless Outer.build.label == 'nested'"
        ))
        .current_dir(&dir)
        .output()
        .expect("run emitted nested constructor");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "nested emitted constructor failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn model_new_calls_keep_the_active_record_attribute_hash_contract() {
    let files = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :projects do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "app/models/project.rb",
            "class Project < ApplicationRecord\n  def initialize(name: \"default\")\n    @name = name\n  end\n\n  def self.build\n    Project.new(name: \"source model\")\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest model");
    roundhouse::session::analyze_and_lower(&mut app);
    let source = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|file| file.path.to_string_lossy().ends_with("project.rb"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        source.contains("Project.new({ name: \"source model\" })"),
        "a model's Active Record constructor must keep its attributes Hash:\n{source}"
    );
}

#[test]
fn models_without_flattened_initializers_keep_keyword_hashes_under_unknown_lookup() {
    let model_source = "class LegacyProject < ApplicationRecord\n  include ExternalConstructorHooks\n\n  def self.build(attributes)\n    [LegacyProject.new(name: attributes[:name]), LegacyProject.new(**attributes)]\n  end\nend\n";
    let files = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :legacy_projects do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/models/legacy_project.rb", model_source),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest model");
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let call_start = model_source
        .find("new(name: attributes[:name])")
        .expect("keyword constructor") as u32;
    assert!(
        diagnostics.iter().all(|diagnostic| {
            diagnostic.span.start != call_start
                || !matches!(
                    &diagnostic.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )
        }),
        "a model without a flattened initialize slot must keep its Active Record attribute-hash contract: {diagnostics:#?}"
    );
    let emitted = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|file| file.path.to_string_lossy().ends_with("legacy_project.rb"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        emitted.contains("LegacyProject.new({ name: attributes[:name] })")
            && emitted.contains("LegacyProject.new(**attributes)"),
        "model keyword hashes and splats must remain intact when constructor lookup is external:\n{emitted}"
    );
}
