//! A signature can name "an instance of the class that received the
//! call" — RBS `instance`, sorbet `T.attached_class`.
//!
//! The fact a base class cannot otherwise state: an inherited factory
//! answers with an instance of whichever SUBCLASS was called, and the
//! declaring class does not know its subclasses. Written once on the
//! base, it used to be readable as the base class or not at all, and
//! everything downstream of the call went untyped.
//!
//! The substitution happens at DISPATCH, where the receiver is known —
//! against the class the ancestor walk started from, not the class the
//! signature was found on.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

fn app_from_files(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Dispatch failures, as `Receiver#method` — the sharper signal when
/// a receiver resolves to the WRONG class: an unresolved name says
/// only that nothing was known, while this says what was assumed.
fn dispatch_failures(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                Some(format!("{}#{}", roundhouse::ide::render_ty(&recv_ty), method.as_str()))
            }
            _ => None,
        })
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "gauges", force: :cascade do |t|
    t.string "label", null: false
  end
end
"#;

const BASE_RBS: &str = r#"class Facade
  def self.instance: () -> instance
end
"#;

/// The base class comes from a gem nothing models; the sidecar states
/// the one thing about it that matters.
fn files(subclass: &str, caller: &str) -> Vec<(&'static str, String)> {
    vec![
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string(),
        ),
        ("sig/vendor.rbs", BASE_RBS.to_string()),
        ("app/services/readings.rb", subclass.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/gauges_controller.rb", caller.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n"
                .to_string(),
        ),
    ]
}

fn app(subclass: &str, caller: &str) -> roundhouse::App {
    let owned = files(subclass, caller);
    let borrowed: Vec<(&str, &str)> =
        owned.iter().map(|(p, c)| (*p, c.as_str())).collect();
    app_from_files(&borrowed)
}

const SUBCLASS: &str = r#"class Readings < Facade
  def latest
    "42"
  end
end
"#;

#[test]
fn the_factory_answers_with_the_subclass_that_called_it() {
    // `Readings.instance` — the signature is on the BASE, and the only
    // useful answer is a `Readings`. Reading it as the base class
    // would lose `latest`; not reading it at all loses the chain.
    let caller = r#"class GaugesController < ApplicationController
  def index
    @label = Readings.instance.latest
  end
end
"#;
    // Asserted on dispatch, not on unresolved names: a self type that
    // was never substituted leaves the receiver as `instance` itself,
    // and the call below it fails THERE — which an unresolved-name
    // check cannot see, because the failure is about the receiver's
    // type rather than about a name nothing knew.
    let failed = dispatch_failures(&app(SUBCLASS, caller));
    assert!(
        failed.is_empty(),
        "the chain should dispatch on `Readings`; failures = {failed:?}"
    );
}

#[test]
fn the_base_class_is_still_the_answer_when_the_base_is_the_receiver() {
    // Called ON the base, `instance` is a base instance — and the base
    // has no `latest`, so this one must NOT resolve. Without that, the
    // substitution would be indistinguishable from "answers whatever
    // the call site wants".
    let caller = r#"class GaugesController < ApplicationController
  def index
    @label = Facade.instance.latest
  end
end
"#;
    // The dispatch fails ON THE BASE — naming the receiver it
    // substituted. Without that half, "substituted with the receiver"
    // would be indistinguishable from "answers whatever the call site
    // needs".
    let failed = dispatch_failures(&app(SUBCLASS, caller));
    assert!(
        failed.iter().any(|f| f == "Facade#latest"),
        "`latest` is not on the base; dispatch failures = {failed:?}"
    );
}

#[test]
fn a_self_type_survives_a_container_on_the_way_out() {
    // `Array[instance]` — the substitution recurses, so an element
    // read off the array is still the subclass.
    let base = r#"class Registry
  def self.all: () -> Array[instance]
end
"#;
    let subclass = r#"class Readings < Registry
  def latest
    "42"
  end
end
"#;
    let caller = r#"class GaugesController < ApplicationController
  def index
    @label = Readings.all.first.latest
  end
end
"#;
    let owned = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("sig/vendor.rbs", base),
        ("app/services/readings.rb", subclass),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/gauges_controller.rb", caller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ];
    let failed = dispatch_failures(&app_from_files(&owned));
    assert!(
        failed.is_empty(),
        "the element should dispatch on `Readings`; failures = {failed:?}"
    );
}

#[test]
fn sorbets_spelling_is_read_as_the_same_type() {
    // `T.attached_class` is the same fact in sorbet's grammar. Read by
    // the `sig` reader rather than the RBS one, and landing as the
    // same `Ty`, so dispatch needs to know only the one thing.
    //
    // Asserted at the READER. A `sig` seed for a method that HAS a
    // body is overwritten by the return harvested from that body
    // (`new` infers `Facade`), so the sorbet spelling reaches dispatch
    // only where the RBS one does: on a method the analyzer cannot
    // infer. That precedence is older than this change and is left
    // exactly as it was.
    let subclass = r#"class Facade
  sig { returns(T.attached_class) }
  def self.instance
    new
  end
end
"#;
    let caller = r#"class GaugesController < ApplicationController
  def index
    @label = Facade.instance
  end
end
"#;
    let owned = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/services/readings.rb", subclass),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/gauges_controller.rb", caller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ];
    let app = app_from_files(&owned);
    let ty = app
        .rbs_signatures
        .get(&roundhouse::ClassId(roundhouse::Symbol::new("Facade")))
        .and_then(|m| m.get(&roundhouse::Symbol::new("instance")))
        .cloned()
        .expect("the sig is read — without the arm the whole sig drops");
    match ty {
        roundhouse::ty::Ty::Fn { ret, .. } => {
            assert_eq!(*ret, roundhouse::ty::Ty::SelfInstance)
        }
        other => panic!("expected a function type, got {other:?}"),
    }
}
