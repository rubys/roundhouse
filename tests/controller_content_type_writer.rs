use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn analyze(files: &[(&str, &str)]) -> roundhouse::App {
    let tree = files.iter().map(|(name, source)| ((*name).into(), source.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn result(app: &roundhouse::App, controller: &str, method: &str) -> Ty {
    app.controllers.iter().find(|c| c.name.0.as_str() == controller).unwrap()
        .actions().find(|a| a.name.as_str() == method).unwrap().body.ty.clone().unwrap()
}

#[test]
fn controller_writer_preserves_literal_nil_and_gradual_assignment_values() {
    let app = analyze(&[("app/controllers/probes_controller.rb", r#"
class ProbesController < ActionController::Base
  def literal = (self.content_type = "video/mp4")
  def unset = (self.content_type = nil)
  def gradual(value) = (self.content_type = value)
  def pending(value) = (self.content_type = value)
end
"#), ("sig/probes_controller.rbs", "class ProbesController\ndef gradual: (untyped value) -> untyped\nend\n")]);
    assert_eq!(result(&app, "ProbesController", "literal"), Ty::Str);
    assert_eq!(result(&app, "ProbesController", "unset"), Ty::Nil);
    assert_eq!(result(&app, "ProbesController", "gradual"), Ty::Untyped);
    assert!(matches!(result(&app, "ProbesController", "pending"), Ty::Var { .. }));
    let errors: Vec<_> = diagnose(&app).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn concern_writer_resolves_for_each_controller_includer() {
    let app = analyze(&[
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/concerns/media_headers.rb", r#"
module MediaHeaders
  def send_media(value:)
    self.content_type = value
    self.content_type = value
  end
end
"#),
        ("app/controllers/alphas_controller.rb", "class AlphasController < ApplicationController\ninclude MediaHeaders\ndef show = send_media(value: \"video/mp4\")\nend\n"),
        ("app/controllers/betas_controller.rb", "class BetasController < ApplicationController\ninclude MediaHeaders\ndef show = send_media(value: nil)\nend\n"),
    ]);
    let errors: Vec<_> = diagnose(&app).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_missing_writer_and_a_non_controller_are_still_rejected() {
    let app = analyze(&[
        ("app/controllers/probes_controller.rb", r#"
class ProbesController < ActionController::Base
  def missing(value) = (self.absent_writer = value)
  def unrelated(value) = (Plain.new.content_type = value)
  def literal_unrelated = (Plain.new.content_type = "text/plain")
  def union_unrelated(choice) = ((choice ? self : Plain.new).content_type = "text/plain")
  def class_receiver = (ActionController::Base.content_type = "text/plain")
end
"#),
        ("app/models/plain.rb", "class Plain\ndef initialize\nend\nend\n"),
    ]);
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Plain"));
    let errors: Vec<_> = diagnose(&app).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.iter().any(|d| d.message.contains("`absent_writer=`")), "{errors:?}");
    assert!(errors.iter().any(|d| d.message.contains("`content_type=`")), "{errors:?}");
    assert_eq!(errors.iter().filter(|d| d.message.contains("`content_type=`")).count(), 4, "{errors:?}");
}

#[test]
fn a_source_writer_override_retains_ruby_assignment_semantics() {
    let app = analyze(&[("app/controllers/probes_controller.rb", r#"
class ProbesController < ActionController::Base
  def content_type=(value)
    123
  end
  def literal = (self.content_type = "video/mp4")
  def unset = (self.content_type = nil)
  def gradual(value) = (self.content_type = value)
end
"#), ("sig/probes_controller.rbs", "class ProbesController\ndef gradual: (untyped value) -> untyped\nend\n")]);
    assert_eq!(result(&app, "ProbesController", "literal"), Ty::Str);
    assert_eq!(result(&app, "ProbesController", "unset"), Ty::Nil);
    assert_eq!(result(&app, "ProbesController", "content_type="), Ty::Int);
    assert_eq!(result(&app, "ProbesController", "gradual"), Ty::Untyped);
}

#[test]
fn an_unresolved_assignment_value_is_not_changed_into_gradual_or_string() {
    use roundhouse::analyze::{BodyTyper, Ctx, ClassInfo};
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ident::{ClassId, Symbol, TyVar, VarId};
    use roundhouse::span::Span;
    let id = ClassId(Symbol::from("ActionController::Base"));
    let mut expression = Expr::new(Span::synthetic(), ExprNode::Send {
        recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
        method: Symbol::from("content_type="),
        args: vec![Expr::new(Span::synthetic(), ExprNode::Var { name: Symbol::from("value"),
            id: VarId(0) })], block: None, parenthesized: false,
    });
    for writer_result in [Ty::Untyped, Ty::Var { var: TyVar(13) }] {
        let mut info = ClassInfo::default();
        info.instance_methods.insert(Symbol::from("content_type="), writer_result);
        let classes = [(id.clone(), info)].into_iter().collect();
        for value in [Ty::Var { var: TyVar(42) }, Ty::Untyped, Ty::Str, Ty::Nil] {
            let mut context = Ctx { self_ty: Some(Ty::Class { id: id.clone(), args: vec![].into() }),
                instance_body: true, ..Ctx::default() };
            context.local_bindings.insert(Symbol::from("value"), value.clone());
            assert_eq!(BodyTyper::new(&classes).analyze_expr(&mut expression, &context), value);
            assert_eq!(expression.diagnostic().is_some(), matches!(value, Ty::Var { .. }));
        }
    }
}

#[test]
fn strict_targets_refuse_the_framework_writer_without_refusing_an_app_override() {
    use roundhouse::project::{BuildTarget, target_files};
    for (source, refused) in [
        ("class ProbesController < ActionController::Base\ndef show\nself.content_type = \"video/mp4\"\nself.response_body = \"body\"\nend\nend\n", true),
        ("class ProbesController < ActionController::Base\ndef content_type=(value)\n123\nend\ndef show\nself.content_type = \"video/mp4\"\nself.response_body = \"body\"\nend\nend\n", false),
    ] {
        let tree = [("app/controllers/probes_controller.rb".into(), source.as_bytes().to_vec())].into_iter().collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        let (_files, diagnostics) = roundhouse::emit::diagnostics::scope(||
            target_files(&app, std::path::Path::new("."), BuildTarget::Rust));
        assert_eq!(diagnostics.iter().any(|d| d.message.contains("controller writer's charset and nil behavior")),
            refused, "{diagnostics:?}");
    }
}
