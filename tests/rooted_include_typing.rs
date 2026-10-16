//! `include ::A::B` names the top-level `A::B`.
//!
//! Ingest keeps the root as an empty first path segment, and the
//! model's include list joined it whole as `::A::B`, which no registry
//! entry carries. `include ::Vendor::ApiClient` was refused as an unresolved
//! include, and the module's `mixes_in_class_methods(ClassMethods)` never gave
//! the includer `current`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const API_CLIENT: &str = "module Vendor\n  module ApiClient\n    extend T::Sig\n    extend T::Helpers\n\n    module ClassMethods\n      extend T::Sig\n\n      sig { returns(Integer) }\n      def current = 1\n    end\n\n    sig { returns(T::Boolean) }\n    def internal_access? = false\n\n    mixes_in_class_methods(ClassMethods)\n  end\nend\n";

fn check(name: &str, include: &str) -> String {
    let root = std::env::temp_dir().join(format!("rh_rooted_include_typing_{}_{name}", std::process::id()));
    let account = format!("class Account < ApplicationRecord\n  include {include}\n\n  #: -> untyped\n  def self.use = [Account.current.zzz, new.internal_access?.yyy]\nend\n");
    let files = [
        ("roundhouse.yml".to_string(), "test_paths:\n  - test\n".to_string()),
        ("db/structure.sql".to_string(), "CREATE TABLE accounts (\n    id bigint NOT NULL\n);\n".to_string()),
        ("app/models/application_record.rb".to_string(), "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("app/services/vendor.rb".to_string(), API_CLIENT.to_string()),
        ("app/models/account.rb".to_string(), account),
    ];
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_rooted_include_reaches_both_sides() {
    for (name, include) in [("rooted", "::Vendor::ApiClient"), ("relative", "Vendor::ApiClient")] {
        let text = check(name, include);
        assert!(!text.contains("includes unresolved"), "{name}: {text}");
        assert!(text.contains("no known method `zzz` on Integer"), "{name}: {text}");
        assert!(text.contains("no known method `yyy` on bool"), "{name}: {text}");
    }
}

/// The same module on native Ruby, with the Sorbet declarations spelled
/// as the `extend` hook they stand for.
const NATIVE_API_CLIENT: &str = "module Vendor\n  module ApiClient\n    module ClassMethods\n      def current = 1\n    end\n\n    def internal_access? = false\n\n    def self.included(base)\n      base.extend(ClassMethods)\n    end\n  end\nend\n";

const SCRIPT: &str = "p [Account.current, Account.new.internal_access?]\n";

const EXPECTED: &str = "[1, false]\n";

/// Rooted and relative includes both load, and give the includer the
/// module's class-side and instance-side methods, as on native Ruby.
#[test]
fn a_rooted_include_runs_with_its_class_methods() {
    for (name, include) in [("rooted", "::Vendor::ApiClient"), ("relative", "Vendor::ApiClient")] {
        let account = format!("class Account < ApplicationRecord\n  include {include}\nend\n");
        let native = emit_and_run::ruby()
            .arg("-e")
            .arg(format!(
                "class ApplicationRecord; end\n{NATIVE_API_CLIENT}\n{account}\n{SCRIPT}"
            ))
            .output()
            .expect("native Ruby control");
        assert!(native.status.success(), "{name}: {}", String::from_utf8_lossy(&native.stderr));
        assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED, "{name}");
        let run = emit_and_run::empty_app()
            .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
            .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n")
            .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
            .write("app/services/vendor.rb", API_CLIENT)
            .write("app/models/account.rb", &account)
            .run_ruby(SCRIPT);
        run.assert_passes();
        assert_eq!(run.stdout, EXPECTED, "{name}");
    }
}
