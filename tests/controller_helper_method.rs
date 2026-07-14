//! Controller `helper_method :x` — the third helper channel (a method
//! on the controller exposed to templates). ARG-PURE marked methods
//! (no ivar reads) register in `helper_method_index` at ingest, the
//! controller lowering adds a class-side clone, and the bare view call
//! rewrites to `<Controller>.x(args)` like any app helper. Ivar-reading
//! marked methods stay instance-only (honest residue). Surfaced by
//! lobsters' DomainsController#caption_of_button refusing under spinel
//! AOT as an unresolvable bare call in domains/edit.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;

fn ingest() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![(
        "app/controllers/domains_controller.rb",
        r#"class DomainsController < ApplicationController
  def caption_of_button(domain)
    domain.banned_at ? 'Unban' : 'Ban'
  end

  helper_method :caption_of_button

  def current_thing
    @thing
  end

  helper_method :current_thing
end
"#,
    )];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn pure_helper_methods_register_and_clone_class_side() {
    let app = ingest();

    // Ingest registers the ARG-PURE one only.
    assert_eq!(
        app.helper_method_index.get(&Symbol::from("caption_of_button")),
        Some(&ClassId(Symbol::from("DomainsController"))),
        "pure helper_method must register: {:?}",
        app.helper_method_index
    );
    assert!(
        !app.helper_method_index.contains_key(&Symbol::from("current_thing")),
        "an ivar-reading helper_method must NOT register"
    );

    // The controller lowering adds a class-side clone for the pure one.
    let lcs = roundhouse::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        Vec::new(),
        Some(&app.schema),
        &app.views,
    );
    let ctrl = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "DomainsController")
        .expect("controller lowered");
    assert!(
        ctrl.methods.iter().any(|m| {
            m.name.as_str() == "caption_of_button" && m.receiver == MethodReceiver::Class
        }),
        "class-side clone synthesized: {:?}",
        ctrl.methods.iter().map(|m| (m.name.as_str(), m.receiver)).collect::<Vec<_>>()
    );
    assert!(
        !ctrl.methods.iter().any(|m| {
            m.name.as_str() == "current_thing" && m.receiver == MethodReceiver::Class
        }),
        "ivar-reading helper_method stays instance-only"
    );
}
