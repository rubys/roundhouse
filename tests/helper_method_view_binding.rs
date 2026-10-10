//! `helper_method :name` — the app declaring which controller methods a
//! view may call.
//!
//! Our views lower to module FUNCTIONS with no controller instance, so a
//! bare `platform` in a template resolved to nothing and campfire's room
//! page died on `NameError: undefined local variable or method
//! 'platform' for module Views::Pwa`.
//!
//! There are two arms, and the discriminator already existed.
//! `controller_helper_method_names` clones a marked method CLASS-SIDE
//! when its body is pure over its arguments, and the view calls
//! `DomainsController.caption_of_button(domain)` — a static call, which
//! is right because there is no per-request state to reach. A marked
//! method that READS REQUEST STATE cannot be cloned for exactly that
//! reason, and used to be left as residue. Those route through the live
//! controller, the seam `flash` and `cookies` already use.
//!
//! Ingest also had to learn the CONCERN spelling: the existing scan
//! reads `helper_method` from a controller class body, and campfire
//! writes all three of its declarations inside a concern's `included
//! do`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::controller_to_library::{
    lower_controllers_with_arel_views_assocs_and_routes, LowerControllerOptions,
};
use roundhouse::ty::Ty;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

/// The campfire shape: declared in a CONCERN's `included do`, and the
/// body reads request state, so it cannot be cloned class-side.
const SET_PLATFORM: &str = r#"module SetPlatform
  extend ActiveSupport::Concern

  included do
    helper_method :platform
  end

  def platform
    @platform ||= request.user_agent
  end
end
"#;

/// The lobsters shape: declared in the controller body, ARG-PURE, so it
/// keeps its class-side clone.
const CONTROLLER: &str = r#"class RoomsController < ApplicationController
  include SetPlatform

  helper_method :caption_of

  def show
    @room = Room.find(params[:id])
  end

  def caption_of(room)
    "Room #{room.name}"
  end
end
"#;

fn app(view: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
        ("app/views/rooms/show.html.erb", view),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest")
}

fn view_body(view: &str) -> String {
    let app = app(view);
    let files = ruby::emit_lowered_views(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms/show.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted rooms/show; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

/// The gap this closes: a marked method whose body reads request state.
/// It cannot be cloned class-side, so it routes through the live
/// controller.
#[test]
fn a_request_reading_helper_method_routes_through_the_controller() {
    let body = view_body("<p><%= platform %></p>\n");
    assert!(
        body.contains("ActionController::Current.controller.platform"),
        "bare `platform` must reach the controller instance:\n{body}"
    );
}

/// …and the declaration is read from the CONCERN, which is where
/// campfire writes all three of its `helper_method` calls.
#[test]
fn the_declaration_is_read_from_a_concerns_included_block() {
    let app = app("<p><%= platform %></p>\n");
    assert!(
        app.view_visible_controller_methods
            .iter()
            .any(|m| m.as_str() == "platform"),
        "`helper_method :platform` inside `included do` must register: {:?}",
        app.view_visible_controller_methods
    );
}

/// The arm that already worked must not be taken. An ARG-PURE marked
/// method has a class-side clone, and rewriting it to a dynamic
/// `Current.controller` call is a regression — it is how this change
/// first broke lobsters' `DomainsController.caption_of_button`.
#[test]
fn an_arg_pure_helper_method_keeps_its_static_call() {
    let body = view_body("<p><%= caption_of(room) %></p>\n");
    assert!(
        !body.contains("ActionController::Current.controller.caption_of"),
        "the class-side clone serves this one:\n{body}"
    );
    assert!(
        body.contains("RoomsController.caption_of"),
        "and the view calls it statically:\n{body}"
    );
}

/// A template local of the same name IS that local — Rails mixes
/// helpers BENEATH a template's locals, and the emitted view takes them
/// as parameters. Without the guard, a partial declaring `platform:`
/// would ignore what its caller passed and read the controller.
#[test]
fn a_template_local_of_the_same_name_shadows_the_routing() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
        (
            "app/views/rooms/_badge.html.erb",
            "<%# locals: (platform:) %>\n<p><%= platform %></p>\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest");
    let files = ruby::emit_lowered_views(&app);
    let badge = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms/_badge.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted _badge; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        });
    assert!(
        badge.contains("def self.badge(platform)"),
        "the local is a parameter:\n{badge}"
    );
    assert!(
        !badge.contains("ActionController::Current.controller.platform"),
        "and the parameter wins over the helper_method:\n{badge}"
    );
    let mut app = app;
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let views = roundhouse::lower::view_to_library::lower_views_to_library_classes_with_controller_helpers(
        &app.views, &app, Vec::new(), &app.view_visible_controller_methods,
    );
    let method = views.iter().flat_map(|lc| &lc.methods)
        .find(|method| method.name.as_str() == "badge").expect("strict-local partial");
    assert_eq!(method.params.iter().filter(|param| param.name.as_str() == "platform").count(), 1,
        "the caller-provided strict local is the sole binding");
}

/// `params` in a view is the same seam one name over — a bare reference
/// that resolved to nothing. lobsters' password-reset page read
/// `params[:token]` from a module function that never declared it.
#[test]
fn params_in_a_view_reaches_the_controller() {
    let body = view_body("<p><%= params[:id] %></p>\n");
    assert!(
        body.contains("ActionController::Current.controller.params"),
        "bare `params` must reach the controller instance:\n{body}"
    );
}

/// …and the read must keep the STRING key the store is keyed by:
/// `@params` is a plain `Hash[String, ParamValue]`, so a Symbol key
/// finds nothing — campfire's join form rendered `action="/join/"`
/// because the view's `params[:join_code]` read nil through this seam
/// while the controller's own `@params["join_code"]` guard passed.
/// The grounding stamps the chain's type so the emitter's string-key
/// coercion fires exactly as it does inside a controller body.
#[test]
fn a_view_params_read_coerces_its_symbol_key() {
    let body = view_body("<p><%= params[:id] %></p>\n");
    assert!(
        body.contains("params[\"id\"]"),
        "the symbol key must coerce to the string the store is keyed by:\n{body}"
    );
    assert!(
        !body.contains("params[:id]"),
        "no symbol key may survive the grounding:\n{body}"
    );
}

fn analyzed_app(view: &str) -> roundhouse::App {
    let mut app = app(view);
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn rust_view_signature_takes_typed_helper_value_and_rewrites_the_read() {
    let app = analyzed_app("<p><%= platform %></p>\n");
    let views = roundhouse::lower::view_to_library::lower_views_to_library_classes_with_controller_helpers(
        &app.views, &app, Vec::new(), &app.view_visible_controller_methods,
    );
    let method = views.iter().flat_map(|lc| &lc.methods)
        .find(|method| method.name.as_str() == "show").expect("show view method");
    assert!(method.params.iter().any(|param| param.name.as_str() == "platform"));
    let Ty::Fn { params, .. } = method.signature.as_ref().expect("typed signature") else {
        panic!("view signature must be a function")
    };
    assert!(params.iter().any(|param| param.name.as_str() == "platform" && param.ty == Ty::Str));
    fn has_bare_platform(expr: &roundhouse::expr::Expr) -> bool {
        let mut found = matches!(&*expr.node, roundhouse::expr::ExprNode::Send {
            recv: None, method, args, ..
        } if method.as_str() == "platform" && args.is_empty());
        expr.node.for_each_child(&mut |child| found |= has_bare_platform(child));
        found
    }
    assert!(!has_bare_platform(&method.body));
}

#[test]
fn controller_render_passes_helper_value_from_its_own_instance() {
    let app = analyzed_app("<p><%= platform %></p>\n");
    let lcs = lower_controllers_with_arel_views_assocs_and_routes(
        &app.controllers,
        Vec::new(),
        LowerControllerOptions {
            views: &app.views,
            view_visible_controller_methods: Some(&app.view_visible_controller_methods),
            ..Default::default()
        },
    );
    let controller = lcs.iter().find(|lc| lc.name.0.as_str() == "RoomsController")
        .expect("RoomsController lowered");
    fn helper_call(expr: &roundhouse::expr::Expr) -> bool {
        let mut found = matches!(&*expr.node, roundhouse::expr::ExprNode::Send {
            recv: Some(receiver), method, args, ..
        } if method.as_str() == "platform" && args.is_empty()
            && matches!(&*receiver.node, roundhouse::expr::ExprNode::SelfRef));
        expr.node.for_each_child(&mut |child| found |= helper_call(child));
        found
    }
    let action = controller.methods.iter().find(|method| method.name.as_str() == "show")
        .expect("show action");
    assert!(helper_call(&action.body), "helper must be evaluated on controller self");
}

#[test]
fn inherited_controller_helper_is_evaluated_on_the_child_instance() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include SetPlatform\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def show; end\nend\n",
        ),
        ("app/views/rooms/show.html.erb", "<p><%= platform %></p>\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest");
    let mut app = app;
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    assert!(
        app.view_visible_controller_methods
            .iter()
            .any(|name| name.as_str() == "platform"),
        "inherited concern helper should be visible to views"
    );
    let controllers = lower_controllers_with_arel_views_assocs_and_routes(
        &app.controllers,
        Vec::new(),
        LowerControllerOptions {
            views: &app.views,
            view_visible_controller_methods: Some(&app.view_visible_controller_methods),
            ..Default::default()
        },
    );
    let rooms = controllers
        .iter()
        .find(|lc| lc.name.0.as_str() == "RoomsController")
        .expect("RoomsController lowered");
    let platform = rooms
        .methods
        .iter()
        .find(|method| {
            method.name.as_str() == "platform"
                && method.receiver == roundhouse::dialect::MethodReceiver::Instance
        })
        .expect("inherited platform helper is flattened onto the Rust controller");
    fn reads_platform_ivar(expr: &roundhouse::expr::Expr) -> bool {
        let mut found = matches!(
            &*expr.node,
            roundhouse::expr::ExprNode::OpAssign {
                target: roundhouse::expr::LValue::Ivar { name },
                ..
            } if name.as_str() == "platform"
        );
        expr.node
            .for_each_child(&mut |child| found |= reads_platform_ivar(child));
        found
    }
    assert!(
        reads_platform_ivar(&platform.body),
        "the inherited implementation must retain its per-controller memoized state: {:?}",
        platform.body
    );
    let show = rooms
        .methods
        .iter()
        .find(|method| method.name.as_str() == "show")
        .expect("show action");
    fn has_self_platform_call(expr: &roundhouse::expr::Expr) -> bool {
        let mut found = matches!(&*expr.node, roundhouse::expr::ExprNode::Send {
            recv: Some(receiver), method, args, ..
        } if method.as_str() == "platform"
            && args.is_empty()
            && matches!(&*receiver.node, roundhouse::expr::ExprNode::SelfRef));
        expr.node
            .for_each_child(&mut |child| found |= has_self_platform_call(child));
        found
    }
    assert!(
        has_self_platform_call(&show.body),
        "view render must dispatch inherited helper on the live child instance: {:?}",
        show.body
    );
}

#[test]
fn controller_helper_values_are_threaded_through_partial_signatures() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        ("app/controllers/rooms_controller.rb", "class RoomsController < ApplicationController\n  include SetPlatform\n  def show; end\nend\n"),
        ("app/views/rooms/show.html.erb", r#"<%= render "rooms/badge" %>
"#),
        ("app/views/rooms/_badge.html.erb", "<p><%= platform %></p>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms\nend\n"),
    ])).expect("ingest");
    let mut app = app;
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let views = roundhouse::lower::view_to_library::lower_views_to_library_classes_with_controller_helpers(
        &app.views, &app, Vec::new(), &app.view_visible_controller_methods,
    );
    let mut methods = views.iter().flat_map(|lc| &lc.methods);
    let action = methods.clone().find(|method| method.name.as_str() == "show").expect("action view");
    let partial = methods.find(|method| method.name.as_str() == "badge").expect("partial view");
    for method in [action, partial] {
        assert!(method.params.iter().any(|param| param.name.as_str() == "platform"));
    }
    fn forwards_platform(expr: &roundhouse::expr::Expr) -> bool {
        let mut found = matches!(&*expr.node, roundhouse::expr::ExprNode::Send {
            recv: Some(_), method, args, ..
        } if method.as_str() == "badge" && args.iter().any(|arg| matches!(
            &*arg.node, roundhouse::expr::ExprNode::Var { name, .. } if name.as_str() == "platform"
        )));
        expr.node.for_each_child(&mut |child| found |= forwards_platform(child));
        found
    }
    assert!(forwards_platform(&action.body));
}
