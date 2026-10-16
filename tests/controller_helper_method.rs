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
    let routed = std::collections::HashMap::new();
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
