//! Two load-time names the emitted app must resolve the way Ruby's
//! lexical scope does:
//!
//! * `class AlphaHandler < Base` inside `module Outer; module Inner`, and
//!   `class AlphaReader < Base` inside `module Outer::Inner`, name
//!   `Outer::Inner::Base`. The emitted file must require that sibling
//!   before the class opens; the aggregator loads files in name order,
//!   and `alpha_*` sorts before `base`.
//! * A constant assigned inside `class << self` is read by bare name
//!   from the block's own methods. Ingest hoists it to the enclosing
//!   module, so the qualified reference must name that module, not
//!   Rubydex's singleton segment (`Gate::<Gate>::ERR` is not Ruby).
//!
//! Native Ruby is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const BASE: &str = r#"module Outer
  module Inner
    class Base
      def kind
        "base"
      end
    end
  end
end
"#;

const ALPHA_HANDLER: &str = r#"module Outer
  module Inner
    class AlphaHandler < Base
      def kind
        "handler:" + super
      end
    end
  end
end
"#;

const ALPHA_READER: &str = r#"module Outer::Inner
  class AlphaReader < Base
    def kind
      "reader:" + super
    end
  end
end
"#;

const GATE: &str = r#"module Outer
  module Gate
    class << self
      def check(option)
        raise ArgumentError, ERR unless [:ignore, :raise].include?(option)
        option
      end

      ERR = "bad option"
    end
  end
end
"#;

const REPORT: &str = r#"puts Outer::Inner::AlphaHandler.new.kind
puts Outer::Inner::AlphaReader.new.kind
puts Outer::Gate.check(:raise)
begin
  Outer::Gate.check(:other)
rescue ArgumentError => e
  puts e.message
end
"#;

const EXPECTED: &str = "handler:base\nreader:base\nraise\nbad option\n";

const FILES: &[(&str, &str)] = &[
    ("app/lib/outer/inner/base.rb", BASE),
    ("app/lib/outer/inner/alpha_handler.rb", ALPHA_HANDLER),
    ("app/lib/outer/inner/alpha_reader.rb", ALPHA_READER),
    ("app/lib/outer/gate.rb", GATE),
];

#[test]
fn native_ruby_resolves_lexical_names() {
    let app = std::env::temp_dir().join(format!("roundhouse-lexical-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    let mut requires = String::new();
    for (path, text) in FILES {
        let full = app.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        requires.push_str(&format!("require {:?}\n", full.display().to_string()));
    }
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("{requires}{REPORT}"))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&app);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

#[test]
fn emitted_ruby_resolves_lexical_names() {
    let mut fixture = emit_and_run::real_blog();
    for (path, text) in FILES {
        fixture = fixture.write(path, text);
    }
    let (emitted, errors) = fixture.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {main:?}\nMain.configure_default_adapter!\n{REPORT}"))
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "emitted program failed in {}:\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}
