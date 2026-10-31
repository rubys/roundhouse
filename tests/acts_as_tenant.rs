//! The `acts_as_tenant` gem: `acts_as_tenant :account` is expanded into
//! the `belongs_to`, the dynamic `default_scope` and the create stamp it
//! stands for, `validates_uniqueness_to_tenant` into a uniqueness scoped
//! to the tenant key, `ActsAsTenant` into a `CurrentAttributes` class, and
//! `set_current_tenant_through_filter` into `set_current_tenant` /
//! `current_tenant`. Options and settings it does not model are
//! refused, not dropped.

use roundhouse::dialect::{ControllerBodyItem, ModelBodyItem, ValidationRule};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    acts_as_tenant (1.0.1)\n      activerecord (>= 6.0)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  acts_as_tenant (~> 1.0)\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "accounts", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "account_id"
    t.string "name"
  end
  create_table "gadgets", force: :cascade do |t|
    t.integer "owner_id"
    t.string "name"
  end
end
"#;

const ACCOUNT: &str = "class Account < ApplicationRecord\n  has_many :widgets\nend\n";
const WIDGET: &str = "class Widget < ApplicationRecord\n  acts_as_tenant :account\n  validates_uniqueness_to_tenant :name\nend\n";
const RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";
const CONTROLLER: &str = "class ApplicationController < ActionController::API\n  set_current_tenant_through_filter\n  before_action :find_tenant\n\n  private\n\n  def find_tenant\n    set_current_tenant(Account.find_by(id: params[:account_id]))\n  end\nend\n";
const WIDGETS_CONTROLLER: &str = "class WidgetsController < ApplicationController\n  def index\n    render json: Widget.includes(:account).order(name: :asc).map { |w| { id: w.id, account_id: w.account_id } }\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  resources :widgets, only: %i[index]\nend\n";

fn tree(lock: Option<&str>, files: &[(&str, &str)]) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
    let mut all: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        ("app/models/application_record.rb".into(), RECORD.into()),
        ("app/models/account.rb".into(), ACCOUNT.into()),
    ];
    if let Some(lock) = lock {
        all.push(("Gemfile.lock".into(), lock.into()));
    }
    for (path, src) in files {
        all.push(((*path).into(), (*src).into()));
    }
    all.into_iter().map(|(p, c)| (p.into(), c.into_bytes())).collect()
}

fn model<'a>(app: &'a roundhouse::App, name: &str) -> &'a roundhouse::dialect::Model {
    app.models.iter().find(|m| m.name.0.as_str() == name).unwrap_or_else(|| panic!("{name}"))
}

fn unknown_calls(m: &roundhouse::dialect::Model) -> Vec<String> {
    m.body
        .iter()
        .filter_map(|i| match i {
            ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                ExprNode::Send { method, .. } => Some(method.as_str().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn the_macro_becomes_belongs_to_a_dynamic_default_scope_and_a_create_stamp() {
    let app = ingest_app_from_tree(tree(Some(LOCK), &[("app/models/widget.rb", WIDGET)])).expect("ingest");
    let widget = model(&app, "Widget");
    let assocs: Vec<String> = widget
        .body
        .iter()
        .filter_map(|i| match i {
            ModelBodyItem::Association { assoc, .. } => Some(assoc.name().as_str().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(assocs, ["account"]);
    // The macro is gone; the default_scope is what replaced it.
    let unknown = unknown_calls(widget);
    assert!(!unknown.iter().any(|m| m == "acts_as_tenant" || m == "validates_uniqueness_to_tenant"), "{unknown:?}");
    assert!(unknown.iter().any(|m| m == "default_scope"), "{unknown:?}");
    let scope = format!("{:?}", widget.body.iter().find_map(|i| match i {
        ModelBodyItem::Unknown { expr, .. } if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "default_scope") => Some(expr),
        _ => None,
    }).unwrap());
    assert!(scope.contains("ActsAsTenant") && scope.contains("current_tenant") && scope.contains("account_id"), "{scope}");
    // The create stamp: the block-form callback the model lowering reads.
    assert_eq!(unknown.iter().filter(|m| *m == "before_validation").count(), 1, "{unknown:?}");
    // Uniqueness scoped to the tenant's key.
    let scopes: Vec<Vec<String>> = widget
        .body
        .iter()
        .filter_map(|i| match i {
            ModelBodyItem::Validation { validation, .. } => Some(validation.rules.iter().filter_map(|r| match r {
                ValidationRule::Uniqueness { scope, .. } => Some(scope.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>()),
                _ => None,
            })),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(scopes, [vec!["account_id".to_string()]]);
    // The gem's state, as a CurrentAttributes class reset per request.
    let state = app.library_classes.iter().find(|c| c.name.0.as_str() == "ActsAsTenant").expect("ActsAsTenant");
    let methods: Vec<String> = state.methods.iter().map(|m| format!("{:?}:{}", m.receiver, m.name.as_str())).collect();
    for want in ["Class:current_tenant", "Class:current_tenant=", "Class:with_tenant", "Class:without_tenant"] {
        assert!(methods.iter().any(|m| m == want), "{want} in {methods:?}");
    }
    assert!(app.current_attribute_classes.iter().any(|c| c.0.as_str() == "ActsAsTenant"));
}

#[test]
fn options_rename_the_key_and_the_class() {
    let gadget = "class Gadget < ApplicationRecord\n  acts_as_tenant :owner, class_name: \"Account\", foreign_key: :owner_id, optional: true\n  validates_uniqueness_to_tenant :name, scope: :id, case_sensitive: false\nend\n";
    let app = ingest_app_from_tree(tree(Some(LOCK), &[("app/models/gadget.rb", gadget)])).expect("ingest");
    let gadget = model(&app, "Gadget");
    let debug = format!("{:?}", gadget.body);
    assert!(debug.contains("owner_id") && !debug.contains("account_id"), "{debug}");
    let uniq = gadget.body.iter().find_map(|i| match i {
        ModelBodyItem::Validation { validation, .. } => validation.rules.iter().find_map(|r| match r {
            ValidationRule::Uniqueness { scope, case_sensitive, .. } => Some((scope.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>(), *case_sensitive)),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(uniq, Some((vec!["id".to_string(), "owner_id".to_string()], false)));
}

#[test]
fn the_controller_filter_macro_becomes_its_methods() {
    let app = ingest_app_from_tree(tree(
        Some(LOCK),
        &[("app/models/widget.rb", WIDGET), ("app/controllers/application_controller.rb", CONTROLLER)],
    ))
    .expect("ingest");
    let ctrl = app.controllers.iter().find(|c| c.name.0.as_str() == "ApplicationController").unwrap();
    let actions: Vec<&str> = ctrl
        .body
        .iter()
        .filter_map(|i| match i {
            ControllerBodyItem::Action { action, .. } => Some(action.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(actions.contains(&"set_current_tenant") && actions.contains(&"current_tenant"), "{actions:?}");
    assert!(!format!("{:?}", ctrl.body).contains("set_current_tenant_through_filter"));
}

#[test]
fn a_scoped_read_is_not_folded_to_unscoped_sql_at_transpile_time() {
    let mut app = ingest_app_from_tree(tree(
        Some(LOCK),
        &[
            ("app/models/widget.rb", WIDGET),
            ("app/controllers/application_controller.rb", CONTROLLER),
            ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
            ("config/routes.rb", ROUTES),
        ],
    ))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-blog"), roundhouse::project::BuildTarget::Spinel)
        .expect("emit");
    let ctrl = &files.iter().find(|(p, _)| p.ends_with("app/controllers/widgets_controller.rb")).expect("controller").1;
    assert!(!ctrl.contains("FROM widgets ORDER BY"), "the scope was folded away:\n{ctrl}");
    assert!(ctrl.contains("ActiveRecord::Relation.new(Widget)"), "{ctrl}");
    let widget = &files.iter().find(|(p, _)| p.ends_with("app/models/widget.rb")).expect("model").1;
    assert!(widget.contains("def self._default_scope_where"), "{widget}");
    assert!(widget.contains("self.account_id = ActsAsTenant.current_tenant.id if ActsAsTenant.current_tenant"), "{widget}");
    let main = &files.iter().find(|(p, _)| p == "main.rb").expect("main").1;
    assert!(main.contains("ActsAsTenant.reset"), "the tenant is reset per request");
}

#[test]
fn unmodelled_options_are_refused() {
    for opts in ["has_global_records: true", "through: :project", "polymorphic: true", "foreign_type: :kind", "scope_name: :x"] {
        let src = format!("class Widget < ApplicationRecord\n  acts_as_tenant :account, {opts}\nend\n");
        let err = ingest_app_from_tree(tree(Some(LOCK), &[("app/models/widget.rb", &src)])).err().unwrap_or_else(|| panic!("{opts} accepted"));
        let key = opts.split(':').next().unwrap();
        assert!(err.to_string().contains("acts_as_tenant") && err.to_string().contains(key), "{opts}: {err}");
    }
    let scoped = "class Widget < ApplicationRecord\n  acts_as_tenant :account, -> { where(name: \"x\") }\nend\n";
    assert!(ingest_app_from_tree(tree(Some(LOCK), &[("app/models/widget.rb", scoped)])).is_err());
    let uniq = "class Widget < ApplicationRecord\n  acts_as_tenant :account\n  validates_uniqueness_to_tenant :name, if: :draft?\nend\n";
    let err = ingest_app_from_tree(tree(Some(LOCK), &[("app/models/widget.rb", uniq)])).err().expect("refused");
    assert!(err.to_string().contains("validates_uniqueness_to_tenant"), "{err}");
}

#[test]
fn a_require_tenant_setting_is_refused() {
    let init = "ActsAsTenant.configure do |config|\n  config.require_tenant = true\nend\n";
    let err = ingest_app_from_tree(tree(
        Some(LOCK),
        &[("app/models/widget.rb", WIDGET), ("config/initializers/acts_as_tenant.rb", init)],
    ))
    .err()
    .expect("refused");
    assert!(err.to_string().contains("require_tenant"), "{err}");
}

#[test]
fn without_the_gem_in_the_lock_the_macro_is_left_alone() {
    let app = ingest_app_from_tree(tree(None, &[("app/models/widget.rb", WIDGET)])).expect("ingest");
    assert!(unknown_calls(model(&app, "Widget")).iter().any(|m| m == "acts_as_tenant"));
    assert!(!app.library_classes.iter().any(|c| c.name.0.as_str() == "ActsAsTenant"));
}

/// A single-quoted `message:` is literal text in Ruby (and Rails keeps it
/// verbatim): re-rendering it into the synthesized source must not turn
/// `#{...}`, `#@x` or `#$x` into interpolation.
#[test]
fn a_literal_uniqueness_message_stays_literal() {
    let msg = r##"taken #{x} #@y #$z \ "q""##;
    let src = format!("class Widget < ApplicationRecord\n  acts_as_tenant :account\n  validates_uniqueness_to_tenant :name, message: '{}'\nend\n", msg.replace('\\', "\\\\"));
    let app = ingest_app_from_tree(tree(Some(LOCK), &[("app/models/widget.rb", &src)])).expect("ingest");
    let got: Vec<Option<String>> = model(&app, "Widget")
        .body
        .iter()
        .filter_map(|i| match i {
            ModelBodyItem::Validation { validation, .. } => Some(validation.message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(got, [Some(msg.to_string())]);
}

/// The gem's `belongs_to` is a second declaration of the association, and
/// in Rails the later declaration's reflection wins (Rails 8.1.4: with the
/// model's `belongs_to :account, foreign_key: :legacy_account_id` first,
/// `reflect_on_association(:account).foreign_key` is `account_id`; with
/// it after the macro, `legacy_account_id`). The scope and the stamp use
/// the gem's key either way.
#[test]
fn a_redeclared_tenant_association_follows_the_later_declaration() {
    let schema_with_legacy = SCHEMA.replace("    t.integer \"account_id\"\n", "    t.integer \"account_id\"\n    t.integer \"legacy_account_id\"\n");
    let fkey = |src: &str| {
        let files: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = tree(Some(LOCK), &[("app/models/widget.rb", src)])
            .into_iter()
            .map(|(p, c)| if p.ends_with("db/schema.rb") { (p, schema_with_legacy.clone().into_bytes()) } else { (p, c) })
            .collect();
        let app = ingest_app_from_tree(files).expect("ingest");
        let widget = model(&app, "Widget");
        let assocs: Vec<String> = widget
            .body
            .iter()
            .filter_map(|i| match i {
                ModelBodyItem::Association { assoc, .. } => Some(format!("{assoc:?}")),
                _ => None,
            })
            .collect();
        assert_eq!(assocs.len(), 1, "{assocs:?}");
        assocs[0].contains("legacy_account_id")
    };
    let own_first = "class Widget < ApplicationRecord\n  belongs_to :account, foreign_key: :legacy_account_id\n  acts_as_tenant :account\nend\n";
    let own_last = "class Widget < ApplicationRecord\n  acts_as_tenant :account\n  belongs_to :account, foreign_key: :legacy_account_id\nend\n";
    assert!(!fkey(own_first), "the gem's later belongs_to (account_id) wins");
    assert!(fkey(own_last), "the model's later belongs_to (legacy_account_id) wins");
}
