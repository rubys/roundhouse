use roundhouse::App;
use roundhouse::ingest::ingest_library_classes;

const SOURCE: &str = r#"class Presenter
  attr_reader :label, :scope, :now

  def initialize(label:, scope: nil, now: Time.now)
    @label = label
    @scope = scope
    @now = now
  end

  def summary
    @label
  end
end

class PresenterCaller
  def self.build
    Presenter.new(label: "first", scope: nil)
  end

  def self.replace_summary
    Presenter.define_method(:summary) { "replacement" }
  end
end
"#;

#[test]
fn redefining_an_unrelated_method_keeps_constructor_keywords_supported() {
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(SOURCE.as_bytes(), "presenter.rb").expect("ingest"));
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        !diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "{diagnostics:#?}"
    );
    let emitted = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !emitted.contains("constructor keyword arguments not supported"),
        "{emitted}"
    );
}

#[test]
fn named_mutations_only_refuse_constructor_methods_or_unknown_names() {
    for (mutation, refused) in [
        ("define_method(:summary) { 'changed' }", false),
        ("define_singleton_method('summary') { 'changed' }", false),
        ("remove_method(:summary)", false),
        ("undef_method(:summary)", false),
        ("define_method(:initialize) { |**options| }", true),
        (
            "define_singleton_method(:new) { |**options| options }",
            true,
        ),
        ("remove_method(:summary, :initialize)", true),
        ("undef_method(:initialize)", true),
        ("define_method(method_name) { 'changed' }", true),
        ("remove_method(method_name)", true),
        (
            "define_singleton_method(:method_added) { |name| name }",
            true,
        ),
        (
            "define_singleton_method(:inherited) { |child| child }",
            true,
        ),
        ("define_method(:method_added) { |name| name }", true),
    ] {
        let source = SOURCE.replace("define_method(:summary) { \"replacement\" }", mutation);
        let mut app = App::new();
        app.library_classes
            .extend(ingest_library_classes(source.as_bytes(), "mutation.rb").expect("ingest"));
        let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
        let actual = diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        });
        assert_eq!(actual, refused, "{mutation}: {diagnostics:#?}");
    }
}

#[test]
fn a_method_added_hook_still_makes_constructor_lookup_unknown() {
    let source = SOURCE.replace(
        "  def summary\n",
        "  def self.method_added(name)\n  end\n\n  def summary\n",
    );
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(source.as_bytes(), "hook.rb").expect("ingest"));
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "{diagnostics:#?}"
    );
}

fn constructor_keywords_are_refused(source: &str) -> bool {
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(source.as_bytes(), "hook.rb").expect("ingest"));
    roundhouse::session::analyze_and_lower(&mut app)
        .iter()
        .any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        })
}

#[test]
fn define_singleton_method_inherited_still_hides_subclass_keywords() {
    let source = r#"
class Parent
  define_singleton_method(:inherited) do |child|
    child.define_singleton_method(:new) { |**options| options }
  end
end

class Child < Parent
  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "define_singleton_method(:inherited) can replace Child.new"
    );
}

#[test]
fn included_callback_that_defines_method_added_still_hides_keywords() {
    let source = r#"
module Hooks
  def self.included(base)
    base.define_singleton_method(:method_added) { |name| name }
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "included that installs method_added can replace Child.new"
    );
}

#[test]
fn singleton_class_define_method_inherited_still_hides_subclass_keywords() {
    let source = r#"
class Parent
  class << self
    define_method(:inherited) do |child|
      child.define_singleton_method(:new) { |**options| options }
    end
  end
end

class Child < Parent
  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "class << self; define_method(:inherited) can replace Child.new"
    );
}

#[test]
fn included_callback_that_only_calls_a_macro_on_the_receiver_keeps_keywords() {
    for body in [
        "base.before_action(:load_tag) if base.methods.include?(:before_action)",
    ] {
        let source = format!(
            r#"
module Tagging
  def self.included(base)
    {body}
  end
end

class WidgetClient
  include Tagging

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#
        );
        assert!(!constructor_keywords_are_refused(&source), "{body}");
    }
}

#[test]
fn included_eval_block_that_replaces_new_still_refuses() {
    for body in [
        "base.instance_eval do\n      define_singleton_method(:new) { |**o| o }\n    end",
        "base.class_eval do\n      extend Unknown::Factory\n    end",
        "base.class_eval(\"def self.new(**o) = o\")",
        // The block's parameter is the receiver too (`klass` is `base`).
        "base.instance_eval do |klass|\n      klass.define_singleton_method(:new) { |**o| o }\n    end",
        "base.class_eval { |klass| klass.extend(Unknown::Factory) }",
        "base.instance_eval { it.define_singleton_method(:new) { |**o| o } }",
        "base.instance_eval { _1.define_singleton_method(:new) { |**o| o } }",
        // Any literal eval block on the receiver refuses, as on `main`: even a
        // bare macro call, a numbered parameter of a nested block, or `self`
        // through a local (CodeRabbit on #1525).
        "base.instance_eval do\n      before_action(:load_tag) if base.methods.include?(:before_action)\n    end",
        "base.instance_eval { [1].map { _1 } }",
        "base.instance_eval do\n      target = self\n      target.define_singleton_method(:new) { |**o| o }\n    end",
    ] {
        let source = format!(
            r#"
module Hooks
  def self.included(base)
    {body}
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{body}");
    }
}

/// A lookup change sent reflectively on the hook's receiver: `send`,
/// `public_send` and `__send__` each run the named method (CodeRabbit on
/// #1525), so keywords stay refused.
#[test]
fn included_callback_sending_a_lookup_change_still_refuses() {
    for body in [
        "base.send(:define_singleton_method, :new) { |**o| o }",
        "base.public_send(:define_singleton_method, :new) { |**o| o }",
        "base.__send__(:define_singleton_method, :new) { |**o| o }",
    ] {
        let source = format!(
            r#"
module Hooks
  def self.included(base)
    {body}
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{body}");
    }
}

/// A hook that calls, on its receiver, a method an extended module defines,
/// and that method replaces `new` (CodeRabbit on #1525): the call can't be
/// read as a harmless macro, so keywords stay refused.
#[test]
fn included_callback_calling_an_extended_constructor_installer_still_refuses() {
    for (hook, class_extends) in [
        ("base.extend(Factory)\n    base.install_factory", ""),
        ("base.install_factory", "  extend Factory\n"),
        ("base.extend(Factory)\n    base.send(:install_factory)", ""),
        ("base.extend(Factory)\n    base.public_send(\"install_factory\")", ""),
        ("base.extend(Factory)\n    base.__send__(:install_factory)", ""),
        ("base.extend(Factory)\n    base.send(installer)", ""),
        ("base.extend(Factory)\n    base.instance_eval { install_factory }", ""),
    ] {
        let source = format!(
            r#"
module Factory
  def install_factory
    define_singleton_method(:new) {{ |**options| options }}
  end
end

module Hooked
  def self.installer = :install_factory

  def self.included(base)
    {hook}
  end
end

class WidgetClient
{class_extends}  include Hooked

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{hook} / {class_extends:?}");
    }
}
