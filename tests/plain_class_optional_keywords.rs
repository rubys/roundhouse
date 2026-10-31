//! A plain library class's optional keyword parameters, called with
//! keywords (#800).
//!
//! Ingest flattens an all-optional keyword group to positionals with
//! defaults (`def field(name, type = nil, null = true, description =
//! nil)`), marking each slot `from_keyword`, and `lower::helper_kwargs`
//! moves each call's keywords into those slots. That rewrite resolved
//! the callee only from a receiver typed as a class. A receiver typed as
//! an instance of the class it is written in (`b = new` in a class
//! method), `self.`, and a receiverless call from the class's own
//! methods all fell through, and Ruby bound the whole keyword Hash to
//! the first flattened slot with no diagnostic.
//!
//! Flattened-source and refusal assertions exercise target-neutral lowering.
//! Ruby-family target-aware emission preserves the native keyword contract.
//!
//! Native Ruby on the same source is the oracle. A call whose keywords
//! cannot be moved into slot order (a `**` splat, a key that is not a
//! literal symbol, a skipped slot whose default reads the callee's
//! context, a `send`) is refused with a named gap instead.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const BUILDER: &str = r##"module Kwlab
  class Builder
    attr_reader :rows

    def initialize
      @rows = []
    end

    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def add(name, null: true)
      @rows << field(name, :string, null: null)
      @rows << self.field(name, description: "self")
      self
    end

    def self.column(name, limit: 8, comment: "")
      [name, limit, comment]
    end

    def self.report
      b = new
      [b.field(:a, :string, null: false), b.field(:b, :string, description: "kept"), b.field(:c),
       b.add(:d, null: false).rows, column(:e, comment: "c"), self.column(:f, limit: 2)]
    end
  end
end
"##;

const GADGET: &str = r##"module Kwlab
  class Gadget < Builder
    def self.report
      g = new
      [g.field(:g, null: false), column(:h, comment: "inherited"), Kwlab::Builder.new.field(:i, description: "typed")]
    end
  end
end
"##;

const EXPECTED: &str = "[[:a, :string, false, nil], [:b, :string, true, \"kept\"], [:c, nil, true, nil], \
[[:d, :string, false, nil], [:d, nil, true, \"self\"]], [:e, 8, \"c\"], [:f, 2, \"\"]]\n\
[[:g, nil, false, nil], [:h, 8, \"inherited\"], [:i, nil, true, \"typed\"]]\n";

const REPORT: &str = "p Kwlab::Builder.report\np Kwlab::Gadget.report\n";

fn native(files: &[(&str, &str)], script: &str) -> String {
    // One directory per call: tests run in parallel, and two with the
    // same number of files shared one.
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("roundhouse-kwlab-{}-{call}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut requires = String::new();
    for (name, source) in files {
        let path = dir.join(name);
        std::fs::write(&path, source).unwrap();
        requires.push_str(&format!("require {:?}\n", path.display().to_string()));
    }
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("{requires}{script}"))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn optional_keywords_bind_by_name_through_every_receiver_shape() {
    assert_eq!(
        native(&[("builder.rb", BUILDER), ("gadget.rb", GADGET)], REPORT),
        EXPECTED,
        "native control"
    );
    let run = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", BUILDER)
        .write("app/lib/kwlab/gadget.rb", GADGET)
        .run_ruby(&format!(
            "require_relative \"app/models/kwlab/builder\"\nrequire_relative \"app/models/kwlab/gadget\"\n{REPORT}"
        ));
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED, "emitted vs native");
}

/// The issue's reproducer, verbatim: the emitted `def` keeps its
/// flattened shape, and every call site moves its keywords into it.
#[test]
fn the_issue_reproducer_matches_ruby() {
    const SOURCE: &str = r##"module Kwlab
  class Builder
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def self.report
      b = new
      [b.field(:a, :string, null: false), b.field(:b, :string, description: "kept"), b.field(:c)]
    end
  end
end
"##;
    let expected = "[[:a, :string, false, nil], [:b, :string, true, \"kept\"], [:c, nil, true, nil]]\n";
    assert_eq!(native(&[("builder.rb", SOURCE)], "p Kwlab::Builder.report\n"), expected);
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", SOURCE)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/kwlab/builder.rb")).unwrap();
    assert!(
        source.contains("b.field(:a, :string, false)")
            && source.contains("b.field(:b, :string, true, \"kept\")"),
        "call sites not moved into slot order:\n{source}"
    );
    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require {main:?}\nMain.configure_default_adapter!\nrequire_relative \"app/models/kwlab/builder\"\np Kwlab::Builder.report\n"
        ))
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected, "emitted vs native:\n{source}");
}

/// Emit `report`'s body against the builder and return the errors.
fn refusal_errors(report: &str) -> Vec<String> {
    let source = format!(
        r##"module Kwlab
  class Builder
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def self.stamp
      Time.now.to_i
    end

    def mark(name, at: Builder.stamp, note: "")
      [name, at, note]
    end

    def self.report(opts, key)
      b = new
      {report}
    end
  end
end
"##
    );
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", &source)
        .emit_target_neutral(BuildTarget::Ruby);
    errors
}

fn assert_refused(report: &str) {
    let errors = refusal_errors(report);
    assert!(
        errors.iter().any(|e| e.contains("flattened keyword arguments not supported")),
        "`{report}` was not refused; errors: {errors:#?}"
    );
}

#[test]
fn a_keyword_splat_into_flattened_keywords_is_refused() {
    assert_refused("b.field(:a, **opts)");
    assert_refused("b.field(:a, null: false, **opts)");
}

#[test]
fn a_dynamic_keyword_key_is_refused() {
    assert_refused("b.field(:a, key => false)");
    assert_refused("b.field(:a, \"null\" => false)");
}

#[test]
fn a_send_with_keywords_to_a_flattened_method_is_refused() {
    assert_refused("b.send(:field, :a, null: false)");
    assert_refused("b.public_send(:field, :a, description: \"x\")");
}

/// A skipped slot is filled with the callee's default only when that
/// default means the same thing at the call site; the context-free
/// counterparts of each refused call still move and stay quiet.
#[test]
fn a_skipped_slot_whose_default_reads_the_callee_is_refused() {
    assert_refused("b.mark(:a, note: \"late\")");
    let errors = refusal_errors("[b.field(:a, null: false), b.mark(:a, at: 1), b.mark(:b), b.field(:c, opts)]");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// The same refusals through a receiver typed as the class, from
/// another class.
#[test]
fn a_typed_receiver_keyword_splat_is_refused() {
    const CALLER: &str = r##"module Kwlab
  class Caller
    def self.run(opts)
      Kwlab::Builder.new.field(:a, **opts)
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/lib/kwlab/builder.rb",
            "module Kwlab\n  class Builder\n    def field(name, type = nil, null: true, description: nil)\n      [name, type, null, description]\n    end\n  end\nend\n",
        )
        .write("app/lib/kwlab/caller.rb", CALLER)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("flattened keyword arguments not supported")),
        "not refused; errors: {errors:#?}"
    );
}

/// `self.field(…)` in the base reaches a subclass's `field` when the
/// receiver is a subclass instance. With different keyword slots there,
/// no one positional spelling binds for both.
#[test]
fn a_subclass_redefinition_with_other_slots_is_refused() {
    const BASE: &str = r##"module Kwlab
  class Builder
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def add(name)
      field(name, description: "base")
    end
  end
end
"##;
    const SUB: &str = r##"module Kwlab
  class Gadget < Builder
    def field(name, description: nil, null: true)
      [name, description, null]
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", BASE)
        .write("app/lib/kwlab/gadget.rb", SUB)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("a subclass or includer redefines this method")),
        "not refused; errors: {errors:#?}"
    );
}

/// The same through an included module: a call in the module's body
/// reaches the includer's own `field` first, so a different slot
/// layout there is refused too.
#[test]
fn an_includer_redefinition_with_other_slots_is_refused() {
    const FIELDS: &str = r##"module Kwlab
  module Fields
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def add(name)
      field(name, description: "module")
    end
  end
end
"##;
    const GADGET: &str = r##"module Kwlab
  class Gadget
    include Fields

    def field(name, description: nil, null: true)
      [name, description, null]
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/fields.rb", FIELDS)
        .write("app/lib/kwlab/gadget.rb", GADGET)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("a subclass or includer redefines this method")),
        "not refused; errors: {errors:#?}"
    );
}

/// Ruby resolves `field` through each included module's own includes:
/// `Gadget.ancestors` is `[Gadget, Inner, Patch, Fields]`, so `self.field`
/// in `Fields#add` on a `Gadget` reaches `Patch#field`, which `Inner`
/// brings in without defining it. Its slots differ, so the call is
/// refused rather than moved into `Fields#field`'s slots.
#[test]
fn a_nested_include_redefinition_with_other_slots_is_refused() {
    const FIELDS: &str = r##"module Kwlab
  module Fields
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def add(name)
      field(name, description: "module")
    end
  end
end
"##;
    const PATCH: &str = r##"module Kwlab
  module Patch
    def field(name, description: nil, null: true)
      [name, description, null]
    end
  end
end
"##;
    const INNER: &str =
        "module Kwlab\n  module Inner\n    include Patch\n\n    def marker\n      :inner\n    end\n  end\nend\n";
    const GADGET: &str = "module Kwlab\n  class Gadget\n    include Fields\n    include Inner\n  end\nend\n";
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/fields.rb", FIELDS)
        .write("app/lib/kwlab/patch.rb", PATCH)
        .write("app/lib/kwlab/inner.rb", INNER)
        .write("app/lib/kwlab/gadget.rb", GADGET)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("a subclass or includer redefines this method")),
        "not refused; errors: {errors:#?}"
    );
}

/// A module a class and its superclass both include is one ancestor in
/// Ruby (`Child.ancestors == [Child, Base, Helper]`, Ruby 4.0.7), so a
/// keyword call in the module's body reaches its own definition on every
/// receiver. The index listed the module twice and refused every such
/// call as ambiguous (a large real app's JSON-LD helper, included by a
/// service and by its subclass).
#[test]
fn a_module_included_by_a_class_and_its_superclass_binds_its_keywords() {
    const HELPER: &str = r##"module Kwlab
  module Fetching
    def fetch(uri, on_behalf_of = nil, raise_on_error: :none, options: {})
      [uri, on_behalf_of, raise_on_error, options]
    end

    def outer(uri)
      fetch(uri, nil, raise_on_error: :temporary)
    end

    def outer2(uri)
      fetch(uri, :me, options: { a: 1 })
    end
  end
end
"##;
    const BASE: &str = "module Kwlab\n  class Base\n    include Fetching\n  end\nend\n";
    const CHILD: &str = "module Kwlab\n  class Child < Base\n    include Fetching\n  end\nend\n";
    const SCRIPT: &str = "p Kwlab::Child.new.outer(\"u\")\np Kwlab::Child.new.outer2(\"u\")\np Kwlab::Base.new.outer(\"u\")\n";
    let expected = native(&[("fetching.rb", HELPER), ("base.rb", BASE), ("child.rb", CHILD)], SCRIPT);
    assert_eq!(expected, "[\"u\", nil, :temporary, {}]\n[\"u\", :me, :none, {a: 1}]\n[\"u\", nil, :temporary, {}]\n", "native control");
    let run = emit_and_run::real_blog()
        .write("app/lib/kwlab/fetching.rb", HELPER)
        .write("app/lib/kwlab/base.rb", BASE)
        .write("app/lib/kwlab/child.rb", CHILD)
        .run_ruby(&format!(
            "require_relative \"app/models/kwlab/fetching\"\nrequire_relative \"app/models/kwlab/base\"\nrequire_relative \"app/models/kwlab/child\"\n{SCRIPT}"
        ));
    run.assert_passes();
    assert_eq!(run.stdout, expected, "emitted vs native");
}

/// Which copy of a twice-included module Ruby keeps decides the binding
/// when another module defining the name sits between the copies:
/// `class A; include M; end; class B < A; include X; include M; end` has
/// `B.ancestors == [B, X, A, M]` and reaches X's, but the copy in B would
/// reach M's. That stays refused.
#[test]
fn a_definer_between_two_copies_of_a_module_is_refused() {
    const M: &str = r##"module Kwlab
  module Fields
    def field(name, type = nil, null: true)
      [name, type, null]
    end

    def add(name)
      field(name, null: false)
    end
  end
end
"##;
    const X: &str = r##"module Kwlab
  module Loud
    def field(name, null: true, type: nil)
      [:loud, name, null, type]
    end
  end
end
"##;
    const A: &str = "module Kwlab\n  class Plain\n    include Fields\n  end\nend\n";
    const B: &str = "module Kwlab\n  class Gadget < Plain\n    include Loud\n    include Fields\n  end\nend\n";
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/fields.rb", M)
        .write("app/lib/kwlab/loud.rb", X)
        .write("app/lib/kwlab/plain.rb", A)
        .write("app/lib/kwlab/gadget.rb", B)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("flattened keyword arguments")), "not refused; errors: {errors:#?}");
}

/// A prepended module sits ahead of the class in Ruby's lookup, so a
/// call in the class's body reaches the module's `field`, whose slots
/// differ. The index does not see `prepend`, so the call is refused
/// rather than respelled for the class's own definition.
#[test]
fn a_call_through_a_prepended_module_is_refused() {
    const LOUD: &str = r##"module Kwlab
  module Loud
    def field(name, null: true, type: nil)
      [:loud, name, null, type]
    end
  end
end
"##;
    const GADGET: &str = r##"module Kwlab
  class Gadget
    prepend Loud

    def field(name, type = nil, null: true)
      [name, type, null]
    end

    def add(name)
      field(name, null: false)
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/loud.rb", LOUD)
        .write("app/lib/kwlab/gadget.rb", GADGET)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("flattened keyword arguments")), "not refused; errors: {errors:#?}");
}

const LOUD: &str = r##"module Kwlab
  module Loud
    def field(name, null: true, type: nil)
      [:loud, name, null, type]
    end
  end
end
"##;
const PREPENDING_BASE: &str = r##"module Kwlab
  class Plain
    prepend Loud

    def field(name, type = nil, null: true)
      [:base, name, type, null]
    end
  end
end
"##;

/// A prepended module sits right ahead of the class that prepends it
/// (Ruby 4.0.7: `Base` prepending `Loud` gives `[Child, Loud, Base]`), so a
/// subclass's own definition is found first and the call binds to it.
#[test]
fn a_prepend_above_the_first_definer_does_not_refuse() {
    const CHILD: &str = r##"module Kwlab
  class Gadget < Plain
    def field(name, type = nil, null: true)
      [:child, name, type, null]
    end

    def add(name)
      field(name, null: false)
    end
  end
end
"##;
    const SCRIPT: &str = "p Kwlab::Gadget.new.add(:a)\n";
    let expected = native(&[("loud.rb", LOUD), ("plain.rb", PREPENDING_BASE), ("gadget.rb", CHILD)], SCRIPT);
    assert_eq!(expected, "[:child, :a, nil, false]\n", "native control");
    let run = emit_and_run::real_blog()
        .write("app/lib/kwlab/loud.rb", LOUD)
        .write("app/lib/kwlab/plain.rb", PREPENDING_BASE)
        .write("app/lib/kwlab/gadget.rb", CHILD)
        .run_ruby(&format!(
            "require_relative \"app/models/kwlab/loud\"\nrequire_relative \"app/models/kwlab/plain\"\nrequire_relative \"app/models/kwlab/gadget\"\n{SCRIPT}"
        ));
    run.assert_passes();
    assert_eq!(run.stdout, expected, "emitted vs native");
}

/// Without a definition of its own, the subclass's call reaches the
/// prepended module ahead of `Base`'s: still refused.
#[test]
fn a_prepend_at_or_before_the_first_definer_is_refused() {
    const CHILD: &str = r##"module Kwlab
  class Gadget < Plain
    def add(name)
      field(name, null: false)
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/loud.rb", LOUD)
        .write("app/lib/kwlab/plain.rb", PREPENDING_BASE)
        .write("app/lib/kwlab/gadget.rb", CHILD)
        .emit_target_neutral(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("flattened keyword arguments")), "not refused; errors: {errors:#?}");
}
