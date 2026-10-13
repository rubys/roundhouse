//! A plain library class's optional keyword parameters, called with
//! keywords (#800).
//!
//! Ordinary application methods keep the source keyword contract
//! (`ingest::library_class::ingest_library_method` preserves keywords).
//! Native Ruby on the same source is the oracle: keywords bind by name
//! in any order, a skipped slot takes the callee's own default, and an
//! unknown key or a missing method raises. A quiet diagnostic list is
//! not that claim — each supported call is executed against the emitted
//! Ruby and compared with the native control.
//!
//! One shape stays refused. `b = new` inside a class method types the
//! receiver as an untyped instance, so `b.field(:a, **opts)` cannot be
//! verified against the destination's native keyword ABI. The packet is
//! not admitted.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::sync::atomic::{AtomicU64, Ordering};

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

const REPORT: &str = "p Kwlab::Builder.report\np Kwlab::Gadget.report\n";

fn native(files: &[(&str, &str)], script: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-kwlab-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut requires = String::new();
    for (name, source) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, source).unwrap();
        requires.push_str(&format!("require {:?}\n", path.display().to_string()));
    }
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("{requires}{script}"))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "native Ruby failed\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Emit the overlay and run `script` through the booted app. `main.rb`
/// loads every emitted model, so the script names classes, not files.
fn emitted(files: &[(&str, &str)], script: &str) -> emit_and_run::Run {
    let mut overlay = emit_and_run::real_blog();
    for (name, source) in files {
        overlay = overlay.write(&format!("app/lib/{name}"), source);
    }
    overlay.run_ruby(script)
}

/// The supported claim: native Ruby and the emitted program produce the
/// same stdout, and analysis/emit report no errors.
fn assert_matches(files: &[(&str, &str)], script: &str) {
    let expected = native(files, script);
    let run = emitted(files, script);
    run.assert_passes();
    assert_eq!(run.stdout, expected, "emitted vs native");
}

#[test]
fn optional_keywords_bind_by_name_through_every_receiver_shape() {
    assert_matches(&[("kwlab/builder.rb", BUILDER), ("kwlab/gadget.rb", GADGET)], REPORT);
}

/// The issue's reproducer: `b = new` still binds each keyword by name,
/// including a skipped slot, without rewriting the call into positional
/// slot order.
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
    let script = "p Kwlab::Builder.report\n";
    let expected = "[[:a, :string, false, nil], [:b, :string, true, \"kept\"], [:c, nil, true, nil]]\n";
    assert_eq!(native(&[("kwlab/builder.rb", SOURCE)], script), expected, "native control");
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/builder.rb", SOURCE)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/kwlab/builder.rb")).unwrap();
    assert!(
        source.contains("def field(name, type = nil, null: true, description: nil)")
            && source.contains("b.field(:a, :string, null: false)")
            && source.contains("b.field(:b, :string, description: \"kept\")")
            && !source.contains("b.field(:a, :string, false)"),
        "optional keywords were flattened instead of preserved:\n{source}"
    );
    assert_matches(&[("kwlab/builder.rb", SOURCE)], script);
}

/// A key computed at the call, and `send` / `public_send`, stay Ruby's
/// dispatch: a Symbol key binds, a String key is unknown, and a missing
/// method raises `NoMethodError`.
#[test]
fn dynamic_keyword_keys_and_send_match_ruby() {
    const SOURCE: &str = r##"module Kwlab
  class Dyn
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def self.go(key, method_name)
      b = new
      bound = begin
        [:ok, b.field(:a, key => false)]
      rescue ArgumentError => e
        [:arg, e.class.name, e.message]
      end
      sent = begin
        [:ok, b.send(method_name, :a, null: false), b.public_send(:field, :a, description: "x")]
      rescue NoMethodError => e
        [:nomethod, e.class.name, e.message]
      end
      [bound, sent]
    end
  end
end
"##;
    assert_matches(
        &[("kwlab/dyn.rb", SOURCE)],
        "p Kwlab::Dyn.go(:null, :field)\n\
         p Kwlab::Dyn.go(:description, \"field\")\n\
         p Kwlab::Dyn.go(:nope, :missing)\n\
         p Kwlab::Dyn.go(\"null\", :field)\n",
    );
}

/// A skipped slot evaluates its default in the callee, not at the call.
/// `SubMarker.stamp` is 99, but `mark`'s default still calls `Marker.stamp`.
#[test]
fn a_skipped_slot_evaluates_the_callee_default() {
    const SOURCE: &str = r##"module Kwlab
  class Marker
    def self.stamp
      7
    end

    def mark(name, at: Marker.stamp, note: "")
      [name, at, note]
    end

    def self.report
      b = new
      [b.mark(:a, note: "late"), b.mark(:a, at: 1), b.mark(:b)]
    end
  end

  class SubMarker < Marker
    def self.stamp
      99
    end
  end
end
"##;
    assert_matches(
        &[("kwlab/marker.rb", SOURCE)],
        "p Kwlab::Marker.report\np Kwlab::SubMarker.new.mark(:z, note: \"sub\")\n",
    );
}

/// A typed constructor receiver can take a keyword splat. The callee
/// owns unknown keys; an explicit keyword and the packet combine in
/// source order, the later one winning.
#[test]
fn a_typed_receiver_keyword_splat_matches_ruby() {
    const BUILDER: &str = r##"module Kwlab
  class Builder
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end
  end
end
"##;
    const CALLER: &str = r##"module Kwlab
  class Caller
    def self.run(opts)
      begin
        [:ok, Kwlab::Builder.new.field(:a, **opts)]
      rescue ArgumentError => e
        [:arg, e.class.name, e.message]
      end
    end

    def self.overlay(opts)
      begin
        [:ok, Kwlab::Builder.new.field(:a, null: true, **opts)]
      rescue ArgumentError => e
        [:arg, e.class.name, e.message]
      end
    end

    def self.after(opts)
      begin
        [:ok, Kwlab::Builder.new.field(:a, **opts, description: "after")]
      rescue ArgumentError => e
        [:arg, e.class.name, e.message]
      end
    end
  end
end
"##;
    assert_matches(
        &[("kwlab/builder.rb", BUILDER), ("kwlab/caller.rb", CALLER)],
        "p Kwlab::Caller.run({null: false, description: \"d\"})\n\
         p Kwlab::Caller.run({description: \"only\"})\n\
         p Kwlab::Caller.run({})\n\
         p Kwlab::Caller.run({null: false, extra: 1})\n\
         p Kwlab::Caller.run({\"null\" => false})\n\
         p Kwlab::Caller.run({nope: 1})\n\
         p Kwlab::Caller.overlay({null: false, description: \"s\"})\n\
         p Kwlab::Caller.after({null: false})\n",
    );
}

/// `field` in the base reaches the subclass override. The two keyword
/// orders are different, and each receiver binds the call it actually
/// reaches.
#[test]
fn a_subclass_redefinition_binds_the_method_it_reaches() {
    const BASE: &str = r##"module Kwlab
  class Builder
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def add(name)
      field(name, description: "base")
    end

    def self.direct
      new.field(:g, null: false, description: "x")
    end
  end
end
"##;
    const SUB: &str = r##"module Kwlab
  class Gadget < Builder
    def field(name, description: nil, null: true)
      [name, description, null]
    end

    def self.direct
      new.field(:g, null: false, description: "x")
    end
  end
end
"##;
    assert_matches(
        &[("kwlab/builder.rb", BASE), ("kwlab/gadget.rb", SUB)],
        "p Kwlab::Gadget.new.add(:g)\n\
         p Kwlab::Gadget.direct\n\
         p Kwlab::Builder.new.add(:g)\n\
         p Kwlab::Builder.direct\n",
    );
}

/// A call written in an included module reaches the includer's own
/// `field` first. Direct and inherited calls keep that method's slots.
#[test]
fn an_includer_redefinition_binds_the_method_it_reaches() {
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
  class Widget
    include Fields

    def field(name, description: nil, null: true)
      [name, description, null]
    end
  end
end
"##;
    assert_matches(
        &[("kwlab/fields.rb", FIELDS), ("kwlab/widget.rb", GADGET)],
        "p Kwlab::Widget.new.add(:w)\np Kwlab::Widget.new.field(:w, null: false, description: \"direct\")\n",
    );
}

/// Ruby resolves `field` through each included module's own includes.
/// The app ancestor prefix is `[Gadget, Inner, Patch, Fields]`; runtime
/// mixins may follow it. `Fields#add` on a `Gadget` therefore reaches
/// `Patch#field`, whose keyword order differs from `Fields#field`.
#[test]
fn a_nested_include_redefinition_binds_the_method_it_reaches() {
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
    const INNER: &str = "module Kwlab\n  module Inner\n    include Patch\n\n    def marker\n      :inner\n    end\n  end\nend\n";
    const GADGET: &str = "module Kwlab\n  class Gadget\n    include Fields\n    include Inner\n  end\nend\n";
    let files = [
        ("kwlab/fields.rb", FIELDS),
        ("kwlab/patch.rb", PATCH),
        ("kwlab/inner.rb", INNER),
        ("kwlab/gadget.rb", GADGET),
    ];
    let script = "names = Kwlab::Gadget.ancestors.map { |a| a.name }\n\
                  raise names.inspect unless names.take(4) == \
                  [\"Kwlab::Gadget\", \"Kwlab::Inner\", \"Kwlab::Patch\", \"Kwlab::Fields\"]\n\
                  p Kwlab::Gadget.new.add(:n)\n\
                  p Kwlab::Gadget.new.field(:n, null: false, description: \"p\")\n\
                  p Kwlab::Gadget.new.marker\n";
    assert_matches(&files, script);
}

/// `b = new` does not prove which class runs the method, so a keyword
/// splat into that receiver's preserved keyword parameters stays refused.
/// A typed `Kwlab::Host.new` splat of the same method is a different
/// call and is not this refusal.
#[test]
fn an_untyped_receiver_keyword_splat_is_refused() {
    const SOURCE: &str = r##"module Kwlab
  class Host
    def field(name, type = nil, null: true, description: nil)
      [name, type, null, description]
    end

    def self.report(opts)
      b = new
      b.field(:a, **opts)
    end

    def self.overlay(opts)
      b = new
      b.field(:a, null: false, **opts)
    end
  end
end
"##;
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kwlab/host.rb", SOURCE)
        .emit(BuildTarget::Ruby);
    let refusals: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.contains("keyword splat into source keyword parameters not supported")
                && e.contains("keyword destination's native argument ABI cannot be verified")
        })
        .collect();
    assert!(
        refusals.len() >= 2,
        "both untyped `b.field(..., **opts)` calls must be refused; errors: {errors:#?}"
    );
    assert!(
        errors.iter().all(|e| {
            e.contains("keyword splat into source keyword parameters not supported")
                && e.contains("keyword destination's native argument ABI cannot be verified")
        }),
        "unexpected errors beside the unverified keyword ABI: {errors:#?}"
    );
}
