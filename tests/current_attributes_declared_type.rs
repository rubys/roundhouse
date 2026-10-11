//! `attribute :tenant #: Account::Context::Impl` — a
//! CurrentAttributes attribute whose type rides a trailing RBS comment.
//!
//! A `Current < ActiveSupport::CurrentAttributes` class may be written so. Ingest
//! reads the comment as a cast around the `attribute` call, and the
//! lowering took only a bare `attribute` send: the declaration stayed
//! unconsumed, the class got no accessor, and `Current.tenant`
//! was `no known method` (and absent from the emitted class).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const CURRENT: &str = r#"
module Workspace
  module Account
    class Current < ActiveSupport::CurrentAttributes
      attribute :tenant #: Context::Impl
      attribute :locale, :zone #: String
    end
  end
end
"#;

const IMPL: &str = r#"
module Workspace
  module Account
    module Context
      class Impl
        #: -> Integer
        def n = 1
      end
    end
  end
end
"#;

const CONTROLLER: &str = r#"
module Workspace
  module Account
    class AppController < ApplicationController
      def show
        make unless Current.tenant
        render plain: Current.tenant&.n.zzz
      end

      private

      def make
        Current.tenant = Context::Impl.new
      end
    end
  end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/workspace/account/current.rb", CURRENT),
        ("app/models/workspace/account/context/impl.rb", IMPL),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/workspace/account/app_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(app: &roundhouse::App, stem: &str) -> String {
    let mut files = ruby::emit_spinel(app);
    files.extend(ruby::emit_library(app));
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(stem))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("no emitted file ends with {stem}"))
}

/// The accessor exists, and its read answers the written type, found
/// in the class's lexical scope (`Context::Impl` is `Workspace::Account::Context::Impl`).
#[test]
fn an_annotated_attribute_reads_its_declared_type() {
    let errors: Vec<String> = diagnose(&app())
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error"))
        .collect();
    assert!(!errors.iter().any(|e| e.contains("`tenant`")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("no known method `zzz` on Integer")), "{errors:?}");
}

/// The attribute is nil until the request sets it, so the declared
/// type reads nilable and `unless Current.tenant` is still asked.
#[test]
fn the_declared_type_stays_nilable_so_presence_is_asked() {
    let app = app();
    let sig = emitted(&app, "workspace/account/current.rbs");
    assert!(sig.contains("def tenant: () -> Context::Impl?"), "{sig}");
    let current = emitted(&app, "app/models/workspace/account/current.rb");
    assert!(current.contains("def tenant=(value)"), "{current}");
    let controller = emitted(&app, "app/controllers/workspace/account/app_controller.rb");
    assert!(controller.contains("if Workspace::Account::Current.tenant"), "{controller}");
}

/// One comment types every name the call lists; none is left untyped.
#[test]
fn a_multi_name_declaration_types_each_name() {
    let app = app();
    let sig = emitted(&app, "workspace/account/current.rbs");
    for name in ["locale", "zone"] {
        assert!(sig.contains(&format!("def {name}: () -> String?")), "{sig}");
        assert!(sig.contains(&format!("def {name}=: (String? value) -> String?")), "{sig}");
    }
}

const SCRIPT: &str = r#"
current = Workspace::Account::Current
p current.tenant
current.tenant = Workspace::Account::Context::Impl.new
p current.tenant.n
current.locale = "en"
p [current.locale, current.zone]
current.reset
p [current.tenant, current.locale]
"#;

const EXPECTED: &str = "nil\n1\n[\"en\", nil]\n[nil, nil]\n";

/// The emitted class has a reader and writer for each attribute: nil until
/// set, the value after, nil again after `reset`, as on ActiveSupport.
#[test]
fn the_emitted_current_reads_nil_until_set() {
    let sources = format!("{IMPL}\n{CURRENT}");
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require \"active_support\"\nrequire \"active_support/current_attributes\"\n{sources}\n{SCRIPT}"
        ))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED);
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/workspace/account/current.rb", CURRENT)
        .write("app/models/workspace/account/context/impl.rb", IMPL)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}
