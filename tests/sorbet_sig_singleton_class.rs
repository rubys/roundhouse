//! A `sig` inside `class << self` is read.
//!
//! The walk descended into class and module bodies only, so a class
//! that declares its whole singleton surface with `class << self` —
//! the idiom for more than one class method — read as having declared
//! nothing at all. Not a degraded type: no type.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn app(service: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/services/registry.rb", service),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn signature(app: &roundhouse::App, class: &str, method: &str) -> Option<Ty> {
    app.rbs_signatures
        .get(&ClassId(Symbol::new(class)))
        .and_then(|methods| methods.get(&Symbol::new(method)))
        .cloned()
}

fn ret_of(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Fn { ret, .. } => Some((**ret).clone()),
        _ => None,
    }
}

#[test]
fn a_sig_in_a_singleton_class_body_lands_on_the_enclosing_class() {
    let app = app(r#"class Registry
  class << self
    extend T::Sig

    sig { params(code: String).returns(String) }
    def label_for(code)
      code.upcase
    end
  end
end
"#);
    let ty = signature(&app, "Registry", "label_for").expect("the sig is read");
    assert_eq!(ret_of(&ty), Some(Ty::Str));
    let Ty::Fn { params, .. } = &ty else { panic!("expected a function type, got {ty:?}") };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].ty, Ty::Str);
}

#[test]
fn a_receiverless_def_there_is_still_singleton_side() {
    // The trap this opens: inside `class << self` a `def` carries no
    // receiver, but it is a CLASS method all the same. `T.self_type`
    // therefore means the class object, which `Ty` cannot spell — so
    // the signature stays unread, exactly as it does on a `def
    // self.x`. Reading it as an instance would be a wrong type rather
    // than a missing one, and nothing downstream could tell.
    let app = app(r#"class Registry
  class << self
    extend T::Sig

    sig { returns(T.self_type) }
    def build
      self
    end

    sig { returns(T.attached_class) }
    def instance
      new
    end
  end
end
"#);
    assert_eq!(
        signature(&app, "Registry", "build"),
        None,
        "a singleton `self` is not readable"
    );
    // `T.attached_class` IS readable there — it means an instance of
    // the attached class whichever side declares it.
    assert_eq!(
        ret_of(&signature(&app, "Registry", "instance").expect("attached_class is read")),
        Some(Ty::SelfInstance)
    );
}

#[test]
fn the_enclosing_scope_is_not_lost_on_the_way_back_out() {
    // A `class << self` block in the middle of a body must not take
    // the scope with it: the `def` after it belongs to the same class.
    let app = app(r#"class Registry
  class << self
    extend T::Sig

    sig { returns(Integer) }
    def count
      0
    end
  end

  sig { returns(String) }
  def label
    "x"
  end
end
"#);
    assert_eq!(ret_of(&signature(&app, "Registry", "count").expect("class side")), Some(Ty::Int));
    assert_eq!(
        ret_of(&signature(&app, "Registry", "label").expect("instance side")),
        Some(Ty::Str)
    );
}
