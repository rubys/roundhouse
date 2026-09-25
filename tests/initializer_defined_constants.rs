//! A top-level module an initializer DEFINES, kept when the app's own
//! code names it.
//!
//! Lobsters' `config/initializers/telebugs.rb` opens with
//!
//! ```ruby
//! module Telebugs
//!   def self.user *args, **kwargs
//!   end
//!   …
//! end
//! ```
//!
//! and reopens it to forward to Sentry only inside an
//! `if credentials.telebugs.present?`. `authenticate_user` calls
//! `Telebugs.user` on every signed-in request, so dropping the module
//! (the old rule kept only initializer modules that were mixed in)
//! turned each of those requests into a NameError. An initializer
//! module the app never names (`SneakWrapperIntoPath`, prepended into
//! `rails dbconsole`) stays out of the tree.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const TELEBUGS: &str = r#"module Telebugs
  def self.user *args, **kwargs
  end
end

if ENV["TELEBUGS_DSN"]
  module Telebugs
    def self.user id:, username:
      Sentry.set_user id:, username:
    end
  end
end
"#;

const DBCONSOLE: &str = r#"module SneakWrapperIntoPath
  def find_cmd_and_exec(commands, *args)
    super
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  def touch_user\n    Telebugs.user id: 1, username: name\n  end\nend\n",
        ),
        ("config/initializers/telebugs.rb", TELEBUGS),
        ("config/initializers/dbconsole.rb", DBCONSOLE),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn a_referenced_initializer_module_is_ingested_once() {
    let app = app();
    let telebugs: Vec<_> = app
        .library_classes
        .iter()
        .filter(|lc| lc.name.0.as_str() == "Telebugs")
        .collect();
    assert_eq!(telebugs.len(), 1, "only the top-level definition, not the `if` reopen");
    let user = telebugs[0]
        .methods
        .iter()
        .find(|m| m.name.as_str() == "user")
        .expect("Telebugs.user");
    // `*args, **kwargs` with an empty body: the `**` slot cannot follow
    // the rest as a defaulted positional (it would not parse), and a
    // caller's keywords already land in the rest.
    let names: Vec<&str> = user.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["args"], "params = {:?}", user.params);
}

#[test]
fn an_unreferenced_initializer_module_stays_out() {
    let app = app();
    assert!(
        !app.library_classes.iter().any(|lc| lc.name.0.as_str() == "SneakWrapperIntoPath"),
        "classes = {:?}",
        app.library_classes.iter().map(|lc| lc.name.0.as_str()).collect::<Vec<_>>()
    );
}
