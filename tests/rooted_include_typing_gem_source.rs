//! A rooted `include` of a module from a gem compiled as source reaches
//! both the class and instance side.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const GEM_API_CLIENT: &str = "module Fakeav\n  module ApiClient\n    extend T::Sig\n    extend T::Helpers\n\n    module ClassMethods\n      extend T::Sig\n\n      sig { returns(Integer) }\n      def current = 1\n    end\n\n    sig { returns(T::Boolean) }\n    def internal_access? = false\n\n    mixes_in_class_methods(ClassMethods)\n  end\nend\n";


fn check_gem(name: &str, include: &str) -> String {
    let root = std::env::temp_dir().join(format!("rh_rooted_include_typing_gem_{}_{name}", std::process::id()));
    let account = format!("class Account < ApplicationRecord\n  include {include}\n\n  #: -> untyped\n  def self.use = [Account.current.zzz, new.internal_access?.yyy]\nend\n");
    let files = [
        ("roundhouse.yml".to_string(), "test_paths:\n  - test\n".to_string()),
        ("Gemfile.lock".to_string(), "PATH\n  remote: vendor/gems/fakeav-1.0\n  specs:\n    fakeav (1.0)\n\nDEPENDENCIES\n  fakeav!\n".to_string()),
        ("db/structure.sql".to_string(), "CREATE TABLE main.accounts (\n    id bigint NOT NULL\n);\n".to_string()),
        ("app/models/application_record.rb".to_string(), "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("vendor/gems/fakeav-1.0/lib/fakeav.rb".to_string(), "require \"fakeav/api_client\"\n".to_string()),
        ("vendor/gems/fakeav-1.0/lib/fakeav/api_client.rb".to_string(), GEM_API_CLIENT.to_string()),
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
fn a_rooted_include_from_a_gem_source_reaches_both_sides() {
    for (name, include) in [("rooted", "::Fakeav::ApiClient"), ("relative", "Fakeav::ApiClient")] {
        let text = check_gem(name, include);
        assert!(!text.contains("includes unresolved"), "{name}: {text}");
        assert!(text.contains("no known method `zzz` on Integer"), "{name}: {text}");
        assert!(text.contains("no known method `yyy` on bool"), "{name}: {text}");
    }
}
