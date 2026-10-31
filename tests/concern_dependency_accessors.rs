//! PostgreSQL: concern dependency accessors, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = "ActiveRecord::Schema.define { create_table(:messages) { |t| t.string :body } }";
const SECRET: &str = "module Secret\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :password\n    validates :password, presence: true\n  end\nend\n";

fn lowered_methods(app: &roundhouse::App, model: &str) -> Vec<String> {
    let model = app.models.iter().find(|m| m.name.0.as_str() == model).expect(model);
    roundhouse::lower::lower_model_to_library_class(model, &app.schema)
        .methods
        .iter()
        .map(|m| m.name.as_str().to_string())
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_module_level_concern_dependency_supplies_its_accessor_to_the_model() {
    for wrapper in [
        // Module-level dependency (User::Extension's `include HashablePassword`).
        "module Wrapper\n  extend ActiveSupport::Concern\n  include Secret\nend\n",
        // An `include` the outer block performs on the base (SofterDelete::UsingIsDeleted).
        "module Wrapper\n  extend ActiveSupport::Concern\n  included do\n    include Secret\n  end\nend\n",
        // Two levels of module-level dependency.
        "module Middle\n  extend ActiveSupport::Concern\n  include Secret\nend\nmodule Wrapper\n  extend ActiveSupport::Concern\n  include Middle\nend\n",
    ] {
        let source = format!("{SECRET}{wrapper}class NativeMessage; include ActiveModel::Validations; include Wrapper; end\nm = NativeMessage.new\nabort 'invalid without' if m.valid?\nm.password = 'x'\nabort 'valid with' unless m.valid?\nabort 'owner' unless NativeMessage.instance_method(:password=).owner == NativeMessage\n");
        let native = std::process::Command::new("ruby")
            .args(["-e", &format!("require 'active_model'\nrequire 'active_support/concern'\n{source}")])
            .output()
            .unwrap();
        assert!(native.status.success(), "{wrapper}: {}", String::from_utf8_lossy(&native.stderr));
        let files = tree(&[
            ("db/schema.rb", SCHEMA),
            ("app/models/concerns/secret.rb", SECRET),
            ("app/models/concerns/wrapper.rb", wrapper),
            ("app/models/message.rb", "class Message < ApplicationRecord\n  include Wrapper\nend\n"),
        ]);
        let app = ingest_app_from_tree(files).unwrap_or_else(|e| panic!("{wrapper}: {e}"));
        let methods = lowered_methods(&app, "Message");
        for name in ["password", "password="] {
            assert!(methods.iter().any(|m| m == name), "{wrapper}: {name} missing: {methods:?}");
        }
    }
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_dependency_of_a_load_hook_included_extension_supplies_its_accessor() {
    let check = |extension_include: &str| {
        let root = std::env::temp_dir().join(format!("rh_concern_dependency_accessors_{}_{}", std::process::id(), extension_include.len()));
        let files = [
            ("roundhouse.yml", "test_paths:\n  - test\n".to_string()),
            ("db/structure.sql", "CREATE TABLE main.accounts (\n    id bigint NOT NULL,\n    type character varying\n);\n".to_string()),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
            ("app/models/concerns/secret.rb", SECRET.to_string()),
            ("app/models/account/extension.rb", format!("class Account\n  module Extension\n    extend ActiveSupport::Concern\n    {extension_include}\n\n    #: -> Integer\n    def hello = 1\n  end\nend\n")),
            ("app/models/account.rb", "class Account < ApplicationRecord\n  #: -> untyped\n  def use = [hello.nope, password]\n  ActiveSupport.run_load_hooks(:account)\nend\n".to_string()),
            ("config/initializers/model_extensions.rb", "ActiveSupport.on_load(:account) do\n  class Account\n    include Extension\n  end\nend\n".to_string()),
        ];
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    };
    let text = check("include Secret");
    // The extension is lifted (its method types), and the dependency's
    // accessor is the model's.
    assert!(text.contains("no known method `nope` on Integer"), "{text}");
    assert!(!text.contains("concern attr_accessor"), "{text}");
    assert!(!text.contains("`password` has unresolved type"), "{text}");
    // Without the dependency there is no accessor.
    let text = check("");
    assert!(text.contains("`password` has unresolved type"), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concern_dependency_accessor_works_and_validates_in_emitted_ruby() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            r#"  module Secret
    extend ActiveSupport::Concern
    included do
      attr_accessor :password
      validates :password, presence: true
    end
  end
  module Wrapper
    extend ActiveSupport::Concern
    include Secret
  end
  include Wrapper
  has_many :comments, dependent: :destroy"#,
        )
        .run_ruby(
            r#"article = Article.new(title: "Dependency", body: "Long enough body")
raise "valid without password" if article.valid?
article.password = "hunter2"
raise "password reader" unless article.password == "hunter2"
raise "invalid with password: #{article.errors.full_messages}" unless article.valid?
"#,
        )
        .assert_passes();
}
