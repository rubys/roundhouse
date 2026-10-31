//! View→helper param unification: a bare helper call in a template
//! (`shout("hi")`) is dispatch to the helper module Rails mixes into
//! every view, so its arg types are call-site evidence for the
//! helper's params. The fixpoint unifies them, and the post-fixpoint
//! stamp writes the discovered signature onto the helper `MethodDef`
//! so the emitted RBS carries it.

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::View;
use roundhouse::expr::{Expr, ExprNode, Literal};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_library_classes;
use roundhouse::span::Span;
use roundhouse::ty::{Row, Ty};
use roundhouse::App;

fn helper_app(view_body: Expr) -> App {
    let mut app = App::new();
    let classes = ingest_library_classes(
        b"module ApplicationHelper\n  def shout(msg)\n    msg\n  end\nend\n",
        "app/helpers/application_helper.rb",
    )
    .expect("ingest helper");
    app.library_classes.extend(classes);
    app.helper_method_index
        .insert(Symbol::from("shout"), ClassId(Symbol::from("ApplicationHelper")));
    app.views.push(View {
        name: Symbol::from("posts/index"),
        format: Symbol::from("html"),
        locals: Row::default(),
        body: view_body,
        strict_locals: None,
        analysis_only: false,
        jbuilder: false,
    });
    app
}

fn bare_call(method: &str, arg: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(method),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    )
}

fn str_lit(value: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: value.into() } },
    )
}

/// An app ingested from `(path, source)` files and analyzed.
fn analyzed_app(files: &[(&str, &str)]) -> App {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::{collections::HashMap, path::PathBuf};
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::for_source(&app).analyze(&mut app);
    app
}

/// What a module method's callers read: the stamped signature's return,
/// else the body's type.
fn returns(app: &App, module: &str, method: &str, class_side: bool) -> Option<Ty> {
    use roundhouse::dialect::MethodReceiver;
    let side = if class_side { MethodReceiver::Class } else { MethodReceiver::Instance };
    let m = app
        .library_classes
        .iter()
        .filter(|lc| lc.name.0.as_str() == module)
        .flat_map(|lc| lc.methods.iter())
        .find(|m| m.name.as_str() == method && m.receiver == side)
        .unwrap_or_else(|| panic!("{module}#{method}"));
    match &m.signature {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => m.body.ty.clone(),
    }
}

/// Unresolved or gradual: what such a body answered before the view
/// fallback.
fn unresolved(ty: &Option<Ty>) -> bool {
    matches!(ty, None | Some(Ty::Var { .. }) | Some(Ty::Untyped))
}

#[test]
fn a_helper_body_calls_actionview_helpers_with_the_view_as_self() {
    // Rails mixes app/helpers into the view, so a helper's implicit self is
    // the view: `tag.span`, `image_tag` and `link_to` are ActionView's and
    // render a String (SafeBuffer). The body was typed with the module as
    // self, where none of them exist, and every such helper was untyped.
    let app = analyzed_app(&[(
        "app/helpers/badges_helper.rb",
        "module BadgesHelper\n\
         \x20 def badge(text)\n    tag.span(text, class: \"badge\")\n  end\n\
         \x20 def logo\n    image_tag(\"logo.png\", alt: \"Logo\")\n  end\n\
         \x20 def home_link\n    link_to \"Home\", \"/\"\n  end\n\
         \x20 def boxed\n    tag.div(class: \"box\") { badge(\"x\") }\n  end\n\
         end\n",
    )]);
    for m in ["badge", "logo", "home_link", "boxed"] {
        assert_eq!(returns(&app, "BadgesHelper", m, false), Some(Ty::Str), "BadgesHelper#{m}");
    }
}

#[test]
fn a_helper_calls_another_helper_modules_method() {
    // `helpers :all`: every helper module is in the same view, so one helper
    // reaches another module's method by implicit self.
    let app = analyzed_app(&[
        ("app/helpers/counts_helper.rb", "module CountsHelper\n  def page_size\n    50\n  end\nend\n"),
        ("app/helpers/rooms_helper.rb", "module RoomsHelper\n  def rooms_per_page\n    page_size\n  end\nend\n"),
    ]);
    assert_eq!(returns(&app, "RoomsHelper", "rooms_per_page", false), Some(Ty::Int));
}

#[test]
fn a_helper_that_defines_the_name_keeps_its_own_answer() {
    // The app's definition answers before ActionView's, as Rails' method
    // lookup does, also while it is still untyped (it is in no method table
    // yet), and also when it comes from a module the helper includes.
    let app = analyzed_app(&[
        (
            "app/helpers/links_helper.rb",
            "module LinksHelper\n\
             \x20 def link_to(name, path)\n    37\n  end\n\
             \x20 def home\n    link_to \"Home\", \"/\"\n  end\n\
             end\n",
        ),
        (
            "app/helpers/images_helper.rb",
            "module ImagesHelper\n\
             \x20 def image_tag(source)\n    Gallery.render(source)\n  end\n\
             \x20 def logo\n    image_tag(\"logo.png\")\n  end\n\
             end\n",
        ),
        ("lib/linking.rb", "module Linking\n  def button_to(name, path)\n    Gallery.render(name)\n  end\nend\n"),
        (
            "app/helpers/nav_helper.rb",
            "module NavHelper\n  include Linking\n\n  def nav\n    button_to \"Home\", \"/\"\n  end\nend\n",
        ),
    ]);
    assert_eq!(returns(&app, "LinksHelper", "home", false), Some(Ty::Int));
    let logo = returns(&app, "ImagesHelper", "logo", false);
    assert!(unresolved(&logo), "the app's image_tag, not ActionView's: {logo:?}");
    let nav = returns(&app, "NavHelper", "nav", false);
    assert!(unresolved(&nav), "the included module's button_to, not ActionView's: {nav:?}");
}

#[test]
fn only_a_helper_instance_body_runs_in_the_view() {
    // A helper module's `def self.x` runs on the module, not in a view, and a
    // module outside app/helpers is not mixed into the view: neither reaches
    // ActionView's `link_to`.
    let app = analyzed_app(&[
        (
            "app/helpers/banners_helper.rb",
            "module BannersHelper\n\
             \x20 def self.banner\n    link_to \"Home\", \"/\"\n  end\n\
             \x20 def inline\n    \"x\"\n  end\n\
             end\n",
        ),
        ("lib/widgets.rb", "module Widgets\n  def home_link\n    link_to \"Home\", \"/\"\n  end\nend\n"),
    ]);
    let banner = returns(&app, "BannersHelper", "banner", true);
    assert!(unresolved(&banner), "class-side helper: {banner:?}");
    let widget = returns(&app, "Widgets", "home_link", false);
    assert!(unresolved(&widget), "module outside app/helpers: {widget:?}");
    // And a helper module's `def self.image_tag` is no answer for another
    // helper's bare `image_tag`: the view's instance method is ActionView's.
    let app = analyzed_app(&[
        ("app/helpers/images_helper.rb", "module ImagesHelper\n  def self.image_tag(source)\n    37\n  end\nend\n"),
        ("app/helpers/logos_helper.rb", "module LogosHelper\n  def logo\n    image_tag(\"logo.png\")\n  end\nend\n"),
    ]);
    assert_eq!(returns(&app, "LogosHelper", "logo", false), Some(Ty::Str), "a singleton image_tag is not the view's");
    // Nor does a later module's `def self.badge` hide an earlier helper's
    // instance `badge`, though it took the name's entry in the index.
    let app = analyzed_app(&[
        ("app/helpers/badges_helper.rb", "module BadgesHelper\n  def badge\n    37\n  end\nend\n"),
        ("app/helpers/logos_helper.rb", "module LogosHelper\n  def logo\n    badge\n  end\nend\n"),
        ("app/helpers/zebras_helper.rb", "module ZebrasHelper\n  def self.badge\n    \"z\"\n  end\nend\n"),
    ]);
    assert_eq!(returns(&app, "LogosHelper", "logo", false), Some(Ty::Int), "the instance badge answers");
}

#[test]
fn view_call_sites_unify_helper_params_and_stamp_signature() {
    let mut app = helper_app(bare_call("shout", str_lit("hi")));
    Analyzer::new(&app).analyze(&mut app);

    let helper = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "ApplicationHelper")
        .expect("helper LC");
    let shout = helper
        .methods
        .iter()
        .find(|m| m.name.as_str() == "shout")
        .expect("shout method");
    let Some(Ty::Fn { params, ret, .. }) = &shout.signature else {
        panic!("expected stamped Fn signature, got {:?}", shout.signature);
    };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].ty, Ty::Str.into(), "param should unify to Str from the call site");
    // Body is `msg` — the param read — so the return follows the seed.
    assert_eq!(**ret, Ty::Str);
    // Views are typed once before that harvest; they must be restamped
    // so the template Send is Str, not leftover gradual untyped.
    match &*app.views[0].body.node {
        ExprNode::Send { method, .. } if method.as_str() == "shout" => {
            assert_eq!(
                app.views[0].body.ty.as_ref(),
                Some(&Ty::Str),
                "view helper call should restamp as the harvested return, got {:?}",
                app.views[0].body.ty
            );
        }
        other => panic!("expected shout Send, got {other:?}"),
    }
}

#[test]
fn untyped_view_args_are_no_evidence_not_absorption() {
    // Two call sites: one typed Str, one whose arg the view typer
    // leaves untyped (an ivar with no controller seed dispatched
    // through an untyped read). The untyped site must not absorb the
    // union — the param stays Str.
    let typed = bare_call("shout", str_lit("hi"));
    let untyped_arg = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: Symbol::from("mystery") },
            )),
            method: Symbol::from("value"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    let both = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![typed, bare_call("shout", untyped_arg)] },
    );
    let mut app = helper_app(both);
    Analyzer::new(&app).analyze(&mut app);

    let helper = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "ApplicationHelper")
        .expect("helper LC");
    let shout = helper
        .methods
        .iter()
        .find(|m| m.name.as_str() == "shout")
        .expect("shout method");
    let Some(Ty::Fn { params, .. }) = &shout.signature else {
        panic!("expected stamped Fn signature, got {:?}", shout.signature);
    };
    assert_eq!(
        params[0].ty,
        Ty::Str.into(),
        "an untyped call site is no evidence; it must not widen the param: {:?}",
        params[0].ty
    );
}

#[test]
fn a_cataloged_helper_uses_the_selected_owner_not_another_helpers_signature() {
    use roundhouse::analyze::{diagnose, DiagnosticKind, Severity};
    use roundhouse::ingest::ingest_app_from_tree;
    use std::{collections::HashMap, path::PathBuf};

    // AHelper remains included through `marker`, but neither its instance
    // signature nor its singleton method answers the view's content_tag.
    // Without an RBS for ZHelper, its return must refine from the view arg.
    // An explicit sidecar for the view context still overrides inference.
    for (signature, view_signature, expected) in [
        ("", "", Ty::Int),
        ("module ZHelper\n  def content_tag: (Integer value) -> Integer\nend\n", "", Ty::Int),
        ("", "class ActionView::Base\n  def content_tag: (Integer value) -> String\nend\n", Ty::Str),
    ] {
        for operation in ["div(2)", "length"] {
            let view = format!("<%= content_tag(37).{operation} %>");
            let files = [
                ("app/helpers/a_helper.rb", "module AHelper\n  def content_tag(value)\n    \"wrong owner\"\n  end\n  def self.content_tag(value)\n    \"singleton\"\n  end\n  def marker\n    \"a\"\n  end\nend\n"),
                ("app/helpers/z_helper.rb", "module ZHelper\n  def content_tag(value)\n    value\n  end\nend\n"),
                ("sig/a_helper.rbs", "module AHelper\n  def content_tag: (Integer value) -> String\nend\n"),
                ("sig/z_helper.rbs", signature),
                ("sig/action_view/base.rbs", view_signature),
                ("app/views/articles/index.html.erb", view.as_str()),
            ];
            let tree: HashMap<PathBuf, Vec<u8>> = files.into_iter()
                .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
                .collect();
            let mut app = ingest_app_from_tree(tree).expect("ingest helpers");
            let mut analyzer = Analyzer::for_source(&app);
            analyzer.analyze(&mut app);
            let entry = &analyzer.class_registry()[&ClassId(Symbol::from("ActionView::Base"))]
                .instance_methods[&Symbol::from("content_tag")];
            let actual = match entry {
                Ty::Fn { ret, .. } => ret.as_ref(),
                ty => ty,
            };
            assert_eq!(actual, &expected);
            let errors: Vec<_> = diagnose(&app).into_iter()
                .filter(|d| d.severity == Severity::Error).collect();
            let invalid = (operation == "length") == (expected == Ty::Int);
            if invalid {
                assert!(errors.iter().any(|d| matches!(&d.kind,
                    DiagnosticKind::SendDispatchFailed { method, recv_ty }
                        if method.as_str() == operation.trim_end_matches("(2)") && recv_ty == &expected)),
                    "{expected:?} must not silently accept {operation}: {errors:?}");
            } else {
                assert!(errors.is_empty(), "valid {expected:?} operation rejected: {errors:?}");
            }
        }
    }
}

#[test]
fn a_singleton_only_helper_does_not_erase_framework_instance_typing() {
    use roundhouse::analyze::{diagnose, DiagnosticKind, Severity};
    use roundhouse::ingest::ingest_app_from_tree;

    for helper in [
        "module ApplicationHelper\n  def self.content_tag(name, text)\n    \"singleton\"\n  end\nend\n",
        "module ApplicationHelper\n  include TagDefaults\nend\n",
    ] {
        let tree = [
            ("app/helpers/application_helper.rb", helper),
            ("lib/tag_defaults.rb", "module TagDefaults\n  def self.content_tag(name, text)\n    \"singleton\"\n  end\nend\n"),
            ("app/views/articles/index.html.erb", "<%= content_tag(:span, \"text\").div(2) %>"),
        ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest helpers");
        let mut analyzer = Analyzer::for_source(&app);
        analyzer.analyze(&mut app);
        assert_eq!(
            analyzer.class_registry()[&ClassId(Symbol::from("ActionView::Base"))]
                .instance_methods[&Symbol::from("content_tag")],
            Ty::Str,
        );
        assert!(diagnose(&app).iter().any(|d| d.severity == Severity::Error && matches!(
            &d.kind, DiagnosticKind::SendDispatchFailed { method, recv_ty }
                if method.as_str() == "div" && recv_ty == &Ty::Str
        )), "a singleton definition must not turn String#div into a gradual escape");
    }
}
