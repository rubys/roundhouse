//! Controller `helper_method :x` — the third helper channel (a method
//! on the controller exposed to templates). ARG-PURE marked methods
//! (no instance state) register in `helper_method_index` at ingest, the
//! controller lowering adds a class-side clone, and the bare view call
//! rewrites to `<Controller>.x(args)` like any app helper. Marked
//! methods that touch instance state stay instance-only. Surfaced by
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

/// Instance state reaches a helper by more routes than an ivar read:
/// a memoizing write, a call to another method of the controller that
/// reads the request, or a call this controller cannot resolve. Each
/// keeps the helper on the instance. A helper whose callees are all
/// pure is cloned together with them, so the clone's bare call finds a
/// class method.
#[test]
fn only_helpers_free_of_instance_state_clone_class_side_with_their_callees() {
    let tree = [(
        "app/controllers/accounts_controller.rb",
        r##"class AccountsController < ApplicationController
  helper_method :memo_account, :slug_account, :inherited_account, :greeting

  def memo_account
    @memo_account ||= Account.find_by(slug: Current.slug)
  end

  def slug_account
    Account.where(slug: current_slug).first if current_slug
  end

  def inherited_account
    Account.find_by(id: current_user_id)
  end

  def greeting(name)
    "#{self.salutation}, #{name}"
  end

  private

  def current_slug
    params[:slug]
  end

  def salutation
    words.first
  end

  def words
    %w[hello hi]
  end
end
"##,
    )]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");

    let registered: Vec<&str> = ["memo_account", "slug_account", "inherited_account", "greeting"]
        .into_iter()
        .filter(|name| app.helper_method_index.contains_key(&Symbol::from(*name)))
        .collect();
    assert_eq!(registered, ["greeting"], "{:?}", app.helper_method_index);

    let lcs = roundhouse::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        Vec::new(),
        Some(&app.schema),
        &app.views,
    );
    let ctrl = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "AccountsController")
        .expect("controller lowered");
    let mut class_side: Vec<&str> = ctrl
        .methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .map(|m| m.name.as_str())
        .filter(|name| !name.starts_with("__"))
        .collect();
    class_side.sort_unstable();
    assert_eq!(class_side, ["greeting", "salutation", "words"]);
}

#[test]
fn helper_class_clone_is_typed_and_rewrites_permitted_fields() {
    use roundhouse::expr::ExprNode;
    use roundhouse::lower::controller_to_library::{
        LowerControllerOptions, lower_controllers_with_arel_views_assocs_and_routes,
    };
    use roundhouse::ty::Ty;

    let tree = [(
        "app/controllers/articles_controller.rb",
        r#"class ArticlesController < ApplicationController
  helper_method :caption
  def caption(permitted)
    permitted[:title]
  end
  private
  def article_params
    params.require(:article).permit(:title, :body)
  end
end
"#,
    )].into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).unwrap();
    let params_ty = Ty::Class { id: ClassId(Symbol::from("ArticleParams")), args: vec![].into() };
    let inferred = [((ClassId(Symbol::from("ArticlesController")), Symbol::from("caption")), vec![params_ty.clone()])]
        .into_iter().collect();
    let routed = roundhouse::det::HashMap::default();
    let classes = lower_controllers_with_arel_views_assocs_and_routes(
        &app.controllers,
        Vec::new(),
        LowerControllerOptions {
            inferred_params: Some(&inferred),
            routed_by_controller: Some(&routed),
            ..Default::default()
        },
    );
    let controller = classes.iter().find(|lc| lc.name.0.as_str() == "ArticlesController").unwrap();
    for receiver in [MethodReceiver::Instance, MethodReceiver::Class] {
        let method = controller.methods.iter()
            .find(|m| m.name.as_str() == "caption" && m.receiver == receiver).unwrap();
        let tail = match &*method.body.node {
            ExprNode::Seq { exprs } => exprs.last().unwrap(),
            _ => &method.body,
        };
        assert!(matches!(&*tail.node, ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "title" && args.is_empty() && recv.ty.as_ref() == Some(&params_ty)),
            "{receiver:?} helper missed typing or bracket rewriting: {tail:?}");
        assert_eq!(tail.ty, Some(Ty::Str), "{receiver:?} accessor return type");
    }
}

/// Ingest judges a helper on the controller as written, before its
/// concerns are spliced in; the lowering judges it on the controller
/// they produce. Here the concern's `class_attribute :salutation` gives
/// the controller a class-side `salutation`, so a clone of `greeting`
/// would reach that one rather than the private instance method the
/// original calls, and the lowering keeps `greeting` on the instance.
/// The index has to agree, or the view calls a
/// `GreetingsController.greeting` nobody defined.
#[test]
fn a_helper_a_concern_makes_impure_leaves_the_index_with_its_clone() {
    let tree = [
        (
            "app/controllers/concerns/salutations.rb",
            r#"module Salutations
  extend ActiveSupport::Concern

  included do
    class_attribute :salutation, default: "hello"
  end
end
"#,
        ),
        (
            "app/controllers/greetings_controller.rb",
            r##"class GreetingsController < ApplicationController
  include Salutations

  helper_method :greeting

  def show
  end

  def greeting(name)
    "#{salutation}, #{name}"
  end

  private

  def salutation
    "hi"
  end
end
"##,
        ),
        ("app/views/greetings/show.html.erb", "<p><%= greeting(\"ada\") %></p>\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"greeting\", to: \"greetings#show\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");

    assert!(
        !app.helper_method_index.contains_key(&Symbol::from("greeting")),
        "{:?}",
        app.helper_method_index
    );

    let lcs = roundhouse::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        Vec::new(),
        Some(&app.schema),
        &app.views,
    );
    let ctrl = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "GreetingsController")
        .expect("controller lowered");
    assert!(
        !ctrl
            .methods
            .iter()
            .any(|m| m.name.as_str() == "greeting" && m.receiver == MethodReceiver::Class)
    );

    let files = roundhouse::emit::ruby::emit_lowered_views(&app);
    let view = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("greetings/show.rb"))
        .expect("greetings/show emitted");
    assert!(
        view.content.contains("ActionController::Current.controller.greeting(\"ada\")"),
        "{}",
        view.content
    );
    assert!(!view.content.contains("GreetingsController.greeting"), "{}", view.content);
}
