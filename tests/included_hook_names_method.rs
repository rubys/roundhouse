//! An include whose `included(base)` hook names a method the class
//! defines above the include.
//!
//! A class defines `store` and `delete`, then `include Guard::Validations`,
//! whose hook runs `base.send(:alias_method, :store_without_validation,
//! :store)`. The emitted class
//! opened its body with the include, ahead of `def store`, so loading it
//! raised NameError (undefined method 'store') in the hook. The include
//! must run after the defs it names, as in the source.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const VALIDATIONS: &str = r#"module Guard
  module Validations
    class << self
      def included(base)
        base.send(:alias_method, :store_without_validation, :store)
      end
    end

    def valid?
      true
    end
  end
end
"#;

const LOGGING: &str = r#"module Guard
  module Logging
    def self.included(base)
      base.alias_method :raw_delete, :delete
    end

    def logged
      "logged #{raw_delete}"
    end
  end
end
"#;

const EXTENSION: &str = r#"module Guard
  class Extension
    class Mismatch < StandardError
    end

    def store
      "stored"
    end

    def delete
      "deleted"
    end

    include Guard::Logging
    include Guard::Validations

    def label
      "extension"
    end
  end
end
"#;

const SCRIPT: &str = r#"e = Guard::Extension.new
p [e.store, e.store_without_validation, e.logged, e.label]
p Guard::Extension.ancestors.take(3)
"#;

#[test]
fn an_included_hook_aliasing_a_method_defined_above_the_include_runs_after_it() {
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("{LOGGING}\n{VALIDATIONS}\n{EXTENSION}\n{SCRIPT}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = "[\"stored\", \"stored\", \"logged deleted\", \"extension\"]\n[Guard::Extension, Guard::Validations, Guard::Logging]\n";
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/guard/validations.rb", VALIDATIONS)
        .write("app/models/guard/logging.rb", LOGGING)
        .write("app/models/guard/extension.rb", EXTENSION)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

const ACCESSORS: &str = r#"module Guard
  module Accessors
    def self.included(base)
      base.attr_accessor :label
      base.send(:attr_reader, :tag)
    end
  end
end
"#;

/// The hook defines `label` and `tag`; it does not look them up. The
/// class's own `def label` below the include must still win.
const DECLARED: &str = r#"module Guard
  class Declared
    include Guard::Accessors

    def label
      "custom"
    end

    def tag
      "own"
    end
  end
end
"#;

const DECLARED_SCRIPT: &str = "d = Guard::Declared.new\np [d.label, d.tag]\n";

/// A hook's defining call (`attr_accessor :label`) names no method the
/// class must have defined first, so the include keeps opening the class
/// body and the hook's accessors do not replace the class's own methods.
#[test]
fn an_included_hook_defining_a_method_does_not_override_a_later_def() {
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("{ACCESSORS}\n{DECLARED}\n{DECLARED_SCRIPT}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = "[\"custom\", \"own\"]\n";
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/guard/accessors.rb", ACCESSORS)
        .write("app/models/guard/declared.rb", DECLARED)
        .run_ruby(DECLARED_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}
