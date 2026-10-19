//! Every superclass and `include` target the source writes is resolved
//! once, from Rubydex's answers, and stored on the IR as the name Ruby
//! binds it to.
//!
//! The written name alone cannot say which class it is. Ruby looks a
//! constant up in the lexical scopes around it, then in the ancestors of
//! the innermost one, then at the top level, and the class's own name
//! is not its nesting:
//!
//! ```ruby
//! module Billing
//!   class Invoice < Record; end      # Billing::Record
//! end
//! class Billing::Receipt < Record; end # ::Record
//! ```
//!
//! Each consumer used to re-derive the parent from the class's name.
//! Analysis tried the bare name first and said `::Record` for both; the
//! Ruby emitter tried the class's own segments and said
//! `Billing::Record` for both.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const RECORD: &str = "class Record\n  def kind = \"top\"\n  def code = \"top\"\nend\n";

const BILLING_RECORD: &str = "module Billing\n  class Record\n    def kind = \"billing\"\n    def code = 2\n  end\nend\n";

/// Nested: `Billing` is a lexical scope of the superclass expression.
const INVOICE: &str = "module Billing\n  class Invoice < Record\n  end\nend\n";

/// Compact: the `Billing::` prefix names the class without opening
/// `Billing`, so `Record` is the top-level one.
const RECEIPT: &str = "class Billing::Receipt < Record\nend\n";

/// Qualified and rooted superclasses.
const QUALIFIED: &str = "class Qualified < Billing::Record\nend\n";
const ROOTED: &str = "module Billing\n  class Rooted < ::Record\n  end\nend\n";

/// Past the lexical scopes, Ruby searches the ancestors of the class
/// whose body includes the module.
const PARENT: &str = "class Parent\n  module Helpers\n    def helped = 1\n  end\nend\n";
const CHILD: &str = "class Child < Parent\n  include Helpers\nend\n";

/// A relative mixin path resolves from an enclosing module.
const SHOP: &str = r#"
module Shop
  module Concerns
    module Audited
      def audited = true
    end
  end

  class Order
    include Concerns::Audited
  end
end
"#;

const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const BASE_CONTROLLER: &str = "class BaseController < ApplicationController\nend\n";
const ADMIN_BASE_CONTROLLER: &str = "module Admin\n  class BaseController < ApplicationController\n  end\nend\n";
const ADMIN_NOTES_CONTROLLER: &str = "module Admin\n  class NotesController < BaseController\n  end\nend\n";
const ADMIN_REPORTS_CONTROLLER: &str = "class Admin::ReportsController < BaseController\nend\n";

/// Dispatch through each parent: the two `code`s return different types.
const PROBE: &str = "class Probe\n  def invoice_code = Billing::Invoice.new.code\n  def receipt_code = Billing::Receipt.new.code\nend\n";

const FILES: &[(&str, &str)] = &[
    ("app/models/record.rb", RECORD),
    ("app/models/billing/record.rb", BILLING_RECORD),
    ("app/models/billing/invoice.rb", INVOICE),
    ("app/models/billing/receipt.rb", RECEIPT),
    ("app/models/qualified.rb", QUALIFIED),
    ("app/models/billing/rooted.rb", ROOTED),
    ("app/models/parent.rb", PARENT),
    ("app/models/child.rb", CHILD),
    ("app/models/shop.rb", SHOP),
    ("app/models/probe.rb", PROBE),
];

fn app() -> roundhouse::App {
    let controllers = [
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/base_controller.rb", BASE_CONTROLLER),
        ("app/controllers/admin/base_controller.rb", ADMIN_BASE_CONTROLLER),
        ("app/controllers/admin/notes_controller.rb", ADMIN_NOTES_CONTROLLER),
        ("app/controllers/admin/reports_controller.rb", ADMIN_REPORTS_CONTROLLER),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .chain(FILES.iter().copied())
    .chain(controllers)
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn class<'a>(app: &'a roundhouse::App, name: &str) -> &'a roundhouse::dialect::LibraryClass {
    app.library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == name)
        .unwrap_or_else(|| panic!("no library class {name}"))
}

fn parent(app: &roundhouse::App, name: &str) -> String {
    class(app, name).parent.as_ref().map(|p| p.0.as_str().to_string()).unwrap_or_default()
}

fn includes(app: &roundhouse::App, name: &str) -> Vec<String> {
    class(app, name).includes.iter().map(|c| c.0.as_str().to_string()).collect()
}

#[test]
fn a_superclass_is_the_class_ruby_binds_in_its_real_nesting() {
    let app = app();
    assert_eq!(parent(&app, "Billing::Invoice"), "Billing::Record");
    assert_eq!(parent(&app, "Billing::Receipt"), "Record");
    assert_eq!(parent(&app, "Qualified"), "Billing::Record");
    assert_eq!(parent(&app, "Billing::Rooted"), "Record");
}

/// Method lookup walks the stored parent, so a call on the compact class
/// reaches the top-level `Record` and one on the nested class reaches
/// `Billing::Record`.
#[test]
fn dispatch_follows_the_resolved_superclass() {
    let app = app();
    let ret = |method: &str| {
        let def = class(&app, "Probe").methods.iter().find(|m| m.name.as_str() == method)
            .unwrap_or_else(|| panic!("no Probe#{method}"));
        match def.signature.as_ref() {
            Some(roundhouse::ty::Ty::Fn { ret, .. }) => (**ret).clone(),
            other => panic!("Probe#{method} has no signature: {other:?}"),
        }
    };
    assert_eq!(ret("invoice_code"), roundhouse::ty::Ty::Int);
    assert_eq!(ret("receipt_code"), roundhouse::ty::Ty::Str);
}

#[test]
fn a_mixin_resolves_through_ancestors_and_enclosing_modules() {
    let app = app();
    assert_eq!(includes(&app, "Child"), ["Parent::Helpers"]);
    assert_eq!(includes(&app, "Shop::Order"), ["Shop::Concerns::Audited"]);
    let errors: Vec<String> = diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_controller_superclass_follows_nesting_not_the_class_name() {
    let app = app();
    let parent = |name: &str| {
        app.controllers
            .iter()
            .find(|c| c.name.0.as_str() == name)
            .and_then(|c| c.parent.as_ref())
            .map(|p| p.0.as_str().to_string())
            .unwrap_or_else(|| panic!("no controller {name}"))
    };
    assert_eq!(parent("Admin::NotesController"), "Admin::BaseController");
    assert_eq!(parent("Admin::ReportsController"), "BaseController");
}

const SCRIPT: &str = r#"
p [Billing::Invoice.new.kind, Billing::Receipt.new.kind, Qualified.new.kind, Billing::Rooted.new.kind]
p [Billing::Invoice.superclass, Billing::Receipt.superclass]
p [Child.new.helped, Shop::Order.new.audited]
"#;

const EXPECTED: &str = "[\"billing\", \"top\", \"billing\", \"top\"]\n[Billing::Record, Record]\n[1, true]\n";

/// The emitted program binds each name as native Ruby does. The emitter
/// nests every segment of a class name, so the compact `Billing::Receipt`
/// is emitted inside `module Billing` and has to spell `::Record`.
#[test]
fn the_resolved_names_run_as_on_native_ruby() {
    let order = [RECORD, BILLING_RECORD, INVOICE, RECEIPT, QUALIFIED, ROOTED, PARENT, CHILD, SHOP, PROBE];
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("{}\n{SCRIPT}", order.join("\n")))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED);
    let mut run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n");
    for (path, content) in FILES {
        run = run.write(path, content);
    }
    let run = run.run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}
