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
    // Even unread keyword-rest consumes keywords separately from *args.
    // Keep it native, rather than flattening it to a positional default
    // or dropping it and letting keywords leak into the rest array.
    let names: Vec<&str> = user.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["args", "kwargs"], "params = {:?}", user.params);
    assert!(user.params[0].rest && !user.params[0].keyword);
    assert!(user.params[1].rest && user.params[1].keyword);
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

/// A `module X` / `class X` declaration names X without reading it: an
/// app model written `module Shops; class Item …` is not the app naming
/// an initializer's `module Shops`, so that module stays out (the
/// autoloaded namespace is what the model reopens).
#[test]
fn a_namespace_declaration_around_an_app_model_is_not_a_read() {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/shops/item.rb",
            "module Shops\n  class Item < ApplicationRecord\n    self.table_name = \"rooms\"\n  end\nend\n",
        ),
        (
            "config/initializers/shops.rb",
            "module Shops\n  def self.ids_tag\n    :tag\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    assert!(
        !app.library_classes.iter().any(|lc| lc.name.0.as_str() == "Shops"),
        "classes = {:?}",
        app.library_classes.iter().map(|lc| lc.name.0.as_str()).collect::<Vec<_>>()
    );
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The same namespace once the app does read it (`Shops.ids_tag` from a
/// model nested in `module Shops`): the initializer's module is kept and
/// its method resolves at run time, so the declaration alone not counting
/// as a read drops nothing a caller needs.
#[test]
fn an_initializer_module_the_app_reads_inside_its_namespace_still_resolves() {
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/models/shops/item.rb",
            "module Shops\n  class Item < ApplicationRecord\n    self.table_name = \"rooms\"\n\n    def tag\n      Shops.ids_tag\n    end\n  end\nend\n",
        )
        .write("config/initializers/shops.rb", "module Shops\n  def self.ids_tag\n    :tag\n  end\nend\n")
        .run_ruby("p Shops::Item.new.tag\n");
    run.assert_passes();
    assert_eq!(run.stdout, ":tag\n");
}
