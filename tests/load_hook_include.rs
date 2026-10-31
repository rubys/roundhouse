//! PostgreSQL: load hook include, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

const LIFTABLE: &str = "ActiveSupport.on_load(:account) do\n  class Account\n    include Extension\n  end\nend\n";

fn check(initializer: &str, run_hooks: &str) -> String {
    check_with(initializer, run_hooks, "", &[])
}

fn check_with(initializer: &str, run_hooks: &str, account_extra: &str, extra: &[(&str, &str)]) -> String {
    let root = std::env::temp_dir().join(format!("rh_load_hook_include_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    let model = format!("class Account < ApplicationRecord\n  #: -> untyped\n  def use = [hello.succ, things.first, hello.nope]\n{account_extra}\n  {run_hooks}\nend\n");
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n".to_string()),
        (
            "db/structure.sql",
            "CREATE TABLE main.accounts (\n    id bigint NOT NULL,\n    type character varying\n);\nCREATE TABLE extra.things (\n    id bigint NOT NULL,\n    account_id bigint\n);\n".to_string(),
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\n  belongs_to :account\nend\n".to_string()),
        (
            "app/models/account/extension.rb",
            "class Account\n  module Extension\n    extend ActiveSupport::Concern\n\n    included do\n      has_many :things\n    end\n\n    #: -> Integer\n    def hello = 1\n  end\nend\n".to_string(),
        ),
        ("app/models/account.rb", model),
        ("config/initializers/model_extensions.rb", initializer.to_string()),
    ];
    let files = files.into_iter().chain(extra.iter().map(|(path, text)| (*path, text.to_string())));
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn lifted(text: &str) -> bool {
    text.contains("no known method `nope` on Integer") && !text.contains("unresolved type")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_reopen_include_types_the_extension_and_its_association() {
    let text = check(LIFTABLE, "ActiveSupport.run_load_hooks(:account)");
    assert!(lifted(&text), "{text}");
    assert!(!text.contains("initializer_unsupported"), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_block_doing_more_than_include_stays_refused() {
    let initializer = "ActiveSupport.on_load(:account) do\n  class Account\n    include Extension\n    self.table_name = \"accounts\"\n  end\nend\n";
    let text = check(initializer, "ActiveSupport.run_load_hooks(:account)");
    assert!(text.contains("model_extensions.rb:1:1: error[initializer_unsupported]"), "{text}");
    assert!(!lifted(&text), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_hook_no_class_body_fires_stays_refused() {
    let text = check(LIFTABLE, "");
    assert!(text.contains("model_extensions.rb:1:1: error[initializer_unsupported]"), "{text}");
    assert!(!lifted(&text), "{text}");
}

#[test]
fn a_bare_include_of_a_relative_name_is_not_lifted() {
    // `Extension` in the block is `::Extension`, which does not exist.
    let initializer = "ActiveSupport.on_load(:account) do\n  include Extension\nend\n";
    let text = check(initializer, "ActiveSupport.run_load_hooks(:account, self)");
    assert!(!lifted(&text), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_bare_include_of_a_top_level_path_is_lifted() {
    let initializer = "ActiveSupport.on_load(:account) do\n  include Account::Extension\nend\n";
    let text = check(initializer, "ActiveSupport.run_load_hooks(:account, self)");
    assert!(lifted(&text), "{text}");
    assert!(!text.contains("initializer_unsupported"), "{text}");
}

#[test]
fn a_bare_include_under_an_object_base_is_not_lifted() {
    // Without `self` the block runs in Object, not in Account.
    let initializer = "ActiveSupport.on_load(:account) do\n  include Account::Extension\nend\n";
    let text = check(initializer, "ActiveSupport.run_load_hooks(:account)");
    assert!(!lifted(&text), "{text}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_sti_subclass_takes_the_include() {
    let initializer = format!("{LIFTABLE}\nActiveSupport.on_load(:sub) do\n  class Sub\n    include Extension\n  end\nend\n");
    let sub = "class Sub < Account\n  ActiveSupport.run_load_hooks(:sub)\nend\n";
    let sub_extension = "class Sub\n  module Extension\n    #: -> Integer\n    def bye = 1\n  end\nend\n";
    let text = check_with(
        &initializer,
        "ActiveSupport.run_load_hooks(:account)",
        "\n  #: (Sub s) -> untyped\n  def through(s) = [s.bye.succ, s.zzz]\n",
        &[("app/models/sub.rb", sub), ("app/models/sub/extension.rb", sub_extension)],
    );
    assert!(lifted(&text), "{text}");
    assert!(text.contains("no known method `zzz` on Sub"), "{text}");
    assert!(!text.contains("`bye`"), "{text}");
    assert!(!text.contains("initializer_unsupported"), "{text}");
}
