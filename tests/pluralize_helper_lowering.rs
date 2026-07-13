//! Bare `pluralize(count, word)` in a helper body grounds to
//! `Inflector.pluralize` — the same count-labeling home the view
//! pipeline's classifier uses (spinel-blog convention), via
//! `apply_helper_lowering`'s framework-call rewrite. Two-arg form
//! only; the optional plural-word/locale variants stay verbatim
//! (honest residue) rather than mis-bind the runtime's arity.
//! Surfaced by lobsters' ApplicationHelper#errors_for
//! (`pluralize(object.errors.count, "error")`) refusing under spinel
//! AOT as an unresolvable bare call.

use roundhouse::ingest::ingest_app_from_tree;

fn emit_helper(src: &str) -> String {
    let files: Vec<(&str, &str)> = vec![("app/helpers/application_helper.rb", src)];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn two_arg_pluralize_grounds_to_inflector() {
    let out = emit_helper(
        r##"module ApplicationHelper
  def error_heading(errors)
    "#{pluralize(errors.count, "error")} prohibited this"
  end
end
"##,
    );
    assert!(
        out.contains("Inflector.pluralize("),
        "bare 2-arg pluralize must ground to Inflector:\n{out}"
    );
}

#[test]
fn three_arg_pluralize_stays_verbatim() {
    let out = emit_helper(
        r#"module ApplicationHelper
  def custom(n)
    pluralize(n, "person", "people")
  end
end
"#,
    );
    assert!(
        !out.contains("Inflector.pluralize("),
        "3-arg pluralize is outside the runtime's surface and must stay verbatim:\n{out}"
    );
}
