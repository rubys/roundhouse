//! #1229: a zero-argument `def` called without parens is a call in the
//! TypeScript emit, not a property read, wherever the emit knows the
//! method: on a plain class (instance or class side), from a subclass's
//! class method, and from a model's class method. Before, only a
//! model's instance methods were known, so `label.upcase` read the
//! function (`this.label.toUpperCase()`). The run itself is
//! `typescript_toolchain::real_blog_parenless_calls_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/parenless_calls.rs"]
mod parenless_calls;

use roundhouse::project::BuildTarget;

#[test]
fn a_zero_argument_def_without_parens_is_called() {
    let (emitted, errors) = parenless_calls::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let meter = read("app/models/meter.ts");
    let widget = read("app/models/widget.ts");
    let comment = read("app/models/comment.ts");
    for call in ["return this.label().toUpperCase();", "return this.label();", "return this.name().toUpperCase();"] {
        assert!(meter.contains(call), "{call}:\n{meter}");
    }
    assert!(!meter.contains("this.label.") && !meter.contains("this.label;"), "{meter}");
    assert!(widget.contains("return this.label().toUpperCase();"), "{widget}");
    assert!(comment.contains("[this.tag(), this.tag()]"), "{comment}");
}

/// A class method the emit declares as a `static` field (`def self.mode;
/// @mode ||= "x"; end` is `static mode`) is read, not called: `this.mode()`
/// on a field was TS2349 and a TypeError at run time.
#[test]
fn a_class_side_field_is_read_not_called() {
    let (emitted, errors) = parenless_calls::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let dial = std::fs::read_to_string(emitted.join("app/models/dial.ts")).expect("dial.ts");
    assert!(dial.contains("static mode"), "{dial}");
    assert!(dial.contains("return this.mode.toUpperCase();"), "{dial}");
    assert!(!dial.contains("this.mode()"), "{dial}");
}

/// An instance `attr_reader :label` no longer hides `def self.label`:
/// the class method is emitted, and a class-side `label` calls it.
#[test]
fn an_instance_field_leaves_a_class_method_of_its_name() {
    let (emitted, errors) = parenless_calls::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let knob = std::fs::read_to_string(emitted.join("app/models/knob.ts")).expect("knob.ts");
    assert!(knob.contains("  label: string;"), "{knob}");
    assert!(knob.contains("static label(): string {"), "{knob}");
    assert!(knob.contains("return this.label().toUpperCase();"), "{knob}");
}

/// The predicate/bang twin check is per side too: an instance `is_ready`
/// no longer drops `def self.ready?`, which sanitizes to the same name.
#[test]
fn an_instance_method_leaves_a_class_predicate_of_its_sanitized_name() {
    let (emitted, errors) = parenless_calls::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let lamp = std::fs::read_to_string(emitted.join("app/models/lamp.ts")).expect("lamp.ts");
    assert!(lamp.contains("  is_ready(): boolean {"), "{lamp}");
    assert!(lamp.contains("static is_ready(): boolean {"), "{lamp}");
}


/// A method whose every parameter is optional (`def self.label(prefix =
/// "x")`, `def tag(sep = "-")`) takes a call with no arguments, so a
/// parenless send is a call there too: `this.label` read the function.
#[test]
fn a_def_with_only_optional_parameters_is_called() {
    let (emitted, errors) = parenless_calls::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let gauge = std::fs::read_to_string(emitted.join("app/models/gauge.ts")).expect("gauge.ts");
    assert!(gauge.contains("return this.label();"), "{gauge}");
    assert!(gauge.contains("return this.tag();"), "{gauge}");
}
