//! `X and return` / `X or return` grounding (`lower::apply_and_return_lowering`).

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_and_return_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> String {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_and_return_lowering(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_response_call_and_return_always_returns() {
    let out = lower_and_emit(
        r#"
class DraftsController
  def check(stop)
    head(:no_content) and return if stop
    render(plain: "x")
  end
end
"#,
    );
    assert!(!out.contains("if head("), "the response call is not a condition:\n{out}");
    assert!(out.contains("head(:no_content)\n"), "{out}");
    assert!(out.contains("return"), "{out}");
}

#[test]
fn a_response_call_or_return_never_returns() {
    let out = lower_and_emit(
        r#"
class DraftsController
  def check
    redirect_to("/") or return
    @after = true
  end
end
"#,
    );
    assert!(!out.contains("return"), "the return is dropped:\n{out}");
    assert!(out.contains("redirect_to(\"/\")"), "{out}");
}

#[test]
fn another_call_and_return_keeps_its_condition() {
    let out = lower_and_emit(
        r#"
class DraftsController
  def check(record)
    record.save and return
    @after = true
  end
end
"#,
    );
    assert!(out.contains("if record.save"), "{out}");
}
