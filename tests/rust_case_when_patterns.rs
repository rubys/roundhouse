//! Rust renders `case`/`when` as a `match`. Only literal (Str/Sym/Int/
//! Bool), binding and wildcard patterns have a `match` form; a range,
//! class, nil/float, or guarded `when` used to come out as `_`, so the
//! first such arm won for every input. Those shapes are reported
//! unsupported instead (fail-closed, same intent as Python/TS; Bind
//! stays for indexer symbol dispatch).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::project::BuildTarget;

/// real-blog with `Article.probe(x)` added; returns emitted `article.rs`,
/// error diagnostics, and the app (for span provenance).
fn emit_rust_with_probe(body: &str) -> (String, Vec<Diagnostic>, roundhouse::App) {
    let (emitted, app, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n  def self.probe(x)\n    {body}\n  end\n"),
        )
        .emit_with_app(BuildTarget::Rust);
    let article = std::fs::read_to_string(emitted.join("src/models/article.rs"))
        .expect("emitted src/models/article.rs");
    (article, errors, app)
}

fn case_rust_unsupported<'a>(errors: &'a [Diagnostic]) -> Vec<&'a Diagnostic> {
    errors
        .iter()
        .filter(|d| {
            d.severity == Severity::Error
                && matches!(
                    &d.kind,
                    DiagnosticKind::Unsupported { construct, target: Some(t), .. }
                        if construct.as_str() == "Case" && t.as_str() == "rust"
                )
        })
        .collect()
}

fn span_text(app: &roundhouse::App, d: &Diagnostic) -> String {
    let source = roundhouse::ide::source(app, d.span.file).expect("diagnostic has source");
    source.text[d.span.start as usize..d.span.end as usize].to_string()
}

#[test]
fn non_literal_when_patterns_are_unsupported_not_wildcards() {
    // Each body pairs with the arm the diagnostic must point at.
    for (body, failing) in [
        ("case x\n    when 0..3 then \"low\"\n    when 4..9 then \"high\"\n    else \"neg\"\n    end", "0..3"),
        ("case x\n    when 0...3 then \"low\"\n    else \"other\"\n    end", "0...3"),
        ("case x\n    when ..0 then \"nonpos\"\n    when 10.. then \"big\"\n    else \"mid\"\n    end", "..0"),
        ("case x\n    when String then \"s\"\n    when Integer then \"i\"\n    else \"o\"\n    end", "String"),
        ("case x\n    when 0 then \"zero\"\n    when 1..5 then \"few\"\n    else \"many\"\n    end", "1..5"),
        // Nil Lit has no pattern span → falls back to the scrutinee (`x`).
        // (Case `Arm.guard` is synthesizer-only; Ruby `when` does not carry it.)
        ("case x\n    when nil then \"n\"\n    else \"o\"\n    end", "x"),
    ] {
        let (article, errors, app) = emit_rust_with_probe(body);
        let case_errs = case_rust_unsupported(&errors);
        assert!(
            !case_errs.is_empty(),
            "rust accepted {body}:\n{article}\nall errors: {errors:?}"
        );
        assert_eq!(case_errs.len(), 1, "{body}: {case_errs:?}");
        let span = case_errs[0].span;
        assert!(!span.is_synthetic(), "lost source span");
        assert_eq!(span_text(&app, case_errs[0]), failing, "{body}: span");
    }
}

#[test]
fn literal_when_patterns_still_emit_a_match() {
    let (article, errors, _) = emit_rust_with_probe(
        "case x\n    when 1, 2 then \"small\"\n    when 3 then \"three\"\n    else \"other\"\n    end",
    );
    assert!(
        case_rust_unsupported(&errors).is_empty(),
        "{errors:?}"
    );
    assert!(article.contains("3 => "), "{article}");
}
