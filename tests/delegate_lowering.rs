//! ActiveSupport's `delegate :a, to: :b` becomes real methods
//! (`ingest::delegate`).
//!
//! Rails defines them with `class_eval` at load time, so nothing about
//! them reaches an emitted tree: the declaration lands in
//! `unknown_calls` and every call to a delegated name is a bare send no
//! class defines. Where the caller rescues, the failure is invisible —
//! campfire's `message_presentation` wraps its body in `rescue
//! Exception` and returns `""`, so a missing `fragment` drew every
//! message with an EMPTY body and no error anywhere.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(helper: &str) -> String {
    let app = ingest_app_from_tree(tree(helper)).expect("ingest");
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("filter.rb"))
        .map(|f| f.content.clone())
        .expect("filter.rb")
}

fn tree(helper: &str) -> HashMap<PathBuf, Vec<u8>> {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n".to_string(),
        ),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n".to_string()),
        ("app/helpers/filter.rb", helper.to_string()),
    ];
    files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
        .collect()
}

/// The plain form: the forwarder CALLS the target, which is what Rails'
/// generated body does and what makes it work when the target is an
/// `attr_reader` rather than a raw ivar.
#[test]
fn a_delegated_reader_becomes_a_forwarding_method() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  def initialize(content)
    @content = content
  end

  delegate :fragment, :to_plain_text, to: :content
end
"#,
    );
    assert!(out.contains("def fragment\n    content.fragment\n  end"), "got:\n{out}");
    assert!(
        out.contains("def to_plain_text\n    content.to_plain_text\n  end"),
        "got:\n{out}"
    );
    // The declaration is CONSUMED, not replayed beside its expansion.
    assert!(!out.contains("delegate"), "got:\n{out}");
}

/// `prefix: true` and `allow_nil: true` — campfire's `Current` spells
/// both, and the general pass has to answer them the same way. The
/// nil guard is written as a ternary so the body still ends in a
/// read; the ruby emitter renders it as the if/else it lowers to.
#[test]
fn prefix_and_allow_nil_are_honoured() {
    let out = emit(
        r#"class Filter
  attr_reader :request

  delegate :host, to: :request, prefix: true, allow_nil: true
end
"#,
    );
    assert!(
        out.contains(
            "def request_host\n    __delegate_target = request\n    if __delegate_target.nil?\n      nil\n    else\n      __delegate_target.host\n    end\n  end"
        ),
        "got:\n{out}"
    );
}

/// DECLINED: a delegated name this class calls with ARGUMENTS. Rails
/// forwards them with `*args, &block`, which the strict targets do not
/// lower, and a zero-arg forwarder for a method that takes two is an
/// arity error standing in for a NameError. campfire's
/// `Messages::AttachmentPresentation` is exactly this shape.
#[test]
fn a_delegation_called_with_arguments_is_left_alone() {
    let out = emit(
        r#"class Filter
  attr_reader :context

  delegate :link_to, to: :context

  def render
    link_to "text", "/path"
  end
end
"#,
    );
    assert!(!out.contains("def link_to"), "got:\n{out}");
}

/// DECLINED: an option this pass does not reproduce. Half-expanding a
/// declaration is worse than leaving it visible.
#[test]
fn an_unreproducible_option_is_left_alone() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  delegate :fragment, to: :content, prefix: :body
end
"#,
    );
    assert!(!out.contains("def fragment"), "got:\n{out}");
    assert!(!out.contains("def body_fragment"), "got:\n{out}");
}

/// A method the class writes itself WINS — the expansion fills gaps, it
/// does not overwrite.
#[test]
fn a_hand_written_method_is_not_replaced() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  delegate :fragment, to: :content

  def fragment
    "already mine"
  end
end
"#,
    );
    assert!(out.contains("\"already mine\""), "got:\n{out}");
    assert_eq!(out.matches("def fragment").count(), 1, "got:\n{out}");
}

/// `to: :class` (or any other Ruby keyword) is `self.class` in the
/// forwarder, as Rails writes it: a bare `class.label` does not parse.
/// Shopify core has dozens (`delegate :context, to: :class`).
#[test]
fn a_keyword_target_is_read_through_self() {
    let out = emit(
        r#"class Filter
  attr_reader :return
  delegate :label, to: :class
  delegate :id, to: :return

  def self.label
    "f"
  end
end
"#,
    );
    assert!(out.contains("self.class.label"), "got:\n{out}");
    assert!(out.contains("self.return.id"), "got:\n{out}");
}

#[test]
fn a_delegated_writer_forwards_its_value() {
    // billing's `delegate :request_id, :request_id=, to: :class`: the
    // writer needs a parameter, and a zero-arg `def request_id=` is not
    // Ruby at all — the snippet failed to parse and took the reader
    // down with it.
    let out = emit(
        r#"class Filter
  delegate :request_id, :request_id=, to: :class

  def self.request_id
    @request_id
  end
end
"#,
    );
    assert!(out.contains("self.class.request_id"), "got:\n{out}");
    assert!(out.contains("self.class.request_id = value"), "got:\n{out}");
}

#[test]
fn delegated_operators_forward_their_operands() {
    // core's collection wrappers: `delegate :each, :[], :[]=, :<<, :==,
    // to: :@set`. An operator is not a reader — `[]=` and `==` end in
    // `=` without being writers — and each has a fixed arity, so it
    // forwards exactly.
    let out = emit(
        r#"class Filter
  delegate :[], :[]=, :<<, :==, to: :@set

  def initialize
    @set = {}
  end
end
"#,
    );
    assert!(out.contains("@set[key] = value"), "got:\n{out}");
    assert!(out.contains("@set[key]"), "got:\n{out}");
    assert!(out.contains("@set << other"), "got:\n{out}");
    assert!(out.contains("@set == other"), "got:\n{out}");
}

fn run_emitted(source: &str, exercise: &str) -> String {
    let emitted = emit(source);
    let script = format!("{emitted}\n{exercise}");
    let output = std::process::Command::new("ruby")
        .args(["-e", &script]).output().expect("ruby");
    assert!(output.status.success(), "{}\n{script}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn writer_arguments_do_not_shadow_the_target_method() {
    let source = r#"class Filter
  attr_reader :value
  def initialize(value)
    @value = value
  end
  delegate :title=, to: :value
end
"#;
    assert_eq!(run_emitted(source,
        "target = Struct.new(:title).new; Filter.new(target).title = 'changed'; puts target.title"),
        "changed\n");
}

#[test]
fn nil_operators_return_nil_without_evaluating_the_target_twice() {
    let source = r#"class Filter
  attr_reader :calls
  def initialize(value)
    @value = value
    @calls = 0
  end
  def key
    @calls += 1
    @value
  end
  delegate :[], :[]=, :<<, to: :key, allow_nil: true
end
"#;
    assert_eq!(run_emitted(source,
        "f = Filter.new(nil); p [f[0], f.[]=(0, 'x'), f << 'x', f.calls]"),
        "[nil, nil, nil, 3]\n");
    assert_eq!(run_emitted(source,
        "f = Filter.new(['old']); p [f[0], f.[]=(0, 'new'), f << 'tail', f.calls]"),
        "[\"old\", \"new\", [\"new\", \"tail\"], 3]\n");
}

#[test]
fn nil_comparison_operators_keep_their_boolean_result() {
    let source = r#"class Filter
  def initialize
    @target = nil
  end
  delegate :==, :!=, to: :@target, allow_nil: true
end
"#;
    assert_eq!(run_emitted(source, "f = Filter.new; p [f == nil, f != nil, f == 1]"),
        "[true, false, false]\n");
}
