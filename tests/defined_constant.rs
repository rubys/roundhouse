//! `defined?(Constant)` and `defined?(A::B)` are non-evaluating guards.
//! Ingest retains the written path and does not resolve or autoload it.
//! A missing constant is not an ingest failure.

use roundhouse::analyze::Analyzer;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::ty::Ty;
use roundhouse::App;

fn method_body(source: &str) -> roundhouse::Expr {
    let classes = ingest_library_classes(source.as_bytes(), "guard.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes = classes;
    Analyzer::new(&app).analyze(&mut app);
    app.library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "Probe")
        .expect("Probe")
        .methods[0]
        .body
        .clone()
}

fn defined_operand(body: &roundhouse::Expr) -> &roundhouse::Expr {
    fn find<'a>(expr: &'a roundhouse::Expr) -> Option<&'a roundhouse::Expr> {
        if let ExprNode::Defined { operand } = &*expr.node {
            return Some(operand);
        }
        if let ExprNode::Send { recv: None, method, args, .. } = &*expr.node {
            if method.as_str() == "defined?" && args.len() == 1 {
                return Some(&args[0]);
            }
        }
        let mut found = None;
        expr.node.for_each_child(&mut |child| {
            if found.is_none() {
                found = find(child);
            }
        });
        found
    }
    find(body).expect("defined? marker")
}

#[test]
fn constant_guards_keep_the_written_path_and_are_not_resolved() {
    let cases = [
        ("defined?(Sentry)", vec!["Sentry"], false),
        ("defined?(RubyLLM::PaymentRequiredError)", vec!["RubyLLM", "PaymentRequiredError"], false),
        ("defined?(File::NOFOLLOW)", vec!["File", "NOFOLLOW"], false),
        ("defined?(::File)", vec!["", "File"], true),
        ("defined?(A::B::C)", vec!["A", "B", "C"], false),
        ("defined?(MissingConstant)", vec!["MissingConstant"], false),
    ];
    for (guard, path, rooted) in cases {
        let source = format!("class File\n  NOFOLLOW = 0\nend\nclass Probe\n  def check\n    {guard}\n  end\nend\n");
        let body = method_body(&source);
        let operand = defined_operand(&body);
        assert!(matches!(&*body.node, ExprNode::Defined { .. }), "{guard}");
        let ExprNode::Const { path: written } = &*operand.node else {
            panic!("{guard} did not retain a constant path: {operand:?}");
        };
        let written: Vec<_> = written.iter().map(|s| s.as_str()).collect();
        assert_eq!(written, path, "{guard}");
        assert_eq!(written[0].is_empty(), rooted, "{guard}");
        match &body.ty {
            Some(Ty::Union { variants }) => {
                assert!(variants.iter().any(|ty| matches!(ty, Ty::Str)), "{guard}: {variants:?}");
                assert!(variants.iter().any(|ty| matches!(ty, Ty::Nil)), "{guard}: {variants:?}");
            }
            other => panic!("{guard} must type as the existing defined? result Str?, got {other:?}"),
        }
        assert_eq!(operand.ty, None, "{guard}: syntax operands must not be evaluated");
        assert_eq!(operand.decisions & roundhouse::expr::RESOLVED_CLASS_REF, 0, "{guard}");
        assert_eq!(roundhouse::emit::ruby::emit_expr(&body), guard, "{guard}");
    }
}

#[test]
fn existing_bareword_and_ivar_guards_still_ingest() {
    for guard in ["defined?(maybe_local)", "defined?(@parsed_url)"] {
        let source = format!("class Probe\n  def check\n    {guard}\n  end\nend\n");
        let body = method_body(&source);
        let operand = defined_operand(&body);
        assert!(
            matches!(&*operand.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }),
            "{guard}: {operand:?}"
        );
    }
}

#[test]
fn argument_free_method_queries_keep_native_syntax() {
    let body = method_body("class Probe; def check; defined?(obj.method); end; end");
    assert!(matches!(&*body.node, ExprNode::Defined { .. }));
    assert_eq!(roundhouse::emit::ruby::emit_expr(&body), "defined?(obj.method)");
    assert_eq!(defined_operand(&body).ty, None);
}

#[test]
fn argument_and_index_queries_stay_unsupported() {
    for guard in ["defined?(obj.method(11))", "defined?(foo[0])"] {
        let source = format!("class Probe\n  def check\n    {guard}\n  end\nend\n");
        let err = ingest_library_classes(source.as_bytes(), "guard.rb").expect_err(guard);
        let message = err.to_string();
        assert!(message.contains("defined? calls with arguments"), "{guard}: {message}");
    }
}
