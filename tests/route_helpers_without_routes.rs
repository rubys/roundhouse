//! A controller that names route helpers, in an app whose tree carries no
//! `config/routes.rb` (an engine, a gem's test app). With no routes no
//! `app/route_helpers.rb` was emitted, and the tree did not boot
//! (`cannot load such file -- app/route_helpers`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER: &str = r##"class ApplicationController < ActionController::Base
  def area_key
    "#{controller_path}##{action_name}"
  end

  def checkout_link
    cart_path
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/controllers/application_controller.rb", CONTROLLER)
}

#[test]
fn a_tree_without_routes_loads_the_route_helpers_it_names() {
    let (dir, _) = app().emit(BuildTarget::Ruby);
    let controller =
        std::fs::read_to_string(dir.join("app/controllers/application_controller.rb")).expect("controller");
    assert!(controller.contains("RouteHelpers.cart_path"), "{controller}");
    assert!(!controller.contains("RouteHelpers.controller_path"), "{controller}");
    let helpers = std::fs::read_to_string(dir.join("app/route_helpers.rb")).expect("app/route_helpers.rb");
    assert!(helpers.contains("module RouteHelpers\nend\n"), "{helpers}");
    let out = emit_and_run::ruby()
        .args([
            "-e",
            "require_relative 'app/route_helpers'\n\
             begin\n  RouteHelpers.cart_path\nrescue NoMethodError => e\n  puts e.name\nend\n",
        ])
        .current_dir(&*dir)
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "cart_path\n");
}
